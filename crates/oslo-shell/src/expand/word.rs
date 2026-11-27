//! The expansion pipeline, and the provenance every later stage consults.
//!
//! A shell word is not a string. It is a sequence of *runs*, each of which remembers whether the
//! characters in it were quoted, typed literally, or produced by an expansion — because the three
//! answers lead to three different behaviours downstream:
//!
//! * field splitting acts on the result of an *unquoted expansion* and on nothing else, so
//!   `IFS=:; echo a:b:c` is one word while `IFS=:; v=a:b:c; echo $v` is three;
//! * pathname expansion reads `*` as a metacharacter only where the user did not quote it, so
//!   `echo "a"*` globs and `echo "a*"` does not;
//! * `"$@"` yields one field per positional parameter, which a single string cannot represent
//!   at all.
//!
//! Collapsing all of that into one `String` plus one word-level `is_quoted` flag was the single
//! defect behind five separate wrong answers, which is why the representation lives here rather
//! than being reconstructed by each consumer.

use crate::env::Environment;
use crate::expand::arithmetic::eval_arithmetic;
use crate::expand::fields::{ifs_of, split_field};
use crate::expand::glob::{NoMatch, expand_field};
use crate::expand::param::{expand_array_ref, expand_param};
use crate::expand::tilde::expand_tilde;
use oslo_base::ast::{ParamExpansion, Word, WordPart};
use oslo_base::error::{Result, ShellError};

/// Where the characters in a [`Run`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Unquoted text the script itself contains. Its metacharacters glob, but it is never
    /// field-split: POSIX splits expansion *results*, not the source text around them.
    Literal,
    /// Quoted or backslash-escaped text. Literal for pathname expansion, never field-split.
    Quoted,
    /// The output of an unquoted expansion. Field-split on IFS; whatever survives still globs.
    Expanded,
    /// An empty run opening a field that an assignment joins to the field before it on IFS's
    /// first character rather than a space. See [`crate::expand::join`].
    Seam,
    /// An empty run opening a field that every single-string context joins to the field before
    /// it with nothing in between. See [`crate::expand::join`].
    Glue,
}

impl Origin {
    /// Whether this is one of the empty marks [`crate::expand::join`] reads.
    pub fn is_mark(self) -> bool {
        matches!(self, Origin::Seam | Origin::Glue)
    }
}

/// A maximal stretch of expanded text sharing one [`Origin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub origin: Origin,
}

impl Run {
    pub fn new(text: impl Into<String>, origin: Origin) -> Self {
        Self {
            text: text.into(),
            origin,
        }
    }

    /// Whether pathname expansion may read this run's `*`, `?` and `[` as metacharacters.
    pub fn globs(&self) -> bool {
        self.origin != Origin::Quoted
    }

    /// Whether IFS field splitting may cut this run.
    pub fn splits(&self) -> bool {
        self.origin == Origin::Expanded
    }
}

/// One prospective word: the runs that concatenate into a single field.
pub type Field = Vec<Run>;

/// The text a field carries, with quoting forgotten.
pub fn field_text(field: &[Run]) -> String {
    field.iter().map(|r| r.text.as_str()).collect()
}

/// Accumulates parts into fields, splicing a multi-field part into its neighbours.
///
/// `pre"$@"post` over three positionals is `pre<1>`, `<2>`, `<3>post`: the first field joins what
/// came before it and the last stays open for what follows. `open: None` distinguishes "nothing
/// has contributed yet" from "an empty field so far", which is exactly what lets `"$@"` with no
/// positionals vanish while `""` survives as an empty argument.
#[derive(Default)]
struct FieldBuilder {
    done: Vec<Field>,
    open: Option<Field>,
}

impl FieldBuilder {
    fn push(&mut self, segments: Vec<Field>) {
        let mut segments = segments.into_iter();
        // No segments at all means the part contributed nothing *and* broke nothing: the word
        // `x"$@"y` with no positionals is still the single field `xy`.
        let Some(first) = segments.next() else {
            return;
        };
        let mut current = self.open.take().unwrap_or_default();
        current.extend(first);
        for segment in segments {
            self.done.push(std::mem::replace(&mut current, segment));
        }
        self.open = Some(current);
    }

    fn finish(mut self) -> Vec<Field> {
        if let Some(open) = self.open.take() {
            self.done.push(open);
        }
        self.done
    }
}

/// Expand `word` into fields, *before* IFS splitting and pathname expansion.
///
/// This is the representation later stages want: quoting is still attached to each run, so a
/// consumer can decide for itself which of the remaining steps apply.
pub fn expand_word_fields(env: &mut Environment, word: &Word) -> Result<Vec<Field>> {
    expand_word_fields_in(env, word, false)
}

/// Expand `word` into fields inside an enclosing quoting context.
///
/// Only `${x-word}` and `${x+word}` need the `in_quotes` argument: their payload is expanded
/// where the `${…}` stands, so `"${1+"$@"}"` has to see that it is inside quotes even though the
/// payload's own `"$@"` carries quotes of its own.
pub fn expand_word_fields_in(
    env: &mut Environment,
    word: &Word,
    in_quotes: bool,
) -> Result<Vec<Field>> {
    let mut builder = FieldBuilder::default();
    for part in &word.parts {
        let segments = expand_word_part(env, part, in_quotes)?;
        builder.push(segments);
    }
    Ok(builder.finish())
}

/// Expand one word part into the fields it contributes.
///
/// Almost always exactly one field. `"$@"` is the exception that forces the return type: it is one
/// field per positional parameter, and *no field at all* when there are none — which is how
/// `cmd "$@"` with no arguments manages to run `cmd` rather than `cmd ""`.
///
/// `in_quotes` is the enclosing double-quote context, which decides whether an expansion's output
/// is splittable and whether literal text globs.
pub fn expand_word_part(
    env: &mut Environment,
    part: &WordPart,
    in_quotes: bool,
) -> Result<Vec<Field>> {
    let typed = if in_quotes {
        Origin::Quoted
    } else {
        Origin::Literal
    };
    let produced = if in_quotes {
        Origin::Quoted
    } else {
        Origin::Expanded
    };

    let single = |text: String, origin: Origin| vec![vec![Run::new(text, origin)]];

    Ok(match part {
        WordPart::Literal(s) => single(s.clone(), typed),
        // `\*` is a literal asterisk and `a\ b` is one field: an escaped character is quoted in
        // every sense that matters after the lexer.
        WordPart::Escaped(s) => single(s.clone(), Origin::Quoted),
        WordPart::SingleQuoted(s) => single(s.clone(), Origin::Quoted),
        WordPart::DoubleQuoted(parts) => {
            if parts.is_empty() {
                // `""` is an explicit empty field, not the absence of one.
                single(String::new(), Origin::Quoted)
            } else {
                let mut builder = FieldBuilder::default();
                for inner in parts {
                    let segments = expand_word_part(env, inner, true)?;
                    builder.push(segments);
                }
                builder.finish()
            }
        }
        // A home directory that happens to contain a glob character is still just a directory.
        WordPart::Tilde(user) => single(expand_tilde(env, user), Origin::Quoted),
        WordPart::Variable {
            name,
            expansion_type,
        } => {
            check_nounset(env, name, expansion_type)?;
            expand_param(env, name, expansion_type, in_quotes)?
        }
        // `"${a[@]}"` is the one other part that can be several fields, or none. Its own module
        // enforces `set -u`, because "unset" for an element means an index that was never
        // assigned rather than a name that does not exist.
        WordPart::ArrayRef {
            name,
            subscript,
            expansion_type,
        } => expand_array_ref(env, name, subscript, expansion_type, in_quotes)?,
        WordPart::Arithmetic(expr) => single(eval_arithmetic(env, expr)?.to_string(), produced),
        WordPart::ProcessSubstitution {
            reads_from_command,
            command,
        } => {
            // The path is an ordinary word from here on: it globs like one and splits like one,
            // which is what lets `cat <(echo hi)` pass it as a plain argument.
            let (path, handle) = crate::exec::procsub::open(env, command, *reads_from_command)?;
            env.procsubs.push(handle);
            single(path, produced)
        }
        WordPart::CommandSubstitution(cmd) => {
            let output = crate::exec::eval_command_substitution(env, cmd)?;

            // A substitution is a command that ran, so `$?` *later in the same word* reports it:
            // `echo "[$(exit 7)] $?"` prints `[] 7` in bash, because the word's expansions happen
            // left to right and each substitution updates the status as it finishes. oslo recorded
            // the status but never published it, so the `$?` beside it read whatever the previous
            // *command* left — always the wrong number, and silently so.
            //
            // Not under `--posix`: bash 5.3 stopped doing it there, leaving `$?` as the previous
            // command's, and oslo follows the newer answer. `tests/corpus/command_v_keywords.sh`
            // is the case that separates the two, and is gated on 5.3 for exactly this reason.
            if !env.posix()
                && let Some(status) = env.peek_substitution_status()
            {
                env.last_status = status;
            }

            // Trailing newlines are stripped per POSIX.
            single(output.trim_end_matches('\n').to_string(), produced)
        }
    })
}

/// `set -u`: refuse to expand a parameter that was never set.
///
/// The point of the option is to turn a typo into a diagnostic instead of an empty string, so the
/// check has to happen *before* [`expand_param`] hands back `""`. Three families are exempt, and
/// each for a reason a script relies on:
///
/// * `${x-d}`, `${x:=d}`, `${x+alt}`, `${x?msg}` — every operator whose whole job is to say what
///   an unset parameter means. `${x-default}` under `set -u` is the standard way to *read* a
///   possibly-unset variable, and erroring on it would leave no way to do so at all.
/// * `$@` and `$*` with no positional parameters. bash stopped treating an empty argument list as
///   unset in 4.4, and `set -u; f() { echo "$@"; }` is far too common to break.
/// * a parameter that is set but empty. `nounset` tests for *unset*, not for null; that is the
///   difference between `${x-d}` and `${x:-d}`, and it applies here too.
fn check_nounset(env: &Environment, name: &str, expansion_type: &ParamExpansion) -> Result<()> {
    if !env.nounset() || matches!(name, "@" | "*") {
        return Ok(());
    }
    // An indirection checks the name it reaches, not the pointer; see `operators::apply`.
    if matches!(
        expansion_type,
        ParamExpansion::DefaultValue { .. }
            | ParamExpansion::UseAlternative { .. }
            | ParamExpansion::ErrorIfUnset { .. }
            | ParamExpansion::Indirect(_)
    ) {
        return Ok(());
    }
    if env.get_param(name).is_some() {
        return Ok(());
    }
    // bash names a positional parameter with its `$`, an ordinary variable without one, and
    // scripts grep for the "unbound variable" wording.
    let subject = if name.chars().all(|c| c.is_ascii_digit()) {
        format!("${name}")
    } else {
        name.to_string()
    };
    Err(ShellError::UnsetParameter(format!(
        "{subject}: unbound variable"
    )))
}

/// Expand to exactly one string, skipping field splitting and globbing.
///
/// This is what `case` needs for both the subject and its patterns. POSIX excludes both of the
/// latter steps there, and applying them is actively wrong: globbing a pattern turns
/// `case foo in f*)` into a match against whatever files happen to be in the working directory,
/// so the branch silently stops firing depending on where you run the script.
pub fn expand_word_to_string(env: &mut Environment, word: &Word) -> Result<String> {
    expand_word_joined(env, word, crate::expand::join::Joining::Value)
}

/// [`expand_word_to_string`] for a `[[ ]]` operand or a `case` word, which join a list
/// differently from an assignment; see [`crate::expand::join`].
pub fn expand_word_to_test_string(env: &mut Environment, word: &Word) -> Result<String> {
    expand_word_joined(env, word, crate::expand::join::Joining::Test)
}

fn expand_word_joined(
    env: &mut Environment,
    word: &Word,
    joining: crate::expand::join::Joining,
) -> Result<String> {
    let fields = expand_word_fields(env, word)?;
    let fields =
        crate::expand::sugar::marked_fields(env, fields).map_err(ShellError::ExpansionError)?;
    if fields.len() == 1 {
        return Ok(field_text(&fields[0]));
    }
    // Only a list expansion reaches here with more than one field.
    Ok(crate::expand::join::join_fields(env, &fields, joining))
}

/// Expand a word that is going to be used as a *pattern*, keeping its quoting.
///
/// The same expansions as [`expand_word_to_string`], but the result is runs rather than a flat
/// string, because for a pattern the quoting is not decoration — it decides per character whether
/// a `*` is a metacharacter. Flattening first is how `case $answer in "$expected")` came to match
/// anything at all when `$expected` happened to contain a `*`.
///
/// Several fields can only arise from `$@`; they are concatenated, which is the same text a
/// context insisting on one string would have got.
pub fn expand_word_to_pattern(env: &mut Environment, word: &Word) -> Result<Vec<Run>> {
    let fields = expand_word_fields(env, word)?;
    let fields =
        crate::expand::sugar::marked_fields(env, fields).map_err(ShellError::ExpansionError)?;
    Ok(crate::expand::join::join_runs(env, fields))
}

/// Full expansion of one word: parameters, substitutions, field splitting, then pathname
/// expansion — the argument list a command actually receives.
///
/// Brace expansion is not here, and deliberately so. It is the one expansion that yields whole
/// *words* rather than fields, and matching bash means running it on the word's source text
/// before the word is lexed at all — so it lives in `oslo_base::brace` and is applied by
/// the parser, once per word, at the positions where bash applies it. By the time a [`Word`]
/// reaches this function its groups are already gone.
pub fn expand_word(env: &mut Environment, word: &Word) -> Result<Vec<String>> {
    expand_word_at(env, word, Place::Argument)
}

/// Where in a command a word sits, which only the shorthands care about.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The command name.
    Command,
    /// Everything after it.
    Argument,
}

/// The command word, which the shorthands leave alone.
///
/// **The start of a line is reserved.** A leading symbol is being kept for something else, so
/// `@proj` typed on its own must not quietly become a path — it did, and then failed with
/// `Is a directory`, which is the position spoken for by an error message nobody chose.
pub fn expand_command_word(env: &mut Environment, word: &Word) -> Result<Vec<String>> {
    expand_word_at(env, word, Place::Command)
}

fn expand_word_at(env: &mut Environment, word: &Word, place: Place) -> Result<Vec<String>> {
    let posix_word;
    let word = if env.posix() && has_assignment_tilde(word) {
        posix_word = without_assignment_tildes(word);
        &posix_word
    } else {
        word
    };
    let mut out = Vec::new();
    // `set -f` switches pathname expansion off wholesale, so the field's own text is the answer.
    // Read once per word rather than per field: an expansion cannot change the option mid-word.
    let glob = !env.noglob();
    let fields = expand_word_fields(env, word)?;
    let ifs = ifs_of(env);
    let globignore = env.get_var("GLOBIGNORE").map(str::to_string);
    for field in fields {
        // **`@name` is substituted here, where a tilde is, and for the same reason.** It names a
        // directory, so the glob that follows it is the user's own and has to run: `@proj/*.rs` was
        // reaching the command with a literal `*` while `~/*.rs` and `$M/*.rs` both expanded. Done
        // before the split and the glob, and only when the word is an argument.
        let field = match place {
            Place::Argument if env.interactive() => {
                crate::expand::sugar::marked_directory(field).map_err(ShellError::ExpansionError)?
            }
            _ => field,
        };
        // **`=command` is substituted here too, and for the third of the same reasons.** It has to
        // see the field's *origin* — `echo "=ls"` is a literal and must stay one — and the origin
        // is gone by the time the field is a `String`. What it answers with is marked quoted, so
        // the path is still not split or globbed afterwards.
        let field =
            crate::expand::sugar::equals_field(env, field).map_err(ShellError::ExpansionError)?;
        for split in split_field(ifs, field) {
            if glob {
                crate::expand::glob::refuse_extglob(&split)?;
                // `failglob` is bash's `no match: zz*`: the command does not run, status 1.
                let words = expand_field(&split, globignore.as_deref())
                    .map_err(|NoMatch(text)| ShellError::NoMatch(text))?;
                out.extend(words);
            } else {
                out.push(field_text(&split));
            }
        }
    }
    Ok(out)
}

/// Whether `word` has a tilde past its start, which only a `name=…` word's value can open.
fn has_assignment_tilde(word: &Word) -> bool {
    word.parts
        .iter()
        .skip(1)
        .any(|part| matches!(part, WordPart::Tilde(_)))
}

/// `word` with every tilde past its start made literal, which is POSIX's rule for an ordinary
/// argument shaped like an assignment: `set -o posix; echo a=~` prints `a=~`.
fn without_assignment_tildes(word: &Word) -> Word {
    let parts = word.parts.iter().enumerate().map(|(i, part)| match part {
        WordPart::Tilde(user) if i > 0 => WordPart::Literal(format!("~{user}")),
        other => other.clone(),
    });
    Word {
        parts: parts.collect(),
    }
}

#[cfg(test)]
#[path = "word/tests.rs"]
mod tests;

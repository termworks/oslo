//! `[[ ... ]]` conversion.
//!
//! oslo has no dedicated node for extended tests, so the expression tree is lowered onto
//! constructs it already evaluates: `&&`/`||` become an and-or list, `!` becomes a negated
//! pipeline, and each leaf predicate becomes a call to the `[[` builtin.
//!
//! Everything here works on a word's source text, so it knows nothing about the parser that
//! produced the expression — only about what shell means by it.

use super::words::{convert_words_from_str, single_command, single_word_from_str};
use oslo_base::ast as oslo_ast;
use oslo_base::error::Result;

/// Wrap an and-or list up as one command, which is what a `[[ ]]` is in the end.
pub(crate) fn as_command(list: oslo_ast::AndOrList) -> oslo_ast::Command {
    single_command(list)
}

/// `a && b` or `a || b`, joining two lowered tests.
pub(crate) fn join(
    mut left: oslo_ast::AndOrList,
    op: oslo_ast::AndOrOp,
    right: oslo_ast::AndOrList,
) -> oslo_ast::AndOrList {
    left.rest.push((op, right.first));
    left.rest.extend(right.rest);
    left
}

/// The word a negated predicate carries after `[[`, as `[[ ! -n x ]]` is traced.
pub const NEGATION: &str = "\u{1}!\u{2}";

/// `! a`.
///
/// A single predicate is negated inside its own call, which toggles a [`NEGATION`] word; a
/// compound expression is grouped first so `! (a && b)` does not silently become `(! a) && b`.
pub(crate) fn negate(list: oslo_ast::AndOrList) -> oslo_ast::AndOrList {
    if list.rest.is_empty() {
        let mut list = list;
        if let [oslo_ast::Command::Simple(simple)] = list.first.commands.as_mut_slice()
            && !list.first.negated
        {
            let marked = simple.words.get(1) == Some(&oslo_ast::Word::from_literal(NEGATION));
            match marked {
                true => drop(simple.words.remove(1)),
                false => simple
                    .words
                    .insert(1, oslo_ast::Word::from_literal(NEGATION)),
            }
            return list;
        }
        list.first.negated = !list.first.negated;
        return list;
    }
    let grouped = oslo_ast::Command::Compound {
        kind: oslo_ast::CompoundCommand::Group(oslo_ast::CommandList {
            items: vec![oslo_ast::ListItem {
                and_or: list,
                op: oslo_ast::ListOp::Sequential,
                line: 0,
            }],
        }),
        redirections: Vec::new(),
    };
    oslo_ast::AndOrList {
        first: oslo_ast::Pipeline {
            negated: true,
            timed: false,
            commands: vec![grouped],
        },
        rest: Vec::new(),
    }
}

/// `-f x` — an operator and one operand.
///
/// The name `-v` tests is expanded as an arithmetic expression is, so an escaped `$` reaches the
/// subscript still escaped: `[[ -v a[\$i] ]]` evaluates `$i` and not `i`'s value.
pub(crate) fn unary(op: &str, operand_text: &str) -> Result<oslo_ast::AndOrList> {
    let word = match op {
        "-v" => operand(&keep_escapes(operand_text), Coordinates::Substituted)?,
        _ => operand(operand_text, Coordinates::Substituted)?,
    };
    Ok(bracket_and_or(vec![oslo_ast::Word::from_literal(op), word]))
}

/// `text` with each backslash before `$`, `` ` `` or `\` outside single quotes doubled, so quote
/// removal leaves that escape in place.
fn keep_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut chars, mut single, mut double) = (text.chars(), false, false);
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '\\' if !single => match chars.next() {
                Some(next @ ('$' | '`' | '\\')) => {
                    out.push_str("\\\\\\");
                    out.push(next);
                    continue;
                }
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                    continue;
                }
                None => {}
            },
            _ => {}
        }
        out.push(ch);
    }
    out
}

/// `x == y` — two operands with an operator between them, the operator as written.
///
/// The right operand of a pattern or regex operator is where a *part* of a word can be quoted and
/// mean something different from the rest of it: `[[ abc == "a*"c ]]` is false, because only the
/// `c` is a pattern. See [`mark_quoted_runs`].
pub(crate) fn binary(left: &str, op: &str, right: &str) -> Result<oslo_ast::AndOrList> {
    let right = match op {
        "=~" => marked_operand(right, Coordinates::Literal)?,
        "==" | "=" | "!=" => marked_operand(right, Coordinates::Substituted)?,
        _ => operand(right, Coordinates::Substituted)?,
    };
    Ok(bracket_and_or(vec![
        operand(left, Coordinates::Substituted)?,
        oslo_ast::Word::from_literal(op),
        right,
    ]))
}

/// `[[ $x ]]` — a word on its own, which is true when it is not empty.
pub(crate) fn bare(text: &str) -> Result<oslo_ast::AndOrList> {
    unary("-n", text)
}

/// Whether a bare stream coordinate in this operand is left for the substitution to find.
///
/// **The right operand of `=~` is a regex, and a regex already owns `{}`.** `{4}` is a repeat
/// count there, not line 4, and `^([0-9]{4})-([0-9]{2})` parses as a coordinate perfectly well —
/// so leaving it bare fed the quantifiers to the stream substitution, which resolved them against
/// nothing and handed the matcher `^([0-9])-([0-9])`. That is the worst shape a bug can take: the
/// match still *succeeds* on a short string, so `[[ 20 =~ ^[0-9]{4} ]]` was true.
///
/// Nowhere else has this problem, because brace expansion runs on a word's source text before the
/// lexer sees it — by the time there is a syntax tree, an ordinary command word has already become
/// its several words and has no braces left to mistake. The positions that still hold a literal
/// brace are exactly the ones bash refuses to brace-expand, and a regex is one of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Coordinates {
    Substituted,
    Literal,
}

/// One operand of a predicate, in a form that cannot become anything other than one word.
///
/// This is the difference between `[[ ]]` and `[ ]` that the lowering would otherwise lose. `[[`
/// is a *syntactic* construct: its operands are not field-split and not pathname-expanded, so
/// `[[ $x == "a b" ]]` is a comparison and `[[ -n $x ]]` with an empty `x` is still a test on one
/// (empty) operand. Lowered to an ordinary command, they were expanded like ordinary arguments —
/// so a value with a space became two operands (`too many arguments`), a value containing `*` was
/// globbed against the working directory, and an empty value vanished, shifting the operator into
/// the operand slot.
///
/// Wrapping the whole word in double quotes says exactly that, and reuses the expansion rules
/// already written rather than adding a second set. It does not make the `==` right-hand side
/// literal: which parts of a pattern were quoted is carried by [`marked_operand`].
fn operand(word: &str, coordinates: Coordinates) -> Result<oslo_ast::Word> {
    if let Some(substitution) = process_substitution(word) {
        return Ok(substitution);
    }
    let inner = single_word_from_str(word)?;
    if let Some(expanding) = relex_inside_quotes(word, &inner) {
        return Ok(expanding);
    }
    // **A bare `@name` is left unwrapped**, because the quotes would be indistinguishable from
    // quotes the user typed and `@name` is deliberately literal inside those. `[ -d @proj ]` was
    // true and `[[ -d @proj ]]` false — the same test written two ways disagreeing — and the
    // shell's own error message gave it away by echoing back `[[ -d "@proj" ]]`.
    //
    // Safe to leave bare: the substitution hands back the resolved path as an already-quoted run,
    // so it cannot split or glob however many spaces are in it, and a name that resolves to
    // nothing keeps its own text. One literal part only, so nothing here can expand to a field
    // this was wrapping to protect.
    //
    // **A stream coordinate is left bare for exactly the same reason**, and it is the same bug
    // written twice: `test {0:0} = alpha` was true and `[[ {0:0} == alpha ]]` false, because the
    // wrapping hid the coordinate from the substitution that runs over literal words. It is safe
    // to leave bare on the same grounds — a substituted value arrives already quoted and cannot
    // split or glob however many spaces are in it. `@(a|b)` is an `extglob` group, not a name.
    if let [oslo_ast::WordPart::Literal(text)] = inner.parts.as_slice()
        && ((text.starts_with('@') && !text.starts_with("@("))
            || (coordinates == Coordinates::Substituted
                && crate::exec::streams::holds_a_coordinate(text)))
    {
        return Ok(inner);
    }
    Ok(oslo_ast::Word {
        parts: vec![oslo_ast::WordPart::DoubleQuoted(inner.parts)],
    })
}

/// An operand that is a whole `<(cmd)` or `>(cmd)`, as the pipe's path: `[[ -e <(true) ]]`.
fn process_substitution(word: &str) -> Option<oslo_ast::Word> {
    let word = word.trim();
    let reads_from_command = word.starts_with("<(");
    let command = word
        .strip_prefix("<(")
        .or_else(|| word.strip_prefix(">("))?
        .strip_suffix(')')?;
    Some(oslo_ast::Word {
        parts: vec![oslo_ast::WordPart::ProcessSubstitution {
            reads_from_command,
            command: command.to_string(),
        }],
    })
}

/// Build a `[[ <args> ]]` invocation as a one-pipeline and-or list.
fn bracket_and_or(args: Vec<oslo_ast::Word>) -> oslo_ast::AndOrList {
    let mut words = vec![oslo_ast::Word::from_literal("[[")];
    words.extend(args);
    words.push(oslo_ast::Word::from_literal("]]"));

    oslo_ast::AndOrList {
        first: oslo_ast::Pipeline {
            negated: false,
            timed: false,
            commands: vec![oslo_ast::Command::Simple(oslo_ast::SimpleCommand {
                assignments: Vec::new(),
                words,
                redirections: Vec::new(),
                unbraced: None,
            })],
        },
        rest: Vec::new(),
    }
}

/// The pair of bytes the adapter puts around a **quoted run** inside an unquoted regex operand.
///
/// **bash's rule is per part, not per operand.** `[[ a =~ ^"$R"$ ]]` anchors with the `^` and `$`
/// it was given and matches `$R`'s value *literally* — quoting turns the metacharacters off for
/// the piece it covers and nothing else. oslo decided quoting for the whole word, so a mixed
/// operand was read as entirely unquoted: with `R="a|b"` the pattern became `^a|b$`, which matches
/// `a`, where bash matches only the two characters `a|b`. Wrong rather than refused, and silent.
///
/// The adapter cannot escape the quoted text itself — it is `$R`, whose value is not known until
/// expansion — so it marks where the quoting was and this module escapes what arrives between the
/// marks. Control characters no terminal sends and no regex means, so text carrying one of its own
/// is not a case anybody can reach by accident.
pub const QUOTED_OPEN: char = '\u{1}';
pub const QUOTED_CLOSE: char = '\u{2}';

/// The `=~` right operand, with its quoted runs marked for the matcher.
///
/// **bash decides quoting per part.** `[[ a =~ ^"$R"$ ]]` keeps the `^` and `$` it was given as
/// anchors and matches `$R`'s value literally; oslo decided for the whole word, so with
/// `R="a|b"` the pattern became `^a|b$` — which matches `a`, where bash matches only `a|b`.
///
/// The escaping cannot happen here: the quoted part is `$R`, and its value is not known until the
/// word is expanded. So the boundaries are marked instead and
/// `conditionals::matching::eval_regex_match` escapes what arrives between them. A word with no
/// quoted part gets no marks and travels exactly as it did.
fn marked_operand(word: &str, coordinates: Coordinates) -> Result<oslo_ast::Word> {
    let plain = operand(word, coordinates)?;
    let marked = mark_quoted_runs(&plain);
    Ok(marked.unwrap_or(plain))
}

/// Wrap every quoted part of `word` in the matcher's markers, or `None` if it has none.
fn mark_quoted_runs(word: &oslo_ast::Word) -> Option<oslo_ast::Word> {
    use oslo_ast::WordPart;

    // `operand` has already wrapped the whole word in one `DoubleQuoted`, which is how `[[ ]]`
    // suppresses field splitting and globbing; the parts that were quoted *in the source* are the
    // ones inside it.
    let [WordPart::DoubleQuoted(inner)] = word.parts.as_slice() else {
        return None;
    };
    if !inner.iter().any(is_source_quoted) {
        return None;
    }
    let mut parts = Vec::with_capacity(inner.len());
    for part in inner {
        match is_source_quoted(part) {
            true => {
                parts.push(WordPart::Literal(QUOTED_OPEN.to_string()));
                parts.push(part.clone());
                parts.push(WordPart::Literal(QUOTED_CLOSE.to_string()));
            }
            false => parts.push(part.clone()),
        }
    }
    Some(oslo_ast::Word {
        parts: vec![WordPart::DoubleQuoted(parts)],
    })
}

/// Whether this part was quoted where it was written, and so is literal text to a regex.
fn is_source_quoted(part: &oslo_ast::WordPart) -> bool {
    matches!(
        part,
        oslo_ast::WordPart::DoubleQuoted(_)
            | oslo_ast::WordPart::SingleQuoted(_)
            | oslo_ast::WordPart::Escaped(_)
    )
}

/// Re-lex an operand that the plain word lexer had to give up on, as double-quoted text.
///
/// **This is a regex with a variable in it, and it was reaching the matcher unexpanded.**
///
/// ```text
/// R="a|b"; [[ a =~ ^($R)$ ]]     bash: match      oslo, before this: no match
/// R="a|b"; [[ a =~ (${R}) ]]     bash: match      oslo: invalid regular expression `(${R})'
/// ```
///
/// The cause is one line in [`super::words::convert_words_from_str`]: the raw text is re-lexed
/// with the *shell's* lexer, `(` is a shell operator, and a word that turns out not to be a plain
/// word falls back to `Word::from_literal` — a part nothing ever expands. A bare `$R` was fine,
/// because a bare `$R` lexes cleanly; putting it in a group broke it, and putting a regex in a
/// group is what people do.
///
/// Re-lexing inside double quotes is the fix, and it is not a trick: it is exactly the rule
/// `[[ ]]` operands already follow. [`operand`] wraps the result in `DoubleQuoted` anyway, because
/// these words are not field-split or globbed — so `(` was always going to be an ordinary
/// character here, and the only thing the quotes change is that `$R` now expands, which is what
/// bash does with an *unquoted* regex operand. A quoted one is escaped later by
/// `matching::eval_regex_match` and is unaffected.
///
/// Answers `None` when there was nothing to recover: the word lexed normally, or it carries no
/// expansion, or the second lex fails too — and then the old literal stands, as it did.
fn relex_inside_quotes(raw: &str, lexed: &oslo_ast::Word) -> Option<oslo_ast::Word> {
    let raw = raw.trim();
    // The fallback's fingerprint: one literal part holding the whole word, untouched.
    let gave_up = matches!(
        lexed.parts.as_slice(),
        [oslo_ast::WordPart::Literal(text)] if text == raw
    );
    // Nothing to recover unless something in there wanted to expand.
    if !gave_up || !raw.contains(['$', '`']) || raw.contains('"') {
        return None;
    }
    let quoted = format!("\"{raw}\"");
    let mut words = convert_words_from_str(&quoted).ok()?;
    match words.len() {
        1 => Some(words.remove(0)),
        _ => None,
    }
}

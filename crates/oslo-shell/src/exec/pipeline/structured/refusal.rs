//! Refusing a column the stream cannot have, before anything runs.

use super::*;

/// The operands of `name` that must name a column the stream **already has**.
///
/// Only these. A verb that *creates* a column (`insert`, `default`) names one that is supposed to be
/// absent, and a verb whose operand is an expression (`where`, `map`, `reduce`) names no column at
/// all — checking either would refuse working pipelines, which is the one thing this must not do.
fn column_operands<'a>(name: &str, words: &'a [String]) -> Vec<&'a str> {
    let rest = || words.iter().skip(1).map(String::as_str);
    let first = || words.get(1).map(String::as_str).into_iter().collect();
    match name {
        // Every operand is a column.
        "cols" | "reject" => rest().collect(),
        // Flags first, then keys; `--` ends them, as `sort_operands` reads it.
        "sort-by" => {
            let mut done = false;
            rest()
                .filter(|word| {
                    if done || !word.starts_with('-') || *word == "-" {
                        return true;
                    }
                    done |= *word == "--";
                    false
                })
                .collect()
        }
        // The first operand, and only it.
        "get" | "group-by" | "stats" | "histogram" | "update" => first(),
        // Optional: absent means "by the whole row", which names nothing.
        "distinct" | "compact" => first(),
        // The old name has to be there; the new one must not be.
        "rename" => first(),
        // The key, which sits after the Lua expression — and after `--keep` when it is given.
        "lookup" => {
            let at = if words.get(1).is_some_and(|w| w == "--keep") {
                3
            } else {
                2
            };
            words.get(at).map(String::as_str).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// The operand of `name` that must name a column the stream does **not** already have.
///
/// The mirror of [`column_operands`], and only `insert` has one: it exists to add a column, so a
/// name the stream already carries is nearly always a typo for `update`. `upsert` is deliberately
/// absent — refusing nothing is the whole of what makes it `upsert`.
///
/// **A plain name only.** `insert a.b` writes a field *inside* the column `a`, and whether `a.b` is
/// already there is a question about the data rather than about the column set — which is exactly
/// what this pass cannot see. Left to `assign`, which can.
fn created_operand<'a>(name: &str, words: &'a [String]) -> Option<&'a str> {
    if name != "insert" {
        return None;
    }
    words
        .get(1)
        .map(String::as_str)
        .filter(|column| !column.contains('.'))
}

/// A refusal, with everything a caret needs to be drawn under the word that caused it.
///
/// A `String` was enough while the answer was one line. It is not enough now: the report has to
/// point at *which* word, and it has to be able to say what would have been right — which is known
/// here, at plan time, and nowhere later. Carrying the words out is cheaper than teaching the
/// printer to re-derive them.
pub(super) struct Refusal {
    /// The stage's words, as the source line to point into.
    pub words: Vec<String>,
    /// The one at fault.
    pub word: String,
    /// The one-liner, byte for byte what this used to return.
    pub message: String,
    pub label: &'static str,
    pub help: Option<String>,
}

/// Refuse a column no stage can be carrying, **before any stage runs**.
///
/// This is `data::plan`'s question asked one level down: the pipe already decides what shape crosses
/// an edge, and now it decides whether what a stage names is in it. `ls | cols nmae` used to run
/// `ls`, build the rows, and only then have `tools::unknown_column` scan them — harmless for `ls`,
/// and not harmless at all for a tool a config registered that does something on the way.
///
/// **It may only refuse what it is sure of.** A column set derived from data is
/// [`Columns::Unknown`](crate::data::columns::Columns::Unknown) and refuses nothing; an operand that
/// is not a plain literal is not read, by the same rule [`simple_command_name`] follows. Everything
/// this cannot see is still caught by `unknown_column` when the rows exist.
pub(super) fn refuse_unknown_column(env: &Environment, pipeline: &Pipeline) -> Option<Refusal> {
    use crate::data::columns::{Columns, through};
    let mut columns = Columns::Unknown;
    for command in &pipeline.commands {
        let Command::Simple(simple) = command else {
            columns = Columns::Unknown;
            continue;
        };
        let Some(name) = simple_command_name(env, simple) else {
            columns = Columns::Unknown;
            continue;
        };
        if crate::data::tool::lookup(&name).is_none() {
            // An external in the middle: whatever it prints, nothing here knows its columns.
            columns = Columns::Unknown;
            continue;
        }
        // A word that comes out of an expansion is not known until it runs, so it is not judged.
        let Some(words) = literal_words(simple) else {
            columns = Columns::Unknown;
            continue;
        };
        for wanted in column_operands(&name, &words) {
            if !columns.accepts(wanted) {
                return Some(Refusal {
                    message: format!("{name}: {wanted}: no such column"),
                    word: wanted.to_string(),
                    words,
                    label: "no column of that name",
                    // The set is `Known` or `accepts` would not have refused, so there is always a
                    // list to give — which is the whole argument for refusing at plan time rather
                    // than emitting empty rows and letting the reader work it out.
                    help: columns.names().map(carried),
                });
            }
        }
        // The same words `assign` would answer, one stage earlier. Identical wording on purpose:
        // the two checks differ in *when* they notice, never in what they say.
        if let Some(made) = created_operand(&name, &words)
            && columns
                .names()
                .is_some_and(|have| have.iter().any(|c| c == made))
        {
            return Some(Refusal {
                message: format!(
                    "{name}: {made}: already a column; use update to replace it, or upsert for either"
                ),
                word: made.to_string(),
                words,
                label: "already here",
                // The message already says what to do instead, and a help line repeating it is a
                // keyword in front of the same sentence.
                help: None,
            });
        }
        columns = through(&name, &words, &columns);
    }
    None
}

/// One part of a word, when it is the same text however the shell is feeling.
///
/// **Quoting is not expansion**, and conflating the two is what made this pass nearly inert. It
/// used to accept a bare `Literal` and nothing else, so `where 'size > 100'` — the spelling every
/// example in the docs uses — was "not known", which skipped the check for that stage *and* left the
/// column set `Unknown` for every stage after it. `ls | where 'true' | cols nmae | length` therefore
/// ran the whole pipeline and answered 0, where the same line without the quotes refused before
/// anything started.
///
/// A variable, a substitution or an arithmetic expansion is genuinely unknowable here and still
/// answers `None`.
pub(super) fn constant(part: &WordPart) -> Option<String> {
    match part {
        WordPart::Literal(text) | WordPart::SingleQuoted(text) | WordPart::Escaped(text) => {
            Some(text.clone())
        }
        WordPart::DoubleQuoted(parts) => parts.iter().map(constant).collect::<Option<String>>(),
        _ => None,
    }
}

/// Every word of a simple command as a constant, or `None` if any of them is not.
///
/// All or nothing: a command with one expanded word has operands at unknown positions, so reading
/// the rest of them would be reading the wrong ones.
pub(super) fn literal_words(simple: &SimpleCommand) -> Option<Vec<String>> {
    simple
        .words
        .iter()
        .map(|word| word.parts.iter().map(constant).collect::<Option<String>>())
        .collect()
}

/// A structured verb in the middle of a pipeline that redirects its own stdout.
///
/// **What used to happen was the worst of the three possibilities.** The planner forces text on such
/// a stage, which leaves no structured edge, so the whole line fell to the byte path — where the
/// verbs are not commands at all. `ls | first 2 > mid.txt | cat` answered `first: command not found`
/// and *created an empty `mid.txt`* on the way, because the byte path applies a redirection before
/// discovering there is nothing to run. A diagnostic naming the wrong problem, and a side effect for
/// a pipeline that never ran.
///
/// Refused here instead, before anything is applied. Only a *middle* stage: a redirection on the
/// last one is the ordinary case and `run` applies it. And only stdout — `2>/dev/null` leaves the
/// rows alone and is applied around its own stage.
///
/// Rows cross in memory rather than on a descriptor, so a verb whose output went to a file would
/// leave the next stage with nothing to read. Saying so is better than either half-doing it or
/// pretending the name does not exist.
pub(crate) fn refuse_redirected_middle(env: &Environment, pipeline: &Pipeline) -> Option<String> {
    let last = pipeline.commands.len().checked_sub(1)?;
    for command in &pipeline.commands[..last] {
        let Command::Simple(simple) = command else {
            continue;
        };
        let Some(name) = super::simple_command_name(env, simple) else {
            continue;
        };
        if crate::data::tool::lookup(&name).is_none() {
            continue;
        }
        if simple.redirections.iter().any(super::redirects_stdout) {
            return Some(format!(
                "{name}: a verb in the middle of a pipeline cannot redirect its output: \
                 its rows go to the next stage, which would then have nothing to read. \
                 Redirect the last stage instead."
            ));
        }
    }
    None
}

/// The columns a stage is carrying, for the help line under a refused name.
///
/// Capped, because a wide stream has forty of them and a help line that wraps three times pushes
/// the caret off the screen — which is the one thing the report exists to show.
fn carried(have: &[String]) -> String {
    let shown = have.len().min(12);
    let more = match have.len() > shown {
        true => format!(", and {} more", have.len() - shown),
        false => String::new(),
    };
    format!("the columns here are: {}{more}", have[..shown].join(", "))
}

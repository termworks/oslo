//! Saying a diagnostic once, in whichever of its two faces the reader can use.
//!
//! `origin_now` answers *where* a diagnostic is speaking from. This answers
//! *how* it is drawn: a one-line message to a pipe, and on a terminal the same message with a caret
//! under the word at fault.
//!
//! ```text
//! oslo: kill: NOPE: invalid signal specification      ← a pipe, a script, a test
//!
//! oslo: kill: NOPE: invalid signal specification      ← a terminal
//!    ╭─[ kill:1:9 ]
//!  1 │ kill -s NOPE 1
//!    │         ──┬─
//!    │           ╰─── not a signal
//!    │ Help: a signal is a name (TERM), a number (15), or SIG-prefixed
//! ───╯
//! ```
//!
//! # One call, not five
//!
//! There are two hundred and fifty diagnostics in the builtins alone, and every one of them is
//! today a single `eprintln!`. If converting one meant five lines — ask whether to draw, build a
//! snapshot, find the word, build a report, fall back — then converting them all would be a
//! thousand lines of the same five, and the two hundred and fifty-first would be written the old
//! way because the new way is a chore.
//!
//! So it is one call with the same shape as the `eprintln!` it replaces, and the fallback is inside
//! it. A caller that has nothing to point at keeps its `eprintln!`; that is a decision about the
//! error, not an omission.
//!
//! # What `body` is
//!
//! Everything after the origin — `kill: NOPE: invalid signal specification` — exactly as the
//! `eprintln!` wrote it. **The message a pipe sees is byte-for-byte what it saw before**, which is
//! what `tests/diagnostics_stay_plain.rs` exists to hold true, and it is also the report's own
//! first line, so the two faces cannot drift into saying different things.

use super::origin_now;

/// Pointing into a script or a Lua chunk — a real path, a real line, the code as written.
mod source;
use oslo_base::diag;
use source::in_the_script;
pub use source::{complain_at, complain_lua, parsed_position};

/// The one-liner, with a caret under `word` when there is a terminal to draw one on.
///
/// `words` is the command as the shell has it — the name and its operands. `word` is the one at
/// fault; when it is not among them the report is skipped and the one-liner printed, which is the
/// right answer for a word the message rewrote on its way there.
///
/// **Answers whether it drew**, which most callers ignore. The ones that do not are the ones with a
/// second line to print — a usage block after the diagnostic — because that line belongs *inside*
/// the report as its help and *after* the message when there is no report. Ignoring the answer is
/// always safe; the message is printed either way.
pub fn complain(words: &[String], word: &str, body: &str, label: &str, help: Option<&str>) -> bool {
    complain_from(&origin_now(), words, word, body, label, help)
}

/// The same, for a caller that already holds the origin.
///
/// **Not every diagnostic comes from a builtin.** `x=1` against a readonly name is a complaint about
/// a *line of a script* with no builtin involved, and `rm` walks a tree carrying the origin it
/// started with — neither has published one to the thread-local [`origin_now`] reads, so calling
/// that would print `oslo: ` where the file and line belong. The prefix a site already has is the
/// right one, and passing it is what keeps the message byte-identical.
pub fn complain_from(
    origin: &str,
    words: &[String],
    word: &str,
    body: &str,
    label: &str,
    help: Option<&str>,
) -> bool {
    let message = format!("{origin}{body}");
    if drawn(origin, words, word, &message, label, help) {
        return true;
    }
    eprintln!("{message}");
    false
}

/// The one-liner followed by a usage block — the commonest shape in the builtins.
///
/// **The block moves into the report rather than under it.** Printed beneath a drawn box it reads
/// as a second, unrelated message; as the report's help it is the answer to the question the caret
/// just asked. Where nothing is drawn it is the line it has always been, in the place it has always
/// been, which is what a pipe still sees.
pub fn complain_with_usage(words: &[String], word: &str, body: &str, label: &str, usage: &str) {
    if !complain(words, word, body, label, Some(usage)) {
        eprintln!("{usage}");
    }
}

/// The same, with the caret under one option **letter**.
///
/// `-pqz` is one word carrying three options, and a message about `-z` names a word that is not in
/// the argv at all. So the letter is found inside whichever word groups it, and the caret is that
/// wide — one character, where the mistake is.
pub fn complain_option(words: &[String], letter: char, body: &str, usage: &str) {
    let grouped = words.iter().find(|word| {
        word.starts_with('-')
            && !word.starts_with("--")
            && word.chars().skip(1).any(|c| c == letter)
    });
    let drew = match grouped {
        Some(word) => {
            // Byte offset, because that is what the span is measured in — a grouped option can
            // follow a multi-byte character in a word somebody typed by accident.
            let at = word
                .char_indices()
                .find(|(_, c)| *c == letter)
                .map(|(at, c)| at..at + c.len_utf8());
            match at {
                Some(inside) => {
                    complain_within(words, word, inside, body, "not an option here", Some(usage))
                }
                None => false,
            }
        }
        None => complain(
            words,
            &format!("-{letter}"),
            body,
            "not an option here",
            Some(usage),
        ),
    };
    if !drew {
        eprintln!("{usage}");
    }
}

/// The same, for a caret under part of a word: `cols a,b,nmae` under `nmae` alone.
///
/// `inside` is a byte range within `word`.
pub fn complain_within(
    words: &[String],
    word: &str,
    inside: std::ops::Range<usize>,
    body: &str,
    label: &str,
    help: Option<&str>,
) -> bool {
    complain_within_from(&origin_now(), words, word, inside, body, label, help)
}

/// The same, for a caller that already holds the origin — see [`complain_from`].
#[allow(clippy::too_many_arguments)]
pub fn complain_within_from(
    origin: &str,
    words: &[String],
    word: &str,
    inside: std::ops::Range<usize>,
    body: &str,
    label: &str,
    help: Option<&str>,
) -> bool {
    let message = format!("{origin}{body}");
    if diag::enabled() {
        // The script first, for the same reason [`drawn`] tries it first: a real path and a real
        // line beat a rebuilt one.
        if in_the_script(origin, word, Some(inside.clone()), &message, label, help) {
            return true;
        }
        let report = diag::Report {
            message: &message,
            source: source_of(words),
            label,
            help,
        };
        let snapshot = diag::Snapshot::of(words);
        if let Some(at) = snapshot.index_of(word)
            && snapshot.draw_within(at, inside, &report)
        {
            return true;
        }
    }
    eprintln!("{message}");
    false
}

/// Whether a report was drawn. Split out so both entry points ask the question the same way.
fn drawn(
    origin: &str,
    words: &[String],
    word: &str,
    message: &str,
    label: &str,
    help: Option<&str>,
) -> bool {
    // **Asked before anything is built.** On a pipe this is the whole cost of the feature: one
    // cached bool, no snapshot, no format beyond the message that was going to be printed anyway.
    if !diag::enabled() {
        return false;
    }
    // **A real file beats a rebuilt line, every time.** When the diagnostic came from a script the
    // origin already names the file and the line; reading that line and pointing into it gives a
    // path, a line number and the code as written — which is the difference between a caret and a
    // compiler's diagnostic. The snapshot below is the fallback for a prompt and a `-c` string,
    // which have no file to name.
    if in_the_script(origin, word, None, message, label, help) {
        return true;
    }
    let snapshot = diag::Snapshot::of(words);
    let Some(at) = snapshot.index_of(word) else {
        return false;
    };
    snapshot.draw(
        at,
        &diag::Report {
            message,
            source: source_of(words),
            label,
            help,
        },
    )
}

///
/// A builtin's argv is not a file, and pretending it has a path would be a lie in the one place a
/// person looks for one. `kill:1:9` reads as "the ninth column of what you typed", which is what it
/// is.
fn source_of(words: &[String]) -> &str {
    words.first().map(String::as_str).unwrap_or("oslo")
}

/// A command line rebuilt from its name and its operands.
///
/// **For the many functions handed `names` rather than argv.** `print_variables(env, names)`,
/// `export_functions(env, names)`, `select(jobs, operands, name)` — each knows the operands it is
/// working through and the builtin it belongs to, and neither has the original `args` slice. Those
/// two are what a source line is: `export foo bar` is faithful enough to point into, and threading
/// argv down through five signatures to recover the same three words is not.
pub fn line(name: &str, operands: &[String]) -> Vec<String> {
    std::iter::once(name.to_string())
        .chain(operands.iter().cloned())
        .collect()
}

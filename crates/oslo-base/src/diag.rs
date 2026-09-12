//! A caret under the word that was wrong.
//!
//! ```text
//! oslo: kill: NOPE: invalid signal specification
//!    ╭─[ kill:1:9 ]
//!    │
//!  1 │ kill -s NOPE 1
//!    │         ──┬─
//!    │           ╰─── not a signal
//!    │
//!    │ Help: a signal is a name (TERM), a number (15), or SIG-prefixed (SIGTERM)
//! ───╯
//! ```
//!
//! # The one rule
//!
//! **A script, a pipe and a test see exactly what they saw before this existed.**
//!
//! oslo is POSIX-first, and POSIX says what a shell writes to standard error. The report above on
//! the stderr of a non-interactive shell would break `2>&1 | grep`, break every conformance suite,
//! and break scripts written before oslo existed. So this is the *drawn face* of an error and the
//! one-liner is its transport — the same split `render_display` and `render_transport` are two
//! functions for — and [`crate::diag::enabled`] is what decides between them.
//!
//! The first line of the report **is** the one-liner. So nothing is lost when it draws, nothing is
//! printed twice, and a caller that has drawn one simply does not print its own.
//!
//! # The source is manufactured here, not carried from a parser
//!
//! A [`crate::diag::Snapshot`] joins a command's own words back into one line and remembers where
//! each of them
//! landed. That line is a perfectly good thing to point into, and it costs nothing upstream: no
//! parser learns to keep spans, no error type grows a field, no signature changes. It is what makes
//! this affordable across three hundred sites rather than affordable across five.
//!
//! Where a *real* source exists — a script, an `init.lua`, the text of a `where` expression —
//! [`crate::diag::draw_source`] points into that instead, and the report names the file.
//!
//! # Nothing here may panic
//!
//! Release builds are `panic = "abort"`, so a panic on the diagnostic path kills the shell *while
//! it is already reporting an error* — the worst possible moment and the hardest to reproduce. No
//! `unwrap` on a span, no slicing at a byte offset that might not be a character boundary. See
//! [`crate::diag::floor_boundary`].

use std::io::IsTerminal;
use std::ops::Range;
use std::sync::OnceLock;

/// What to put around the caret.
pub struct Report<'a> {
    /// The one-line message, **exactly** as it would have been printed to a pipe.
    ///
    /// Passed in rather than assembled here, which is what makes the two faces provably the same
    /// error: the drawn one is the transport one with a picture under it.
    pub message: &'a str,
    /// What the report calls the source — a builtin's name, or a file's path.
    pub source: &'a str,
    /// A few words against the caret itself. Not the message again: the message says what went
    /// wrong, this says what is wrong *with this word*.
    pub label: &'a str,
    /// What would have been right. Absent when there is nothing useful to say, because a help line
    /// that restates the message is noise with a keyword in front of it.
    pub help: Option<&'a str>,
}

/// Whether a report would be drawn at all.
///
/// **Asked before one is built**, so a pipeline that is not going to draw pays nothing: no
/// allocation for the snapshot, no walk over the words, no format.
///
/// `OSLO_DIAG=always` and `OSLO_DIAG=never` override; otherwise it is whether stderr is a terminal.
/// Cached in a `OnceLock` because this is the failure path of every builtin in the shell and an
/// `ioctl` per diagnostic is the same waste `structured_sinks` refuses to spend on its own gate.
pub fn enabled() -> bool {
    static MODE: OnceLock<bool> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("OSLO_DIAG").ok().as_deref() {
        Some("always") => true,
        Some("never") => false,
        _ => std::io::stderr().is_terminal(),
    })
}

/// Whether the caret and its rule should be coloured.
///
/// Separate from [`enabled`]: a terminal that set `NO_COLOR` still wants to be shown *where* the
/// error is, it just does not want it in red. `OSLO_DIAG=always` into a file is the same case.
///
/// `CLICOLOR_FORCE` is the other half of the same convention `NO_COLOR` is, and it is honoured for
/// the reason every tool honours it: something is capturing the output and wants to keep the
/// colour. It cannot put an escape anywhere a report was not already going, because [`enabled`]
/// decides that first and answers `false` for a pipe unless `OSLO_DIAG` said otherwise.
fn coloured() -> bool {
    static COLOUR: OnceLock<bool> = OnceLock::new();
    *COLOUR.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        std::env::var_os("CLICOLOR_FORCE").is_some_and(|on| on != "0")
            || std::io::stderr().is_terminal()
    })
}

/// The greatest character boundary at or below `at`.
///
/// A byte offset that came from arithmetic — a column count, a length in bytes, an index into a
/// word that was itself sliced — can land inside a multi-byte character, and both `Source` and
/// `&str[..]` would panic on it. With `panic = "abort"` that ends the shell, so every offset this
/// module hands to ariadne goes through here first.
pub fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// A command's words, rejoined into one line to point into.
pub struct Snapshot {
    text: String,
    spans: Vec<Range<usize>>,
}

impl Snapshot {
    /// The words as one line, remembering where each of them landed.
    ///
    /// Single spaces, whatever separated them on the real command line. The line is a *drawing* of
    /// the command rather than a transcript of it — the shell has already expanded, split and
    /// unquoted by the time a builtin can complain, so the original text no longer exists to be
    /// faithful to. What a person needs is to see which of the words they gave is the one at fault.
    pub fn of<S: AsRef<str>>(words: &[S]) -> Snapshot {
        let mut text = String::new();
        let mut spans = Vec::with_capacity(words.len());
        for word in words {
            if !text.is_empty() {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(word.as_ref());
            spans.push(start..text.len());
        }
        Snapshot { text, spans }
    }

    /// Which word this is, by its text.
    ///
    /// The first match, because a builtin complaining about `foo` in `cmd foo foo` means the one it
    /// looked at, and it looked at the first. Answering `None` is ordinary: the word may have been
    /// rewritten on the way to the message — a signal name upper-cased, a path made absolute — and
    /// a caret under nothing is worse than no caret.
    pub fn index_of(&self, word: &str) -> Option<usize> {
        self.spans
            .iter()
            .position(|span| self.text.get(span.clone()) == Some(word))
    }

    /// The nth word, counting the command name as zero.
    pub fn index_of_positional(&self, n: usize) -> Option<usize> {
        self.spans.get(n).map(|_| n)
    }

    /// How many words there are.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Draw, with the caret under word `at`. Answers whether it drew.
    ///
    /// **`false` means the caller prints its own one-liner**, and that is the ordinary answer
    /// whenever stderr is not a terminal. A caller that ignores it prints nothing at all on a pipe.
    pub fn draw(&self, at: usize, report: &Report) -> bool {
        match self.spans.get(at) {
            Some(span) => draw_at(&self.text, span.clone(), report),
            None => false,
        }
    }

    /// Draw, with the caret under part of word `at` — `cols a,b,nmae` under `nmae` alone.
    ///
    /// `inside` is relative to the word. An empty or out-of-range range falls back to the whole
    /// word rather than drawing a caret of width zero somewhere arbitrary.
    pub fn draw_within(&self, at: usize, inside: Range<usize>, report: &Report) -> bool {
        let Some(word) = self.spans.get(at) else {
            return false;
        };
        let start = word.start + inside.start;
        let end = word.start + inside.end;
        let span = match start < end && end <= word.end {
            true => start..end,
            false => word.clone(),
        };
        draw_at(&self.text, span, report)
    }
}

/// Draw a caret into text that really is a source — a script, an `init.lua`, an expression.
///
/// The report names `report.source`, so this is where a diagnostic gets to look like a compiler's:
/// a path, a line and a column that a person can act on.
pub fn draw_source(text: &str, span: Range<usize>, report: &Report) -> bool {
    draw_at(text, span, report)
}

/// The whole of the ariadne dependency, in one function.
///
/// Kept to one call site so the crate's API is a detail of this file: everything above deals in
/// `&str` and `Range<usize>`, which is what makes [`super::diag_stub`] able to mirror it exactly.
fn draw_at(text: &str, span: Range<usize>, report: &Report) -> bool {
    use ariadne::{CharSet, Color, Config, IndexType, Label, ReportKind, Source};

    if !enabled() {
        return false;
    }
    // Every offset is floored to a character boundary before it reaches ariadne. See the module
    // note on `panic = "abort"`: a diagnostic that panics kills the shell mid-diagnostic.
    let start = floor_boundary(text, span.start);
    let end = floor_boundary(text, span.end).max(start);
    if text.is_empty() || start >= text.len() {
        return false;
    }
    let span = start..end.max(start + 1).min(text.len());

    let id = report.source;
    let config = Config::default()
        .with_index_type(IndexType::Byte)
        .with_color(coloured())
        .with_char_set(CharSet::Unicode);

    let caret = Label::new((id, span.clone()))
        .with_color(Color::Rgb(CARET.0, CARET.1, CARET.2))
        .with_message(report.label);

    let mut built = ariadne::Report::build(ReportKind::Error, (id, span))
        .with_config(config)
        .with_label(caret);
    if let Some(help) = report.help {
        built = built.with_help(help);
    }

    let mut body: Vec<u8> = Vec::new();
    if built
        .finish()
        .write((id, Source::from(text)), &mut body)
        .is_err()
    {
        return false;
    }
    let body = String::from_utf8_lossy(&body);
    // **The message is the report's first line**, so the drawn face and the transport face carry
    // the same words and a caller that drew one does not print the other.
    //
    // ariadne opens every report with a `Kind: message` line of its own, and this one is built with
    // no message — oslo's already names the origin, the builtin and the operand, and a second
    // summary above it would say the same thing in a different order. So that line is dropped
    // rather than filled in. Always the first line, and always present, which is what makes taking
    // it off safe without matching on its text — it may be coloured.
    let drawing = body
        .split_once('\n')
        .map_or(body.as_ref(), |(_, rest)| rest);
    let drawing = truecolour(drawing);
    eprint!("{}\n{drawing}", report.message);
    true
}

#[cfg(test)]
#[path = "diag/tests.rs"]
mod tests;

/// The report's colours, as **truecolour** triples oslo owns.
///
/// # Why not the terminal's palette
///
/// ariadne paints with `Color::Red` and four `Color::Fixed(n)` indices, which are the *terminal's*
/// colours by number rather than colours. A palette generator that rewrites them — pywal writes
/// 256 `OSC 4` redefinitions on every shell start, and this repository's own `init.lua` is one of
/// the things that does — moves every one of them, so the caret comes out whatever the wallpaper
/// happened to be and the gutter comes out at whatever contrast is left. A diagnostic is the one
/// piece of output that has to stay legible when everything else has been re-themed.
///
/// So the report is painted in `38;2;r;g;b`, which no `OSC 4` can touch. The five below are the
/// sRGB values of the xterm indices ariadne chose, so the report looks exactly as it did on a
/// terminal that had not been re-themed — the change is what it is *immune to*, not how it looks.
///
/// The caret is the exception: `Color::Red` is index 1, the most aggressively re-themed colour
/// there is, and this is a red picked to read on a light background and a dark one.
const CARET: (u8, u8, u8) = (215, 95, 95);

/// ariadne's own four, and what each becomes. Left is exactly what `yansi` writes for
/// `Color::Fixed(n)`; right is that index's sRGB value.
///
/// **A rewrite rather than a setting**, because `ariadne::Config` has no colour knob: the palette
/// is five hard-coded constants in its `lib.rs`. Rewriting the finished bytes is a smaller and more
/// honest change than forking the crate, and it cannot go wrong quietly — a code that stops being
/// emitted simply stops being replaced.
const PALETTE: [(&str, &str); 5] = [
    // The `Error` header's red. Dropped with its line, but mapped so a future report that keeps a
    // header cannot leak an ANSI colour.
    ("\x1b[31m", "\x1b[38;2;215;95;95m"),
    // margin
    ("\x1b[38;5;246m", "\x1b[38;2;148;148;148m"),
    // skipped margin
    ("\x1b[38;5;240m", "\x1b[38;2;88;88;88m"),
    // the source line itself
    ("\x1b[38;5;249m", "\x1b[38;2;178;178;178m"),
    // `Help:` and `Note:`
    ("\x1b[38;5;115m", "\x1b[38;2;135;215;175m"),
];

/// The report with every palette colour turned into the sRGB value it stood for.
fn truecolour(body: &str) -> String {
    // Nothing to do on the common path: a pipe draws nothing, and `NO_COLOR` draws no escapes.
    if !body.contains('\x1b') {
        return body.to_string();
    }
    let mut out = body.to_string();
    for (palette, rgb) in PALETTE {
        if out.contains(palette) {
            out = out.replace(palette, rgb);
        }
    }
    out
}

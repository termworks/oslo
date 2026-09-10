//! Untrusted text on its way into a diagnostic.
//!
//! A shell says the names it was given back to you — `cd: $1: No such file or directory` — and the
//! names come from wherever the script got them: an argument, a glob, a line of somebody else's
//! output, a filename in a tarball. Written through unchanged, a name carrying an escape sequence
//! is not text the terminal shows but an instruction the terminal obeys.
//!
//! ```console
//! $ cd "$(printf '\033[2Jwiped')"
//! oslo: cd: ^[[2Jwiped: No such file or directory     # and the screen is now blank
//! ```
//!
//! Under a terminal that speaks `OSC 8` the same gap puts an arbitrary hyperlink inside oslo's own
//! error text, so the message a person is reading offers them a link the message did not write.
//!
//! # Why not just strip them
//!
//! Because the name is the answer. `cd` failed *on this name*, and a diagnostic that silently drops
//! part of it describes a different failure — the one thing worse than an unreadable name is a
//! plausible wrong one. So nothing is removed: a byte the terminal would act on is spelled instead,
//! in the form a shell already has for exactly this, and the name stays complete and re-typable.
//!
//! # The form
//!
//! `$'…'`, which is what bash prints and what a person can paste straight back into a command line.
//! bash applies it to some of its messages and not others — `cd` and `command not found` are
//! escaped, `source` and a failed redirect are not — so there is no compatible behaviour to copy
//! here, only a choice. oslo escapes everywhere.

/// A character the terminal would act on rather than draw.
///
/// `char::is_control` and nothing else: it covers C0, `DEL` and the C1 block, and it leaves every
/// printable character alone. **A name is usually not ASCII and that is not a problem** — `é`, `☃`
/// and `日本語` are ordinary things to call a file, and escaping them would make the common case
/// unreadable to protect against the rare one.
fn acts(c: char) -> bool {
    c.is_control()
}

/// `text` as a diagnostic may print it: unchanged when it is safe, `$'…'` when it is not.
///
/// Quoting is all-or-nothing per string, the way a shell quotes: a name with one escape in the
/// middle comes out as one `$'…'` rather than as a splice of quoted and bare runs, which is both
/// what bash does and what stays re-typable.
pub fn shown(text: &str) -> String {
    if !text.chars().any(acts) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    out.push_str("$'");
    for c in text.chars() {
        match c {
            // The named forms, so the common cases read as themselves. `\E` rather than `\e`
            // because that is the spelling bash prints.
            '\x1b' => out.push_str("\\E"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            // Inside `$'…'` these two end or escape the quoting, so they carry their own weight
            // whether or not anything else here needed escaping.
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            // ASCII, where a byte and a character are the same thing and `\xHH` says so exactly.
            c if acts(c) && c.is_ascii() => out.push_str(&format!("\\x{:02x}", c as u32)),
            // **Above ASCII, `\xHH` would be a different character.** The C1 block is `U+0080` to
            // `U+009F`, and each is two bytes in UTF-8 — `$'\x85'` pastes back as the single byte
            // `0x85`, which is not `U+0085` and is not even valid UTF-8 on its own. `\u` is the
            // form that reproduces the name that was actually there.
            c if acts(c) => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `text` as a diagnostic that already wraps the name in quotes may print it.
///
/// **One token, not a quote inside a quote.** `rm: cannot remove '{}'` around an escaped name gives
/// `'$'\E[2Jx''`, which reads as quoted and is not: pasted back it is a literal `$`, some bare
/// characters and an empty string — a different name from the one that failed. `$'…'` already
/// carries its own quoting, so the wrapper is the thing to drop.
///
/// Use this where the message supplies the quotes, and [`shown`] where it does not.
pub fn quoted(text: &str) -> String {
    match text.chars().any(acts) {
        true => shown(text),
        false => format!("'{text}'"),
    }
}

#[cfg(test)]
#[path = "shown/tests.rs"]
mod tests;

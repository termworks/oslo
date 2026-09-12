//! Closing a bracket or a quote you have only opened.
//!
//! ```text
//! echo |          "  →  echo "|"        opened, and closed for you
//! echo "hi|"      "  →  echo "hi"|      stepped over, not doubled
//! echo "|"    backspace →  echo |       both halves, because you only made one gesture
//! it|            '  →  it'|             not paired: a word to the left is an apostrophe
//! echo |x         (  →  echo (|x        not paired: something is already there to close over
//! ```
//!
//! # Why the rules are about the *neighbours*
//!
//! Pairing is only ever right when the closer would land where nothing else wants to be. Typing `(`
//! before a word means wrapping that word, and inserting `)` there splits it; typing `'` after a
//! letter is nearly always an apostrophe, and in a shell a stray one turns the rest of the line
//! into a quoted string. So the whole feature is two questions about the characters on either side
//! of the cursor, and the answer to both has to be no before anything is inserted.
//!
//! The rules are [zsh-autopair](https://github.com/hlissner/zsh-autopair)'s, which are the ones
//! people have actually lived with, restated as a table.
//!
//! # What it does not try to be
//!
//! **It does not know whether the cursor is inside a string.** That would need the line parsed on
//! every keystroke, and the answer would still be wrong halfway through typing one. Every rule here
//! looks at one character on each side, which is why it is predictable — you can see the reason for
//! what it did without knowing what the shell made of the line.
//!
//! It also never *removes* a character you typed. The worst it does is put one more in, and
//! `Pairing::Skip` is the case where it puts none.

use super::buffer::Buffer;

/// The pairs, opener first. A quote is its own partner, which is the whole reason the rules below
/// have to treat them separately: for a bracket, what to do is decided by which one you typed.
const PAIRS: [(char, char); 6] = [
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('"', '"'),
    ('\'', '\''),
    ('`', '`'),
];

/// What typing a character should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pairing {
    /// Insert it, then its partner, and stay between the two.
    Close(char),
    /// The partner is already at the cursor. Step over it rather than making a second one.
    Skip,
    /// Ordinary text.
    Plain,
}

/// Whether the feature is on. Set once from the config; read on the keystroke path.
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Type `typed`, closing it or stepping over it as the rules below say.
///
/// The whole behaviour is here rather than in the session's key handling, so that "what typing a
/// bracket does" is one thing to read and the editor's own arm stays one line.
pub(super) fn insert(buffer: &mut Buffer, typed: char) {
    match on_insert(buffer, typed) {
        Pairing::Close(closer) => {
            buffer.insert(typed);
            buffer.insert(closer);
            buffer.move_left();
        }
        Pairing::Skip => buffer.move_right(),
        Pairing::Plain => buffer.insert(typed),
    }
}

/// Backspace, taking the closer with it when the cursor is between a pair.
///
/// Answers whether anything moved, which is what the editor needs to know to redraw.
pub(super) fn backspace(buffer: &mut Buffer) -> bool {
    let paired = on_backspace(buffer);
    let moved = buffer.backspace();
    if paired && moved {
        buffer.delete();
    }
    moved
}

/// What should happen when `typed` is inserted at the cursor.
fn on_insert(buffer: &Buffer, typed: char) -> Pairing {
    match enabled() {
        true => decide(buffer, typed),
        false => Pairing::Plain,
    }
}

/// The rules themselves, with no opinion about whether the feature is switched on.
///
/// Separate so that the rules can be tested as what they are — a function of two neighbours — and
/// the switch tested once, on its own. They were one function, and the switch being process-wide
/// made every test of a rule race with the test of the switch.
fn decide(buffer: &Buffer, typed: char) -> Pairing {
    let after = buffer.at_cursor();
    let before = buffer.char_at(buffer.previous_grapheme(buffer.cursor()));

    // **Stepping over comes first**, and it has to: for a quote, the character that closes is the
    // character that opens, so a rule that asked "is this an opener?" first would answer every
    // closing quote by opening another pair.
    if closes(typed) && after == Some(typed) && !opens_here(typed, before, after) {
        return Pairing::Skip;
    }
    match partner(typed) {
        Some(closer) if opens_here(typed, before, after) => Pairing::Close(closer),
        _ => Pairing::Plain,
    }
}

/// Whether backspace should take the closer with it.
///
/// Only when the two are still adjacent and still a pair — `("` is not one, and neither is a `(`
/// whose `)` you have already typed past. One gesture made both characters, so one gesture removes
/// both; anything else is deleting something the user did not put there.
fn on_backspace(buffer: &Buffer) -> bool {
    enabled() && straddles_a_pair(buffer)
}

/// Whether the cursor sits between an opener and its own closer.
fn straddles_a_pair(buffer: &Buffer) -> bool {
    let before = buffer.char_at(buffer.previous_grapheme(buffer.cursor()));
    match (before, buffer.at_cursor()) {
        (Some(opener), Some(closer)) => partner(opener) == Some(closer),
        _ => false,
    }
}

/// The character that closes `opener`, if it opens anything.
fn partner(opener: char) -> Option<char> {
    PAIRS
        .iter()
        .find(|(open, _)| *open == opener)
        .map(|(_, close)| *close)
}

/// Whether this character ever closes something.
fn closes(c: char) -> bool {
    PAIRS.iter().any(|(_, close)| *close == c)
}

/// Whether a pair opened here would be one the user wants.
///
/// **Two neighbours, two reasons to decline.**
///
/// To the right: something is already there for the closer to be in the way of. `echo (x` should
/// not become `echo ()x`, and the test is deliberately wide — a letter, a digit, another opener,
/// or punctuation that means the line continues.
///
/// To the left, and only for quotes: a word character means an apostrophe far more often than it
/// means the start of a string. `it's`, `don't`, `Bob's`. Getting this wrong is worse than not
/// pairing at all, because in a shell the extra quote swallows the rest of the line.
fn opens_here(typed: char, before: Option<char>, after: Option<char>) -> bool {
    if partner(typed).is_none() {
        return false;
    }
    if after.is_some_and(crowded_on_the_right) {
        return false;
    }
    let quote = matches!(typed, '"' | '\'' | '`');
    if quote && before.is_some_and(word_like) {
        return false;
    }
    // A backslash means the next character is data, so it is not opening anything.
    before != Some('\\')
}

/// A character the closer would be pushed up against.
fn crowded_on_the_right(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '(' | '['
                | '{'
                | '<'
                | ','
                | '.'
                | ':'
                | '?'
                | '/'
                | '%'
                | '$'
                | '!'
                | '"'
                | '\''
                | '`'
                | '~'
                | '-'
                | '_'
                | '@'
                | '#'
                | '*'
                | '+'
                | '='
                | '\\'
        )
}

/// A character that makes a following quote an apostrophe rather than an opening.
fn word_like(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ']' | '}' | ')' | '_')
}

#[cfg(test)]
#[path = "pair/tests.rs"]
mod tests;

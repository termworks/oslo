//! What a key means while a line is being typed.
//!
//! A table, and nothing else — [`action`] is a pure function from a key to an intent, so the set
//! of bindings can be read in one screen and asserted in a test. Deciding what a key *does* while
//! also deciding how to redraw is how key handling becomes untestable.
//!
//! # readline's bindings, because they are the ones in people's fingers
//!
//! oslo is replacing `/bin/sh`, so the emacs bindings bash has used since 1989 are not a style
//! choice — a shell that moved `C-w` would be wrong however good the new place was. Every binding
//! here has a readline name, given in the comment beside it.
//!
//! Some chords never reach this table: [`crate::term`] decodes `C-a` as
//! [`Key::Home`], `C-b` as [`Key::Left`] and so on, because those have a meaning every widget
//! shares. Only what is left over arrives as [`Key::Ctrl`].

use crate::term::Key;

/// What the editor should do about a keypress.
///
/// Deliberately about *editing*, not about keys: the same action can arrive from a chord, an
/// arrow, or a config binding, and the loop that performs it should not care which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Put this character in the line.
    Insert(char),

    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,

    Backspace,
    /// Forward delete. On an *empty* line this is end-of-input instead, which only the caller can
    /// tell — see [`Action::Delete`]'s use in the loop.
    Delete,
    KillToEnd,
    KillToStart,
    KillWordLeft,
    KillWordRight,
    KillSpaceWordLeft,
    Yank,
    Transpose,
    Upper,
    Lower,
    Capitalise,

    /// Enter, and Ctrl+Enter: run what is on the line.
    ///
    /// One line. A Lua block spanning several is accumulated by the *reader* behind a continuation
    /// prompt, not by putting newlines in a buffer the editor draws as one row.
    Accept,
    /// Ctrl-C: abandon this line and start a new one.
    Abort,
    /// Ctrl-D on an empty line: end of input.
    Eof,
    /// Tab.
    Complete,
    /// Shift-Tab.
    CompleteBack,
    /// Previous / next history entry.
    HistoryPrev,
    HistoryNext,
    /// Insert, while the line is a history entry: copy that entry to the clipboard.
    CopyHistoryEntry,
    /// Ctrl-R: search history.
    SearchHistory,
    /// Ctrl-X: hand the line to `$EDITOR` and take back what it wrote.
    EditExternally,
    /// Ctrl-L: clear the screen and redraw.
    Redraw,
    /// Esc, which in emacs mode is only the start of a chord and on its own means nothing.
    ///
    /// Kept as an action rather than dropped so vi mode — which very much does bind it — can be
    /// added without changing the decoder.
    Escape,
    /// A key with no binding. Doing nothing is the correct response and saying so explicitly is
    /// what keeps a redraw from being triggered by it.
    None,
}

/// The emacs binding table.
pub fn action(key: Key) -> Action {
    match key {
        Key::Char(c) => Action::Insert(c),

        // `term` already resolved C-a/C-e/C-b/C-f to these.
        Key::Left => Action::Left,
        Key::Right => Action::Right,
        Key::Home => Action::Home,
        Key::End => Action::End,
        Key::Up => Action::HistoryPrev,
        Key::Down => Action::HistoryNext,
        Key::Insert => Action::CopyHistoryEntry,

        Key::Backspace => Action::Backspace,
        Key::Delete => Action::Delete,
        // `C-u`, which `term` names for what it does at a prompt. readline calls it
        // `unix-line-discard` and it cuts to the *cursor*, not the whole line.
        Key::Clear => Action::KillToStart,

        Key::Accept => Action::Accept,
        // Ctrl+Enter, and Alt+Enter where that cannot arrive — both decode to `Submit` on
        // purpose, see `term::input`. Always sends, whatever Enter has been set to do.
        Key::Submit => Action::Accept,
        Key::Abort => Action::Abort,
        Key::ToggleScope => Action::Complete,
        Key::BackTab => Action::CompleteBack,
        Key::Cancel => Action::Escape,

        Key::Ctrl('k') => Action::KillToEnd,         // kill-line
        Key::Ctrl('w') => Action::KillSpaceWordLeft, // unix-word-rubout
        Key::Ctrl('y') => Action::Yank,              // yank
        Key::Ctrl('t') => Action::Transpose,         // transpose-chars
        Key::Ctrl('l') => Action::Redraw,            // clear-screen
        Key::Ctrl('r') => Action::SearchHistory,     // reverse-search-history
        // **Not readline's `C-x C-e`.** That is a two-key chord and oslo has no chord mechanism at
        // all; one keystroke on the same letter is the nearest thing that costs nothing to build,
        // and `C-x` is bound to nothing else here.
        Key::Ctrl('x') => Action::EditExternally,

        Key::Alt('b') => Action::WordLeft,      // backward-word
        Key::Alt('f') => Action::WordRight,     // forward-word
        Key::Alt('d') => Action::KillWordRight, // kill-word
        // `M-DEL`, which is *not* `C-w`: it takes one alphanumeric run, where `C-w` takes a whole
        // whitespace-delimited word. Both are bound because both are habits.
        Key::Alt('\x7f') => Action::KillWordLeft, // backward-kill-word
        Key::Alt('u') => Action::Upper,           // upcase-word
        Key::Alt('l') => Action::Lower,           // downcase-word
        Key::Alt('c') => Action::Capitalise,      // capitalize-word

        // A resize is not an edit: the loop redraws on it and the buffer is untouched.
        // `CtrlTab` is not an editing action: it is resolved by name as a language toggle, the
        // same way `shift-tab` is, so the table has nothing to say about it.
        Key::Ctrl(_)
        | Key::Alt(_)
        | Key::CtrlTab
        | Key::Function(_)
        | Key::PageUp
        | Key::PageDown
        | Key::Resized
        | Key::Ignored => Action::None,
    }
}

#[cfg(test)]
#[path = "keymap/tests.rs"]
mod tests;

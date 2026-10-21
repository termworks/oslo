//! The bindings, asserted one by one.
//!
//! A keymap is exactly the kind of thing that looks obviously right and is quietly wrong, and the
//! cost of being wrong is a key that does something else in oslo than in every other shell. So the
//! table is checked against readline's names rather than against itself.

use super::*;

/// Every chord readline binds that oslo must bind the same way.
///
/// One list rather than a test each: the value is in seeing them together, and a missing binding
/// shows up as a row that says `None`.
#[test]
fn the_emacs_bindings_match_readline() {
    let bindings: &[(Key, Action, &str)] = &[
        (Key::Ctrl('k'), Action::KillToEnd, "C-k kill-line"),
        (
            Key::Ctrl('w'),
            Action::KillSpaceWordLeft,
            "C-w unix-word-rubout",
        ),
        (Key::Ctrl('y'), Action::Yank, "C-y yank"),
        (Key::Ctrl('t'), Action::Transpose, "C-t transpose-chars"),
        (Key::Ctrl('l'), Action::Redraw, "C-l clear-screen"),
        (
            Key::Ctrl('r'),
            Action::SearchHistory,
            "C-r reverse-search-history",
        ),
        (Key::Clear, Action::KillToStart, "C-u unix-line-discard"),
        (Key::Alt('b'), Action::WordLeft, "M-b backward-word"),
        (Key::Alt('f'), Action::WordRight, "M-f forward-word"),
        (Key::Alt('d'), Action::KillWordRight, "M-d kill-word"),
        (
            Key::Alt('\x7f'),
            Action::KillWordLeft,
            "M-DEL backward-kill-word",
        ),
        (Key::Alt('u'), Action::Upper, "M-u upcase-word"),
        (Key::Alt('l'), Action::Lower, "M-l downcase-word"),
        (Key::Alt('c'), Action::Capitalise, "M-c capitalize-word"),
    ];
    for (key, want, name) in bindings {
        assert_eq!(action(*key), *want, "{name}");
    }
}

/// **`C-w` and `M-DEL` must not be the same action.** They kill different amounts, and collapsing
/// them is the single most likely way to get this table wrong.
#[test]
fn the_two_word_kills_stay_distinct() {
    assert_ne!(action(Key::Ctrl('w')), action(Key::Alt('\x7f')));
}

/// The chords `term` resolves to shared names still arrive as motions, so `C-a`/`C-e`/`C-b`/`C-f`
/// work without this table naming them.
#[test]
fn the_chords_term_already_named_still_move_the_cursor() {
    assert_eq!(action(Key::Home), Action::Home);
    assert_eq!(action(Key::End), Action::End);
    assert_eq!(action(Key::Left), Action::Left);
    assert_eq!(action(Key::Right), Action::Right);
}

#[test]
fn typing_is_an_insert_of_the_character() {
    assert_eq!(action(Key::Char('x')), Action::Insert('x'));
    assert_eq!(action(Key::Char('é')), Action::Insert('é'));
    assert_eq!(action(Key::Char('日')), Action::Insert('日'));
}

/// **Enter ends the line, and Ctrl/Alt+Enter ends it unconditionally.**
///
/// Both end the line. A Lua block spanning several lines is accumulated by the reader behind a
/// continuation prompt — the editor edits one line and draws one line.
#[test]
fn the_line_is_ended_by_enter_and_abandoned_by_ctrl_c() {
    assert_eq!(action(Key::Accept), Action::Accept);
    assert_eq!(action(Key::Submit), Action::Accept);
    assert_eq!(action(Key::Abort), Action::Abort);
}

/// Up and Down walk history at a prompt — unlike in a widget, where they move a selection.
#[test]
fn the_arrows_walk_history() {
    assert_eq!(action(Key::Up), Action::HistoryPrev);
    assert_eq!(action(Key::Down), Action::HistoryNext);
    assert_eq!(action(Key::Insert), Action::CopyHistoryEntry);
}

/// An unbound chord does nothing, and says so — rather than falling through to inserting a
/// control character into the line, which is how a stray keypress corrupts a command.
#[test]
fn an_unbound_chord_does_nothing() {
    assert_eq!(action(Key::Ctrl('q')), Action::None);
    assert_eq!(action(Key::Alt('z')), Action::None);
    assert_eq!(action(Key::Ignored), Action::None);
    assert_eq!(action(Key::PageUp), Action::None);
}

/// Nothing may map to `Insert` except an actual character. A control chord that inserted itself
/// would put an unprintable byte in the command line.
#[test]
fn only_characters_insert() {
    let keys = [
        Key::Ctrl('k'),
        Key::Ctrl('w'),
        Key::Ctrl('y'),
        Key::Ctrl('t'),
        Key::Ctrl('l'),
        Key::Ctrl('r'),
        Key::Ctrl('q'),
        Key::Alt('b'),
        Key::Alt('f'),
        Key::Alt('z'),
        Key::Alt('\x7f'),
        Key::Backspace,
        Key::Delete,
        Key::Home,
        Key::End,
        Key::Left,
        Key::Right,
        Key::Up,
        Key::Down,
        Key::Accept,
        Key::Abort,
        Key::Cancel,
        Key::Clear,
        Key::ToggleScope,
        Key::BackTab,
        Key::PageUp,
        Key::PageDown,
        Key::Ignored,
    ];
    for key in keys {
        assert!(
            !matches!(action(key), Action::Insert(_)),
            "{key:?} would type itself into the line"
        );
    }
}

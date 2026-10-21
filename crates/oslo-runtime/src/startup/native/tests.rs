//! What the shell plugs into the editor, driven without a terminal.
//!
//! The history walk and the key hook are the two with state, so they are the two worth pinning:
//! one must not consume what it shows, and the other must be offered keys a binding never sees.

use super::*;

fn assist(entries: &[&str]) -> ShellAssist<'static> {
    ShellAssist::new(
        entries.iter().map(|e| e.to_string()).collect(),
        None,
        0,
        vec!["shift-tab".to_string(), "ctrl-space".to_string()],
    )
}

/// Up walks backwards from the newest, and Down walks back out — restoring what was typed.
#[test]
fn the_history_walk_is_not_destructive() {
    let mut a = assist(&["first", "second"]);
    assert_eq!(a.history_prev("draft").as_deref(), Some("second"));
    assert_eq!(a.history_prev("draft").as_deref(), Some("first"));
    assert_eq!(a.history_next().as_deref(), Some("second"));
    assert_eq!(
        a.history_next().as_deref(),
        Some("draft"),
        "the composed line must come back, not a blank"
    );
    assert_eq!(a.history_next(), None, "and then there is nowhere to go");
}

/// The entry Insert would copy is the one the walk is standing on, for exactly as long as
/// the line still is that entry.
#[test]
fn the_walk_names_the_entry_it_stands_on() {
    let mut a = assist(&["first", "second"]);
    assert_eq!(a.history_entry("second"), None, "not walking yet");
    a.history_prev("draft");
    assert_eq!(a.history_entry("second").as_deref(), Some("second"));
    assert_eq!(a.history_entry("second!"), None, "edited since recall");
    a.history_prev("draft");
    assert_eq!(a.history_entry("first").as_deref(), Some("first"));
    a.history_next();
    a.history_next();
    assert_eq!(a.history_entry("draft"), None, "back to composing");
}

/// A new line resets the walk. Without this, Up on the next prompt resumes where the last one
/// stopped and looks like it skipped entries.
#[test]
fn a_new_line_starts_the_walk_over() {
    let mut a = assist(&["one", "two"]);
    a.history_prev("");
    a.begin();
    assert_eq!(a.history_prev("").as_deref(), Some("two"));
}

/// Running off the end leaves the line alone rather than clearing it.
#[test]
fn walking_past_the_oldest_entry_stops() {
    let mut a = assist(&["only"]);
    assert_eq!(a.history_prev("x").as_deref(), Some("only"));
    assert_eq!(a.history_prev("x"), None);
}

/// An empty line paints to nothing at all — no escapes, so the layout measures zero cells.
#[test]
fn an_empty_line_paints_to_nothing() {
    assert_eq!(assist(&[]).highlight(""), "");
}

/// **Every key has a hook name.** A key the hook cannot name is a key it cannot be told about,
/// and the whole point of the hook is that it sees all of them — so this asserts the ones
/// `key_name` deliberately leaves out, which is where a missing arm would actually hide.
#[test]
fn the_key_hook_can_name_the_keys_a_binding_cannot() {
    for (key, name) in [
        (Key::Accept, "enter"),
        (Key::Cancel, "esc"),
        (Key::Abort, "ctrl-c"),
        (Key::Backspace, "backspace"),
        (Key::Delete, "delete"),
    ] {
        assert_eq!(hook_key_name(key).0, name);
        assert!(
            key_name(key).is_none(),
            "{name} is not bindable, which is why the hook needs its own table"
        );
    }
    // An ordinary character is the one case with a `char`, and they all share one name so a
    // handler can ask "was this typing?" without listing the alphabet.
    assert_eq!(
        hook_key_name(Key::Char('ß')),
        ("char".to_string(), Some('ß'))
    );
    assert_eq!(hook_key_name(Key::Ctrl('k')).0, "ctrl-k");
    assert_eq!(hook_key_name(Key::Ctrl('k')).1, None);
    for number in 1..=12 {
        let name = format!("f{number}");
        assert_eq!(
            key_name(Key::Function(number)).as_deref(),
            Some(name.as_str())
        );
        assert_eq!(hook_key_name(Key::Function(number)).0, name);
    }
}

/// The cursor crosses the boundary between the editor, which counts characters, and Lua, which
/// counts bytes — so `line.text:sub(1, line.cursor)` is the text before the cursor even when
/// the line is not ASCII. Handing over the character index put the split in the wrong place.
#[test]
fn the_cursor_a_handler_sees_is_in_bytes() {
    let line = "größe x";
    // Six characters in, which is after the space: eight bytes, since ö and ß are two each.
    assert_eq!(byte_cursor(line, 6), 8);
    assert_eq!(&line[..byte_cursor(line, 6)], "größe ");
    // And back again, so a handler's answer lands where it asked.
    assert_eq!(char_cursor(line, 8), 6);
    for at in 0..=line.chars().count() {
        assert_eq!(char_cursor(line, byte_cursor(line, at)), at, "round trip");
    }

    let clustered = "ae\u{301}👍🏽z";
    assert_eq!(byte_cursor(clustered, 2), 1);
    assert_eq!(char_cursor(clustered, 2), 1);
    let after_accent = "ae\u{301}".len();
    assert_eq!(char_cursor(clustered, after_accent), 3);
    assert_eq!(byte_cursor(clustered, 3), after_accent);
}

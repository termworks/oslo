use super::shown;

/// An ordinary name is not touched, which is most of them.
#[test]
fn a_plain_name_is_left_alone() {
    for plain in [
        "config.lua",
        "a b",
        "--flag",
        "héllo",
        "☃",
        "日本語",
        "it's",
        "back\\slash",
        "",
    ] {
        assert_eq!(shown(plain), plain, "{plain:?} was quoted for no reason");
    }
}

/// The sequences that made this necessary.
#[test]
fn an_escape_sequence_is_spelled_rather_than_obeyed() {
    assert_eq!(shown("\x1b[2Jwiped"), "$'\\E[2Jwiped'");
    assert_eq!(shown("\x1b]0;title\x07"), "$'\\E]0;title\\a'");
    assert_eq!(shown("\x1b]8;;http://evil\x1b\\click"), {
        "$'\\E]8;;http://evil\\E\\\\click'"
    });
}

/// A name is not ASCII and stays readable; only what the terminal would act on is spelled.
#[test]
fn only_the_acting_characters_are_spelled() {
    assert_eq!(shown("héllo\nthere"), "$'héllo\\nthere'");
    assert_eq!(shown("日本\t語"), "$'日本\\t語'");
}

/// Inside `$'…'` a quote and a backslash carry their own weight, so they are escaped as soon as
/// anything puts the string in quotes at all.
#[test]
fn quoting_escapes_what_would_end_the_quoting() {
    assert_eq!(shown("it's\nhere"), "$'it\\'s\\nhere'");
    assert_eq!(shown("a\\b\nc"), "$'a\\\\b\\nc'");
}

/// Anything without a named form gets a numeric one rather than being dropped.
#[test]
fn an_unnamed_control_character_keeps_its_value() {
    assert_eq!(shown("\u{1}\u{2}"), "$'\\x01\\x02'");
    assert_eq!(shown("\u{7f}"), "$'\\x7f'");
    assert_eq!(shown("\u{85}"), "$'\\u0085'");
}

/// **Nothing is removed.** The name is what the diagnostic is about, so every character survives in
/// a form that can be read back — a stripped name would describe a different failure.
#[test]
fn nothing_is_lost() {
    let hostile = "\x1b[2J\u{1}héllo\nthere\t☃\u{7f}";
    let quoted = shown(hostile);
    let body = quoted
        .strip_prefix("$'")
        .and_then(|q| q.strip_suffix('\''))
        .expect("quoted form");
    for piece in [
        "\\E[2J", "\\x01", "héllo", "\\n", "there", "\\t", "☃", "\\x7f",
    ] {
        assert!(body.contains(piece), "{piece:?} missing from {quoted}");
    }
}

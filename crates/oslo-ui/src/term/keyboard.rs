use super::input::Key;
use std::sync::atomic::{AtomicU8, Ordering};

/// Disambiguate escape codes (1), and report alternate keys (4).
///
/// The second flag is what makes a shifted key legible. Without it the terminal names only the
/// base key — `3` for Shift+3 — and a shell has no way back to `#`, because which character the
/// shift layer produces is a property of the keymap, not of the codepoint. Asking costs one byte.
pub const PUSH_ENHANCEMENTS: &str = "\x1b[>5u";
pub const POP: &str = "\x1b[<u";

static SUPPORT: AtomicU8 = AtomicU8::new(0);

pub fn remember_support(supported: bool) {
    SUPPORT.store(if supported { 2 } else { 1 }, Ordering::Release);
}

pub fn supported() -> bool {
    SUPPORT.load(Ordering::Acquire) == 2
}

pub fn decode(sequence: &[u8]) -> Key {
    let Ok(body) = std::str::from_utf8(sequence) else {
        return Key::Ignored;
    };
    let Some(body) = body.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('u')) else {
        return Key::Ignored;
    };
    let mut fields = body.split(';');
    let mut key = fields.next().unwrap_or_default().split(':');
    let Some(code) = key.next().and_then(|s| s.parse::<u32>().ok()) else {
        return Key::Ignored;
    };
    let shifted = key
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .and_then(char::from_u32);
    let modifier_field = fields.next().unwrap_or("1");
    let mut modifier_parts = modifier_field.split(':');
    let modifiers = modifier_parts
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    let event = modifier_parts.next();
    if !matches!(event, None | Some("1")) || modifier_parts.next().is_some() {
        return Key::Ignored;
    }
    let shift = modifiers & 1 != 0;
    let alt = modifiers & 2 != 0;
    let ctrl = modifiers & 4 != 0;

    if let Some(key) = functional(code) {
        if code == 9 && shift {
            return Key::BackTab;
        }
        // Alt+Enter is the same act, and spells itself the same, for a terminal that cannot
        // report Ctrl+Enter.
        if alt && code == 13 {
            return Key::Submit;
        }
        if ctrl {
            return match code {
                // **Ctrl+Tab and Ctrl+Enter exist only here.** In the legacy encoding each is
                // indistinguishable from the key it modifies — Ctrl-I *is* Tab and Ctrl-M *is*
                // Enter, the same bytes — so this used to answer `Ctrl('i')` and `Ctrl('m')`,
                // which are those two keys under another name. The kitty protocol reports the
                // modifier, which is what makes them separable at all.
                9 => Key::CtrlTab,
                13 => Key::Submit,
                27 => Key::Ctrl('['),
                _ => Key::Ignored,
            };
        }
        return key;
    }

    let Some(mut character) = char::from_u32(code) else {
        return Key::Ignored;
    };
    // The terminal's own shifted key wins; uppercasing is the fallback for one that reports none,
    // and it is only ever right for a letter. Applying it to `3` yields `3`, which is how Shift+3
    // stopped producing `#`.
    if shift {
        character = match shifted {
            Some(alternate) => alternate,
            None if character.is_alphabetic() => {
                character.to_uppercase().next().unwrap_or(character)
            }
            None => character,
        };
    }
    if ctrl && !shift {
        control(character)
    } else if ctrl {
        Key::Ctrl(character)
    } else if alt {
        Key::Alt(character)
    } else if character.is_control() {
        Key::Ignored
    } else {
        Key::Char(character)
    }
}

fn control(character: char) -> Key {
    match character {
        'a' => Key::Home,
        'b' => Key::Left,
        'c' => Key::Abort,
        'd' => Key::Delete,
        'e' => Key::End,
        'f' => Key::Right,
        'j' => Key::Accept,
        'k' | 'l' | 'r' | 't' | 'w' | 'y' => Key::Ctrl(character),
        'n' => Key::Down,
        'p' => Key::Up,
        'u' => Key::Clear,
        _ => Key::Ctrl(character),
    }
}

fn functional(code: u32) -> Option<Key> {
    Some(match code {
        9 => Key::ToggleScope,
        13 => Key::Accept,
        27 => Key::Cancel,
        127 => Key::Backspace,
        57348 => Key::Insert,
        57349 => Key::Delete,
        57350 => Key::Left,
        57351 => Key::Right,
        57352 => Key::Up,
        57353 => Key::Down,
        57354 => Key::PageUp,
        57355 => Key::PageDown,
        57356 => Key::Home,
        57357 => Key::End,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collisions_are_disambiguated() {
        assert_eq!(decode(b"\x1b[9u"), Key::ToggleScope);
        assert_eq!(decode(b"\x1b[105;5u"), Key::Ctrl('i'));
        assert_eq!(decode(b"\x1b[13u"), Key::Accept);
        assert_eq!(decode(b"\x1b[109;5u"), Key::Ctrl('m'));
        // **The two chords only this protocol can report.** `Ctrl('i')` and `Ctrl('m')` above are
        // the *letters*; these are Ctrl held with the Tab and Enter keys themselves, which the
        // legacy encoding cannot tell apart from bare Tab and bare Enter.
        assert_eq!(decode(b"\x1b[9;5u"), Key::CtrlTab);
        assert_eq!(decode(b"\x1b[13;5u"), Key::Submit);
        assert_eq!(decode(b"\x1b[27u"), Key::Cancel);
        assert_eq!(decode(b"\x1b[91;5u"), Key::Ctrl('['));
    }

    #[test]
    fn modifiers_and_functional_keys_decode() {
        assert_eq!(decode(b"\x1b[9;2u"), Key::BackTab);
        assert_eq!(decode(b"\x1b[107;6u"), Key::Ctrl('K'));
        // Alt+Enter stands in for Ctrl+Enter where that cannot arrive, so it decodes to the same
        // key rather than to a second one nothing is bound to.
        assert_eq!(decode(b"\x1b[13;3u"), Key::Submit);
        assert_eq!(decode(b"\x1b[47;5u"), Key::Ctrl('/'));
        assert_eq!(decode(b"\x1b[57348u"), Key::Insert);
        assert_eq!(decode(b"\x1b[57350u"), Key::Left);
        assert_eq!(decode(b"\x1b[57350;1:1u"), Key::Left);
        assert_eq!(decode(b"\x1b[107;5:2u"), Key::Ignored);
        assert_eq!(decode(b"\x1b[107;5:3u"), Key::Ignored);
    }

    #[test]
    fn a_shifted_key_is_the_one_the_terminal_names() {
        assert_eq!(decode(b"\x1b[51:35;2u"), Key::Char('#'));
        assert_eq!(decode(b"\x1b[50:64;2u"), Key::Char('@'));
        assert_eq!(decode(b"\x1b[97:65;2u"), Key::Char('A'));
        assert_eq!(decode(b"\x1b[59:246;2u"), Key::Char('ö'));
        assert_eq!(decode(b"\x1b[97;2u"), Key::Char('A'));
        assert_eq!(decode(b"\x1b[51;2u"), Key::Char('3'));
        assert_eq!(decode(b"\x1b[51::51;2u"), Key::Char('3'));
        assert_eq!(decode(b"\x1b[51u"), Key::Char('3'));
    }

    #[test]
    fn control_letters_keep_the_editors_existing_meanings() {
        for (character, expected) in [
            ('a', Key::Home),
            ('b', Key::Left),
            ('c', Key::Abort),
            ('d', Key::Delete),
            ('e', Key::End),
            ('f', Key::Right),
            ('j', Key::Accept),
            ('n', Key::Down),
            ('p', Key::Up),
            ('u', Key::Clear),
            ('w', Key::Ctrl('w')),
        ] {
            let sequence = format!("\x1b[{};5u", character as u32);
            assert_eq!(decode(sequence.as_bytes()), expected, "Ctrl-{character}");
        }
        assert_eq!(decode(b"\x1b[107;6u"), Key::Ctrl('K'));
    }

    #[test]
    fn negotiated_support_is_remembered_for_the_line_guard() {
        remember_support(true);
        assert!(supported());
        remember_support(false);
        assert!(!supported());
    }
}

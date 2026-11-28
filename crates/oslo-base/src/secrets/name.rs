//! What a secret may be called.
//!
//! A name is a filename in the store and a word printed by `list`, so it may hold neither a path
//! separator nor anything a terminal acts on: no `/`, no leading `.`, no C0 or C1 control character
//! (newline and escape among them), no `DEL`, and none of the bidirectional overrides that make a
//! name display as a different one.
//!
//! **Names are bytes.** No Unicode normalisation is applied, so `é` written precomposed and `é`
//! written as `e` and a combining accent are two different names.

/// The longest name, in bytes: with `.sealed` and a scratch file's suffix it still fits in the 255
/// a Linux filename may have.
pub const LONGEST: usize = 200;

/// Refuse `name` with the reason, or accept it.
pub fn check(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.len() > LONGEST
        || name.chars().any(refused)
    {
        return Err(format!("{name:?} is not a usable name for a secret"));
    }
    Ok(())
}

/// A character no name may hold: a control character, or one that reorders the text around it.
pub fn refused(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// `name` as it may be printed: each refused character spelled `\u{…}`.
pub fn shown(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match refused(c) {
            true => out.push_str(&format!("\\u{{{:04x}}}", c as u32)),
            false => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_hold_what_a_terminal_acts_on() {
        for bad in [
            "",
            ".hidden",
            "..",
            "a/b",
            "with\0nul",
            "two\nlines",
            "tab\there",
            "\x1b[31mred",
            "del\x7f",
            "c1\u{85}",
            "evil\u{202e}txt.exe",
            "isolate\u{2066}x",
            "mark\u{200f}",
        ] {
            assert!(check(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(check(&"x".repeat(LONGEST + 1)).is_err());
        for good in [
            "deploy-token",
            "gh_token.2",
            "café",
            "日本語",
            &"x".repeat(LONGEST),
        ] {
            assert!(check(good).is_ok(), "{good:?} was refused");
        }
    }

    #[test]
    fn a_shown_name_carries_no_escape() {
        assert_eq!(shown("plain"), "plain");
        assert_eq!(shown("\x1b[2J"), "\\u{001b}[2J");
        assert_eq!(shown("a\u{202e}b"), "a\\u{202e}b");
        assert!(!shown("x\ny").contains('\n'));
    }
}

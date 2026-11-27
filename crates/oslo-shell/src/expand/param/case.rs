//! Case mapping: `${x^}`, `${x,,}`, `${x~}` and the `@U`, `@u`, `@L` transformations.
//!
//! bash maps one character at a time, with the C library's `towupper`/`towlower`: a character
//! whose full mapping is several characters (`ß` to `SS`, `ﬁ` to `FI`) stays as it is, and `İ`
//! lowers to a plain `i`. Outside a UTF-8 locale only ASCII has a case.

use crate::expand::glob::ShellPattern;

/// Which way a character is mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMap {
    Upper,
    Lower,
    /// Upper to lower and lower to upper.
    Toggle,
}

/// Map the characters matching `pattern` (all of them when it is `None`): the first one only, or
/// every one when `all`.
pub fn convert_case(
    value: &str,
    pattern: Option<&ShellPattern>,
    map: CaseMap,
    all: bool,
    utf8: bool,
) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, ch) in value.chars().enumerate() {
        let selected = match pattern {
            None => true,
            Some(p) => p.matches(&ch.to_string()),
        };
        // The single-character forms examine the first character and no other, so
        // `v=hello; ${v^l}` leaves the value alone: `h` is not what the pattern named.
        let eligible = selected && (all || index == 0);
        out.push(if eligible { map_char(ch, map, utf8) } else { ch });
    }
    out
}

/// One character mapped as `towupper`/`towlower` would.
pub fn map_char(ch: char, map: CaseMap, utf8: bool) -> char {
    if !utf8 && !ch.is_ascii() {
        return ch;
    }
    let map = match map {
        CaseMap::Toggle if ch.is_uppercase() => CaseMap::Lower,
        CaseMap::Toggle if ch.is_lowercase() => CaseMap::Upper,
        CaseMap::Toggle => return ch,
        other => other,
    };
    if map == CaseMap::Lower && ch == 'İ' {
        return 'i';
    }
    let mut mapped = match map {
        CaseMap::Upper => ch.to_uppercase().collect::<Vec<_>>(),
        _ => ch.to_lowercase().collect::<Vec<_>>(),
    };
    match mapped.len() {
        1 => mapped.remove(0),
        _ => ch,
    }
}

#[cfg(test)]
mod tests {
    use super::{CaseMap, convert_case};
    use crate::expand::glob::ShellPattern;

    fn pat(text: &str) -> ShellPattern {
        ShellPattern::from_unquoted(text)
    }

    #[test]
    fn case_conversion_respects_the_doubled_form() {
        let up = |v: &str, p: Option<&ShellPattern>, all| convert_case(v, p, CaseMap::Upper, all, true);
        assert_eq!(up("hello", None, true), "HELLO");
        assert_eq!(up("hello", None, false), "Hello");
        assert_eq!(convert_case("WORLD", None, CaseMap::Lower, true, true), "world");
        assert_eq!(convert_case("WORLD", None, CaseMap::Lower, false, true), "wORLD");
        assert_eq!(up("hello", Some(&pat("l")), true), "heLLo");
        // `${v^l}`: only the first character is a candidate, and it is not an `l`.
        assert_eq!(up("hello", Some(&pat("l")), false), "hello");
        assert_eq!(up("hello", Some(&pat("[el]")), true), "hELLo");
    }

    #[test]
    fn toggling_swaps_each_case() {
        assert_eq!(convert_case("HeLLo", None, CaseMap::Toggle, true, true), "hEllO");
        assert_eq!(convert_case("HeLLo", None, CaseMap::Toggle, false, true), "heLLo");
    }

    /// One character in, one out, as the C library maps them.
    #[test]
    fn a_character_maps_to_a_single_character() {
        assert_eq!(convert_case("Straße", None, CaseMap::Upper, true, true), "STRAßE");
        assert_eq!(convert_case("ﬁ", None, CaseMap::Upper, true, true), "ﬁ");
        assert_eq!(convert_case("İ", None, CaseMap::Lower, true, true), "i");
        assert_eq!(convert_case("ÉCOLE", None, CaseMap::Lower, true, true), "école");
        // Outside a UTF-8 locale only ASCII has a case.
        assert_eq!(convert_case("ÉCOLE", None, CaseMap::Lower, true, false), "École");
    }
}

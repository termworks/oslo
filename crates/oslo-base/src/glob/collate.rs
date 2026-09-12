//! Ordering glob results the way bash does in the user's locale.
//!
//! bash sorts a match list with `strcoll`, so in `en_US.UTF-8` it reads `9num a1 a10 abc Abc ABC
//! -dash dir1 émile _under Zed` — punctuation ignored at first, accents folded, lowercase first —
//! and only in `C` is the order plain bytes. oslo's release is a static musl binary, and musl's
//! `strcoll` *is* byte order in every locale, so calling it would change nothing where it matters.
//! This is the ISO 14651 shape glibc uses, written out once, so both builds sort alike.
//!
//! # The comparison, level by level
//!
//! ```text
//!   1  letters and digits only, accents stripped, case folded      dash < dir1 < emile
//!   2  accents                                                     emile < émile
//!   3  case, lowercase first                                       abc < Abc < ABC
//!   4  the bytes, punctuation and all                              the last word on a tie
//! ```

use std::cmp::Ordering;

/// Accented Latin letters and the letter each folds to. The position in its set is the accent's
/// rank, so the plain letter (rank 0) always sorts first.
const FOLDS: &[(&str, char)] = &[
    ("àáâãäåāăą", 'a'),
    ("çćĉċč", 'c'),
    ("ďđ", 'd'),
    ("èéêëēĕėęě", 'e'),
    ("ĝğġģ", 'g'),
    ("ĥħ", 'h'),
    ("ìíîïĩīĭįı", 'i'),
    ("ĵ", 'j'),
    ("ķ", 'k'),
    ("ĺļľŀł", 'l'),
    ("ñńņňŉ", 'n'),
    ("òóôõöøōŏő", 'o'),
    ("ŕŗř", 'r'),
    ("śŝşš", 's'),
    ("ţťŧ", 't'),
    ("ùúûüũūŭůűų", 'u'),
    ("ŵ", 'w'),
    ("ýÿŷ", 'y'),
    ("źżž", 'z'),
];

/// Whether the environment's collating locale sorts by anything but bytes.
///
/// `LC_ALL`, then `LC_COLLATE`, then `LANG`, the precedence every POSIX program uses. `C`,
/// `POSIX` and `C.UTF-8` collate by code point, which for UTF-8 is byte order.
pub fn locale_collates() -> bool {
    let name = ["LC_ALL", "LC_COLLATE", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    !(name.is_empty() || name == "C" || name == "POSIX" || name.starts_with("C."))
}

/// Sort `paths` in place: by the locale's rules when `collate`, by bytes otherwise.
pub fn sort(paths: &mut [String], collate: bool) {
    if collate {
        paths.sort_by_cached_key(|path| Key::of(path));
    } else {
        paths.sort_unstable();
    }
}

/// The four levels of one string, compared in field order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    letters: Vec<char>,
    accents: Vec<u8>,
    cases: Vec<u8>,
    bytes: Vec<u8>,
}

impl Key {
    fn of(text: &str) -> Key {
        let mut key = Key {
            letters: Vec::new(),
            accents: Vec::new(),
            cases: Vec::new(),
            bytes: text.as_bytes().to_vec(),
        };
        for ch in text.chars().filter(|ch| ch.is_alphanumeric()) {
            let (letter, accent) = fold(ch);
            key.letters.push(letter);
            key.accents.push(accent);
            key.cases.push(u8::from(ch.is_uppercase()));
        }
        key
    }
}

/// A character's base letter, lowercased, and its accent rank.
pub(crate) fn fold(ch: char) -> (char, u8) {
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    FOLDS
        .iter()
        .find_map(|(set, base)| {
            set.chars()
                .position(|c| c == lower)
                .map(|at| (*base, at as u8 + 1))
        })
        .unwrap_or((lower, 0))
}

/// Compare two strings the way [`sort`] orders them with `collate` on.
pub fn compare(a: &str, b: &str) -> Ordering {
    Key::of(a).cmp(&Key::of(b))
}

#[cfg(test)]
mod tests {
    use super::sort;

    /// Each list is GNU bash 5.3.9's own order in `en_US.UTF-8`, and is sorted from reversed.
    fn bash_order(expected: &[&str]) {
        let mut got: Vec<String> = expected.iter().rev().map(|s| s.to_string()).collect();
        sort(&mut got, true);
        assert_eq!(got, expected);
    }

    #[test]
    fn names_sort_like_bash_in_en_us() {
        bash_order(&[
            "9num",
            "a1",
            "a10",
            "a2",
            "abc",
            "Abc",
            "ABC",
            "b1",
            "B1",
            "br[ack]et",
            "c.txt",
            "dangling",
            "-dash",
            "dir1",
            "dir2",
            "d.txt",
            "émile",
            "emptydir",
            "e.TXT",
            "flink",
            "f.md",
            "lnk",
            "loop",
            "q?mark",
            "sp ace",
            "sp dir",
            "star*name",
            "tab",
            "two  sp",
            "_under",
            "ünï",
            "x]y",
            "Zed",
        ]);
    }

    #[test]
    fn hidden_names_and_paths_sort_like_bash() {
        bash_order(&[".hdir", ".hid2", ".hidden", ".hid txt"]);
        bash_order(&[
            "c.txt",
            "dir1/f1.txt",
            "dir1/sub1/deep1/f3.txt",
            "dir1/sub1/f2.txt",
            "dir2/sub2/g2.txt",
            "d.txt",
            ".hdir/hsub/h2.txt",
            ".hdir/h.txt",
            "sp dir/in.txt",
        ]);
    }

    #[test]
    fn an_accent_sorts_after_its_plain_letter() {
        bash_order(&["emile", "émile"]);
    }

    #[test]
    fn without_collation_the_order_is_bytes() {
        let mut got = vec!["b".to_string(), "B".to_string(), "-a".to_string()];
        sort(&mut got, false);
        assert_eq!(got, ["-a", "B", "b"]);
    }
}

//! Anchored shell-pattern matching for the `${v#pat}` / `${v%pat}` / `${v/pat/rep}` operators.
//!
//! The operators these back are *pattern* operators, not substring searches. `${p##*/}` is
//! basename and `${f%.*}` drops an extension only because `*` is a metacharacter and the match is
//! anchored to one end of the value. An earlier implementation used `str::find`/`rfind` on the raw
//! pattern text, which made both idioms silently return the wrong string — the failure mode this
//! module exists to prevent.
//!
//! Every entry point works in *characters*, never bytes, so a multi-byte value cannot be cut
//! mid-character.

use crate::expand::glob::ShellPattern;
use crate::expand::word::{Origin, Run};

/// Every index at which `s` may be cut, ascending: each character boundary plus the end.
fn cuts(s: &str) -> Vec<usize> {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .collect()
}

/// `${v#pat}` (`longest == false`) and `${v##pat}`.
///
/// Tests every prefix of `value`; the shortest form takes the first one that matches, the longest
/// form the last. The empty prefix is a candidate, so `${v#}` and a non-matching pattern both
/// leave the value alone.
pub fn remove_prefix(value: &str, pattern: &ShellPattern, longest: bool) -> String {
    let mut candidates = cuts(value);
    if longest {
        candidates.reverse();
    }
    for cut in candidates {
        if pattern.matches(&value[..cut]) {
            return value[cut..].to_string();
        }
    }
    value.to_string()
}

/// `${v%pat}` (`longest == false`) and `${v%%pat}`.
///
/// The mirror image of [`remove_prefix`]: a *short* suffix starts at a *high* index, so the
/// shortest form walks the cut points from the end. Getting this direction backwards is what made
/// `v=abcabc; ${v%abc}` yield `""` instead of `abc`.
pub fn remove_suffix(value: &str, pattern: &ShellPattern, longest: bool) -> String {
    let mut candidates = cuts(value);
    if !longest {
        candidates.reverse();
    }
    for cut in candidates {
        if pattern.matches(&value[cut..]) {
            return value[..cut].to_string();
        }
    }
    value.to_string()
}

/// The longest match of `pattern` starting exactly at byte index `start`, as an end index.
///
/// **Three shapes, because the general one is cubic when it runs at every position.** This is
/// called once per character by [`replace`], and it used to build a vector of every cut point in
/// the remaining text and then test every prefix against it: `${v//a/b}` on 250 bytes took 45 ms,
/// on 2 KB eleven seconds, and on 20 KB it did not finish. bash answers 20 KB instantly.
///
/// * A pattern of **fixed width** — anything without `*` — consumes exactly as many characters as
///   it has items, so there is one end worth testing rather than every end from the longest down.
/// * The **general** case still walks the ends, but from an iterator rather than an allocation.
///
/// The third shape, a pattern that is only literal text, never reaches here at all: [`replace`]
/// answers it with a substring search.
fn match_at(value: &str, start: usize, pattern: &ShellPattern) -> Option<usize> {
    let rest = &value[start..];
    if let Some(width) = pattern.fixed_chars() {
        let end = match width {
            0 => 0,
            _ => rest
                .char_indices()
                .nth(width)
                .map_or(rest.len(), |(i, _)| i),
        };
        // Short of `width` characters left means no match, not a shorter one.
        if rest[..end].chars().count() != width {
            return None;
        }
        return pattern.matches(&rest[..end]).then_some(start + end);
    }
    // Longest first: the end of the text, then each character boundary descending.
    std::iter::once(rest.len())
        .chain(rest.char_indices().rev().map(|(at, _)| at))
        .find(|&end| pattern.matches(&rest[..end]))
        .map(|end| start + end)
}

/// The replacement text of a `${v/pat/rep}`, with `&` standing for whatever matched.
///
/// **`&` is the matched text unless it was quoted.** `v=abc; ${v//?/[&]}` is `[a][b][c]` in bash,
/// and used to be `[&][&][&]` here — silently, because the escaped spelling `\&` already agreed.
/// The distinction cannot be drawn on the finished string: by then `\&` and `&` are both one byte.
/// So the replacement arrives as [`Run`]s and quoting is read off their [`Origin`], which is the
/// same information the pattern side already uses to decide whether a `*` globs.
///
/// Text from an *unquoted* expansion counts as unquoted, which is bash's rule and not an accident
/// of it: `r='&'; ${v/b/$r}` substitutes the match, while `${v/b/"$r"}` and `${v/b/'&'}` do not.
pub struct Replacement {
    pieces: Vec<Piece>,
}

enum Piece {
    Text(String),
    /// Whatever the pattern matched, spliced in where the `&` was.
    Matched,
}

impl Replacement {
    /// Read the runs of an expanded replacement word into a template.
    pub fn from_runs(runs: &[Run]) -> Replacement {
        let mut pieces = Vec::new();
        for run in runs {
            if run.origin == Origin::Quoted {
                push_text(&mut pieces, &run.text);
                continue;
            }
            for (i, between) in run.text.split('&').enumerate() {
                if i > 0 {
                    pieces.push(Piece::Matched);
                }
                push_text(&mut pieces, between);
            }
        }
        Replacement { pieces }
    }

    fn render(&self, matched: &str, out: &mut String) {
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Matched => out.push_str(matched),
            }
        }
    }
}

/// Append to the trailing text piece rather than adding an empty or a second one.
fn push_text(pieces: &mut Vec<Piece>, text: &str) {
    if text.is_empty() {
        return;
    }
    match pieces.last_mut() {
        Some(Piece::Text(last)) => last.push_str(text),
        _ => pieces.push(Piece::Text(text.to_string())),
    }
}

/// `${v/pat/rep}` — replace the leftmost match, or every match when `all`.
///
/// Matching is longest-at-each-position, left to right. A zero-length match is ignored rather
/// than replaced — `v=ab; ${v//x*/-}` is `ab` in bash, not `-a-b-` — which also keeps the scan
/// from standing still.
pub fn replace(
    value: &str,
    pattern: &ShellPattern,
    replacement: &Replacement,
    all: bool,
) -> String {
    // **A pattern with nothing in it but text is a substring search.** `${v//a/b}` is the commonest
    // replacement there is and has no pattern in it; answering it by testing every prefix at every
    // position is what made this cubic. `str::find` is the question that was actually asked.
    if let Some(text) = pattern.literal().filter(|text| !text.is_empty()) {
        return substituted(value, text, replacement, all);
    }
    let mut out = String::new();
    let mut pos = 0;
    let mut replaced = false;
    while pos < value.len() {
        let hit = if all || !replaced {
            match_at(value, pos, pattern).filter(|&end| end > pos)
        } else {
            None
        };
        if let Some(end) = hit {
            replacement.render(&value[pos..end], &mut out);
            pos = end;
            replaced = true;
            continue;
        }
        // No usable match here: carry one character across and move on.
        let ch = value[pos..].chars().next().expect("pos is a char boundary");
        out.push(ch);
        pos += ch.len_utf8();
    }
    out
}

/// Replace occurrences of plain `text`, which is a search rather than a match.
///
/// Linear in the value: `find` scans forward from where the last one ended and never looks at a
/// position twice.
fn substituted(value: &str, text: &str, replacement: &Replacement, all: bool) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find(text) {
        out.push_str(&rest[..at]);
        replacement.render(text, &mut out);
        rest = &rest[at + text.len()..];
        if !all {
            break;
        }
    }
    out.push_str(rest);
    out
}

/// `${v/#pat/rep}` — replace only a match anchored at the start, longest first.
pub fn replace_prefix(value: &str, pattern: &ShellPattern, replacement: &Replacement) -> String {
    match match_at(value, 0, pattern) {
        Some(end) => {
            let mut out = String::new();
            replacement.render(&value[..end], &mut out);
            out.push_str(&value[end..]);
            out
        }
        None => value.to_string(),
    }
}

/// `${v/%pat/rep}` — replace only a match anchored at the end, longest first.
pub fn replace_suffix(value: &str, pattern: &ShellPattern, replacement: &Replacement) -> String {
    for cut in cuts(value) {
        if pattern.matches(&value[cut..]) {
            let mut out = value[..cut].to_string();
            replacement.render(&value[cut..], &mut out);
            return out;
        }
    }
    value.to_string()
}

/// `${v:offset:length}` over characters, with bash's negative-index rules.
///
/// A negative offset counts back from the end; a negative length names an end position rather
/// than a count. `Err` carries the offending length for the caller to report, because bash makes
/// `${v:2:-9}` a fatal expansion error rather than an empty string.
pub fn substring(value: &str, offset: i64, length: Option<i64>) -> Result<String, i64> {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len() as i64;

    let start = if offset < 0 {
        // Still negative after counting back from the end means the window starts before the
        // string and bash yields nothing at all.
        let from_end = len + offset;
        if from_end < 0 {
            return Ok(String::new());
        }
        from_end
    } else {
        offset.min(len)
    };

    let end = match length {
        None => len,
        Some(n) if n < 0 => {
            let e = len + n;
            if e < start {
                return Err(n);
            }
            e
        }
        Some(n) => (start + n).min(len),
    };

    Ok(chars[start as usize..end.max(start) as usize]
        .iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests spell patterns as source text, so every metacharacter in them is meant as one.
    fn pat(text: &str) -> ShellPattern {
        ShellPattern::from_unquoted(text)
    }

    /// Replacement text with no `&` in it, which is every case in this module bar the ones that
    /// test `&` itself — those live in `tests/corpus/param_ampersand_and_element.sh`, against bash.
    fn rep(text: &str) -> Replacement {
        Replacement::from_runs(&[Run::new(text, Origin::Quoted)])
    }

    #[test]
    fn prefix_removal_is_anchored_and_globs() {
        let p = "/usr/local/lib/libfoo.so";
        assert_eq!(remove_prefix(p, &pat("*/"), true), "libfoo.so");
        assert_eq!(
            remove_prefix(p, &pat("*/"), false),
            "usr/local/lib/libfoo.so"
        );
        assert_eq!(
            remove_prefix(p, &pat("/usr"), false),
            "/local/lib/libfoo.so"
        );
        assert_eq!(remove_prefix(p, &pat("*."), true), "so");
        // Unanchored substring search would have stripped nothing here.
        assert_eq!(remove_prefix("abcabc", &pat("abc"), false), "abc");
        assert_eq!(remove_prefix("abcabc", &pat("abc"), true), "abc");
        assert_eq!(remove_prefix("abc", &pat("x"), true), "abc");
        assert_eq!(remove_prefix("abc", &pat(""), false), "abc");
    }

    #[test]
    fn suffix_removal_picks_the_right_length() {
        assert_eq!(
            remove_suffix("archive.tar.gz", &pat(".*"), false),
            "archive.tar"
        );
        assert_eq!(remove_suffix("archive.tar.gz", &pat(".*"), true), "archive");
        let p = "/usr/local/lib/libfoo.so";
        assert_eq!(remove_suffix(p, &pat("/*"), false), "/usr/local/lib");
        assert_eq!(remove_suffix(p, &pat("/*"), true), "");
        // The regression that started this: shortest-suffix must leave the first `abc` standing.
        assert_eq!(remove_suffix("abcabc", &pat("abc"), false), "abc");
        assert_eq!(remove_suffix("abcabc", &pat("abc"), true), "abc");
        assert_eq!(remove_suffix("abc", &pat("x"), false), "abc");
    }

    /// The quoting a pattern arrived with is what the operator matches by: `${v#"a*"}` strips a
    /// literal `a*` and leaves `axc` alone.
    #[test]
    fn a_quoted_metacharacter_is_a_character() {
        use crate::expand::word::{Origin, Run};
        let literal = crate::expand::glob::pattern_from_runs(&[Run::new("a*", Origin::Quoted)]);
        assert_eq!(remove_prefix("a*c", &literal, false), "c");
        assert_eq!(remove_prefix("axc", &literal, false), "axc");
    }

    #[test]
    fn replacement_honours_scope() {
        assert_eq!(replace("a-b-c", &pat("-"), &rep("+"), false), "a+b-c");
        assert_eq!(replace("a-b-c", &pat("-"), &rep("+"), true), "a+b+c");
        assert_eq!(
            replace("one.two.three", &pat("."), &rep(" "), true),
            "one two three"
        );
        assert_eq!(replace_prefix("a-b-c", &pat("a"), &rep("A")), "A-b-c");
        assert_eq!(replace_prefix("a-b-c", &pat("b"), &rep("B")), "a-b-c");
        assert_eq!(replace_suffix("a-b-c", &pat("c"), &rep("C")), "a-b-C");
        assert_eq!(replace_suffix("a-b-c", &pat("b"), &rep("B")), "a-b-c");
    }

    /// A pattern that matches only the empty string replaces nothing, and does not spin.
    #[test]
    fn an_empty_match_is_ignored() {
        assert_eq!(replace("ab", &pat("x*"), &rep("-"), true), "ab");
        assert_eq!(replace("ab", &pat("x*"), &rep("-"), false), "ab");
        assert_eq!(replace("ab", &pat(""), &rep("-"), true), "ab");
        // The anchored forms do accept one, which is how `${v/#/X}` prepends.
        assert_eq!(replace_prefix("ab", &pat(""), &rep("X")), "Xab");
        assert_eq!(replace_suffix("ab", &pat(""), &rep("X")), "abX");
    }

    /// Longest-at-each-position, not shortest: `*` eats the rest of the value in one match.
    #[test]
    fn replacement_takes_the_longest_match_at_each_position() {
        assert_eq!(replace("aaa", &pat("a*"), &rep("X"), true), "X");
        assert_eq!(replace("abc", &pat("?"), &rep("X"), true), "XXX");
    }

    #[test]
    fn substring_handles_negative_indices() {
        let v = "abcdefgh";
        assert_eq!(substring(v, 2, Some(3)), Ok("cde".into()));
        assert_eq!(substring(v, 2, None), Ok("cdefgh".into()));
        assert_eq!(substring(v, 0, Some(1)), Ok("a".into()));
        assert_eq!(substring(v, -3, None), Ok("fgh".into()));
        assert_eq!(substring(v, -3, Some(2)), Ok("fg".into()));
        assert_eq!(substring(v, -20, None), Ok(String::new()));
        assert_eq!(substring(v, 20, None), Ok(String::new()));
        assert_eq!(substring(v, 2, Some(-3)), Ok("cde".into()));
        assert_eq!(substring(v, 2, Some(0)), Ok(String::new()));
        assert_eq!(substring(v, 2, Some(-9)), Err(-9));
    }

    /// Characters, not bytes: cutting a multi-byte value by byte index would panic.
    #[test]
    fn substring_counts_characters() {
        assert_eq!(substring("héllo", 1, Some(2)), Ok("él".into()));
        assert_eq!(remove_prefix("héllo", &pat("h?"), false), "llo");
    }
}

/// **The replacement operators are linear enough to use.**
///
/// `match_at` used to build a vector of every cut point in the remaining text and test every prefix
/// against the pattern, at every position — cubic. Measured before this: `${v//a/b}` took 45 ms on
/// 250 bytes, 208 ms on 500, 1.4 s on 1 KB, **eleven seconds on 2 KB**, and 20 KB did not finish
/// inside ten minutes. bash answers 20 KB instantly.
///
/// A wall-clock bound rather than a growth-rate check: the numbers involved are three orders of
/// magnitude apart, so a generous ceiling on a debug build still fails loudly if the cubic
/// behaviour comes back, and cannot fail for being run on a slow machine.
#[cfg(test)]
mod scale_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn pat(text: &str) -> ShellPattern {
        ShellPattern::from_unquoted(text)
    }

    fn rep(text: &str) -> Replacement {
        Replacement::from_runs(&[Run::new(text, Origin::Quoted)])
    }

    #[track_caller]
    fn within(limit: Duration, what: &str, run: impl FnOnce()) {
        let start = Instant::now();
        run();
        let took = start.elapsed();
        assert!(took < limit, "{what} took {took:?}, over {limit:?}");
    }

    #[test]
    fn replacing_in_a_large_value_does_not_take_seconds() {
        let value = "a".repeat(20_000);

        // A literal pattern is a substring search.
        within(Duration::from_secs(2), "a literal pattern", || {
            assert_eq!(replace(&value, &pat("a"), &rep("b"), true).len(), 20_000);
        });
        // A fixed-width pattern tests one end per position rather than every end.
        within(Duration::from_secs(2), "a fixed-width pattern", || {
            assert_eq!(replace(&value, &pat("?"), &rep("b"), true).len(), 20_000);
        });
        within(Duration::from_secs(2), "a class", || {
            assert_eq!(replace(&value, &pat("[ab]"), &rep("b"), true).len(), 20_000);
        });
        // And a starred one still leaves on the first match.
        within(Duration::from_secs(2), "a starred pattern", || {
            assert_eq!(replace(&value, &pat("a*"), &rep("X"), true), "X");
        });
    }

    /// The prefix and suffix operators run once per value rather than per position, but they walk
    /// the same cut points — the idioms every script uses are on this path.
    #[test]
    fn trimming_a_large_value_does_not_take_seconds() {
        let path = format!("/{}/name.ext", "dir/".repeat(4_000));
        within(Duration::from_secs(2), "basename and extension", || {
            assert_eq!(remove_prefix(&path, &pat("*/"), true), "name.ext");
            assert_eq!(
                remove_suffix(&path, &pat(".*"), false).len(),
                path.len() - 4
            );
        });
    }
}

//! Extended patterns — bash's `extglob`: `?(a|b)` `*(a|b)` `+(a|b)` `@(a|b)` `!(a|b)`.
//!
//! `parse` turns one group into an [`Item::Ext`] while `compile_items` runs, and `matches`
//! answers for any pattern holding one, because alternation is the one thing the linear matcher —
//! which only ever backtracks to its last `*` — cannot do.
//!
//! **Memoised, so it stays polynomial.** Plain backtracking on `+(a|aa)+(a|aa)b` against a long run
//! of `a`s tries every way of splitting the run and is exponential. Every question asked here is
//! "does this piece of the pattern match exactly this span of the name", and there are only so many
//! of those, so each is answered once.

use super::{Item, compile_items_with};
use std::collections::HashMap;

/// Which of the five operators a group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtKind {
    /// `?(…)`: zero or one of them.
    ZeroOrOne,
    /// `*(…)`: zero or more.
    ZeroOrMore,
    /// `+(…)`: one or more.
    OneOrMore,
    /// `@(…)`: exactly one.
    One,
    /// `!(…)`: anything none of them matches.
    Not,
}

impl ExtKind {
    fn of(ch: char) -> Option<Self> {
        match ch {
            '?' => Some(Self::ZeroOrOne),
            '*' => Some(Self::ZeroOrMore),
            '+' => Some(Self::OneOrMore),
            '@' => Some(Self::One),
            '!' => Some(Self::Not),
            _ => None,
        }
    }
}

/// The group opening at `start` — its operator, then `(` — and the index just past its `)`.
///
/// `None` when there is no group there or it is never closed, so the caller reads the characters
/// as themselves, as it does an unclosed `[`. Only unquoted `(`, `|` and `)` are structure:
/// `@(a"|"b)` is one alternative containing a `|`.
pub(super) fn parse(chars: &[(char, bool)], start: usize) -> Option<(Item, usize)> {
    let Some(&(op, true)) = chars.get(start) else {
        return None;
    };
    let kind = ExtKind::of(op)?;
    if chars.get(start + 1) != Some(&('(', true)) {
        return None;
    }
    let mut alts = Vec::new();
    let (mut from, mut depth, mut i) = (start + 2, 0usize, start + 2);
    while let Some(&(ch, active)) = chars.get(i) {
        if active {
            match ch {
                '\\' => i += 1,
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                ')' => {
                    alts.push(compile_items_with(&chars[from..i], true).0);
                    return Some((Item::Ext { kind, alts }, i + 1));
                }
                '|' if depth == 0 => {
                    alts.push(compile_items_with(&chars[from..i], true).0);
                    from = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Whether the whole of `name` matches `items`, which hold at least one group.
pub(super) fn matches(items: &[Item], name: &[char], nocase: bool) -> bool {
    Matcher {
        name,
        nocase,
        memo: HashMap::new(),
    }
    .seq(items, 0, 0, name.len())
}

struct Matcher<'a> {
    name: &'a [char],
    nocase: bool,
    /// (which item list, index into it, span start, span end, repeating a group) → the answer.
    memo: HashMap<(usize, usize, usize, usize, bool), bool>,
}

impl Matcher<'_> {
    /// Whether `items[i..]` matches exactly `name[j..end]`.
    fn seq(&mut self, items: &[Item], i: usize, j: usize, end: usize) -> bool {
        let key = (items.as_ptr() as usize, i, j, end, false);
        self.remembered(key, |m| m.step(items, i, j, end))
    }

    fn remembered(
        &mut self,
        key: (usize, usize, usize, usize, bool),
        answer: impl FnOnce(&mut Self) -> bool,
    ) -> bool {
        if let Some(&hit) = self.memo.get(&key) {
            return hit;
        }
        let hit = answer(self);
        self.memo.insert(key, hit);
        hit
    }

    fn step(&mut self, items: &[Item], i: usize, j: usize, end: usize) -> bool {
        let Some(item) = items.get(i) else {
            return j == end;
        };
        match item {
            Item::Star => (j..=end).any(|k| self.seq(items, i + 1, k, end)),
            Item::Ext { kind, alts } => match kind {
                ExtKind::One => {
                    (j..=end).any(|k| self.any(alts, j, k) && self.seq(items, i + 1, k, end))
                }
                ExtKind::ZeroOrOne => {
                    self.seq(items, i + 1, j, end)
                        || (j..=end).any(|k| self.any(alts, j, k) && self.seq(items, i + 1, k, end))
                }
                ExtKind::ZeroOrMore => self.more(items, i, j, end),
                ExtKind::OneOrMore => {
                    (j..=end).any(|k| self.any(alts, j, k) && self.more(items, i, k, end))
                }
                ExtKind::Not => {
                    (j..=end).any(|k| !self.any(alts, j, k) && self.seq(items, i + 1, k, end))
                }
            },
            single => {
                j < end
                    && single.matches_char_with(self.name[j], self.nocase)
                    && self.seq(items, i + 1, j + 1, end)
            }
        }
    }

    /// Zero or more further repetitions of the group at `items[i]`, then the rest of `items`.
    ///
    /// A repetition must consume something, or `*(a|)` would repeat an empty match forever.
    fn more(&mut self, items: &[Item], i: usize, j: usize, end: usize) -> bool {
        let Some(Item::Ext { alts, .. }) = items.get(i) else {
            return false;
        };
        let key = (items.as_ptr() as usize, i, j, end, true);
        self.remembered(key, |m| {
            m.seq(items, i + 1, j, end)
                || (j + 1..=end).any(|k| m.any(alts, j, k) && m.more(items, i, k, end))
        })
    }

    /// Whether any alternative matches exactly `name[j..k]`.
    fn any(&mut self, alts: &[Vec<Item>], j: usize, k: usize) -> bool {
        alts.iter().any(|alt| self.seq(alt, 0, j, k))
    }
}

#[cfg(test)]
mod tests {
    use crate::glob::ShellPattern;

    fn ext(pattern: &str) -> ShellPattern {
        let chars: Vec<(char, bool)> = pattern.chars().map(|c| (c, true)).collect();
        ShellPattern::from_chars_with(&chars, true)
    }

    /// Each operator, against what bash 5.3 answers for it.
    #[test]
    fn the_five_operators_match_as_bash_does() {
        let cases = [
            ("@(ab|c)", "ab", true),
            ("@(ab|c)", "abc", false),
            ("?(a)b", "b", true),
            ("?(a)b", "aab", false),
            ("*(a)b", "aaab", true),
            ("*(a)b", "b", true),
            ("+(a)b", "b", false),
            ("+(a|bc)d", "abcad", true),
            ("!(x*)", "abc", true),
            ("!(*.txt)", "a.txt", false),
            ("!(*.txt)", "a.md", true),
            ("*.@(txt|md)", "notes.md", true),
            ("*.@(txt|md)", "notes.rs", false),
            ("@(a|+(b))c", "bbbc", true),
        ];
        for (pattern, name, want) in cases {
            assert_eq!(ext(pattern).matches(name), want, "{pattern} against {name}");
        }
    }

    /// A quoted `|` is data, and a group left open is literal text.
    #[test]
    fn only_unquoted_structure_counts() {
        let chars = [
            ('@', true),
            ('(', true),
            ('a', true),
            ('|', false),
            ('b', true),
            (')', true),
        ];
        let quoted_bar = ShellPattern::from_chars_with(&chars, true);
        assert!(quoted_bar.matches("a|b"));
        assert!(!quoted_bar.matches("a"));
        assert!(ext("@(ab").matches("@(ab"));
    }

    /// Without `extglob` the characters are themselves.
    #[test]
    fn off_it_is_plain_text() {
        let chars: Vec<(char, bool)> = "@(a|b)".chars().map(|c| (c, true)).collect();
        let off = ShellPattern::from_chars_with(&chars, false);
        assert!(off.matches("@(a|b)"));
        assert!(!off.matches("a"));
    }

    /// The pattern that is exponential without the memo finishes at once with it.
    #[test]
    fn a_pathological_pattern_is_not_exponential() {
        let started = std::time::Instant::now();
        let name = "a".repeat(200);
        assert!(!ext("+(a|aa)+(a|aa)b").matches(&name));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}

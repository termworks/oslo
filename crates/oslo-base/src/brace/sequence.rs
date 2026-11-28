//! Sequence expressions: `{1..5}`, `{01..10}`, `{a..e}`, `{0..10..3}`.
//!
//! The other half of a brace group. A comma list says what its alternatives are; a sequence
//! expression *computes* them from two endpoints and an optional step, which makes it the only
//! part of brace expansion that can be asked for more than it can deliver — `{1..999999999}` is
//! one typo away from `{1..9}`.
//!
//! Split out from the group syntax next door because the two answer different questions. That
//! file decides which characters are brace syntax and how groups combine; this one decides only
//! what a pair of endpoints denotes, and hands back the items as [`Atom`]s for the group machinery
//! to splice.

use super::{Atom, SEQUENCE_LIMIT};

/// Expand `{n..m}`, `{n..m..step}` and their single-character form `{a..e}`.
pub(super) fn sequence_alternatives(inner: &[Atom]) -> Option<Vec<Vec<Atom>>> {
    // A sequence expression is pure text; a `${x}` anywhere in it means this is not one.
    let text = inner
        .iter()
        .map(|a| match a {
            Atom::Raw(c) => Some(*c),
            Atom::Opaque(_) => None,
        })
        .collect::<Option<String>>()?;

    let mut fields = text.split("..");
    let start = fields.next()?;
    let end = fields.next()?;
    let step = fields.next();
    if fields.next().is_some() {
        return None;
    }
    let step = match step {
        // bash takes the magnitude and gets direction from the endpoints, so `{1..5..-2}` counts
        // up; a zero step is read as one rather than as a sequence that never terminates.
        Some(s) => s.parse::<i64>().ok()?.checked_abs()?.max(1),
        None => 1,
    };

    let items = numeric_sequence(start, end, step).or_else(|| char_sequence(start, end, step))?;
    Some(
        items
            .into_iter()
            .map(|s| s.chars().map(item_atom).collect())
            .collect(),
    )
}

/// One character of a generated item. `{A..z}` passes `\` and `` ` ``, which the lexer that runs
/// after this pass would read as syntax: the backquote goes out escaped, and the backslash as the
/// empty word bash's quote removal leaves of it.
fn item_atom(c: char) -> Atom {
    match c {
        '\\' => Atom::Opaque("''".to_string()),
        '`' => Atom::Opaque("\\`".to_string()),
        c => Atom::Raw(c),
    }
}

/// Whether an operand asks for zero padding, i.e. it has a leading zero of its own.
///
/// `{01..10}` is `01 02 ... 10`; `{1..10}` is not padded.
fn is_zero_padded(s: &str) -> bool {
    let digits = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    digits.len() > 1 && digits.starts_with('0')
}

fn numeric_sequence(start: &str, end: &str, step: i64) -> Option<Vec<String>> {
    let from: i64 = start.parse().ok()?;
    let to: i64 = end.parse().ok()?;
    // bash counts the sign into the field width: `{-01..1}` is `-01 000 001`.
    let width = if is_zero_padded(start) || is_zero_padded(end) {
        start.len().max(end.len())
    } else {
        0
    };

    let count = (i128::from(to) - i128::from(from)).abs() / i128::from(step) + 1;
    if count > SEQUENCE_LIMIT {
        return None;
    }

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let v = if from <= to {
            i128::from(from) + i * i128::from(step)
        } else {
            i128::from(from) - i * i128::from(step)
        };
        out.push(format!("{v:0width$}"));
    }
    Some(out)
}

fn char_sequence(start: &str, end: &str, step: i64) -> Option<Vec<String>> {
    let from = i64::from(single_ascii(start)?);
    let to = i64::from(single_ascii(end)?);

    // Counting first keeps the arithmetic bounded: the largest multiple of `step` used is at most
    // the distance between the endpoints, so an absurd step just yields the single first item.
    let count = (to - from).abs() / step + 1;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let c = if from <= to {
            from + i * step
        } else {
            from - i * step
        };
        out.push(char::from(u8::try_from(c).ok()?).to_string());
    }
    Some(out)
}

/// A sequence endpoint written as one ASCII letter, as in `{a..z}`.
///
/// Letters only: `{1..z}` is not a range in bash either, and reading it as one would turn a
/// mistyped numeric range into a stream of punctuation.
fn single_ascii(s: &str) -> Option<u8> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_alphabetic() {
        return None;
    }
    Some(c as u8)
}

#[cfg(test)]
mod tests {
    use crate::brace::expand_braces_text as expand;

    #[test]
    fn numeric_ranges_count_both_ways() {
        assert_eq!(expand("{1..4}"), vec!["1", "2", "3", "4"]);
        assert_eq!(expand("{4..1}"), vec!["4", "3", "2", "1"]);
        assert_eq!(expand("{-2..2}"), vec!["-2", "-1", "0", "1", "2"]);
    }

    #[test]
    fn range_step_is_a_magnitude() {
        assert_eq!(expand("{0..10..3}"), vec!["0", "3", "6", "9"]);
        // A negative step is not a direction: the endpoints already gave one.
        assert_eq!(expand("{1..5..-2}"), vec!["1", "3", "5"]);
        // bash reads a zero step as one, rather than as a group it refuses to expand.
        assert_eq!(expand("{1..3..0}"), vec!["1", "2", "3"]);
    }

    #[test]
    fn leading_zero_pads_the_whole_range() {
        assert_eq!(expand("{08..11}"), vec!["08", "09", "10", "11"]);
        assert_eq!(expand("{-01..1}"), vec!["-01", "000", "001"]);
    }

    #[test]
    fn character_ranges_walk_the_alphabet() {
        assert_eq!(expand("{a..e}"), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(expand("{e..a..2}"), vec!["e", "c", "a"]);
        // Between the cases sit six punctuation characters; two would be syntax to the lexer.
        assert_eq!(
            expand("{Z..a}"),
            vec!["Z", "[", "''", "]", "^", "_", "\\`", "a"]
        );
    }

    #[test]
    fn malformed_ranges_stay_literal() {
        assert_eq!(expand("{1...5}"), vec!["{1...5}"]);
        assert_eq!(expand("{1..z}"), vec!["{1..z}"]);
        assert_eq!(expand("{a..b..c}"), vec!["{a..b..c}"]);
        // Refusing an absurd range is the same answer as any other unparsable group.
        assert_eq!(expand("{1..99999999}"), vec!["{1..99999999}"]);
    }
}

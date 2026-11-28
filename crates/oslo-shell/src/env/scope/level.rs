//! `$SHLVL`, moved up at startup and down by `exec` the way bash moves it.

use super::Environment;

/// The level `value` moves to by `change`: anything that is not a decimal number counts as 0, the
/// level stops at 0, and one of 1000 or more warns and starts again at 1.
pub(super) fn next_level(value: Option<&str>, change: i64) -> i64 {
    let level = value.and_then(number).unwrap_or(0).saturating_add(change);
    if level >= 1000 {
        eprintln!(
            "{}warning: shell level ({level}) too high, resetting to 1",
            super::origin::origin_now()
        );
        return 1;
    }
    level.max(0)
}

/// bash's `legal_number`: blanks around an optionally signed run of decimal digits.
fn number(text: &str) -> Option<i64> {
    let digits = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let unsigned = digits.strip_prefix(['+', '-']).unwrap_or(digits);
    if unsigned.is_empty() || !unsigned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl Environment {
    /// Move `$SHLVL` by `change` and export it, as `exec` does on its way to a program.
    pub(crate) fn adjust_shell_level(&mut self, change: i64) {
        let level = next_level(self.get_var("SHLVL"), change);
        self.set_var("SHLVL", &level.to_string(), true);
    }
}

#[cfg(test)]
mod tests {
    use super::{next_level, number};

    #[test]
    fn a_level_reads_as_bash_reads_a_number() {
        assert_eq!(number(" 7 "), Some(7));
        assert_eq!(number("+3"), Some(3));
        assert_eq!(number("010"), Some(10));
        assert_eq!(number("0x10"), None);
        assert_eq!(number(""), None);
        assert_eq!(number("-"), None);
    }

    #[test]
    fn a_level_stops_at_zero_and_wraps_past_999() {
        assert_eq!(next_level(Some("-5"), 1), 0);
        assert_eq!(next_level(Some("abc"), 1), 1);
        assert_eq!(next_level(None, -1), 0);
        assert_eq!(next_level(Some("998"), 1), 999);
        assert_eq!(next_level(Some("999"), 1), 1);
    }
}

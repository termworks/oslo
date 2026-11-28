//! Turning a value back into shell source.
//!
//! Every listing builtin promises that its output can be fed back to a shell, and that promise is
//! only worth something if the *value* is quoted. `alias` and `abbr` single-quote theirs; the
//! variable listings (`set`, `declare`, `export -p`) use bash's own rules, in `declare::quote`.

/// Wrap `value` in single quotes, whatever it contains.
///
/// A single quote cannot be escaped *inside* single quotes, so the only way to carry one is to
/// leave the quoted run, emit an escaped quote, and start a new run: `it's` becomes
/// `'it'\''s'`. A newline needs nothing special — it is literal inside single quotes, and the
/// listing simply spans two lines, exactly as bash's does.
pub fn single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::single_quoted;

    #[test]
    fn an_embedded_quote_leaves_and_re_enters_the_quoted_run() {
        assert_eq!(single_quoted("it's"), r"'it'\''s'");
        assert_eq!(single_quoted("'"), r"''\'''");
        assert_eq!(single_quoted(""), "''");
    }
}

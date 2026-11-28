//! Quoting a value the way bash's listings quote it.
//!
//! `declare -p` double-quotes a value unless it holds a character the locale cannot show, and then
//! writes it as `$'…'`. A key of an associative array is written bare unless it needs quoting. The
//! rules are bash's own (`sh_double_quote`, `ansic_quote`, `sh_contains_shell_metas`); which
//! characters are printable follows the locale, as the `@` transformations already decide it.

use crate::expand::param::transform::{ansi_c_quote, needs_ansi_c, quote_element};

/// `"…"`, with the four characters special inside double quotes escaped.
pub fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A value as `declare -p` writes it.
pub fn value(text: &str, utf8: bool) -> String {
    quote_element(text, utf8)
}

/// bash's `sh_contains_shell_metas`: whether a word would not survive being written bare.
pub fn has_shell_metas(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| match b {
        b' ' | b'\t' | b'\n' | b'\'' | b'"' | b'\\' | b'|' | b'&' | b';' | b'(' | b')' | b'<'
        | b'>' | b'!' | b'{' | b'}' | b'*' | b'[' | b'?' | b']' | b'^' | b'$' | b'`' => true,
        b'~' => i == 0 || matches!(bytes[i - 1], b'=' | b':'),
        b'#' => i == 0,
        _ => false,
    })
}

/// A key of an associative array as `declare -p` and `${m[@]@K}` write it.
pub fn key(text: &str, utf8: bool) -> String {
    if needs_ansi_c(text, utf8) {
        ansi_c_quote(text, utf8)
    } else if has_shell_metas(text) || matches!(text, "@" | "*") {
        double_quoted(text)
    } else {
        text.to_string()
    }
}

/// A value as `set` and a bare `declare` write it: bare when it can be, single-quoted otherwise.
pub fn set_style(text: &str, utf8: bool) -> String {
    if needs_ansi_c(text, utf8) {
        ansi_c_quote(text, utf8)
    } else if has_shell_metas(text) {
        format!("'{}'", text.replace('\'', "'\\''"))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each line as bash 5.3 prints it in a UTF-8 locale.
    #[test]
    fn values_quote_as_bash_quotes_them() {
        assert_eq!(value("a\"b$c`d\\e", true), r#""a\"b\$c\`d\\e""#);
        assert_eq!(value("a\nb", true), r"$'a\nb'");
        assert_eq!(value("\x1b[0m", true), r"$'\E[0m'");
        assert_eq!(value("\x7f", true), r"$'\177'");
        assert_eq!(value("it's", true), r#""it's""#);
        assert_eq!(value("café", true), r#""café""#);
        assert_eq!(value("café", false), r"$'caf\303\251'");
    }

    #[test]
    fn keys_are_bare_unless_they_need_quoting() {
        assert_eq!(key("a", true), "a");
        assert_eq!(key("a b", true), r#""a b""#);
        assert_eq!(key("e$f", true), r#""e\$f""#);
        assert_eq!(key("c]d", true), r#""c]d""#);
        assert_eq!(key("k\n", true), r"$'k\n'");
        assert_eq!(key("1+1", true), "1+1");
    }
}

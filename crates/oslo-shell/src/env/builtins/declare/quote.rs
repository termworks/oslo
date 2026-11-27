//! Quoting a value the way bash's listings quote it.
//!
//! `declare -p` double-quotes a value unless it holds a character that is not printable, and then
//! writes it as `$'…'`. A key of an associative array is written bare unless it needs quoting. The
//! rules here are bash's own (`sh_double_quote`, `ansic_quote`, `sh_contains_shell_metas`), so
//! a listing reads back as what was declared and compares byte for byte with bash's.

/// Whether `text` holds a character a listing cannot show as itself.
pub fn needs_ansi_c(text: &str) -> bool {
    text.chars().any(|c| c.is_control())
}

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

/// `$'…'`, with bash's escapes: the named ones, and three octal digits for any other byte that is
/// not printable.
pub fn ansi_c_quoted(text: &str) -> String {
    let mut out = String::from("$'");
    for c in text.chars() {
        match c {
            '\x1b' => out.push_str("\\E"),
            '\x07' => out.push_str("\\a"),
            '\x0b' => out.push_str("\\v"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("\\{byte:03o}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A value as `declare -p` writes it.
pub fn value(text: &str) -> String {
    match needs_ansi_c(text) {
        true => ansi_c_quoted(text),
        false => double_quoted(text),
    }
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

/// A key of an associative array as `declare -p` writes it between the brackets.
pub fn key(text: &str) -> String {
    if needs_ansi_c(text) {
        ansi_c_quoted(text)
    } else if has_shell_metas(text) || matches!(text, "@" | "*") {
        double_quoted(text)
    } else {
        text.to_string()
    }
}

/// A value as `set` and a bare `declare` write it: bare when it can be, single-quoted otherwise.
pub fn set_style(text: &str) -> String {
    if needs_ansi_c(text) {
        ansi_c_quoted(text)
    } else if has_shell_metas(text) {
        format!("'{}'", text.replace('\'', "'\\''"))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each line as bash 5.3 prints it.
    #[test]
    fn values_quote_as_bash_quotes_them() {
        assert_eq!(value("a\"b$c`d\\e"), r#""a\"b\$c\`d\\e""#);
        assert_eq!(value("a\nb"), r"$'a\nb'");
        assert_eq!(value("\x1b[0m"), r"$'\E[0m'");
        assert_eq!(value("\x7f"), r"$'\177'");
        assert_eq!(value("it's"), r#""it's""#);
        assert_eq!(value("café"), r#""café""#);
    }

    #[test]
    fn keys_are_bare_unless_they_need_quoting() {
        assert_eq!(key("a"), "a");
        assert_eq!(key("a b"), r#""a b""#);
        assert_eq!(key("e$f"), r#""e\$f""#);
        assert_eq!(key("c]d"), r#""c]d""#);
        assert_eq!(key("k\n"), r"$'k\n'");
        assert_eq!(key("1+1"), "1+1");
    }
}

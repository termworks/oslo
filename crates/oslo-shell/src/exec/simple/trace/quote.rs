//! How a trace line quotes what it prints: bash's `xtrace_print_word_list`, its
//! `xtrace_print_assignment`, and the `[[ … ]]` term printer.

use crate::syntax::lower::cond::{NEGATION, QUOTED_CLOSE, QUOTED_OPEN};

/// Quote one word the way bash's trace does: `''` for an empty word, `$'…'` for one holding a
/// character that is not printable, `'…'` for one holding a shell metacharacter, else as it is.
pub fn quote_word(word: &str) -> String {
    if word.is_empty() {
        return "''".to_string();
    }
    if needs_ansi_c(word) {
        return ansi_c_quote(word);
    }
    if has_shell_metas(word) {
        return single_quote(word);
    }
    word.to_string()
}

/// `name=value` as bash traces a scalar assignment; an empty value is printed as nothing.
pub fn assignment_text(lhs: &str, append: bool, value: &str) -> String {
    let op = if append { "+=" } else { "=" };
    let value = match value.is_empty() {
        true => String::new(),
        false => quote_word(value),
    };
    format!("{lhs}{op}{value}")
}

/// `name=(…)` as bash traces an array assignment: the list as it was written.
pub fn list_assignment_text(lhs: &str, append: bool, list: &str) -> String {
    let op = if append { "+=" } else { "=" };
    format!("{lhs}{op}({list})")
}

/// The body of a `[[ … ]]` trace line, from the lowered call's operands and closing `]]`.
///
/// Operands are printed bare, an empty one as `''`; the right side of a pattern or regex
/// operator escapes each quoted character that is special to it, as bash prints `a\*`.
pub fn conditional_text(args: &[String]) -> String {
    let mut args = args.strip_suffix(&["]]".to_string()]).unwrap_or(args);
    let mut negated = false;
    while args.len() > 1 && args[0] == NEGATION {
        negated = !negated;
        args = &args[1..];
    }
    let mut out = String::from("[[ ");
    if negated {
        out.push_str("! ");
    }
    let terms: Vec<String> = match args {
        [op, operand] => vec![op.clone(), bare(operand)],
        [left, op, right] => {
            let right = match op.as_str() {
                "==" | "=" | "!=" => pattern(right, GLOB_SPECIAL),
                "=~" => pattern(right, REGEX_SPECIAL),
                _ => bare(right),
            };
            vec![bare(left), op.clone(), right]
        }
        other => other.iter().map(|arg| bare(arg)).collect(),
    };
    out.push_str(&terms.join(" "));
    out.push_str(" ]]");
    out
}

/// The quoted characters a trace escapes in a glob pattern.
const GLOB_SPECIAL: &str = ".-!()|^\\?*[]%#=:";

/// The quoted characters a trace escapes in a regular expression.
const REGEX_SPECIAL: &str = ".+()|{^$\\?*[";

fn bare(word: &str) -> String {
    match word.is_empty() {
        true => "''".to_string(),
        false => word.to_string(),
    }
}

/// A marked operand with its quoted special characters escaped and the marks removed.
fn pattern(word: &str, special: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut quoted = false;
    for ch in word.chars() {
        match ch {
            QUOTED_OPEN => quoted = true,
            QUOTED_CLOSE => quoted = false,
            _ => {
                if quoted && special.contains(ch) {
                    out.push('\\');
                }
                out.push(ch);
            }
        }
    }
    bare(&out)
}

/// bash's `sh_contains_shell_metas`.
fn has_shell_metas(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.iter().enumerate().any(|(at, &b)| match b {
        b' ' | b'\t' | b'\n' | b'\'' | b'"' | b'\\' | b'|' | b'&' | b';' | b'(' | b')' | b'<'
        | b'>' | b'!' | b'{' | b'}' | b'*' | b'[' | b'?' | b']' | b'^' | b'$' | b'`' => true,
        b'~' => at == 0 || matches!(bytes[at - 1], b'=' | b':'),
        b'#' => at == 0,
        _ => false,
    })
}

/// bash's `sh_single_quote`.
fn single_quote(word: &str) -> String {
    if word == "'" {
        return "\\'".to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Whether the locale says multibyte characters are text, so only control characters need `$'…'`.
fn utf8_locale() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
}

/// Whether a character prints as itself in the trace.
fn printable(ch: char, utf8: bool) -> bool {
    match ch.is_ascii() {
        true => (' '..='~').contains(&ch),
        false => utf8 && !ch.is_control() && !oslo_base::lossless::is_raw(ch),
    }
}

/// bash's `ansic_shouldquote`.
fn needs_ansi_c(word: &str) -> bool {
    let utf8 = utf8_locale();
    word.chars().any(|ch| !printable(ch, utf8))
}

/// bash's `ansic_quote`: `$'…'` with C escapes and octal for what has no escape.
fn ansi_c_quote(word: &str) -> String {
    let utf8 = utf8_locale();
    let mut out = String::from("$'");
    for ch in word.chars() {
        let escape = match ch {
            '\u{1b}' => Some('E'),
            '\u{7}' => Some('a'),
            '\u{b}' => Some('v'),
            '\u{8}' => Some('b'),
            '\u{c}' => Some('f'),
            '\n' => Some('n'),
            '\r' => Some('r'),
            '\t' => Some('t'),
            '\\' | '\'' => Some(ch),
            _ => None,
        };
        match escape {
            Some(letter) => {
                out.push('\\');
                out.push(letter);
            }
            None if printable(ch, utf8) => out.push(ch),
            None => {
                for byte in oslo_base::lossless::decode(ch.encode_utf8(&mut [0u8; 4])) {
                    out.push_str(&format!("\\{byte:03o}"));
                }
            }
        }
    }
    out.push('\'');
    out
}

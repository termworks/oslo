//! `${x@op}`: bash's parameter transformations.
//!
//! ```text
//!   Q  quoted for reuse          E  backslash escapes, as in $'…'     P  prompt expansion
//!   A  an assignment recreating  a  the attribute letters             K  quoted, keys too
//!   k  K, as separate words      U  upper case   u  first upper       L  lower case
//! ```
//!
//! An unset parameter transforms to nothing, whatever the letter; a set one with a letter bash
//! does not know is a bad substitution.

use super::Target;
use super::case::{CaseMap, convert_case};
use crate::env::Environment;
use crate::expand::locale;
use oslo_base::error::{Result, ShellError};

/// Whether `op` is a transformation bash knows.
pub(super) fn is_known(op: char) -> bool {
    matches!(op, 'Q' | 'E' | 'P' | 'A' | 'K' | 'a' | 'k' | 'U' | 'u' | 'L')
}

/// The error a set parameter with an unknown transformation raises; bash ends the shell with 127.
pub(super) fn unknown(name: &str, op: char) -> ShellError {
    ShellError::UnsetParameter(format!("${{{name}@{op}}}: bad substitution"))
}

/// A transformation that reads only the text of the value.
pub(super) fn text(env: &mut Environment, op: char, value: &str) -> Result<String> {
    let utf8 = locale::is_utf8(env);
    Ok(match op {
        'E' => crate::lexer::decode_ansi_c(value),
        'P' => crate::expand::prompt::expand(env, value)?,
        'U' => convert_case(value, None, CaseMap::Upper, true, utf8),
        'u' => convert_case(value, None, CaseMap::Upper, false, utf8),
        'L' => convert_case(value, None, CaseMap::Lower, true, utf8),
        _ => quote_reusable(value, utf8),
    })
}

/// `${name@op}` on one parameter or element.
pub(super) fn scalar(
    env: &mut Environment,
    target: &Target<'_>,
    op: char,
    value: Option<String>,
) -> Result<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    if !is_known(op) {
        return Err(unknown(&target.display(), op));
    }
    let name = match target {
        Target::Param(name) | Target::Element { name, .. } => *name,
    };
    let declared = crate::env::scope::is_valid_identifier(name);
    Ok(match op {
        'a' if declared => attributes(env, name),
        'a' => String::new(),
        'A' if !declared => String::new(),
        'A' => {
            let quoted = quote_reusable(&value, locale::is_utf8(env));
            match attributes(env, name) {
                flags if flags.is_empty() => format!("{name}={quoted}"),
                flags => format!("declare -{flags} {name}={quoted}"),
            }
        }
        _ => text(env, op, &value)?,
    })
}

/// The attribute letters of `name`, in bash's order.
pub(super) fn attributes(env: &Environment, name: &str) -> String {
    let mut flags = String::new();
    if env.get_array(name).is_some() {
        flags.push('a');
    }
    if env.is_integer(name) {
        flags.push('i');
    }
    if env.is_readonly(name) {
        flags.push('r');
    }
    if env.is_exported(name) {
        flags.push('x');
    }
    flags
}

/// Quote `value` so the shell reads it back as itself: single quotes, or `$'…'` when it holds a
/// character that is not printable.
pub(crate) fn quote_reusable(value: &str, utf8: bool) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if needs_ansi_c(value, utf8) {
        return ansi_c_quote(value, utf8);
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// An array element as `declare -p` writes it: double quotes, or `$'…'` where those cannot say it.
pub(super) fn quote_element(value: &str, utf8: bool) -> String {
    if needs_ansi_c(value, utf8) {
        return ansi_c_quote(value, utf8);
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        if matches!(ch, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn printable(ch: char, utf8: bool) -> bool {
    match ch.is_ascii() {
        true => !ch.is_ascii_control(),
        false => utf8 && !ch.is_control() && !oslo_base::lossless::has_raw(&ch.to_string()),
    }
}

fn needs_ansi_c(value: &str, utf8: bool) -> bool {
    value.chars().any(|ch| !printable(ch, utf8))
}

/// `$'…'`, escaping what is not printable the way bash's `ansic_quote` does.
fn ansi_c_quote(value: &str, utf8: bool) -> String {
    let mut out = String::from("$'");
    for ch in value.chars() {
        match ch {
            '\x1b' => out.push_str("\\E"),
            '\x07' => out.push_str("\\a"),
            '\x0b' => out.push_str("\\v"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' | '\'' => {
                out.push('\\');
                out.push(ch);
            }
            ch if printable(ch, utf8) => out.push(ch),
            ch => {
                for byte in oslo_base::lossless::decode(&ch.to_string()) {
                    out.push_str(&format!("\\{byte:03o}"));
                }
            }
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::{quote_element, quote_reusable};

    #[test]
    fn a_value_is_quoted_for_reuse() {
        assert_eq!(quote_reusable("abc", true), "'abc'");
        assert_eq!(quote_reusable("it's", true), r"'it'\''s'");
        assert_eq!(quote_reusable("", true), "''");
        assert_eq!(quote_reusable("a\nb", true), r"$'a\nb'");
        assert_eq!(quote_reusable("it's\n", true), r"$'it\'s\n'");
        assert_eq!(quote_reusable("\x01", true), r"$'\001'");
        assert_eq!(quote_reusable("\x1b[0m", true), r"$'\E[0m'");
        assert_eq!(quote_reusable("é", true), "'é'");
        assert_eq!(quote_reusable("é", false), r"$'\303\251'");
    }

    #[test]
    fn an_element_is_double_quoted() {
        assert_eq!(quote_element("q\"r", true), r#""q\"r""#);
        assert_eq!(quote_element("x$y", true), r#""x\$y""#);
        assert_eq!(quote_element("a\nb", true), r"$'a\nb'");
    }
}

//! `oslo secret run --env-file FILE`: a dotenv file whose values may name secrets.
//!
//! ```text
//! # a comment, and blank lines, are skipped
//! REGION=eu-west-1                 a plain value, passed through
//! export LOG_LEVEL=debug           `export ` in front is allowed
//! GITHUB_TOKEN=secret:gh-token     the secret gh-token, from the store run uses
//! DEPLOY_KEY=secret:work/deploy    the secret deploy, from the store work
//! GREETING="hello\tworld"          double quotes: \n \t \r \" \\ are escapes
//! PATTERN='a $literal\n'           single quotes: nothing is an escape
//! NOTE=plain value  # trailing     unquoted: a `#` after a space or tab starts a comment
//! ```
//!
//! One `NAME=VALUE` per line, with spaces or tabs allowed around the `=`. `NAME` is
//! `[A-Za-z_][A-Za-z0-9_]*`. A quoted value closes on its own line, and only a comment may follow
//! it. Nothing is interpolated: `$` is a character like any other. A value that starts with
//! `secret:`, quoted or not, always names a secret. A name given twice, and any line that is not
//! one of these shapes, is an error that names the line.

/// What one line gives its variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Plain(String),
    /// `secret:NAME`, or `secret:STORE/NAME` for another store.
    Secret {
        store: Option<String>,
        name: String,
    },
}

/// One variable the file sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub name: String,
    pub value: Value,
}

/// The largest file read, in bytes.
const LARGEST: u64 = 1 << 20;

/// Read and parse `path`.
pub fn read(path: &str) -> Result<Vec<Line>, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{path}: {e}"))?
        .len();
    if size > LARGEST {
        return Err(format!("{path}: larger than {LARGEST} bytes"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse(&text).map_err(|e| format!("{path}:{e}"))
}

/// Parse a whole file. An error starts with the line number.
pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    let mut lines: Vec<Line> = Vec::new();
    for (at, raw) in text.split('\n').enumerate() {
        let number = at + 1;
        let Some((name, value)) =
            line(raw.strip_suffix('\r').unwrap_or(raw)).map_err(|e| format!("{number}: {e}"))?
        else {
            continue;
        };
        if let Some(earlier) = lines.iter().find(|line| line.name == name) {
            return Err(format!(
                "{number}: {name} is already set on line {}",
                earlier.number
            ));
        }
        lines.push(Line {
            number,
            name,
            value,
        });
    }
    Ok(lines)
}

/// One line: `None` for a blank line or a comment.
fn line(raw: &str) -> Result<Option<(String, Value)>, String> {
    let text = raw.trim_matches([' ', '\t']);
    if text.is_empty() || text.starts_with('#') {
        return Ok(None);
    }
    let text = match text.strip_prefix("export") {
        Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start_matches([' ', '\t']),
        _ => text,
    };
    let Some((name, rest)) = text.split_once('=') else {
        let shown = oslo::secrets::recipient::shown(text);
        return Err(format!("{shown:?} is not NAME=VALUE"));
    };
    let name = name.trim_end_matches([' ', '\t']);
    if !super::asked::is_identifier(name) {
        return Err(format!("{name:?} is not a variable name"));
    }
    let written = value(rest)?;
    let value = match written.strip_prefix("secret:") {
        Some(reference) => secret(reference)?,
        None => Value::Plain(written),
    };
    Ok(Some((name.to_string(), value)))
}

/// The text of a value, its quotes and escapes resolved.
fn value(rest: &str) -> Result<String, String> {
    let rest = rest.trim_start_matches([' ', '\t']);
    let (text, after) = match rest.chars().next() {
        Some('\'') => {
            let body = &rest[1..];
            let end = body.find('\'').ok_or("a ' that is never closed")?;
            (body[..end].to_string(), &body[end + 1..])
        }
        Some('"') => double_quoted(&rest[1..])?,
        _ => return Ok(unquoted(rest)),
    };
    let after = after.trim_start_matches([' ', '\t']);
    if !after.is_empty() && !after.starts_with('#') {
        return Err(format!("{after:?} after the closing quote"));
    }
    Ok(text)
}

/// The inside of `"…"` and what follows the closing quote.
fn double_quoted(body: &str) -> Result<(String, &str), String> {
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &body[at + 1..])),
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, escaped @ ('"' | '\\'))) => out.push(escaped),
                Some((_, other)) => {
                    out.push('\\');
                    out.push(other);
                }
                None => break,
            },
            other => out.push(other),
        }
    }
    Err("a \" that is never closed".to_string())
}

/// An unquoted value: up to a `#` that follows a space or tab, trailing blanks trimmed.
fn unquoted(rest: &str) -> String {
    let bytes = rest.as_bytes();
    let end = (1..bytes.len())
        .find(|&at| bytes[at] == b'#' && matches!(bytes[at - 1], b' ' | b'\t'))
        .unwrap_or(bytes.len());
    rest[..end].trim_end_matches([' ', '\t']).to_string()
}

/// `NAME` or `STORE/NAME`, after `secret:`.
fn secret(reference: &str) -> Result<Value, String> {
    let (store, name) = match reference.split_once('/') {
        Some((store, name)) => (Some(store.to_string()), name),
        None => (None, reference),
    };
    if name.is_empty() || store.as_deref() == Some("") {
        return Err(format!("secret:{reference} names no secret"));
    }
    Ok(Value::Secret {
        store,
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Value {
        Value::Plain(text.to_string())
    }

    #[test]
    fn every_shape_the_doc_names_is_read() {
        let lines = parse(
            "# a comment\n\
             \n\
             REGION=eu-west-1\n\
             export LOG_LEVEL=debug\n\
             GITHUB_TOKEN=secret:gh-token\n\
             DEPLOY_KEY = secret:work/deploy\n\
             GREETING=\"hello\\tworld \\\"x\\\" \\$HOME\"\n\
             PATTERN='a $literal\\n'   # after\n\
             NOTE=plain value  # trailing\n\
             HASH=a#b\n\
             EMPTY=\n\
             QUOTED_REF=\"secret:quoted\"\r\n",
        )
        .expect("parses");
        let got: Vec<(&str, Value)> = lines
            .iter()
            .map(|l| (l.name.as_str(), l.value.clone()))
            .collect();
        let secret = |store: Option<&str>, name: &str| Value::Secret {
            store: store.map(str::to_string),
            name: name.to_string(),
        };
        assert_eq!(
            got,
            vec![
                ("REGION", plain("eu-west-1")),
                ("LOG_LEVEL", plain("debug")),
                ("GITHUB_TOKEN", secret(None, "gh-token")),
                ("DEPLOY_KEY", secret(Some("work"), "deploy")),
                ("GREETING", plain("hello\tworld \"x\" \\$HOME")),
                ("PATTERN", plain("a $literal\\n")),
                ("NOTE", plain("plain value")),
                ("HASH", plain("a#b")),
                ("EMPTY", plain("")),
                ("QUOTED_REF", secret(None, "quoted")),
            ]
        );
        assert_eq!(lines[0].number, 3, "line numbers count the skipped lines");
    }

    #[test]
    fn a_bad_line_is_an_error_naming_it() {
        for (text, says) in [
            ("A=1\nNOPE\n", "2: \"NOPE\" is not NAME=VALUE"),
            ("1A=x", "1: \"1A\" is not a variable name"),
            ("A='open", "1: a ' that is never closed"),
            ("A=\"open", "1: a \" that is never closed"),
            ("A='x' trailing", "after the closing quote"),
            ("A=secret:", "names no secret"),
            ("A=secret:work/", "names no secret"),
            ("A=secret:/x", "names no secret"),
            ("A=1\nA=2", "2: A is already set on line 1"),
        ] {
            let refused = parse(text).expect_err(text);
            assert!(refused.contains(says), "{text:?}: {refused}");
        }
    }
}

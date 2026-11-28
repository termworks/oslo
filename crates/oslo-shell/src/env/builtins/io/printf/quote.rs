//! `%q` and `%Q`: quote a string so the shell reads it back unchanged, in bash's exact style.
//!
//! A string with anything unprintable becomes `$'...'`; otherwise each special character gets a
//! backslash, or with `#` the whole string goes in single quotes.

/// The bytes that always take a backslash: blanks, newline and the shell's metacharacters.
const SPECIAL: &[u8] = b"\t\n !\"$&'()*,;<>?[\\]^`{|}";

/// The ISO C basic character set: printable ASCII except `$`, `@` and `` ` ``, plus `\t\v\f`.
fn is_basic(byte: u8) -> bool {
    matches!(byte, b'\t' | 0x0b | 0x0c)
        || (byte.is_ascii_graphic() && !b"$@`".contains(&byte))
        || byte == b' '
}

/// Whether a character prints: not a C0 or C1 control, and not a line or paragraph separator.
fn printable(c: char) -> bool {
    !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}')
}

/// The length of the character at the front of `text` if it is valid and printable.
fn printable_char(text: &[u8], utf8: bool) -> Option<usize> {
    if !utf8 {
        return (text[0].is_ascii_graphic() || text[0] == b' ').then_some(1);
    }
    let chunk = text.utf8_chunks().next()?;
    let c = chunk.valid().chars().next()?;
    printable(c).then_some(c.len_utf8())
}

/// Whether `text` needs `$'...'`: some character in it does not print.
fn needs_ansi(text: &[u8], utf8: bool) -> bool {
    for (at, &byte) in text.iter().enumerate() {
        if !is_basic(byte) {
            let rest = &text[at..];
            return match utf8 {
                true => std::str::from_utf8(rest).map_or(true, |s| !s.chars().all(printable)),
                false => !rest.iter().all(|b| b.is_ascii_graphic() || *b == b' '),
            };
        }
        if !(byte.is_ascii_graphic() || byte == b' ') {
            return true;
        }
    }
    false
}

/// `$'...'`, with C's letter escapes, `\E` for escape and octal for anything else unprintable.
fn ansi(text: &[u8], utf8: bool) -> Vec<u8> {
    let mut out = b"$'".to_vec();
    let mut at = 0;
    while at < text.len() {
        let byte = text[at];
        let letter = match byte {
            0x1b => Some(b'E'),
            0x07 => Some(b'a'),
            0x0b => Some(b'v'),
            0x08 => Some(b'b'),
            0x0c => Some(b'f'),
            b'\n' => Some(b'n'),
            b'\r' => Some(b'r'),
            b'\t' => Some(b't'),
            b'\\' | b'\'' => Some(byte),
            _ => None,
        };
        if let Some(letter) = letter {
            out.extend_from_slice(&[b'\\', letter]);
            at += 1;
            continue;
        }
        let shown = match is_basic(byte) {
            true => (byte.is_ascii_graphic() || byte == b' ').then_some(1),
            false => printable_char(&text[at..], utf8),
        };
        match shown {
            Some(len) => out.extend_from_slice(&text[at..at + len]),
            None => out.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
        at += shown.unwrap_or(1);
    }
    out.push(b'\'');
    out
}

/// `'...'`, with each `'` closed, escaped and reopened.
fn single(text: &[u8]) -> Vec<u8> {
    if text == b"'" {
        return b"\\'".to_vec();
    }
    let mut out = vec![b'\''];
    for &byte in text {
        out.push(byte);
        if byte == b'\'' {
            out.extend_from_slice(b"\\''");
        }
    }
    out.push(b'\'');
    out
}

/// A backslash before each special byte, a leading `#`, and a `~` at the start of a word or
/// after `:` or `=`. UTF-8 characters pass through whole in a UTF-8 locale.
fn backslashed(text: &[u8], utf8: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    let mut at = 0;
    while at < text.len() {
        let byte = text[at];
        if utf8 && byte >= 0x80 {
            let len = printable_char(&text[at..], true).unwrap_or(1);
            out.extend_from_slice(&text[at..at + len]);
            at += len;
            continue;
        }
        let after_word = at == 0 || matches!(text[at - 1], b':' | b'=');
        if SPECIAL.contains(&byte) || (byte == b'#' && at == 0) || (byte == b'~' && after_word) {
            out.push(b'\\');
        }
        out.push(byte);
        at += 1;
    }
    out
}

/// Quote `text` as `%q` does; `alt` is the `#` flag, which prefers single quotes.
pub fn quote(text: &[u8], utf8: bool, alt: bool) -> Vec<u8> {
    if text.is_empty() {
        b"''".to_vec()
    } else if needs_ansi(text, utf8) {
        ansi(text, utf8)
    } else if alt {
        single(text)
    } else {
        backslashed(text, utf8)
    }
}

/// Quote `text` so that reading it back as shell input yields `text` exactly, as `%q` does in a
/// UTF-8 locale.
pub fn shell_quote(text: &str) -> String {
    let bytes = oslo_base::lossless::decode(text);
    let quoted = quote(&bytes, true, false);
    oslo_base::lossless::encode(std::os::unix::ffi::OsStrExt::from_bytes(&quoted))
}

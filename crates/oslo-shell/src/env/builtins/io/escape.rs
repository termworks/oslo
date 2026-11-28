//! Backslash escapes for `printf` and `echo -e`, decoded to bytes the way bash decodes them.
//!
//! Three dialects share one table and differ at the edges:
//!
//! * the `printf` **format**: `\NNN` takes up to three octal digits, `\'` `\"` `\?` drop the
//!   backslash, and `\c` is not an escape;
//! * a `printf` **`%b`** argument: `\0NNN` takes up to three digits after the `0`, the quotes
//!   keep their backslash, and `\c` ends all output;
//! * **`echo -e`**: only `\0NNN` is octal, so `\101` is left as it is.

use crate::env::origin_now;

fn octal(byte: u8) -> Option<u32> {
    matches!(byte, b'0'..=b'7').then(|| u32::from(byte - b'0'))
}

fn hex(byte: u8) -> Option<u32> {
    (byte as char).to_digit(16)
}

/// Up to `most` digits of `text` in a base, as a value and the count taken.
fn digits(text: &[u8], most: usize, digit: fn(u8) -> Option<u32>, base: u32) -> (u32, usize) {
    let mut value = 0u32;
    let mut taken = 0;
    while taken < most
        && let Some(d) = text.get(taken).copied().and_then(digit)
    {
        value = value.wrapping_mul(base).wrapping_add(d);
        taken += 1;
    }
    (value, taken)
}

fn control(byte: u8) -> Option<u8> {
    Some(match byte {
        b'a' => 0x07,
        b'b' => 0x08,
        b'e' | b'E' => 0x1b,
        b'f' => 0x0c,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'v' => 0x0b,
        _ => return None,
    })
}

/// The bytes a `\u`/`\U` code point becomes: UTF-8 in a UTF-8 locale, a C99 escape otherwise.
pub fn code_point(value: u32, utf8: bool, out: &mut Vec<u8>) {
    if value <= 0x7f {
        out.push(value as u8);
    } else if value >= 0x8000_0000 {
        // No encoding reaches this far, so it converts to nothing.
    } else if !utf8 {
        let escaped = match value < 0x10000 {
            true => format!("\\u{value:04X}"),
            false => format!("\\U{value:08X}"),
        };
        out.extend_from_slice(escaped.as_bytes());
    } else if let Some(c) = char::from_u32(value) {
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    } else {
        // Surrogates and values past U+10FFFF, in the original six-byte UTF-8 scheme.
        let len = match value {
            0..0x10000 => 3,
            0x10000..0x200000 => 4,
            0x200000..0x4000000 => 5,
            0x4000000..0x80000000 => 6,
            _ => return,
        };
        let lead: u8 = [0, 0, 0, 0xe0, 0xf0, 0xf8, 0xfc][len];
        out.push(lead | (value >> (6 * (len - 1))) as u8);
        for at in (0..len - 1).rev() {
            out.push(0x80 | ((value >> (6 * at)) & 0x3f) as u8);
        }
    }
}

/// Decode one `printf` escape. `text` starts after the backslash; `in_b` selects the `%b`
/// dialect. Returns the bytes of `text` consumed (0 leaves the following byte as data) and
/// whether it was a `%b` `\c`.
pub fn printf_escape(text: &[u8], in_b: bool, utf8: bool, out: &mut Vec<u8>) -> (usize, bool) {
    let Some(&first) = text.first() else {
        out.push(b'\\');
        return (0, false);
    };
    if let Some(byte) = control(first) {
        out.push(byte);
        return (1, false);
    }
    match first {
        b'0'..=b'7' => {
            let lead = u32::from(first - b'0');
            let most = 2 + usize::from(lead == 0 && in_b);
            let (rest, taken) = digits(&text[1..], most, octal, 8);
            let value = (0..taken)
                .fold(lead, |v, _| v.wrapping_mul(8))
                .wrapping_add(rest);
            out.push(value as u8);
            (1 + taken, false)
        }
        b'x' => match digits(&text[1..], 2, hex, 16) {
            (_, 0) => {
                eprintln!("{}printf: missing hex digit for \\x", origin_now());
                out.push(b'\\');
                (0, false)
            }
            (value, taken) => {
                out.push(value as u8);
                (1 + taken, false)
            }
        },
        b'u' | b'U' => match digits(&text[1..], if first == b'u' { 4 } else { 8 }, hex, 16) {
            (_, 0) => {
                match first {
                    b'u' => eprintln!("{}printf: missing unicode digit for \\u", origin_now()),
                    _ => eprintln!("{}printf: missing unicode digit for \\U", origin_now()),
                }
                out.push(b'\\');
                (0, false)
            }
            (value, taken) => {
                code_point(value, utf8, out);
                (1 + taken, false)
            }
        },
        b'\\' => {
            out.push(b'\\');
            (1, false)
        }
        b'\'' | b'"' | b'?' if !in_b => {
            out.push(first);
            (1, false)
        }
        b'c' if in_b => (1, true),
        _ => {
            out.push(b'\\');
            (0, false)
        }
    }
}

/// A `%b` argument with its escapes decoded, and whether a `\c` cut it short.
pub fn expand_b(text: &[u8], utf8: bool) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if at + 1 == text.len() || text[at] != b'\\' {
            out.push(text[at]);
            at += 1;
            continue;
        }
        let (used, stop) = printf_escape(&text[at + 1..], true, utf8, &mut out);
        if stop {
            return (out, true);
        }
        at += 1 + used;
    }
    (out, false)
}

/// One `echo -e` argument with its escapes decoded, and whether a `\c` ended the output.
pub fn expand_echo(text: &[u8], utf8: bool) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let byte = text[at];
        at += 1;
        if byte != b'\\' || at == text.len() {
            out.push(byte);
            continue;
        }
        let first = text[at];
        at += 1;
        if let Some(byte) = control(first) {
            out.push(byte);
            continue;
        }
        match first {
            b'0' => {
                let (value, taken) = digits(&text[at..], 3, octal, 8);
                out.push(value as u8);
                at += taken;
            }
            b'x' => match digits(&text[at..], 2, hex, 16) {
                (_, 0) => out.extend_from_slice(b"\\x"),
                (value, taken) => {
                    out.push(value as u8);
                    at += taken;
                }
            },
            b'u' | b'U' => match digits(&text[at..], if first == b'u' { 4 } else { 8 }, hex, 16) {
                (_, 0) => out.extend_from_slice(&[b'\\', first]),
                (value, taken) => {
                    code_point(value, utf8, &mut out);
                    at += taken;
                }
            },
            b'\\' => out.push(b'\\'),
            b'c' => return (out, true),
            other => out.extend_from_slice(&[b'\\', other]),
        }
    }
    (out, false)
}

#[cfg(test)]
#[path = "escape/tests.rs"]
mod tests;

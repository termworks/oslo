//! ANSI-C quoting: decoding the body of `$'…'`.
//!
//! The decoded text is quoted data — it is handed back as a single-quoted word part, so no
//! expansion, field splitting or globbing may touch it afterwards. That is the whole reason
//! `IFS=$'\n'` has to be decoded here rather than left for the expander: the newline the user
//! asked for must survive as *one* literal character.

/// Decode `$'…'` escapes.
///
/// The body arrives with backslash sequences intact (the scanner only used them to know which
/// `'` closes the string). An unrecognised escape keeps both characters, which is what bash does.
///
/// Decoding stops at a NUL: a shell variable is a C string, so bash's `$'a\0b'` is one character
/// long. Producing the byte and losing it later would be worse — it would reach `CString` and
/// fail at exec time.
///
/// `\xHH` and `\NNN` are bytes, not code points: `$'\xe2\x94\x80'` is `─`, and a run that is not
/// UTF-8 is carried as `oslo_base::lossless` carries any other byte.
pub fn decode(raw: &str) -> String {
    oslo_base::lossless::from_bytes(&decode_bytes(raw))
}

/// The bytes the body of `$'…'` stands for.
fn decode_bytes(raw: &str) -> Vec<u8> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<u8>, c: char| {
        out.extend(oslo_base::lossless::decode(c.encode_utf8(&mut [0; 4])))
    };

    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            push(&mut out, chars[i]);
            i += 1;
            continue;
        }

        let esc = chars[i + 1];
        i += 2;
        let simple = match esc {
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'e' | 'E' => Some('\x1b'),
            'f' => Some('\x0c'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\x0b'),
            '\\' => Some('\\'),
            '\'' => Some('\''),
            '"' => Some('"'),
            '?' => Some('?'),
            _ => None,
        };
        if let Some(c) = simple {
            out.push(c as u8);
            continue;
        }

        let byte = matches!(esc, '0'..='7' | 'x' | 'c');
        let decoded = match esc {
            // `\0nnn` spends its `0` as an introducer, so three digits still follow it; `\nnn`
            // has already spent one of its three. That is why `\0101` is `A` but `\1011` is `A1`.
            '0' => radix_escape(&chars, &mut i, Some(0), 0, 8, 3),
            '1'..='7' => radix_escape(&chars, &mut i, esc.to_digit(8), 1, 8, 3),
            'x' => radix_escape(&chars, &mut i, None, 0, 16, 2),
            'u' => radix_escape(&chars, &mut i, None, 0, 16, 4),
            'U' => radix_escape(&chars, &mut i, None, 0, 16, 8),
            // `\cX` keeps the low five bits of X upper-cased (`\c?` is DEL), and `\c\\` spends
            // both backslashes on one control-backslash.
            'c' => match chars.get(i) {
                Some(&c) => {
                    i += 1;
                    if c == '\\' && chars.get(i) == Some(&'\\') {
                        i += 1;
                    }
                    Some(match c {
                        '?' => 0x7f,
                        _ => (c.to_ascii_uppercase() as u32) & 0x1f,
                    })
                }
                None => None,
            },
            _ => None,
        };

        match decoded {
            // A shell string cannot carry a NUL, so everything from here on is unreachable data.
            Some(0) => return out,
            Some(v) if byte => out.push(v as u8),
            Some(v) => match char::from_u32(v) {
                Some(c) => push(&mut out, c),
                // A surrogate or a value past U+10FFFF is written as bash writes it, in the
                // original UTF-8 scheme of up to six bytes.
                None => out.extend(raw_code_point(v)),
            },
            None => {
                out.push(b'\\');
                push(&mut out, esc);
            }
        }
    }

    out
}

/// The bytes of `value` in UTF-8's original scheme, which reaches 31 bits in six bytes.
fn raw_code_point(value: u32) -> Vec<u8> {
    let len = match value {
        0..=0x7f => 1,
        0x80..=0x7ff => 2,
        0x800..=0xffff => 3,
        0x1_0000..=0x1f_ffff => 4,
        0x20_0000..=0x3ff_ffff => 5,
        _ => 6,
    };
    let lead: [u8; 7] = [0, 0, 0xc0, 0xe0, 0xf0, 0xf8, 0xfc];
    let mut bytes = vec![0u8; len];
    let mut rest = value;
    for byte in bytes.iter_mut().skip(1).rev() {
        *byte = 0x80 | (rest & 0x3f) as u8;
        rest >>= 6;
    }
    bytes[0] = if len == 1 {
        rest as u8
    } else {
        lead[len] | rest as u8
    };
    bytes
}

/// Consume up to `max - taken` further digits in `radix`, folding them onto `seed`.
///
/// Returns `None` when there is nothing to read at all, so the caller can fall back to keeping
/// the escape verbatim the way bash does for a bare `\x`.
fn radix_escape(
    chars: &[char],
    i: &mut usize,
    seed: Option<u32>,
    taken: usize,
    radix: u32,
    max: usize,
) -> Option<u32> {
    let mut value = seed;
    let mut taken = taken;

    while taken < max {
        match chars.get(*i).and_then(|c| c.to_digit(radix)) {
            Some(d) => {
                value = Some(value.unwrap_or(0) * radix + d);
                *i += 1;
                taken += 1;
            }
            None => break,
        }
    }

    value
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn control_escapes() {
        assert_eq!(decode("a\\tb"), "a\tb");
        assert_eq!(decode("l1\\nl2"), "l1\nl2");
        assert_eq!(decode("\\r"), "\r");
        assert_eq!(decode("\\\\"), "\\");
        assert_eq!(decode("\\'"), "'");
        assert_eq!(decode("\\\""), "\"");
        assert_eq!(decode("\\e"), "\x1b");
    }

    #[test]
    fn numeric_escapes() {
        assert_eq!(decode("\\101"), "A");
        assert_eq!(decode("\\0101"), "A");
        assert_eq!(decode("\\x41"), "A");
        assert_eq!(decode("\\x4"), "\u{4}");
        assert_eq!(decode("\\u00e9"), "é");
        assert_eq!(decode("\\U0001F600"), "\u{1F600}");
        assert_eq!(decode("\\cA"), "\x01");
        assert_eq!(decode("\\c1"), "\u{11}");
        assert_eq!(decode("\\c\\\\"), "\u{1c}");
        assert_eq!(decode("\\c?"), "\u{7f}");
    }

    /// A surrogate and a value past U+10FFFF come out as the bytes bash writes for them.
    #[test]
    fn out_of_range_code_points_are_raw_bytes() {
        let bytes = |s: &str| oslo_base::lossless::decode(&decode(s));
        assert_eq!(bytes("\\uD800"), vec![0xed, 0xa0, 0x80]);
        assert_eq!(bytes("\\U110000"), vec![0xf4, 0x90, 0x80, 0x80]);
        assert_eq!(
            bytes("\\U7fffffff"),
            vec![0xfd, 0xbf, 0xbf, 0xbf, 0xbf, 0xbf]
        );
    }

    /// `\x` and octal escapes are bytes: a UTF-8 run is one character, anything else stays bytes.
    #[test]
    fn byte_escapes_are_bytes() {
        let bytes = |s: &str| oslo_base::lossless::decode(&decode(s));
        assert_eq!(decode("\\xe2\\x94\\x80"), "─");
        assert_eq!(decode("\\303\\251"), "é");
        assert_eq!(bytes("\\xff\\376"), vec![0xff, 0xfe]);
        assert_eq!(bytes("\\777"), vec![0xff]);
    }

    /// An octal escape is at most three digits, so the fourth character is data again.
    #[test]
    fn an_octal_escape_stops_at_three_digits() {
        assert_eq!(decode("\\1011"), "A1");
    }

    /// bash keeps both characters of an escape it does not know; dropping the backslash would
    /// silently rewrite `$'\d'` into `d`.
    #[test]
    fn an_unknown_escape_is_kept_verbatim() {
        assert_eq!(decode("\\d"), "\\d");
        assert_eq!(decode("\\x"), "\\x");
        assert_eq!(decode("trailing\\"), "trailing\\");
    }

    /// A shell string is a C string: `$'a\0b'` is one character in bash too.
    #[test]
    fn a_nul_truncates_the_string() {
        assert_eq!(decode("a\\0b"), "a");
        assert_eq!(decode("a\\x00b"), "a");
    }
}

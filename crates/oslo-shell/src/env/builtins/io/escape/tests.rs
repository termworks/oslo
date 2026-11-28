//! The three escape dialects, each expectation taken from bash 5.3.

use super::{expand_b, expand_echo, printf_escape};

/// The whole format-string dialect over `text`, as `printf` walks it.
fn format(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        if text[at] == b'\\' {
            let (used, _) = printf_escape(&text[at + 1..], false, true, &mut out);
            at += 1 + used;
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

#[test]
fn the_format_takes_three_octal_digits_after_the_backslash() {
    assert_eq!(format(b"\\101\\33[0m"), b"A\x1b[0m");
    assert_eq!(format(b"\\0101"), b"\x081");
    assert_eq!(format(b"\\777\\12"), b"\xff\n");
    assert_eq!(format(b"\\\"\\?\\'"), b"\"?'");
    assert_eq!(format(b"a\\cb"), b"a\\cb");
    assert_eq!(format(b"\\E\\e\\q"), b"\x1b\x1b\\q");
}

#[test]
fn percent_b_takes_three_octal_digits_after_a_leading_zero() {
    assert_eq!(expand_b(b"\\0101|\\101|\\1234", true).0, b"A|A|S4");
    assert_eq!(expand_b(b"\\\"\\?", true).0, b"\\\"\\?");
    assert_eq!(expand_b(b"a\\cb", true), (b"a".to_vec(), true));
    assert_eq!(expand_b(b"end\\", true).0, b"end\\");
}

#[test]
fn hex_and_unicode_escapes_follow_the_locale() {
    assert_eq!(format(b"\\x41\\x4142"), b"AA42");
    assert_eq!(format(b"\\u00e9\\U0001F600"), "é😀".as_bytes());
    assert_eq!(format(b"\\uD800"), b"\xed\xa0\x80");
    assert_eq!(format(b"\\u00"), b"\0");
    let mut out = Vec::new();
    printf_escape(b"u20ac", false, false, &mut out);
    assert_eq!(out, b"\\u20AC");
}

#[test]
fn echo_only_reads_octal_after_a_zero() {
    assert_eq!(
        expand_echo(b"\\0101 \\101 \\E\\x\\u", true).0,
        b"A \\101 \x1b\\x\\u"
    );
    assert_eq!(expand_echo(b"\\U0001F600", true).0, "😀".as_bytes());
    assert_eq!(expand_echo(b"x\\cy", true), (b"x".to_vec(), true));
    assert_eq!(expand_echo(b"\\'\\\"a\\", true).0, b"\\'\\\"a\\");
}

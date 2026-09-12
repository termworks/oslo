//! Filenames are bytes; the shell's words are strings. This carries the bytes that are not UTF-8.
//!
//! **`rm b*` removed the wrong name.** A file called `bad\xffname` was read with
//! `to_string_lossy`, which turns `\xff` into `U+FFFD`, and the glob then handed `rm` a path that
//! does not exist. Expansion is `String` from the lexer to `execve`, and converting all of it to
//! `OsString` would touch every builtin — so the invalid bytes are carried inside the string
//! instead, and turned back into bytes wherever a string becomes a path or an argument.
//!
//! ```text
//!   byte 0xXY that is not part of valid UTF-8   ⇄   U+10FFXY
//! ```
//!
//! The last 256 code points of plane 16: private use, produced by no input method, so the only
//! collision is a real filename spelled in those code points — which is written down as the limit.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

const BASE: u32 = 0x10FF00;

fn is_raw(ch: char) -> bool {
    (BASE..=BASE + 0xFF).contains(&(ch as u32))
}

/// A name as the shell carries it: valid UTF-8 unchanged, each other byte as its private-use twin.
pub fn encode(name: &OsStr) -> String {
    let bytes = name.as_bytes();
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let mut out = String::with_capacity(bytes.len() + 8);
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for &byte in chunk.invalid() {
            out.extend(char::from_u32(BASE + u32::from(byte)));
        }
    }
    out
}

/// Whether `text` carries any byte that [`encode`] had to escape.
pub fn has_raw(text: &str) -> bool {
    text.chars().any(is_raw)
}

/// The original bytes of `text`.
pub fn decode(text: &str) -> Vec<u8> {
    if !has_raw(text) {
        return text.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        if is_raw(ch) {
            out.push((ch as u32 - BASE) as u8);
        } else {
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// `text` as a path or an argument: the bytes [`encode`] read, exactly.
pub fn to_os(text: &str) -> OsString {
    OsString::from_vec(decode(text))
}

/// Output already turned into bytes, with each carried byte put back.
///
/// For writers that build UTF-8 first — `echo`, `printf` — so `printf '%s' b*` writes the `\xff`
/// the file has. A carried byte is always the four-byte sequence `F4 8F BC..BF 80..BF`, which no
/// other character encodes to.
pub fn restore_bytes(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let carried = |w: &[u8]| w[0] == 0xF4 && w[1] == 0x8F && (0xBC..=0xBF).contains(&w[2]);
    if !bytes.windows(3).any(carried) {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len() && carried(&bytes[i..i + 3]) && (bytes[i + 3] & 0xC0) == 0x80 {
            out.push(((bytes[i + 2] & 0x03) << 6) | (bytes[i + 3] & 0x3F));
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_is_not_utf8_survives_the_round_trip() {
        let raw = OsStr::from_bytes(b"bad\xffname\xc3");
        let carried = encode(raw);
        assert!(has_raw(&carried));
        assert_eq!(decode(&carried), b"bad\xffname\xc3");
        assert_eq!(to_os(&carried), raw);
    }

    #[test]
    fn written_output_gets_its_bytes_back() {
        let carried = encode(OsStr::from_bytes(b"a\xffb\x80"));
        let text = format!("[{carried}]\n");
        assert_eq!(&*restore_bytes(text.as_bytes()), b"[a\xffb\x80]\n");
        assert_eq!(
            &*restore_bytes("plain é\n".as_bytes()),
            "plain é\n".as_bytes()
        );
    }

    #[test]
    fn valid_utf8_is_carried_unchanged() {
        let name = OsStr::new("émile.txt");
        assert_eq!(encode(name), "émile.txt");
        assert!(!has_raw("émile.txt"));
        assert_eq!(decode("émile.txt"), "émile.txt".as_bytes());
    }
}

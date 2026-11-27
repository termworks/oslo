//! `printf`'s arguments, read as each conversion wants them.
//!
//! Numbers are read the way C's `strtoimax`, `strtoumax` and `strtold` read them, and a bad one
//! is reported and still yields the prefix that did parse, which is what POSIX asks for.

use super::float::{self, Real, Width};

/// The arguments after the format, and whether any of them failed to convert.
pub struct Args<'a> {
    list: &'a [Vec<u8>],
    pub next: usize,
    /// Set when a conversion reported its argument; the final status is then 1.
    pub error: bool,
    utf8: bool,
    /// The whole command line, and the words `list` was decoded from, for diagnostics.
    argv: &'a [String],
    words: &'a [String],
}

/// What `%lc` read from its argument in a UTF-8 locale.
pub enum Wide {
    Nul,
    Char(char),
    /// A byte that does not start a character; it converts to nothing.
    Invalid,
}

/// An integer read from the front of a string: sign, magnitude, overflow, bytes used.
struct Integer {
    negative: bool,
    magnitude: u64,
    overflow: bool,
    end: usize,
}

fn read_integer(text: &[u8]) -> Integer {
    let mut at = text
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .count();
    let negative = text.get(at) == Some(&b'-');
    if matches!(text.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let digit_at = |i: usize, base: u32| text.get(i).and_then(|&b| (b as char).to_digit(base));
    let prefixed = text.get(at) == Some(&b'0');
    let base = match text.get(at + 1) {
        Some(b'x' | b'X') if prefixed && digit_at(at + 2, 16).is_some() => 16,
        Some(b'b' | b'B') if prefixed && digit_at(at + 2, 2).is_some() => 2,
        _ if prefixed => 8,
        _ => 10,
    };
    if matches!(base, 16 | 2) {
        at += 2;
    }
    let (start, mut magnitude, mut overflow) = (at, 0u64, false);
    while let Some(d) = digit_at(at, base) {
        match magnitude
            .checked_mul(u64::from(base))
            .and_then(|m| m.checked_add(u64::from(d)))
        {
            Some(m) => magnitude = m,
            None => overflow = true,
        }
        at += 1;
    }
    let end = if at == start { 0 } else { at };
    Integer {
        negative,
        magnitude,
        overflow,
        end,
    }
}

impl<'a> Args<'a> {
    pub fn new(list: &'a [Vec<u8>], utf8: bool) -> Self {
        Args {
            list,
            next: 0,
            error: false,
            utf8,
            argv: &[],
            words: &[],
        }
    }

    /// Name the command line and the argument words, so a diagnostic can point at its word.
    pub fn with_words(self, argv: &'a [String], words: &'a [String]) -> Self {
        Args {
            argv,
            words,
            ..self
        }
    }

    /// Report a problem with the argument taken last.
    pub fn complain(&self, body: &str, label: &str) {
        let word = self.next.checked_sub(1).and_then(|at| self.words.get(at));
        crate::env::complain(
            self.argv,
            word.map_or("", String::as_str),
            body,
            label,
            None,
        );
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    fn take(&mut self) -> Option<&'a [u8]> {
        let arg = self.list.get(self.next)?;
        self.next += 1;
        Some(arg)
    }

    /// `%c`: the first byte, or NUL.
    pub fn byte(&mut self) -> u8 {
        self.take().and_then(|a| a.first().copied()).unwrap_or(0)
    }

    /// `%s` and the rest: the argument, or nothing.
    pub fn string(&mut self) -> &'a [u8] {
        self.take().unwrap_or(b"")
    }

    /// `%lc` in a UTF-8 locale.
    pub fn wide_char(&mut self) -> Wide {
        let Some(arg) = self.take() else {
            return Wide::Nul;
        };
        match arg.utf8_chunks().next() {
            None => Wide::Nul,
            Some(chunk) => match chunk.valid().chars().next() {
                Some('\0') => Wide::Nul,
                Some(c) => Wide::Char(c),
                None => Wide::Invalid,
            },
        }
    }

    /// `%ls` in a UTF-8 locale: the characters, or every byte on its own if it is not UTF-8.
    pub fn wide_string(&mut self) -> Vec<Option<char>> {
        let arg = self.string();
        match std::str::from_utf8(arg) {
            Ok(text) => text.chars().map(Some).collect(),
            Err(_) => arg
                .iter()
                .map(|&b| b.is_ascii().then_some(b as char))
                .collect(),
        }
    }

    /// The value of the character after a leading quote.
    fn quoted(&self, arg: &[u8]) -> i64 {
        let rest = &arg[1..];
        if self.utf8
            && let Some(c) = rest
                .utf8_chunks()
                .next()
                .and_then(|c| c.valid().chars().next())
        {
            return i64::from(u32::from(c));
        }
        i64::from(rest.first().copied().unwrap_or(0))
    }

    /// Report an argument that did not convert whole, or overflowed.
    fn check(&mut self, arg: &[u8], end: usize, range: bool) {
        let shown = String::from_utf8_lossy(arg);
        if end == 0 || end < arg.len() {
            let what = match (arg.first(), arg.get(1)) {
                (Some(b'0'), Some(b'0'..=b'9')) => "invalid octal number",
                (Some(b'0'), Some(b'x')) => "invalid hex number",
                _ => "invalid number",
            };
            self.complain(&format!("printf: {shown}: {what}"), "not a number");
            self.error = true;
        } else if range {
            let body = format!("printf: {shown}: Numerical result out of range");
            self.complain(&body, "out of range");
            self.error = true;
        }
    }

    /// A `*` width or precision; one outside `int` is reported and becomes `overflow`.
    pub fn int(&mut self, overflow: i32) -> i32 {
        let Some(arg) = self.take() else { return 0 };
        if matches!(arg.first(), Some(b'\'' | b'"')) {
            return self.quoted(arg) as i32;
        }
        let read = read_integer(arg);
        let value = signed(&read);
        let wide = read.overflow || i32::try_from(value.0).is_err();
        self.check(arg, read.end, wide);
        if wide { overflow } else { value.0 as i32 }
    }

    /// `%d` and `%i`, as `strtoimax`.
    pub fn intmax(&mut self) -> i64 {
        let Some(arg) = self.take() else { return 0 };
        if matches!(arg.first(), Some(b'\'' | b'"')) {
            return self.quoted(arg);
        }
        let read = read_integer(arg);
        let (value, range) = signed(&read);
        self.check(arg, read.end, range);
        value
    }

    /// `%o`, `%u`, `%x` and `%X`, as `strtoumax`.
    pub fn uintmax(&mut self) -> u64 {
        let Some(arg) = self.take() else { return 0 };
        if matches!(arg.first(), Some(b'\'' | b'"')) {
            return self.quoted(arg) as u64;
        }
        let read = read_integer(arg);
        let value = match (read.overflow, read.negative) {
            (true, _) => u64::MAX,
            (false, true) => read.magnitude.wrapping_neg(),
            (false, false) => read.magnitude,
        };
        self.check(arg, read.end, read.overflow);
        value
    }

    /// The floating conversions, as `strtold` (or `strtod` for [`Width::Double`]).
    pub fn float(&mut self, width: Width) -> Real {
        let Some(arg) = self.take() else {
            return Real::from_int(0, width);
        };
        if matches!(arg.first(), Some(b'\'' | b'"')) {
            return Real::from_int(self.quoted(arg), width);
        }
        let read = float::parse(arg, width);
        self.check(arg, read.end, read.range);
        read.value
    }
}

/// `strtoimax`'s answer: the value, clamped, and whether it had to be.
fn signed(read: &Integer) -> (i64, bool) {
    let limit = if read.negative {
        1u64 << 63
    } else {
        i64::MAX as u64
    };
    if read.overflow || read.magnitude > limit {
        return (if read.negative { i64::MIN } else { i64::MAX }, true);
    }
    match read.negative {
        true => ((read.magnitude as i64).wrapping_neg(), false),
        false => (read.magnitude as i64, false),
    }
}

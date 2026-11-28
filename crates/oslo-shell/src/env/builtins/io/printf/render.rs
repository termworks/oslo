//! One conversion's text, padded to its field the way C's `printf` pads it.

use super::float::{self, Real, Width};
use super::out::Out;

/// The flags, width and precision of one conversion, with every `*` resolved.
#[derive(Default, Clone, Copy, Debug)]
pub struct Field {
    pub left: bool,
    pub plus: bool,
    pub space: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
}

impl Field {
    /// `prefix` (a sign, `0x`) then `body`, padded to the width; zeros go between the two when
    /// `zeros` allows them and the field is not left-justified.
    fn number(&self, out: &mut Out, prefix: &[u8], body: &[u8], zeros: bool) {
        let pad = self.width.saturating_sub(prefix.len() + body.len());
        if self.left {
            out.bytes(prefix);
            out.bytes(body);
            out.fill(b' ', pad);
        } else if zeros && self.zero {
            out.bytes(prefix);
            out.fill(b'0', pad);
            out.bytes(body);
        } else {
            out.fill(b' ', pad);
            out.bytes(prefix);
            out.bytes(body);
        }
    }

    /// `body` padded with spaces to the width, which counts `units` for the body.
    pub fn text_counted(&self, out: &mut Out, body: &[u8], units: usize) {
        let pad = self.width.saturating_sub(units);
        if !self.left {
            out.fill(b' ', pad);
        }
        out.bytes(body);
        if self.left {
            out.fill(b' ', pad);
        }
    }

    /// `%s`: at most `precision` bytes of `body`, padded with spaces.
    pub fn text(&self, out: &mut Out, body: &[u8]) {
        let cut = &body[..self.precision.unwrap_or(body.len()).min(body.len())];
        self.text_counted(out, cut, cut.len());
    }

    /// The sign a signed conversion prints for a value of this sign.
    fn sign(&self, negative: bool) -> &'static [u8] {
        match (negative, self.plus, self.space) {
            (true, _, _) => b"-",
            (false, true, _) => b"+",
            (false, false, true) => b" ",
            _ => b"",
        }
    }

    /// The digits of an integer conversion: at least `precision` of them, none for a zero
    /// precision and a zero value.
    fn digits(&self, text: String) -> Vec<u8> {
        match self.precision {
            Some(0) if text == "0" => Vec::new(),
            Some(p) => format!("{text:0>p$}").into_bytes(),
            None => text.into_bytes(),
        }
    }

    /// `%d` and `%i`.
    pub fn signed(&self, out: &mut Out, value: i64) {
        let body = self.digits(value.unsigned_abs().to_string());
        self.number(out, self.sign(value < 0), &body, self.precision.is_none());
    }

    /// `%o`, `%u`, `%x` and `%X`; `#` adds a leading `0` or a `0x`.
    pub fn unsigned(&self, out: &mut Out, value: u64, conversion: u8) {
        let (mut body, prefix): (Vec<u8>, &[u8]) = match conversion {
            b'o' => (self.digits(format!("{value:o}")), b""),
            b'x' => (
                self.digits(format!("{value:x}")),
                if self.alt && value != 0 { b"0x" } else { b"" },
            ),
            b'X' => (
                self.digits(format!("{value:X}")),
                if self.alt && value != 0 { b"0X" } else { b"" },
            ),
            _ => (self.digits(value.to_string()), b""),
        };
        if conversion == b'o' && self.alt && body.first() != Some(&b'0') {
            body.insert(0, b'0');
        }
        self.number(out, prefix, &body, self.precision.is_none());
    }

    /// `%f %F %e %E %g %G %a %A` of a value of `width`.
    pub fn float(&self, out: &mut Out, value: Real, conversion: u8, width: Width) {
        let upper = conversion.is_ascii_uppercase();
        let (negative, mant, exp) = match value {
            Real::Finite {
                negative,
                mant,
                exp,
            } => (negative, mant, exp),
            Real::Infinite { negative } | Real::Nan { negative } => {
                let word: &[u8] = match (matches!(value, Real::Nan { .. }), upper) {
                    (false, false) => b"inf",
                    (false, true) => b"INF",
                    (true, false) => b"nan",
                    (true, true) => b"NAN",
                };
                return self.number(out, self.sign(negative), word, false);
            }
        };
        let precision = self.precision.unwrap_or(6);
        let body = match conversion.to_ascii_lowercase() {
            b'f' => float::fixed(mant, exp, precision, self.alt),
            b'e' => float::exponent(mant, exp, precision, self.alt, upper),
            b'g' => float::general(mant, exp, precision, self.alt, upper),
            _ => {
                let body = float::hex(mant, exp, width, self.precision, self.alt, upper);
                let mut prefix = self.sign(negative).to_vec();
                prefix.extend_from_slice(if upper { b"0X" } else { b"0x" });
                return self.number(out, &prefix, &body, true);
            }
        };
        self.number(out, self.sign(negative), &body, true);
    }
}

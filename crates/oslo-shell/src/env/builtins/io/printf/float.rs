//! Floating-point arguments: read the way `strtold`/`strtod` read them, printed exactly.
//!
//! bash converts `%f` and friends through C's `long double` (`double` under `--posix` without
//! `L`). Both are modelled as a significand and a binary exponent with the target's width and
//! range, so every value printed is the value C would have held.

mod big;
mod decimal;
mod hex;

use big::Big;
pub use decimal::{exponent, fixed, general};
pub use hex::hex;

/// Which C type the conversion goes through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// `double`: 53 significant bits.
    Double,
    /// x87 `long double`: 64 significant bits, explicit leading bit.
    Extended,
}

impl Width {
    fn bits(self) -> i64 {
        match self {
            Width::Double => 53,
            Width::Extended => 64,
        }
    }

    /// The exponent of the smallest subnormal's only bit.
    fn least(self) -> i64 {
        match self {
            Width::Double => -1074,
            Width::Extended => -16445,
        }
    }

    /// The largest exponent a leading bit can have.
    fn greatest(self) -> i64 {
        match self {
            Width::Double => 1023,
            Width::Extended => 16383,
        }
    }
}

/// A value of one of the [`Width`]s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Real {
    /// `mant * 2^exp`; `mant` is below `2^bits`, and below `2^(bits-1)` only when subnormal.
    Finite {
        negative: bool,
        mant: u64,
        exp: i64,
    },
    Infinite {
        negative: bool,
    },
    Nan {
        negative: bool,
    },
}

impl Real {
    /// An integer converted to `width`, as C converts one in an assignment.
    pub fn from_int(value: i64, width: Width) -> Self {
        let one = Big::from_u64(1);
        round(
            &Big::from_u64(value.unsigned_abs()),
            &one,
            0,
            value < 0,
            width,
        )
        .0
    }
}

/// What reading a number from the front of an argument produced.
pub struct Parsed {
    pub value: Real,
    /// Bytes consumed; 0 when nothing could be read.
    pub end: usize,
    /// `ERANGE`: the value overflowed, or underflowed with lost bits.
    pub range: bool,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Read a number from the front of `text` as `strtold` does when `width` is extended.
pub fn parse(text: &[u8], width: Width) -> Parsed {
    let mut at = 0;
    while at < text.len() && is_space(text[at]) {
        at += 1;
    }
    let negative = text.get(at) == Some(&b'-');
    if matches!(text.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let rest = &text[at..];
    let none = Parsed {
        value: Real::Finite {
            negative: false,
            mant: 0,
            exp: 0,
        },
        end: 0,
        range: false,
    };
    let starts =
        |word: &[u8]| rest.len() >= word.len() && rest[..word.len()].eq_ignore_ascii_case(word);
    if starts(b"inf") {
        let used = if starts(b"infinity") { 8 } else { 3 };
        return Parsed {
            value: Real::Infinite { negative },
            end: at + used,
            range: false,
        };
    }
    if starts(b"nan") {
        let mut used = 3;
        if rest.get(3) == Some(&b'(') {
            let tail = rest[4..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                .count();
            if rest.get(4 + tail) == Some(&b')') {
                used = 5 + tail;
            }
        }
        return Parsed {
            value: Real::Nan { negative },
            end: at + used,
            range: false,
        };
    }
    let hexadecimal = rest.len() > 2
        && rest[0] == b'0'
        && matches!(rest[1], b'x' | b'X')
        && (rest[2].is_ascii_hexdigit()
            || (rest[2] == b'.' && rest.get(3).is_some_and(u8::is_ascii_hexdigit)));
    let read = match hexadecimal {
        true => read_hex(&rest[2..]).map(|(n, d, e, used)| (n, d, e, used + 2)),
        false => read_decimal(rest),
    };
    let Some((num, den, exp, used)) = read else {
        return none;
    };
    let (value, range) = round(&num, &den, exp, negative, width);
    Parsed {
        value,
        end: at + used,
        range,
    }
}

/// The digits of a hex float after `0x`: numerator, denominator, binary exponent, bytes used.
fn read_hex(text: &[u8]) -> Option<(Big, Big, i64, usize)> {
    let mut num = Big::from_u64(0);
    let (mut at, mut exp, mut point, mut digits) = (0, 0i64, false, 0);
    while at < text.len() {
        match text[at] {
            b'.' if !point => point = true,
            b if b.is_ascii_hexdigit() => {
                num.shl(4);
                num.add_small((b as char).to_digit(16).unwrap_or(0));
                digits += 1;
                if point {
                    exp -= 4;
                }
            }
            _ => break,
        }
        at += 1;
    }
    if digits == 0 {
        return None;
    }
    let (power, used) = read_exponent(&text[at..], b'p');
    Some((num, Big::from_u64(1), exp.saturating_add(power), at + used))
}

/// A decimal number: numerator, denominator, binary exponent, bytes used.
fn read_decimal(text: &[u8]) -> Option<(Big, Big, i64, usize)> {
    let mut kept = Vec::new();
    let (mut at, mut scale, mut point, mut digits) = (0, 0i64, false, 0);
    while at < text.len() {
        match text[at] {
            b'.' if !point => point = true,
            b if b.is_ascii_digit() => {
                digits += 1;
                if !(kept.is_empty() && b == b'0') {
                    kept.push(b - b'0');
                }
                if point {
                    scale -= 1;
                }
            }
            _ => break,
        }
        at += 1;
    }
    if digits == 0 {
        return None;
    }
    let (power, used) = read_exponent(&text[at..], b'e');
    let mut scale = scale.saturating_add(power);
    // Digits past every halfway point a `long double` has are folded into one sticky digit.
    const KEEP: usize = 12_000;
    if kept.len() > KEEP {
        let sticky = kept[KEEP..].iter().any(|&d| d != 0);
        scale += (kept.len() - KEEP) as i64;
        kept.truncate(KEEP);
        if sticky {
            kept.push(1);
            scale -= 1;
        }
    }
    while kept.last() == Some(&0) {
        kept.pop();
        scale += 1;
    }
    let mut num = Big::from_u64(0);
    for chunk in kept.chunks(9) {
        let mut value = 0u32;
        for &d in chunk {
            value = value * 10 + u32::from(d);
        }
        num.mul_small(10u32.pow(chunk.len() as u32));
        num.add_small(value);
    }
    if num.is_zero() {
        return Some((num, Big::from_u64(1), 0, at + used));
    }
    // Far outside every range: settle it without building the power of ten.
    let magnitude = scale + kept.len() as i64;
    if magnitude > 5000 {
        return Some((Big::from_u64(1), Big::from_u64(1), 1 << 20, at + used));
    }
    if magnitude < -5000 {
        return Some((Big::from_u64(1), Big::from_u64(1), -(1 << 20), at + used));
    }
    // `num * 10^scale` is `num * 5^scale * 2^scale`; the power of two goes into the exponent.
    match scale >= 0 {
        true => {
            let mut num = num;
            num.mul_pow5(scale as u64);
            Some((num, Big::from_u64(1), scale, at + used))
        }
        false => Some((num, Big::pow10(scale.unsigned_abs()), 0, at + used)),
    }
}

/// The exponent after the mantissa: `marker`, a sign, at least one digit. Returns it and its length.
fn read_exponent(text: &[u8], marker: u8) -> (i64, usize) {
    if !text
        .first()
        .is_some_and(|b| b.eq_ignore_ascii_case(&marker))
    {
        return (0, 0);
    }
    let mut at = 1;
    let negative = text.get(at) == Some(&b'-');
    if matches!(text.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let start = at;
    let mut value = 0i64;
    while at < text.len() && text[at].is_ascii_digit() {
        value = (value * 10 + i64::from(text[at] - b'0')).min(1 << 40);
        at += 1;
    }
    match at == start {
        true => (0, 0),
        false => (if negative { -value } else { value }, at),
    }
}

/// `num / den * 2^exp` rounded half-to-even to `width`, and whether that was a range error.
fn round(num: &Big, den: &Big, exp: i64, negative: bool, width: Width) -> (Real, bool) {
    if num.is_zero() {
        return (
            Real::Finite {
                negative,
                mant: 0,
                exp: 0,
            },
            false,
        );
    }
    let (a, b) = (num.bits() as i64, den.bits() as i64);
    let aligned = match a >= b {
        true => *num >= den.shifted((a - b) as u64),
        false => num.shifted((b - a) as u64) >= *den,
    };
    let top = a - b + exp - i64::from(!aligned);
    if top > width.greatest() {
        return (Real::Infinite { negative }, true);
    }
    if top < width.least() - 1 {
        return (
            Real::Finite {
                negative,
                mant: 0,
                exp: 0,
            },
            true,
        );
    }
    let mut quantum = (top - (width.bits() - 1)).max(width.least());
    let shift = exp - quantum;
    let (n2, d2) = match shift >= 0 {
        true => (num.shifted(shift as u64), den.clone()),
        false => (num.clone(), den.shifted(shift.unsigned_abs())),
    };
    let (mut mant, rem) = n2.div_rem(&d2);
    let twice = rem.shifted(1);
    if twice > d2 || (twice == d2 && mant & 1 == 1) {
        mant += 1;
    }
    if mant >> width.bits() != 0 {
        mant >>= 1;
        quantum += 1;
        if quantum + width.bits() - 1 > width.greatest() {
            return (Real::Infinite { negative }, true);
        }
    }
    let subnormal = mant >> (width.bits() - 1) == 0;
    let real = Real::Finite {
        negative,
        mant: mant as u64,
        exp: quantum,
    };
    (real, subnormal && !rem.is_zero())
}

#[cfg(test)]
#[path = "float/tests.rs"]
mod tests;

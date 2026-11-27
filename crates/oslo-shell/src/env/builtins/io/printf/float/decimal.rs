//! `%f`, `%e` and `%g` from the exact decimal expansion, rounded half to even as glibc rounds.

use super::big::Big;

/// The exact decimal expansion of `mant * 2^exp`: `0.d1d2d3... * 10^point`.
struct Digits {
    digits: Vec<u8>,
    point: i64,
}

impl Digits {
    fn of(mant: u64, exp: i64) -> Self {
        if mant == 0 {
            return Digits {
                digits: b"0".to_vec(),
                point: 1,
            };
        }
        let mut n = Big::from_u64(mant);
        let fraction = match exp >= 0 {
            true => {
                n.shl(exp as u64);
                0
            }
            false => {
                n.mul_pow5(exp.unsigned_abs());
                exp.unsigned_abs() as i64
            }
        };
        let digits = n.to_decimal();
        let point = digits.len() as i64 - fraction;
        Digits { digits, point }
    }

    fn is_zero(&self) -> bool {
        self.digits == b"0"
    }

    /// The first `keep` digits rounded half to even, and the point after any carry.
    fn round(&self, keep: i64) -> (Vec<u8>, i64) {
        if keep < 0 {
            return (Vec::new(), self.point);
        }
        let keep = keep as usize;
        let mut kept: Vec<u8> = (0..keep)
            .map(|i| *self.digits.get(i).unwrap_or(&b'0'))
            .collect();
        let next = self.digits.get(keep).copied().unwrap_or(b'0');
        let rest = self
            .digits
            .get(keep + 1..)
            .is_some_and(|r| r.iter().any(|&d| d != b'0'));
        let odd = kept.last().is_some_and(|&d| (d - b'0') % 2 == 1);
        let up = next > b'5' || (next == b'5' && (rest || odd));
        if !up {
            return (kept, self.point);
        }
        for digit in kept.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                return (kept, self.point);
            }
        }
        kept.insert(0, b'1');
        (kept, self.point + 1)
    }
}

/// `%f`: `precision` digits after the point.
pub fn fixed(mant: u64, exp: i64, precision: usize, alt: bool) -> Vec<u8> {
    let digits = Digits::of(mant, exp);
    let (kept, point) = digits.round(digits.point + precision as i64);
    let at = |i: i64| match i >= 0 && (i as usize) < kept.len() {
        true => kept[i as usize],
        false => b'0',
    };
    let mut out = Vec::with_capacity(point.max(1) as usize + precision + 1);
    match point > 0 {
        true => out.extend((0..point).map(at)),
        false => out.push(b'0'),
    }
    if precision > 0 || alt {
        out.push(b'.');
    }
    out.extend((point..point + precision as i64).map(at));
    out
}

/// `%e`: one digit, the point, `precision` digits, and a signed exponent of at least two digits.
pub fn exponent(mant: u64, exp: i64, precision: usize, alt: bool, upper: bool) -> Vec<u8> {
    let digits = Digits::of(mant, exp);
    let (mut kept, power) = match digits.is_zero() {
        true => (vec![b'0'; precision + 1], 1),
        false => digits.round(precision as i64 + 1),
    };
    kept.truncate(precision + 1);
    let mut out = vec![kept[0]];
    if precision > 0 || alt {
        out.push(b'.');
    }
    out.extend_from_slice(&kept[1..]);
    push_exponent(&mut out, power - 1, upper);
    out
}

fn push_exponent(out: &mut Vec<u8>, power: i64, upper: bool) {
    out.push(if upper { b'E' } else { b'e' });
    out.push(if power < 0 { b'-' } else { b'+' });
    out.extend_from_slice(format!("{:02}", power.unsigned_abs()).as_bytes());
}

/// `%g`: `%e` when the exponent is below -4 or not below the precision, `%f` otherwise, with
/// trailing zeros removed unless `alt`.
pub fn general(mant: u64, exp: i64, precision: usize, alt: bool, upper: bool) -> Vec<u8> {
    let significant = precision.max(1) as i64;
    let digits = Digits::of(mant, exp);
    let power = match digits.is_zero() {
        true => 0,
        false => digits.round(significant).1 - 1,
    };
    let mut out = match significant > power && power >= -4 {
        true => fixed(mant, exp, (significant - 1 - power) as usize, alt),
        false => exponent(mant, exp, (significant - 1) as usize, alt, upper),
    };
    if alt || !out.contains(&b'.') {
        return out;
    }
    let marker = out.iter().position(|&b| b == b'e' || b == b'E');
    let tail = marker.map(|at| out.split_off(at)).unwrap_or_default();
    while out.last() == Some(&b'0') {
        out.pop();
    }
    if out.last() == Some(&b'.') {
        out.pop();
    }
    out.extend_from_slice(&tail);
    out
}

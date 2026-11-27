//! `%a`: the significand in hex, laid out as glibc lays out its `double` and `long double`.

use super::Width;

/// Everything after `0x`: the leading digit, the fraction, `p` and a signed binary exponent.
///
/// `double` puts one bit in the leading digit and 13 digits after the point; `long double` puts
/// four bits there and 15 after it.
pub fn hex(
    mant: u64,
    exp: i64,
    width: Width,
    precision: Option<usize>,
    alt: bool,
    upper: bool,
) -> Vec<u8> {
    let nibbles: usize = match width {
        Width::Double => 13,
        Width::Extended => 15,
    };
    let fraction = mant & ((1u64 << (4 * nibbles)) - 1);
    let mut leading = (mant >> (4 * nibbles)) as u8;
    let mut digits: Vec<u8> = (0..nibbles)
        .rev()
        .map(|i| ((fraction >> (4 * i)) & 0xf) as u8)
        .collect();
    while digits.last() == Some(&0) {
        digits.pop();
    }
    let mut power = match mant {
        0 => 0,
        _ => exp + 4 * nibbles as i64,
    };
    let shown = precision.unwrap_or(digits.len());
    if shown < digits.len() {
        let last = if shown > 0 {
            digits[shown - 1]
        } else {
            leading
        };
        let next = digits[shown];
        let more = next & 7 != 0 || shown + 1 < digits.len();
        digits.truncate(shown);
        if next >= 8 && (last & 1 == 1 || more) {
            let carried = digits.iter_mut().rev().all(|d| {
                *d = (*d + 1) & 0xf;
                *d == 0
            });
            if carried {
                leading += 1;
                if leading > 0xf {
                    leading = 1;
                    power += 4;
                }
            }
        }
    }
    let digit = |d: u8| {
        let c = b"0123456789abcdef"[d as usize];
        if upper { c.to_ascii_uppercase() } else { c }
    };
    let mut out = vec![digit(leading)];
    if shown > 0 || alt {
        out.push(b'.');
    }
    out.extend(digits.iter().map(|&d| digit(d)));
    out.resize(out.len() + shown - digits.len(), b'0');
    out.push(if upper { b'P' } else { b'p' });
    out.push(if power < 0 { b'-' } else { b'+' });
    out.extend_from_slice(power.unsigned_abs().to_string().as_bytes());
    out
}

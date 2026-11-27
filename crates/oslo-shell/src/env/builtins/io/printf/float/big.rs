//! An unsigned integer of any size: just the operations exact decimal and binary conversion need.

use std::cmp::Ordering;

/// Little-endian 32-bit limbs, never with a trailing zero limb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big(Vec<u32>);

impl Big {
    pub fn from_u64(value: u64) -> Self {
        let mut big = Big(vec![value as u32, (value >> 32) as u32]);
        big.trim();
        big
    }

    /// `5^n`.
    pub fn pow5(n: u64) -> Self {
        let mut big = Big::from_u64(1);
        big.mul_pow5(n);
        big
    }

    /// Multiply by `5^n`, a 13th power at a time.
    pub fn mul_pow5(&mut self, n: u64) {
        let mut left = n;
        while left >= 13 {
            self.mul_small(1_220_703_125);
            left -= 13;
        }
        self.mul_small(5u32.pow(left as u32));
    }

    /// `10^n`.
    pub fn pow10(n: u64) -> Self {
        let mut big = Big::pow5(n);
        big.shl(n);
        big
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of significant bits.
    pub fn bits(&self) -> u64 {
        match self.0.last() {
            Some(top) => self.0.len() as u64 * 32 - u64::from(top.leading_zeros()),
            None => 0,
        }
    }

    pub fn mul_small(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.0 {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
        if carry != 0 {
            self.0.push(carry as u32);
        }
        self.trim();
    }

    pub fn add_small(&mut self, addend: u32) {
        let mut carry = u64::from(addend);
        for limb in &mut self.0 {
            if carry == 0 {
                return;
            }
            let sum = u64::from(*limb) + carry;
            *limb = sum as u32;
            carry = sum >> 32;
        }
        if carry != 0 {
            self.0.push(carry as u32);
        }
    }

    pub fn shl(&mut self, n: u64) {
        if self.is_zero() || n == 0 {
            return;
        }
        let (limbs, bits) = ((n / 32) as usize, (n % 32) as u32);
        if bits != 0 {
            let mut carry = 0u32;
            for limb in &mut self.0 {
                let next = *limb >> (32 - bits);
                *limb = (*limb << bits) | carry;
                carry = next;
            }
            if carry != 0 {
                self.0.push(carry);
            }
        }
        self.0.splice(0..0, std::iter::repeat_n(0, limbs));
    }

    pub fn shifted(&self, n: u64) -> Self {
        let mut copy = self.clone();
        copy.shl(n);
        copy
    }

    /// Subtract `other`, which must not be larger.
    pub fn sub_assign(&mut self, other: &Big) {
        let mut borrow = 0i64;
        for (at, limb) in self.0.iter_mut().enumerate() {
            let diff = i64::from(*limb) - i64::from(other.0.get(at).copied().unwrap_or(0)) - borrow;
            *limb = diff.rem_euclid(1 << 32) as u32;
            borrow = i64::from(diff < 0);
        }
        self.trim();
    }

    /// Divide by `divisor` in place, returning the remainder.
    pub fn div_small(&mut self, divisor: u32) -> u32 {
        let mut rem = 0u64;
        for limb in self.0.iter_mut().rev() {
            let cur = (rem << 32) | u64::from(*limb);
            *limb = (cur / u64::from(divisor)) as u32;
            rem = cur % u64::from(divisor);
        }
        self.trim();
        rem as u32
    }

    /// `self / divisor` and the remainder, for a quotient known to fit in 127 bits.
    pub fn div_rem(&self, divisor: &Big) -> (u128, Big) {
        let mut rem = self.clone();
        let mut quotient = 0u128;
        let span = self.bits().saturating_sub(divisor.bits());
        for at in (0..=span.min(126)).rev() {
            let step = divisor.shifted(at);
            if rem.cmp(&step) != Ordering::Less {
                rem.sub_assign(&step);
                quotient |= 1 << at;
            }
        }
        (quotient, rem)
    }

    /// The decimal digits as ASCII, `"0"` for zero.
    pub fn to_decimal(&self) -> Vec<u8> {
        if self.is_zero() {
            return b"0".to_vec();
        }
        let mut work = self.clone();
        let mut chunks = Vec::new();
        while !work.is_zero() {
            chunks.push(work.div_small(1_000_000_000));
        }
        let mut out = Vec::with_capacity(chunks.len() * 9);
        let top = chunks.pop().unwrap_or(0);
        out.extend_from_slice(top.to_string().as_bytes());
        for chunk in chunks.iter().rev() {
            out.extend_from_slice(format!("{chunk:09}").as_bytes());
        }
        out
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Big {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

#[cfg(test)]
mod tests {
    use super::Big;

    #[test]
    fn arithmetic_round_trips_through_decimal() {
        let mut n = Big::pow10(30);
        assert_eq!(n.to_decimal(), format!("1{}", "0".repeat(30)).as_bytes());
        n.add_small(7);
        n.mul_small(3);
        assert_eq!(n.to_decimal(), format!("3{}21", "0".repeat(28)).as_bytes());
        let (q, rem) = n.div_rem(&Big::pow10(20));
        assert_eq!(q, 30_000_000_000);
        assert_eq!(rem.to_decimal(), b"21");
        assert_eq!(Big::from_u64(0).to_decimal(), b"0");
        assert_eq!(
            Big::pow5(27).to_decimal(),
            5u128.pow(27).to_string().as_bytes()
        );
        assert_eq!(Big::from_u64(1).shifted(100).bits(), 101);
    }
}

//! Natural numbers up to 4096 bits and arithmetic modulo an odd number in Montgomery form, for
//! checking signatures (ECDSA's curves, RSA). Only public values go through here: the time
//! these take depends on the numbers.

use core::cmp::Ordering;
use core::fmt;

/// The most 64-bit limbs a number has: 4096 bits.
pub const MAX_LIMBS: usize = 64;
/// The most bytes a number has.
pub const MAX_BYTES: usize = MAX_LIMBS * 8;

/// A natural number below 2^4096, in little-endian 64-bit limbs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Number {
    limbs: [u64; MAX_LIMBS],
}

impl fmt::Debug for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Number({} bits)", self.bits())
    }
}

impl Number {
    /// Zero.
    pub const ZERO: Self = Self {
        limbs: [0; MAX_LIMBS],
    };

    /// A number from one limb.
    pub const fn from_u64(value: u64) -> Self {
        let mut number = Self::ZERO;
        number.limbs[0] = value;
        number
    }

    /// The number written in big-endian `bytes`; `None` when it needs more than 4096 bits.
    pub fn from_be_bytes(bytes: &[u8]) -> Option<Self> {
        let first = bytes
            .iter()
            .position(|&byte| byte != 0)
            .unwrap_or(bytes.len());
        let bytes = &bytes[first..];
        if bytes.len() > MAX_BYTES {
            return None;
        }
        let mut number = Self::ZERO;
        for (index, &byte) in bytes.iter().rev().enumerate() {
            number.limbs[index / 8] |= u64::from(byte) << (8 * (index % 8));
        }
        Some(number)
    }

    /// Writes the number's low `out.len()` bytes into `out`, big-endian.
    pub fn write_be_bytes(&self, out: &mut [u8]) {
        let length = out.len();
        for (index, byte) in out.iter_mut().enumerate() {
            let position = length - 1 - index;
            *byte = match self.limbs.get(position / 8) {
                Some(limb) => (limb >> (8 * (position % 8))) as u8,
                None => 0,
            };
        }
    }

    /// The number of bits up to the highest set one; 0 for zero.
    pub fn bits(&self) -> usize {
        match self.limbs.iter().rposition(|&limb| limb != 0) {
            Some(index) => index * 64 + 64 - self.limbs[index].leading_zeros() as usize,
            None => 0,
        }
    }

    /// Whether the number is zero.
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&limb| limb == 0)
    }

    /// Bit `index`, counted from the least significant.
    pub fn bit(&self, index: usize) -> bool {
        self.limbs
            .get(index / 64)
            .is_some_and(|limb| (limb >> (index % 64)) & 1 == 1)
    }

    fn add_assign(&mut self, other: &Self) -> bool {
        let mut carry = 0u64;
        for (limb, &added) in self.limbs.iter_mut().zip(&other.limbs) {
            let (sum, first) = limb.overflowing_add(added);
            let (sum, second) = sum.overflowing_add(carry);
            *limb = sum;
            carry = u64::from(first) + u64::from(second);
        }
        carry != 0
    }

    fn subtract_assign(&mut self, other: &Self) -> bool {
        let mut borrow = 0u64;
        for (limb, &taken) in self.limbs.iter_mut().zip(&other.limbs) {
            let (difference, first) = limb.overflowing_sub(taken);
            let (difference, second) = difference.overflowing_sub(borrow);
            *limb = difference;
            borrow = u64::from(first) + u64::from(second);
        }
        borrow != 0
    }

    fn double_assign(&mut self) -> bool {
        let mut carry = 0u64;
        for limb in self.limbs.iter_mut() {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        carry != 0
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Number {
    fn cmp(&self, other: &Self) -> Ordering {
        self.limbs.iter().rev().cmp(other.limbs.iter().rev())
    }
}

/// An odd modulus above 2, ready for Montgomery multiplication with R = 2^(64 * limbs).
#[derive(Clone, Copy)]
pub struct Modulus {
    value: Number,
    limbs: usize,
    inverse: u64,
    r_squared: Number,
}

impl fmt::Debug for Modulus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Modulus({} bits)", self.value.bits())
    }
}

impl Modulus {
    /// `value` as a modulus; `None` when it is even or below 3.
    pub fn new(value: &Number) -> Option<Self> {
        if !value.bit(0) || value.bits() < 2 {
            return None;
        }
        let limbs = value.bits().div_ceil(64);
        let mut inverse = 1u64;
        for _ in 0..6 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(value.limbs[0].wrapping_mul(inverse)));
        }
        let mut modulus = Self {
            value: *value,
            limbs,
            inverse: inverse.wrapping_neg(),
            r_squared: Number::ZERO,
        };
        let mut r_squared = Number::from_u64(1);
        for _ in 0..128 * limbs {
            let carried = r_squared.double_assign();
            if carried || r_squared >= *value {
                r_squared.subtract_assign(value);
            }
        }
        modulus.r_squared = r_squared;
        Some(modulus)
    }

    /// The modulus.
    pub fn value(&self) -> &Number {
        &self.value
    }

    /// The modulus's bits.
    pub fn bits(&self) -> usize {
        self.value.bits()
    }

    /// `x` modulo the modulus, for any `x`.
    pub fn reduce(&self, x: &Number) -> Number {
        if *x < self.value {
            return *x;
        }
        let mut remainder = Number::ZERO;
        for bit in (0..x.bits()).rev() {
            let carried = remainder.double_assign();
            if x.bit(bit) {
                remainder.limbs[0] |= 1;
            }
            if carried || remainder >= self.value {
                remainder.subtract_assign(&self.value);
            }
        }
        remainder
    }

    /// `x` (any number) in Montgomery form: x R mod n.
    pub fn to_montgomery(&self, x: &Number) -> Number {
        self.multiply(&self.reduce(x), &self.r_squared)
    }

    /// `x` back from Montgomery form.
    pub fn from_montgomery(&self, x: &Number) -> Number {
        self.multiply(x, &Number::from_u64(1))
    }

    /// One in Montgomery form.
    pub fn one(&self) -> Number {
        self.to_montgomery(&Number::from_u64(1))
    }

    /// The Montgomery product a b / R mod n of `a` and `b`, both below the modulus.
    pub fn multiply(&self, a: &Number, b: &Number) -> Number {
        let count = self.limbs;
        let modulus = &self.value.limbs;
        let mut t = [0u64; MAX_LIMBS + 2];
        for i in 0..count {
            let mut carry = 0u64;
            for j in 0..count {
                let sum = u128::from(t[j])
                    + u128::from(a.limbs[j]) * u128::from(b.limbs[i])
                    + u128::from(carry);
                t[j] = sum as u64;
                carry = (sum >> 64) as u64;
            }
            let sum = u128::from(t[count]) + u128::from(carry);
            t[count] = sum as u64;
            t[count + 1] = (sum >> 64) as u64;
            let factor = t[0].wrapping_mul(self.inverse);
            let sum = u128::from(t[0]) + u128::from(factor) * u128::from(modulus[0]);
            let mut carry = (sum >> 64) as u64;
            for j in 1..count {
                let sum = u128::from(t[j])
                    + u128::from(factor) * u128::from(modulus[j])
                    + u128::from(carry);
                t[j - 1] = sum as u64;
                carry = (sum >> 64) as u64;
            }
            let sum = u128::from(t[count]) + u128::from(carry);
            t[count - 1] = sum as u64;
            t[count] = t[count + 1] + (sum >> 64) as u64;
            t[count + 1] = 0;
        }
        let mut result = Number::ZERO;
        result.limbs[..count].copy_from_slice(&t[..count]);
        if t[count] != 0 || result >= self.value {
            result.subtract_assign(&self.value);
            result.limbs[count..].fill(0);
        }
        result
    }

    /// (a + b) mod n for `a` and `b` below the modulus.
    pub fn add(&self, a: &Number, b: &Number) -> Number {
        let mut sum = *a;
        let carried = sum.add_assign(b);
        if carried || sum >= self.value {
            sum.subtract_assign(&self.value);
        }
        sum
    }

    /// (a - b) mod n for `a` and `b` below the modulus.
    pub fn subtract(&self, a: &Number, b: &Number) -> Number {
        let mut difference = *a;
        if difference.subtract_assign(b) {
            difference.add_assign(&self.value);
        }
        difference
    }

    /// `base` (in Montgomery form) to the power `exponent`, in Montgomery form.
    pub fn power(&self, base: &Number, exponent: &Number) -> Number {
        let mut result = self.one();
        for bit in (0..exponent.bits()).rev() {
            result = self.multiply(&result, &result);
            if exponent.bit(bit) {
                result = self.multiply(&result, base);
            }
        }
        result
    }

    /// The inverse of `a` (in Montgomery form, not zero) when the modulus is prime, by
    /// Fermat's little theorem; in Montgomery form.
    pub fn invert_prime(&self, a: &Number) -> Number {
        let mut exponent = self.value;
        exponent.subtract_assign(&Number::from_u64(2));
        self.power(a, &exponent)
    }
}

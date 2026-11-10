//! The exact decimal value of a finite double, digit for digit, and its rounding to a number of
//! digits, half to even on an exact tie, as C's `printf` rounds `%f`, `%e` and `%g` (glibc, and
//! the Universal C Runtime since Visual Studio 2019). A double is `m * 2^e`; for `e >= 0` its
//! digits are those of the integer `m << e`, otherwise those of `m * 5^-e` with the decimal
//! point `-e` places from the right, both computed in a fixed-size big integer.

const LIMBS: usize = 82;
const MAX_DIGITS: usize = 780;
const CHUNK_DIVISOR: u32 = 1_000_000_000;
const CHUNK_DIGITS: usize = 9;
const FIVE_POW_13: u32 = 1_220_703_125;
const FRACTION_BITS: u32 = 52;
const EXPONENT_BIAS_AND_FRACTION: i32 = 1075;
const SUBNORMAL_EXPONENT: i32 = -1074;

struct Big {
    limbs: [u32; LIMBS],
    length: usize,
}

impl Big {
    fn of(value: u64) -> Self {
        let mut big = Self {
            limbs: [0; LIMBS],
            length: 2,
        };
        big.limbs[0] = value as u32;
        big.limbs[1] = (value >> 32) as u32;
        big.trim();
        big
    }

    fn trim(&mut self) {
        while self.length > 0 && self.limbs[self.length - 1] == 0 {
            self.length -= 1;
        }
    }

    fn multiply(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.limbs[..self.length] {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
        if carry != 0 && self.length < LIMBS {
            self.limbs[self.length] = carry as u32;
            self.length += 1;
        }
    }

    fn shift_left(&mut self, bits: u32) {
        let whole = (bits / 32) as usize;
        let part = bits % 32;
        let length = (self.length + whole + 1).min(LIMBS);
        for index in (0..length).rev() {
            let source = index.checked_sub(whole);
            let high = source.map_or(0, |at| self.limbs.get(at).copied().unwrap_or(0));
            let low = source
                .and_then(|at| at.checked_sub(1))
                .map_or(0, |at| self.limbs[at]);
            self.limbs[index] = match part {
                0 => high,
                _ => (high << part) | (low >> (32 - part)),
            };
        }
        self.length = length;
        self.trim();
    }

    fn divide(&mut self, divisor: u32) -> u32 {
        let mut remainder = 0u64;
        for limb in self.limbs[..self.length].iter_mut().rev() {
            let current = (remainder << 32) | u64::from(*limb);
            *limb = (current / u64::from(divisor)) as u32;
            remainder = current % u64::from(divisor);
        }
        self.trim();
        remainder as u32
    }
}

/// A non-negative finite double's decimal digits, `0.d1d2d3... * 10^point`, the first digit not
/// zero; no digits for zero.
pub(crate) struct ExactDecimal {
    digits: [u8; MAX_DIGITS],
    length: usize,
    point: i32,
}

impl ExactDecimal {
    /// The exact digits of `magnitude`, which is finite and not negative.
    pub(crate) fn of(magnitude: f64) -> Self {
        let mut decimal = Self {
            digits: [0; MAX_DIGITS],
            length: 0,
            point: 1,
        };
        let bits = magnitude.to_bits();
        let field = ((bits >> FRACTION_BITS) & 0x7ff) as i32;
        let fraction = bits & ((1 << FRACTION_BITS) - 1);
        let (mut significand, mut exponent) = match field {
            0 => (fraction, SUBNORMAL_EXPONENT),
            _ => (
                fraction | (1 << FRACTION_BITS),
                field - EXPONENT_BIAS_AND_FRACTION,
            ),
        };
        if significand == 0 {
            return decimal;
        }
        while exponent < 0 && significand & 1 == 0 {
            significand >>= 1;
            exponent += 1;
        }
        let mut big = Big::of(significand);
        let mut fraction_digits = 0;
        if exponent >= 0 {
            big.shift_left(exponent as u32);
        } else {
            let mut fives = exponent.unsigned_abs();
            fraction_digits = fives as i32;
            while fives >= 13 {
                big.multiply(FIVE_POW_13);
                fives -= 13;
            }
            big.multiply(5u32.pow(fives));
        }
        decimal.fill_from(&mut big);
        decimal.point = decimal.length as i32 - fraction_digits;
        decimal
    }

    fn fill_from(&mut self, big: &mut Big) {
        let mut reversed = [0u8; MAX_DIGITS + CHUNK_DIGITS];
        let mut count = 0;
        while big.length > 0 && count + CHUNK_DIGITS <= reversed.len() {
            let mut chunk = big.divide(CHUNK_DIVISOR);
            for _ in 0..CHUNK_DIGITS {
                reversed[count] = (chunk % 10) as u8;
                chunk /= 10;
                count += 1;
            }
        }
        while count > 0 && reversed[count - 1] == 0 {
            count -= 1;
        }
        let count = count.min(MAX_DIGITS);
        for (digit, from) in self.digits[..count]
            .iter_mut()
            .zip(reversed[..count].iter().rev())
        {
            *digit = *from;
        }
        self.length = count;
    }

    /// Whether the value is zero.
    pub(crate) fn is_zero(&self) -> bool {
        self.length == 0
    }

    /// Where the decimal point is: the value is `0.d1d2d3... * 10^point`.
    pub(crate) fn point(&self) -> i32 {
        self.point
    }

    /// The digit `index` places after the first (0 for the first), 0 past the last or before it.
    pub(crate) fn digit(&self, index: i32) -> u8 {
        usize::try_from(index)
            .ok()
            .filter(|&at| at < self.length)
            .map_or(0, |at| self.digits[at])
    }

    /// Keeps the first `keep` digits, rounding half to even on what follows; none (zero) for a
    /// `keep` below 0, or for 0 when the value is at most half a unit of the first digit's place.
    pub(crate) fn round_to(&mut self, keep: i32) {
        let Ok(keep) = usize::try_from(keep) else {
            self.length = 0;
            return;
        };
        if keep >= self.length {
            return;
        }
        let first = self.digits[keep];
        let beyond = self.digits[keep + 1..self.length].iter().any(|&d| d != 0);
        let odd = keep > 0 && self.digits[keep - 1] % 2 == 1;
        let up = first > 5 || (first == 5 && (beyond || odd));
        self.length = keep;
        if !up {
            return;
        }
        for index in (0..keep).rev() {
            if self.digits[index] == 9 {
                self.digits[index] = 0;
            } else {
                self.digits[index] += 1;
                return;
            }
        }
        self.digits[0] = 1;
        self.length = 1;
        self.point += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(decimal: &ExactDecimal) -> String {
        decimal.digits[..decimal.length]
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect()
    }

    #[test]
    fn a_double_s_digits_are_its_exact_value() {
        let tenth = ExactDecimal::of(0.1);
        assert_eq!(tenth.point(), 0);
        assert_eq!(
            text(&tenth),
            "1000000000000000055511151231257827021181583404541015625"
        );
        let big = ExactDecimal::of(2f64.powi(60));
        assert_eq!(
            (text(&big).as_str(), big.point()),
            ("1152921504606846976", 19)
        );
        let largest = ExactDecimal::of(f64::MAX);
        assert_eq!(largest.point(), 309);
        assert!(text(&largest).starts_with("17976931348623157081452742373170435679"));
        let smallest = ExactDecimal::of(f64::from_bits(1));
        assert_eq!(smallest.point(), -323);
        assert!(text(&smallest).starts_with("4940656458412465441765687928682213723650598"));
        assert_eq!(text(&smallest).len(), 751);
        assert!(ExactDecimal::of(0.0).is_zero());
    }

    #[test]
    fn rounding_is_half_to_even_only_on_an_exact_tie() {
        let mut root = ExactDecimal::of(2f64.sqrt());
        root.round_to(17);
        assert_eq!(text(&root), "14142135623730951");
        for (value, keep, digits, point) in [
            (0.5, 0, "", 0),
            (1.5, 1, "2", 1),
            (2.5, 1, "2", 1),
            (0.125, 2, "12", 0),
            (0.375, 2, "38", 0),
            (9.96, 2, "1", 2),
            (0.0005, 0, "1", -2),
        ] {
            let mut decimal = ExactDecimal::of(value);
            decimal.round_to(keep);
            assert_eq!(
                (text(&decimal).as_str(), decimal.point()),
                (digits, point),
                "{value}"
            );
        }
        let mut more_than_half = ExactDecimal::of(0.6);
        more_than_half.round_to(0);
        assert_eq!(
            (text(&more_than_half).as_str(), more_than_half.point()),
            ("1", 1)
        );
        let mut gone = ExactDecimal::of(0.6);
        gone.round_to(-1);
        assert!(gone.is_zero());
    }
}

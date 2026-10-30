//! Numbers as Swift's `description` writes them (D180): integers in decimal, and `Double` as the
//! shortest digits that read back as the same value (Burger and Dybvig's free-format algorithm
//! on exact big integers), in Swift's notation: `2.0`, `0.001`, `1e-05`, `1e+16`, `inf`,
//! `nan`. Pure logic with host tests.

const LIMBS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Big {
    limbs: [u32; LIMBS],
}

impl Big {
    fn from_u64(value: u64) -> Self {
        let mut limbs = [0u32; LIMBS];
        limbs[0] = value as u32;
        limbs[1] = (value >> 32) as u32;
        Self { limbs }
    }

    fn shift_left(&mut self, bits: u32) {
        let words = (bits / 32) as usize;
        let rest = bits % 32;
        if words > 0 {
            for index in (0..LIMBS).rev() {
                self.limbs[index] = if index >= words {
                    self.limbs[index - words]
                } else {
                    0
                };
            }
        }
        if rest > 0 {
            let mut carry = 0u32;
            for limb in &mut self.limbs {
                let next = *limb >> (32 - rest);
                *limb = (*limb << rest) | carry;
                carry = next;
            }
        }
    }

    fn multiply_small(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
    }

    fn multiply_power_of_ten(&mut self, mut exponent: u32) {
        while exponent >= 9 {
            self.multiply_small(1_000_000_000);
            exponent -= 9;
        }
        self.multiply_small(10u32.pow(exponent));
    }

    fn add(&self, other: &Self) -> Self {
        let mut sum = *self;
        let mut carry = 0u64;
        for (limb, &addend) in sum.limbs.iter_mut().zip(&other.limbs) {
            let total = u64::from(*limb) + u64::from(addend) + carry;
            *limb = total as u32;
            carry = total >> 32;
        }
        sum
    }

    fn subtract(&mut self, other: &Self) {
        let mut borrow = 0i64;
        for (limb, &subtrahend) in self.limbs.iter_mut().zip(&other.limbs) {
            let difference = i64::from(*limb) - i64::from(subtrahend) - borrow;
            *limb = difference as u32;
            borrow = i64::from(difference < 0);
        }
    }

    fn compare(&self, other: &Self) -> core::cmp::Ordering {
        self.limbs.iter().rev().cmp(other.limbs.iter().rev())
    }
}

/// The shortest decimal digits of the positive finite `value`, which reads back as `value`
/// and is the closest such: `(digits, count, exponent)` with `value` = 0.d1d2...dn × 10^exponent.
pub fn shortest(value: f64) -> ([u8; 17], usize, i32) {
    use core::cmp::Ordering::{Greater, Less};
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), biased - 1075)
    };
    let even = mantissa % 2 == 0;
    let lower_closer = biased > 1 && fraction == 0;
    let mut r = Big::from_u64(mantissa);
    let mut s = Big::from_u64(1);
    let mut low = Big::from_u64(1);
    let (r_shift, s_shift, high_shift) = match (exponent >= 0, lower_closer) {
        (true, false) => (exponent as u32 + 1, 1, 0),
        (true, true) => (exponent as u32 + 2, 2, 1),
        (false, false) => (1, (-exponent) as u32 + 1, 0),
        (false, true) => (2, (-exponent) as u32 + 2, 1),
    };
    if exponent >= 0 {
        low.shift_left(exponent as u32);
    }
    let mut high = low;
    high.shift_left(high_shift);
    r.shift_left(r_shift);
    s.shift_left(s_shift);
    let bit_length = 64 - mantissa.leading_zeros() as i32;
    let lower_bound = f64::from(exponent + bit_length - 1) * core::f64::consts::LOG10_2 - 1e-10;
    let truncated = lower_bound as i32;
    let mut k = if f64::from(truncated) < lower_bound {
        truncated + 1
    } else {
        truncated
    };
    if k >= 0 {
        s.multiply_power_of_ten(k as u32);
    } else {
        let scale = (-k) as u32;
        r.multiply_power_of_ten(scale);
        high.multiply_power_of_ten(scale);
        low.multiply_power_of_ten(scale);
    }
    let reaches = |r: &Big, high: &Big, s: &Big| {
        let top = r.add(high).compare(s);
        if even { top != Less } else { top == Greater }
    };
    if reaches(&r, &high, &s) {
        s.multiply_small(10);
        k += 1;
    }
    let mut digits = [0u8; 17];
    let mut count = 0;
    loop {
        r.multiply_small(10);
        high.multiply_small(10);
        low.multiply_small(10);
        let mut digit = 0u8;
        while r.compare(&s) != Less {
            r.subtract(&s);
            digit += 1;
        }
        let low_reached = if even {
            r.compare(&low) != Greater
        } else {
            r.compare(&low) == Less
        };
        let high_reached = reaches(&r, &high, &s);
        let last = match (low_reached, high_reached) {
            (false, false) => None,
            (true, false) => Some(digit),
            (false, true) => Some(digit + 1),
            (true, true) => {
                let mut twice = r;
                twice.shift_left(1);
                Some(if twice.compare(&s) == Less {
                    digit
                } else {
                    digit + 1
                })
            }
        };
        match last {
            Some(final_digit) => {
                digits[count.min(16)] = final_digit;
                count += 1;
                break;
            }
            None => {
                if count == 16 {
                    digits[16] = digit;
                    count += 1;
                    break;
                }
                digits[count] = digit;
                count += 1;
            }
        }
    }
    (digits, count.min(17), k)
}

/// Writes into a fixed buffer.
#[derive(Debug)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }
}

impl<const N: usize> Text<N> {
    pub fn push(&mut self, byte: u8) {
        if self.length < N {
            self.bytes[self.length] = byte;
            self.length += 1;
        }
    }

    pub fn push_all(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// `value` in decimal, as `Int.description` writes it.
pub fn signed(value: i64) -> Text<24> {
    let mut text = Text::default();
    if value < 0 {
        text.push(b'-');
    }
    text.push_all(unsigned(value.unsigned_abs()).as_bytes());
    text
}

/// `value` in decimal, as `UInt.description` writes it.
pub fn unsigned(mut value: u64) -> Text<24> {
    let mut reversed = [0u8; 20];
    let mut count = 0;
    loop {
        reversed[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut text = Text::default();
    for &digit in reversed[..count].iter().rev() {
        text.push(digit);
    }
    text
}

const EXPONENTIAL_ABOVE: f64 = 9_007_199_254_740_992.0;

/// `value` as `Double.description` writes it.
pub fn double(value: f64) -> Text<32> {
    let mut text = Text::default();
    if value.is_nan() {
        if value.is_sign_negative() {
            text.push(b'-');
        }
        text.push_all(b"nan");
        return text;
    }
    if value.is_sign_negative() {
        text.push(b'-');
    }
    let magnitude = value.abs();
    if magnitude.is_infinite() {
        text.push_all(b"inf");
        return text;
    }
    if magnitude == 0.0 {
        text.push_all(b"0.0");
        return text;
    }
    let (digits, count, k) = shortest(magnitude);
    let digits = &digits[..count];
    let ascii = |digit: &u8| b'0' + digit;
    if k < -3 || magnitude > EXPONENTIAL_ABOVE {
        text.push(ascii(&digits[0]));
        if count > 1 {
            text.push(b'.');
            for digit in &digits[1..] {
                text.push(ascii(digit));
            }
        }
        let exponent = k - 1;
        text.push(b'e');
        text.push(if exponent < 0 { b'-' } else { b'+' });
        let magnitude = unsigned(u64::from(exponent.unsigned_abs()));
        if magnitude.as_bytes().len() < 2 {
            text.push(b'0');
        }
        text.push_all(magnitude.as_bytes());
    } else if k <= 0 {
        text.push_all(b"0.");
        for _ in 0..-k {
            text.push(b'0');
        }
        for digit in digits {
            text.push(ascii(digit));
        }
    } else {
        let point = k as usize;
        for (index, digit) in digits.iter().enumerate() {
            if index == point {
                text.push(b'.');
            }
            text.push(ascii(digit));
        }
        if count <= point {
            for _ in count..point {
                text.push(b'0');
            }
            text.push_all(b".0");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(value: f64) -> String {
        String::from_utf8(double(value).as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn doubles_print_as_swift_prints_them() {
        let cases: [(f64, &str); 25] = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.3333333333333333"),
            (2.0, "2.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (9007199254740992.0, "9007199254740992.0"),
            (18014398509481984.0, "1.8014398509481984e+16"),
            (36028797018963968.0, "3.602879701896397e+16"),
            (1e17, "1e+17"),
            (0.001, "0.001"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e300, "1.5e+300"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (123456.789, "123456.789"),
            (-2.5, "-2.5"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (100.0, "100.0"),
            (1e-7, "1e-07"),
            (2.5e-5, "2.5e-05"),
        ];
        for (value, expected) in cases {
            assert_eq!(written(value), expected, "{value:e}");
        }
        assert_eq!(written(f64::NAN), "nan");
    }

    #[test]
    fn shortest_digits_agree_with_rusts_round_trip_formatting() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state & !(1 << 63));
            if !value.is_finite() || value == 0.0 {
                continue;
            }
            let (digits, count, k) = shortest(value);
            let ours: String = digits[..count].iter().map(|d| (b'0' + d) as char).collect();
            let rust = format!("{value:e}");
            let (mantissa, exponent) = rust.split_once('e').unwrap();
            let theirs: String = mantissa.chars().filter(char::is_ascii_digit).collect();
            assert_eq!(ours, theirs, "{rust}");
            assert_eq!(k - 1, exponent.parse::<i32>().unwrap(), "{rust}");
        }
    }

    #[test]
    fn integers_in_decimal() {
        assert_eq!(signed(-42).as_bytes(), b"-42");
        assert_eq!(signed(i64::MIN).as_bytes(), b"-9223372036854775808");
        assert_eq!(unsigned(u64::MAX).as_bytes(), b"18446744073709551615");
        assert_eq!(unsigned(0).as_bytes(), b"0");
    }
}

//! C's text-to-number conversions (D146, D174), shared by both layers' C libraries: `strtol`
//! and its family, for narrow and wide strings alike (blanks, a sign, a base prefix and
//! digits, with C's rules for what a value out of range becomes), and `strtod`'s decimal and
//! `inf`/`nan` text read into an `f64` (exact whenever the digits and the power of ten are
//! exact in an `f64`, within a few units in the last place otherwise).

const MOST_SIGNIFICANT_DIGITS: u32 = 19;
const EXACT_POWERS: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];
const LARGEST_EXPONENT: i32 = 400;

/// What a conversion read from the start of a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integer {
    /// The digits' value, if it fits in 64 bits.
    pub magnitude: Option<u64>,
    pub negative: bool,
    /// How many characters it took; 0 if there was no number, so the end pointer is the start.
    pub consumed: usize,
}

/// The integer at the start of `text` (a string without its NUL) in `base`, which is 0 (decimal,
/// octal after `0` or hexadecimal after `0x`) or 2 to 36; `None` for any other base.
pub fn parse_integer<T: Copy + Into<u32>>(text: &[T], base: u32) -> Option<Integer> {
    if base == 1 || base > 36 {
        return None;
    }
    let at = |index: usize| text.get(index).map(|&unit| unit.into());
    let mut index = 0;
    while at(index).is_some_and(is_space) {
        index += 1;
    }
    let negative = at(index) == Some(u32::from(b'-'));
    if negative || at(index) == Some(u32::from(b'+')) {
        index += 1;
    }
    let mut base = base;
    let hex_prefix = at(index) == Some(u32::from(b'0'))
        && matches!(at(index + 1), Some(x) if x == u32::from(b'x') || x == u32::from(b'X'))
        && at(index + 2).and_then(|unit| digit(unit, 16)).is_some();
    if (base == 0 || base == 16) && hex_prefix {
        base = 16;
        index += 2;
    } else if base == 0 {
        base = if at(index) == Some(u32::from(b'0')) {
            8
        } else {
            10
        };
    }
    let first_digit = index;
    let mut magnitude = Some(0u64);
    while let Some(value) = at(index).and_then(|unit| digit(unit, base)) {
        magnitude = magnitude
            .and_then(|sum| sum.checked_mul(u64::from(base)))
            .and_then(|sum| sum.checked_add(u64::from(value)));
        index += 1;
    }
    let consumed = if index == first_digit { 0 } else { index };
    Some(Integer {
        magnitude: if consumed == 0 { Some(0) } else { magnitude },
        negative: negative && consumed != 0,
        consumed,
    })
}

impl Integer {
    /// The value as a signed integer of `bits` bits (`strtol`, `strtoll`), and whether it was
    /// out of range, in which case it is the type's nearest limit.
    pub fn signed(&self, bits: u32) -> (i64, bool) {
        let limit = 1u64 << (bits - 1);
        match (self.magnitude, self.negative) {
            (Some(magnitude), false) if magnitude < limit => (magnitude as i64, false),
            (Some(magnitude), true) if magnitude <= limit => {
                ((magnitude as i64).wrapping_neg(), false)
            }
            (_, false) => ((limit - 1) as i64, true),
            (_, true) => ((limit as i64).wrapping_neg(), true),
        }
    }

    /// The value as an unsigned integer of `bits` bits (`strtoul`, `strtoull`): a negative
    /// value wraps around, as C says; and whether it was out of range, in which case it is the
    /// type's largest value.
    pub fn unsigned(&self, bits: u32) -> (u64, bool) {
        let largest = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        match self.magnitude {
            Some(magnitude) if magnitude <= largest => {
                let value = if self.negative {
                    magnitude.wrapping_neg() & largest
                } else {
                    magnitude
                };
                (value, false)
            }
            _ => (largest, true),
        }
    }
}

/// Whether `unit` is a C blank: space, tab, line feed, vertical tab, form feed or carriage
/// return.
pub fn is_space(unit: u32) -> bool {
    unit == u32::from(b' ') || (0x09..=0x0d).contains(&unit)
}

fn digit(unit: u32, base: u32) -> Option<u32> {
    char::from_u32(unit)?
        .to_digit(36)
        .filter(|value| *value < base)
}

fn is_space_byte(byte: u8) -> bool {
    is_space(u32::from(byte))
}

fn starts_with_ignoring_case(text: &[u8], word: &[u8]) -> bool {
    text.len() >= word.len() && text[..word.len()].eq_ignore_ascii_case(word)
}

fn scale(value: f64, exponent: i32) -> f64 {
    let mut value = value;
    let mut left = exponent.clamp(-LARGEST_EXPONENT * 2, LARGEST_EXPONENT * 2);
    while left > 22 {
        value *= 1e22;
        left -= 22;
    }
    while left < -22 {
        value /= 1e22;
        left += 22;
    }
    match left >= 0 {
        true => value * EXACT_POWERS[left as usize],
        false => value / EXACT_POWERS[(-left) as usize],
    }
}

/// The number at the start of `text` (blanks first, then a sign, then digits with an optional
/// point and exponent, or `inf`, `infinity` or `nan`) and how many bytes it took; 0 bytes and
/// 0.0 when there is none, as `strtod` reports it.
pub fn parse_float(text: &[u8]) -> (f64, usize) {
    let mut index = 0;
    while text.get(index).copied().is_some_and(is_space_byte) {
        index += 1;
    }
    let negative = text.get(index) == Some(&b'-');
    if negative || text.get(index) == Some(&b'+') {
        index += 1;
    }
    let signed = |value: f64| if negative { -value } else { value };
    let rest = &text[index.min(text.len())..];
    if starts_with_ignoring_case(rest, b"infinity") {
        return (signed(f64::INFINITY), index + 8);
    }
    if starts_with_ignoring_case(rest, b"inf") {
        return (signed(f64::INFINITY), index + 3);
    }
    if starts_with_ignoring_case(rest, b"nan") {
        return (signed(f64::NAN), index + 3);
    }
    let mut mantissa = 0u64;
    let mut kept = 0u32;
    let mut exponent = 0i32;
    let mut digits = 0usize;
    let mut seen_point = false;
    while let Some(&byte) = text.get(index) {
        match byte {
            b'0'..=b'9' => {
                digits += 1;
                if mantissa == 0 && byte == b'0' {
                    if seen_point {
                        exponent -= 1;
                    }
                } else if kept < MOST_SIGNIFICANT_DIGITS {
                    mantissa = mantissa * 10 + u64::from(byte - b'0');
                    kept += 1;
                    if seen_point {
                        exponent -= 1;
                    }
                } else if !seen_point {
                    exponent += 1;
                }
            }
            b'.' if !seen_point => seen_point = true,
            _ => break,
        }
        index += 1;
    }
    if digits == 0 {
        return (0.0, 0);
    }
    if matches!(text.get(index), Some(b'e' | b'E')) {
        let mut at = index + 1;
        let negative_exponent = text.get(at) == Some(&b'-');
        if negative_exponent || text.get(at) == Some(&b'+') {
            at += 1;
        }
        let start = at;
        let mut written = 0i32;
        while let Some(&byte) = text.get(at).filter(|byte| byte.is_ascii_digit()) {
            written = (written * 10 + i32::from(byte - b'0')).min(LARGEST_EXPONENT * 10);
            at += 1;
        }
        if at > start {
            exponent += if negative_exponent { -written } else { written };
            index = at;
        }
    }
    let value = match mantissa {
        0 => 0.0,
        mantissa => scale(mantissa as f64, exponent),
    };
    (signed(value), index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_text_is_read_with_its_sign_point_and_exponent() {
        assert_eq!(parse_float(b"  100.0"), (100.0, 7));
        assert_eq!(parse_float(b"-2.5e3x"), (-2500.0, 6));
        assert_eq!(parse_float(b"0.125"), (0.125, 5));
        assert_eq!(parse_float(b".5"), (0.5, 2));
        assert_eq!(parse_float(b"1e"), (1.0, 1));
        assert_eq!(parse_float(b"1.23456"), (1.23456, 7));
        assert_eq!(parse_float(b"1e-5"), (1e-5, 4));
        assert_eq!(parse_float(b"abc"), (0.0, 0));
        assert_eq!(parse_float(b"-"), (0.0, 0));
        assert_eq!(parse_float(b"INF"), (f64::INFINITY, 3));
        assert_eq!(parse_float(b"-infinity"), (f64::NEG_INFINITY, 9));
        assert!(parse_float(b"nan").0.is_nan());
        assert_eq!(parse_float(b"0.000001"), (0.000001, 8));
        assert_eq!(parse_float(b"12345678901234567890123").1, 23);
    }

    fn parsed(text: &str, base: u32) -> Integer {
        parse_integer(text.as_bytes(), base).unwrap()
    }

    #[test]
    fn numbers_are_read_after_blanks_with_a_sign_and_a_base() {
        let decimal = parsed("  \t-42abc", 10);
        assert_eq!(
            (decimal.magnitude, decimal.negative, decimal.consumed),
            (Some(42), true, 6)
        );
        assert_eq!(decimal.signed(32), (-42, false));
        let hex = parsed("0x1F", 0);
        assert_eq!((hex.magnitude, hex.consumed), (Some(31), 4));
        assert_eq!(parsed("0X1f", 16).magnitude, Some(31));
        assert_eq!(parsed("017", 0).magnitude, Some(15));
        assert_eq!(parsed("z", 36).magnitude, Some(35));
        assert_eq!(parsed("+7", 0).signed(32), (7, false));
        let wide: Vec<u16> = "  99".encode_utf16().collect();
        assert_eq!(parse_integer(&wide, 10).unwrap().magnitude, Some(99));
    }

    #[test]
    fn no_digits_consume_nothing() {
        for text in ["", "   ", "-", "+x", "abc"] {
            let none = parsed(text, 10);
            assert_eq!((none.magnitude, none.consumed), (Some(0), 0), "{text:?}");
        }
        let zero = parsed("0xg", 0);
        assert_eq!((zero.magnitude, zero.consumed), (Some(0), 1));
        assert_eq!(parse_integer(b"1", 1), None);
        assert_eq!(parse_integer(b"1", 37), None);
    }

    #[test]
    fn values_out_of_range_become_the_limits() {
        assert_eq!(parsed("2147483647", 10).signed(32), (2_147_483_647, false));
        assert_eq!(parsed("2147483648", 10).signed(32), (2_147_483_647, true));
        assert_eq!(
            parsed("-2147483648", 10).signed(32),
            (-2_147_483_648, false)
        );
        assert_eq!(parsed("-2147483649", 10).signed(32), (-2_147_483_648, true));
        assert_eq!(
            parsed("99999999999999999999", 10).signed(64),
            (i64::MAX, true)
        );
        assert_eq!(
            parsed("-9223372036854775808", 10).signed(64),
            (i64::MIN, false)
        );
        assert_eq!(
            parsed("4294967295", 10).unsigned(32),
            (4_294_967_295, false)
        );
        assert_eq!(parsed("4294967296", 10).unsigned(32), (4_294_967_295, true));
        assert_eq!(parsed("-1", 10).unsigned(32), (4_294_967_295, false));
        assert_eq!(parsed("-1", 10).unsigned(64), (u64::MAX, false));
        assert_eq!(
            parsed("18446744073709551616", 10).unsigned(64),
            (u64::MAX, true)
        );
    }
}

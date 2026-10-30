//! The C runtime's floating-point text and square root (D174), for `atof`, `strtod`, `scanf`'s
//! `%f` family and `sqrtf`: decimal and `inf`/`nan` text read into an `f64` (exact whenever the
//! digits and the power of ten are exact in an `f64`, within a few units in the last place
//! otherwise), and a correctly rounded single-precision square root computed with integers.

const MOST_SIGNIFICANT_DIGITS: u32 = 19;
const EXACT_POWERS: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];
const LARGEST_EXPONENT: i32 = 400;

fn is_space(byte: u8) -> bool {
    byte == b' ' || (0x09..=0x0d).contains(&byte)
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
    while text.get(index).copied().is_some_and(is_space) {
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

/// `sqrtf`: the square root of `value` rounded to the nearest `f32` (ties to even), from the
/// integer square root of its significand; NaN for a negative number or NaN, `value` itself
/// for zeros and positive infinity.
pub fn square_root(value: f32) -> f32 {
    if value.is_nan() || value < 0.0 {
        return f32::NAN;
    }
    if value == 0.0 || value.is_infinite() {
        return value;
    }
    let bits = value.to_bits();
    let raw_exponent = ((bits >> 23) & 0xff) as i32;
    let fraction = u64::from(bits & 0x7f_ffff);
    let (mut significand, mut exponent) = match raw_exponent {
        0 => (fraction, -149),
        _ => (fraction | 0x80_0000, raw_exponent - 150),
    };
    while significand < 0x80_0000 {
        significand <<= 1;
        exponent -= 1;
    }
    if exponent & 1 != 0 {
        significand <<= 1;
        exponent -= 1;
    }
    let scaled = significand << 26;
    let root = integer_square_root(scaled);
    let inexact = root * root != scaled;
    let shift = (u64::BITS - root.leading_zeros()) as i32 - 24;
    let kept = root >> shift;
    let lost = root & ((1 << shift) - 1);
    let half = 1u64 << (shift - 1);
    let round_up = lost > half || (lost == half && (inexact || kept & 1 != 0));
    let mut rounded = kept + u64::from(round_up);
    let mut rounded_exponent = exponent / 2 - 13 + shift;
    if rounded >= 1 << 24 {
        rounded >>= 1;
        rounded_exponent += 1;
    }
    let biased = (rounded_exponent + 150) as u32;
    f32::from_bits((biased << 23) | (rounded as u32 & 0x7f_ffff))
}

fn integer_square_root(value: u64) -> u64 {
    let mut remainder = value;
    let mut root = 0u64;
    let mut bit = 1u64 << 62;
    while bit > remainder {
        bit >>= 2;
    }
    while bit != 0 {
        if remainder >= root + bit {
            remainder -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
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
        assert_eq!(parse_float(b"3.14159"), (3.14159, 7));
        assert_eq!(parse_float(b"1e-5"), (1e-5, 4));
        assert_eq!(parse_float(b"abc"), (0.0, 0));
        assert_eq!(parse_float(b"-"), (0.0, 0));
        assert_eq!(parse_float(b"INF"), (f64::INFINITY, 3));
        assert_eq!(parse_float(b"-infinity"), (f64::NEG_INFINITY, 9));
        assert!(parse_float(b"nan").0.is_nan());
        assert_eq!(parse_float(b"0.000001"), (0.000001, 8));
        assert_eq!(parse_float(b"12345678901234567890123").1, 23);
    }

    #[test]
    fn the_square_root_is_correctly_rounded() {
        for value in [
            4.0f32, 2.0, 0.5, 1e-40, 3.0, 10.0, 1.0e30, 0.1, 16777215.0, 7.0e-3,
        ] {
            let exact = f64::from(value).sqrt();
            let ours = square_root(value);
            let error = (f64::from(ours) - exact).abs();
            let neighbour_error = |next: f32| (f64::from(next) - exact).abs();
            let up = f32::from_bits(ours.to_bits() + 1);
            let down = f32::from_bits(ours.to_bits() - 1);
            assert!(
                error <= neighbour_error(up) && error <= neighbour_error(down),
                "{value}"
            );
        }
        assert_eq!(square_root(4.0), 2.0);
        assert_eq!(square_root(0.25), 0.5);
        assert_eq!(square_root(-0.0), -0.0);
        assert!(square_root(-1.0).is_nan());
        assert_eq!(square_root(f32::INFINITY), f32::INFINITY);
    }
}

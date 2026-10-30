//! `sqrtf` (D174): a correctly rounded single-precision square root computed with integers.

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

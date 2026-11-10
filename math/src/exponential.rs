//! Exponentials, logarithms and powers. `exp` reduces its argument by a multiple `k` of ln 2
//! split in Cody and Waite's way (the multiple of the high part is exact) and sums the Taylor
//! series of the rest, `2^k` scaling the result. Logarithms take `x = 2^k * m` with `m` near 1
//! and sum the series of `2 atanh(s)`, `s = (m - 1) / (m + 1)`, carrying `s` and the first
//! terms in two doubles; `pow` multiplies that double-double logarithm by `y` exactly and
//! hands both halves to the exponential.

use crate::double_double::{fast_two_sum, multiply, two_product, two_sum};
use crate::manipulation::{
    EXPONENT_BIAS, FRACTION_MASK, copysign, exponent_field, fabs, frexp, is_integer,
    is_odd_integer, power_of_two, rint, scalbn,
};

pub(crate) const LN2_HIGH: f64 = f64::from_bits(0x3fe6_2e42_fefa_3800);
pub(crate) const LN2_LOW: f64 = f64::from_bits(0x3d2e_f357_93c7_6730);
pub(crate) const LN2: (f64, f64) = (
    f64::from_bits(0x3fe6_2e42_fefa_39ef),
    f64::from_bits(0x3c7a_bc9e_3b39_803f),
);
pub(crate) const INVERSE_LN2: f64 = f64::from_bits(0x3ff7_1547_652b_82fe);
const INVERSE_LN2_DOUBLE: (f64, f64) = (INVERSE_LN2, f64::from_bits(0x3c77_77d0_ffda_0d24));
const INVERSE_LN10: (f64, f64) = (
    f64::from_bits(0x3fdb_cb7b_1526_e50e),
    f64::from_bits(0x3c69_5355_baaa_fad3),
);
const TWO_THIRDS: (f64, f64) = (
    f64::from_bits(0x3fe5_5555_5555_5555),
    f64::from_bits(0x3c85_5555_5555_5555),
);
const SQRT2: f64 = f64::from_bits(0x3ff6_a09e_667f_3bcd);
const ONE_BITS: u64 = 0x3ff0_0000_0000_0000;
pub(crate) const ROUNDING_SHIFTER: f64 = 6_755_399_441_055_744.0;
pub(crate) const EXP_OVERFLOW: f64 = 709.79;
pub(crate) const EXP_UNDERFLOW: f64 = -745.2;
const EXPM1_SATURATION: f64 = -38.0;
const EXPM1_SERIES_LIMIT: f64 = 0.34;
const TWO_POW_MINUS_54: f64 = power_of_two(-54);
const TWO_POW_54: f64 = power_of_two(54);
const POW_DECIDED_EXPONENT: f64 = power_of_two(900);

pub(crate) const EXP_TAIL: [f64; 12] = inverse_factorials(2);
const EXPM1_TAIL: [f64; 14] = inverse_factorials(2);
const ATANH_TAIL: [f64; 10] = doubled_odd_reciprocals(5);

/// `1/first!`, `1/(first + 1)!` and on.
pub(crate) const fn inverse_factorials<const N: usize>(first: u32) -> [f64; N] {
    let mut factorial = 1.0;
    let mut n = 2;
    while n <= first {
        factorial *= n as f64;
        n += 1;
    }
    let mut table = [0.0; N];
    let mut index = 0;
    while index < N {
        table[index] = 1.0 / factorial;
        factorial *= (first as usize + index + 1) as f64;
        index += 1;
    }
    table
}

const fn doubled_odd_reciprocals<const N: usize>(first: u32) -> [f64; N] {
    let mut table = [0.0; N];
    let mut index = 0;
    while index < N {
        table[index] = 2.0 / (first as usize + 2 * index) as f64;
        index += 1;
    }
    table
}

/// The polynomial with `coefficients` (lowest power first) at `x`, by Horner's rule.
#[inline]
pub(crate) fn horner(x: f64, coefficients: &[f64]) -> f64 {
    let mut sum = 0.0;
    for coefficient in coefficients.iter().rev() {
        sum = sum * x + coefficient;
    }
    sum
}

fn times_power_of_two(value: f64, exponent: i32) -> f64 {
    if (-1022..=1023).contains(&exponent) {
        value * power_of_two(exponent)
    } else {
        scalbn(value, exponent)
    }
}

/// `e^(high + low)` for `|low|` well below an ulp of `high`.
pub(crate) fn exp_parts(high: f64, low: f64) -> f64 {
    if high.is_nan() {
        return high;
    }
    if high > EXP_OVERFLOW {
        return f64::INFINITY;
    }
    if high < EXP_UNDERFLOW {
        return 0.0;
    }
    let multiple = (high * INVERSE_LN2 + ROUNDING_SHIFTER) - ROUNDING_SHIFTER;
    let reduced_high = high - multiple * LN2_HIGH;
    let reduced_low = multiple * LN2_LOW - low;
    let reduced = reduced_high - reduced_low;
    let minus_one = reduced_high + (reduced * reduced * horner(reduced, &EXP_TAIL) - reduced_low);
    times_power_of_two(1.0 + minus_one, multiple as i32)
}

/// `exp`: `e^x`.
pub fn exp(x: f64) -> f64 {
    if fabs(x) < TWO_POW_MINUS_54 {
        return 1.0 + x;
    }
    exp_parts(x, 0.0)
}

/// `exp2`: `2^x`, exact when `x` is whole.
pub fn exp2(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 1024.0 {
        return f64::INFINITY;
    }
    if x < -1075.0 {
        return 0.0;
    }
    let whole = rint(x);
    let fraction = x - whole;
    if fraction == 0.0 {
        return scalbn(1.0, whole as i32);
    }
    let (high, error) = two_product(fraction, LN2.0);
    scalbn(exp_parts(high, error + fraction * LN2.1), whole as i32)
}

/// `expm1`: `e^x - 1`, accurate for `x` near zero.
pub fn expm1(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > EXP_OVERFLOW {
        return f64::INFINITY;
    }
    if x < EXPM1_SATURATION {
        return -1.0;
    }
    if fabs(x) < EXPM1_SERIES_LIMIT {
        if fabs(x) < TWO_POW_MINUS_54 {
            return x;
        }
        return x + x * x * horner(x, &EXPM1_TAIL);
    }
    let multiple = (x * INVERSE_LN2 + ROUNDING_SHIFTER) - ROUNDING_SHIFTER;
    if multiple > 1023.0 {
        return exp(x);
    }
    let reduced_high = x - multiple * LN2_HIGH;
    let reduced_low = multiple * LN2_LOW;
    let reduced = reduced_high - reduced_low;
    let minus_one = reduced_high + (reduced * reduced * horner(reduced, &EXP_TAIL) - reduced_low);
    let scale = power_of_two(multiple as i32);
    (scale - 1.0) + scale * minus_one
}

/// The natural logarithm of a positive finite `x` as a double-double, to about 100 bits.
pub(crate) fn log_double_double(x: f64) -> (f64, f64) {
    let (scaled, adjustment) = match exponent_field(x) {
        0 => (x * TWO_POW_54, 54),
        _ => (x, 0),
    };
    let mut exponent = exponent_field(scaled) - EXPONENT_BIAS - adjustment;
    let mut significand = f64::from_bits((scaled.to_bits() & FRACTION_MASK) | ONE_BITS);
    if significand > SQRT2 {
        significand *= 0.5;
        exponent += 1;
    }
    let fraction = significand - 1.0;
    let denominator = 2.0 + fraction;
    let denominator_low = fraction - (denominator - 2.0);
    let s = fraction / denominator;
    let (product, product_error) = two_product(s, denominator);
    let s_low = (((fraction - product) - product_error) - s * denominator_low) / denominator;
    let square = two_product(s, s);
    let square = (square.0, square.1 + 2.0 * s * s_low);
    let cube = multiply(square, (s, s_low));
    let third_term = multiply(cube, TWO_THIRDS);
    let rest = cube.0 * square.0 * horner(square.0, &ATANH_TAIL);
    let multiple = f64::from(exponent);
    let (sum, sum_error) = two_sum(multiple * LN2_HIGH, 2.0 * s);
    let (sum, second_error) = two_sum(sum, third_term.0);
    let low = sum_error + second_error + (2.0 * s_low + third_term.1 + rest + multiple * LN2_LOW);
    fast_two_sum(sum, low)
}

fn log_special(x: f64) -> Option<f64> {
    if x.is_nan() || x == f64::INFINITY {
        Some(x)
    } else if x == 0.0 {
        Some(f64::NEG_INFINITY)
    } else if x < 0.0 {
        Some(f64::NAN)
    } else {
        None
    }
}

/// `log`: the natural logarithm.
pub fn log(x: f64) -> f64 {
    if let Some(special) = log_special(x) {
        return special;
    }
    let (high, low) = log_double_double(x);
    high + low
}

/// `log2`: the base-2 logarithm, exact for powers of two.
pub fn log2(x: f64) -> f64 {
    if let Some(special) = log_special(x) {
        return special;
    }
    let (fraction, exponent) = frexp(x);
    if fraction == 0.5 {
        return f64::from(exponent - 1);
    }
    let (high, low) = multiply(log_double_double(x), INVERSE_LN2_DOUBLE);
    high + low
}

/// `log10`: the base-10 logarithm.
pub fn log10(x: f64) -> f64 {
    if let Some(special) = log_special(x) {
        return special;
    }
    let (high, low) = multiply(log_double_double(x), INVERSE_LN10);
    high + low
}

/// `log1p`: `log(1 + x)`, accurate for `x` near zero.
pub fn log1p(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == -1.0 {
        return f64::NEG_INFINITY;
    }
    if x < -1.0 {
        return f64::NAN;
    }
    if fabs(x) < TWO_POW_MINUS_54 {
        return x;
    }
    let (sum, error) = two_sum(1.0, x);
    let (high, low) = log_double_double(sum);
    high + (low + error / sum)
}

/// `pow`: `x^y`, with C's rules for zeros, infinities, NaNs and negative bases.
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let odd = is_odd_integer(y);
    if x == 0.0 {
        return match (y < 0.0, odd) {
            (true, true) => copysign(f64::INFINITY, x),
            (true, false) => f64::INFINITY,
            (false, true) => x,
            (false, false) => 0.0,
        };
    }
    if y.is_infinite() {
        let base = fabs(x);
        if base == 1.0 {
            return 1.0;
        }
        return if (base > 1.0) == (y > 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }
    if x.is_infinite() {
        let magnitude = if y > 0.0 { f64::INFINITY } else { 0.0 };
        return if x < 0.0 && odd {
            -magnitude
        } else {
            magnitude
        };
    }
    if x < 0.0 && !is_integer(y) {
        return f64::NAN;
    }
    let negate = x < 0.0 && odd;
    let (high, low) = log_double_double(fabs(x));
    let magnitude = if high == 0.0 {
        1.0
    } else if fabs(y) > POW_DECIDED_EXPONENT {
        if (high > 0.0) == (y > 0.0) {
            f64::INFINITY
        } else {
            0.0
        }
    } else {
        let (product, error) = two_product(y, high);
        let (product, error) = fast_two_sum(product, error + y * low);
        exp_parts(product, error)
    };
    if negate { -magnitude } else { magnitude }
}

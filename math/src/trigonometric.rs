//! Circular functions and their inverses. The argument is reduced to `[-π/4, π/4]` and a
//! quadrant: below about 1.6 million by Cody and Waite's subtraction of a multiple of π/2
//! split in four parts (the first two exact against the multiple, the third an exact product),
//! above it by Payne and Hanek's method, which multiplies the significand by the 192 bits of
//! 2/π that matter for its exponent (from a table of 1280 bits) and keeps the product's
//! fraction. The reduced argument is a double-double; Taylor series take it from there. The
//! inverse functions reduce to `atan` of a double-double quotient in `[0, 1]`, which subtracts
//! the nearest eighth and its arctangent from a table, `atan(x) = atan(c) + atan((x - c) /
//! (1 + x c))`.

use crate::double_double::{add, divide, fast_two_sum, multiply, two_product, two_sum};
use crate::exponential::{ROUNDING_SHIFTER, horner};
use crate::manipulation::{
    EXPONENT_BIAS, copysign, decompose, exponent_field, fabs, power_of_two, rint, scalbn,
};
use crate::root::sqrt;

const FRAC_PI_4: f64 = f64::from_bits(0x3fe9_21fb_5444_2d18);
const THREE_QUARTERS_PI: f64 = f64::from_bits(0x4002_d97c_7f33_21d2);
const FRAC_2_PI: f64 = f64::from_bits(0x3fe4_5f30_6dc9_c883);
pub(crate) const HALF_PI: (f64, f64) = (
    f64::from_bits(0x3ff9_21fb_5444_2d18),
    f64::from_bits(0x3c91_a626_3314_5c07),
);
pub(crate) const PI: (f64, f64) = (
    f64::from_bits(0x4009_21fb_5444_2d18),
    f64::from_bits(0x3ca1_a626_3314_5c07),
);
const HALF_PI_PART_1: f64 = f64::from_bits(0x3ff9_21fb_5400_0000);
const HALF_PI_PART_2: f64 = f64::from_bits(0x3e11_0b46_1180_0000);
const HALF_PI_PART_3: f64 = f64::from_bits(0x3c23_1319_8a2e_0370);
const HALF_PI_PART_4: f64 = f64::from_bits(0x38cc_d129_024e_088a);
const CODY_WAITE_LIMIT: f64 = 1_572_864.0;
const TWO_POW_MINUS_26: f64 = power_of_two(-26);
const TWO_POW_MINUS_27: f64 = power_of_two(-27);
const TWO_POW_MINUS_128: f64 = power_of_two(-128);
const TWO_POW_52: f64 = power_of_two(52);
const TWO_POW_60: f64 = power_of_two(60);
const WINDOW_BITS: i32 = 192;

const TWO_OVER_PI: [u64; 20] = [
    0xa2f9_836e_4e44_1529,
    0xfc27_57d1_f534_ddc0,
    0xdb62_9599_3c43_9041,
    0xfe51_63ab_debb_c561,
    0xb724_6e3a_424d_d2e0,
    0x0649_2eea_09d1_921c,
    0xfe1d_eb1c_b129_a73e,
    0xe882_35f5_2ebb_4484,
    0xe99c_7026_b45f_7e41,
    0x3991_d639_8353_39f4,
    0x9c84_5f8b_bdf9_283b,
    0x1ff8_97ff_de05_980f,
    0xef2f_118b_5a0a_6d1f,
    0x6d36_7ecf_27cb_09b7,
    0x4f46_3f66_9e5f_ea2d,
    0x7527_bac7_ebe5_f17b,
    0x3d07_39f7_8a52_92ea,
    0x6bfb_5fb1_1f8d_5d08,
    0x5603_3046_fc7b_6bab,
    0xf0cf_bc20_9af4_361d,
];

const ATAN_EIGHTHS: [(f64, f64); 9] = [
    (0.0, 0.0),
    (
        f64::from_bits(0x3fbf_d5ba_9aac_2f6e),
        f64::from_bits(0xbc4c_d376_8676_0c17),
    ),
    (
        f64::from_bits(0x3fcf_5b75_f92c_80dd),
        f64::from_bits(0x3c68_ab6e_3cf7_afbd),
    ),
    (
        f64::from_bits(0x3fd6_f619_41e4_def1),
        f64::from_bits(0xbc7c_63aa_e6f6_e918),
    ),
    (
        f64::from_bits(0x3fdd_ac67_0561_bb4f),
        f64::from_bits(0x3c7a_2b7f_222f_65e2),
    ),
    (
        f64::from_bits(0x3fe1_e00b_abde_feb4),
        f64::from_bits(0xbc59_28df_287a_668f),
    ),
    (
        f64::from_bits(0x3fe4_978f_a326_9ee1),
        f64::from_bits(0x3c72_419a_87f2_a458),
    ),
    (
        f64::from_bits(0x3fe7_00a7_c578_4634),
        f64::from_bits(0xbc78_c34d_25aa_def6),
    ),
    (
        f64::from_bits(0x3fe9_21fb_5444_2d18),
        f64::from_bits(0x3c81_a626_3314_5c07),
    ),
];

const SIN_TAIL: [f64; 8] = alternating_inverse_factorials(3, true);
const COS_TAIL: [f64; 7] = alternating_inverse_factorials(4, false);
const ATAN_TAIL: [f64; 7] = alternating_odd_reciprocals(3);

const fn alternating_inverse_factorials<const N: usize>(
    first: u32,
    first_negative: bool,
) -> [f64; N] {
    let mut factorial = 1.0;
    let mut n = 2;
    while n <= first {
        factorial *= n as f64;
        n += 1;
    }
    let mut sign = if first_negative { -1.0 } else { 1.0 };
    let mut table = [0.0; N];
    let mut index = 0;
    while index < N {
        table[index] = sign / factorial;
        let next = (first as usize + 2 * index) as f64;
        factorial *= (next + 1.0) * (next + 2.0);
        sign = -sign;
        index += 1;
    }
    table
}

const fn alternating_odd_reciprocals<const N: usize>(first: u32) -> [f64; N] {
    let mut table = [0.0; N];
    let mut sign = -1.0;
    let mut index = 0;
    while index < N {
        table[index] = sign / (first as usize + 2 * index) as f64;
        sign = -sign;
        index += 1;
    }
    table
}

/// `sin(high + low)` for `|high| <= π/4`, `low` below an ulp of `high`.
pub(crate) fn sin_kernel(high: f64, low: f64) -> f64 {
    let square = high * high;
    let tail = high * square * horner(square, &SIN_TAIL);
    high + (tail + low * (1.0 - 0.5 * square))
}

/// `cos(high + low)` for `|high| <= π/4`, `low` below an ulp of `high`.
pub(crate) fn cos_kernel(high: f64, low: f64) -> f64 {
    let square = high * high;
    let half = 0.5 * square;
    let rounded = 1.0 - half;
    let correction = (1.0 - rounded) - half;
    rounded + (correction + (square * square * horner(square, &COS_TAIL) - high * low))
}

fn two_over_pi_window(first_bit: usize) -> [u64; 3] {
    let position = first_bit - 1;
    let word = position / 64;
    let offset = position % 64;
    let take = |index: usize| -> u64 {
        match offset {
            0 => TWO_OVER_PI[index],
            _ => (TWO_OVER_PI[index] << offset) | (TWO_OVER_PI[index + 1] >> (64 - offset)),
        }
    };
    [take(word + 2), take(word + 1), take(word)]
}

fn multiply_wide(significand: u64, window: [u64; 3]) -> (u128, u128) {
    let first = u128::from(significand) * u128::from(window[0]);
    let second = u128::from(significand) * u128::from(window[1]);
    let third = u128::from(significand) * u128::from(window[2]);
    let middle = (first >> 64) + u128::from(second as u64);
    let low = (middle << 64) | u128::from(first as u64);
    let high = third + (second >> 64) + (middle >> 64);
    (low, high)
}

fn shift_right_wide((low, high): (u128, u128), by: u32) -> u128 {
    match by {
        0 => low,
        1..128 => (low >> by) | (high << (128 - by)),
        _ => high >> (by - 128),
    }
}

fn payne_hanek(magnitude: f64) -> (u32, f64, f64) {
    let (significand, exponent) = decompose(magnitude);
    let first_bit = (exponent - 1).max(1);
    let shift = first_bit + WINDOW_BITS - 1 - exponent;
    let product = multiply_wide(significand, two_over_pi_window(first_bit as usize));
    let whole = shift_right_wide(product, shift as u32) as u32;
    let fraction = shift_right_wide(product, (shift - 128) as u32);
    let quadrant = whole.wrapping_add((fraction >> 127) as u32) & 3;
    let centered = fraction as i128;
    let high = centered as f64;
    let low = centered.wrapping_sub(high as i128) as f64;
    let (high, low) = (high * TWO_POW_MINUS_128, low * TWO_POW_MINUS_128);
    let (product, error) = two_product(high, HALF_PI.0);
    let (high, low) = fast_two_sum(product, error + (high * HALF_PI.1 + low * HALF_PI.0));
    (quadrant, high, low)
}

fn reduce(x: f64) -> (u32, f64, f64) {
    let magnitude = fabs(x);
    if magnitude <= FRAC_PI_4 {
        return (0, x, 0.0);
    }
    if magnitude < CODY_WAITE_LIMIT {
        let multiple = (x * FRAC_2_PI + ROUNDING_SHIFTER) - ROUNDING_SHIFTER;
        let first = x - multiple * HALF_PI_PART_1;
        let (sum, error) = two_sum(first, -(multiple * HALF_PI_PART_2));
        let (third, third_error) = two_product(multiple, HALF_PI_PART_3);
        let (high, high_error) = two_sum(sum, -third);
        let low = ((high_error + error) - third_error) - multiple * HALF_PI_PART_4;
        let (high, low) = fast_two_sum(high, low);
        return ((multiple as i64 & 3) as u32, high, low);
    }
    let (quadrant, high, low) = payne_hanek(magnitude);
    if x < 0.0 {
        (quadrant.wrapping_neg() & 3, -high, -low)
    } else {
        (quadrant, high, low)
    }
}

/// `sin`: the sine of `x` radians.
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if fabs(x) < TWO_POW_MINUS_26 {
        return x;
    }
    let (quadrant, high, low) = reduce(x);
    match quadrant {
        0 => sin_kernel(high, low),
        1 => cos_kernel(high, low),
        2 => -sin_kernel(high, low),
        _ => -cos_kernel(high, low),
    }
}

/// `cos`: the cosine of `x` radians.
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if fabs(x) < TWO_POW_MINUS_27 {
        return 1.0;
    }
    let (quadrant, high, low) = reduce(x);
    match quadrant {
        0 => cos_kernel(high, low),
        1 => -sin_kernel(high, low),
        2 => -cos_kernel(high, low),
        _ => sin_kernel(high, low),
    }
}

/// `tan`: the tangent of `x` radians.
pub fn tan(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if fabs(x) < TWO_POW_MINUS_27 {
        return x;
    }
    let (quadrant, high, low) = reduce(x);
    let (sine, cosine) = (sin_kernel(high, low), cos_kernel(high, low));
    if quadrant & 1 == 0 {
        sine / cosine
    } else {
        -cosine / sine
    }
}

fn times_pi(value: f64) -> (f64, f64) {
    let (product, error) = two_product(value, PI.0);
    fast_two_sum(product, error + value * PI.1)
}

fn less_nearest_even(x: f64) -> f64 {
    x - 2.0 * rint(0.5 * x)
}

/// `sin(π x)`, exact in its reduction, for a finite `x`.
pub(crate) fn sin_pi(x: f64) -> f64 {
    if fabs(x) >= TWO_POW_52 {
        return copysign(0.0, x);
    }
    let reduced = less_nearest_even(x);
    let mut turn = fabs(reduced);
    if turn > 0.5 {
        turn = 1.0 - turn;
    }
    let value = if turn > 0.25 {
        let (high, low) = times_pi(0.5 - turn);
        cos_kernel(high, low)
    } else {
        let (high, low) = times_pi(turn);
        sin_kernel(high, low)
    };
    if reduced < 0.0 { -value } else { value }
}

/// `sinpi`: `sin(π x)`, exact in its reduction, so whole `x` give zeros.
pub fn sinpi(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    sin_pi(x)
}

/// `cospi`: `cos(π x)`, exact in its reduction, so half-odd `x` give zeros.
pub fn cospi(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if fabs(x) >= TWO_POW_52 {
        return 1.0;
    }
    let mut turn = fabs(less_nearest_even(fabs(x)));
    let sign = if turn > 0.5 {
        turn = 1.0 - turn;
        -1.0
    } else {
        1.0
    };
    let value = if turn > 0.25 {
        let (high, low) = times_pi(0.5 - turn);
        sin_kernel(high, low)
    } else {
        let (high, low) = times_pi(turn);
        cos_kernel(high, low)
    };
    sign * value
}

fn atan_reduced(x: (f64, f64)) -> (f64, f64) {
    let index = (x.0 * 8.0 + 0.5) as usize;
    let center = index as f64 * 0.125;
    let numerator = two_sum(x.0 - center, x.1);
    let (product, product_error) = two_product(x.0, center);
    let denominator = add((1.0, 0.0), (product, product_error + x.1 * center));
    let (ratio, ratio_low) = divide(numerator, denominator);
    let square = ratio * ratio;
    let tail = ratio * square * horner(square, &ATAN_TAIL);
    add(ATAN_EIGHTHS[index], (ratio, ratio_low + tail))
}

fn atan_of_ratio(numerator: (f64, f64), denominator: (f64, f64)) -> (f64, f64) {
    if numerator.0 <= denominator.0 {
        atan_reduced(divide(numerator, denominator))
    } else {
        let angle = atan_reduced(divide(denominator, numerator));
        add(HALF_PI, (-angle.0, -angle.1))
    }
}

fn complement_root(a: f64) -> (f64, f64) {
    let below = two_sum(1.0, -a);
    let above = two_sum(1.0, a);
    let product = multiply(below, above);
    let root = sqrt(product.0);
    if root == 0.0 {
        return (0.0, 0.0);
    }
    let (square, error) = two_product(root, root);
    (
        root,
        (((product.0 - square) - error) + product.1) / (2.0 * root),
    )
}

/// `atan`: the arctangent, in [-π/2, π/2].
pub fn atan(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() || magnitude < TWO_POW_MINUS_27 {
        return x;
    }
    if magnitude > TWO_POW_60 {
        return copysign(HALF_PI.0, x);
    }
    let (high, low) = atan_of_ratio((magnitude, 0.0), (1.0, 0.0));
    copysign(high + low, x)
}

/// `atan2`: the angle of the point `(x, y)`, in [-π, π], with C's rules for zeros and
/// infinities.
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let x_negative = x.is_sign_negative();
    if y == 0.0 {
        return if x_negative { copysign(PI.0, y) } else { y };
    }
    if x == 0.0 {
        return copysign(HALF_PI.0, y);
    }
    if x.is_infinite() {
        return match (y.is_infinite(), x_negative) {
            (true, false) => copysign(FRAC_PI_4, y),
            (true, true) => copysign(THREE_QUARTERS_PI, y),
            (false, false) => copysign(0.0, y),
            (false, true) => copysign(PI.0, y),
        };
    }
    if y.is_infinite() {
        return copysign(HALF_PI.0, y);
    }
    let (a, b) = (fabs(y), fabs(x));
    let larger = if a > b { a } else { b };
    let exponent = exponent_field(larger) - EXPONENT_BIAS;
    let angle = atan_of_ratio((scalbn(a, -exponent), 0.0), (scalbn(b, -exponent), 0.0));
    let angle = if x_negative {
        add(PI, (-angle.0, -angle.1))
    } else {
        angle
    };
    copysign(angle.0 + angle.1, y)
}

/// `asin`: the arcsine, in [-π/2, π/2].
pub fn asin(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() || magnitude < TWO_POW_MINUS_26 {
        return x;
    }
    if magnitude > 1.0 {
        return f64::NAN;
    }
    let (high, low) = atan_of_ratio((magnitude, 0.0), complement_root(magnitude));
    copysign(high + low, x)
}

/// `acos`: the arccosine, in [0, π].
pub fn acos(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let magnitude = fabs(x);
    if magnitude > 1.0 {
        return f64::NAN;
    }
    let angle = atan_of_ratio(complement_root(magnitude), (magnitude, 0.0));
    let (high, low) = if x < 0.0 {
        add(PI, (-angle.0, -angle.1))
    } else {
        angle
    };
    high + low
}

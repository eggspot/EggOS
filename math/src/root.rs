//! Square and cube roots, `hypot` and `fma`. `sqrt` is correctly rounded: Newton's iteration
//! on the reciprocal square root from the integer estimate of Lomont's paper, one correction
//! from the exact residual, then Tuckerman's test against the neighbouring doubles.

use crate::double_double::{add, multiply, two_product, two_sum};
use crate::manipulation::{
    EXPONENT_BIAS, FRACTION_MASK, copysign, exponent_field, fabs, fmax, power_of_two, scalbn,
};

const RECIPROCAL_SQUARE_ROOT_ESTIMATE: u64 = 0x5fe6_eb50_c7b5_37a9;
const ONE_BITS: u64 = 0x3ff0_0000_0000_0000;
const TWO_POW_54: f64 = power_of_two(54);
const TWO_POW_600: f64 = power_of_two(600);
const HYPOT_NEGLIGIBLE_EXPONENTS: i32 = 60;

fn product_at_least(a: f64, b: f64, target: f64) -> bool {
    let (product, error) = two_product(a, b);
    (product - target) + error >= 0.0
}

/// `sqrt`: the square root, correctly rounded.
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x == f64::INFINITY {
        return x;
    }
    if x < 0.0 {
        return f64::NAN;
    }
    let (scaled, mut exponent) = match exponent_field(x) {
        0 => {
            let scaled = x * TWO_POW_54;
            (scaled, exponent_field(scaled) - EXPONENT_BIAS - 54)
        }
        field => (x, field - EXPONENT_BIAS),
    };
    let mut significand = f64::from_bits((scaled.to_bits() & FRACTION_MASK) | ONE_BITS);
    if exponent & 1 != 0 {
        significand *= 2.0;
        exponent -= 1;
    }
    let mut reciprocal =
        f64::from_bits(RECIPROCAL_SQUARE_ROOT_ESTIMATE - (significand.to_bits() >> 1));
    for _ in 0..4 {
        reciprocal *= 1.5 - 0.5 * significand * reciprocal * reciprocal;
    }
    let mut root = significand * reciprocal;
    let (square, error) = two_product(root, root);
    root += ((significand - square) - error) * (0.5 * reciprocal);
    for _ in 0..2 {
        let below = f64::from_bits(root.to_bits() - 1);
        let above = f64::from_bits(root.to_bits() + 1);
        if product_at_least(root, below, significand) {
            root = below;
        } else if !product_at_least(root, above, significand) {
            root = above;
        } else {
            break;
        }
    }
    scalbn(root, exponent / 2)
}

/// `cbrt`: the real cube root.
pub fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let magnitude = fabs(x);
    let (scaled, adjustment) = match exponent_field(magnitude) {
        0 => (magnitude * TWO_POW_54, 18),
        _ => (magnitude, 0),
    };
    let exponent = exponent_field(scaled) - EXPONENT_BIAS;
    let whole = exponent.div_euclid(3);
    let spare = exponent.rem_euclid(3);
    let significand = f64::from_bits((scaled.to_bits() & FRACTION_MASK) | ONE_BITS)
        * power_of_two(spare);
    let mut root = f64::from_bits((significand.to_bits() - ONE_BITS) / 3 + ONE_BITS);
    for _ in 0..4 {
        root = (2.0 * root + significand / (root * root)) / 3.0;
    }
    let cube = multiply(two_product(root, root), (root, 0.0));
    root += ((significand - cube.0) - cube.1) / (3.0 * root * root);
    copysign(scalbn(root, whole - adjustment), x)
}

/// `hypot`: `sqrt(x * x + y * y)` without overflow or underflow on the way.
pub fn hypot(x: f64, y: f64) -> f64 {
    let (a, b) = (fabs(x), fabs(y));
    if a.is_infinite() || b.is_infinite() {
        return f64::INFINITY;
    }
    if a.is_nan() || b.is_nan() {
        return x + y;
    }
    let (big, small) = if a >= b { (a, b) } else { (b, a) };
    if small == 0.0 {
        return big;
    }
    if exponent_field(big) == 0 {
        return hypot(big * TWO_POW_600, small * TWO_POW_600) / TWO_POW_600;
    }
    let exponent = exponent_field(big) - EXPONENT_BIAS;
    if exponent - (exponent_field(small) - EXPONENT_BIAS) > HYPOT_NEGLIGIBLE_EXPONENTS {
        return big + small;
    }
    let big = scalbn(big, -exponent);
    let small = scalbn(small, -exponent);
    let sum = add(two_product(big, big), two_product(small, small));
    let mut root = sqrt(sum.0);
    let (square, error) = two_product(root, root);
    root += (((sum.0 - square) - error) + sum.1) / (2.0 * root);
    scalbn(root, exponent)
}

/// `fma`: `x * y + z` with the product kept exact before the sum; within one unit in the
/// last place, not always correctly rounded.
pub fn fma(x: f64, y: f64, z: f64) -> f64 {
    let product = x * y;
    let magnitude = fabs(product);
    if !product.is_finite()
        || !z.is_finite()
        || product == 0.0
        || !(power_of_two(-960)..power_of_two(960)).contains(&magnitude)
        || fmax(fabs(x), fabs(y)) >= power_of_two(990)
    {
        return product + z;
    }
    let (product, error) = two_product(x, y);
    let (sum, sum_error) = two_sum(product, z);
    sum + (sum_error + error)
}

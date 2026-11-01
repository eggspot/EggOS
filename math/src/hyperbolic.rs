//! Hyperbolic functions and their inverses, from [`expm1`] and [`log1p`] where the argument is
//! near zero (so nothing cancels) and from [`exp`] and [`log`] elsewhere.

use crate::exponential::{LN2, exp, expm1, log, log1p};
use crate::manipulation::{copysign, fabs, power_of_two};
use crate::root::sqrt;

const TWO_POW_MINUS_28: f64 = power_of_two(-28);
const TWO_POW_28: f64 = power_of_two(28);
const TANH_SATURATION: f64 = 22.0;
const EXP_LIMIT: f64 = 709.0;

/// `sinh`: the hyperbolic sine.
pub fn sinh(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() || magnitude < TWO_POW_MINUS_28 {
        return x;
    }
    let value = if magnitude < 1.0 {
        let grown = expm1(magnitude);
        0.5 * (grown + grown / (grown + 1.0))
    } else if magnitude < EXP_LIMIT {
        let grown = exp(magnitude);
        0.5 * (grown - 1.0 / grown)
    } else {
        let half = exp(0.5 * magnitude);
        (0.5 * half) * half
    };
    copysign(value, x)
}

/// `cosh`: the hyperbolic cosine.
pub fn cosh(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() {
        return x;
    }
    if magnitude < 1.0 {
        let grown = expm1(magnitude);
        return 1.0 + grown * grown / (2.0 * (1.0 + grown));
    }
    if magnitude < EXP_LIMIT {
        let grown = exp(magnitude);
        return 0.5 * (grown + 1.0 / grown);
    }
    let half = exp(0.5 * magnitude);
    (0.5 * half) * half
}

/// `tanh`: the hyperbolic tangent.
pub fn tanh(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() || magnitude < TWO_POW_MINUS_28 {
        return x;
    }
    if magnitude > TANH_SATURATION {
        return copysign(1.0, x);
    }
    let grown = expm1(2.0 * magnitude);
    copysign(grown / (grown + 2.0), x)
}

/// `asinh`: the inverse hyperbolic sine.
pub fn asinh(x: f64) -> f64 {
    let magnitude = fabs(x);
    if !x.is_finite() || magnitude < TWO_POW_MINUS_28 {
        return x;
    }
    let value = if magnitude > TWO_POW_28 {
        log(magnitude) + LN2.0
    } else if magnitude > 2.0 {
        log(2.0 * magnitude + 1.0 / (sqrt(magnitude * magnitude + 1.0) + magnitude))
    } else {
        let square = magnitude * magnitude;
        log1p(magnitude + square / (1.0 + sqrt(1.0 + square)))
    };
    copysign(value, x)
}

/// `acosh`: the inverse hyperbolic cosine, for `x >= 1`.
pub fn acosh(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 1.0 {
        return f64::NAN;
    }
    if x > TWO_POW_28 {
        return log(x) + LN2.0;
    }
    if x > 2.0 {
        return log(2.0 * x - 1.0 / (x + sqrt(x * x - 1.0)));
    }
    let above_one = x - 1.0;
    log1p(above_one + sqrt(2.0 * above_one + above_one * above_one))
}

/// `atanh`: the inverse hyperbolic tangent, for `|x| <= 1`.
pub fn atanh(x: f64) -> f64 {
    let magnitude = fabs(x);
    if x.is_nan() || magnitude < TWO_POW_MINUS_28 {
        return x;
    }
    if magnitude > 1.0 {
        return f64::NAN;
    }
    if magnitude == 1.0 {
        return copysign(f64::INFINITY, x);
    }
    let value = if magnitude < 0.5 {
        let doubled = 2.0 * magnitude;
        0.5 * log1p(doubled + doubled * magnitude / (1.0 - magnitude))
    } else {
        0.5 * log1p(2.0 * magnitude / (1.0 - magnitude))
    };
    copysign(value, x)
}

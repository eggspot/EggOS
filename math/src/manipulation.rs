//! The functions that work on a double's bits: absolute value and sign, rounding to an
//! integer, splitting into fraction and exponent, scaling by a power of two and stepping to
//! the neighbouring double. All are exact.

pub(crate) const SIGN: u64 = 1 << 63;
pub(crate) const EXPONENT_MASK: u64 = 0x7ff << 52;
pub(crate) const FRACTION_MASK: u64 = (1 << 52) - 1;
pub(crate) const EXPONENT_BIAS: i32 = 1023;
const TWO_POW_52: f64 = 4_503_599_627_370_496.0;
const TWO_POW_54: f64 = 18_014_398_509_481_984.0;
const TWO_POW_63: f64 = power_of_two(63);
const TWO_POW_1023: f64 = f64::from_bits(0x7fe0_0000_0000_0000);
const TWO_POW_MINUS_969: f64 = f64::from_bits(0x0360_0000_0000_0000);

/// `2^exponent` for `exponent` in the normal range, -1022 to 1023.
#[inline]
pub(crate) const fn power_of_two(exponent: i32) -> f64 {
    f64::from_bits(((exponent + EXPONENT_BIAS) as u64) << 52)
}

/// The biased exponent field of `x`.
#[inline]
pub(crate) const fn exponent_field(x: f64) -> i32 {
    ((x.to_bits() & EXPONENT_MASK) >> 52) as i32
}

/// `x` as an integer significand and a power of two, `significand * 2^exponent`, for a
/// finite `x`; the significand has its leading bit for a normal `x`.
#[inline]
pub(crate) const fn decompose(x: f64) -> (u64, i32) {
    let bits = x.to_bits();
    let field = ((bits & EXPONENT_MASK) >> 52) as i32;
    match field {
        0 => (bits & FRACTION_MASK, -1074),
        _ => ((bits & FRACTION_MASK) | (1 << 52), field - 1075),
    }
}

/// Whether the finite or infinite `x` is a whole number.
#[inline]
pub(crate) fn is_integer(x: f64) -> bool {
    !x.is_nan() && trunc(x) == x
}

/// Whether `x` is an odd whole number.
#[inline]
pub(crate) fn is_odd_integer(x: f64) -> bool {
    fabs(x) < 2.0 * TWO_POW_52 && is_integer(x) && (x as i64) & 1 == 1
}

/// `fabs`: `x` without its sign.
pub const fn fabs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SIGN)
}

/// `copysign`: the magnitude of `magnitude` with the sign of `sign`.
pub const fn copysign(magnitude: f64, sign: f64) -> f64 {
    f64::from_bits((magnitude.to_bits() & !SIGN) | (sign.to_bits() & SIGN))
}

/// `trunc`: `x` rounded toward zero.
pub fn trunc(x: f64) -> f64 {
    let bits = x.to_bits();
    let exponent = exponent_field(x) - EXPONENT_BIAS;
    if exponent >= 52 {
        return x;
    }
    if exponent < 0 {
        return f64::from_bits(bits & SIGN);
    }
    let fraction = FRACTION_MASK >> exponent;
    f64::from_bits(bits & !fraction)
}

/// `floor`: the largest whole number not above `x`.
pub fn floor(x: f64) -> f64 {
    let whole = trunc(x);
    if whole > x { whole - 1.0 } else { whole }
}

/// `ceil`: the smallest whole number not below `x`.
pub fn ceil(x: f64) -> f64 {
    let whole = trunc(x);
    if whole < x { whole + 1.0 } else { whole }
}

/// `round`: `x` rounded to the nearest whole number, halfway cases away from zero.
pub fn round(x: f64) -> f64 {
    let whole = trunc(x);
    if fabs(x - whole) >= 0.5 {
        whole + copysign(1.0, x)
    } else {
        whole
    }
}

/// `rint`: `x` rounded to the nearest whole number, halfway cases to even (the default
/// rounding mode, the only one the layers run in).
pub fn rint(x: f64) -> f64 {
    let magnitude = fabs(x);
    if magnitude.is_nan() || magnitude >= TWO_POW_52 {
        return x;
    }
    copysign((magnitude + TWO_POW_52) - TWO_POW_52, x)
}

/// `nearbyint`: as [`rint`], which raises no flag the layers report.
pub fn nearbyint(x: f64) -> f64 {
    rint(x)
}

fn to_integer(whole: f64) -> i64 {
    if whole.is_nan() || !(-TWO_POW_63..TWO_POW_63).contains(&whole) {
        i64::MIN
    } else {
        whole as i64
    }
}

/// `llrint`: [`rint`] as a 64-bit integer; `i64::MIN` when it does not fit, as x64's
/// conversion gives.
pub fn llrint(x: f64) -> i64 {
    to_integer(rint(x))
}

/// `llround`: [`round`] as a 64-bit integer; `i64::MIN` when it does not fit.
pub fn llround(x: f64) -> i64 {
    to_integer(round(x))
}

/// `frexp`: `x` as a fraction of magnitude in [0.5, 1) and a power of two; zero, infinities
/// and NaNs come back as they are with exponent 0.
pub fn frexp(x: f64) -> (f64, i32) {
    let field = exponent_field(x);
    if field == 0x7ff || x == 0.0 {
        return (x, 0);
    }
    if field == 0 {
        let (fraction, exponent) = frexp(x * TWO_POW_54);
        return (fraction, exponent - 54);
    }
    let bits = (x.to_bits() & !EXPONENT_MASK) | ((EXPONENT_BIAS as u64 - 1) << 52);
    (f64::from_bits(bits), field - (EXPONENT_BIAS - 1))
}

/// `scalbn`: `x * 2^exponent`, rounded once.
pub fn scalbn(x: f64, exponent: i32) -> f64 {
    let mut scaled = x;
    let mut exponent = exponent;
    if exponent > 1023 {
        scaled *= TWO_POW_1023;
        exponent -= 1023;
        if exponent > 1023 {
            scaled *= TWO_POW_1023;
            exponent = (exponent - 1023).min(1023);
        }
    } else if exponent < -1022 {
        scaled *= TWO_POW_MINUS_969;
        exponent += 969;
        if exponent < -1022 {
            scaled *= TWO_POW_MINUS_969;
            exponent = (exponent + 969).max(-1022);
        }
    }
    scaled * power_of_two(exponent)
}

/// `ldexp`: as [`scalbn`].
pub fn ldexp(x: f64, exponent: i32) -> f64 {
    scalbn(x, exponent)
}

/// `ilogb`: the exponent of `x` as a whole number; [`crate::ILOGB_ZERO`] for zero,
/// [`crate::ILOGB_NAN`] for a NaN and `i32::MAX` for an infinity.
pub fn ilogb(x: f64) -> i32 {
    let field = exponent_field(x);
    if x == 0.0 {
        return crate::ILOGB_ZERO;
    }
    if field == 0x7ff {
        return if x.is_nan() { crate::ILOGB_NAN } else { i32::MAX };
    }
    if field == 0 {
        return ilogb(x * TWO_POW_54) - 54;
    }
    field - EXPONENT_BIAS
}

/// `logb`: the exponent of `x` as a double; negative infinity for zero.
pub fn logb(x: f64) -> f64 {
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if exponent_field(x) == 0x7ff {
        return x * x;
    }
    f64::from(ilogb(x))
}

/// `modf`: `x` split into its fractional part and its whole part, both with `x`'s sign.
pub fn modf(x: f64) -> (f64, f64) {
    let whole = trunc(x);
    let fraction = if x.is_infinite() { 0.0 } else { x - whole };
    (copysign(fraction, x), whole)
}

/// `nextafter`: the double after `x` in the direction of `toward`.
pub fn nextafter(x: f64, toward: f64) -> f64 {
    if x.is_nan() || toward.is_nan() {
        return x + toward;
    }
    if x == toward {
        return toward;
    }
    if x == 0.0 {
        return copysign(f64::from_bits(1), toward);
    }
    let bits = x.to_bits();
    let away_from_zero = (toward > x) == (x > 0.0);
    f64::from_bits(if away_from_zero { bits + 1 } else { bits - 1 })
}

/// `fmax`: the larger of the two; the other when one is a NaN.
pub fn fmax(x: f64, y: f64) -> f64 {
    if x.is_nan() || x < y { y } else { x }
}

/// `fmin`: the smaller of the two; the other when one is a NaN.
pub fn fmin(x: f64, y: f64) -> f64 {
    if x.is_nan() || x > y { y } else { x }
}

/// `fdim`: `x - y` when `x` is above `y`, else +0.
pub fn fdim(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        x + y
    } else if x > y {
        x - y
    } else {
        0.0
    }
}

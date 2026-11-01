//! The `float` twins of every function (`sinf` for `sin`): each widens its arguments to
//! `double`, which holds them exactly, calls the `double` function and rounds once, so a twin
//! is within one unit in the last place of `float`, and exact where the `double` function is.

macro_rules! unary {
    ($($twin:ident => $double:ident),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($twin), "`: `", stringify!($double), "` for `float`.")]
            pub fn $twin(x: f32) -> f32 {
                crate::$double(f64::from(x)) as f32
            }
        )*
    };
}

macro_rules! binary {
    ($($twin:ident => $double:ident),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($twin), "`: `", stringify!($double), "` for `float`.")]
            pub fn $twin(x: f32, y: f32) -> f32 {
                crate::$double(f64::from(x), f64::from(y)) as f32
            }
        )*
    };
}

unary![
    sinf => sin,
    cosf => cos,
    tanf => tan,
    asinf => asin,
    acosf => acos,
    atanf => atan,
    sinhf => sinh,
    coshf => cosh,
    tanhf => tanh,
    asinhf => asinh,
    acoshf => acosh,
    atanhf => atanh,
    expf => exp,
    exp2f => exp2,
    expm1f => expm1,
    logf => log,
    log2f => log2,
    log10f => log10,
    log1pf => log1p,
    sqrtf => sqrt,
    cbrtf => cbrt,
    floorf => floor,
    ceilf => ceil,
    truncf => trunc,
    roundf => round,
    rintf => rint,
    nearbyintf => nearbyint,
    fabsf => fabs,
    logbf => logb,
    erff => erf,
    erfcf => erfc,
    tgammaf => tgamma,
    lgammaf => lgamma,
];

binary![
    atan2f => atan2,
    powf => pow,
    hypotf => hypot,
    fmodf => fmod,
    remainderf => remainder,
    copysignf => copysign,
    fminf => fmin,
    fmaxf => fmax,
    fdimf => fdim,
];

/// `fmaf`: `x * y + z` for `float`; the product is exact in `double`.
pub fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    (f64::from(x) * f64::from(y) + f64::from(z)) as f32
}

/// `frexpf`: `frexp` for `float`.
pub fn frexpf(x: f32) -> (f32, i32) {
    let (fraction, exponent) = crate::frexp(f64::from(x));
    (fraction as f32, exponent)
}

/// `ldexpf`: `ldexp` for `float`, rounded once.
pub fn ldexpf(x: f32, exponent: i32) -> f32 {
    crate::scalbn(f64::from(x), exponent.clamp(-400, 400)) as f32
}

/// `scalbnf`: as [`ldexpf`].
pub fn scalbnf(x: f32, exponent: i32) -> f32 {
    ldexpf(x, exponent)
}

/// `ilogbf`: `ilogb` for `float`.
pub fn ilogbf(x: f32) -> i32 {
    crate::ilogb(f64::from(x))
}

/// `modff`: `modf` for `float`.
pub fn modff(x: f32) -> (f32, f32) {
    let (fraction, whole) = crate::modf(f64::from(x));
    (fraction as f32, whole as f32)
}

/// `remquof`: `remquo` for `float`.
pub fn remquof(x: f32, y: f32) -> (f32, i32) {
    let (rest, quotient) = crate::remquo(f64::from(x), f64::from(y));
    (rest as f32, quotient)
}

/// `llrintf`: `llrint` for `float`.
pub fn llrintf(x: f32) -> i64 {
    crate::llrint(f64::from(x))
}

/// `llroundf`: `llround` for `float`.
pub fn llroundf(x: f32) -> i64 {
    crate::llround(f64::from(x))
}

/// `nextafterf`: the `float` after `x` in the direction of `toward`.
pub fn nextafterf(x: f32, toward: f32) -> f32 {
    if x.is_nan() || toward.is_nan() {
        return x + toward;
    }
    if x == toward {
        return toward;
    }
    if x == 0.0 {
        let smallest = f32::from_bits(1);
        return if toward > 0.0 { smallest } else { -smallest };
    }
    let bits = x.to_bits();
    let away_from_zero = (toward > x) == (x > 0.0);
    f32::from_bits(if away_from_zero { bits + 1 } else { bits - 1 })
}

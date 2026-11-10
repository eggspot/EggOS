//! The C runtime's math library (`math.h`, D229): every function of the Universal C Runtime
//! and the old `msvcrt.dll`, for `double` and `float`, from `eggos-math`, our own, under
//! Microsoft's names and types (`long` is 32 bits, `_dclass` and `fpclassify` use `FP_*` as
//! `corecrt_math.h` numbers them). Translated x64 programs on ARM64 take these functions'
//! floating-point arguments from their vector registers (`crate::translated`).

use crate::library::Export;

const FP_INFINITE: i16 = 1;
const FP_NAN: i16 = 2;
const FP_NORMAL: i16 = -1;
const FP_SUBNORMAL: i16 = -2;
const FP_ZERO: i16 = 0;

macro_rules! unary {
    ($($wrapper:ident => $function:ident: $float:ty),* $(,)?) => {
        $(extern "efiapi" fn $wrapper(x: $float) -> $float {
            eggos_math::$function(x)
        })*
    };
}

macro_rules! binary {
    ($($wrapper:ident => $function:ident: $float:ty),* $(,)?) => {
        $(extern "efiapi" fn $wrapper(x: $float, y: $float) -> $float {
            eggos_math::$function(x, y)
        })*
    };
}

unary![
    acos => acos: f64, asin => asin: f64, atan => atan: f64, cos => cos: f64,
    sin => sin: f64, tan => tan: f64, cosh => cosh: f64, sinh => sinh: f64,
    tanh => tanh: f64, acosh => acosh: f64, asinh => asinh: f64, atanh => atanh: f64,
    exp => exp: f64, exp2 => exp2: f64, expm1 => expm1: f64, log => log: f64,
    log10 => log10: f64, log2 => log2: f64, log1p => log1p: f64, logb => logb: f64,
    sqrt => sqrt: f64, cbrt => cbrt: f64, fabs => fabs: f64, ceil => ceil: f64,
    floor => floor: f64, trunc => trunc: f64, round => round: f64, rint => rint: f64,
    nearbyint => nearbyint: f64, erf => erf: f64, erfc => erfc: f64,
    tgamma => tgamma: f64, lgamma => lgamma: f64,
    acosf => acosf: f32, asinf => asinf: f32, atanf => atanf: f32, cosf => cosf: f32,
    sinf => sinf: f32, tanf => tanf: f32, coshf => coshf: f32, sinhf => sinhf: f32,
    tanhf => tanhf: f32, acoshf => acoshf: f32, asinhf => asinhf: f32,
    atanhf => atanhf: f32, expf => expf: f32, exp2f => exp2f: f32,
    expm1f => expm1f: f32, logf => logf: f32, log10f => log10f: f32,
    log2f => log2f: f32, log1pf => log1pf: f32, logbf => logbf: f32,
    cbrtf => cbrtf: f32, fabsf => fabsf: f32, ceilf => ceilf: f32,
    floorf => floorf: f32, truncf => truncf: f32, roundf => roundf: f32,
    rintf => rintf: f32, nearbyintf => nearbyintf: f32, erff => erff: f32,
    erfcf => erfcf: f32, tgammaf => tgammaf: f32, lgammaf => lgammaf: f32,
];

binary![
    atan2 => atan2: f64, pow => pow: f64, hypot => hypot: f64, fmod => fmod: f64,
    remainder => remainder: f64, copysign => copysign: f64, fmin => fmin: f64,
    fmax => fmax: f64, fdim => fdim: f64, nextafter => nextafter: f64,
    atan2f => atan2f: f32, powf => powf: f32, hypotf => hypotf: f32, fmodf => fmodf: f32,
    remainderf => remainderf: f32, copysignf => copysignf: f32, fminf => fminf: f32,
    fmaxf => fmaxf: f32, fdimf => fdimf: f32, nextafterf => nextafterf: f32,
];

extern "efiapi" fn fma(x: f64, y: f64, z: f64) -> f64 {
    eggos_math::fma(x, y, z)
}

extern "efiapi" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    eggos_math::fmaf(x, y, z)
}

unsafe extern "efiapi" fn frexp(x: f64, exponent: *mut i32) -> f64 {
    let (fraction, power) = eggos_math::frexp(x);
    if !exponent.is_null() {
        // SAFETY: `frexp`'s contract: a writable `int`.
        unsafe { exponent.write(power) };
    }
    fraction
}

unsafe extern "efiapi" fn frexpf(x: f32, exponent: *mut i32) -> f32 {
    let (fraction, power) = eggos_math::frexpf(x);
    if !exponent.is_null() {
        // SAFETY: `frexpf`'s contract: a writable `int`.
        unsafe { exponent.write(power) };
    }
    fraction
}

extern "efiapi" fn ldexp(x: f64, exponent: i32) -> f64 {
    eggos_math::ldexp(x, exponent)
}

extern "efiapi" fn ldexpf(x: f32, exponent: i32) -> f32 {
    eggos_math::ldexpf(x, exponent)
}

unsafe extern "efiapi" fn modf(x: f64, whole: *mut f64) -> f64 {
    let (fraction, integral) = eggos_math::modf(x);
    if !whole.is_null() {
        // SAFETY: `modf`'s contract: a writable `double`.
        unsafe { whole.write(integral) };
    }
    fraction
}

unsafe extern "efiapi" fn modff(x: f32, whole: *mut f32) -> f32 {
    let (fraction, integral) = eggos_math::modff(x);
    if !whole.is_null() {
        // SAFETY: `modff`'s contract: a writable `float`.
        unsafe { whole.write(integral) };
    }
    fraction
}

unsafe extern "efiapi" fn remquo(x: f64, y: f64, quotient: *mut i32) -> f64 {
    let (rest, bits) = eggos_math::remquo(x, y);
    if !quotient.is_null() {
        // SAFETY: `remquo`'s contract: a writable `int`.
        unsafe { quotient.write(bits) };
    }
    rest
}

unsafe extern "efiapi" fn remquof(x: f32, y: f32, quotient: *mut i32) -> f32 {
    let (rest, bits) = eggos_math::remquof(x, y);
    if !quotient.is_null() {
        // SAFETY: `remquof`'s contract: a writable `int`.
        unsafe { quotient.write(bits) };
    }
    rest
}

extern "efiapi" fn ilogb(x: f64) -> i32 {
    eggos_math::ilogb(x)
}

extern "efiapi" fn ilogbf(x: f32) -> i32 {
    eggos_math::ilogbf(x)
}

fn to_long(whole: i64) -> i32 {
    i32::try_from(whole).unwrap_or(i32::MIN)
}

extern "efiapi" fn lround(x: f64) -> i32 {
    to_long(eggos_math::llround(x))
}

extern "efiapi" fn lroundf(x: f32) -> i32 {
    to_long(eggos_math::llroundf(x))
}

extern "efiapi" fn lrint(x: f64) -> i32 {
    to_long(eggos_math::llrint(x))
}

extern "efiapi" fn lrintf(x: f32) -> i32 {
    to_long(eggos_math::llrintf(x))
}

extern "efiapi" fn llround(x: f64) -> i64 {
    eggos_math::llround(x)
}

extern "efiapi" fn llroundf(x: f32) -> i64 {
    eggos_math::llroundf(x)
}

extern "efiapi" fn llrint(x: f64) -> i64 {
    eggos_math::llrint(x)
}

extern "efiapi" fn llrintf(x: f32) -> i64 {
    eggos_math::llrintf(x)
}

fn class(x: f64) -> i16 {
    if x.is_nan() {
        FP_NAN
    } else if x.is_infinite() {
        FP_INFINITE
    } else if x == 0.0 {
        FP_ZERO
    } else if x.is_subnormal() {
        FP_SUBNORMAL
    } else {
        FP_NORMAL
    }
}

extern "efiapi" fn dclass(x: f64) -> i16 {
    class(x)
}

extern "efiapi" fn fdclass(x: f32) -> i16 {
    class(f64::from(x))
}

unsafe extern "efiapi" fn dtest(x: *const f64) -> i16 {
    // SAFETY: `_dtest`'s contract: a readable `double`.
    class(unsafe { x.read_unaligned() })
}

unsafe extern "efiapi" fn fdtest(x: *const f32) -> i16 {
    // SAFETY: `_fdtest`'s contract: a readable `float`.
    class(f64::from(unsafe { x.read_unaligned() }))
}

extern "efiapi" fn dsign(x: f64) -> i32 {
    i32::from(x.is_sign_negative())
}

extern "efiapi" fn fdsign(x: f32) -> i32 {
    i32::from(x.is_sign_negative())
}

extern "efiapi" fn is_nan(x: f64) -> i32 {
    i32::from(x.is_nan())
}

extern "efiapi" fn is_nan_float(x: f32) -> i32 {
    i32::from(x.is_nan())
}

extern "efiapi" fn finite(x: f64) -> i32 {
    i32::from(x.is_finite())
}

extern "efiapi" fn finite_float(x: f32) -> i32 {
    i32::from(x.is_finite())
}

extern "efiapi" fn change_sign(x: f64) -> f64 {
    -x
}

extern "efiapi" fn change_sign_float(x: f32) -> f32 {
    -x
}

pub(super) const EXPORTS: &[Export] = exports![
    b"acos" => acos, b"asin" => asin, b"atan" => atan, b"atan2" => atan2,
    b"cos" => cos, b"sin" => sin, b"tan" => tan, b"cosh" => cosh, b"sinh" => sinh,
    b"tanh" => tanh, b"acosh" => acosh, b"asinh" => asinh, b"atanh" => atanh,
    b"exp" => exp, b"exp2" => exp2, b"expm1" => expm1, b"log" => log, b"log10" => log10,
    b"log2" => log2, b"log1p" => log1p, b"logb" => logb, b"_logb" => logb,
    b"pow" => pow, b"sqrt" => sqrt, b"cbrt" => cbrt, b"hypot" => hypot,
    b"_hypot" => hypot, b"fabs" => fabs, b"ceil" => ceil, b"floor" => floor,
    b"trunc" => trunc, b"round" => round, b"rint" => rint, b"nearbyint" => nearbyint,
    b"fmod" => fmod, b"remainder" => remainder, b"remquo" => remquo, b"frexp" => frexp,
    b"ldexp" => ldexp, b"scalbn" => ldexp, b"scalbln" => ldexp, b"_scalb" => ldexp,
    b"modf" => modf, b"nextafter" => nextafter, b"_nextafter" => nextafter,
    b"nexttoward" => nextafter, b"copysign" => copysign, b"_copysign" => copysign,
    b"fmin" => fmin, b"fmax" => fmax, b"fdim" => fdim, b"fma" => fma, b"erf" => erf,
    b"erfc" => erfc, b"tgamma" => tgamma, b"lgamma" => lgamma, b"ilogb" => ilogb,
    b"lround" => lround, b"lrint" => lrint, b"llround" => llround, b"llrint" => llrint,
    b"acosf" => acosf, b"asinf" => asinf, b"atanf" => atanf, b"atan2f" => atan2f,
    b"cosf" => cosf, b"sinf" => sinf, b"tanf" => tanf, b"coshf" => coshf,
    b"sinhf" => sinhf, b"tanhf" => tanhf, b"acoshf" => acoshf, b"asinhf" => asinhf,
    b"atanhf" => atanhf, b"expf" => expf, b"exp2f" => exp2f, b"expm1f" => expm1f,
    b"logf" => logf, b"log10f" => log10f, b"log2f" => log2f, b"log1pf" => log1pf,
    b"logbf" => logbf, b"_logbf" => logbf, b"powf" => powf,
    b"cbrtf" => cbrtf, b"hypotf" => hypotf, b"_hypotf" => hypotf, b"fabsf" => fabsf,
    b"ceilf" => ceilf, b"floorf" => floorf, b"truncf" => truncf, b"roundf" => roundf,
    b"rintf" => rintf, b"nearbyintf" => nearbyintf, b"fmodf" => fmodf,
    b"remainderf" => remainderf, b"remquof" => remquof, b"frexpf" => frexpf,
    b"ldexpf" => ldexpf, b"scalbnf" => ldexpf, b"scalblnf" => ldexpf,
    b"_scalbf" => ldexpf, b"modff" => modff, b"nextafterf" => nextafterf,
    b"_nextafterf" => nextafterf, b"nexttowardf" => nextafterf,
    b"copysignf" => copysignf, b"_copysignf" => copysignf, b"fminf" => fminf,
    b"fmaxf" => fmaxf, b"fdimf" => fdimf, b"fmaf" => fmaf, b"erff" => erff,
    b"erfcf" => erfcf, b"tgammaf" => tgammaf, b"lgammaf" => lgammaf,
    b"ilogbf" => ilogbf, b"lroundf" => lroundf, b"lrintf" => lrintf,
    b"llroundf" => llroundf, b"llrintf" => llrintf, b"_dclass" => dclass,
    b"_fdclass" => fdclass, b"_dtest" => dtest, b"_fdtest" => fdtest,
    b"_dsign" => dsign, b"_fdsign" => fdsign, b"_isnan" => is_nan,
    b"_isnanf" => is_nan_float, b"_finite" => finite, b"_finitef" => finite_float,
    b"_chgsign" => change_sign, b"_chgsignf" => change_sign_float,
];

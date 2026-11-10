//! libSystem's math library (`libm`, D229): every `math.h` function for `double` and `float`
//! from `eggos-math`, our own, the Windows layer's too, under Darwin's names (a leading
//! underscore; `long` is 64 bits), with Darwin's own `__sincos_stret`, `__exp10` and
//! `__sinpi` family.

macro_rules! unary {
    ($($wrapper:ident => $function:ident: $float:ty),* $(,)?) => {
        $(extern "C" fn $wrapper(x: $float) -> $float {
            eggos_math::$function(x)
        })*
    };
}

macro_rules! binary {
    ($($wrapper:ident => $function:ident: $float:ty),* $(,)?) => {
        $(extern "C" fn $wrapper(x: $float, y: $float) -> $float {
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
    sqrtf => sqrtf: f32, cbrtf => cbrtf: f32, fabsf => fabsf: f32, ceilf => ceilf: f32,
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

#[repr(C)]
struct SineCosine<T> {
    sine: T,
    cosine: T,
}

extern "C" fn sincos_stret(x: f64) -> SineCosine<f64> {
    SineCosine {
        sine: eggos_math::sin(x),
        cosine: eggos_math::cos(x),
    }
}

extern "C" fn sincosf_stret(x: f32) -> SineCosine<f32> {
    SineCosine {
        sine: eggos_math::sinf(x),
        cosine: eggos_math::cosf(x),
    }
}

extern "C" fn exp10(x: f64) -> f64 {
    eggos_math::pow(10.0, x)
}

extern "C" fn exp10f(x: f32) -> f32 {
    eggos_math::powf(10.0, x)
}

extern "C" fn sinpi(x: f64) -> f64 {
    eggos_math::sinpi(x)
}

extern "C" fn cospi(x: f64) -> f64 {
    eggos_math::cospi(x)
}

extern "C" fn fma(x: f64, y: f64, z: f64) -> f64 {
    eggos_math::fma(x, y, z)
}

extern "C" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    eggos_math::fmaf(x, y, z)
}

unsafe extern "C" fn frexp(x: f64, exponent: *mut i32) -> f64 {
    let (fraction, power) = eggos_math::frexp(x);
    // SAFETY: `frexp`'s contract: a writable `int`.
    unsafe { exponent.write(power) };
    fraction
}

unsafe extern "C" fn frexpf(x: f32, exponent: *mut i32) -> f32 {
    let (fraction, power) = eggos_math::frexpf(x);
    // SAFETY: `frexpf`'s contract: a writable `int`.
    unsafe { exponent.write(power) };
    fraction
}

extern "C" fn ldexp(x: f64, exponent: i32) -> f64 {
    eggos_math::ldexp(x, exponent)
}

extern "C" fn ldexpf(x: f32, exponent: i32) -> f32 {
    eggos_math::ldexpf(x, exponent)
}

extern "C" fn scalbln(x: f64, exponent: i64) -> f64 {
    eggos_math::scalbn(x, exponent.clamp(-4000, 4000) as i32)
}

unsafe extern "C" fn modf(x: f64, whole: *mut f64) -> f64 {
    let (fraction, integral) = eggos_math::modf(x);
    // SAFETY: `modf`'s contract: a writable `double`.
    unsafe { whole.write(integral) };
    fraction
}

unsafe extern "C" fn modff(x: f32, whole: *mut f32) -> f32 {
    let (fraction, integral) = eggos_math::modff(x);
    // SAFETY: `modff`'s contract: a writable `float`.
    unsafe { whole.write(integral) };
    fraction
}

unsafe extern "C" fn remquo(x: f64, y: f64, quotient: *mut i32) -> f64 {
    let (rest, bits) = eggos_math::remquo(x, y);
    // SAFETY: `remquo`'s contract: a writable `int`.
    unsafe { quotient.write(bits) };
    rest
}

unsafe extern "C" fn lgamma_r(x: f64, sign: *mut i32) -> f64 {
    let (value, gamma_sign) = eggos_math::lgamma_r(x);
    // SAFETY: `lgamma_r`'s contract: a writable `int`.
    unsafe { sign.write(gamma_sign) };
    value
}

extern "C" fn ilogb(x: f64) -> i32 {
    match eggos_math::ilogb(x) {
        eggos_math::ILOGB_ZERO | eggos_math::ILOGB_NAN => i32::MIN,
        exponent => exponent,
    }
}

extern "C" fn llround(x: f64) -> i64 {
    eggos_math::llround(x)
}

extern "C" fn llroundf(x: f32) -> i64 {
    eggos_math::llroundf(x)
}

extern "C" fn llrint(x: f64) -> i64 {
    eggos_math::llrint(x)
}

extern "C" fn llrintf(x: f32) -> i64 {
    eggos_math::llrintf(x)
}

macro_rules! table {
    ($name:expr; $($symbol:literal => $function:path),* $(,)?) => {
        match $name {
            $($symbol => Some($function as *const () as usize),)*
            _ => None,
        }
    };
}

/// The address of libm's function `name` (Darwin's name), if `eggos-math` has it.
pub fn function(name: &[u8]) -> Option<usize> {
    table![name;
        b"_acos" => acos, b"_asin" => asin, b"_atan" => atan, b"_atan2" => atan2,
        b"_cos" => cos, b"_sin" => sin, b"_tan" => tan, b"_cosh" => cosh, b"_sinh" => sinh,
        b"_tanh" => tanh, b"_acosh" => acosh, b"_asinh" => asinh, b"_atanh" => atanh,
        b"_exp" => exp, b"_exp2" => exp2, b"_expm1" => expm1, b"_log" => log,
        b"_log10" => log10, b"_log2" => log2, b"_log1p" => log1p, b"_logb" => logb,
        b"_pow" => pow, b"_sqrt" => sqrt, b"_cbrt" => cbrt, b"_hypot" => hypot,
        b"_fabs" => fabs, b"_ceil" => ceil, b"_floor" => floor, b"_trunc" => trunc,
        b"_round" => round, b"_rint" => rint, b"_nearbyint" => nearbyint, b"_fmod" => fmod,
        b"_remainder" => remainder, b"_remquo" => remquo, b"_frexp" => frexp,
        b"_ldexp" => ldexp, b"_scalbn" => ldexp, b"_scalbln" => scalbln, b"_modf" => modf,
        b"_nextafter" => nextafter, b"_nexttoward" => nextafter, b"_copysign" => copysign,
        b"_fmin" => fmin, b"_fmax" => fmax, b"_fdim" => fdim, b"_fma" => fma, b"_erf" => erf,
        b"_erfc" => erfc, b"_tgamma" => tgamma, b"_lgamma" => lgamma,
        b"_lgamma_r" => lgamma_r, b"_ilogb" => ilogb, b"_lround" => llround,
        b"_lrint" => llrint, b"_llround" => llround, b"_llrint" => llrint,
        b"_acosf" => acosf, b"_asinf" => asinf, b"_atanf" => atanf, b"_atan2f" => atan2f,
        b"_cosf" => cosf, b"_sinf" => sinf, b"_tanf" => tanf, b"_coshf" => coshf,
        b"_sinhf" => sinhf, b"_tanhf" => tanhf, b"_acoshf" => acoshf, b"_asinhf" => asinhf,
        b"_atanhf" => atanhf, b"_expf" => expf, b"_exp2f" => exp2f, b"_expm1f" => expm1f,
        b"_logf" => logf, b"_log10f" => log10f, b"_log2f" => log2f, b"_log1pf" => log1pf,
        b"_logbf" => logbf, b"_powf" => powf, b"_sqrtf" => sqrtf, b"_cbrtf" => cbrtf,
        b"_hypotf" => hypotf, b"_fabsf" => fabsf, b"_ceilf" => ceilf, b"_floorf" => floorf,
        b"_truncf" => truncf, b"_roundf" => roundf, b"_rintf" => rintf,
        b"_nearbyintf" => nearbyintf, b"_fmodf" => fmodf, b"_remainderf" => remainderf,
        b"_frexpf" => frexpf, b"_ldexpf" => ldexpf, b"_scalbnf" => ldexpf, b"_modff" => modff,
        b"_nextafterf" => nextafterf, b"_copysignf" => copysignf, b"_fminf" => fminf,
        b"_fmaxf" => fmaxf, b"_fdimf" => fdimf, b"_fmaf" => fmaf, b"_erff" => erff,
        b"_erfcf" => erfcf, b"_tgammaf" => tgammaf, b"_lgammaf" => lgammaf,
        b"_lroundf" => llroundf, b"_lrintf" => llrintf, b"_llroundf" => llroundf,
        b"_llrintf" => llrintf, b"___sincos_stret" => sincos_stret,
        b"___sincosf_stret" => sincosf_stret, b"___exp10" => exp10, b"___exp10f" => exp10f,
        b"___sinpi" => sinpi, b"___cospi" => cospi,
    ]
}

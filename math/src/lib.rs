//! The C math library, our own (D229): every `<math.h>` function of C99 for `double`, with its
//! `float` twin (`sinf` for `sin`), in pure Rust without the standard library, for the Windows
//! layer's C runtime and the macOS layer's libSystem. Each function follows C's Annex F for
//! zeros, infinities and NaNs. Arguments of the trigonometric functions are reduced with
//! Cody and Waite's split of π/2 below about 1.6 million and with Payne and Hanek's
//! multiplication by 1280 bits of 2/π above; logarithms and powers are carried in two
//! doubles. `fma` and `fmaf` are correctly rounded. The error bounds the host tests hold every
//! function to, in units in the last place against the host's own library or reference
//! values, are in [`ACCURACY`].

#![cfg_attr(not(test), no_std)]

mod double_double;
mod exponential;
mod hyperbolic;
mod manipulation;
mod remainder;
mod root;
mod single;
mod special;
mod trigonometric;

#[cfg(test)]
mod tests;

pub use exponential::{exp, exp2, expm1, log, log1p, log2, log10, pow};
pub use hyperbolic::{acosh, asinh, atanh, cosh, sinh, tanh};
pub use manipulation::{
    ceil, copysign, fabs, fdim, floor, fmax, fmin, frexp, ilogb, ldexp, llrint, llround, logb,
    modf, nearbyint, nextafter, rint, round, scalbn, trunc,
};
pub use remainder::{fmod, remainder, remquo};
pub use root::{cbrt, fma, hypot, sqrt};
pub use single::*;
pub use special::{erf, erfc, lgamma, lgamma_r, tgamma};
pub use trigonometric::{acos, asin, atan, atan2, cos, cospi, sin, sinpi, tan};

/// [`ilogb`]'s result for zero, as MSVC's `FP_ILOGB0`.
pub const ILOGB_ZERO: i32 = i32::MIN;
/// [`ilogb`]'s result for a NaN, as MSVC's `FP_ILOGBNAN`.
pub const ILOGB_NAN: i32 = i32::MAX;

/// Each function's error bound in units in the last place, exactly as the host tests check
/// it: against macOS's libm, with 2 more ulps on other hosts for their own libm's error
/// (glibc's and the UCRT's are further from the correctly rounded result), except `sinpi` and
/// `cospi`, against the host's `sin` and `cos` of the exactly reduced turn carried in two
/// doubles, the error and gamma functions, against values computed to 80 digits, and `sin`,
/// `cos` and `tan` of the hardest huge arguments, against values computed to 1200 digits. A
/// bound of 0 is the correctly rounded result. The float twins are checked against the host's
/// `double` function rounded to `float`.
pub const ACCURACY: &[(&str, u32)] = &[
    (
        "sqrt fabs floor ceil trunc round rint fmod remainder frexp ldexp modf nextafter fma fmaf",
        0,
    ),
    (
        "sin cos sinpi cospi atan exp exp2 expm1 log log2 log10 log1p cbrt hypot",
        2,
    ),
    (
        "tan asin acos atan2 pow sinh cosh tanh asinh acosh atanh",
        3,
    ),
    (
        "erf erfc tgamma lgamma (absolutely within 2^-60 next to its zeros at 1 and 2)",
        4,
    ),
    ("the other float twins", 1),
];

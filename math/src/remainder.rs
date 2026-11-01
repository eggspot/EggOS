//! `fmod`, `remainder` and `remquo`, exact: the significands are divided as integers, eleven
//! bits of the quotient at a time, keeping its low bits for `remquo`.

use crate::manipulation::{copysign, decompose, fabs, scalbn};

const QUOTIENT_BITS_PER_STEP: i32 = 11;

fn reduce(x: f64, y: f64) -> (f64, u32) {
    let (mut rest, x_exponent) = decompose(fabs(x));
    let (divisor, y_exponent) = decompose(fabs(y));
    let mut quotient = (rest / divisor) as u32;
    rest %= divisor;
    let mut gap = x_exponent - y_exponent;
    while gap > 0 {
        let step = gap.min(QUOTIENT_BITS_PER_STEP);
        rest <<= step;
        quotient = quotient
            .wrapping_shl(step as u32)
            .wrapping_add((rest / divisor) as u32);
        rest %= divisor;
        gap -= step;
    }
    (scalbn(rest as f64, y_exponent), quotient)
}

/// `fmod`: `x - n * y` for the whole `n` that makes it smaller than `y` and of `x`'s sign.
pub fn fmod(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x.is_infinite() || y == 0.0 {
        return f64::NAN;
    }
    if y.is_infinite() || fabs(x) < fabs(y) {
        return x;
    }
    copysign(reduce(x, y).0, x)
}

/// `remquo`: [`remainder`], and the low 31 bits of the rounded quotient with its sign.
pub fn remquo(x: f64, y: f64) -> (f64, i32) {
    if x.is_nan() || y.is_nan() {
        return (x + y, 0);
    }
    if x.is_infinite() || y == 0.0 {
        return (f64::NAN, 0);
    }
    if y.is_infinite() {
        return (x, 0);
    }
    let divisor = fabs(y);
    let (mut rest, mut quotient) = if fabs(x) < divisor {
        (fabs(x), 0)
    } else {
        reduce(x, y)
    };
    let to_next = divisor - rest;
    if rest > to_next || (rest == to_next && quotient & 1 == 1) {
        rest = -to_next;
        quotient = quotient.wrapping_add(1);
    }
    let low = (quotient & 0x7fff_ffff) as i32;
    let signed = if (x < 0.0) != (y < 0.0) { -low } else { low };
    (if x < 0.0 { -rest } else { rest }, signed)
}

/// `remainder`: `x - n * y` for the whole `n` nearest `x / y`, halfway cases to even.
pub fn remainder(x: f64, y: f64) -> f64 {
    remquo(x, y).0
}

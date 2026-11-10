//! Error-free transformations: a sum or a product of two doubles as an unevaluated sum of
//! two (Knuth's two-sum, Dekker's fast two-sum and Veltkamp's split with Dekker's product),
//! which carry the extra precision logarithms, powers and argument reduction need.

const VELTKAMP_SPLITTER: f64 = 134_217_729.0;

/// `a + b` exactly, as the rounded sum and its error.
#[inline]
pub(crate) fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let b_part = sum - a;
    let a_part = sum - b_part;
    (sum, (a - a_part) + (b - b_part))
}

/// `a + b` exactly when `|a| >= |b|` (or `a` is zero), as the rounded sum and its error.
#[inline]
pub(crate) fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    (sum, b - (sum - a))
}

#[inline]
fn split(a: f64) -> (f64, f64) {
    let scaled = VELTKAMP_SPLITTER * a;
    let high = scaled - (scaled - a);
    (high, a - high)
}

/// `a * b` exactly, as the rounded product and its error, for products and factors well
/// inside the normal range (below about `2^995`).
#[inline]
pub(crate) fn two_product(a: f64, b: f64) -> (f64, f64) {
    let product = a * b;
    let (a_high, a_low) = split(a);
    let (b_high, b_low) = split(b);
    let error = ((a_high * b_high - product) + a_high * b_low + a_low * b_high) + a_low * b_low;
    (product, error)
}

/// The product of two double-double numbers, to about 100 bits.
#[inline]
pub(crate) fn multiply(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (product, error) = two_product(a.0, b.0);
    fast_two_sum(product, error + (a.0 * b.1 + a.1 * b.0))
}

/// The sum of two double-double numbers, to about 100 bits.
#[inline]
pub(crate) fn add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (sum, error) = two_sum(a.0, b.0);
    fast_two_sum(sum, error + a.1 + b.1)
}

/// The quotient of two double-double numbers, to about 100 bits; `b` is not zero.
#[inline]
pub(crate) fn divide(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let quotient = a.0 / b.0;
    let (product, error) = two_product(quotient, b.0);
    let remainder = ((a.0 - product) - error) + a.1 - quotient * b.1;
    fast_two_sum(quotient, remainder / b.0)
}

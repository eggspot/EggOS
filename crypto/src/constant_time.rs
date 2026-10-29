//! Comparisons whose time does not depend on the bytes compared, for tags and MACs.

use core::hint::black_box;

/// Whether `a` and `b` hold the same bytes, looking at every byte whatever the first
/// difference is. Their lengths are public: different lengths are unequal at once.
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let difference = a.iter().zip(b).fold(0u8, |sum, (x, y)| sum | (x ^ y));
    black_box(difference) == 0
}

/// All ones when `bit` is 1, zero when it is 0.
#[inline(always)]
pub fn mask_u64(bit: u64) -> u64 {
    0u64.wrapping_sub(black_box(bit & 1))
}

/// All ones when `bit` is 1, zero when it is 0.
#[inline(always)]
pub fn mask_u128(bit: u128) -> u128 {
    0u128.wrapping_sub(black_box(bit & 1))
}

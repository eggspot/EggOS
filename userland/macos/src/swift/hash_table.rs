//! The sizes of a native `Dictionary`'s hash table as Swift computes them (D180): a power of two
//! buckets (its scale is the exponent) kept at most three quarters full and never full, and a
//! bitmap of occupied buckets whose bits past the last bucket are set, so a scan for the next
//! occupied bucket stops at the end. Pure logic with host tests.

/// The scale of a table for `capacity` elements.
pub fn scale_for(capacity: usize) -> u8 {
    let capacity = capacity.max(1);
    let minimum = capacity.saturating_mul(4).div_ceil(3).max(capacity + 1);
    let largest = minimum.max(2) - 1;
    (usize::BITS - largest.leading_zeros()) as u8
}

/// How many elements a table of `buckets` buckets holds.
pub fn capacity(buckets: usize) -> usize {
    buckets * 3 / 4
}

/// The first bitmap word of an empty table of `buckets` buckets.
pub fn first_bitmap_word(buckets: usize) -> usize {
    if buckets < 64 { !0usize << buckets } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_keep_a_free_bucket_at_three_quarters_load() {
        let scales: Vec<u8> = [0, 1, 2, 3, 4, 6, 7, 12, 13].map(scale_for).to_vec();
        assert_eq!(scales, [1, 1, 2, 2, 3, 3, 4, 4, 5]);
        assert_eq!(capacity(1 << 2), 3);
        assert_eq!(capacity(1 << 1), 1);
        assert_eq!(capacity(1), 0);
    }

    #[test]
    fn bits_past_the_last_bucket_are_set() {
        assert_eq!(first_bitmap_word(1), !1);
        assert_eq!(first_bitmap_word(4), !0xf);
        assert_eq!(first_bitmap_word(64), 0);
        assert_eq!(first_bitmap_word(128), 0);
    }
}

//! Clock arithmetic shared by the architecture modules.

/// `value * multiplier / divisor` without intermediate overflow, rounded up, saturating at
/// `u64::MAX`. Used to convert between clocks of different frequencies, rounding so that a
/// deadline converted to another clock is never reached early. Returns `u64::MAX` when
/// `divisor` is 0.
pub const fn scale_up(value: u64, multiplier: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return u64::MAX;
    }
    let product = value as u128 * multiplier as u128;
    let quotient = product.div_ceil(divisor as u128);
    if quotient > u64::MAX as u128 {
        u64::MAX
    } else {
        quotient as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_exactly_and_rounds_up() {
        assert_eq!(scale_up(10, 3, 5), 6);
        assert_eq!(scale_up(11, 3, 5), 7);
        assert_eq!(scale_up(0, 3, 5), 0);
        // A 1 ms window of the 8254's clock, measured as 2 000 000 ticks of a 2 GHz clock.
        assert_eq!(scale_up(2_000_000, 1_193_182, 1193), 2_000_305_114);
    }

    #[test]
    fn no_intermediate_overflow_and_saturates() {
        assert_eq!(scale_up(u64::MAX, 1 << 32, 1 << 32), u64::MAX);
        assert_eq!(scale_up(u64::MAX, 2, 1), u64::MAX);
        assert_eq!(scale_up(1, 1, 0), u64::MAX);
    }
}

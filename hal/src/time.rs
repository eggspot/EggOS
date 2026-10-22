//! Clock arithmetic shared by the architecture modules.

/// `value * multiplier / divisor` without intermediate overflow, rounded up, saturating at
/// `u64::MAX`. Used to convert between clocks of different frequencies, rounding so that a
/// deadline converted to another clock is never reached early. Returns `u64::MAX` when
/// `divisor` is 0.
pub const fn mul_div_ceil(value: u64, multiplier: u64, divisor: u64) -> u64 {
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

/// `multiplier / divisor` as a 32.32 fixed-point factor, rounded up, for [`scale_ceil`]: a clock
/// conversion that runs often divides once, here, instead of in [`mul_div_ceil`] at every use.
/// Saturates at `u64::MAX`, and returns it when `divisor` is 0.
pub const fn ratio_ceil(multiplier: u64, divisor: u64) -> u64 {
    mul_div_ceil(multiplier, 1 << 32, divisor)
}

/// `value` scaled by `ratio` (from [`ratio_ceil`]), rounded up, saturating at `u64::MAX`: one
/// multiplication. While `multiplier / divisor` is below 2^32 (the factor did not saturate; clock
/// ratios are far below it), never below `mul_div_ceil(value, multiplier, divisor)`, so a
/// converted deadline is never reached early, and above it by less than `value / 2^32 + 1` (the
/// factor's rounding).
#[inline]
pub const fn scale_ceil(value: u64, ratio: u64) -> u64 {
    let product = value as u128 * ratio as u128;
    // At most (2^64 - 1)^2 + 2^32 - 1, so the rounding cannot overflow.
    let scaled = (product + 0xffff_ffff) >> 32;
    if scaled > u64::MAX as u128 {
        u64::MAX
    } else {
        scaled as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_exactly_and_rounds_up() {
        assert_eq!(mul_div_ceil(10, 3, 5), 6);
        assert_eq!(mul_div_ceil(11, 3, 5), 7);
        assert_eq!(mul_div_ceil(0, 3, 5), 0);
        // A 1 ms window of the 8254's clock, measured as 2 000 000 ticks of a 2 GHz clock.
        assert_eq!(mul_div_ceil(2_000_000, 1_193_182, 1193), 2_000_305_114);
    }

    #[test]
    fn no_intermediate_overflow_and_saturates() {
        assert_eq!(mul_div_ceil(u64::MAX, 1 << 32, 1 << 32), u64::MAX);
        assert_eq!(mul_div_ceil(u64::MAX, 2, 1), u64::MAX);
        assert_eq!(mul_div_ceil(1, 1, 0), u64::MAX);
    }

    #[test]
    fn a_fixed_point_ratio_scales_exactly_when_it_can() {
        let half = ratio_ceil(1, 2);
        assert_eq!(half, 1 << 31);
        assert_eq!(scale_ceil(10, half), 5);
        assert_eq!(scale_ceil(11, half), 6);
        assert_eq!(scale_ceil(0, half), 0);
        assert_eq!(scale_ceil(7, ratio_ceil(1, 1)), 7);
        assert_eq!(scale_ceil(u64::MAX, ratio_ceil(2, 1)), u64::MAX);
        assert_eq!(ratio_ceil(1, 0), u64::MAX);
    }

    #[test]
    fn a_fixed_point_ratio_never_rounds_below_the_exact_conversion() {
        // TSC and local APIC timer pairs as measured (QEMU, then real machines), and a few
        // awkward ones; values from one tick to beyond the APIC timer's 32-bit count.
        let clocks = [
            (999_152_405, 1_000_000_000),
            (2_400_000_000, 25_000_000),
            (3_696_000_000, 100_000_000),
            (3, 7),
            (1_000_000_007, 999_999_937),
        ];
        let values = [
            1,
            2,
            999,
            10_000_000,
            123_456_789,
            u64::from(u32::MAX),
            1 << 40,
            u64::MAX / 3,
        ];
        for (tsc, apic) in clocks {
            let ratio = ratio_ceil(apic, tsc);
            for value in values {
                let exact = mul_div_ceil(value, apic, tsc);
                let fast = scale_ceil(value, ratio);
                assert!(fast >= exact, "{value} at {apic}/{tsc}: {fast} < {exact}");
                assert!(
                    fast - exact <= (value >> 32) + 1,
                    "{value} at {apic}/{tsc}: {fast} vs {exact}"
                );
            }
        }
    }
}

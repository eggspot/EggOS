//! Ice Lake's MG PHY behind its Type-C ports C to F (D788). Each port's PHY answers in its own
//! 4 KiB of BAR 0 from `0x168000`, two lanes 1 KiB apart, with no bank index. Its PLL runs the
//! oscillator at 8.1 GHz for every DisplayPort rate, divided down by the high- and low-speed
//! dividers, the feedback divider's integer and 22-bit fraction from the reference clock, and
//! its loop filter, lock and bias settings; each lane's transmitter takes the de-emphasis
//! override Intel's table gives for the level the sink asks for. Values and places are from
//! Intel's MIT-licensed i915 driver (`INTEL-NOTICE.md`).

use crate::io::RegisterIo;

const WINDOWS: usize = 0x16_8000;
const WINDOW_BYTES: usize = 0x1000;
const LANE_STRIDE: usize = 0x400;
const DP_MODE: usize = 0x03a0;
const TX1_DRIVE: usize = 0x00c4;
const TX2_DRIVE: usize = 0x01c4;
const REFERENCE_INPUT: usize = 0x092c;
const CORE_CLOCK: usize = 0x08d8;
const HIGH_SPEED_CLOCK: usize = 0x08d4;
const PLL_DIVIDER_0: usize = 0x0a00;
const PLL_DIVIDER_1: usize = 0x0a04;
const PLL_LOOP_FILTER: usize = 0x0a08;
const PLL_FRACTION_LOCK: usize = 0x0a0c;
const PLL_SPREAD: usize = 0x0a10;
const PLL_BIAS: usize = 0x0a14;
const PLL_COLD_START: usize = 0x0a18;
const DP_X1_MODE: u32 = 1 << 6;
const DP_X2_MODE: u32 = 1 << 7;
const DEEMPHASIS_OVERRIDE: u32 = 1 << 22;
const DCO_KHZ: u32 = 8_100_000;
const POST_DIVIDERS: [u32; 4] = [7, 5, 3, 2];
const FEEDBACK_PREDIVIDER: u32 = 2;
const FRACTION_BITS: u32 = 22;
const FRACTION_ENABLE: u32 = 1 << 30;
const AFC_COUNT_512: u32 = 1 << 20;
const TRUE_LOCK_32: u32 = 1 << 18;
const EARLY_LOCK_16: u32 = 1 << 16;
const LOCK_THRESHOLD: u32 = 1 << 11;
const DCO_DITHER: u32 = 1 << 10;
const FEED_FORWARD_CALIBRATION: u32 = 1 << 8;
const BIAS_FIXED: u32 = 3 << 30 | 0x3f << 24 | 10 << 16 | 1 << 15 | 12 << 8 | 4 << 5;
const DRIVE_TABLE: [(u32, u32, u32); 10] = [
    (0x18, 0x00, 0x00),
    (0x1d, 0x00, 0x05),
    (0x24, 0x00, 0x0c),
    (0x2b, 0x00, 0x14),
    (0x21, 0x00, 0x00),
    (0x2b, 0x00, 0x08),
    (0x30, 0x00, 0x0f),
    (0x31, 0x00, 0x03),
    (0x34, 0x00, 0x0b),
    (0x3f, 0x00, 0x00),
];

/// An MG PLL setting: each register's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MgPll {
    pub reference_input: u32,
    pub core_clock: u32,
    pub high_speed_clock: u32,
    pub divider_0: u32,
    pub divider_1: u32,
    pub loop_filter: u32,
    pub fraction_lock: u32,
    pub spread: u32,
    pub bias: u32,
    pub cold_start: u32,
}

/// The MG PLL setting for a DisplayPort link at `link_khz` (the port clock) from a
/// `reference_khz` reference (19200, 24000 or 38400).
pub fn pll_setting(link_khz: u32, reference_khz: u32) -> Option<MgPll> {
    let (post, divider) = POST_DIVIDERS.iter().find_map(|&post| {
        (1..=10u32)
            .rev()
            .find(|&divider| post * divider * link_khz * 5 == DCO_KHZ)
            .map(|divider| (post, divider))
    })?;
    let (core_ratio, line_driver) = if divider >= 2 { (10, 1) } else { (5, 0) };
    let high_speed_ratio = match post {
        2 => 0,
        3 => 1,
        5 => 2,
        _ => 3,
    };
    let reference_divider = match reference_khz {
        19_200 | 24_000 => 1,
        38_400 => 2,
        _ => return None,
    };
    let reference = reference_khz / reference_divider;
    let step = reference * FEEDBACK_PREDIVIDER;
    let integer = DCO_KHZ / step;
    if integer > 255 {
        return None;
    }
    let remainder = DCO_KHZ % step;
    let fraction = ((u64::from(remainder) << FRACTION_BITS) / u64::from(step)) as u32;
    let target_count = (2 * 1000 * 100_000 * 10 / (132 * reference) + 5) / 10;
    let feed_forward = match remainder {
        0 => 0,
        _ => FEEDBACK_PREDIVIDER * 1_000_000 * 100 / (DCO_KHZ * 3 / 10),
    };
    let (proportional, integral, gain) = (4, 8, 1);
    Some(MgPll {
        reference_input: 1 << 8,
        core_clock: core_ratio << 8,
        high_speed_clock: line_driver << 14 | high_speed_ratio << 12 | divider << 8,
        divider_0: u32::from(fraction != 0) * FRACTION_ENABLE | fraction << 8 | integer,
        divider_1: reference_divider << 16 | 1 << 4 | FEEDBACK_PREDIVIDER,
        loop_filter: target_count << 24 | AFC_COUNT_512 | gain << 16 | integral << 8 | proportional,
        fraction_lock: TRUE_LOCK_32
            | EARLY_LOCK_16
            | LOCK_THRESHOLD
            | u32::from(fraction != 0) * DCO_DITHER
            | u32::from(feed_forward != 0) * FEED_FORWARD_CALIBRATION
            | feed_forward,
        spread: 0,
        bias: BIAS_FIXED,
        cold_start: 0,
    })
}

/// Type-C port `index`'s MG PHY (0 for port C).
#[derive(Debug)]
pub struct MgPhy<'a, R: RegisterIo + ?Sized> {
    regs: &'a R,
    base: usize,
}

impl<'a, R: RegisterIo + ?Sized> MgPhy<'a, R> {
    pub fn new(regs: &'a R, index: usize) -> Self {
        Self {
            regs,
            base: WINDOWS + index * WINDOW_BYTES,
        }
    }

    /// Writes `setting` into the PLL's registers; the caller enables the PLL afterwards.
    pub fn program_pll(&self, setting: &MgPll) {
        for (at, value) in [
            (REFERENCE_INPUT, setting.reference_input),
            (CORE_CLOCK, setting.core_clock),
            (HIGH_SPEED_CLOCK, setting.high_speed_clock),
            (PLL_DIVIDER_0, setting.divider_0),
            (PLL_DIVIDER_1, setting.divider_1),
            (PLL_LOOP_FILTER, setting.loop_filter),
            (PLL_FRACTION_LOCK, setting.fraction_lock),
            (PLL_SPREAD, setting.spread),
            (PLL_BIAS, setting.bias),
            (PLL_COLD_START, setting.cold_start),
        ] {
            self.regs.write32(self.base + at, value);
        }
    }

    /// Sets both lanes for DisplayPort on `lanes` lanes (1, 2 or 4).
    pub fn set_lane_mode(&self, lanes: usize) {
        for lane in 0..2 {
            let at = self.base + lane * LANE_STRIDE + DP_MODE;
            let mode = match lanes {
                1 => DP_X1_MODE,
                _ => DP_X2_MODE,
            };
            let kept = self.regs.read32(at) & !(DP_X1_MODE | DP_X2_MODE);
            self.regs.write32(at, kept | mode);
        }
    }

    /// Sets every lane's transmitters to drive level `level` (from
    /// [`super::intel_dekel::drive_level`]).
    pub fn set_drive(&self, level: usize) {
        let (high, middle, low) = DRIVE_TABLE[level.min(DRIVE_TABLE.len() - 1)];
        let value = high | middle << 8 | low << 16 | DEEMPHASIS_OVERRIDE;
        for lane in 0..2 {
            for transmitter in [TX1_DRIVE, TX2_DRIVE] {
                let at = self.base + lane * LANE_STRIDE + transmitter;
                let kept = self.regs.read32(at) & !(0x3f | 0x3f << 8 | 0x3f << 16);
                self.regs.write32(at, kept | value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_display_port_rate_divides_the_8_1_ghz_oscillator() {
        for link in [162_000, 270_000, 540_000, 810_000] {
            let pll = pll_setting(link, 24_000).unwrap();
            assert_eq!(pll.divider_0 & 0xff, 8_100_000 / 48_000);
        }
        let halved = pll_setting(270_000, 38_400).unwrap();
        assert_eq!(halved.divider_1 >> 16 & 0x7, 2);
        assert_eq!(pll_setting(300_000, 24_000), None);
    }
}

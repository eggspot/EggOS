//! The Dekel PHY behind Tiger, Alder and Raptor Lake-P's Type-C ports (D526). Each port's PHY
//! answers in a 4 KiB window of BAR 0 (`0x168000` on, one a port); a bank index per port in a
//! shared HIP index register picks which 4 KiB of the PHY's larger space the window shows. Its
//! PLL runs its oscillator at 8.1 GHz for every DisplayPort rate, divided down by the high- and
//! low-speed dividers; the feedback divider's integer and 22-bit fraction come from the
//! reference clock. The lanes are set up for the pin assignment and width, and each lane pair's
//! transmitters take a swing, pre-shoot and de-emphasis from Intel's table for the level the
//! sink asks for. Values and places are from Intel's MIT-licensed i915 driver
//! (`INTEL-NOTICE.md`).

use crate::io::RegisterIo;

const WINDOWS: usize = 0x16_8000;
const WINDOW_BYTES: usize = 0x1000;
const BANK_SHIFT: u32 = 12;
const HIP_INDEX: [usize; 2] = [0x10_10a0, 0x10_10a4];
const LANE_STRIDE: u32 = 0x1000;
const DP_MODE: u32 = 0x00a0;
const TX_CONTROL_0: u32 = 0x02c0;
const TX_CONTROL_1: u32 = 0x02c4;
const TX_CONTROL_2: u32 = 0x02c8;
const TX_LANE_SUSPEND: u32 = 0x0d00;
const REFERENCE_INPUT: u32 = 0x212c;
const HIGH_SPEED_CLOCK: u32 = 0x20d4;
const CORE_CLOCK: u32 = 0x20d8;
const PLL_DIVIDER_0: u32 = 0x2200;
const PLL_DIVIDER_1: u32 = 0x2204;
const PLL_SPREAD: u32 = 0x2210;
const PLL_BIAS: u32 = 0x2214;
const PLL_COLD_START: u32 = 0x2218;
const DP_X1_MODE: u32 = 1 << 6;
const DP_X2_MODE: u32 = 1 << 7;
const DP_20_BIT_MODE: u32 = 1 << 2;
const LOAD_GENERATOR_SELECT: u32 = 0xf << 3;
const DRIVE_MASK: u32 = 0x1f << 13 | 0x1f << 8 | 0x7;
const DCO_KHZ: u32 = 8_100_000;
const POST_DIVIDERS: [u32; 4] = [7, 5, 3, 2];
const FEEDBACK_PREDIVIDER: u32 = 2;
const FRACTION_BITS: u32 = 22;
const SPREAD_STEPS_LOG: u32 = 4;

const DRIVE_TABLE: [(u32, u32, u32); 10] = [
    (0x7, 0x0, 0x00),
    (0x5, 0x0, 0x05),
    (0x2, 0x0, 0x0b),
    (0x0, 0x0, 0x18),
    (0x5, 0x0, 0x00),
    (0x2, 0x0, 0x08),
    (0x0, 0x0, 0x14),
    (0x2, 0x0, 0x00),
    (0x0, 0x0, 0x0b),
    (0x0, 0x0, 0x00),
];
const DRIVE_TABLE_HIGH_RATE_LEVEL_3: u32 = 0x19;
const LEVELS_BEFORE_SWING: [usize; 4] = [0, 4, 7, 9];

type Field = (u32, u32);

/// A Dekel PLL setting: each register's value and mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PllSetting {
    pub reference_input: Field,
    pub core_clock: Field,
    pub high_speed_clock: Field,
    pub divider_0: Field,
    pub divider_1: Field,
    pub spread: Field,
    pub bias: Field,
    pub cold_start: Field,
}

/// The PLL setting for a DisplayPort link at `link_khz` (the port clock: 162000 for RBR, ...)
/// from a `reference_khz` reference (19200, 24000 or 38400); `None` for a rate the 8.1 GHz
/// oscillator cannot divide down to, or another reference.
pub fn pll_setting(link_khz: u32, reference_khz: u32) -> Option<PllSetting> {
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
    let (reference_divider, reference_trim) = match reference_khz {
        19_200 => (1, 28),
        24_000 => (1, 25),
        38_400 => (2, 28),
        _ => return None,
    };
    let step = reference_khz * FEEDBACK_PREDIVIDER;
    let integer = DCO_KHZ / step;
    if integer > 255 {
        return None;
    }
    let remainder = DCO_KHZ % step;
    let fraction = ((u64::from(remainder) << FRACTION_BITS) / u64::from(step)) as u32;
    let target_count = (2 * 1000 * 100_000 * 10 / (132 * reference_khz) + 5) / 10;
    let feed_forward = match remainder {
        0 => 0,
        _ => FEEDBACK_PREDIVIDER * 1_000_000 * 100 / (DCO_KHZ * 3 / 10),
    };
    let (proportional, integral) = if DCO_KHZ >= 9_000_000 {
        (5, 10)
    } else {
        (4, 8)
    };
    Some(PllSetting {
        reference_input: (1 << 8, 0x7 << 8),
        core_clock: (core_ratio << 8, 0xff << 8),
        high_speed_clock: (
            line_driver << 14 | high_speed_ratio << 12 | divider << 8,
            0x3 << 14 | 1 << 16 | 0x3 << 12 | 0xf << 8,
        ),
        divider_0: (
            integral << 16 | proportional << 12 | FEEDBACK_PREDIVIDER << 8 | integer,
            0x1f << 16 | 0xf << 12 | 0xf << 8 | 0xff,
        ),
        divider_1: (reference_trim << 16 | target_count, 0x1f << 16 | 0xff),
        spread: (
            reference_divider << 29 | SPREAD_STEPS_LOG << 11,
            0x7 << 29 | 0xff << 16 | 0x7 << 11 | 1 << 9,
        ),
        bias: (
            u32::from(fraction != 0) << 30 | fraction << 8,
            1 << 30 | 0x3f_ffff << 8,
        ),
        cold_start: (feed_forward, 0xff << 8 | 0xff),
    })
}

/// The drive level index for `swing` and `pre_emphasis` (each 0 to 3, their sum at most 3).
pub fn drive_level(swing: u8, pre_emphasis: u8) -> usize {
    let swing = usize::from(swing.min(3));
    let pre_emphasis = usize::from(pre_emphasis).min(3 - swing);
    LEVELS_BEFORE_SWING[swing] + pre_emphasis
}

/// The Dekel PHY of Type-C port `index` (0 for port D) behind `regs`.
#[derive(Debug)]
pub struct Dekel<'a, R: RegisterIo + ?Sized> {
    regs: &'a R,
    index: usize,
}

impl<'a, R: RegisterIo + ?Sized> Dekel<'a, R> {
    pub fn new(regs: &'a R, index: usize) -> Self {
        Self { regs, index }
    }

    fn place(&self, offset: u32) -> usize {
        let bank = offset >> BANK_SHIFT & 0xf;
        let shift = 8 * (self.index % 4) as u32;
        self.regs.write32(HIP_INDEX[self.index / 4], bank << shift);
        WINDOWS + self.index * WINDOW_BYTES + (offset as usize & (WINDOW_BYTES - 1))
    }

    fn read(&self, offset: u32) -> u32 {
        let place = self.place(offset);
        self.regs.read32(place)
    }

    fn write(&self, offset: u32, value: u32) {
        let place = self.place(offset);
        self.regs.write32(place, value);
    }

    fn modify(&self, offset: u32, (value, mask): Field) {
        let old = self.read(offset);
        self.write(offset, old & !mask | value & mask);
    }

    /// Writes PLL setting `setting` (the PLL powered and not yet enabled).
    pub fn program_pll(&self, setting: &PllSetting) {
        self.modify(REFERENCE_INPUT, setting.reference_input);
        self.modify(CORE_CLOCK, setting.core_clock);
        self.modify(HIGH_SPEED_CLOCK, setting.high_speed_clock);
        self.modify(PLL_DIVIDER_0, setting.divider_0);
        self.modify(PLL_DIVIDER_1, setting.divider_1);
        self.modify(PLL_SPREAD, setting.spread);
        self.modify(PLL_BIAS, setting.bias);
        self.modify(PLL_COLD_START, setting.cold_start);
    }

    /// Sets both lane pairs up for `lanes` DisplayPort lanes under pin assignment C, D or E
    /// (display 12's): one lane each, or two.
    pub fn set_lane_mode(&self, lanes: usize) {
        let mode = if lanes == 1 { DP_X1_MODE } else { DP_X2_MODE };
        for pair in 0..2 {
            let offset = DP_MODE + pair * LANE_STRIDE;
            let value = self.read(offset) & !(DP_X1_MODE | DP_X2_MODE) | mode;
            self.write(offset, value);
            if self.read(offset) != value {
                self.write(offset, value);
            }
        }
    }

    /// Drives every lane at drive level `level` ([`drive_level`]), from the table for rates
    /// above HBR when `high_rate`; on Alder Lake-P (`alder`) the load generators go back to
    /// DisplayPort's.
    pub fn set_drive(&self, level: usize, high_rate: bool, alder: bool) {
        let (swing, preshoot, mut de_emphasis) = DRIVE_TABLE[level.min(DRIVE_TABLE.len() - 1)];
        if high_rate && level == 3 {
            de_emphasis = DRIVE_TABLE_HIGH_RATE_LEVEL_3;
        }
        let drive = preshoot << 13 | de_emphasis << 8 | swing;
        for pair in 0..2 {
            let lane = pair * LANE_STRIDE;
            self.write(TX_LANE_SUSPEND + lane, 0);
            self.modify(TX_CONTROL_0 + lane, (drive, DRIVE_MASK));
            self.modify(TX_CONTROL_1 + lane, (drive, DRIVE_MASK));
            self.modify(TX_CONTROL_2 + lane, (0, DP_20_BIT_MODE));
            if alder {
                self.modify(TX_CONTROL_2 + lane, (0, LOAD_GENERATOR_SELECT));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl RegisterIo for Registers {
        fn read8(&self, offset: usize) -> u8 {
            self.read32(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.write32(offset, value.into());
        }
        fn read16(&self, offset: usize) -> u16 {
            self.read32(offset) as u16
        }
        fn write16(&self, offset: usize, value: u16) {
            self.write32(offset, value.into());
        }
        fn read32(&self, offset: usize) -> u32 {
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
    }

    #[test]
    fn hbr_from_38_4_mhz_divides_8_1_ghz_by_three_and_two() {
        let setting = pll_setting(270_000, 38_400).unwrap();
        assert_eq!(setting.high_speed_clock.0, 1 << 14 | 1 << 12 | 2 << 8);
        assert_eq!(setting.core_clock.0, 10 << 8);
        assert_eq!(setting.divider_0.0 & 0xff, 105);
        assert_eq!(setting.bias.0 >> 8 & 0x3f_ffff, 1_966_080);
        assert_eq!(setting.divider_1.0 & 0xff, 39);
        assert_eq!(setting.cold_start.0, 82);
        assert_eq!(
            pll_setting(810_000, 38_400).unwrap().high_speed_clock.0 >> 8 & 0xf,
            1
        );
        assert_eq!(pll_setting(250_000, 38_400), None);
        assert_eq!(pll_setting(270_000, 25_000), None);
    }

    #[test]
    fn drive_levels_follow_the_specification_s_order() {
        assert_eq!(drive_level(0, 0), 0);
        assert_eq!(drive_level(1, 2), 6);
        assert_eq!(drive_level(2, 3), 8);
        assert_eq!(drive_level(3, 0), 9);
    }

    #[test]
    fn registers_go_through_the_bank_window() {
        let regs = Registers::default();
        let dekel = Dekel::new(&regs, 5);
        dekel.set_drive(1, false, false);
        assert_eq!(regs.read32(HIP_INDEX[1]), 1 << 8);
        let window = WINDOWS + 5 * WINDOW_BYTES;
        assert_eq!(regs.read32(window + 0x2c0), 0x05 << 8 | 0x5);
        dekel.set_lane_mode(2);
        assert_eq!(regs.read32(window + 0x0a0), DP_X2_MODE);
    }
}

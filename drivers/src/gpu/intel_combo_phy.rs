//! Intel display 11's combo PHY clocks (Icelake; D465): the PLL a combo port's DisplayPort
//! link runs from. Its digitally controlled oscillator runs at 8.1 GHz for every DisplayPort
//! rate, as a whole and a fractional multiple of the reference clock (38.4 MHz is halved
//! first), and three dividers bring it down to the port's clock, half the link rate. Pure
//! arithmetic, tested on the host; the engine writes it ([`super::intel::DisplayEngine`]).

/// The oscillator's frequency for DisplayPort, in kHz.
pub const DCO_KHZ: u64 = 8_100_000;
const FRACTION_BITS: u32 = 15;
const HALVED_REFERENCE_KHZ: u32 = 38_400;
const CENTRAL_FREQUENCY_8400: u32 = 3;
const PDIV_SHIFT: u32 = 2;
const KDIV_SHIFT: u32 = 6;
const DCO_FRACTION_SHIFT: u32 = 10;
const QDIV_RATIO_SHIFT: u32 = 10;
const QDIV_MODE: u32 = 1 << 9;

/// A combo PLL's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComboPll {
    pub dco_integer: u32,
    pub dco_fraction: u32,
    /// The P divider: 2, 3, 5 or 7.
    pub pdiv: u32,
    /// The K divider: 1, 2 or 3.
    pub kdiv: u32,
    /// The Q divider: 1, or more with a K divider of 2 (HDMI, D787).
    pub qdiv: u32,
}

impl ComboPll {
    /// `DPLL_CFGCR0`: the oscillator's whole and fractional multiples.
    pub fn configuration_0(&self) -> u32 {
        self.dco_integer & 0x3ff | (self.dco_fraction & 0x7fff) << DCO_FRACTION_SHIFT
    }

    /// `DPLL_CFGCR1`: the dividers, encoded, and the oscillator's central frequency.
    pub fn configuration_1(&self) -> u32 {
        let pdiv = match self.pdiv {
            2 => 1,
            3 => 2,
            5 => 4,
            _ => 8,
        };
        let kdiv = match self.kdiv {
            1 => 1,
            2 => 2,
            _ => 4,
        };
        let qdiv = match self.qdiv {
            0 | 1 => 0,
            ratio => (ratio & 0xff) << QDIV_RATIO_SHIFT | QDIV_MODE,
        };
        qdiv | pdiv << PDIV_SHIFT | kdiv << KDIV_SHIFT | CENTRAL_FREQUENCY_8400
    }
}

/// The reference clock `DSSM`'s field names, in kHz: 0 is 24 MHz, 1 is 19.2 MHz, 2 is 38.4 MHz.
pub fn reference_khz(field: u32) -> Option<u32> {
    match field {
        0 => Some(24_000),
        1 => Some(19_200),
        2 => Some(38_400),
        _ => None,
    }
}

/// The PLL for DisplayPort link rate code `rate` (`0x06` 1.62 Gbps to `0x1e` 8.1 Gbps) from a
/// `reference_khz` reference clock.
pub fn for_display_port(rate: u8, reference_khz: u32) -> Option<ComboPll> {
    let (pdiv, kdiv) = match rate {
        0x06 => (5, 2),
        0x0a => (3, 2),
        0x14 => (3, 1),
        0x1e => (2, 1),
        _ => return None,
    };
    let reference = match reference_khz {
        HALVED_REFERENCE_KHZ => u64::from(reference_khz / 2),
        0 => return None,
        reference => u64::from(reference),
    };
    let integer = DCO_KHZ / reference;
    let fraction = ((DCO_KHZ % reference) << FRACTION_BITS) / reference;
    Some(ComboPll {
        dco_integer: integer as u32,
        dco_fraction: fraction as u32,
        pdiv,
        kdiv,
        qdiv: 1,
    })
}

const DCO_LOWEST_KHZ: u64 = 7_998_000;
const DCO_HIGHEST_KHZ: u64 = 10_000_000;
const DCO_MIDDLE_KHZ: u64 = 8_999_000;

/// The PLL for an HDMI TMDS character rate of `tmds_khz` (D787): the oscillator five times the
/// rate times a divider the P and K dividers make, within 7998 to 10000 MHz, nearest the
/// middle of that range.
pub fn for_hdmi(tmds_khz: u32, reference_khz: u32) -> Option<ComboPll> {
    let reference = match reference_khz {
        HALVED_REFERENCE_KHZ => u64::from(reference_khz / 2),
        0 => return None,
        reference => u64::from(reference),
    };
    let (dco, pdiv, kdiv, qdiv) = [2u32, 3, 5, 7]
        .into_iter()
        .flat_map(|pdiv| [1u32, 2, 3].into_iter().map(move |kdiv| (pdiv, kdiv)))
        .flat_map(|(pdiv, kdiv)| {
            let most = if kdiv == 2 { 255 } else { 1 };
            (1..=most).map(move |qdiv| (pdiv, kdiv, qdiv))
        })
        .map(|(pdiv, kdiv, qdiv)| {
            let divider = u64::from(pdiv * kdiv * qdiv);
            (5 * u64::from(tmds_khz) * divider, pdiv, kdiv, qdiv)
        })
        .filter(|(dco, ..)| (DCO_LOWEST_KHZ..=DCO_HIGHEST_KHZ).contains(dco))
        .min_by_key(|(dco, ..)| dco.abs_diff(DCO_MIDDLE_KHZ))?;
    Some(ComboPll {
        dco_integer: (dco / reference) as u32,
        dco_fraction: (((dco % reference) << FRACTION_BITS) / reference) as u32,
        pdiv,
        kdiv,
        qdiv,
    })
}

/// The port clock a PLL gives, in kHz: the oscillator over the dividers.
pub fn port_clock_khz(pll: &ComboPll, reference_khz: u32) -> u32 {
    let reference = match reference_khz {
        HALVED_REFERENCE_KHZ => u64::from(reference_khz / 2),
        reference => u64::from(reference),
    };
    let dco = reference * u64::from(pll.dco_integer)
        + ((reference * u64::from(pll.dco_fraction)) >> FRACTION_BITS);
    (dco / u64::from(pll.pdiv * pll.kdiv * pll.qdiv.max(1)).max(1)) as u32
}

const PHY_BASES: [usize; 2] = [0x16_2000, 0x6_c000];
const TX_BASE: usize = 0x680;
const TX_LANE_FIRST: usize = 0x200;
const TX_LANE_STRIDE: usize = 0x100;
const DW2: usize = 2 * 4;
const DW4: usize = 4 * 4;
const DW5: usize = 5 * 4;
const DW7: usize = 7 * 4;
const TRAINING_ENABLE: u32 = 1 << 31;
const TAP3_DISABLE: u32 = 1 << 29;
const SCALING_MODE_MASK: u32 = 0x7 << 18;
const SCALING_MODE: u32 = 2 << 18;
const TERMINATION_MASK: u32 = 0x7 << 3;
const TERMINATION: u32 = 6 << 3;
const RCOMP_SCALAR: u32 = 0x98;
const SWING_MASK: u32 = 1 << 15 | 0x7 << 11 | 0xff;
const CURSOR_MASK: u32 = 0x3f << 12 | 0x3f << 6 | 0x3f;
const N_SCALAR_MASK: u32 = 0x7f << 24;
const LANES: usize = 4;
const DRIVE_TABLE: [(u32, u32, u32, u32, u32); 10] = [
    (0xa, 0x35, 0x3f, 0x00, 0x00),
    (0xa, 0x4f, 0x37, 0x00, 0x08),
    (0xc, 0x71, 0x2f, 0x00, 0x10),
    (0x6, 0x7f, 0x2b, 0x00, 0x14),
    (0xa, 0x4c, 0x3f, 0x00, 0x00),
    (0xc, 0x73, 0x34, 0x00, 0x0b),
    (0x6, 0x7f, 0x2f, 0x00, 0x10),
    (0xc, 0x6c, 0x3c, 0x00, 0x03),
    (0x6, 0x7f, 0x35, 0x00, 0x0a),
    (0x6, 0x7f, 0x3f, 0x00, 0x00),
];

/// Sets combo PHY `phy`'s (0 for port A, 1 for B) transmitters to drive level `level` (from
/// [`super::intel_dekel::drive_level`]) from Intel's DisplayPort table (D788): the swing
/// select, the N scalar and the cursor coefficients, with training turned off while they
/// change.
pub fn set_drive(regs: &(impl crate::io::RegisterIo + ?Sized), phy: usize, level: usize) {
    let Some(&base) = PHY_BASES.get(phy) else {
        return;
    };
    let (swing, n_scalar, cursor, post_2, post_1) = DRIVE_TABLE[level.min(DRIVE_TABLE.len() - 1)];
    let group = base + TX_BASE;
    let lane = |index: usize| base + TX_BASE + TX_LANE_FIRST + index * TX_LANE_STRIDE;
    let modify = |at: usize, mask: u32, value: u32| {
        regs.write32(at, regs.read32(at) & !mask | value);
    };
    modify(group + DW5, TRAINING_ENABLE, 0);
    modify(
        group + DW5,
        SCALING_MODE_MASK | TERMINATION_MASK | TAP3_DISABLE,
        SCALING_MODE | TERMINATION | TAP3_DISABLE,
    );
    for index in 0..LANES {
        let swing_bits = (swing >> 3) << 15 | (swing & 0x7) << 11 | RCOMP_SCALAR;
        modify(lane(index) + DW2, SWING_MASK, swing_bits);
        modify(lane(index) + DW4, CURSOR_MASK, post_1 << 12 | post_2 << 6 | cursor);
        modify(lane(index) + DW7, N_SCALAR_MASK, n_scalar << 24);
    }
    modify(group + DW5, TRAINING_ENABLE, TRAINING_ENABLE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oscillator_is_8_1_ghz_as_whole_and_fractional_multiples_of_the_reference() {
        let pll = for_display_port(0x0a, 24_000).unwrap();
        assert_eq!((pll.dco_integer, pll.dco_fraction), (0x151, 0x4000));
        let pll = for_display_port(0x0a, 19_200).unwrap();
        assert_eq!((pll.dco_integer, pll.dco_fraction), (0x1a5, 0x7000));
        assert_eq!(
            for_display_port(0x0a, 38_400),
            for_display_port(0x0a, 19_200)
        );
    }

    #[test]
    fn each_rate_s_port_clock_is_half_its_link_rate() {
        for (rate, khz) in [
            (0x06, 810_000),
            (0x0a, 1_350_000),
            (0x14, 2_700_000),
            (0x1e, 4_050_000),
        ] {
            let pll = for_display_port(rate, 24_000).unwrap();
            assert_eq!(port_clock_khz(&pll, 24_000), khz);
        }
        assert_eq!(for_display_port(0x10, 24_000), None);
    }

    #[test]
    fn an_hdmi_oscillator_is_five_times_the_rate_times_its_dividers() {
        let pll = for_hdmi(594_000, 24_000).unwrap();
        assert_eq!(pll.pdiv * pll.kdiv, 3);
        assert_eq!(port_clock_khz(&pll, 24_000) / 5, 594_000);
        assert!(for_hdmi(148_500, 24_000).is_some());
    }

    #[test]
    fn the_registers_carry_the_encoded_dividers() {
        let pll = for_display_port(0x14, 24_000).unwrap();
        assert_eq!(pll.configuration_0(), 0x151 | 0x4000 << 10);
        assert_eq!(pll.configuration_1(), 2 << 2 | 1 << 6 | 3);
    }
}

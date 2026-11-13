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

/// A combo PLL's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComboPll {
    pub dco_integer: u32,
    pub dco_fraction: u32,
    /// The P divider: 2, 3, 5 or 7.
    pub pdiv: u32,
    /// The K divider: 1, 2 or 3.
    pub kdiv: u32,
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
        pdiv << PDIV_SHIFT | kdiv << KDIV_SHIFT | CENTRAL_FREQUENCY_8400
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
    (dco / u64::from(pll.pdiv * pll.kdiv).max(1)) as u32
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
    fn the_registers_carry_the_encoded_dividers() {
        let pll = for_display_port(0x14, 24_000).unwrap();
        assert_eq!(pll.configuration_0(), 0x151 | 0x4000 << 10);
        assert_eq!(pll.configuration_1(), 2 << 2 | 1 << 6 | 3);
    }
}

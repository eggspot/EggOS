//! HDMI on Intel's display 9 (D466): a sink's EDID over the engine's own I2C controller,
//! GMBUS, and the HDMI PLL's settings. The PLL's oscillator must sit within 1% above or 6%
//! below one of three central frequencies while it runs at five times the pixel clock times
//! a divider; the divider, even ones first, is then split into the PLL's three dividers.
//! Pure arithmetic and a register walk over [`RegisterIo`], tested on the host.

use crate::io::RegisterIo;

const GMBUS_SELECT: usize = 0xc5100;
const GMBUS_COMMAND: usize = 0xc5104;
const GMBUS_STATUS: usize = 0xc5108;
const GMBUS_DATA: usize = 0xc510c;
const SOFTWARE_READY: u32 = 1 << 30;
const CYCLE_WAIT: u32 = 1 << 25;
const CYCLE_INDEX: u32 = 1 << 26;
const CYCLE_STOP: u32 = 1 << 27;
const BYTE_COUNT_SHIFT: u32 = 16;
const READ: u32 = 1;
const HARDWARE_READY: u32 = 1 << 11;
const NOT_ACKNOWLEDGED: u32 = 1 << 10;
const WAIT_PHASE: u32 = 1 << 14;
const ACTIVE: u32 = 1 << 9;
const EDID_ADDRESS: u32 = 0x50;
const REFERENCE_KHZ: u64 = 24_000;
const FRACTION_ONE: u64 = 0x8000;
const CENTRAL_KHZ: [u64; 3] = [8_400_000, 9_000_000, 9_600_000];
const EVEN_DIVIDERS: [u32; 36] = [
    4, 6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 30, 32, 36, 40, 42, 44, 48, 52, 54, 56, 60, 64, 66,
    68, 70, 72, 76, 78, 80, 84, 88, 90, 92, 96, 98,
];
const ODD_DIVIDERS: [u32; 7] = [3, 5, 7, 9, 15, 21, 35];
const ABOVE_LIMIT: u64 = 100;
const BELOW_LIMIT: u64 = 600;
const FREQUENCY_ENABLE: u32 = 1 << 31;
const FRACTION_SHIFT: u32 = 9;
const QDIV_RATIO_SHIFT: u32 = 8;
const QDIV_MODE: u32 = 1 << 7;
const KDIV_SHIFT: u32 = 5;
const PDIV_SHIFT: u32 = 2;

/// Why GMBUS could not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GmbusError {
    /// Nothing answered at the address: no sink on the pins.
    NotAcknowledged,
    /// The controller did not finish in time.
    TimedOut,
}

/// The GMBUS pin pair of digital port `port` on display 9 (B to D are 1 to 3).
pub fn gmbus_pins(port: usize) -> Option<u32> {
    match port {
        1 => Some(5),
        2 => Some(4),
        3 => Some(6),
        _ => None,
    }
}

fn wait<R: RegisterIo>(
    regs: &R,
    done: u32,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), GmbusError> {
    loop {
        let status = regs.read32(GMBUS_STATUS);
        if status & NOT_ACKNOWLEDGED != 0 {
            return Err(GmbusError::NotAcknowledged);
        }
        if status & done != 0 {
            return Ok(());
        }
        if !keep_waiting() {
            return Err(GmbusError::TimedOut);
        }
    }
}

/// Reads `out.len()` bytes of the EDID from offset 0 over GMBUS pin pair `pins`, polling while
/// `keep_waiting` says to; the bus is stopped and released however it ends.
pub fn read_edid<R: RegisterIo>(
    regs: &R,
    pins: u32,
    out: &mut [u8],
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), GmbusError> {
    regs.write32(GMBUS_SELECT, pins);
    regs.write32(
        GMBUS_COMMAND,
        SOFTWARE_READY
            | CYCLE_WAIT
            | CYCLE_INDEX
            | (out.len() as u32) << BYTE_COUNT_SHIFT
            | EDID_ADDRESS << 1
            | READ,
    );
    let mut read = (|| {
        for chunk in out.chunks_mut(4) {
            wait(regs, HARDWARE_READY, keep_waiting)?;
            let word = regs.read32(GMBUS_DATA).to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
        wait(regs, WAIT_PHASE, keep_waiting)
    })();
    regs.write32(GMBUS_COMMAND, SOFTWARE_READY | CYCLE_STOP);
    while regs.read32(GMBUS_STATUS) & ACTIVE != 0 {
        if !keep_waiting() {
            read = read.and(Err(GmbusError::TimedOut));
            break;
        }
    }
    regs.write32(GMBUS_SELECT, 0);
    read
}

/// A display-9 HDMI PLL's settings: its oscillator and its three dividers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdmiPll {
    pub dco_khz: u64,
    pub central_khz: u64,
    pub pdiv: u32,
    pub qdiv: u32,
    pub kdiv: u32,
}

fn deviation(dco: u64, central: u64) -> Option<u64> {
    match dco >= central {
        true => Some((dco - central) * 10_000 / central).filter(|&d| d <= ABOVE_LIMIT),
        false => Some((central - dco) * 10_000 / central).filter(|&d| d <= BELOW_LIMIT),
    }
}

fn split(divider: u32) -> Option<(u32, u32, u32)> {
    if divider % 2 == 0 {
        let half = divider / 2;
        return match half {
            1 | 2 | 3 | 5 => Some((2, 1, half)),
            half if half % 2 == 0 => Some((2, half / 2, 2)),
            half if half % 3 == 0 => Some((3, half / 3, 2)),
            half if half % 7 == 0 => Some((7, half / 7, 2)),
            _ => None,
        };
    }
    match divider {
        3 | 9 => Some((3, 1, divider / 3)),
        5 | 7 => Some((divider, 1, 1)),
        15 => Some((3, 1, 5)),
        21 => Some((7, 1, 3)),
        35 => Some((7, 1, 5)),
        _ => None,
    }
}

/// The HDMI PLL for a `pixel_khz` pixel clock at 8 bits a colour: the divider whose
/// oscillator lies closest to a central frequency, even dividers chosen over odd ones.
pub fn hdmi_pll(pixel_khz: u32) -> Option<HdmiPll> {
    let afe = u64::from(pixel_khz) * 5;
    let best = |dividers: &[u32]| {
        CENTRAL_KHZ
            .iter()
            .flat_map(|&central| dividers.iter().map(move |&divider| (central, divider)))
            .filter_map(|(central, divider)| {
                deviation(afe * u64::from(divider), central).map(|d| (d, central, divider))
            })
            .min_by_key(|&(d, ..)| d)
    };
    let (_, central, divider) = best(&EVEN_DIVIDERS).or_else(|| best(&ODD_DIVIDERS))?;
    let (pdiv, qdiv, kdiv) = split(divider)?;
    Some(HdmiPll {
        dco_khz: afe * u64::from(divider),
        central_khz: central,
        pdiv,
        qdiv,
        kdiv,
    })
}

impl HdmiPll {
    /// `DPLL_CFGCR1`: the oscillator as a whole and fractional multiple of the 24 MHz reference.
    pub fn configuration_1(&self) -> u32 {
        let integer = self.dco_khz / REFERENCE_KHZ;
        let fraction = (self.dco_khz % REFERENCE_KHZ) * FRACTION_ONE / REFERENCE_KHZ;
        FREQUENCY_ENABLE | (fraction as u32) << FRACTION_SHIFT | integer as u32 & 0x1ff
    }

    /// `DPLL_CFGCR2`: the dividers, encoded, and the central frequency.
    pub fn configuration_2(&self) -> u32 {
        let pdiv = match self.pdiv {
            1 => 0,
            2 => 1,
            3 => 2,
            _ => 4,
        };
        let kdiv = match self.kdiv {
            5 => 0,
            2 => 1,
            3 => 2,
            _ => 3,
        };
        let central = match self.central_khz {
            9_600_000 => 0,
            9_000_000 => 1,
            _ => 3,
        };
        let qdiv = match self.qdiv {
            1 => 0,
            ratio => ratio << QDIV_RATIO_SHIFT | QDIV_MODE,
        };
        qdiv | kdiv << KDIV_SHIFT | pdiv << PDIV_SHIFT | central
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use std::collections::VecDeque;

    #[test]
    fn a_1080p_pixel_clock_gets_an_even_divider_near_a_central_frequency() {
        let pll = hdmi_pll(148_500).unwrap();
        assert_eq!(pll.dco_khz % 742_500, 0);
        let divider = pll.dco_khz / 742_500;
        assert_eq!(divider as u32, pll.pdiv * pll.qdiv * pll.kdiv);
        assert!(divider % 2 == 0);
        assert!(deviation(pll.dco_khz, pll.central_khz).is_some());
    }

    #[test]
    fn dividers_split_into_the_pll_s_three() {
        assert_eq!(split(4), Some((2, 1, 2)));
        assert_eq!(split(20), Some((2, 5, 2)));
        assert_eq!(split(42), Some((3, 7, 2)));
        assert_eq!(split(98), Some((7, 7, 2)));
        assert_eq!(split(15), Some((3, 1, 5)));
        assert_eq!(split(35), Some((7, 1, 5)));
    }

    #[test]
    fn the_registers_carry_the_oscillator_and_the_encoded_dividers() {
        let pll = HdmiPll {
            dco_khz: 8_910_000,
            central_khz: 9_000_000,
            pdiv: 2,
            qdiv: 3,
            kdiv: 2,
        };
        assert_eq!(pll.configuration_1(), FREQUENCY_ENABLE | 0x2000 << 9 | 371);
        assert_eq!(pll.configuration_2(), 3 << 8 | QDIV_MODE | 1 << 5 | 1 << 2 | 1);
    }

    #[derive(Default)]
    struct Gmbus {
        status: RefCell<VecDeque<u32>>,
        data: RefCell<VecDeque<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
    }

    impl RegisterIo for Gmbus {
        fn read8(&self, _: usize) -> u8 {
            0
        }
        fn write8(&self, _: usize, _: u8) {}
        fn read16(&self, _: usize) -> u16 {
            0
        }
        fn write16(&self, _: usize, _: u16) {}
        fn read32(&self, offset: usize) -> u32 {
            match offset {
                GMBUS_STATUS => self.status.borrow_mut().pop_front().unwrap_or(0),
                GMBUS_DATA => self.data.borrow_mut().pop_front().unwrap_or(0),
                _ => 0,
            }
        }
        fn write32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
        }
    }

    #[test]
    fn an_edid_is_read_four_bytes_at_a_time_then_the_bus_stops_and_lets_go() {
        let bus = Gmbus::default();
        bus.status
            .borrow_mut()
            .extend([HARDWARE_READY, HARDWARE_READY, WAIT_PHASE, 0]);
        bus.data
            .borrow_mut()
            .extend([0xffff_ff00, 0x00ff_ffff]);
        let mut out = [0u8; 8];
        read_edid(&bus, 5, &mut out, &mut || true).unwrap();
        assert_eq!(out, [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        let writes = bus.writes.borrow();
        assert_eq!(writes[0], (GMBUS_SELECT, 5));
        assert_eq!(
            writes[1].1,
            SOFTWARE_READY | CYCLE_WAIT | CYCLE_INDEX | 8 << BYTE_COUNT_SHIFT | 0x50 << 1 | READ
        );
        assert_eq!(writes[2], (GMBUS_COMMAND, SOFTWARE_READY | CYCLE_STOP));
        assert_eq!(writes[3], (GMBUS_SELECT, 0));
    }

    #[test]
    fn no_sink_on_the_pins_is_not_acknowledged_and_the_bus_still_stops() {
        let bus = Gmbus::default();
        bus.status.borrow_mut().extend([NOT_ACKNOWLEDGED, 0]);
        let outcome = read_edid(&bus, 4, &mut [0u8; 8], &mut || true);
        assert_eq!(outcome, Err(GmbusError::NotAcknowledged));
        assert_eq!(bus.writes.borrow().last(), Some(&(GMBUS_SELECT, 0)));
    }
}

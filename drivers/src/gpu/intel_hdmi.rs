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
const SCDC_ADDRESS: u32 = 0x54;
const INDEX_SHIFT: u32 = 8;
const SCDC_TMDS_CONFIG: u8 = 0x20;
const SCDC_SCRAMBLING: u8 = 1 << 0;
const SCDC_HIGH_RATIO: u8 = 1 << 1;
const CEA_EXTENSION: u8 = 0x02;
const VENDOR_BLOCK: u8 = 3;
const HDMI_OUI: [u8; 3] = [0x03, 0x0c, 0x00];
const HDMI_FORUM_OUI: [u8; 3] = [0xd8, 0x5d, 0xc4];
const DEEP_COLOUR_30: u8 = 1 << 4;
const DEEP_COLOUR_36: u8 = 1 << 5;
const SCDC_PRESENT: u8 = 1 << 7;
const TMDS_STEP_KHZ: u32 = 5_000;
/// The highest TMDS character rate HDMI 1.4 runs without scrambling, in kHz.
pub const SCRAMBLING_ABOVE_KHZ: u32 = 340_000;
const LEGACY_MAX_TMDS_KHZ: u32 = 165_000;
const AVI_TYPE: u8 = 0x82;
const AVI_VERSION: u8 = 2;
const AVI_LENGTH: u8 = 13;
const AUDIO_TYPE: u8 = 0x84;
const AUDIO_VERSION: u8 = 1;
const AUDIO_LENGTH: u8 = 10;
const AVI_ACTIVE_FORMAT_PRESENT: u8 = 1 << 4;
const AVI_UNDERSCAN: u8 = 2;
const AVI_FULL_RANGE: u8 = 2 << 2;
const AVI_SAME_AS_PICTURE: u8 = 8;
/// The bytes a data island packet register block holds.
pub const DIP_BYTES: usize = 32;
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
    read_at(regs, pins, 0, out, keep_waiting)
}

/// Reads `out.len()` bytes of the EDID from byte `index` (128 for the first extension block,
/// D787) over GMBUS.
pub fn read_at<R: RegisterIo>(
    regs: &R,
    pins: u32,
    index: u8,
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
            | u32::from(index) << INDEX_SHIFT
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

/// Writes `value` to the sink's SCDC register `register` over GMBUS (D787).
pub fn write_scdc<R: RegisterIo>(
    regs: &R,
    pins: u32,
    register: u8,
    value: u8,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), GmbusError> {
    regs.write32(GMBUS_SELECT, pins);
    regs.write32(GMBUS_DATA, u32::from(register) | u32::from(value) << 8);
    regs.write32(
        GMBUS_COMMAND,
        SOFTWARE_READY | CYCLE_WAIT | 2 << BYTE_COUNT_SHIFT | SCDC_ADDRESS << 1,
    );
    let mut written = wait(regs, WAIT_PHASE, keep_waiting);
    regs.write32(GMBUS_COMMAND, SOFTWARE_READY | CYCLE_STOP);
    while regs.read32(GMBUS_STATUS) & ACTIVE != 0 {
        if !keep_waiting() {
            written = written.and(Err(GmbusError::TimedOut));
            break;
        }
    }
    regs.write32(GMBUS_SELECT, 0);
    written
}

/// The SCDC `TMDS_Config` value for scrambling and the 1/40 clock ratio above 340 MHz.
pub fn scdc_tmds_config(scrambling: bool) -> (u8, u8) {
    let value = match scrambling {
        true => SCDC_SCRAMBLING | SCDC_HIGH_RATIO,
        false => 0,
    };
    (SCDC_TMDS_CONFIG, value)
}

/// What an HDMI sink's CEA extension says it takes (D787).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdmiSink {
    /// The highest TMDS character rate, in kHz.
    pub max_tmds_khz: u32,
    pub deep_colour_30: bool,
    pub deep_colour_36: bool,
    /// HDMI 2.0's status and control data channel, needed for scrambling.
    pub scdc: bool,
}

impl Default for HdmiSink {
    fn default() -> Self {
        Self {
            max_tmds_khz: LEGACY_MAX_TMDS_KHZ,
            deep_colour_30: false,
            deep_colour_36: false,
            scdc: false,
        }
    }
}

/// The sink's HDMI and HDMI Forum vendor blocks in CEA extension block `extension`.
pub fn hdmi_sink(extension: &[u8]) -> HdmiSink {
    let mut sink = HdmiSink::default();
    if extension.first() != Some(&CEA_EXTENSION) || extension.len() < 4 {
        return sink;
    }
    let end = usize::from(extension[2]).clamp(4, extension.len());
    let mut at = 4;
    while at < end {
        let tag = extension[at] >> 5;
        let length = usize::from(extension[at] & 0x1f);
        let Some(block) = extension.get(at + 1..at + 1 + length) else {
            break;
        };
        if tag == VENDOR_BLOCK && block.len() >= 3 {
            if block[..3] == HDMI_OUI {
                if let Some(&flags) = block.get(5) {
                    sink.deep_colour_30 = flags & DEEP_COLOUR_30 != 0;
                    sink.deep_colour_36 = flags & DEEP_COLOUR_36 != 0;
                }
                if let Some(&max) = block.get(6).filter(|&&max| max != 0) {
                    sink.max_tmds_khz = sink.max_tmds_khz.max(u32::from(max) * TMDS_STEP_KHZ);
                }
            }
            if block[..3] == HDMI_FORUM_OUI {
                if let Some(&max) = block.get(4).filter(|&&max| max != 0) {
                    sink.max_tmds_khz = sink.max_tmds_khz.max(u32::from(max) * TMDS_STEP_KHZ);
                }
                sink.scdc = block.get(5).is_some_and(|flags| flags & SCDC_PRESENT != 0);
            }
        }
        at += 1 + length;
    }
    sink
}

/// The deepest colour (bits a component: 12, 10 or 8) `sink` takes at `pixel_khz` within its
/// TMDS limit and the source's `source_max_khz`, with its TMDS character rate.
pub fn deep_colour(sink: &HdmiSink, pixel_khz: u32, source_max_khz: u32) -> (u32, u32) {
    let limit = sink.max_tmds_khz.min(source_max_khz);
    let fits = |bits: u32| {
        let tmds = pixel_khz * bits / 8;
        let scrambled_ok = tmds <= SCRAMBLING_ABOVE_KHZ || sink.scdc;
        (tmds <= limit && scrambled_ok).then_some(tmds)
    };
    [(12, sink.deep_colour_36), (10, sink.deep_colour_30), (8, true)]
        .into_iter()
        .filter(|(_, offered)| *offered)
        .find_map(|(bits, _)| fits(bits).map(|tmds| (bits, tmds)))
        .unwrap_or((8, pixel_khz))
}

fn checksummed<const N: usize>(mut frame: [u8; N]) -> [u8; N] {
    let sum = frame.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    frame[3] = 0u8.wrapping_sub(sum);
    frame
}

/// The AVI infoframe for an RGB picture with CEA video code `vic` (0 for none), full range,
/// underscanned, as the source sends it: header, checksum, payload (D787).
pub fn avi_infoframe(vic: u8) -> [u8; 4 + AVI_LENGTH as usize] {
    let mut frame = [0u8; 4 + AVI_LENGTH as usize];
    frame[..3].copy_from_slice(&[AVI_TYPE, AVI_VERSION, AVI_LENGTH]);
    frame[4] = AVI_ACTIVE_FORMAT_PRESENT | AVI_UNDERSCAN;
    frame[5] = AVI_SAME_AS_PICTURE;
    frame[6] = AVI_FULL_RANGE;
    frame[7] = vic;
    checksummed(frame)
}

/// The audio infoframe for `channels` channels of PCM whose format the stream itself gives
/// (D787), for the display audio codec to carry.
pub fn audio_infoframe(channels: u8) -> [u8; 4 + AUDIO_LENGTH as usize] {
    let mut frame = [0u8; 4 + AUDIO_LENGTH as usize];
    frame[..3].copy_from_slice(&[AUDIO_TYPE, AUDIO_VERSION, AUDIO_LENGTH]);
    frame[4] = channels.clamp(1, 8) - 1;
    checksummed(frame)
}

/// `frame` as the data island packet registers take it: little-endian words, zero-padded.
pub fn dip_words(frame: &[u8]) -> [u32; DIP_BYTES / 4] {
    let mut bytes = [0u8; DIP_BYTES];
    let length = frame.len().min(DIP_BYTES);
    bytes[..length].copy_from_slice(&frame[..length]);
    core::array::from_fn(|index| {
        u32::from_le_bytes([
            bytes[4 * index],
            bytes[4 * index + 1],
            bytes[4 * index + 2],
            bytes[4 * index + 3],
        ])
    })
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
    if divider.is_multiple_of(2) {
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

    #[test]
    fn a_sink_s_vendor_blocks_give_its_deep_colour_and_scrambling() {
        let mut extension = vec![0u8; 128];
        extension[0] = 0x02;
        extension[2] = 4 + 8 + 8;
        extension[4..12].copy_from_slice(&[0x67, 0x03, 0x0c, 0x00, 0x10, 0x00, 0x30, 0x78]);
        extension[12..20].copy_from_slice(&[0x67, 0xd8, 0x5d, 0xc4, 0x01, 0x78, 0x80, 0x00]);
        let sink = hdmi_sink(&extension);
        assert!(sink.deep_colour_30 && sink.deep_colour_36 && sink.scdc);
        assert_eq!(sink.max_tmds_khz, 600_000);
        assert_eq!(deep_colour(&sink, 297_000, 600_000), (12, 445_500));
        assert_eq!(deep_colour(&HdmiSink::default(), 148_500, 600_000), (8, 148_500));
        let avi = avi_infoframe(16);
        assert_eq!(avi.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)), 0);
        assert_eq!(dip_words(&avi)[0] & 0xff_ffff, 0x0d_0282);
    }
    use core::cell::RefCell;
    use std::collections::VecDeque;

    #[test]
    fn a_1080p_pixel_clock_gets_an_even_divider_near_a_central_frequency() {
        let pll = hdmi_pll(148_500).unwrap();
        assert_eq!(pll.dco_khz % 742_500, 0);
        let divider = pll.dco_khz / 742_500;
        assert_eq!(divider as u32, pll.pdiv * pll.qdiv * pll.kdiv);
        assert!(divider.is_multiple_of(2));
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
        assert_eq!(
            pll.configuration_2(),
            3 << 8 | QDIV_MODE | 1 << 5 | 1 << 2 | 1
        );
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
        bus.data.borrow_mut().extend([0xffff_ff00, 0x00ff_ffff]);
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

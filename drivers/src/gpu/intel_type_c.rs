//! Intel's Type-C ports on display 12 and 13 (D525): ports D to I, whose PHY the display shares
//! with USB and Thunderbolt. The Flexible IO Adapter (FIA, up to three, two ports each when
//! modular) says per port whether a DisplayPort alternate mode or a Thunderbolt sink is live,
//! which PHY lanes DisplayPort got and the pin assignment; the display takes the PHY from the
//! IOM firmware once it says the PHY is ready. Tiger Lake keeps the PHY from its TC-cold power
//! state through a PCODE request and hands ownership through the FIA; Alder Lake-P and Raptor
//! Lake-P report readiness through the TCSS and take ownership in the port's `DDI_BUF_CTL`, and
//! leave TC-cold while the port's AUX power well is on. Register places are from Intel's
//! MIT-licensed i915 driver (`INTEL-NOTICE.md`).

use crate::io::RegisterIo;

/// The DDI index of the first Type-C port, port D.
pub const FIRST_PORT: usize = 3;
/// The Type-C ports display 12 has at most.
pub const MOST_PORTS: usize = 6;

const FIA_BASES: [usize; 3] = [0x16_3000, 0x16_e000, 0x16_f000];
const PIN_ASSIGNMENT: usize = 0x880;
const PHY_READY: usize = 0x890;
const PHY_TAKEN: usize = 0x894;
const LANE_STATE: usize = 0x8a0;
const MODULAR_FIA: u32 = 1 << 4;
const LIVE_THUNDERBOLT_BIT: u32 = 6;
const LIVE_ALTERNATE_BIT: u32 = 5;
const LANES_MASK: u32 = 0xf;
const PIN_MASK: u32 = 0xf;
const TCSS_STATUS: usize = 0x16_1500;
const TCSS_READY: u32 = 1 << 2;
const TCSS_LIVE_THUNDERBOLT: u32 = 1 << 1;
const TCSS_LIVE_ALTERNATE: u32 = 1;
const DDI_BUF_CTL: usize = 0x64000;
const PORT_STRIDE: usize = 0x100;
const PHY_OWNERSHIP: u32 = 1 << 6;
const PCODE_MAILBOX: usize = 0x13_8124;
const PCODE_DATA_0: usize = 0x13_8128;
const PCODE_DATA_1: usize = 0x13_812c;
const PCODE_READY: u32 = 1 << 31;
const PCODE_ERROR_MASK: u32 = 0xff;
const PCODE_TC_COLD: u32 = 0x26;
const TC_COLD_BLOCK: u32 = 0;
const TC_COLD_EXIT_FAILED: u32 = 1;
const AUX_POWER_WELLS: usize = 0x4_5444;
const AUX_POWER_TYPE_C: usize = 3;
const AUX_POWER_THUNDERBOLT: usize = 9;
const COLD: u32 = u32::MAX;
const TIGER_LAKE: u8 = 0x9a;
const ALDER_LAKE_P: [u16; 7] = [0x46a0, 0x46b0, 0x46c0, 0x46d0, 0xa720, 0xa7a0, 0xa7a8];
const ALDER_LAKE_P_SINGLES: [u16; 6] = [0x4626, 0x4628, 0x462a, 0xa721, 0xa7a1, 0xa7a9];

/// How a display engine reaches its Type-C PHYs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// Tiger Lake: TC-cold through PCODE, ownership through the FIA.
    Tiger,
    /// Alder Lake-P and Raptor Lake-P: readiness in the TCSS, ownership in `DDI_BUF_CTL`.
    Alder,
}

/// The flavour of device `device`'s Type-C ports; `None` when it has none.
pub fn flavour(device: u16) -> Option<Flavour> {
    if (device >> 8) as u8 == TIGER_LAKE {
        return Some(Flavour::Tiger);
    }
    let alder = ALDER_LAKE_P.iter().any(|&base| device & 0xfff0 == base & 0xfff0)
        || ALDER_LAKE_P_SINGLES.contains(&device);
    alder.then_some(Flavour::Alder)
}

/// What a connected Type-C port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// DisplayPort over the USB-C connector's lanes, the display owning the PHY.
    DisplayPortAlternate,
    /// DisplayPort tunnelled through Thunderbolt, the PHY Thunderbolt's.
    Thunderbolt,
}

/// Why a Type-C port could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCError {
    /// The PHY stayed in TC-cold: its registers read all ones.
    Cold,
    /// No sink is live on the port.
    NotConnected,
    /// The IOM firmware never said the PHY was ready to hand over.
    NotReady,
    /// PCODE did not take or answer the request in time.
    PcodeBusy,
    /// PCODE refused to keep the PHY out of TC-cold.
    PcodeFailed,
    /// The port's AUX power well did not come on.
    NoAuxPower,
}

impl TypeCError {
    pub fn description(self) -> &'static str {
        match self {
            Self::Cold => "the Type-C PHY stayed cold",
            Self::NotConnected => "nothing is connected",
            Self::NotReady => "the Type-C PHY was not handed over",
            Self::PcodeBusy => "PCODE did not answer",
            Self::PcodeFailed => "PCODE kept the Type-C PHY cold",
            Self::NoAuxPower => "the Type-C AUX power did not come on",
        }
    }
}

/// A Type-C port the display now drives: its DDI index, what it carries, its DisplayPort
/// lanes and its pin assignment (0 for none, 1 for A, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connected {
    pub port: usize,
    pub mode: Mode,
    pub lanes: u8,
    pub pin_assignment: u8,
}

fn wait(
    regs: &(impl RegisterIo + ?Sized),
    place: usize,
    mask: u32,
    wanted: u32,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> bool {
    loop {
        let value = regs.read32(place);
        if value != COLD && value & mask == wanted {
            return true;
        }
        if !keep_waiting() {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Asks PCODE to keep Tiger Lake's Type-C PHYs out of TC-cold.
pub fn block_cold(
    regs: &(impl RegisterIo + ?Sized),
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), TypeCError> {
    if !wait(regs, PCODE_MAILBOX, PCODE_READY, 0, keep_waiting) {
        return Err(TypeCError::PcodeBusy);
    }
    regs.write32(PCODE_DATA_0, TC_COLD_BLOCK);
    regs.write32(PCODE_DATA_1, 0);
    regs.write32(PCODE_MAILBOX, PCODE_READY | PCODE_TC_COLD);
    if !wait(regs, PCODE_MAILBOX, PCODE_READY, 0, keep_waiting) {
        return Err(TypeCError::PcodeBusy);
    }
    if regs.read32(PCODE_MAILBOX) & PCODE_ERROR_MASK != 0
        || regs.read32(PCODE_DATA_0) & TC_COLD_EXIT_FAILED != 0
    {
        return Err(TypeCError::PcodeFailed);
    }
    Ok(())
}

/// Turns on the AUX power well of Type-C port `index` (0 for port D) in `mode`.
pub fn power_aux(
    regs: &(impl RegisterIo + ?Sized),
    index: usize,
    mode: Mode,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), TypeCError> {
    let well = index
        + match mode {
            Mode::DisplayPortAlternate => AUX_POWER_TYPE_C,
            Mode::Thunderbolt => AUX_POWER_THUNDERBOLT,
        };
    let request = 2 << (2 * well);
    let state = 1 << (2 * well);
    regs.write32(AUX_POWER_WELLS, regs.read32(AUX_POWER_WELLS) | request);
    match wait(regs, AUX_POWER_WELLS, state, state, keep_waiting) {
        true => Ok(()),
        false => Err(TypeCError::NoAuxPower),
    }
}

fn fia(modular: bool, index: usize) -> (usize, u32) {
    match modular {
        true => (FIA_BASES[(index / 2).min(2)], (index % 2) as u32),
        false => (FIA_BASES[0], index as u32),
    }
}

/// Connects Type-C port `index` (0 for port D) of a `flavour` display engine: keeps its PHY
/// out of TC-cold (Tiger Lake), finds what is live on it, and for DisplayPort alternate mode
/// waits for the PHY to be handed over and takes it, then reads its lanes and pin
/// assignment. The port's AUX power is the caller's to turn on for the mode.
pub fn connect(
    regs: &(impl RegisterIo + ?Sized),
    flavour: Flavour,
    index: usize,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<Connected, TypeCError> {
    if flavour == Flavour::Tiger {
        block_cold(regs, keep_waiting)?;
    }
    let first = regs.read32(FIA_BASES[0] + LANE_STATE);
    if first == COLD {
        return Err(TypeCError::Cold);
    }
    let modular = flavour == Flavour::Alder || first & MODULAR_FIA != 0;
    let (base, slot) = fia(modular, index);
    let lane_state = regs.read32(base + LANE_STATE);
    let status_place = TCSS_STATUS + 4 * index;
    let mode = match flavour {
        Flavour::Tiger => {
            let live = |bit: u32| lane_state & 1 << (slot * 8 + bit) != 0;
            match lane_state {
                COLD => return Err(TypeCError::Cold),
                _ if live(LIVE_ALTERNATE_BIT) => Mode::DisplayPortAlternate,
                _ if live(LIVE_THUNDERBOLT_BIT) => Mode::Thunderbolt,
                _ => return Err(TypeCError::NotConnected),
            }
        }
        Flavour::Alder => match regs.read32(status_place) {
            COLD => return Err(TypeCError::Cold),
            status if status & TCSS_LIVE_ALTERNATE != 0 => Mode::DisplayPortAlternate,
            status if status & TCSS_LIVE_THUNDERBOLT != 0 => Mode::Thunderbolt,
            _ => return Err(TypeCError::NotConnected),
        },
    };
    let port = FIRST_PORT + index;
    if mode == Mode::Thunderbolt {
        return Ok(Connected {
            port,
            mode,
            lanes: 4,
            pin_assignment: 0,
        });
    }
    let ready = match flavour {
        Flavour::Tiger => wait(regs, base + PHY_READY, 1 << slot, 1 << slot, keep_waiting),
        Flavour::Alder => wait(regs, status_place, TCSS_READY, TCSS_READY, keep_waiting),
    };
    if !ready {
        return Err(TypeCError::NotReady);
    }
    match flavour {
        Flavour::Tiger => {
            let taken = regs.read32(base + PHY_TAKEN);
            regs.write32(base + PHY_TAKEN, taken | 1 << slot);
        }
        Flavour::Alder => {
            let buffer = DDI_BUF_CTL + port * PORT_STRIDE;
            regs.write32(buffer, regs.read32(buffer) | PHY_OWNERSHIP);
        }
    }
    let lanes = (regs.read32(base + LANE_STATE) >> (slot * 8) & LANES_MASK).count_ones() as u8;
    let pin_assignment = (regs.read32(base + PIN_ASSIGNMENT) >> (slot * 4) & PIN_MASK) as u8;
    Ok(Connected {
        port,
        mode,
        lanes,
        pin_assignment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl Registers {
        fn set(&self, at: usize, value: u32) {
            self.0.borrow_mut().insert(at, value);
        }
    }

    impl RegisterIo for Registers {
        fn read8(&self, offset: usize) -> u8 {
            self.read32(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.set(offset, value.into());
        }
        fn read16(&self, offset: usize) -> u16 {
            self.read32(offset) as u16
        }
        fn write16(&self, offset: usize, value: u16) {
            self.set(offset, value.into());
        }
        fn read32(&self, offset: usize) -> u32 {
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.set(offset, value);
        }
    }

    #[test]
    fn devices_name_their_type_c_flavour() {
        assert_eq!(flavour(0x9a49), Some(Flavour::Tiger));
        assert_eq!(flavour(0x46a6), Some(Flavour::Alder));
        assert_eq!(flavour(0xa7a1), Some(Flavour::Alder));
        assert_eq!(flavour(0x4680), None);
    }

    #[test]
    fn a_tiger_lake_port_in_alternate_mode_is_taken_with_its_lanes() {
        let regs = Registers::default();
        let modular_fia2 = FIA_BASES[1];
        regs.set(FIA_BASES[0] + LANE_STATE, MODULAR_FIA);
        regs.set(modular_fia2 + LANE_STATE, (1 << (8 + LIVE_ALTERNATE_BIT)) | 0x3 << 8);
        regs.set(modular_fia2 + PHY_READY, 1 << 1);
        regs.set(modular_fia2 + PIN_ASSIGNMENT, 4 << 4);
        let mut requests = alloc::vec::Vec::new();
        let connected = connect(&regs, Flavour::Tiger, 3, &mut || {
            let mailbox = regs.read32(PCODE_MAILBOX);
            if mailbox & PCODE_READY != 0 {
                requests.push(mailbox);
                regs.set(PCODE_MAILBOX, 0);
            }
            true
        })
        .unwrap();
        assert_eq!(
            connected,
            Connected {
                port: 6,
                mode: Mode::DisplayPortAlternate,
                lanes: 2,
                pin_assignment: 4,
            }
        );
        assert_eq!(regs.read32(modular_fia2 + PHY_TAKEN), 1 << 1);
        assert_eq!(requests, alloc::vec![PCODE_READY | PCODE_TC_COLD]);
    }

    #[test]
    fn an_alder_lake_port_takes_ownership_in_its_buffer_control() {
        let regs = Registers::default();
        regs.set(FIA_BASES[0] + LANE_STATE, 0xf);
        regs.set(TCSS_STATUS, TCSS_LIVE_ALTERNATE | TCSS_READY);
        regs.set(FIA_BASES[0] + PIN_ASSIGNMENT, 3);
        let connected = connect(&regs, Flavour::Alder, 0, &mut || true).unwrap();
        assert_eq!(connected.lanes, 4);
        assert_eq!(regs.read32(DDI_BUF_CTL + 3 * PORT_STRIDE) & PHY_OWNERSHIP, PHY_OWNERSHIP);
        regs.set(TCSS_STATUS + 4, 0);
        let unplugged = connect(&regs, Flavour::Alder, 1, &mut || true);
        assert_eq!(unplugged, Err(TypeCError::NotConnected));
        regs.set(FIA_BASES[0] + LANE_STATE, COLD);
        assert_eq!(connect(&regs, Flavour::Alder, 0, &mut || true), Err(TypeCError::Cold));
    }
}

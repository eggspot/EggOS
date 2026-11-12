//! Display 14's C10 PHY on ports A and B (D527), Meteor Lake's: the display reaches the PHY's
//! own registers over a message bus, one per PHY lane (a lane serving two transmitters), each
//! read or write a transaction the PHY acknowledges. Bringing a port up is: the port clock
//! muxes set (the forwarded clock ungated, the DDI clocked from the PHY's maximum pixel
//! clock), the PHY lanes reset and taken from reset to their ready power state, the PLL's
//! twenty configuration bytes for the link rate written to lane 0 (Intel's tables for a
//! 38.4 MHz reference, spread spectrum off), the unused transmitters disabled, the DDI's
//! valid frequency set and the PLL requested and acknowledged. Drive levels are presets of
//! swing, pre-cursor and post-cursor overriding each transmitter's. Places and values are from
//! Intel's MIT-licensed i915 driver (`INTEL-NOTICE.md`).

use crate::io::RegisterIo;

/// The ports with a C10 PHY: A and B.
pub const PORTS: usize = 2;

const PORT_STRIDE: usize = 0x100;
const MESSAGE_BUS: usize = 0x64040;
const MESSAGE_STATUS: usize = 8;
const MESSAGE_TIMER: usize = 0x640d8;
const BUFFER_CONTROL_1: usize = 0x64004;
const BUFFER_CONTROL_2: usize = 0x64008;
const BUFFER_CONTROL_3: usize = 0x6400c;
const CLOCK_CONTROL: usize = 0x640e0;
const VALID_FREQUENCY: usize = 0x64030;
const TRANSACTION_PENDING: u32 = 1 << 31;
const COMMAND_SHIFT: u32 = 27;
const COMMAND_MASK: u32 = 0xf << COMMAND_SHIFT;
const WRITE_UNCOMMITTED: u32 = 0x1 << COMMAND_SHIFT;
const WRITE_COMMITTED: u32 = 0x2 << COMMAND_SHIFT;
const READ: u32 = 0x3 << COMMAND_SHIFT;
const READ_ACKNOWLEDGED: u32 = 0x4;
const WRITE_ACKNOWLEDGED: u32 = 0x5;
const DATA_SHIFT: u32 = 16;
const TRANSACTION_RESET: u32 = 1 << 15;
const ADDRESS_MASK: u32 = 0xfff;
const RESPONSE_READY: u32 = 1 << 31;
const RESPONSE_ERROR: u32 = 1 << 15;
const TIMER_VALUE_MASK: u32 = 0xff_ffff;
const TIMER_VALUE: u32 = 0xa000;
const PHY_READY: u32 = 1 << 24;
const PORT_REVERSAL: u32 = 1 << 16;
const PORT_DATA_WIDTH_MASK: u32 = 0x3 << 18;
const PORT_WIDTH_SHIFT: u32 = 1;
const PORT_WIDTH_MASK: u32 = 0x7 << PORT_WIDTH_SHIFT;
const PIPE_RESETS: u32 = 0x3 << 30;
const PHY_STATUSES: u32 = 0x3 << 28;
const POWERDOWN_UPDATES: u32 = 0x3 << 24;
const POWERDOWN_STATES_MASK: u32 = 0xff << 16;
const READY_STATE_SHIFT: u32 = 4;
const READY_STATE_MASK: u32 = 0xf << READY_STATE_SHIFT;
const ACTIVE_AND_STAGGER_MASK: u32 = 0xffff;
const STATE_RESET: u32 = 0x2;
const STATE_READY: u32 = 0x2;
const PLL_REQUESTS: u32 = 1 << 31 | 1 << 27;
const PLL_REQUEST_LANE_0: u32 = 1 << 31;
const PLL_ACKS: u32 = 1 << 30 | 1 << 26;
const PLL_ACK_LANE_0: u32 = 1 << 30;
const REFERENCE_REQUESTS: u32 = 1 << 29 | 1 << 25;
const REFERENCE_REQUEST_LANE_0: u32 = 1 << 29;
const REFERENCE_ACKS: u32 = 1 << 28 | 1 << 24;
const REFERENCE_ACK_LANE_0: u32 = 1 << 28;
const CLOCK_MUX_MASK: u32 = 1 << 8 | 1 << 10 | 0xf << 12 | 0x3;
const FORWARD_CLOCK_UNGATE: u32 = 1 << 10;
const DDI_CLOCK_MAXIMUM_PIXEL_CLOCK: u32 = 0x8 << 12;
const PLL_BYTES: u16 = 0xc00;
const COMMON_BYTES: u16 = 0xc20;
const TRANSMIT_BYTES: u16 = 0xc30;
const VENDOR_CONTROL: u16 = 0xc70;
const CUSTOM_WIDTH: u16 = 0xd02;
const DRIVE_OVERRIDES: u16 = 0xd71;
const DRIVE_LEVELS: u16 = 0xd80;
const TRANSMITTER_CONTROL_2: u16 = 0x402;
const TRANSMITTER_STRIDE: u16 = 0x200;
const BUS_ACCESS: u8 = 1 << 2;
const MASTER_LANE: u8 = 1 << 1;
const UPDATE_CONFIGURATION: u8 = 1;
const CUSTOM_WIDTH_MASK: u8 = 0x3;
const VOLTAGE_BOOST_MASK: u8 = 0x7 << 5;
const TERMINATION_MASK: u8 = 0x7 << 5;
const LEVEL_MASK: u8 = 0x3f;
const OVERRIDE_TRANSMITTERS: u8 = 1 | 1 << 2;
const DISABLE_TRANSMITTER: u8 = 1 << 6;
const SPREAD_BYTES: core::ops::RangeInclusive<usize> = 4..=8;
const TRIES: usize = 3;

/// A C10 PLL configuration: its twenty PLL bytes, its common byte and its transmit byte.
type PllTable = ([u8; 20], u8, u8);

const RBR: PllTable = (
    [
        0xb4, 0, 0x30, 0x1, 0x26, 0x0c, 0x98, 0x46, 0x1, 0x1, 0, 0, 0xc0, 0, 0, 0x2, 0x84, 0x4f,
        0xe5, 0x23,
    ],
    0x21,
    0x10,
);
const HBR: PllTable = (
    [
        0xf4, 0, 0xf8, 0x0, 0x20, 0x0a, 0x29, 0x10, 0x1, 0x1, 0, 0, 0xa0, 0, 0, 0x1, 0x84, 0x4f,
        0xe5, 0x23,
    ],
    0x21,
    0x10,
);
const HBR2: PllTable = (
    [
        0xf4, 0, 0xf8, 0, 0x20, 0x0a, 0x29, 0x10, 0x1, 0x1, 0, 0, 0xa0, 0, 0, 0, 0x84, 0x4f,
        0xe5, 0x23,
    ],
    0x21,
    0x10,
);
const HBR3: PllTable = (
    [
        0x34, 0, 0x84, 0x1, 0x30, 0x0f, 0x3d, 0x98, 0x1, 0x1, 0, 0, 0xf0, 0, 0, 0, 0x84, 0x0f,
        0xe5, 0x23,
    ],
    0x21,
    0x10,
);

/// The swing, pre-cursor and post-cursor of each DisplayPort drive level.
const DRIVE_TABLE: [(u8, u8, u8); 10] = [
    (26, 0, 0),
    (33, 0, 6),
    (38, 0, 11),
    (43, 0, 19),
    (39, 0, 0),
    (45, 0, 7),
    (46, 0, 13),
    (46, 0, 0),
    (55, 0, 7),
    (62, 0, 0),
];

/// The PLL configuration for a DisplayPort link at `link_khz`, without spread spectrum.
pub fn pll_table(link_khz: u32) -> Option<PllTable> {
    let (mut bytes, common, transmit) = match link_khz {
        162_000 => RBR,
        270_000 => HBR,
        540_000 => HBR2,
        810_000 => HBR3,
        _ => return None,
    };
    for at in SPREAD_BYTES {
        bytes[at] = 0;
    }
    Some((bytes, common, transmit))
}

/// Why the PHY did not do as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cx0Error {
    /// A message bus transaction was not acknowledged.
    Bus,
    /// The PHY did not leave or enter reset, change power state, or lock, in time.
    Timeout,
    /// A link rate the tables do not have, or a port without a C10 PHY.
    Unsupported,
}

/// The C10 PHY of port `port` (0 for A) behind `regs`, waiting while `keep_waiting` allows.
pub struct C10<'a, R: RegisterIo + ?Sized> {
    regs: &'a R,
    port: usize,
    keep_waiting: &'a mut dyn FnMut() -> bool,
}

impl<'a, R: RegisterIo + ?Sized> C10<'a, R> {
    pub fn new(
        regs: &'a R,
        port: usize,
        keep_waiting: &'a mut dyn FnMut() -> bool,
    ) -> Result<Self, Cx0Error> {
        if port >= PORTS {
            return Err(Cx0Error::Unsupported);
        }
        Ok(Self {
            regs,
            port,
            keep_waiting,
        })
    }

    fn at(&self, place: usize) -> usize {
        place + self.port * PORT_STRIDE
    }

    fn bus(&self, lane: usize) -> usize {
        self.at(MESSAGE_BUS) + 4 * lane
    }

    fn wait(&mut self, place: usize, mask: u32, wanted: u32) -> Result<u32, Cx0Error> {
        loop {
            let value = self.regs.read32(place);
            if value & mask == wanted {
                return Ok(value);
            }
            if !(self.keep_waiting)() {
                return Err(Cx0Error::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    fn modify_register(&self, place: usize, mask: u32, value: u32) {
        self.regs.write32(place, self.regs.read32(place) & !mask | value);
    }

    fn clear_response(&self, lane: usize) {
        let status = self.bus(lane) + MESSAGE_STATUS;
        self.modify_register(status, 0, RESPONSE_READY | RESPONSE_ERROR);
    }

    fn reset_bus(&mut self, lane: usize) {
        self.regs.write32(self.bus(lane), TRANSACTION_RESET);
        let _ = self.wait(self.bus(lane), TRANSACTION_RESET, 0);
        self.clear_response(lane);
    }

    fn acknowledged(&mut self, lane: usize, command: u32) -> Result<u32, Cx0Error> {
        let status = self.bus(lane) + MESSAGE_STATUS;
        let value = self.wait(status, RESPONSE_READY, RESPONSE_READY)?;
        if value & RESPONSE_ERROR != 0 || (value & COMMAND_MASK) >> COMMAND_SHIFT != command {
            return Err(Cx0Error::Bus);
        }
        Ok(value)
    }

    fn read_once(&mut self, lane: usize, address: u16) -> Result<u8, Cx0Error> {
        self.wait(self.bus(lane), TRANSACTION_PENDING, 0)?;
        self.clear_response(lane);
        let command = TRANSACTION_PENDING | READ | u32::from(address) & ADDRESS_MASK;
        self.regs.write32(self.bus(lane), command);
        let value = self.acknowledged(lane, READ_ACKNOWLEDGED)?;
        self.clear_response(lane);
        self.reset_bus(lane);
        Ok((value >> DATA_SHIFT) as u8)
    }

    fn write_once(
        &mut self,
        lane: usize,
        address: u16,
        data: u8,
        committed: bool,
    ) -> Result<(), Cx0Error> {
        self.wait(self.bus(lane), TRANSACTION_PENDING, 0)?;
        self.clear_response(lane);
        let kind = if committed { WRITE_COMMITTED } else { WRITE_UNCOMMITTED };
        let command = TRANSACTION_PENDING
            | kind
            | u32::from(data) << DATA_SHIFT
            | u32::from(address) & ADDRESS_MASK;
        self.regs.write32(self.bus(lane), command);
        self.wait(self.bus(lane), TRANSACTION_PENDING, 0)?;
        if committed {
            self.acknowledged(lane, WRITE_ACKNOWLEDGED)?;
        } else if self.regs.read32(self.bus(lane) + MESSAGE_STATUS) & RESPONSE_ERROR != 0 {
            return Err(Cx0Error::Bus);
        }
        self.clear_response(lane);
        self.reset_bus(lane);
        Ok(())
    }

    fn read(&mut self, lane: usize, address: u16) -> Result<u8, Cx0Error> {
        let mut last = Err(Cx0Error::Bus);
        for _ in 0..TRIES {
            last = self.read_once(lane, address);
            match last {
                Ok(_) => break,
                Err(_) => self.reset_bus(lane),
            }
        }
        last
    }

    fn write(
        &mut self,
        lane: usize,
        address: u16,
        data: u8,
        committed: bool,
    ) -> Result<(), Cx0Error> {
        let mut last = Err(Cx0Error::Bus);
        for _ in 0..TRIES {
            last = self.write_once(lane, address, data, committed);
            match last {
                Ok(()) => break,
                Err(_) => self.reset_bus(lane),
            }
        }
        last
    }

    fn modify(
        &mut self,
        lanes: &[usize],
        address: u16,
        clear: u8,
        set: u8,
    ) -> Result<(), Cx0Error> {
        for &lane in lanes {
            let old = self.read(lane, address)?;
            let new = old & !clear | set;
            if new != old {
                self.write(lane, address, new, true)?;
            }
        }
        Ok(())
    }

    fn change_power_state(&mut self, state: u32) -> Result<(), Cx0Error> {
        let control = self.at(BUFFER_CONTROL_2);
        self.modify_register(control, POWERDOWN_STATES_MASK, state << 20 | state << 16);
        for lane in 0..2 {
            if self.wait(self.bus(lane), TRANSACTION_PENDING, 0).is_err() {
                self.reset_bus(lane);
            }
        }
        self.modify_register(control, POWERDOWN_UPDATES, POWERDOWN_UPDATES);
        self.wait(control, POWERDOWN_UPDATES, 0).map(|_| ())
    }

    fn reset_lanes(&mut self) -> Result<(), Cx0Error> {
        self.wait(self.at(BUFFER_CONTROL_1), PHY_READY, PHY_READY)?;
        let control = self.at(BUFFER_CONTROL_2);
        self.modify_register(control, PIPE_RESETS, PIPE_RESETS);
        self.wait(control, PHY_STATUSES, PHY_STATUSES)?;
        let clock = self.at(CLOCK_CONTROL);
        self.modify_register(clock, REFERENCE_REQUESTS, REFERENCE_REQUEST_LANE_0);
        self.wait(clock, REFERENCE_ACKS, REFERENCE_ACK_LANE_0)?;
        self.change_power_state(STATE_RESET)?;
        self.modify_register(control, READY_STATE_MASK, STATE_READY << READY_STATE_SHIFT);
        self.modify_register(self.at(BUFFER_CONTROL_3), ACTIVE_AND_STAGGER_MASK, 0);
        self.modify_register(control, PIPE_RESETS, 0);
        self.wait(control, PHY_STATUSES, 0).map(|_| ())
    }

    /// Brings the PHY up for a DisplayPort link at `link_khz` over `lanes` lanes (1, 2 or 4),
    /// its PLL locked.
    pub fn enable(&mut self, link_khz: u32, lanes: usize) -> Result<(), Cx0Error> {
        let (bytes, common, transmit) = pll_table(link_khz).ok_or(Cx0Error::Unsupported)?;
        self.modify_register(self.at(BUFFER_CONTROL_1), PORT_REVERSAL, 0);
        self.modify_register(
            self.at(CLOCK_CONTROL),
            CLOCK_MUX_MASK,
            FORWARD_CLOCK_UNGATE | DDI_CLOCK_MAXIMUM_PIXEL_CLOCK,
        );
        self.reset_lanes()?;
        self.change_power_state(STATE_READY)?;
        for lane in 0..2 {
            let timer = self.at(MESSAGE_TIMER) + 4 * lane;
            self.modify_register(timer, TIMER_VALUE_MASK, TIMER_VALUE);
        }
        self.modify(&[0, 1], VENDOR_CONTROL, 0, BUS_ACCESS)?;
        for (index, byte) in bytes.iter().enumerate() {
            self.write(0, PLL_BYTES + index as u16, *byte, index % 4 == 0)?;
        }
        self.write(0, COMMON_BYTES, common, true)?;
        self.write(0, TRANSMIT_BYTES, transmit, true)?;
        self.modify(&[0, 1], CUSTOM_WIDTH, CUSTOM_WIDTH_MASK, 0)?;
        self.modify(&[0], VENDOR_CONTROL, 0, UPDATE_CONFIGURATION | MASTER_LANE)?;
        self.modify(&[0, 1], VENDOR_CONTROL, 0, BUS_ACCESS)?;
        let disabled = (0xf << lanes) & 0xf;
        for transmitter in 0..4 {
            let lane = transmitter / 2;
            let address =
                TRANSMITTER_CONTROL_2 + (transmitter % 2) as u16 * TRANSMITTER_STRIDE;
            let set = if disabled & 1 << transmitter != 0 { DISABLE_TRANSMITTER } else { 0 };
            self.modify(&[lane], address, DISABLE_TRANSMITTER, set)?;
        }
        self.modify(&[0, 1], VENDOR_CONTROL, 0, UPDATE_CONFIGURATION)?;
        self.regs.write32(self.at(VALID_FREQUENCY), link_khz);
        let clock = self.at(CLOCK_CONTROL);
        self.modify_register(clock, PLL_REQUESTS, PLL_REQUEST_LANE_0);
        self.wait(clock, PLL_ACKS, PLL_ACK_LANE_0).map(|_| ())
    }

    /// Sets the port's width for `lanes` lanes at 10 bits a symbol, before link training.
    pub fn set_width(&self, lanes: usize) {
        let width = match lanes {
            3 => 4,
            lanes => lanes.clamp(1, 4) as u32 - 1,
        };
        self.modify_register(
            self.at(BUFFER_CONTROL_1),
            PORT_WIDTH_MASK | PORT_DATA_WIDTH_MASK,
            width << PORT_WIDTH_SHIFT,
        );
    }

    /// Drives the first `lanes` transmitters at drive level `level` (the Dekel order:
    /// [`super::intel_dekel::drive_level`]) for a link at `link_khz`.
    pub fn set_drive(
        &mut self,
        level: usize,
        link_khz: u32,
        lanes: usize,
    ) -> Result<(), Cx0Error> {
        let (swing, pre_cursor, post_cursor) = DRIVE_TABLE[level.min(DRIVE_TABLE.len() - 1)];
        let strong = matches!(link_khz, 540_000 | 810_000);
        let boost = if strong { 5 } else { 4 };
        let termination = if strong { 5 } else { 2 };
        self.modify(&[0, 1], VENDOR_CONTROL, 0, BUS_ACCESS)?;
        self.modify(&[0, 1], COMMON_BYTES + 3, VOLTAGE_BOOST_MASK, boost << 5)?;
        self.modify(&[0, 1], TRANSMIT_BYTES + 1, TERMINATION_MASK, termination << 5)?;
        for transmitter in 0..lanes.min(4) {
            let lane = transmitter / 2;
            let base = DRIVE_LEVELS + ((lane ^ (transmitter % 2)) as u16) * 0x10;
            for (control, value) in [pre_cursor, swing, post_cursor].into_iter().enumerate() {
                self.modify(&[lane], base + control as u16, LEVEL_MASK, value & LEVEL_MASK)?;
            }
        }
        self.modify(&[0, 1], DRIVE_OVERRIDES, 0, OVERRIDE_TRANSMITTERS)?;
        self.modify(&[0, 1], VENDOR_CONTROL, 0, UPDATE_CONFIGURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pll_tables_drop_their_spread_spectrum_bytes() {
        let (bytes, common, transmit) = pll_table(270_000).unwrap();
        assert_eq!(bytes[0], 0xf4);
        assert_eq!(&bytes[4..=8], &[0; 5]);
        assert_eq!(bytes[15], 0x1);
        assert_eq!((common, transmit), (0x21, 0x10));
        assert_eq!(pll_table(810_000).unwrap().0[17], 0x0f);
        assert_eq!(pll_table(243_000), None);
    }
}

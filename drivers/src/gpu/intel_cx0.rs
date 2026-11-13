//! Display 14's C10 PHY on ports A and B (D527) and C20 PHY on its Type-C ports (D528),
//! Meteor Lake's: the display reaches the PHY's
//! own registers over a message bus, one per PHY lane (a lane serving two transmitters), each
//! read or write a transaction the PHY acknowledges. Bringing a port up is: the port clock
//! muxes set (the forwarded clock ungated, the DDI clocked from the PHY's maximum pixel
//! clock), the PHY lanes reset and taken from reset to their ready power state, the PLL's
//! twenty configuration bytes for the link rate written to lane 0 (Intel's tables for a
//! 38.4 MHz reference, spread spectrum off), the unused transmitters disabled, the DDI's
//! valid frequency set and the PLL requested and acknowledged. Drive levels are presets of
//! swing, pre-cursor and post-cursor overriding each transmitter's. The C20 PHY takes its
//! transmit, common and MPLLB configuration words through an SRAM address and data window,
//! into whichever of its two contexts is not in use, then switches context. Places and values
//! are from Intel's MIT-licensed i915 driver (`INTEL-NOTICE.md`).

use crate::io::RegisterIo;

/// The ports with a C10 PHY: A and B.
pub const PORTS: usize = 2;
/// The DDI index of display 14's first Type-C port, whose PHYs are C20s.
pub const FIRST_TYPE_C_PORT: usize = 3;
/// Display 14's Type-C ports.
pub const TYPE_C_PORTS: usize = 4;

const PORT_STRIDE: usize = 0x100;
const TYPE_C_STRIDE: usize = 0x200;
const BUFFERS: [usize; 2] = [0x64004, 0x16_f200];
const MESSAGE_BUSES: [usize; 2] = [0x64040, 0x16_f240];
const MESSAGE_TIMERS: [usize; 2] = [0x640d8, 0x16_f258];
const CLOCK_CONTROLS: [usize; 2] = [0x640e0, 0x16_f260];
const MESSAGE_STATUS: usize = 8;
const BUFFER_CONTROL_2: usize = 4;
const BUFFER_CONTROL_3: usize = 8;
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
const PIPE_RESET_LANE_0: u32 = 1 << 31;
const PHY_STATUS_LANE_0: u32 = 1 << 29;
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
const SRAM_WRITE_ADDRESS_LOW: u16 = 0xc02;
const SRAM_WRITE_ADDRESS_HIGH: u16 = 0xc03;
const SRAM_WRITE_DATA_LOW: u16 = 0xc04;
const SRAM_WRITE_DATA_HIGH: u16 = 0xc05;
const SERDES_RATE: u16 = 0xd00;
const CONTEXT_TOGGLE: u8 = 1;
const IS_DISPLAY_PORT: u8 = 1 << 6;
const SERDES_RATE_MASK: u8 = 1 << 7 | 1 << 6 | 0xf << 1;
const DISPLAY_PORT_RATE_SHIFT: u8 = 1;
const CONTEXTS: [(u16, u16, u16); 2] = [(0xcf2a, 0xcda5, 0xcb4e), (0xcf2e, 0xcdaa, 0xcb5a)];

type C20Table = ([u16; 3], [u16; 4], [u16; 11]);

const C20_COMMON: [u16; 4] = [0x0500, 0x0005, 0x0000, 0x0000];
const C20_RBR: C20Table = (
    [0xbe88, 0x5800, 0x0000],
    C20_COMMON,
    [
        0x50a8, 0x2120, 0xcd9a, 0xbfc1, 0x5ab8, 0x4c34, 0x2000, 0x0001, 0x6000, 0, 0,
    ],
);
const C20_HBR: C20Table = (
    [0xbe88, 0x4800, 0x0000],
    C20_COMMON,
    [
        0x308c, 0x2110, 0xcc9c, 0xbfc1, 0x4b9a, 0x3f81, 0x2000, 0x0001, 0x5000, 0, 0,
    ],
);
const C20_HBR2: C20Table = (
    [0xbe88, 0x4800, 0x0000],
    C20_COMMON,
    [
        0x108c, 0x2108, 0xcc9c, 0xbfc1, 0x4b9a, 0x3f81, 0x2000, 0x0001, 0x5000, 0, 0,
    ],
);
const C20_HBR3: C20Table = (
    [0xbe88, 0x4800, 0x0000],
    C20_COMMON,
    [
        0x10d2, 0x2108, 0x8d98, 0xbfc1, 0x7166, 0x5f42, 0x2000, 0x0001, 0x7800, 0, 0,
    ],
);

const C20_DRIVE_TABLE: [(u8, u8, u8); 10] = [
    (20, 0, 0),
    (24, 0, 4),
    (30, 0, 9),
    (34, 0, 14),
    (29, 0, 0),
    (34, 0, 5),
    (38, 0, 10),
    (36, 0, 0),
    (40, 0, 6),
    (48, 0, 0),
];

/// The C20 configuration for a DisplayPort link at `link_khz`, and the rate's index in the
/// PHY's serdes rate field.
pub fn c20_table(link_khz: u32) -> Option<(C20Table, u8)> {
    Some(match link_khz {
        162_000 => (C20_RBR, 0),
        270_000 => (C20_HBR, 1),
        540_000 => (C20_HBR2, 2),
        810_000 => (C20_HBR3, 3),
        _ => return None,
    })
}

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
        0xf4, 0, 0xf8, 0, 0x20, 0x0a, 0x29, 0x10, 0x1, 0x1, 0, 0, 0xa0, 0, 0, 0, 0x84, 0x4f, 0xe5,
        0x23,
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

/// The PHY of display 14's port `port` (0 for A, [`FIRST_TYPE_C_PORT`] on for its Type-C
/// ports) behind `regs`, waiting while `keep_waiting` allows; `both_lanes` when the display owns
/// both PHY lanes (a Type-C port with pin assignment D leaves lane 1 to USB).
pub struct Cx0<'a, R: RegisterIo + ?Sized> {
    regs: &'a R,
    port: usize,
    c20: bool,
    both_lanes: bool,
    keep_waiting: &'a mut dyn FnMut() -> bool,
}

impl<R: RegisterIo + ?Sized> core::fmt::Debug for Cx0<'_, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cx0")
            .field("port", &self.port)
            .field("c20", &self.c20)
            .field("both_lanes", &self.both_lanes)
            .finish_non_exhaustive()
    }
}

impl<'a, R: RegisterIo + ?Sized> Cx0<'a, R> {
    pub fn new(
        regs: &'a R,
        port: usize,
        both_lanes: bool,
        keep_waiting: &'a mut dyn FnMut() -> bool,
    ) -> Result<Self, Cx0Error> {
        let type_c = FIRST_TYPE_C_PORT..FIRST_TYPE_C_PORT + TYPE_C_PORTS;
        if port >= PORTS && !type_c.contains(&port) {
            return Err(Cx0Error::Unsupported);
        }
        Ok(Self {
            regs,
            port,
            c20: port >= PORTS,
            both_lanes: both_lanes || port < PORTS,
            keep_waiting,
        })
    }

    fn block(&self, places: [usize; 2]) -> usize {
        match self.c20 {
            false => places[0] + self.port * PORT_STRIDE,
            true => places[1] + (self.port - FIRST_TYPE_C_PORT) * TYPE_C_STRIDE,
        }
    }

    fn buffer(&self, register: usize) -> usize {
        self.block(BUFFERS) + register
    }

    fn bus(&self, lane: usize) -> usize {
        self.block(MESSAGE_BUSES) + 4 * lane
    }

    fn owned(&self) -> &'static [usize] {
        if self.both_lanes { &[0, 1] } else { &[0] }
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
        self.regs
            .write32(place, self.regs.read32(place) & !mask | value);
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
        let kind = if committed {
            WRITE_COMMITTED
        } else {
            WRITE_UNCOMMITTED
        };
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
        let control = self.buffer(BUFFER_CONTROL_2);
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
        self.wait(self.buffer(0), PHY_READY, PHY_READY)?;
        let control = self.buffer(BUFFER_CONTROL_2);
        let (resets, statuses) = match self.both_lanes {
            true => (PIPE_RESETS, PHY_STATUSES),
            false => (PIPE_RESET_LANE_0, PHY_STATUS_LANE_0),
        };
        self.modify_register(control, resets, resets);
        self.wait(control, statuses, statuses)?;
        let clock = self.block(CLOCK_CONTROLS);
        self.modify_register(clock, REFERENCE_REQUESTS, REFERENCE_REQUEST_LANE_0);
        self.wait(clock, REFERENCE_ACKS, REFERENCE_ACK_LANE_0)?;
        self.change_power_state(STATE_RESET)?;
        self.modify_register(control, READY_STATE_MASK, STATE_READY << READY_STATE_SHIFT);
        self.modify_register(self.buffer(BUFFER_CONTROL_3), ACTIVE_AND_STAGGER_MASK, 0);
        self.modify_register(control, resets, 0);
        self.wait(control, statuses, 0).map(|_| ())
    }

    fn write_sram(&mut self, address: u16, data: u16) -> Result<(), Cx0Error> {
        self.write(0, SRAM_WRITE_ADDRESS_HIGH, (address >> 8) as u8, false)?;
        self.write(0, SRAM_WRITE_ADDRESS_LOW, address as u8, false)?;
        self.write(0, SRAM_WRITE_DATA_HIGH, (data >> 8) as u8, false)?;
        self.write(0, SRAM_WRITE_DATA_LOW, data as u8, true)
    }

    fn program_c10(&mut self, link_khz: u32) -> Result<(), Cx0Error> {
        let (bytes, common, transmit) = pll_table(link_khz).ok_or(Cx0Error::Unsupported)?;
        self.modify(&[0, 1], VENDOR_CONTROL, 0, BUS_ACCESS)?;
        for (index, byte) in bytes.iter().enumerate() {
            self.write(0, PLL_BYTES + index as u16, *byte, index % 4 == 0)?;
        }
        self.write(0, COMMON_BYTES, common, true)?;
        self.write(0, TRANSMIT_BYTES, transmit, true)?;
        self.modify(&[0, 1], CUSTOM_WIDTH, CUSTOM_WIDTH_MASK, 0)?;
        self.modify(&[0], VENDOR_CONTROL, 0, UPDATE_CONFIGURATION | MASTER_LANE)
    }

    fn program_c20(&mut self, link_khz: u32) -> Result<(), Cx0Error> {
        let ((transmit, common, pll), rate) = c20_table(link_khz).ok_or(Cx0Error::Unsupported)?;
        let in_use = self.read(0, SERDES_RATE)? & CONTEXT_TOGGLE;
        let (transmit_at, common_at, pll_at) = CONTEXTS[usize::from(in_use)];
        for (index, word) in transmit.iter().enumerate() {
            self.write_sram(transmit_at - index as u16, *word)?;
        }
        for (index, word) in common.iter().enumerate() {
            self.write_sram(common_at - index as u16, *word)?;
        }
        for (index, word) in pll.iter().enumerate() {
            self.write_sram(pll_at - index as u16, *word)?;
        }
        let owned = self.owned();
        self.modify(owned, CUSTOM_WIDTH, CUSTOM_WIDTH_MASK, 0)?;
        let serdes = IS_DISPLAY_PORT | rate << DISPLAY_PORT_RATE_SHIFT;
        self.modify(owned, SERDES_RATE, SERDES_RATE_MASK, serdes)?;
        self.modify(owned, SERDES_RATE, CONTEXT_TOGGLE, CONTEXT_TOGGLE ^ in_use)
    }

    /// Brings the PHY up for a DisplayPort link at `link_khz` over `lanes` lanes (1, 2 or 4),
    /// its PLL locked.
    pub fn enable(&mut self, link_khz: u32, lanes: usize) -> Result<(), Cx0Error> {
        self.modify_register(self.buffer(0), PORT_REVERSAL, 0);
        self.modify_register(
            self.block(CLOCK_CONTROLS),
            CLOCK_MUX_MASK,
            FORWARD_CLOCK_UNGATE | DDI_CLOCK_MAXIMUM_PIXEL_CLOCK,
        );
        self.reset_lanes()?;
        self.change_power_state(STATE_READY)?;
        for lane in 0..2 {
            let timer = self.block(MESSAGE_TIMERS) + 4 * lane;
            self.modify_register(timer, TIMER_VALUE_MASK, TIMER_VALUE);
        }
        match self.c20 {
            false => self.program_c10(link_khz)?,
            true => self.program_c20(link_khz)?,
        }
        let owned = self.owned();
        if !self.c20 {
            self.modify(owned, VENDOR_CONTROL, 0, BUS_ACCESS)?;
        }
        let mut disabled = (0xf << lanes) & 0xf;
        if self.c20 && lanes == 1 {
            disabled = disabled & !0x3 | 0x1;
        }
        for transmitter in 0..4 {
            let lane = transmitter / 2;
            if !owned.contains(&lane) {
                continue;
            }
            let address = TRANSMITTER_CONTROL_2 + (transmitter % 2) as u16 * TRANSMITTER_STRIDE;
            let set = if disabled & 1 << transmitter != 0 {
                DISABLE_TRANSMITTER
            } else {
                0
            };
            self.modify(&[lane], address, DISABLE_TRANSMITTER, set)?;
        }
        if !self.c20 {
            self.modify(owned, VENDOR_CONTROL, 0, UPDATE_CONFIGURATION)?;
        }
        self.regs
            .write32(VALID_FREQUENCY + self.port * PORT_STRIDE, link_khz);
        let clock = self.block(CLOCK_CONTROLS);
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
            self.buffer(0),
            PORT_WIDTH_MASK | PORT_DATA_WIDTH_MASK,
            width << PORT_WIDTH_SHIFT,
        );
    }

    /// Drives the first `lanes` transmitters at drive level `level` (the Dekel order:
    /// [`super::intel_dekel::drive_level`]) for a link at `link_khz`.
    pub fn set_drive(&mut self, level: usize, link_khz: u32, lanes: usize) -> Result<(), Cx0Error> {
        let table = if self.c20 {
            &C20_DRIVE_TABLE
        } else {
            &DRIVE_TABLE
        };
        let (swing, pre_cursor, post_cursor) = table[level.min(table.len() - 1)];
        let owned = self.owned();
        if !self.c20 {
            let strong = matches!(link_khz, 540_000 | 810_000);
            let boost = if strong { 5 } else { 4 };
            let termination = if strong { 5 } else { 2 };
            self.modify(owned, VENDOR_CONTROL, 0, BUS_ACCESS)?;
            self.modify(owned, COMMON_BYTES + 3, VOLTAGE_BOOST_MASK, boost << 5)?;
            self.modify(
                owned,
                TRANSMIT_BYTES + 1,
                TERMINATION_MASK,
                termination << 5,
            )?;
        }
        for transmitter in 0..lanes.min(4) {
            let lane = transmitter / 2;
            if !owned.contains(&lane) {
                continue;
            }
            let base = DRIVE_LEVELS + ((lane ^ (transmitter % 2)) as u16) * 0x10;
            for (control, value) in [pre_cursor, swing, post_cursor].into_iter().enumerate() {
                self.modify(
                    &[lane],
                    base + control as u16,
                    LEVEL_MASK,
                    value & LEVEL_MASK,
                )?;
            }
        }
        self.modify(owned, DRIVE_OVERRIDES, 0, OVERRIDE_TRANSMITTERS)?;
        match self.c20 {
            false => self.modify(owned, VENDOR_CONTROL, 0, UPDATE_CONFIGURATION),
            true => Ok(()),
        }
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

    #[test]
    fn c20_tables_name_their_serdes_rate() {
        let ((transmit, _, pll), rate) = c20_table(540_000).unwrap();
        assert_eq!(rate, 2);
        assert_eq!(transmit[0] & 1 << 7, 1 << 7);
        assert_eq!(pll[0], 0x108c);
        assert_eq!(c20_table(1_000_000), None);
    }
}

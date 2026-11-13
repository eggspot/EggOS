//! The eXtensible Host Controller Interface (xHCI 1.2; D239): the controller taken from the
//! firmware and reset, its command and event rings, and every HID boot keyboard and mouse on its
//! ports addressed, configured and polled through its interrupt endpoint, through up to five tiers
//! of USB 2.0 and 3.x hubs (D254), and every USB flash drive read and written through its
//! bulk endpoints with SCSI commands (D264).

use super::{
    BootDevice, BootInterface, DESCRIPTOR_CONFIGURATION, DESCRIPTOR_DEVICE, Setup,
    StorageInterface, boot_interface, command_block, scsi_read_write_10, status_passed,
    storage_interface,
};
use crate::dma::DmaMemory;
use crate::input::hid::{BootKeyboard, BootMouse, lights_report};
use crate::input::hid_report::{Decoder, Layout};
use crate::input::{Capabilities, LockLights, RawEvent};
use crate::io::RegisterIo;
use crate::storage::{
    Covering, SECTOR_SIZE, copy_in, copy_out, covering, read32, write32, write64, zero,
};
use core::sync::atomic::{Ordering, fence};

const CAPABILITY_LENGTH: usize = 0x00;
const STRUCTURAL_PARAMETERS_1: usize = 0x04;
const STRUCTURAL_PARAMETERS_2: usize = 0x08;
const CAPABILITY_PARAMETERS_1: usize = 0x10;
const DOORBELL_OFFSET: usize = 0x14;
const RUNTIME_OFFSET: usize = 0x18;

const USB_COMMAND: usize = 0x00;
const USB_STATUS: usize = 0x04;
const COMMAND_RING_CONTROL: usize = 0x18;
const DEVICE_CONTEXT_ARRAY: usize = 0x30;
const CONFIGURE: usize = 0x38;
const PORTS: usize = 0x400;
const PORT_STRIDE: usize = 0x10;

const COMMAND_RUN: u32 = 1;
const COMMAND_RESET: u32 = 1 << 1;
const STATUS_HALTED: u32 = 1;
const STATUS_NOT_READY: u32 = 1 << 11;

const PORT_CONNECTED: u32 = 1;
const PORT_ENABLED: u32 = 1 << 1;
const PORT_RESET: u32 = 1 << 4;
const PORT_POWER: u32 = 1 << 9;
const PORT_RESET_CHANGE: u32 = 1 << 21;
const PORT_CHANGE_BITS: u32 = 0x00fe_0000;
const PORT_WRITE_PRESERVE: u32 = !(PORT_CHANGE_BITS | PORT_ENABLED);

const INTERRUPTER: usize = 0x20;
const INTERRUPTER_MANAGEMENT: usize = INTERRUPTER;
const INTERRUPT_PENDING: u32 = 1;
const INTERRUPT_ENABLE: u32 = 1 << 1;
const COMMAND_INTERRUPTS: u32 = 1 << 2;
const STATUS_EVENT_INTERRUPT: u32 = 1 << 3;
const INTERRUPTER_TABLE_SIZE: usize = INTERRUPTER + 0x08;
const INTERRUPTER_TABLE_BASE: usize = INTERRUPTER + 0x10;
const INTERRUPTER_DEQUEUE: usize = INTERRUPTER + 0x18;
const EVENT_HANDLER_BUSY: u64 = 1 << 3;

const LEGACY_SUPPORT: u32 = 1;
const LEGACY_BIOS_OWNED: u32 = 1 << 16;
const LEGACY_OS_OWNED: u32 = 1 << 24;

const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_EVALUATE_CONTEXT: u32 = 13;
const TRB_RESET_ENDPOINT: u32 = 14;
const TRB_SET_DEQUEUE: u32 = 16;
const TRB_TRANSFER_EVENT: u32 = 32;
const TRB_COMMAND_COMPLETION: u32 = 33;
const TRB_CYCLE: u32 = 1;
const TRB_TOGGLE_CYCLE: u32 = 1 << 1;
const TRB_INTERRUPT_ON_SHORT: u32 = 1 << 2;
const TRB_INTERRUPT_ON_COMPLETION: u32 = 1 << 5;
const TRB_IMMEDIATE_DATA: u32 = 1 << 6;
const TRB_DIRECTION_IN: u32 = 1 << 16;
const TRANSFER_TYPE_IN: u32 = 3 << 16;
const TRANSFER_TYPE_OUT: u32 = 2 << 16;
const COMPLETION_SUCCESS: u32 = 1;
const COMPLETION_STALL: u32 = 6;
const COMPLETION_SHORT_PACKET: u32 = 13;

const ENDPOINT_CONTROL: u32 = 4;
const ENDPOINT_INTERRUPT_IN: u32 = 7;
const SPEED_FULL: u32 = 1;
const SPEED_LOW: u32 = 2;
const SPEED_HIGH: u32 = 3;

const TRB_BYTES: usize = 16;
const RING_TRBS: usize = 64;
const EVENT_TRBS: usize = 256;
pub const MAX_DEVICES: usize = 16;
const MAX_SCRATCHPAD_PAGES: usize = 60;

const DEVICE_CONTEXT_ARRAY_AT: usize = 0x0000;
const COMMAND_RING_AT: usize = 0x1000;
const EVENT_RING_AT: usize = 0x2000;
const EVENT_TABLE_AT: usize = 0x3000;
const SCRATCHPAD_ARRAY_AT: usize = 0x3100;
const SCRATCHPAD_PAGES_AT: usize = 0x4000;
const DEVICES_AT: usize = 0x4_0000;
const DEVICE_BYTES: usize = 0x4000;
const OUTPUT_CONTEXT: usize = 0x0000;
const INPUT_CONTEXT: usize = 0x1000;
const CONTROL_RING: usize = 0x2000;
const REPORT_RING: usize = 0x2400;
const DESCRIPTOR_BUFFER: usize = 0x2800;
const DESCRIPTOR_BUFFER_BYTES: usize = 0x400;
const REPORT_BUFFER: usize = 0x2c00;
const LIGHTS_BUFFER: usize = 0x2c40;
const BULK_OUT_RING: usize = 0x3000;
const STORAGE_BUFFER: usize = DEVICES_AT + MAX_DEVICES * DEVICE_BYTES;
const STORAGE_BUFFER_BYTES: usize = 0x1_0000;
pub const MEMORY_BYTES: usize = STORAGE_BUFFER + STORAGE_BUFFER_BYTES;
const ENDPOINT_BULK_OUT: u32 = 2;
const ENDPOINT_BULK_IN: u32 = 6;
const PENDING_EVENTS: usize = 64;
const SCSI_TEST_UNIT_READY: [u8; 6] = [0; 6];
const SCSI_READ_CAPACITY_10: [u8; 10] = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const SCSI_SYNCHRONIZE_CACHE_10: [u8; 10] = [0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const UNIT_READY_TRIES: usize = 20;
const REPORT_BYTES: usize = 64;

const _: () = assert!(SCRATCHPAD_PAGES_AT + MAX_SCRATCHPAD_PAGES * 4096 <= DEVICES_AT);
const _: () = assert!(EVENT_TRBS * TRB_BYTES <= 0x1000);

/// Why the controller or a device could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TimedOut,
    ControllerFailed,
    CommandFailed,
    TransferFailed,
    Unsupported(&'static str),
}

impl Error {
    pub fn description(self) -> &'static str {
        match self {
            Error::TimedOut => "the USB controller did not answer in time",
            Error::ControllerFailed => "the USB controller stopped",
            Error::CommandFailed => "the USB controller refused a command",
            Error::TransferFailed => "a USB device refused a transfer",
            Error::Unsupported(reason) => reason,
        }
    }
}

/// The xHCI `Interval` field for an interrupt endpoint's `bInterval` at `speed`.
pub fn interval_field(speed: u32, interval: u8) -> u32 {
    match speed {
        SPEED_LOW | SPEED_FULL => {
            let microframes = u32::from(interval.max(1)) * 8;
            (31 - microframes.leading_zeros()).clamp(3, 10)
        }
        _ => u32::from(interval.clamp(1, 16)) - 1,
    }
}

/// The default control endpoint's maximum packet size for a device at `speed`.
pub fn default_max_packet(speed: u32) -> u32 {
    match speed {
        SPEED_LOW | SPEED_FULL => 8,
        SPEED_HIGH => 64,
        _ => 512,
    }
}

#[derive(Debug, Clone, Copy)]
struct Ring {
    at: usize,
    trbs: usize,
    index: usize,
    cycle: bool,
}

impl Ring {
    const fn new(at: usize, trbs: usize) -> Self {
        Self {
            at,
            trbs,
            index: 0,
            cycle: true,
        }
    }
}

enum DiskBuffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

impl DiskBuffer<'_> {
    fn len(&self) -> usize {
        match self {
            DiskBuffer::Read(buffer) => buffer.len(),
            DiskBuffer::Write(buffer) => buffer.len(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)]
enum Handler {
    Keyboard(BootKeyboard),
    Mouse(BootMouse),
    Hub {
        ports: u8,
    },
    Report(Layout, Decoder),
    Storage {
        interface: StorageInterface,
        block_bytes: u32,
        blocks: u64,
        tag: u32,
    },
}

/// Where a device is on the bus: its root port, its route through hubs (four bits a tier), its
/// speed, the high-speed hub whose transaction translator serves it when it is a low- or
/// full-speed device behind one, and how many hubs are above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub root_port: usize,
    pub route: u32,
    pub speed: u32,
    pub translator: Option<(u8, u8)>,
    pub depth: u8,
}

const DEVICE_CLASS_HUB: u8 = 9;
const HUB_DESCRIPTOR_USB2: u8 = 0x29;
const HUB_DESCRIPTOR_USB3: u8 = 0x2a;
const PORT_FEATURE_RESET: u16 = 4;
const PORT_FEATURE_POWER: u16 = 8;
const PORT_FEATURE_RESET_CHANGE: u16 = 20;
const HUB_PORT_CONNECTED: u16 = 1;
const HUB_PORT_LOW_SPEED: u16 = 1 << 9;
const HUB_PORT_HIGH_SPEED: u16 = 1 << 10;
const HUB_CHANGE_RESET: u16 = 1 << 4;
const MAX_HUB_DEPTH: u8 = 5;
const HUB_STATUS_POLLS: usize = 100;
const SLOT_HUB: u32 = 1 << 26;
const SPEED_SUPER: u32 = 4;

/// The speed a USB 2.0 hub's port status gives its device.
pub fn hub_port_speed(status: u16, super_speed_hub: bool) -> u32 {
    match (
        super_speed_hub,
        status & HUB_PORT_LOW_SPEED != 0,
        status & HUB_PORT_HIGH_SPEED != 0,
    ) {
        (true, _, _) => SPEED_SUPER,
        (false, true, _) => SPEED_LOW,
        (false, false, true) => SPEED_HIGH,
        _ => SPEED_FULL,
    }
}

/// Where a device on `port` of the hub at `hub` (in slot `hub_slot`) is.
pub fn child_location(hub: &Location, hub_slot: u8, port: u8, speed: u32) -> Location {
    let translator = match (hub.speed, speed) {
        (SPEED_HIGH, SPEED_LOW | SPEED_FULL) => Some((hub_slot, port)),
        _ => hub.translator,
    };
    Location {
        root_port: hub.root_port,
        route: hub.route | (u32::from(port.min(15)) << (4 * u32::from(hub.depth))),
        speed,
        translator,
        depth: hub.depth + 1,
    }
}

#[derive(Debug, Clone, Copy)]
struct Device {
    slot: u8,
    area: usize,
    interface: BootInterface,
    report_endpoint: u32,
    control: Ring,
    reports: Ring,
    handler: Handler,
    location: Location,
    bulk_out: Ring,
}

/// An xHCI controller and the HID boot devices on its root ports.
#[derive(Debug)]
pub struct Xhci<R: RegisterIo, M: DmaMemory> {
    regs: R,
    memory: M,
    operational: usize,
    runtime: usize,
    doorbells: usize,
    ports: usize,
    context_bytes: usize,
    commands: Ring,
    events: Ring,
    devices: [Option<Device>; MAX_DEVICES],
    lights: Option<LockLights>,
    pending: [RawEvent; PENDING_EVENTS],
    pending_count: usize,
}

impl<R: RegisterIo, M: DmaMemory> Xhci<R, M> {
    /// Takes the controller at `regs` from the firmware, resets it and starts it with its rings
    /// in `memory` ([`MEMORY_BYTES`]). On failure `memory` is handed back only when the
    /// controller is halted.
    pub fn new(
        regs: R,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((
                Error::Unsupported("too little DMA memory for a USB controller"),
                Some(memory),
            ));
        }
        let operational = usize::from(regs.read8(CAPABILITY_LENGTH));
        let parameters = regs.read32(CAPABILITY_PARAMETERS_1);
        let addresses_64_bit = parameters & 1 != 0;
        let end = memory.dma_addr().get().saturating_add(memory.len() as u64);
        if !addresses_64_bit && end > u64::from(u32::MAX) {
            return Err((
                Error::Unsupported("the USB controller reaches only 32-bit addresses"),
                Some(memory),
            ));
        }
        let structure = regs.read32(STRUCTURAL_PARAMETERS_1);
        let mut controller = Self {
            operational,
            runtime: (regs.read32(RUNTIME_OFFSET) & !0x1f) as usize,
            doorbells: (regs.read32(DOORBELL_OFFSET) & !0x3) as usize,
            ports: ((structure >> 24) & 0xff) as usize,
            context_bytes: if parameters & (1 << 2) != 0 { 64 } else { 32 },
            commands: Ring::new(COMMAND_RING_AT, RING_TRBS),
            events: Ring::new(EVENT_RING_AT, EVENT_TRBS),
            devices: [None; MAX_DEVICES],
            lights: None,
            pending: [RawEvent::default(); PENDING_EVENTS],
            pending_count: 0,
            regs,
            memory,
        };
        let extended = ((parameters >> 16) & 0xffff) as usize * 4;
        controller.take_from_firmware(extended, keep_waiting);
        match controller.start(structure, keep_waiting) {
            Ok(()) => Ok(controller),
            Err(error) => {
                let halted = controller.halt(keep_waiting);
                Err((error, halted.then_some(controller.memory)))
            }
        }
    }

    fn op(&self, offset: usize) -> usize {
        self.operational + offset
    }

    fn port(&self, port: usize) -> usize {
        self.op(PORTS + (port - 1) * PORT_STRIDE)
    }

    fn write64_register(&self, offset: usize, value: u64) {
        self.regs.write32(offset, value as u32);
        self.regs.write32(offset + 4, (value >> 32) as u32);
    }

    fn take_from_firmware(&self, mut at: usize, keep_waiting: &mut dyn FnMut() -> bool) {
        for _ in 0..64 {
            if at == 0 {
                return;
            }
            let capability = self.regs.read32(at);
            if capability & 0xff == LEGACY_SUPPORT {
                self.regs.write32(at, capability | LEGACY_OS_OWNED);
                while self.regs.read32(at) & LEGACY_BIOS_OWNED != 0 && keep_waiting() {
                    core::hint::spin_loop();
                }
                let control = self.regs.read32(at + 4);
                self.regs.write32(at + 4, control & 0xffff_0000 & !0x1f);
                return;
            }
            let next = ((capability >> 8) & 0xff) as usize * 4;
            at = if next == 0 { 0 } else { at + next };
        }
    }

    fn halt(&self, keep_waiting: &mut dyn FnMut() -> bool) -> bool {
        let command = self.regs.read32(self.op(USB_COMMAND));
        self.regs
            .write32(self.op(USB_COMMAND), command & !COMMAND_RUN);
        loop {
            if self.regs.read32(self.op(USB_STATUS)) & STATUS_HALTED != 0 {
                return true;
            }
            if !keep_waiting() {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    fn start(
        &mut self,
        structure: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if !self.halt(keep_waiting) {
            return Err(Error::TimedOut);
        }
        self.regs.write32(self.op(USB_COMMAND), COMMAND_RESET);
        loop {
            let resetting = self.regs.read32(self.op(USB_COMMAND)) & COMMAND_RESET != 0;
            let not_ready = self.regs.read32(self.op(USB_STATUS)) & STATUS_NOT_READY != 0;
            if !resetting && !not_ready {
                break;
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
        zero(&self.memory, 0, DEVICES_AT);
        let slots = (structure & 0xff).min(MAX_DEVICES as u32);
        self.regs.write32(self.op(CONFIGURE), slots);
        let base = self.memory.dma_addr().get();
        let parameters_2 = self.regs.read32(STRUCTURAL_PARAMETERS_2);
        let scratchpads = (((parameters_2 >> 21) & 0x1f) << 5 | (parameters_2 >> 27)) as usize;
        if scratchpads > MAX_SCRATCHPAD_PAGES {
            return Err(Error::Unsupported(
                "the USB controller wants more scratchpad memory than given",
            ));
        }
        if scratchpads > 0 {
            for page in 0..scratchpads {
                write64(
                    &self.memory,
                    SCRATCHPAD_ARRAY_AT + page * 8,
                    base + (SCRATCHPAD_PAGES_AT + page * 4096) as u64,
                );
            }
            write64(
                &self.memory,
                DEVICE_CONTEXT_ARRAY_AT,
                base + SCRATCHPAD_ARRAY_AT as u64,
            );
        }
        self.write64_register(
            self.op(DEVICE_CONTEXT_ARRAY),
            base + DEVICE_CONTEXT_ARRAY_AT as u64,
        );
        self.write64_register(
            self.op(COMMAND_RING_CONTROL),
            (base + COMMAND_RING_AT as u64) | 1,
        );
        write64(&self.memory, EVENT_TABLE_AT, base + EVENT_RING_AT as u64);
        write32(&self.memory, EVENT_TABLE_AT + 8, EVENT_TRBS as u32);
        self.regs.write32(self.runtime + INTERRUPTER_TABLE_SIZE, 1);
        self.write64_register(
            self.runtime + INTERRUPTER_DEQUEUE,
            base + EVENT_RING_AT as u64,
        );
        self.write64_register(
            self.runtime + INTERRUPTER_TABLE_BASE,
            base + EVENT_TABLE_AT as u64,
        );
        self.regs.write32(self.op(USB_COMMAND), COMMAND_RUN);
        loop {
            if self.regs.read32(self.op(USB_STATUS)) & STATUS_HALTED == 0 {
                break;
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
        for port in 1..=self.ports {
            let status = self.regs.read32(self.port(port));
            if status & PORT_POWER == 0 {
                self.regs
                    .write32(self.port(port), (status & PORT_WRITE_PRESERVE) | PORT_POWER);
            }
        }
        Ok(())
    }

    fn enqueue(&mut self, ring: &mut Ring, parameter: u64, status: u32, control: u32) -> u64 {
        let base = self.memory.dma_addr().get();
        let at = ring.at + ring.index * TRB_BYTES;
        write64(&self.memory, at, parameter);
        write32(&self.memory, at + 8, status);
        fence(Ordering::SeqCst);
        write32(&self.memory, at + 12, control | u32::from(ring.cycle));
        let address = base + at as u64;
        ring.index += 1;
        if ring.index == ring.trbs - 1 {
            let link = ring.at + ring.index * TRB_BYTES;
            write64(&self.memory, link, base + ring.at as u64);
            write32(&self.memory, link + 8, 0);
            fence(Ordering::SeqCst);
            write32(
                &self.memory,
                link + 12,
                (TRB_LINK << 10) | TRB_TOGGLE_CYCLE | u32::from(ring.cycle),
            );
            ring.index = 0;
            ring.cycle = !ring.cycle;
        }
        address
    }

    fn ring_doorbell(&self, slot: u8, target: u32) {
        fence(Ordering::SeqCst);
        self.regs
            .write32(self.doorbells + usize::from(slot) * 4, target);
    }

    fn next_event(&mut self) -> Option<[u32; 4]> {
        let at = self.events.at + self.events.index * TRB_BYTES;
        let control = read32(&self.memory, at + 12);
        if (control & TRB_CYCLE == 1) != self.events.cycle {
            return None;
        }
        fence(Ordering::SeqCst);
        let event = [
            read32(&self.memory, at),
            read32(&self.memory, at + 4),
            read32(&self.memory, at + 8),
            control,
        ];
        self.events.index += 1;
        if self.events.index == self.events.trbs {
            self.events.index = 0;
            self.events.cycle = !self.events.cycle;
        }
        let next =
            self.memory.dma_addr().get() + (self.events.at + self.events.index * TRB_BYTES) as u64;
        self.write64_register(
            self.runtime + INTERRUPTER_DEQUEUE,
            next | EVENT_HANDLER_BUSY,
        );
        Some(event)
    }

    fn wait_for(
        &mut self,
        kind: u32,
        trb: u64,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<[u32; 4], Error> {
        loop {
            while let Some(event) = self.next_event() {
                let event_kind = (event[3] >> 10) & 0x3f;
                let pointer = u64::from(event[0]) | (u64::from(event[1]) << 32);
                if event_kind == kind && pointer == trb {
                    return Ok(event);
                }
                if event_kind == TRB_TRANSFER_EVENT {
                    let mut collected = [RawEvent::default(); 16];
                    let mut count = 0;
                    self.handle_transfer(event, &mut |raw| {
                        if count < collected.len() {
                            collected[count] = raw;
                            count += 1;
                        }
                    });
                    for raw in &collected[..count] {
                        if self.pending_count < PENDING_EVENTS {
                            self.pending[self.pending_count] = *raw;
                            self.pending_count += 1;
                        }
                    }
                }
            }
            if self.regs.read32(self.op(USB_STATUS)) & STATUS_HALTED != 0 {
                return Err(Error::ControllerFailed);
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
            core::hint::spin_loop();
        }
    }

    fn command(
        &mut self,
        parameter: u64,
        control: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<u8, Error> {
        let mut ring = self.commands;
        let trb = self.enqueue(&mut ring, parameter, 0, control);
        self.commands = ring;
        self.ring_doorbell(0, 0);
        let event = self.wait_for(TRB_COMMAND_COMPLETION, trb, keep_waiting)?;
        if event[2] >> 24 != COMPLETION_SUCCESS {
            return Err(Error::CommandFailed);
        }
        Ok((event[3] >> 24) as u8)
    }

    fn input_context(&self, area: usize, index: usize) -> usize {
        area + INPUT_CONTEXT + index * self.context_bytes
    }

    fn control_transfer(
        &mut self,
        device: &mut Device,
        setup: Setup,
        data: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        let base = self.memory.dma_addr().get();
        let length = u32::from(setup.length);
        let transfer_type = match (length, setup.is_in()) {
            (0, _) => 0,
            (_, true) => TRANSFER_TYPE_IN,
            (_, false) => TRANSFER_TYPE_OUT,
        };
        let mut ring = device.control;
        self.enqueue(
            &mut ring,
            setup.as_u64(),
            8,
            (TRB_SETUP << 10) | TRB_IMMEDIATE_DATA | transfer_type,
        );
        if length > 0 {
            let direction = if setup.is_in() { TRB_DIRECTION_IN } else { 0 };
            self.enqueue(
                &mut ring,
                base + (device.area + data) as u64,
                length,
                (TRB_DATA << 10) | direction,
            );
        }
        let status_direction = if length > 0 && setup.is_in() {
            0
        } else {
            TRB_DIRECTION_IN
        };
        let status = self.enqueue(
            &mut ring,
            0,
            0,
            (TRB_STATUS << 10) | status_direction | TRB_INTERRUPT_ON_COMPLETION,
        );
        device.control = ring;
        self.ring_doorbell(device.slot, 1);
        let event = self.wait_for(TRB_TRANSFER_EVENT, status, keep_waiting)?;
        let code = event[2] >> 24;
        if code == COMPLETION_STALL {
            self.recover_control(device, keep_waiting)?;
            return Err(Error::TransferFailed);
        }
        if code != COMPLETION_SUCCESS && code != COMPLETION_SHORT_PACKET {
            return Err(Error::TransferFailed);
        }
        Ok(length as usize)
    }

    fn recover_control(
        &mut self,
        device: &mut Device,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let slot = u32::from(device.slot) << 24;
        self.command(
            0,
            (TRB_RESET_ENDPOINT << 10) | (1 << 16) | slot,
            keep_waiting,
        )?;
        let base = self.memory.dma_addr().get();
        let dequeue = (base + (device.control.at + device.control.index * TRB_BYTES) as u64)
            | u64::from(device.control.cycle);
        self.command(
            dequeue,
            (TRB_SET_DEQUEUE << 10) | (1 << 16) | slot,
            keep_waiting,
        )?;
        Ok(())
    }

    /// Waits for the root ports' devices to show up while `settle` keeps waiting, then
    /// addresses and configures every HID boot keyboard and mouse found on them: how many.
    pub fn attach_devices(
        &mut self,
        settle: &mut dyn FnMut() -> bool,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> usize {
        while settle() {
            core::hint::spin_loop();
        }
        let mut attached = 0;
        for port in 1..=self.ports {
            let Ok(speed) = self.reset_port(port, keep_waiting) else {
                continue;
            };
            let location = Location {
                root_port: port,
                route: 0,
                speed,
                translator: None,
                depth: 0,
            };
            attached += self.attach_at(location, keep_waiting);
        }
        attached
    }

    fn attach_at(&mut self, location: Location, keep_waiting: &mut dyn FnMut() -> bool) -> usize {
        let Some(index) = self.devices.iter().position(Option::is_none) else {
            return 0;
        };
        let Ok(device) = self.attach(location, index, keep_waiting) else {
            return 0;
        };
        self.devices[index] = Some(device);
        let Handler::Hub { ports } = device.handler else {
            return 1;
        };
        if location.depth >= MAX_HUB_DEPTH {
            return 0;
        }
        let mut attached = 0;
        for port in 1..=ports {
            let Some(mut hub) = self.devices[index] else {
                break;
            };
            let speed = self.reset_hub_port(&mut hub, port, keep_waiting);
            self.devices[index] = Some(hub);
            if let Some(speed) = speed {
                attached += self.attach_at(
                    child_location(&location, hub.slot, port, speed),
                    keep_waiting,
                );
            }
        }
        attached
    }

    fn hub_request(
        &mut self,
        hub: &mut Device,
        request: u8,
        feature: u16,
        port: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let setup = Setup {
            request_type: 0x23,
            request,
            value: feature,
            index: u16::from(port),
            length: 0,
        };
        self.control_transfer(hub, setup, 0, keep_waiting)
            .map(|_| ())
    }

    fn hub_port_status(
        &mut self,
        hub: &mut Device,
        port: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Option<(u16, u16)> {
        let setup = Setup {
            request_type: 0xa3,
            request: 0,
            value: 0,
            index: u16::from(port),
            length: 4,
        };
        self.control_transfer(hub, setup, DESCRIPTOR_BUFFER, keep_waiting)
            .ok()?;
        let mut status = [0u8; 4];
        copy_out(&self.memory, hub.area + DESCRIPTOR_BUFFER, &mut status);
        Some((
            u16::from_le_bytes([status[0], status[1]]),
            u16::from_le_bytes([status[2], status[3]]),
        ))
    }

    fn reset_hub_port(
        &mut self,
        hub: &mut Device,
        port: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Option<u32> {
        let connected = (0..HUB_STATUS_POLLS).find_map(|_| {
            let (status, _) = self.hub_port_status(hub, port, keep_waiting)?;
            (status & HUB_PORT_CONNECTED != 0).then_some(())
        });
        connected?;
        self.hub_request(hub, 3, PORT_FEATURE_RESET, port, keep_waiting)
            .ok()?;
        let status = (0..HUB_STATUS_POLLS).find_map(|_| {
            let (status, change) = self.hub_port_status(hub, port, keep_waiting)?;
            (change & HUB_CHANGE_RESET != 0).then_some(status)
        })?;
        self.hub_request(hub, 1, PORT_FEATURE_RESET_CHANGE, port, keep_waiting)
            .ok()?;
        Some(hub_port_speed(status, hub.location.speed >= SPEED_SUPER))
    }

    fn write_slot_context(
        &self,
        area: usize,
        entries: u32,
        location: &Location,
        hub_ports: Option<u8>,
    ) {
        let slot_context = self.input_context(area, 1);
        let hub = if hub_ports.is_some() { SLOT_HUB } else { 0 };
        write32(
            &self.memory,
            slot_context,
            (entries << 27) | (location.speed << 20) | (location.route & 0xf_ffff) | hub,
        );
        write32(
            &self.memory,
            slot_context + 4,
            ((location.root_port as u32) << 16) | (u32::from(hub_ports.unwrap_or(0)) << 24),
        );
        let (translator_slot, translator_port) = location.translator.unwrap_or((0, 0));
        write32(
            &self.memory,
            slot_context + 8,
            u32::from(translator_slot) | (u32::from(translator_port) << 8),
        );
    }

    fn reset_port(
        &self,
        port: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<u32, Error> {
        let status = self.regs.read32(self.port(port));
        if status & PORT_CONNECTED == 0 {
            return Err(Error::Unsupported("nothing is connected"));
        }
        if status & PORT_ENABLED == 0 {
            self.regs
                .write32(self.port(port), (status & PORT_WRITE_PRESERVE) | PORT_RESET);
            loop {
                let status = self.regs.read32(self.port(port));
                if status & PORT_RESET_CHANGE != 0 {
                    self.regs.write32(
                        self.port(port),
                        (status & PORT_WRITE_PRESERVE) | PORT_RESET_CHANGE,
                    );
                    break;
                }
                if !keep_waiting() {
                    return Err(Error::TimedOut);
                }
                core::hint::spin_loop();
            }
        }
        let status = self.regs.read32(self.port(port));
        if status & PORT_ENABLED == 0 {
            return Err(Error::Unsupported("the port did not enable"));
        }
        Ok((status >> 10) & 0xf)
    }

    fn attach(
        &mut self,
        location: Location,
        index: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Device, Error> {
        let speed = location.speed;
        let slot = self.command(0, TRB_ENABLE_SLOT << 10, keep_waiting)?;
        if slot == 0 || usize::from(slot) > MAX_DEVICES {
            return Err(Error::Unsupported(
                "the controller gave a slot past its configured ones",
            ));
        }
        let base = self.memory.dma_addr().get();
        let area = DEVICES_AT + index * DEVICE_BYTES;
        zero(&self.memory, area, DEVICE_BYTES);
        let mut device = Device {
            slot,
            area,
            interface: BootInterface {
                configuration: 0,
                interface: 0,
                kind: BootDevice::Keyboard,
                endpoint: 0,
                max_packet: 0,
                interval: 0,
                report_length: 0,
            },
            report_endpoint: 0,
            control: Ring::new(area + CONTROL_RING, RING_TRBS),
            reports: Ring::new(area + REPORT_RING, RING_TRBS),
            handler: Handler::Keyboard(BootKeyboard::default()),
            location,
            bulk_out: Ring::new(area + BULK_OUT_RING, RING_TRBS),
        };
        write64(
            &self.memory,
            DEVICE_CONTEXT_ARRAY_AT + usize::from(slot) * 8,
            base + (area + OUTPUT_CONTEXT) as u64,
        );
        let mut max_packet = default_max_packet(speed);
        write32(&self.memory, self.input_context(area, 0) + 4, 0b11);
        self.write_slot_context(area, 1, &location, None);
        self.write_control_endpoint(&device, max_packet);
        self.command(
            base + (area + INPUT_CONTEXT) as u64,
            (TRB_ADDRESS_DEVICE << 10) | (u32::from(slot) << 24),
            keep_waiting,
        )?;
        self.control_transfer(
            &mut device,
            Setup::get_descriptor(DESCRIPTOR_DEVICE, 0, 8),
            DESCRIPTOR_BUFFER,
            keep_waiting,
        )?;
        let mut header = [0u8; 8];
        copy_out(&self.memory, area + DESCRIPTOR_BUFFER, &mut header);
        let reported = u32::from(header[7]);
        if speed <= SPEED_HIGH && reported != 0 && reported != max_packet {
            max_packet = reported;
            write32(&self.memory, self.input_context(area, 0) + 4, 0b10);
            self.write_control_endpoint(&device, max_packet);
            self.command(
                base + (area + INPUT_CONTEXT) as u64,
                (TRB_EVALUATE_CONTEXT << 10) | (u32::from(slot) << 24),
                keep_waiting,
            )?;
        }
        self.control_transfer(
            &mut device,
            Setup::get_descriptor(DESCRIPTOR_CONFIGURATION, 0, 9),
            DESCRIPTOR_BUFFER,
            keep_waiting,
        )?;
        let mut head = [0u8; 9];
        copy_out(&self.memory, area + DESCRIPTOR_BUFFER, &mut head);
        let total =
            usize::from(u16::from_le_bytes([head[2], head[3]])).clamp(9, DESCRIPTOR_BUFFER_BYTES);
        self.control_transfer(
            &mut device,
            Setup::get_descriptor(DESCRIPTOR_CONFIGURATION, 0, total as u16),
            DESCRIPTOR_BUFFER,
            keep_waiting,
        )?;
        let mut configuration = [0u8; DESCRIPTOR_BUFFER_BYTES];
        copy_out(
            &self.memory,
            area + DESCRIPTOR_BUFFER,
            &mut configuration[..total],
        );
        if let Some(storage) = storage_interface(&configuration[..total]) {
            self.set_up_storage(&mut device, storage, keep_waiting)?;
            return Ok(device);
        }
        let interface_class = configuration.get(9 + 5).copied().unwrap_or(0);
        if header[4] == DEVICE_CLASS_HUB || interface_class == DEVICE_CLASS_HUB {
            let ports = self.set_up_hub(&mut device, head[5], keep_waiting)?;
            device.handler = Handler::Hub { ports };
            return Ok(device);
        }
        let interface = boot_interface(&configuration[..total])
            .ok_or(Error::Unsupported("not a HID boot keyboard or mouse"))?;
        device.interface = interface;
        self.control_transfer(
            &mut device,
            Setup::set_configuration(interface.configuration),
            0,
            keep_waiting,
        )?;
        device.handler = match interface.kind {
            BootDevice::Keyboard => Handler::Keyboard(BootKeyboard::default()),
            BootDevice::Mouse => Handler::Mouse(BootMouse::default()),
            BootDevice::Report => {
                let length = usize::from(interface.report_length).clamp(1, DESCRIPTOR_BUFFER_BYTES);
                self.control_transfer(
                    &mut device,
                    Setup::hid_report_descriptor(interface.interface, length as u16),
                    DESCRIPTOR_BUFFER,
                    keep_waiting,
                )?;
                let mut descriptor = [0u8; DESCRIPTOR_BUFFER_BYTES];
                copy_out(
                    &self.memory,
                    area + DESCRIPTOR_BUFFER,
                    &mut descriptor[..length],
                );
                let layout = Layout::parse(&descriptor[..length]).ok_or(Error::Unsupported(
                    "the device's report descriptor does not parse",
                ))?;
                Handler::Report(layout, Decoder::default())
            }
        };
        if interface.kind != BootDevice::Report {
            let _ = self.control_transfer(
                &mut device,
                Setup::hid_set_protocol_boot(interface.interface),
                0,
                keep_waiting,
            );
        }
        let _ = self.control_transfer(
            &mut device,
            Setup::hid_set_idle(interface.interface),
            0,
            keep_waiting,
        );

        let dci = u32::from(interface.endpoint) * 2 + 1;
        device.report_endpoint = dci;
        zero(&self.memory, area + INPUT_CONTEXT, 0x1000);
        write32(
            &self.memory,
            self.input_context(area, 0) + 4,
            1 | (1 << dci),
        );
        self.write_slot_context(area, dci, &location, None);
        let endpoint = self.input_context(area, dci as usize + 1);
        let packet = u32::from(interface.max_packet.max(1));
        write32(
            &self.memory,
            endpoint,
            interval_field(speed, interface.interval) << 16,
        );
        write32(
            &self.memory,
            endpoint + 4,
            (3 << 1) | (ENDPOINT_INTERRUPT_IN << 3) | (packet << 16),
        );
        write64(
            &self.memory,
            endpoint + 8,
            (base + (area + REPORT_RING) as u64) | 1,
        );
        write32(&self.memory, endpoint + 16, packet | (packet << 16));
        self.command(
            base + (area + INPUT_CONTEXT) as u64,
            (TRB_CONFIGURE_ENDPOINT << 10) | (u32::from(slot) << 24),
            keep_waiting,
        )?;
        self.queue_report(&mut device);
        Ok(device)
    }

    fn set_up_storage(
        &mut self,
        device: &mut Device,
        storage: StorageInterface,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.control_transfer(
            device,
            Setup::set_configuration(storage.configuration),
            0,
            keep_waiting,
        )?;
        let base = self.memory.dma_addr().get();
        let area = device.area;
        let input = u32::from(storage.bulk_in) * 2 + 1;
        let output = u32::from(storage.bulk_out) * 2;
        zero(&self.memory, area + INPUT_CONTEXT, 0x1000);
        write32(
            &self.memory,
            self.input_context(area, 0) + 4,
            1 | (1 << input) | (1 << output),
        );
        let location = device.location;
        self.write_slot_context(area, input.max(output), &location, None);
        let packet = u32::from(storage.max_packet.max(64));
        for (dci, kind, ring) in [
            (input, ENDPOINT_BULK_IN, area + REPORT_RING),
            (output, ENDPOINT_BULK_OUT, area + BULK_OUT_RING),
        ] {
            let endpoint = self.input_context(area, dci as usize + 1);
            write32(&self.memory, endpoint, 0);
            write32(
                &self.memory,
                endpoint + 4,
                (3 << 1) | (kind << 3) | (packet << 16),
            );
            write64(&self.memory, endpoint + 8, (base + ring as u64) | 1);
            write32(&self.memory, endpoint + 16, 3072);
        }
        self.command(
            base + (area + INPUT_CONTEXT) as u64,
            (TRB_CONFIGURE_ENDPOINT << 10) | (u32::from(device.slot) << 24),
            keep_waiting,
        )?;
        device.interface.endpoint = storage.bulk_in;
        device.report_endpoint = 0;
        device.handler = Handler::Storage {
            interface: storage,
            block_bytes: 512,
            blocks: 0,
            tag: 1,
        };
        let mut ready = false;
        for _ in 0..UNIT_READY_TRIES {
            if self
                .scsi(device, &SCSI_TEST_UNIT_READY, true, 0, keep_waiting)
                .is_ok()
            {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(Error::Unsupported("the USB disk did not become ready"));
        }
        self.scsi(device, &SCSI_READ_CAPACITY_10, true, 8, keep_waiting)?;
        let mut capacity = [0u8; 8];
        copy_out(&self.memory, STORAGE_BUFFER, &mut capacity);
        let last = u32::from_be_bytes([capacity[0], capacity[1], capacity[2], capacity[3]]);
        let block_bytes = u32::from_be_bytes([capacity[4], capacity[5], capacity[6], capacity[7]]);
        if !(512..=4096).contains(&block_bytes) || !block_bytes.is_power_of_two() {
            return Err(Error::Unsupported(
                "the USB disk's block size is not one the kernel takes",
            ));
        }
        if let Handler::Storage {
            blocks,
            block_bytes: size,
            ..
        } = &mut device.handler
        {
            *blocks = u64::from(last) + 1;
            *size = block_bytes;
        }
        Ok(())
    }

    fn bulk(
        &mut self,
        device: &mut Device,
        out: bool,
        offset: usize,
        length: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        let Handler::Storage { interface, .. } = device.handler else {
            return Err(Error::Unsupported("not a USB disk"));
        };
        let base = self.memory.dma_addr().get();
        let (dci, mut ring) = if out {
            (u32::from(interface.bulk_out) * 2, device.bulk_out)
        } else {
            (u32::from(interface.bulk_in) * 2 + 1, device.reports)
        };
        let trb = self.enqueue(
            &mut ring,
            base + offset as u64,
            length as u32,
            (TRB_NORMAL << 10) | TRB_INTERRUPT_ON_COMPLETION | TRB_INTERRUPT_ON_SHORT,
        );
        if out {
            device.bulk_out = ring;
        } else {
            device.reports = ring;
        }
        self.ring_doorbell(device.slot, dci);
        let event = self.wait_for(TRB_TRANSFER_EVENT, trb, keep_waiting)?;
        let code = event[2] >> 24;
        if code != COMPLETION_SUCCESS && code != COMPLETION_SHORT_PACKET {
            return Err(Error::TransferFailed);
        }
        Ok(length.saturating_sub((event[2] & 0x00ff_ffff) as usize))
    }

    fn scsi(
        &mut self,
        device: &mut Device,
        command: &[u8],
        data_in: bool,
        length: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let Handler::Storage { tag, .. } = &mut device.handler else {
            return Err(Error::Unsupported("not a USB disk"));
        };
        *tag = tag.wrapping_add(1);
        let tag = *tag;
        let wrapper = command_block(tag, length as u32, data_in, command);
        copy_in(&self.memory, device.area + DESCRIPTOR_BUFFER, &wrapper);
        self.bulk(
            device,
            true,
            device.area + DESCRIPTOR_BUFFER,
            wrapper.len(),
            keep_waiting,
        )?;
        if length > 0 {
            self.bulk(device, !data_in, STORAGE_BUFFER, length, keep_waiting)?;
        }
        self.bulk(
            device,
            false,
            device.area + DESCRIPTOR_BUFFER,
            13,
            keep_waiting,
        )?;
        let mut status = [0u8; 13];
        copy_out(&self.memory, device.area + DESCRIPTOR_BUFFER, &mut status);
        status_passed(&status, tag)
            .then_some(())
            .ok_or(Error::TransferFailed)
    }

    /// The USB disks attached: each one's device index and size in 512-byte sectors.
    pub fn disks(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| match device.as_ref()?.handler {
                Handler::Storage {
                    block_bytes,
                    blocks,
                    ..
                } => Some((
                    index,
                    blocks.saturating_mul(u64::from(block_bytes / SECTOR_SIZE as u32)),
                )),
                _ => None,
            })
    }

    /// Reads whole 512-byte sectors from `sector` on of USB disk `index` into `buffer`.
    pub fn disk_read(
        &mut self,
        index: usize,
        sector: u64,
        buffer: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.disk_transfer(index, sector, DiskBuffer::Read(buffer), keep_waiting)
    }

    /// Writes `buffer`, whole 512-byte sectors, to the sectors from `sector` on of USB disk
    /// `index`.
    pub fn disk_write(
        &mut self,
        index: usize,
        sector: u64,
        buffer: &[u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.disk_transfer(index, sector, DiskBuffer::Write(buffer), keep_waiting)
    }

    fn disk_transfer(
        &mut self,
        index: usize,
        sector: u64,
        mut buffer: DiskBuffer<'_>,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let mut device = self
            .devices
            .get(index)
            .copied()
            .flatten()
            .ok_or(Error::Unsupported("no such USB disk"))?;
        let Handler::Storage {
            block_bytes,
            blocks: disk_blocks,
            ..
        } = device.handler
        else {
            return Err(Error::Unsupported("not a USB disk"));
        };
        let block_bytes = block_bytes as usize;
        let length = buffer.len();
        if !length.is_multiple_of(SECTOR_SIZE) {
            return Err(Error::Unsupported("not a whole number of sectors"));
        }
        let sectors = (length / SECTOR_SIZE) as u64;
        let disk_sectors = disk_blocks.saturating_mul((block_bytes / SECTOR_SIZE) as u64);
        if sector
            .checked_add(sectors)
            .is_none_or(|end| end > disk_sectors)
        {
            return Err(Error::Unsupported("past the end of the USB disk"));
        }
        let per_chunk =
            ((STORAGE_BUFFER_BYTES / block_bytes - 1) * (block_bytes / SECTOR_SIZE)) as u64;
        let mut done = 0u64;
        let mut result = Ok(());
        while done < sectors && result.is_ok() {
            let count = per_chunk.min(sectors - done);
            let Covering {
                first_block,
                blocks,
                skip_bytes,
            } = covering(sector + done, count, block_bytes);
            let start = done as usize * SECTOR_SIZE;
            let chunk = count as usize * SECTOR_SIZE;
            let bytes = blocks as usize * block_bytes;
            result = match u32::try_from(first_block) {
                Err(_) => Err(Error::Unsupported("past 2 TiB on a USB disk")),
                Ok(lba) => self.disk_chunk(
                    &mut device,
                    lba,
                    blocks as u16,
                    bytes,
                    skip_bytes,
                    &mut buffer,
                    start,
                    chunk,
                    keep_waiting,
                ),
            };
            done += count;
        }
        self.devices[index] = Some(device);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn disk_chunk(
        &mut self,
        device: &mut Device,
        lba: u32,
        blocks: u16,
        bytes: usize,
        skip_bytes: usize,
        buffer: &mut DiskBuffer<'_>,
        start: usize,
        chunk: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let partial = skip_bytes != 0 || chunk != bytes;
        match buffer {
            DiskBuffer::Read(buffer) => {
                self.scsi(
                    device,
                    &scsi_read_write_10(false, lba, blocks),
                    true,
                    bytes,
                    keep_waiting,
                )?;
                copy_out(
                    &self.memory,
                    STORAGE_BUFFER + skip_bytes,
                    &mut buffer[start..start + chunk],
                );
            }
            DiskBuffer::Write(buffer) => {
                if partial {
                    self.scsi(
                        device,
                        &scsi_read_write_10(false, lba, blocks),
                        true,
                        bytes,
                        keep_waiting,
                    )?;
                }
                copy_in(
                    &self.memory,
                    STORAGE_BUFFER + skip_bytes,
                    &buffer[start..start + chunk],
                );
                self.scsi(
                    device,
                    &scsi_read_write_10(true, lba, blocks),
                    false,
                    bytes,
                    keep_waiting,
                )?;
            }
        }
        Ok(())
    }

    /// Asks USB disk `index` to write its cache out.
    pub fn disk_flush(
        &mut self,
        index: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let mut device = self
            .devices
            .get(index)
            .copied()
            .flatten()
            .ok_or(Error::Unsupported("no such USB disk"))?;
        let result = self.scsi(
            &mut device,
            &SCSI_SYNCHRONIZE_CACHE_10,
            true,
            0,
            keep_waiting,
        );
        self.devices[index] = Some(device);
        result
    }

    fn set_up_hub(
        &mut self,
        device: &mut Device,
        configuration: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<u8, Error> {
        self.control_transfer(
            device,
            Setup::set_configuration(configuration),
            0,
            keep_waiting,
        )?;
        let super_speed = device.location.speed >= SPEED_SUPER;
        let kind = if super_speed {
            HUB_DESCRIPTOR_USB3
        } else {
            HUB_DESCRIPTOR_USB2
        };
        let setup = Setup {
            request_type: 0xa0,
            request: 6,
            value: u16::from(kind) << 8,
            index: 0,
            length: 9,
        };
        self.control_transfer(device, setup, DESCRIPTOR_BUFFER, keep_waiting)?;
        let mut descriptor = [0u8; 9];
        copy_out(
            &self.memory,
            device.area + DESCRIPTOR_BUFFER,
            &mut descriptor,
        );
        let ports = descriptor[2].min(15);
        if super_speed {
            let depth = Setup {
                request_type: 0x20,
                request: 12,
                value: u16::from(device.location.depth),
                index: 0,
                length: 0,
            };
            self.control_transfer(device, depth, 0, keep_waiting)?;
        }
        let base = self.memory.dma_addr().get();
        zero(&self.memory, device.area + INPUT_CONTEXT, 0x1000);
        write32(&self.memory, self.input_context(device.area, 0) + 4, 1);
        let location = device.location;
        self.write_slot_context(device.area, 1, &location, Some(ports));
        self.command(
            base + (device.area + INPUT_CONTEXT) as u64,
            (TRB_CONFIGURE_ENDPOINT << 10) | (u32::from(device.slot) << 24),
            keep_waiting,
        )?;
        for port in 1..=ports {
            let mut hub = *device;
            let _ = self.hub_request(&mut hub, 3, PORT_FEATURE_POWER, port, keep_waiting);
            *device = hub;
        }
        Ok(ports)
    }

    fn write_control_endpoint(&self, device: &Device, max_packet: u32) {
        let base = self.memory.dma_addr().get();
        let endpoint = self.input_context(device.area, 2);
        write32(&self.memory, endpoint, 0);
        write32(
            &self.memory,
            endpoint + 4,
            (3 << 1) | (ENDPOINT_CONTROL << 3) | (max_packet << 16),
        );
        write64(
            &self.memory,
            endpoint + 8,
            (base + device.control.at as u64) | u64::from(device.control.cycle),
        );
        write32(&self.memory, endpoint + 16, 8);
    }

    fn queue_report(&mut self, device: &mut Device) {
        let base = self.memory.dma_addr().get();
        let length = usize::from(device.interface.max_packet).clamp(1, REPORT_BYTES) as u32;
        let mut ring = device.reports;
        self.enqueue(
            &mut ring,
            base + (device.area + REPORT_BUFFER) as u64,
            length,
            (TRB_NORMAL << 10) | TRB_INTERRUPT_ON_COMPLETION | TRB_INTERRUPT_ON_SHORT,
        );
        device.reports = ring;
        self.ring_doorbell(device.slot, device.report_endpoint);
    }

    fn handle_transfer(&mut self, event: [u32; 4], emit: &mut dyn FnMut(RawEvent)) -> usize {
        let slot = (event[3] >> 24) as u8;
        let endpoint = (event[3] >> 16) & 0x1f;
        let Some(index) = self.devices.iter().position(|device| {
            device.is_some_and(|device| device.slot == slot && device.report_endpoint == endpoint)
        }) else {
            return 0;
        };
        let Some(mut device) = self.devices[index] else {
            return 0;
        };
        let code = event[2] >> 24;
        let mut sent = 0;
        if code == COMPLETION_SUCCESS || code == COMPLETION_SHORT_PACKET {
            let requested = usize::from(device.interface.max_packet).clamp(1, REPORT_BYTES);
            let left = (event[2] & 0x00ff_ffff) as usize;
            let mut report = [0u8; REPORT_BYTES];
            let length = requested.saturating_sub(left);
            copy_out(
                &self.memory,
                device.area + REPORT_BUFFER,
                &mut report[..length],
            );
            sent = match &mut device.handler {
                Handler::Keyboard(keyboard) => keyboard.report(&report[..length], emit),
                Handler::Mouse(mouse) => mouse.report(&report[..length], emit),
                Handler::Hub { .. } | Handler::Storage { .. } => 0,
                Handler::Report(layout, decoder) => decoder.report(layout, &report[..length], emit),
            };
        }
        self.queue_report(&mut device);
        self.devices[index] = Some(device);
        sent
    }

    /// Asks the controller to interrupt on its events (D255), once a message interrupt is
    /// connected.
    pub fn enable_interrupts(&mut self) {
        self.regs.write32(
            self.runtime + INTERRUPTER_MANAGEMENT,
            INTERRUPT_ENABLE | INTERRUPT_PENDING,
        );
        let command = self.regs.read32(self.op(USB_COMMAND));
        self.regs
            .write32(self.op(USB_COMMAND), command | COMMAND_INTERRUPTS);
    }

    fn clear_interrupt(&self) {
        let management = self.regs.read32(self.runtime + INTERRUPTER_MANAGEMENT);
        self.regs.write32(
            self.runtime + INTERRUPTER_MANAGEMENT,
            management | INTERRUPT_PENDING,
        );
        self.regs
            .write32(self.op(USB_STATUS), STATUS_EVENT_INTERRUPT);
    }

    /// Hands `emit` the reports the devices sent, at most `budget` events' worth of reports;
    /// returns how many reports it took.
    pub fn poll(&mut self, budget: usize, emit: &mut dyn FnMut(RawEvent)) -> usize {
        self.clear_interrupt();
        for raw in &self.pending[..self.pending_count] {
            emit(*raw);
        }
        self.pending_count = 0;
        let mut taken = 0;
        while taken < budget {
            let Some(event) = self.next_event() else {
                break;
            };
            if (event[3] >> 10) & 0x3f == TRB_TRANSFER_EVENT {
                self.handle_transfer(event, emit);
            }
            taken += 1;
        }
        if let Some(lights) = self.lights.take() {
            self.send_lights(lights);
        }
        taken
    }

    fn send_lights(&mut self, lights: LockLights) {
        let base = self.memory.dma_addr().get();
        for index in 0..MAX_DEVICES {
            let Some(mut device) = self.devices[index] else {
                continue;
            };
            if device.interface.kind != BootDevice::Keyboard {
                continue;
            }
            let buffer = device.area + LIGHTS_BUFFER;
            write32(&self.memory, buffer, u32::from(lights_report(lights)));
            let setup = Setup::hid_set_output_report(device.interface.interface, 1);
            let mut ring = device.control;
            self.enqueue(
                &mut ring,
                setup.as_u64(),
                8,
                (TRB_SETUP << 10) | TRB_IMMEDIATE_DATA | TRANSFER_TYPE_OUT,
            );
            self.enqueue(&mut ring, base + buffer as u64, 1, TRB_DATA << 10);
            self.enqueue(&mut ring, 0, 0, (TRB_STATUS << 10) | TRB_DIRECTION_IN);
            device.control = ring;
            self.ring_doorbell(device.slot, 1);
            self.devices[index] = Some(device);
        }
    }

    /// Asks every keyboard to show `lights`, at the next poll.
    pub fn set_lights(&mut self, lights: LockLights) {
        self.lights = Some(lights);
    }

    /// Which kinds of device are attached.
    pub fn has(&self, kind: BootDevice) -> bool {
        self.devices
            .iter()
            .flatten()
            .any(|device| device.interface.kind == kind)
    }

    /// What the attached devices report, together.
    pub fn capabilities(&self) -> Capabilities {
        let mut all = Capabilities::default();
        for device in self.devices.iter().flatten() {
            let each = match &device.handler {
                Handler::Keyboard(_) => Capabilities {
                    keyboard: true,
                    ..Capabilities::default()
                },
                Handler::Mouse(_) => Capabilities {
                    buttons: true,
                    relative: true,
                    wheel: true,
                    ..Capabilities::default()
                },
                Handler::Report(layout, _) => layout.capabilities(),
                Handler::Hub { .. } | Handler::Storage { .. } => Capabilities::default(),
            };
            all.keyboard |= each.keyboard;
            all.buttons |= each.buttons;
            all.relative |= each.relative;
            all.wheel |= each.wheel;
            all.absolute = all.absolute.or(each.absolute);
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_speed_intervals_become_powers_of_two_microframes() {
        assert_eq!(interval_field(SPEED_FULL, 10), 6);
        assert_eq!(interval_field(SPEED_LOW, 1), 3);
        assert_eq!(interval_field(SPEED_FULL, 255), 10);
        assert_eq!(interval_field(SPEED_HIGH, 4), 3);
    }

    #[test]
    fn the_default_control_packet_size_follows_the_speed() {
        assert_eq!(default_max_packet(SPEED_LOW), 8);
        assert_eq!(default_max_packet(SPEED_HIGH), 64);
        assert_eq!(default_max_packet(4), 512);
    }

    #[test]
    fn a_full_speed_device_behind_a_high_speed_hub_uses_its_translator() {
        let root = Location {
            root_port: 3,
            route: 0,
            speed: SPEED_HIGH,
            translator: None,
            depth: 0,
        };
        let child = child_location(&root, 5, 2, SPEED_FULL);
        assert_eq!(child.route, 2);
        assert_eq!(child.translator, Some((5, 2)));
        assert_eq!(child.root_port, 3);
        let grandchild = child_location(
            &Location {
                speed: SPEED_HIGH,
                translator: None,
                ..child
            },
            6,
            4,
            SPEED_HIGH,
        );
        assert_eq!(grandchild.route, 0x42);
        assert_eq!(grandchild.translator, None);
    }

    #[test]
    fn hub_port_status_gives_the_speed() {
        assert_eq!(hub_port_speed(HUB_PORT_LOW_SPEED | 1, false), SPEED_LOW);
        assert_eq!(hub_port_speed(HUB_PORT_HIGH_SPEED | 1, false), SPEED_HIGH);
        assert_eq!(hub_port_speed(1, false), SPEED_FULL);
        assert_eq!(hub_port_speed(1, true), SPEED_SUPER);
    }
}

//! The ACPI namespace the kernel runs (D538): the DSDT and every SSDT loaded into the
//! `eggos-acpi` interpreter at boot, their system-memory regions mapped, the firmware told the
//! interrupt model (`\_PIC`), and every present device's `_INI` run by `_STA`. Devices `_STA`
//! says are absent are left out of the device table, and `\_PTS(5)` runs before the machine
//! powers off.
//!
//! The interpreter's host reaches system memory only inside the regions mapped at boot (device
//! windows are mapped on the boot CPU alone), I/O ports through the HAL, PCI configuration
//! space through segment 0's ECAM window, and the embedded controller's space through its two
//! ports. `Sleep` waits on the clock, since the interpreter runs at boot before the scheduler
//! and at power-off after it.
//!
//! ACPI events (D539): [`start_events`] finds the embedded controller and tells its `_REG` the
//! space is there, turns on every general-purpose event with a `_Lxx` or `_Exx` method and the
//! controller's own, and has the HAL route the system control interrupt to a thread. Each
//! interrupt runs the events' methods (an edge event's status cleared first, a level event's
//! after), answers the controller's queries with its `_Qxx` methods, turns off an event nothing
//! handles, and acts on what AML notified: a power button device's `0x80` is a press.

use super::interrupts::DeviceEvents;
use crate::sync::{SleepLock, Sleeping};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use eggos_acpi::{
    Error, Host, HostError, Interpreter, NameSeg, Object, Path, PciFunction, RegionSpace,
};
use eggos_drivers::embedded_controller::{EcPorts, EmbeddedController};
use eggos_drivers::io::{Mmio, RegisterIo};
use eggos_drivers::pci::{Address, ConfigSpace, ecam::Ecam};
use eggos_hal::acpi::GpeBlock;
use eggos_hal::{PhysicalRange, Platform};

/// What the HAL's system control interrupt raises, for the ACPI event thread (D539).
pub static SYSTEM_CONTROL_EVENTS: DeviceEvents = DeviceEvents::new();

const EMBEDDED_CONTROLLER_POLLS: u32 = 100_000;
const EMBEDDED_CONTROL_SPACE: u64 = 3;
const SPACE_CONNECTED: u64 = 1;
const POWER_BUTTON_PRESSED: u64 = 0x80;
const MOST_QUERIES_PER_EVENT: u32 = 32;
const KEPT_PENDING_NOTIFICATIONS: usize = 64;
const HANDLED_EVENTS: u16 = 256;
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

const KEPT_NOTIFICATIONS: usize = 32;
const HUNDRED_NANOSECONDS_PER_SECOND: u128 = 10_000_000;
const MICROSECONDS_PER_SECOND: u128 = 1_000_000;
const SLEEP_STATE_OFF: u64 = 5;

/// A system-memory region mapped at boot: `length` bytes at physical `start`, at `virt`.
#[derive(Debug, Clone, Copy)]
struct MappedRegion {
    start: u64,
    length: u64,
    virt: usize,
}

/// What the interpreter reaches the machine through.
#[derive(Debug)]
pub struct KernelHost {
    regions: Vec<MappedRegion>,
    configuration: Option<Ecam>,
    clock: fn() -> u64,
    frequency: u64,
    read_port: fn(u16, u32) -> Option<u32>,
    write_port: fn(u16, u32, u32) -> bool,
    notifications: VecDeque<(Path, u64)>,
    pending: Vec<(Path, u64)>,
    controller: Option<ControllerPorts>,
}

/// An embedded controller's data and command ports, reached through the HAL.
#[derive(Debug, Clone, Copy)]
struct ControllerPorts {
    data: u16,
    command: u16,
    read_port: fn(u16, u32) -> Option<u32>,
    write_port: fn(u16, u32, u32) -> bool,
}

impl EcPorts for ControllerPorts {
    fn read_status(&mut self) -> u8 {
        (self.read_port)(self.command, 8).unwrap_or(0) as u8
    }

    fn write_command(&mut self, command: u8) {
        (self.write_port)(self.command, 8, u32::from(command));
    }

    fn read_data(&mut self) -> u8 {
        (self.read_port)(self.data, 8).unwrap_or(0xff) as u8
    }

    fn write_data(&mut self, value: u8) {
        (self.write_port)(self.data, 8, u32::from(value));
    }
}

impl KernelHost {
    fn on<P: Platform>(configuration: Option<Ecam>) -> Self {
        Self {
            regions: Vec::new(),
            configuration,
            clock: P::clock_ticks,
            frequency: P::clock_frequency().max(1),
            read_port: P::read_io_port,
            write_port: P::write_io_port,
            notifications: VecDeque::new(),
            pending: Vec::new(),
            controller: None,
        }
    }

    fn controller(&self) -> Result<EmbeddedController<ControllerPorts>, HostError> {
        self.controller
            .map(|ports| EmbeddedController::new(ports, EMBEDDED_CONTROLLER_POLLS))
            .ok_or(HostError::UnsupportedSpace)
    }

    fn mapped(&self, address: u64, width: u32) -> Result<Mmio, HostError> {
        let bytes = u64::from(width / 8);
        let end = address.checked_add(bytes).ok_or(HostError::OutOfRange)?;
        let region = self
            .regions
            .iter()
            .find(|region| address >= region.start && end <= region.start + region.length)
            .ok_or(HostError::OutOfRange)?;
        let offset = (address - region.start) as usize;
        // SAFETY: `start` mapped the region as device memory for good, and the access lies
        // inside it (checked above).
        Ok(unsafe { Mmio::new(region.virt + offset, bytes as usize) })
    }

    fn wait_ticks(&self, ticks: u64) {
        let deadline = (self.clock)().saturating_add(ticks);
        while (self.clock)() < deadline {
            core::hint::spin_loop();
        }
    }

    fn ticks_for(&self, amount: u64, per_second: u128) -> u64 {
        (u128::from(amount) * u128::from(self.frequency) / per_second).min(u128::from(u64::MAX))
            as u64
    }

    fn configuration_address(
        &self,
        function: PciFunction,
    ) -> Result<(&Ecam, Address), HostError> {
        let ecam = self.configuration.as_ref().ok_or(HostError::UnsupportedSpace)?;
        if function.segment != 0 {
            return Err(HostError::OutOfRange);
        }
        let address = Address::new(function.bus, function.device, function.function)
            .ok_or(HostError::OutOfRange)?;
        Ok((ecam, address))
    }
}

impl Host for KernelHost {
    fn read(&mut self, space: RegionSpace, address: u64, width: u32) -> Result<u64, HostError> {
        match space {
            RegionSpace::SystemMemory => {
                let registers = self.mapped(address, width)?;
                Ok(match width {
                    8 => u64::from(registers.read8(0)),
                    16 => u64::from(registers.read16(0)),
                    32 => u64::from(registers.read32(0)),
                    64 => u64::from(registers.read32(0)) | u64::from(registers.read32(4)) << 32,
                    _ => return Err(HostError::OutOfRange),
                })
            }
            RegionSpace::SystemIo => {
                let port = u16::try_from(address).map_err(|_| HostError::OutOfRange)?;
                (self.read_port)(port, width)
                    .map(u64::from)
                    .ok_or(HostError::UnsupportedSpace)
            }
            RegionSpace::EmbeddedControl => {
                let mut controller = self.controller()?;
                let mut value = 0;
                for byte in 0..u64::from(width / 8) {
                    let at = u8::try_from(address + byte).map_err(|_| HostError::OutOfRange)?;
                    let read = controller.read(at).map_err(|_| HostError::NoResponse)?;
                    value |= u64::from(read) << (8 * byte);
                }
                Ok(value)
            }
            _ => Err(HostError::UnsupportedSpace),
        }
    }

    fn write(
        &mut self,
        space: RegionSpace,
        address: u64,
        width: u32,
        value: u64,
    ) -> Result<(), HostError> {
        match space {
            RegionSpace::SystemMemory => {
                let registers = self.mapped(address, width)?;
                match width {
                    8 => registers.write8(0, value as u8),
                    16 => registers.write16(0, value as u16),
                    32 => registers.write32(0, value as u32),
                    64 => {
                        registers.write32(0, value as u32);
                        registers.write32(4, (value >> 32) as u32);
                    }
                    _ => return Err(HostError::OutOfRange),
                }
                Ok(())
            }
            RegionSpace::SystemIo => {
                let port = u16::try_from(address).map_err(|_| HostError::OutOfRange)?;
                match (self.write_port)(port, width, value as u32) {
                    true => Ok(()),
                    false => Err(HostError::UnsupportedSpace),
                }
            }
            RegionSpace::EmbeddedControl => {
                let mut controller = self.controller()?;
                for byte in 0..u64::from(width / 8) {
                    let at = u8::try_from(address + byte).map_err(|_| HostError::OutOfRange)?;
                    controller
                        .write(at, (value >> (8 * byte)) as u8)
                        .map_err(|_| HostError::NoResponse)?;
                }
                Ok(())
            }
            _ => Err(HostError::UnsupportedSpace),
        }
    }

    fn read_pci(
        &mut self,
        function: PciFunction,
        offset: u16,
        width: u32,
    ) -> Result<u64, HostError> {
        let (ecam, address) = self.configuration_address(function)?;
        let word = |at: u16| u64::from(ecam.read32(address, at));
        if width == 64 {
            return Ok(word(offset) | word(offset + 4) << 32);
        }
        let shift = u32::from(offset & 3) * 8;
        if shift + width > 32 {
            return Err(HostError::OutOfRange);
        }
        Ok(word(offset & !3) >> shift & (u64::MAX >> (64 - width)))
    }

    fn write_pci(
        &mut self,
        function: PciFunction,
        offset: u16,
        width: u32,
        value: u64,
    ) -> Result<(), HostError> {
        let (ecam, address) = self.configuration_address(function)?;
        if width == 64 {
            ecam.write32(address, offset, value as u32);
            ecam.write32(address, offset + 4, (value >> 32) as u32);
            return Ok(());
        }
        let shift = u32::from(offset & 3) * 8;
        if shift + width > 32 {
            return Err(HostError::OutOfRange);
        }
        let aligned = offset & !3;
        let kept = match width {
            32 => 0,
            _ => ecam.read32(address, aligned) & !((u32::MAX >> (32 - width)) << shift),
        };
        ecam.write32(address, aligned, kept | (value as u32) << shift);
        Ok(())
    }

    fn sleep(&mut self, milliseconds: u64) {
        self.wait_ticks(self.ticks_for(milliseconds, 1000));
    }

    fn stall(&mut self, microseconds: u64) {
        self.wait_ticks(self.ticks_for(microseconds, MICROSECONDS_PER_SECOND));
    }

    fn timer(&mut self) -> u64 {
        let ticks = u128::from((self.clock)());
        (ticks * HUNDRED_NANOSECONDS_PER_SECOND / u128::from(self.frequency)) as u64
    }

    fn notify(&mut self, path: &Path, value: u64) {
        if self.notifications.len() == KEPT_NOTIFICATIONS {
            self.notifications.pop_front();
        }
        self.notifications.push_back((path.clone(), value));
        if self.pending.len() < KEPT_PENDING_NOTIFICATIONS {
            self.pending.push((path.clone(), value));
        }
    }
}

/// The namespace once loaded, the devices `_STA` says are absent, and what [`start_events`]
/// found.
struct Loaded {
    interpreter: Interpreter<KernelHost>,
    absent: Vec<Path>,
    events: Option<Events>,
}

/// The general-purpose event blocks, the embedded controller and the power button devices.
struct Events {
    blocks: Vec<GpeBlock>,
    controller: Option<(Path, Option<u16>)>,
    power_buttons: Vec<Path>,
}

static NAMESPACE: SleepLock<Option<Box<Loaded>>> = SleepLock::new(None, Sleeping::spinning());

/// What [`start`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Started {
    /// Definition blocks loaded, the DSDT included.
    pub tables: usize,
    /// Devices `_STA` was asked about.
    pub devices: usize,
    /// The first error a table's load gave, if one did.
    pub error: Option<Error>,
}

/// Loads the DSDT and every SSDT, maps their system-memory regions through `map`, tells the
/// firmware the interrupt model, and runs the devices' `_INI` by `_STA`. Once, at boot on the
/// boot CPU while device windows can still be mapped, with PCI's ECAM window when there is one.
/// `None` on a machine without a DSDT.
pub fn start<P: Platform>(
    map: &mut dyn FnMut(PhysicalRange) -> Option<usize>,
    configuration: Option<Ecam>,
) -> Option<Started> {
    let dsdt = P::acpi_dsdt()?;
    let mut interpreter = Interpreter::new(KernelHost::on::<P>(configuration));
    let mut started = Started::default();
    let mut load = |interpreter: &mut Interpreter<KernelHost>, table: &[u8]| {
        started.tables += 1;
        if let Err(error) = interpreter.load_table(table) {
            started.error.get_or_insert(error);
        }
    };
    load(&mut interpreter, dsdt);
    let mut ssdts = Vec::new();
    P::acpi_tables(b"SSDT", &mut |table| ssdts.push(table));
    for ssdt in ssdts {
        load(&mut interpreter, ssdt);
    }
    map_regions(&mut interpreter, map);
    if let Some(model) = P::acpi_interrupt_model() {
        let _ = interpreter.evaluate_child(&Path::root(), b"_PIC", &[Object::Integer(model)]);
    }
    let statuses = interpreter.initialize_devices();
    started.devices = statuses.len();
    let absent = statuses
        .into_iter()
        .filter(|status| !status.is_present())
        .map(|status| status.path)
        .collect();
    *NAMESPACE.lock() = Some(Box::new(Loaded {
        interpreter,
        absent,
        events: None,
    }));
    Some(started)
}

fn map_regions(
    interpreter: &mut Interpreter<KernelHost>,
    map: &mut dyn FnMut(PhysicalRange) -> Option<usize>,
) {
    let regions: Vec<(u64, u64)> = interpreter
        .namespace()
        .iter()
        .filter_map(|(_, node)| match interpreter.namespace().get(node) {
            Some(Object::Region(region))
                if region.space == RegionSpace::SystemMemory && region.length != 0 =>
            {
                Some((region.offset, region.length))
            }
            _ => None,
        })
        .collect();
    for (start, length) in regions {
        let mapped = &interpreter.host().regions;
        if mapped
            .iter()
            .any(|region| start >= region.start && start + length <= region.start + region.length)
        {
            continue;
        }
        if let Some(virt) = map(PhysicalRange::from_start_length(start, length)) {
            interpreter.host().regions.push(MappedRegion {
                start,
                length,
                virt,
            });
        }
    }
}

/// Whether the ACPI device at `path` (written as `\_SB_.PCI0.GPI0`) may be used: not below a
/// device `_STA` says is absent. Every device is present before [`start`] or without ACPI.
pub fn is_present(path: &[u8]) -> bool {
    let Some(path) = Path::parse(path) else {
        return true;
    };
    match NAMESPACE.lock().as_ref() {
        Some(loaded) => !loaded.absent.iter().any(|absent| path.starts_with(absent)),
        None => true,
    }
}

/// Runs `\_PTS(5)`, which firmware uses to prepare the machine for powering off (ACPI 6.5,
/// 7.4.1), when the namespace has it. Called right before the HAL enters S5.
pub fn prepare_to_power_off() {
    if let Some(loaded) = NAMESPACE.lock().as_mut() {
        let off = [Object::Integer(SLEEP_STATE_OFF)];
        let _ = loaded.interpreter.evaluate_child(&Path::root(), b"_PTS", &off);
    }
}

/// The last notifications AML sent (`Notify`), oldest first: the object's path and the value.
pub fn notifications() -> Vec<(Path, u64)> {
    match NAMESPACE.lock().as_mut() {
        Some(loaded) => loaded.interpreter.host().notifications.iter().cloned().collect(),
        None => Vec::new(),
    }
}

/// Sets ACPI events up and starts the thread that handles them (D539): the embedded controller
/// and its `_REG`, every general-purpose event with a method turned on, and the system control
/// interrupt routed. Once the scheduler runs; an error says why there are no events.
pub fn start_events<P: Platform>() -> Result<(), &'static str> {
    let fadt = P::acpi_table(b"FACP").ok_or("the firmware gives no FADT")?;
    let blocks: Vec<GpeBlock> = eggos_hal::acpi::gpe_blocks(fadt).collect();
    {
        let mut namespace = NAMESPACE.lock();
        let loaded = namespace.as_mut().ok_or("no ACPI namespace")?;
        let interpreter = &mut loaded.interpreter;
        let controller = set_up_controller(interpreter);
        let controller_event = controller.as_ref().and_then(|(_, event)| *event);
        for block in &blocks {
            for index in 0..block.events() {
                let number = block.base + index;
                if controller_event == Some(number) || event_method(interpreter, number).is_some()
                {
                    set_event_enabled::<P>(block, index, true);
                }
            }
        }
        let power_buttons = interpreter.devices_with_id(&[b"PNP0C0C"]);
        loaded.events = Some(Events {
            blocks,
            controller,
            power_buttons,
        });
    }
    P::enable_system_control()?;
    crate::thread::spawn::<P, _>(|| {
        loop {
            let seen = SYSTEM_CONTROL_EVENTS.raised();
            handle_events::<P>();
            P::unmask_system_control();
            SYSTEM_CONTROL_EVENTS.wait::<P>(seen, u64::MAX);
        }
    })
    .map(|_| ())
    .map_err(|_| "no thread for ACPI events")
}

/// Finds the embedded controller (`PNP0C09`), takes its data and command ports from `_CRS` and
/// its event from `_GPE`, and runs its `_REG` to say its space can be used: its path and event.
fn set_up_controller(interpreter: &mut Interpreter<KernelHost>) -> Option<(Path, Option<u16>)> {
    let path = interpreter.devices_with_id(&[b"PNP0C09"]).into_iter().next()?;
    let Ok(Some(Object::Buffer(template))) = interpreter.evaluate_child(&path, b"_CRS", &[])
    else {
        return None;
    };
    let ports = eggos_acpi::resources::io_ports(&template);
    let (&data, &command) = (ports.first()?, ports.get(1)?);
    let host = interpreter.host();
    host.controller = Some(ControllerPorts {
        data,
        command,
        read_port: host.read_port,
        write_port: host.write_port,
    });
    let event = match interpreter.evaluate_child(&path, b"_GPE", &[]) {
        Ok(Some(value)) => interpreter
            .to_integer(&value)
            .ok()
            .and_then(|number| u16::try_from(number).ok()),
        _ => None,
    };
    let connected = [
        Object::Integer(EMBEDDED_CONTROL_SPACE),
        Object::Integer(SPACE_CONNECTED),
    ];
    let _ = interpreter.evaluate_child(&path, b"_REG", &connected);
    Some((path, event))
}

fn hex_name(kind: u8, number: u8) -> [u8; 4] {
    [
        b'_',
        kind,
        HEX_DIGITS[usize::from(number >> 4)],
        HEX_DIGITS[usize::from(number & 0x0f)],
    ]
}

/// The `\_GPE` method of event `number` and whether it is an edge event's (`_Exx`) rather than
/// a level event's (`_Lxx`).
fn event_method(interpreter: &Interpreter<KernelHost>, number: u16) -> Option<(Path, bool)> {
    if number >= HANDLED_EVENTS {
        return None;
    }
    let scope = Path::root().child(NameSeg::new(b"_GPE"));
    [(b'E', true), (b'L', false)].into_iter().find_map(|(kind, edge)| {
        let path = scope.child(NameSeg(hex_name(kind, number as u8)));
        interpreter.namespace().find(&path).map(|_| (path, edge))
    })
}

fn set_event_enabled<P: Platform>(block: &GpeBlock, index: u16, enabled: bool) {
    let port = block.port + u16::from(block.length / 2) + index / 8;
    let bit = 1u32 << (index % 8);
    let held = P::read_io_port(port, 8).unwrap_or(0);
    let wanted = match enabled {
        true => held | bit,
        false => held & !bit,
    };
    P::write_io_port(port, 8, wanted);
}

/// Handles what raised the system control interrupt: every enabled event whose status is set,
/// then what AML notified.
fn handle_events<P: Platform>() {
    let (notified, power_buttons) = {
        let mut namespace = NAMESPACE.lock();
        let Some(loaded) = namespace.as_mut() else {
            return;
        };
        let Some(events) = loaded.events.take() else {
            return;
        };
        for block in &events.blocks {
            let half = u16::from(block.length / 2);
            for byte in 0..half {
                let status_port = block.port + byte;
                let status = P::read_io_port(status_port, 8).unwrap_or(0);
                let enabled = P::read_io_port(status_port + half, 8).unwrap_or(0);
                for bit in 0..8u16 {
                    if status & enabled & 1 << bit == 0 {
                        continue;
                    }
                    let index = byte * 8 + bit;
                    let number = block.base + index;
                    let clear = || P::write_io_port(status_port, 8, 1 << bit);
                    let interpreter = &mut loaded.interpreter;
                    match &events.controller {
                        Some((path, Some(event))) if *event == number => {
                            clear();
                            answer_queries(interpreter, path);
                            continue;
                        }
                        _ => {}
                    }
                    match event_method(interpreter, number) {
                        Some((method, true)) => {
                            clear();
                            let _ = interpreter.evaluate(&method, &[]);
                        }
                        Some((method, false)) => {
                            let _ = interpreter.evaluate(&method, &[]);
                            clear();
                        }
                        None => {
                            set_event_enabled::<P>(block, index, false);
                            clear();
                        }
                    }
                }
            }
        }
        let notified = core::mem::take(&mut loaded.interpreter.host().pending);
        let power_buttons = events.power_buttons.clone();
        loaded.events = Some(events);
        (notified, power_buttons)
    };
    for (path, value) in notified {
        if value == POWER_BUTTON_PRESSED && power_buttons.contains(&path) {
            crate::linux::reboot::power_button::<P>();
        }
    }
}

/// Collects the embedded controller's events and runs the `_Qxx` method each names.
fn answer_queries(interpreter: &mut Interpreter<KernelHost>, controller: &Path) {
    for _ in 0..MOST_QUERIES_PER_EVENT {
        let Some(ports) = interpreter.host().controller else {
            return;
        };
        let mut embedded = EmbeddedController::new(ports, EMBEDDED_CONTROLLER_POLLS);
        if !embedded.has_event() {
            return;
        }
        let query = match embedded.query() {
            Ok(0) | Err(_) => return,
            Ok(query) => query,
        };
        let _ = interpreter.evaluate_child(controller, &hex_name(b'Q', query), &[]);
    }
}

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
use alloc::vec;
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
    /// The FACS's global lock word in the direct map, and the PM1a control port that tells the
    /// firmware the lock is free (D556).
    global_lock: Option<(usize, u16)>,
    tables: fn(&[u8; 4], &mut dyn FnMut(&'static [u8])),
}

/// The global lock word's bits (ACPI 6.5, 5.2.10.1).
const LOCK_PENDING: u32 = 1 << 0;
const LOCK_OWNED: u32 = 1 << 1;
/// PM1 control's `GBL_RLS`: the OS released a lock the firmware waits for.
const GLOBAL_LOCK_RELEASED: u32 = 1 << 2;
/// The FACS's lock word.
const FACS_GLOBAL_LOCK: usize = 16;
/// The FADT's `FIRMWARE_CTRL`, `X_FIRMWARE_CTRL` and `PM1a_CNT_BLK`.
const FADT_FIRMWARE_CTRL: usize = 36;
const FADT_X_FIRMWARE_CTRL: usize = 132;
const FADT_PM1A_CONTROL: usize = 64;

/// The FACS's global lock word, through the direct map, and PM1a's control port (D556).
fn global_lock_place<P: Platform>() -> Option<(usize, u16)> {
    let fadt = P::acpi_table(b"FACP")?;
    let word = |at: usize, bytes: usize| -> Option<u64> {
        let field = fadt.get(at..at + bytes)?;
        Some(field.iter().rev().fold(0u64, |value, &byte| value << 8 | u64::from(byte)))
    };
    let facs = word(FADT_X_FIRMWARE_CTRL, 8)
        .filter(|&address| address != 0)
        .or_else(|| word(FADT_FIRMWARE_CTRL, 4))
        .filter(|&address| address != 0)?;
    let control = u16::try_from(word(FADT_PM1A_CONTROL, 4)?).ok()?;
    Some((
        (facs as usize).wrapping_add(P::direct_map_offset()) + FACS_GLOBAL_LOCK,
        control,
    ))
}

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
            global_lock: global_lock_place::<P>(),
            tables: P::acpi_tables,
        }
    }

    fn lock_word(&self) -> Option<&core::sync::atomic::AtomicU32> {
        let (address, _) = self.global_lock?;
        // SAFETY: the FACS is firmware memory the direct map covers (D554), whose lock word is
        // 4-byte aligned and shared with the firmware only through atomic updates, as here.
        Some(unsafe { &*(address as *const core::sync::atomic::AtomicU32) })
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

    fn configuration_address(&self, function: PciFunction) -> Result<(&Ecam, Address), HostError> {
        let ecam = self
            .configuration
            .as_ref()
            .ok_or(HostError::UnsupportedSpace)?;
        if function.segment != 0 {
            return Err(HostError::OutOfRange);
        }
        let address = Address::new(function.bus, function.device, function.function)
            .ok_or(HostError::OutOfRange)?;
        Ok((ecam, address))
    }
}

impl Host for KernelHost {
    fn acquire_global_lock(&mut self, timeout: u64) -> bool {
        use core::sync::atomic::Ordering as AtomicOrdering;
        let Some(word) = self.lock_word() else {
            return true;
        };
        let mut waited = 0u64;
        loop {
            let previous = word
                .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |old| {
                    let owned = old & LOCK_OWNED != 0;
                    Some(old & !LOCK_PENDING | LOCK_OWNED | if owned { LOCK_PENDING } else { 0 })
                })
                .unwrap_or(LOCK_OWNED);
            if previous & LOCK_OWNED == 0 {
                return true;
            }
            if timeout != 0xffff && waited >= timeout {
                return false;
            }
            self.sleep(1);
            waited += 1;
        }
    }

    fn release_global_lock(&mut self) {
        use core::sync::atomic::Ordering as AtomicOrdering;
        let Some(word) = self.lock_word() else {
            return;
        };
        let previous = word
            .fetch_update(AtomicOrdering::AcqRel, AtomicOrdering::Acquire, |old| {
                Some(old & !(LOCK_PENDING | LOCK_OWNED))
            })
            .unwrap_or(0);
        if previous & LOCK_PENDING != 0
            && let Some((_, control)) = self.global_lock
        {
            let value = (self.read_port)(control, 16).unwrap_or(0);
            (self.write_port)(control, 16, value | GLOBAL_LOCK_RELEASED);
        }
    }

    fn table_bytes(
        &mut self,
        signature: &[u8],
        oem_id: &[u8],
        oem_table_id: &[u8],
    ) -> Option<Vec<u8>> {
        let signature: [u8; 4] = signature.get(..4)?.try_into().ok()?;
        let padded = |text: &[u8], length: usize| {
            let mut field = vec![b' '; length];
            for (slot, &byte) in field.iter_mut().zip(text) {
                *slot = byte;
            }
            field
        };
        let (oem_id, oem_table_id) = (padded(oem_id, 6), padded(oem_table_id, 8));
        let mut found = None;
        (self.tables)(&signature, &mut |table| {
            let matches = |range: core::ops::Range<usize>, wanted: &[u8]| {
                table.get(range).is_some_and(|field| {
                    wanted.iter().all(|&byte| byte == b' ') || field == wanted
                })
            };
            if found.is_none() && matches(10..16, &oem_id) && matches(16..24, &oem_table_id) {
                found = Some(table.to_vec());
            }
        });
        found
    }

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
                let bytes = u64::from(width / 8);
                let burst = bytes > 1 && controller.begin_burst().unwrap_or(false);
                let mut value = 0;
                let mut result = Ok(());
                for byte in 0..bytes {
                    let Ok(at) = u8::try_from(address + byte) else {
                        result = Err(HostError::OutOfRange);
                        break;
                    };
                    match controller.read(at) {
                        Ok(read) => value |= u64::from(read) << (8 * byte),
                        Err(_) => {
                            result = Err(HostError::NoResponse);
                            break;
                        }
                    }
                }
                if burst {
                    let _ = controller.end_burst();
                }
                result.map(|()| value)
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

struct Loaded {
    interpreter: Interpreter<KernelHost>,
    absent: Vec<Path>,
    events: Option<Events>,
}

struct Events {
    /// Each GPE block with the scope its `_Lxx` and `_Exx` methods live in: `\_GPE` for the
    /// FADT's blocks, the device for a GPE block device (`ACPI0006`, D556).
    blocks: Vec<(GpeBlock, Path)>,
    controller: Option<(Path, Option<u16>)>,
    power_buttons: Vec<Path>,
    /// The virtual machine generation counter (Microsoft's VM Generation ID, which QEMU and
    /// Hyper-V give) and the physical address of its 16-byte id, if the machine has one (D553).
    generation_counter: Option<(Path, u64)>,
}

/// The machine's power state as ACPI tells it (D556): the lid, the AC adapter, each battery's
/// charge and each thermal zone's temperature, read at start and again when the device notifies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PowerState {
    pub lid_open: Option<bool>,
    pub on_line_power: Option<bool>,
    /// Each battery's remaining capacity in percent and whether it is charging.
    pub batteries: Vec<(u8, bool)>,
    /// Each thermal zone's temperature in tenths of a kelvin (`_TMP`).
    pub temperatures: Vec<u64>,
    /// Whether a dock is attached (`_STA` of a `_DCK` device).
    pub docked: Option<bool>,
}

static POWER: crate::sync::LeafSpinLock<Option<PowerState>> = crate::sync::LeafSpinLock::new_leaf(None);

/// `/proc/driver/power`'s text (D556): `lid open|closed`, `ac online|offline`, one `battery <n>
/// <percent>% charging|discharging` line each, one `thermal <n> <celsius>.<tenth>` line each and
/// `dock attached|detached`, for what the machine has; `none` without ACPI power devices.
pub fn power_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut text = alloc::string::String::new();
    let Some(state) = POWER.lock().clone() else {
        return alloc::string::String::from("none");
    };
    if let Some(open) = state.lid_open {
        let _ = writeln!(text, "lid {}", if open { "open" } else { "closed" });
    }
    if let Some(online) = state.on_line_power {
        let _ = writeln!(text, "ac {}", if online { "online" } else { "offline" });
    }
    for (index, (percent, charging)) in state.batteries.iter().enumerate() {
        let doing = if *charging { "charging" } else { "discharging" };
        let _ = writeln!(text, "battery {index} {percent}% {doing}");
    }
    for (index, tenths) in state.temperatures.iter().enumerate() {
        let celsius = (*tenths as i64) - 2732;
        let _ = writeln!(text, "thermal {index} {}.{}", celsius / 10, (celsius % 10).abs());
    }
    if let Some(docked) = state.docked {
        let _ = writeln!(text, "dock {}", if docked { "attached" } else { "detached" });
    }
    if text.is_empty() {
        text.push_str("none");
    }
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// Reads the power devices again: lids (`PNP0C0D`, `_LID`), AC adapters (`ACPI0003`, `_PSR`),
/// batteries (`PNP0C0A`, `_BST` against `_BIF`'s last full capacity), thermal zones (`_TMP`) and
/// docks (a device with `_DCK`, its `_STA`).
fn read_power(interpreter: &mut Interpreter<KernelHost>) -> PowerState {
    let mut state = PowerState::default();
    let integer = |interpreter: &mut Interpreter<KernelHost>, path: &Path, name: &[u8; 4]| {
        match interpreter.evaluate_child(path, name, &[]) {
            Ok(Some(value)) => interpreter.to_integer(&value).ok(),
            _ => None,
        }
    };
    for lid in interpreter.devices_with_id(&[b"PNP0C0D"]) {
        state.lid_open = integer(interpreter, &lid, b"_LID").map(|value| value != 0);
    }
    for adapter in interpreter.devices_with_string_id(b"ACPI0003") {
        state.on_line_power = integer(interpreter, &adapter, b"_PSR").map(|value| value != 0);
    }
    for battery in interpreter.devices_with_id(&[b"PNP0C0A"]) {
        let full = match interpreter.evaluate_child(&battery, b"_BIF", &[]) {
            Ok(Some(Object::Package(fields))) => fields
                .get(2)
                .and_then(|field| interpreter.to_integer(field).ok()),
            _ => None,
        };
        let Ok(Some(Object::Package(status))) = interpreter.evaluate_child(&battery, b"_BST", &[])
        else {
            continue;
        };
        let charging = status
            .first()
            .and_then(|field| interpreter.to_integer(field).ok())
            .is_some_and(|flags| flags & 2 != 0);
        let remaining = status
            .get(2)
            .and_then(|field| interpreter.to_integer(field).ok());
        if let (Some(full), Some(remaining)) = (full.filter(|&full| full != 0 && full != u64::from(u32::MAX)), remaining) {
            state.batteries.push(((remaining * 100 / full).min(100) as u8, charging));
        }
    }
    let zones: Vec<Path> = interpreter
        .namespace()
        .iter()
        .filter(|(_, node)| matches!(interpreter.namespace().get(*node), Some(Object::ThermalZone)))
        .map(|(path, _)| path.clone())
        .collect();
    for zone in zones {
        if let Some(tenths) = integer(interpreter, &zone, b"_TMP") {
            state.temperatures.push(tenths);
        }
    }
    let docks: Vec<Path> = interpreter
        .namespace()
        .iter()
        .filter(|(path, _)| path.last() == Some(NameSeg::new(b"_DCK")))
        .filter_map(|(path, _)| path.parent())
        .collect();
    for dock in docks {
        state.docked = integer(interpreter, &dock, b"_STA").map(|status| status & 1 != 0);
    }
    state
}

/// Notifications that ask the OS to read a power device again: a status change (0x80) and a
/// battery's information change (0x81).
const STATUS_CHANGED: u64 = 0x80;
/// A dock asks the OS to look at it (`Notify(…, 0)`, device check).
const DOCK_CHECK: u64 = 0x00;
const INFORMATION_CHANGED: u64 = 0x81;

/// The notification a VM generation counter sends when its id changes: the machine was
/// restored from a snapshot or cloned.
const GENERATION_CHANGED: u64 = 0x80;

/// Finds the VM generation counter (`_CID` `VM_Gen_Counter`, `_HID` `QEMUVGID` on QEMU) and its
/// id's address, from its `ADDR` method's two 32-bit halves.
fn generation_counter(interpreter: &mut Interpreter<KernelHost>) -> Option<(Path, u64)> {
    let path = [b"VM_Gen_Counter".as_slice(), b"QEMUVGID".as_slice()]
        .into_iter()
        .find_map(|id| interpreter.devices_with_string_id(id).into_iter().next())?;
    let Ok(Some(Object::Package(halves))) = interpreter.evaluate_child(&path, b"ADDR", &[]) else {
        return None;
    };
    let low = interpreter.to_integer(halves.first()?).ok()?;
    let high = interpreter.to_integer(halves.get(1)?).ok()?;
    Some((path, high << 32 | low & 0xffff_ffff))
}

/// Hashes the VM generation id at `address` into the random number generator's input and
/// reseeds it, so a snapshot restored twice never repeats the generator's output (D553).
fn mix_generation_id<P: Platform>(address: u64) {
    let virt = (address as usize).wrapping_add(P::direct_map_offset()) as *const [u8; 16];
    // SAFETY: the firmware reserves the id's 16 bytes in RAM, which the direct map maps; they
    // are only read.
    let id = unsafe { virt.read_volatile() };
    crate::random::add_user_input(&id);
    crate::random::reseed::<P>();
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
        let _ = loaded
            .interpreter
            .evaluate_child(&Path::root(), b"_PTS", &off);
    }
}

/// The last notifications AML sent (`Notify`), oldest first: the object's path and the value.
pub fn notifications() -> Vec<(Path, u64)> {
    match NAMESPACE.lock().as_mut() {
        Some(loaded) => loaded
            .interpreter
            .host()
            .notifications
            .iter()
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Sets ACPI events up and starts the thread that handles them (D539): the embedded controller
/// and its `_REG`, every general-purpose event with a method turned on, and the system control
/// interrupt routed. Once the scheduler runs; an error says why there are no events.
pub fn start_events<P: Platform>() -> Result<(), &'static str> {
    let fadt = P::acpi_table(b"FACP").ok_or("the firmware gives no FADT")?;
    let general = Path::root().child(NameSeg::new(b"_GPE"));
    let mut blocks: Vec<(GpeBlock, Path)> = eggos_hal::acpi::gpe_blocks(fadt)
        .map(|block| (block, general.clone()))
        .collect();
    {
        let mut namespace = NAMESPACE.lock();
        let loaded = namespace.as_mut().ok_or("no ACPI namespace")?;
        let interpreter = &mut loaded.interpreter;
        for device in interpreter.devices_with_string_id(b"ACPI0006") {
            let Ok(Some(Object::Buffer(template))) = interpreter.evaluate_child(&device, b"_CRS", &[])
            else {
                continue;
            };
            if let Some(&(port, length)) = eggos_acpi::resources::io_ranges(&template).first() {
                blocks.push((GpeBlock { port, length, base: 0 }, device));
            }
        }
        let controller = set_up_controller::<P>(interpreter);
        let controller_event = controller.as_ref().and_then(|(_, event)| *event);
        for (block, scope) in &blocks {
            for index in 0..block.events() {
                let number = block.base + index;
                let controller = *scope == general && controller_event == Some(number);
                if controller || event_method(interpreter, scope, number).is_some() {
                    set_event_enabled::<P>(block, index, true);
                }
            }
        }
        let power_buttons = interpreter.devices_with_id(&[b"PNP0C0C"]);
        let generation_counter = generation_counter(interpreter);
        loaded.events = Some(Events {
            blocks,
            controller,
            power_buttons,
            generation_counter,
        });
    }
    if let Some((_, address)) = NAMESPACE
        .lock()
        .as_ref()
        .and_then(|loaded| loaded.events.as_ref())
        .and_then(|events| events.generation_counter.clone())
    {
        mix_generation_id::<P>(address);
    }
    if let Some(loaded) = NAMESPACE.lock().as_mut() {
        let state = read_power(&mut loaded.interpreter);
        *POWER.lock() = Some(state);
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

/// The embedded controller the firmware's ECDT describes (ACPI 6.5, 5.2.16): its command and
/// data ports, its event's number and its namespace path, for the controller's region before
/// (or without) its `_CRS` (D556).
fn boot_controller<P: Platform>() -> Option<(u16, u16, u16, Path)> {
    const CONTROL_ADDRESS: usize = 36 + 4;
    const DATA_ADDRESS: usize = 48 + 4;
    const GPE_BIT: usize = 64;
    const ID: usize = 65;
    let table = P::acpi_table(b"ECDT")?;
    let port = |at: usize| -> Option<u16> {
        let bytes: [u8; 2] = table.get(at..at + 2)?.try_into().ok()?;
        Some(u16::from_le_bytes(bytes))
    };
    let (command, data) = (port(CONTROL_ADDRESS)?, port(DATA_ADDRESS)?);
    let event = u16::from(*table.get(GPE_BIT)?);
    let id = table.get(ID..)?;
    let id = &id[..id.iter().position(|&byte| byte == 0).unwrap_or(id.len())];
    Some((data, command, event, Path::parse(id)?))
}

fn set_up_controller<P: Platform>(
    interpreter: &mut Interpreter<KernelHost>,
) -> Option<(Path, Option<u16>)> {
    if let Some((data, command, event, path)) = boot_controller::<P>() {
        let host = interpreter.host();
        host.controller = Some(ControllerPorts {
            data,
            command,
            read_port: host.read_port,
            write_port: host.write_port,
        });
        if interpreter.devices_with_id(&[b"PNP0C09"]).is_empty() {
            return Some((path, Some(event)));
        }
    }
    let path = interpreter
        .devices_with_id(&[b"PNP0C09"])
        .into_iter()
        .next()?;
    let Ok(Some(Object::Buffer(template))) = interpreter.evaluate_child(&path, b"_CRS", &[]) else {
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

fn event_method(
    interpreter: &Interpreter<KernelHost>,
    scope: &Path,
    number: u16,
) -> Option<(Path, bool)> {
    if number >= HANDLED_EVENTS {
        return None;
    }
    [(b'E', true), (b'L', false)]
        .into_iter()
        .find_map(|(kind, edge)| {
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

fn handle_events<P: Platform>() {
    let (notified, power_buttons, generation_counter) = {
        let mut namespace = NAMESPACE.lock();
        let Some(loaded) = namespace.as_mut() else {
            return;
        };
        let Some(events) = loaded.events.take() else {
            return;
        };
        let general = Path::root().child(NameSeg::new(b"_GPE"));
        for (block, scope) in &events.blocks {
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
                        Some((path, Some(event))) if *event == number && *scope == general => {
                            clear();
                            answer_queries(interpreter, path);
                            continue;
                        }
                        _ => {}
                    }
                    match event_method(interpreter, scope, number) {
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
        let generation_counter = events.generation_counter.clone();
        loaded.events = Some(events);
        (notified, power_buttons, generation_counter)
    };
    if notified
        .iter()
        .any(|(path, value)| matches!(*value, STATUS_CHANGED | INFORMATION_CHANGED | DOCK_CHECK) && !power_buttons.contains(path))
        && let Some(loaded) = NAMESPACE.lock().as_mut()
    {
        let state = read_power(&mut loaded.interpreter);
        *POWER.lock() = Some(state);
    }
    for (path, value) in notified {
        if value == POWER_BUTTON_PRESSED && power_buttons.contains(&path) {
            crate::linux::reboot::power_button::<P>();
        }
        if let Some((counter, address)) = &generation_counter
            && value == GENERATION_CHANGED
            && *counter == path
        {
            mix_generation_id::<P>(*address);
        }
    }
}

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

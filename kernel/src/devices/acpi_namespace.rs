//! The ACPI namespace the kernel runs (D538): the DSDT and every SSDT loaded into the
//! `eggos-acpi` interpreter at boot, their system-memory regions mapped, the firmware told the
//! interrupt model (`\_PIC`), and every present device's `_INI` run by `_STA`. Devices `_STA`
//! says are absent are left out of the device table, and `\_PTS(5)` runs before the machine
//! powers off.
//!
//! The interpreter's host reaches system memory only inside the regions mapped at boot (device
//! windows are mapped on the boot CPU alone), I/O ports through the HAL, and PCI configuration
//! space through segment 0's ECAM window. `Sleep` waits on the clock, since the interpreter runs
//! at boot before the scheduler and at power-off after it.

use crate::sync::{SleepLock, Sleeping};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use eggos_acpi::{Error, Host, HostError, Interpreter, Object, Path, PciFunction, RegionSpace};
use eggos_drivers::io::{Mmio, RegisterIo};
use eggos_drivers::pci::{Address, ConfigSpace, ecam::Ecam};
use eggos_hal::{PhysicalRange, Platform};

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
        }
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
    }
}

/// The namespace once loaded, and the devices `_STA` says are absent.
struct Loaded {
    interpreter: Interpreter<KernelHost>,
    absent: Vec<Path>,
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

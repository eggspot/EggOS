//! I2C controllers and the HID devices on them (D462). An ACPI device compatible with HID over
//! I2C (`PNP0C50`) whose connection names its controller ([`I2cTarget`]) brings that
//! controller up, a DesignWare bus master shared by every device on the same bus, and starts
//! the device. Its accelerometer is then polled every [`POLL_INTERVAL_MS`] as one of the
//! kernel's input devices, so the screen turns with it (D435). A device that reports only on
//! its interrupt line is refused until GPIO interrupts are supported.

use super::acpi_bus::{I2cController, I2cTarget};
use super::{
    Deadline, DeviceId, DeviceTable, DriverDescriptor, Identity, Match, Probe, ProbeError,
    ProbePass,
};
use crate::input::InputDevice;
use crate::sync::{SleepLock, Sleeping, SpinLock};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_drivers::i2c::{DesignWare, Speed, designware};
use eggos_drivers::input::i2c_hid::I2cHid;
use eggos_drivers::input::{Capabilities, RawEvent};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_hal::PhysicalRange;

/// How often a sensor is asked for its readings.
pub const POLL_INTERVAL_MS: u64 = 100;
const SETUP_TIMEOUT_MS: u64 = 200;
const TRANSFER_TIMEOUT_MS: u64 = 20;
const INTEL: u16 = 0x8086;
const INTEL_LPSS_CLOCK_HZ: u32 = 120_000_000;
const OTHER_CLOCK_HZ: u32 = 100_000_000;
const INTEL_LPSS_BLOCK_BYTES: usize = 0x1000;

/// HID over I2C devices (`PNP0C50`, and `ACPI0C50` as some firmware writes it).
pub static I2C_HID_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "i2c-hid",
    pass: ProbePass::Device,
    matches: &[Match::AcpiHid(b"PNP0C50"), Match::AcpiHid(b"ACPI0C50")],
    probe: probe_i2c_hid,
};

type Controller = Arc<SleepLock<DesignWare<Mmio>>>;

static CONTROLLERS: SpinLock<Vec<(u64, Controller)>> = SpinLock::new(Vec::new());

struct I2cSensor {
    controller: Controller,
    device: I2cHid,
    next_poll: Deadline,
}

impl InputDevice for I2cSensor {
    fn capabilities(&self) -> Capabilities {
        self.device.capabilities()
    }

    fn poll(&mut self, _budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
        if self.next_poll.keep_waiting() {
            return 0;
        }
        self.next_poll = self.next_poll.again_after(POLL_INTERVAL_MS);
        let Some(mut bus) = self.controller.try_lock() else {
            return 0;
        };
        let mut deadline = self.next_poll.again_after(TRANSFER_TIMEOUT_MS);
        self.device
            .poll(&mut *bus, &mut || deadline.keep_waiting(), event)
            .unwrap_or(0)
    }
}

fn pci_function(
    table: &DeviceTable,
    device: u8,
    function: u8,
) -> Option<(pci::Function, PhysicalRange)> {
    table.iter().find_map(|(_, node)| match node.identity {
        Identity::Pci(found)
            if found.address.bus == 0
                && found.address.device == device
                && found.address.function == function =>
        {
            Some((found, node.regions[0]))
        }
        _ => None,
    })
}

fn controller_for(probe: &mut dyn Probe, target: &I2cTarget) -> Result<Controller, ProbeError> {
    let (region, intel) = match target.controller {
        I2cController::Registers(region) => (region, false),
        I2cController::PciSlot { device, function } => {
            let (found, region) = pci_function(probe.table(), device, function)
                .ok_or(ProbeError::Refused("the I2C controller's PCI function is missing"))?;
            let config = probe
                .pci_config()
                .ok_or(ProbeError::Refused("no PCI configuration space"))?;
            pci::enable_memory_and_bus_master(config, found.address);
            (region, found.vendor_id == INTEL)
        }
    };
    if region.is_empty() {
        return Err(ProbeError::Refused("the I2C controller's registers are not in memory space"));
    }
    let known = CONTROLLERS
        .lock()
        .iter()
        .find(|(start, _)| *start == region.start)
        .map(|(_, controller)| Arc::clone(controller));
    if let Some(controller) = known {
        return Ok(controller);
    }
    let length = usize::try_from(region.len())
        .map_err(|_| ProbeError::Refused("the I2C controller's register block is too large"))?;
    let base = probe.map_window(region)?;
    // SAFETY: the region is the controller's register block, as its PCI BAR 0 or its own `_CRS`
    // names it, mapped as device memory by `map_window` for as long as the kernel runs; only
    // this driver drives the controller, through the one value kept in `CONTROLLERS`.
    let regs = unsafe { Mmio::new(base, length) };
    if intel && length >= INTEL_LPSS_BLOCK_BYTES {
        designware::release_intel_reset(&regs);
    }
    let clock = match intel {
        true => INTEL_LPSS_CLOCK_HZ,
        false => OTHER_CLOCK_HZ,
    };
    let mut deadline = probe.deadline(SETUP_TIMEOUT_MS);
    let bus = DesignWare::new(
        regs,
        clock,
        Speed::of_hz(target.speed_hz),
        &mut || deadline.keep_waiting(),
    )
    .map_err(|error| ProbeError::Refused(error.description()))?;
    let controller = Arc::new(SleepLock::new(bus, Sleeping::spinning()));
    let mut controllers = CONTROLLERS.lock();
    if controllers.try_reserve(1).is_ok() {
        controllers.push((region.start, Arc::clone(&controller)));
    }
    Ok(controller)
}

fn probe_i2c_hid(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Acpi {
        i2c: Some(target), ..
    } = node.identity
    else {
        return Err(ProbeError::Refused(
            "no I2C connection the ACPI namespace names as data",
        ));
    };
    let controller = controller_for(probe, &target)?;
    let mut deadline = probe.deadline(SETUP_TIMEOUT_MS);
    let device = {
        let mut bus = controller.lock();
        I2cHid::start(
            &mut *bus,
            target.address,
            target.ten_bit,
            target.descriptor_register,
            &mut || deadline.keep_waiting(),
        )
    }
    .map_err(|error| ProbeError::Refused(error.description()))?;
    if !device.has_polled_reports() {
        return Err(ProbeError::Refused(
            "an I2C HID device that needs its interrupt line (no GPIO interrupts yet)",
        ));
    }
    let sensor = Box::new(I2cSensor {
        controller,
        device,
        next_poll: probe.deadline(POLL_INTERVAL_MS),
    });
    crate::input::install(sensor)
        .map_err(|_| ProbeError::Refused("no room for another input device"))
}

//! Message-signalled interrupts for the PCI drivers of real machines (D255): MSI-X entry 0 where
//! the function has MSI-X, else MSI with one vector, and legacy INTx turned off, so a device's
//! interrupt raises its driver's [`DeviceEvents`]. Message interrupts need no acknowledging at
//! an interrupt controller; each driver clears its device's own cause when it looks.

use super::interrupts::{self, DeviceEvents, Handler};
use super::{DeviceId, Identity, Probe, ProbeError};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci::{msi, msix, msix::MsixTable};
use eggos_hal::PhysicalRange;

const COMMAND: u16 = 0x04;
const COMMAND_INTX_DISABLE: u32 = 1 << 10;

fn acknowledge_nothing(_: usize) {}

/// Connects the PCI function `id`'s interrupt to `events`; the driver then asks the device for
/// interrupts and looks at it once, for what came before.
pub fn connect(probe: &mut dyn Probe, id: DeviceId, events: &'static DeviceEvents) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let found_msix = msix::find(config, function.address);
    let found_msi = msi::find(config, function.address);
    let (line, message) = probe.message_interrupt()?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    if let Some(capability) = found_msix {
        let bar = node.regions[usize::from(capability.bar)];
        let table_end = u64::from(capability.table_offset) + capability.table_bytes() as u64;
        if bar.is_empty() || table_end > bar.len() {
            return Err(ProbeError::Refused("the MSI-X table lies outside its BAR"));
        }
        let table = probe.map_window(PhysicalRange::from_start_length(
            bar.start + u64::from(capability.table_offset),
            capability.table_bytes() as u64,
        ))?;
        // SAFETY: `map_window` mapped the table as device memory for good, inside the BAR the
        // capability names; only this driver reaches it.
        let table = MsixTable::new(unsafe { Mmio::new(table, capability.table_bytes()) }, capability.entries);
        table.set(0, message.address, message.data);
        let config = probe
            .pci_config()
            .ok_or(ProbeError::Refused("no PCI configuration space"))?;
        capability.enable(config, function.address);
    } else if let Some(capability) = found_msi {
        if !capability.enable(config, function.address, message.address, message.data as u16) {
            return Err(ProbeError::Refused("the function's MSI takes only 32-bit addresses"));
        }
    } else {
        return Err(ProbeError::Refused("the function has neither MSI-X nor MSI"));
    }
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let command = config.read32(function.address, COMMAND);
    config.write32(function.address, COMMAND, command | COMMAND_INTX_DISABLE);
    let handler = Handler {
        acknowledge: acknowledge_nothing,
        context: 0,
        events,
    };
    interrupts::connect(line, handler).map_err(ProbeError::Refused)?;
    if let Some(node) = probe.table().get_mut(id) {
        node.delivery = super::Delivery::Interrupts;
    }
    Ok(())
}

/// A wait for a device: sleeping until its interrupt raises `events` when it has one, polling
/// otherwise, until `deadline`.
pub fn waiting_for<'a>(
    events: Option<&'static DeviceEvents>,
    deadline: &'a mut super::Deadline,
) -> impl FnMut() -> bool + 'a {
    let mut seen = events.map_or(0, DeviceEvents::raised);
    move || match events {
        Some(events) => deadline.sleep_until_raised(events, &mut seen),
        None => deadline.keep_waiting(),
    }
}

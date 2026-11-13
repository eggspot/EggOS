//! Message-signalled interrupts for the PCI drivers of real machines (D255): MSI-X entry 0 where
//! the function has MSI-X, else MSI with one vector, and legacy INTx turned off, so a device's
//! interrupt raises its driver's [`DeviceEvents`]; a function with neither uses its INTx pin
//! through the I/O APIC (D413). Message interrupts need no acknowledging at
//! an interrupt controller; each driver clears its device's own cause when it looks.

use super::interrupts::{self, DeviceEvents, Handler, MessageTable};
use super::{DeviceId, Identity, Probe, ProbeError};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci::{msi, msix, msix::MsixTable};
use eggos_hal::PhysicalRange;

const COMMAND: u16 = 0x04;
const COMMAND_INTX_DISABLE: u32 = 1 << 10;
const INTERRUPT_LINE: u16 = 0x3c;
const NO_INTERRUPT_LINE: u32 = 0xff;

fn acknowledge_nothing(_: usize) {}

/// Connects the PCI function `id`'s interrupt to `events`: MSI-X, else MSI, else its legacy
/// INTx pin through the input its interrupt line register names (D413); the driver then asks
/// the device for interrupts and looks at it once, for what came before. A line routed for a
/// connection that then fails is given back.
pub fn connect(
    probe: &mut dyn Probe,
    id: DeviceId,
    events: &'static DeviceEvents,
) -> Result<(), ProbeError> {
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
    if found_msix.is_none() && found_msi.is_none() {
        return connect_pin(probe, id, function.address, events);
    }
    let (line, message) = probe.message_interrupt()?;
    let enabled = enable_message(
        probe,
        node,
        function.address,
        (found_msix, found_msi),
        message,
    );
    let table = match enabled {
        Ok(table) => table,
        Err(refused) => {
            probe.release_interrupt(line);
            return Err(refused);
        }
    };
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let command = config.read32(function.address, COMMAND);
    config.write32(function.address, COMMAND, command | COMMAND_INTX_DISABLE);
    if table.is_none()
        && let Some(capability) = found_msi
    {
        interrupts::record_msi_function(
            line,
            interrupts::MsiFunction {
                address: function.address,
                capability,
            },
        );
    }
    finish(probe, id, line, events, table)
}

fn enable_message(
    probe: &mut dyn Probe,
    node: super::DeviceNode,
    address: eggos_drivers::pci::Address,
    (found_msix, found_msi): (Option<msix::Msix>, Option<msi::Msi>),
    message: eggos_hal::InterruptMessage,
) -> Result<Option<MessageTable>, ProbeError> {
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
        let registers = unsafe { Mmio::new(table, capability.table_bytes()) };
        let kept = MessageTable {
            address: table,
            bytes: capability.table_bytes(),
            entries: capability.entries,
            entry: 0,
        };
        let table = MsixTable::new(registers, capability.entries);
        table.set(0, message.address, message.data);
        let config = probe
            .pci_config()
            .ok_or(ProbeError::Refused("no PCI configuration space"))?;
        capability.enable(config, address);
        return Ok(Some(kept));
    }
    let Some(capability) = found_msi else {
        return Err(ProbeError::Refused(
            "the function has neither MSI-X nor MSI",
        ));
    };
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    match capability.enable(config, address, message.address, message.data as u16) {
        true => Ok(None),
        false => Err(ProbeError::Refused(
            "the function's MSI takes only 32-bit addresses",
        )),
    }
}

fn connect_pin(
    probe: &mut dyn Probe,
    id: DeviceId,
    address: eggos_drivers::pci::Address,
    events: &'static DeviceEvents,
) -> Result<(), ProbeError> {
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let pin_and_line = config.read32(address, INTERRUPT_LINE);
    let input = pin_and_line & 0xff;
    let pin = pin_and_line >> 8 & 0xff;
    if pin == 0 || input == 0 || input == NO_INTERRUPT_LINE {
        return Err(ProbeError::Refused(
            "the function has no MSI-X, MSI or routed INTx pin",
        ));
    }
    let line = probe.legacy_interrupt(input)?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let command = config.read32(address, COMMAND);
    config.write32(address, COMMAND, command & !COMMAND_INTX_DISABLE);
    events.set_masked_line(line);
    finish(probe, id, line, events, None)
}

fn finish(
    probe: &mut dyn Probe,
    id: DeviceId,
    line: eggos_hal::DeviceLine,
    events: &'static DeviceEvents,
    table: Option<MessageTable>,
) -> Result<(), ProbeError> {
    let handler = Handler {
        acknowledge: acknowledge_nothing,
        context: 0,
        events,
    };
    let connected = match table {
        Some(table) => interrupts::connect_message(line, handler, table),
        None => interrupts::connect(line, handler),
    };
    if let Err(refused) = connected {
        probe.release_interrupt(line);
        return Err(ProbeError::Refused(refused));
    }
    probe.interrupt_connected(line);
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

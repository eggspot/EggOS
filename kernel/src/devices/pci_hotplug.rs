//! PCI Express native hot-plug (D567): at boot the ports whose slots take cards while the
//! machine runs are found, their memory windows mapped once (so a card's registers need no new
//! mapping later, when only the boot CPU may make them), and a thread watches the slots. A
//! card that arrives has its functions found, their BARs placed in the port's windows, added to
//! the device table and offered to the drivers; one that leaves has its devices marked gone,
//! which their drivers see (their registers read all ones).

use super::window;
use super::{
    Binding, Deadline, DeviceId, DeviceNode, DeviceTable, DmaCoherence, Identity, Probe,
    ProbeError, BUILTIN, DEVICES,
};
use crate::memory::Memory;
use crate::sync::LeafSpinLock;
use alloc::vec::Vec;
use core::time::Duration;
use eggos_drivers::pci::hotplug::{HotPlugPort, WindowAllocator};
use eggos_drivers::pci::{self, ConfigSpace};
use eggos_hal::{DeviceLine, InterruptMessage, PhysicalRange, Platform};

/// How often the slots are looked at.
pub const WATCH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy)]
struct MappedWindow {
    start: u64,
    end: u64,
    virt: usize,
}

#[derive(Debug)]
struct Port {
    port: HotPlugPort,
    windows: [Option<MappedWindow>; 2],
    allocator: WindowAllocator,
    present: bool,
}

static PORTS: LeafSpinLock<Vec<Port>> = LeafSpinLock::new_leaf(Vec::new());

/// Finds the hot-plug ports among the PCI functions of `table`, maps their windows through
/// `map` and enables their slots' change reports; the allocator of each starts after the BARs
/// of the cards already there. Run once at boot, after probing.
pub fn prepare(
    config: &dyn ConfigSpace,
    table: &DeviceTable,
    map: &mut dyn FnMut(PhysicalRange) -> Option<usize>,
) -> usize {
    let mut ports = PORTS.lock();
    for (_, node) in table.iter() {
        let Identity::Pci(function) = node.identity else {
            continue;
        };
        if function.header_type != 1 {
            continue;
        }
        let Some(port) = HotPlugPort::find(config, function.address) else {
            continue;
        };
        port.enable(config);
        let windows = port.windows(config);
        let mut allocator = WindowAllocator::new(windows);
        for (_, behind) in table.iter() {
            let Identity::Pci(card) = behind.identity else {
                continue;
            };
            if card.address.bus != port.secondary_bus {
                continue;
            }
            for region in behind.regions.iter().filter(|region| !region.is_empty()) {
                allocator.note_used(region.start, region.end);
            }
        }
        let mapped = windows.map(|window| {
            let (start, end) = window?;
            let virt = map(PhysicalRange { start, end })?;
            Some(MappedWindow { start, end, virt })
        });
        let present = port.take_events(config).present;
        if ports.try_reserve(1).is_ok() {
            ports.push(Port {
                port,
                windows: mapped,
                allocator,
                present,
            });
        }
    }
    ports.len()
}

/// Starts the thread that watches the hot-plug slots, when [`prepare`] found any.
pub fn start<P: Platform>() {
    if PORTS.lock().is_empty() {
        return;
    }
    let _ = crate::thread::spawn::<P, _>(watch::<P>);
}

fn watch<P: Platform>() {
    loop {
        crate::thread::sleep::<P>(WATCH_INTERVAL);
        look::<P>();
    }
}

fn look<P: Platform>() {
    let Some(config) = super::interrupts::kept_configuration() else {
        return;
    };
    let count = PORTS.lock().len();
    for index in 0..count {
        let (arrived, left) = {
            let mut ports = PORTS.lock();
            let Some(port) = ports.get_mut(index) else {
                break;
            };
            let events = port.port.take_events(config);
            let now = events.present && events.link_up;
            let arrived = now && !port.present;
            let left = !events.present && port.present;
            port.present = now || (port.present && events.present);
            (arrived, left)
        };
        if arrived {
            arrive::<P>(config, index);
        }
        if left {
            leave(index);
        }
    }
}

fn arrive<P: Platform>(config: &'static dyn ConfigSpace, index: usize) {
    let (bus, mut allocator, windows) = {
        let ports = PORTS.lock();
        let Some(port) = ports.get(index) else {
            return;
        };
        (port.port.secondary_bus, port.allocator, port.windows)
    };
    let mut found = Vec::new();
    pci::enumerate(config, bus, bus, &mut |function| {
        if found.try_reserve(1).is_ok() {
            found.push(function);
        }
    });
    let mut devices = DEVICES.lock();
    let Some(table) = devices.as_deref_mut() else {
        return;
    };
    for function in found {
        let bars = allocator.assign_function(config, function.address, function.header_type);
        let mut device = DeviceNode::new(Identity::Pci(function));
        device.dma = DmaCoherence::Coherent;
        for bar in bars.into_iter().flatten() {
            device.regions[usize::from(bar.index)] =
                PhysicalRange::from_start_length(bar.address, bar.size);
        }
        let _ = table.add(device);
    }
    if let Some(port) = PORTS.lock().get_mut(index) {
        port.allocator = allocator;
    }
    let mut probe = HotProbe::<P> {
        table,
        config,
        windows,
        platform: core::marker::PhantomData,
    };
    super::probe_all(&mut probe, BUILTIN);
}

fn leave(index: usize) {
    let bus = match PORTS.lock().get(index) {
        Some(port) => port.port.secondary_bus,
        None => return,
    };
    let mut devices = DEVICES.lock();
    let Some(table) = devices.as_deref_mut() else {
        return;
    };
    let gone: Vec<DeviceId> = table
        .iter()
        .filter_map(|(id, node)| match node.identity {
            Identity::Pci(function) if function.address.bus == bus => Some(id),
            _ => None,
        })
        .collect();
    for id in gone {
        if let Some(node) = table.get_mut(id) {
            node.binding = Binding::Refused("pci-hotplug", "the card was taken out");
        }
    }
}

struct HotProbe<'a, P: Platform> {
    table: &'a mut DeviceTable,
    config: &'static dyn ConfigSpace,
    windows: [Option<MappedWindow>; 2],
    platform: core::marker::PhantomData<P>,
}

impl<P: Platform> Probe for HotProbe<'_, P> {
    fn table(&mut self) -> &mut DeviceTable {
        self.table
    }

    fn map_window(&mut self, region: PhysicalRange) -> Result<usize, ProbeError> {
        self.windows
            .iter()
            .flatten()
            .find(|window| window.start <= region.start && region.end <= window.end)
            .map(|window| window.virt + (region.start - window.start) as usize)
            .ok_or(ProbeError::Refused("the registers lie outside the port's mapped windows"))
    }

    fn pci_config(&self) -> Option<&dyn ConfigSpace> {
        Some(self.config)
    }

    fn deadline(&self, milliseconds: u64) -> Deadline {
        Deadline::after::<P>(milliseconds)
    }

    fn message_interrupt(&mut self) -> Result<(DeviceLine, InterruptMessage), ProbeError> {
        P::message_interrupt().map_err(ProbeError::Refused)
    }

    fn interrupt_connected(&mut self, line: DeviceLine) {
        super::interrupts::follow::<P>(line);
    }

    fn device_tree(&self) -> Option<&'static [u8]> {
        P::device_tree()
    }
}

/// Maps `region` for [`prepare`] at boot.
pub fn boot_map<P: Platform>(memory: &mut Memory<P>, region: PhysicalRange) -> Option<usize> {
    window::map::<P>(memory, region).ok()
}

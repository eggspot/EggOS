//! The driver model (M5, D110): devices the firmware describes, and the drivers built into the
//! kernel that bind to them.
//!
//! - **Discovery.** At boot the buses describe what they find as plain [`DeviceNode`]s, an
//!   identity and resources (register ranges, interrupts, DMA coherence), in a [`DeviceTable`]
//!   on the heap, as large as what was found: every enabled device-tree node with a
//!   `compatible` on AArch64 and RISC-V, every PCI function the firmware's `MCFG` lets the
//!   kernel reach on x86_64.
//! - **Drivers.** A driver is a `const` [`DriverDescriptor`]: a name, a probe pass, a match table
//!   and a probe function. [`BUILTIN`] lists those the kernel's cargo features build in, so a
//!   driver not enabled costs nothing (D18).
//! - **Probing.** Passes run in order, interrupt controllers, then buses, then devices (as
//!   FreeBSD's bus passes), each offering every unbound device to the driver that matches it
//!   best ([`best_driver`]). A bus driver adds the devices it finds behind a device as its
//!   children (a virtio transport adds the virtio device), which the later passes probe; a
//!   driver that finds nothing there ([`ProbeError::Absent`], an empty virtio-mmio slot)
//!   leaves the device unbound.
//! - **The `devices` capability** ([`for_each`]) lists them all with their binding, for the
//!   CLI and MCP front-ends (D22), which come with the MCP server (M10).

pub mod dma;
mod fdt_bus;
mod pci_bus;
pub(crate) mod selftest;
#[cfg(any(test, feature = "virtio"))]
pub mod virtio;
pub mod window;

use crate::memory::Memory;
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use eggos_drivers::pci::{self, ConfigSpace};
use eggos_hal::fdt::InterruptSpecifier;
use eggos_hal::{PhysicalRange, Platform};

/// Register ranges a device records: a PCI function's six BARs, by BAR number.
pub const MAX_REGIONS: usize = 6;
/// Interrupts a device records.
pub const MAX_INTERRUPTS: usize = 2;

/// A device's place in the [`DeviceTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(u16);

impl DeviceId {
    /// The device's index in the table, in discovery order.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// When a driver probes: every device is offered to interrupt controller drivers first, then
/// to bus drivers, then to device drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProbePass {
    InterruptController,
    Bus,
    Device,
}

impl ProbePass {
    /// Every pass, in order.
    pub const ALL: [ProbePass; 3] = [Self::InterruptController, Self::Bus, Self::Device];
}

/// One entry of a driver's match table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// A device-tree `compatible` string.
    Compatible(&'static [u8]),
    /// A PCI function's vendor and device id.
    PciId { vendor: u16, device: u16 },
    /// A PCI function of a vendor whose device id is in `first..=last`.
    PciIdRange { vendor: u16, first: u16, last: u16 },
    /// A PCI function's class and subclass.
    PciClass { class: u8, subclass: u8 },
    /// An ACPI `_HID`; nothing is discovered from ACPI's namespace yet (AML is M13's).
    AcpiHid(&'static [u8]),
    /// A virtio device type, behind any transport.
    VirtioId(u32),
}

impl Match {
    /// How well this entry matches `identity`: `None` if it does not, else a rank, lower being
    /// more specific. A device-tree match ranks by the position of the matching string in the
    /// device's `compatible` list (its most specific string is first); a PCI id ranks 0, an
    /// id range 1, a class 2; a virtio type 0.
    pub fn rank(&self, identity: &Identity) -> Option<u32> {
        match (*self, identity) {
            (Match::Compatible(wanted), Identity::DeviceTree { compatible, .. }) => compatible
                .split(|&byte| byte == 0)
                .filter(|string| !string.is_empty())
                .position(|string| string == wanted)
                .map(|position| position as u32),
            (Match::PciId { vendor, device }, Identity::Pci(function)) => {
                (function.vendor_id == vendor && function.device_id == device).then_some(0)
            }
            (
                Match::PciIdRange {
                    vendor,
                    first,
                    last,
                },
                Identity::Pci(function),
            ) => {
                let in_range = (first..=last).contains(&function.device_id);
                (function.vendor_id == vendor && in_range).then_some(1)
            }
            (Match::PciClass { class, subclass }, Identity::Pci(function)) => {
                (function.class == class && function.subclass == subclass).then_some(2)
            }
            (Match::VirtioId(wanted), Identity::Virtio { device_type, .. }) => {
                (*device_type == wanted).then_some(0)
            }
            _ => None,
        }
    }
}

/// Why a driver did not take a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    /// Nothing is there (an empty slot): the device stays unbound, which is no failure.
    Absent,
    /// The driver cannot drive the device, for this reason.
    Refused(&'static str),
}

/// What a probe function is given: the table and what the kernel lends drivers.
pub trait Probe {
    /// The device table, being probed.
    fn table(&mut self) -> &mut DeviceTable;
    /// Maps the device registers at `region` into the kernel's device window as device memory
    /// and returns the virtual address of `region.start`. Windows stay mapped.
    fn map_window(&mut self, region: PhysicalRange) -> Result<usize, ProbeError>;
    /// The PCI configuration space discovery found, if any.
    fn pci_config(&self) -> Option<&dyn ConfigSpace>;
    /// A deadline `milliseconds` from now, for polling a device.
    fn deadline(&self, milliseconds: u64) -> Deadline;
}

/// A driver built into the kernel.
#[derive(Debug)]
pub struct DriverDescriptor {
    pub name: &'static str,
    pub pass: ProbePass,
    pub matches: &'static [Match],
    /// Takes device `DeviceId` of the table `Probe` gives, or says why not.
    pub probe: fn(&mut dyn Probe, DeviceId) -> Result<(), ProbeError>,
}

impl DriverDescriptor {
    /// The best rank of the driver's match table for `identity`, if any entry matches.
    pub fn rank(&self, identity: &Identity) -> Option<u32> {
        self.matches
            .iter()
            .filter_map(|entry| entry.rank(identity))
            .min()
    }
}

/// The driver of `drivers` for `pass` whose match table ranks `identity` best; between equal
/// ranks, the first in `drivers`.
pub fn best_driver(
    drivers: &[&'static DriverDescriptor],
    pass: ProbePass,
    identity: &Identity,
) -> Option<&'static DriverDescriptor> {
    drivers
        .iter()
        .filter(|driver| driver.pass == pass)
        .filter_map(|driver| Some((driver.rank(identity)?, *driver)))
        .min_by_key(|&(rank, _)| rank)
        .map(|(_, driver)| driver)
}

/// The drivers this kernel was built with (cargo features, D18).
pub const BUILTIN: &[&DriverDescriptor] = &[
    #[cfg(any(test, feature = "virtio"))]
    &virtio::MMIO_DRIVER,
    #[cfg(any(test, feature = "virtio"))]
    &virtio::PCI_DRIVER,
    #[cfg(any(test, feature = "virtio-rng"))]
    &virtio::RNG_DRIVER,
    #[cfg(any(test, feature = "virtio-gpu"))]
    &virtio::GPU_DRIVER,
];

/// What a device is, as the bus that found it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// A device-tree node: its name with its unit address, and its `compatible` strings (each
    /// ended by a NUL, most specific first).
    DeviceTree {
        name: &'static [u8],
        compatible: &'static [u8],
    },
    /// A PCI function.
    Pci(pci::Function),
    /// A virtio device behind a transport its parent's driver set up; `transport` is that
    /// driver's handle for it.
    Virtio { device_type: u32, transport: u8 },
}

/// Whether a device's DMA sees the CPU's caches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaCoherence {
    Coherent,
    /// Needs cache maintenance around DMA, which the kernel does not do yet: such devices are
    /// refused.
    NonCoherent,
}

/// Which driver, if any, has a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Unbound,
    Bound(&'static str),
    /// The driver named refused it, for the reason given.
    Refused(&'static str, &'static str),
}

/// A device: what it is and the resources the firmware gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceNode {
    pub identity: Identity,
    /// The device whose driver found this one, if not a firmware description.
    pub parent: Option<DeviceId>,
    /// Register ranges, in the firmware's order (a PCI function's by BAR number); empty slots
    /// are empty ranges.
    pub regions: [PhysicalRange; MAX_REGIONS],
    pub interrupts: [Option<InterruptSpecifier>; MAX_INTERRUPTS],
    pub dma: DmaCoherence,
    pub binding: Binding,
}

impl DeviceNode {
    /// A device with no resources yet, coherent, unbound.
    pub const fn new(identity: Identity) -> Self {
        Self {
            identity,
            parent: None,
            regions: [PhysicalRange { start: 0, end: 0 }; MAX_REGIONS],
            interrupts: [None; MAX_INTERRUPTS],
            dma: DmaCoherence::Coherent,
            binding: Binding::Unbound,
        }
    }

    /// The register ranges that are not empty.
    pub fn regions(&self) -> impl Iterator<Item = PhysicalRange> + '_ {
        self.regions
            .iter()
            .copied()
            .filter(|region| !region.is_empty())
    }
}

/// Every device discovered, in discovery order, children after their parents.
#[derive(Debug)]
pub struct DeviceTable {
    nodes: Vec<DeviceNode>,
    dropped: usize,
}

impl DeviceTable {
    pub const fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dropped: 0,
        }
    }

    /// Adds `node`; `None`, counted as dropped, when there is no memory for it.
    pub fn add(&mut self, node: DeviceNode) -> Option<DeviceId> {
        let id = u16::try_from(self.nodes.len()).ok().map(DeviceId);
        if id.is_none() || self.nodes.try_reserve(1).is_err() {
            self.dropped += 1;
            return None;
        }
        self.nodes.push(node);
        id
    }

    pub fn get(&self, id: DeviceId) -> Option<&DeviceNode> {
        self.nodes.get(id.index())
    }

    pub fn get_mut(&mut self, id: DeviceId) -> Option<&mut DeviceNode> {
        self.nodes.get_mut(id.index())
    }

    /// How many devices it holds.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Devices discovery found but there was no memory to record.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Every device with its id, in order.
    pub fn iter(&self) -> impl Iterator<Item = (DeviceId, &DeviceNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (DeviceId(index as u16), node))
    }
}

impl Default for DeviceTable {
    fn default() -> Self {
        Self::new()
    }
}

/// A point on the platform's clock to poll a device until.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    ticks: fn() -> u64,
    end: u64,
}

impl Deadline {
    /// `milliseconds` from now on platform `P`'s clock.
    pub fn after<P: Platform>(milliseconds: u64) -> Self {
        let ticks = eggos_hal::time::mul_div_ceil(milliseconds, P::clock_frequency(), 1000);
        Self {
            ticks: P::clock_ticks,
            end: P::clock_ticks().saturating_add(ticks),
        }
    }

    /// Whether the deadline is still ahead: what a polling loop asks before it polls again.
    pub fn keep_waiting(&mut self) -> bool {
        (self.ticks)() < self.end
    }
}

static DEVICES: LeafSpinLock<Option<Box<DeviceTable>>> = LeafSpinLock::new_leaf(None);

/// What discovery and probing did at boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub devices: usize,
    pub bound: usize,
    pub refused: usize,
    pub dropped: usize,
    /// Why the firmware's description could not be read, if it could not.
    pub discovery_error: Option<&'static str>,
}

struct BootProbe<'a, P: Platform> {
    table: &'a mut DeviceTable,
    memory: &'a mut Memory<P>,
    ecam: Option<pci::ecam::Ecam>,
}

impl<P: Platform> Probe for BootProbe<'_, P> {
    fn table(&mut self) -> &mut DeviceTable {
        self.table
    }

    fn map_window(&mut self, region: PhysicalRange) -> Result<usize, ProbeError> {
        window::map::<P>(self.memory, region)
            .map_err(|error| ProbeError::Refused(error.description()))
    }

    fn pci_config(&self) -> Option<&dyn ConfigSpace> {
        self.ecam.as_ref().map(|ecam| ecam as &dyn ConfigSpace)
    }

    fn deadline(&self, milliseconds: u64) -> Deadline {
        Deadline::after::<P>(milliseconds)
    }
}

/// Discovers the devices the firmware describes and probes [`BUILTIN`]'s drivers on them, once,
/// at boot on the boot CPU before any other starts (device windows are mapped on it alone).
pub fn discover_and_probe<P: Platform>(memory: &mut Memory<P>) -> Report {
    let mut devices = DEVICES.lock();
    let mut report = Report::default();
    if let Some(table) = devices.as_ref() {
        return summarize(table, report);
    }
    let table = devices.insert(Box::new(DeviceTable::new()));
    if let Some(blob) = P::device_tree()
        && let Err(error) = fdt_bus::discover(blob, table, P::DEVICE_TREE_DMA_COHERENT_BY_DEFAULT)
    {
        report.discovery_error = Some(error);
    }
    let mut probe = BootProbe {
        table,
        memory,
        ecam: None,
    };
    if let Some(mcfg) = P::acpi_table(b"MCFG")
        && let Err(error) = pci_bus::discover(&mut probe, mcfg)
    {
        report.discovery_error = Some(error);
    }
    probe_all(&mut probe, BUILTIN);
    table.nodes.shrink_to_fit();
    summarize(table, report)
}

fn summarize(table: &DeviceTable, mut report: Report) -> Report {
    report.devices = table.len();
    report.dropped = table.dropped();
    for (_, node) in table.iter() {
        match node.binding {
            Binding::Bound(_) => report.bound += 1,
            Binding::Refused(..) => report.refused += 1,
            Binding::Unbound => {}
        }
    }
    report
}

/// Offers every unbound device of the table to `drivers`, pass by pass; devices a pass adds are
/// offered too.
pub fn probe_all(probe: &mut dyn Probe, drivers: &[&'static DriverDescriptor]) {
    for pass in ProbePass::ALL {
        let mut index = 0;
        while index < probe.table().len() {
            let id = DeviceId(index as u16);
            index += 1;
            let Some(node) = probe.table().get(id).copied() else {
                continue;
            };
            if node.binding != Binding::Unbound {
                continue;
            }
            let Some(driver) = best_driver(drivers, pass, &node.identity) else {
                continue;
            };
            let binding = match (driver.probe)(probe, id) {
                Ok(()) => Binding::Bound(driver.name),
                Err(ProbeError::Absent) => Binding::Unbound,
                Err(ProbeError::Refused(reason)) => Binding::Refused(driver.name, reason),
            };
            if let Some(node) = probe.table().get_mut(id) {
                node.binding = binding;
            }
        }
    }
}

/// The `devices` capability (D22): calls `device` with every device discovered, in order.
pub fn for_each(device: &mut dyn FnMut(DeviceId, &DeviceNode)) {
    if let Some(table) = DEVICES.lock().as_ref() {
        for (id, node) in table.iter() {
            device(id, node);
        }
    }
}

impl core::fmt::Display for DeviceNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.identity {
            Identity::DeviceTree { name, compatible } => {
                let first = compatible.split(|&byte| byte == 0).next().unwrap_or(&[]);
                write_ascii(f, name)?;
                f.write_str(" (")?;
                write_ascii(f, first)?;
                f.write_str(")")?;
            }
            Identity::Pci(function) => write!(
                f,
                "pci {} {:04x}:{:04x} class {:02x}{:02x}",
                function.address,
                function.vendor_id,
                function.device_id,
                function.class,
                function.subclass
            )?,
            Identity::Virtio { device_type, .. } => write!(
                f,
                "virtio {} (type {device_type})",
                eggos_drivers::virtio::device_type_name(device_type)
            )?,
        }
        for region in self.regions() {
            write!(f, " {region}")?;
        }
        match self.binding {
            Binding::Unbound => Ok(()),
            Binding::Bound(driver) => write!(f, ": {driver}"),
            Binding::Refused(driver, reason) => {
                write!(f, ": {driver} refused: {reason}")
            }
        }
    }
}

fn write_ascii(f: &mut core::fmt::Formatter<'_>, bytes: &[u8]) -> core::fmt::Result {
    bytes.iter().try_for_each(|&byte| {
        let printable = byte.is_ascii_graphic() || byte == b' ';
        f.write_char(if printable { char::from(byte) } else { '?' })
    })
}

#[cfg(test)]
mod tests;

//! Wired Ethernet adapters on PCI (D240): Intel's e1000, e1000e and igc, Realtek's RTL8111/8168
//! and RTL8125, the first one found installed as the network stack's adapter ([`crate::net`]),
//! polled.

use super::dma::DmaRegion;
use super::{DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::net::NetDevice;
use alloc::boxed::Box;
use eggos_drivers::ethernet::{self, MEMORY_BYTES, e1000, igc, realtek};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_net::ethernet::MacAddress;

const NETWORK: u8 = 0x02;
const ETHERNET: u8 = 0x00;
const INTEL: u16 = 0x8086;
const REALTEK: u16 = 0x10ec;
const START_TIMEOUT_MS: u64 = 1_000;
const ETHERNET_MTU: usize = 1500;

/// Ethernet controllers (PCI class 02h, subclass 00h) of the kinds the drivers take.
pub static ETHERNET_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "ethernet",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: NETWORK,
        subclass: ETHERNET,
    }],
    probe: probe_ethernet,
};

enum Adapter {
    E1000(e1000::E1000<Mmio, DmaRegion>),
    Igc(igc::Igc<Mmio, DmaRegion>),
    Realtek(realtek::Realtek<Mmio, DmaRegion>),
}

impl NetDevice for Adapter {
    fn mac(&self) -> MacAddress {
        MacAddress(match self {
            Adapter::E1000(adapter) => adapter.mac(),
            Adapter::Igc(adapter) => adapter.mac(),
            Adapter::Realtek(adapter) => adapter.mac(),
        })
    }

    fn mtu(&self) -> usize {
        ETHERNET_MTU
    }

    fn is_link_up(&self) -> bool {
        match self {
            Adapter::E1000(adapter) => adapter.is_link_up(),
            Adapter::Igc(adapter) => adapter.is_link_up(),
            Adapter::Realtek(adapter) => adapter.is_link_up(),
        }
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        match self {
            Adapter::E1000(adapter) => adapter.receive(frame),
            Adapter::Igc(adapter) => adapter.receive(frame),
            Adapter::Realtek(adapter) => adapter.receive(frame),
        }
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        match self {
            Adapter::E1000(adapter) => adapter.transmit(frame),
            Adapter::Igc(adapter) => adapter.transmit(frame),
            Adapter::Realtek(adapter) => adapter.transmit(frame),
        }
        .map_err(ethernet::Error::description)
    }
}

enum Kind {
    E1000(e1000::Family),
    Igc,
    Realtek(realtek::Generation),
}

fn kind_of(vendor: u16, device: u16) -> Option<Kind> {
    match vendor {
        INTEL if igc::is_igc(device) => Some(Kind::Igc),
        INTEL => e1000::family_of(device).map(Kind::E1000),
        REALTEK => realtek::generation_of(device).map(Kind::Realtek),
        _ => None,
    }
}

fn probe_ethernet(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    let kind = kind_of(function.vendor_id, function.device_id)
        .ok_or(ProbeError::Refused("an Ethernet adapter no driver takes yet"))?;
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    let region = node
        .regions()
        .next()
        .ok_or(ProbeError::Refused("the adapter's registers are not in memory space"))?;
    let length = usize::try_from(region.len()).map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let base = probe.map_window(region)?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    pci::enable_memory_and_bus_master(config, function.address);
    // SAFETY: the adapter's first memory BAR is its register block, mapped as device memory by
    // `map_window` for as long as the kernel runs; only this driver drives the adapter.
    let regs = unsafe { Mmio::new(base, length) };
    let memory = DmaRegion::allocate(MEMORY_BYTES)
        .map_err(|_| ProbeError::Refused("no memory for the adapter's rings"))?;
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let mut keep_waiting = || deadline.keep_waiting();
    let refuse = |(error, memory): (ethernet::Error, DmaRegion)| {
        // SAFETY: the drivers hand the memory back only before the adapter was given it.
        unsafe { memory.free() };
        ProbeError::Refused(error.description())
    };
    let adapter = match kind {
        Kind::E1000(family) => Adapter::E1000(e1000::E1000::new(regs, memory, family, &mut keep_waiting).map_err(refuse)?),
        Kind::Igc => Adapter::Igc(igc::Igc::new(regs, memory, &mut keep_waiting).map_err(refuse)?),
        Kind::Realtek(generation) => {
            Adapter::Realtek(realtek::Realtek::new(regs, memory, generation, &mut keep_waiting).map_err(refuse)?)
        }
    };
    crate::net::install(Box::new(adapter)).map_err(|_| ProbeError::Refused("a network adapter is already in use"))
}

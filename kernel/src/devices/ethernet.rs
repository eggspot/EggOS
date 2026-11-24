//! Wired Ethernet adapters on PCI (D240, D615): Intel's e1000, e1000e, igb, igc and ixgbe,
//! Realtek's RTL8111/8168, RTL8125 and RTL8139, the first one found installed as the network
//! stack's adapter ([`crate::net`]), polled.

use super::dma::DmaRegion;
use super::{
    DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass,
};
use crate::net::NetDevice;
use alloc::boxed::Box;
use eggos_drivers::ethernet::{
    self, MAX_JUMBO_MTU, MEMORY_BYTES, STANDARD_MTU, WakeOnLan, e1000, frame_bytes_for, igc,
    ixgbe, realtek, rtl8139,
};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_net::ethernet::MacAddress;

const NETWORK: u8 = 0x02;
const ETHERNET: u8 = 0x00;
const INTEL: u16 = 0x8086;
const REALTEK: u16 = 0x10ec;
const START_TIMEOUT_MS: u64 = 1_000;
const SMALLEST_MTU: usize = 68;
const POWER_MANAGEMENT_CAPABILITY: u8 = 0x01;
const POWER_CONTROL_OFFSET: u16 = 4;
const PME_ENABLE: u32 = 1 << 8;
const PME_STATUS: u32 = 1 << 15;

/// The PCI function of the adapter whose Wake-on-LAN is on, for [`arm_wake`].
static WAKE_FUNCTION: crate::sync::SpinLock<Option<pci::Address>> = crate::sync::SpinLock::new(None);

/// Lets the adapter whose Wake-on-LAN is on signal a wake from the bus (its power management
/// capability's PME enable, its old PME status cleared), as the machine goes to sleep or powers
/// off (D708). Nothing when no adapter wakes the machine or it has no such capability.
pub fn arm_wake() {
    let Some(address) = *WAKE_FUNCTION.lock() else {
        return;
    };
    let Some(config) = super::interrupts::kept_configuration() else {
        return;
    };
    let Some(capability) = pci::capabilities(config, address)
        .find(|capability| capability.id == POWER_MANAGEMENT_CAPABILITY)
    else {
        return;
    };
    let offset = u16::from(capability.offset) + POWER_CONTROL_OFFSET;
    let word = config.read32(address, offset & !3);
    config.write32(address, offset & !3, word | PME_ENABLE | PME_STATUS);
}

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
    Ixgbe(ixgbe::Ixgbe<Mmio, DmaRegion>),
    Realtek(realtek::Realtek<Mmio, DmaRegion>),
    Rtl8139(rtl8139::Rtl8139<Mmio, DmaRegion>),
}

/// An adapter with the MTU and wake setting the stack gave it (D708).
struct Ethernet {
    adapter: Adapter,
    function: pci::Address,
    mtu: usize,
    wake: bool,
}

impl Adapter {
    fn takes_jumbo_frames(&self) -> bool {
        matches!(self, Adapter::E1000(_) | Adapter::Igc(_) | Adapter::Ixgbe(_))
    }

    fn set_max_frame(&mut self, bytes: usize) -> Result<(), ethernet::Error> {
        match self {
            Adapter::E1000(adapter) => adapter.set_max_frame(bytes),
            Adapter::Igc(adapter) => adapter.set_max_frame(bytes),
            Adapter::Ixgbe(adapter) => adapter.set_max_frame(bytes),
            Adapter::Realtek(adapter) => adapter.set_max_frame(bytes),
            Adapter::Rtl8139(adapter) => adapter.set_max_frame(bytes),
        }
    }

    fn set_wake_on_lan(&mut self, wake: WakeOnLan) {
        match self {
            Adapter::E1000(adapter) => adapter.set_wake_on_lan(wake),
            Adapter::Igc(adapter) => adapter.set_wake_on_lan(wake),
            Adapter::Ixgbe(adapter) => adapter.set_wake_on_lan(wake),
            Adapter::Realtek(adapter) => adapter.set_wake_on_lan(wake),
            Adapter::Rtl8139(adapter) => adapter.set_wake_on_lan(wake),
        }
    }
}

impl NetDevice for Ethernet {
    fn mac(&self) -> MacAddress {
        self.adapter.mac()
    }

    fn mtu(&self) -> usize {
        self.mtu
    }

    fn is_link_up(&self) -> bool {
        self.adapter.is_link_up()
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.adapter.receive(frame)
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        self.adapter.transmit(frame)
    }

    fn max_mtu(&self) -> usize {
        match self.adapter.takes_jumbo_frames() {
            true => MAX_JUMBO_MTU,
            false => STANDARD_MTU,
        }
    }

    fn set_mtu(&mut self, mtu: usize) -> Result<(), &'static str> {
        if !(SMALLEST_MTU..=self.max_mtu()).contains(&mtu) {
            return Err("the MTU is outside what the adapter takes");
        }
        self.adapter
            .set_max_frame(frame_bytes_for(mtu.max(STANDARD_MTU)))
            .map_err(ethernet::Error::description)?;
        self.mtu = mtu;
        Ok(())
    }

    fn wake_on_lan(&self) -> Option<bool> {
        Some(self.wake)
    }

    fn set_wake_on_lan(&mut self, on: bool) -> Result<(), &'static str> {
        let wake = match on {
            true => WakeOnLan::MagicPacket,
            false => WakeOnLan::Off,
        };
        self.adapter.set_wake_on_lan(wake);
        self.wake = on;
        *WAKE_FUNCTION.lock() = on.then_some(self.function);
        Ok(())
    }
}

impl Adapter {
    fn mac(&self) -> MacAddress {
        MacAddress(match self {
            Adapter::E1000(adapter) => adapter.mac(),
            Adapter::Igc(adapter) => adapter.mac(),
            Adapter::Ixgbe(adapter) => adapter.mac(),
            Adapter::Realtek(adapter) => adapter.mac(),
            Adapter::Rtl8139(adapter) => adapter.mac(),
        })
    }

    fn is_link_up(&self) -> bool {
        match self {
            Adapter::E1000(adapter) => adapter.is_link_up(),
            Adapter::Igc(adapter) => adapter.is_link_up(),
            Adapter::Ixgbe(adapter) => adapter.is_link_up(),
            Adapter::Realtek(adapter) => adapter.is_link_up(),
            Adapter::Rtl8139(adapter) => adapter.is_link_up(),
        }
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        match self {
            Adapter::E1000(adapter) => adapter.receive(frame),
            Adapter::Igc(adapter) => adapter.receive(frame),
            Adapter::Ixgbe(adapter) => adapter.receive(frame),
            Adapter::Realtek(adapter) => adapter.receive(frame),
            Adapter::Rtl8139(adapter) => adapter.receive(frame),
        }
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        match self {
            Adapter::E1000(adapter) => adapter.transmit(frame),
            Adapter::Igc(adapter) => adapter.transmit(frame),
            Adapter::Ixgbe(adapter) => adapter.transmit(frame),
            Adapter::Realtek(adapter) => adapter.transmit(frame),
            Adapter::Rtl8139(adapter) => adapter.transmit(frame),
        }
        .map_err(ethernet::Error::description)
    }
}

enum Kind {
    E1000(e1000::Family),
    Igc(igc::Family),
    Ixgbe,
    Realtek(realtek::Generation),
    Rtl8139,
}

fn kind_of(vendor: u16, device: u16) -> Option<Kind> {
    match vendor {
        INTEL if igc::is_igc(device) => Some(Kind::Igc(igc::Family::Igc)),
        INTEL if igc::is_igb(device) => Some(Kind::Igc(igc::Family::Igb)),
        INTEL if ixgbe::is_ixgbe(device) => Some(Kind::Ixgbe),
        INTEL => e1000::family_of(device).map(Kind::E1000),
        REALTEK if rtl8139::is_rtl8139(device) => Some(Kind::Rtl8139),
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
    let kind = kind_of(function.vendor_id, function.device_id).ok_or(ProbeError::Refused(
        "an Ethernet adapter no driver takes yet",
    ))?;
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    let region = node.regions().next().ok_or(ProbeError::Refused(
        "the adapter's registers are not in memory space",
    ))?;
    let length = usize::try_from(region.len())
        .map_err(|_| ProbeError::Refused("the register block is too large"))?;
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
    let mut adapter = match kind {
        Kind::E1000(family) => Adapter::E1000(
            e1000::E1000::new(regs, memory, family, &mut keep_waiting).map_err(refuse)?,
        ),
        Kind::Igc(family) => Adapter::Igc(
            igc::Igc::new(regs, memory, family, &mut keep_waiting).map_err(refuse)?,
        ),
        Kind::Ixgbe => {
            Adapter::Ixgbe(ixgbe::Ixgbe::new(regs, memory, &mut keep_waiting).map_err(refuse)?)
        }
        Kind::Rtl8139 => Adapter::Rtl8139(
            rtl8139::Rtl8139::new(regs, memory, &mut keep_waiting).map_err(refuse)?,
        ),
        Kind::Realtek(generation) => Adapter::Realtek(
            realtek::Realtek::new(regs, memory, generation, &mut keep_waiting).map_err(refuse)?,
        ),
    };
    let interrupting = matches!(adapter, Adapter::E1000(_))
        && super::message_interrupts::connect(probe, id, &crate::net::DEVICE_EVENTS).is_ok();
    if let (true, Adapter::E1000(e1000)) = (interrupting, &mut adapter) {
        e1000.enable_interrupts();
    }
    let device = Ethernet {
        adapter,
        function: function.address,
        mtu: STANDARD_MTU,
        wake: false,
    };
    match interrupting {
        true => crate::net::install_interrupting(Box::new(device)),
        false => crate::net::install(Box::new(device)),
    }
    .map_err(|_| ProbeError::Refused("a network adapter is already in use"))
}

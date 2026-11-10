//! USB host controllers on PCI (D239): xHCI, with the HID boot keyboards and mice on its root
//! ports installed as one input device ([`crate::input`]), polled.

use super::dma::DmaRegion;
use super::{DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass};
use crate::input::InputDevice;
use alloc::boxed::Box;
use eggos_drivers::input::{Capabilities, LockLights, RawEvent};
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::usb::BootDevice;
use eggos_drivers::usb::xhci::{self, Xhci};

const SERIAL_BUS: u8 = 0x0c;
const USB: u8 = 0x03;
const XHCI_INTERFACE: u8 = 0x30;
const START_TIMEOUT_MS: u64 = 2_000;
const SETTLE_MS: u64 = 300;
const ATTACH_TIMEOUT_MS: u64 = 5_000;

/// xHCI controllers (PCI class 0Ch, subclass 03h, interface 30h).
pub static XHCI_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "xhci",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: SERIAL_BUS,
        subclass: USB,
    }],
    probe: probe_xhci,
};

struct UsbInput {
    controller: Xhci<Mmio, DmaRegion>,
    capabilities: Capabilities,
}

impl InputDevice for UsbInput {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
        self.controller.poll(budget, event)
    }

    fn set_lock_lights(&mut self, lights: LockLights) -> bool {
        self.controller.set_lights(lights);
        true
    }
}

fn probe_xhci(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if function.prog_if != XHCI_INTERFACE {
        return Err(ProbeError::Refused("a USB controller older than xHCI"));
    }
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    let region = node.regions[0];
    if region.is_empty() {
        return Err(ProbeError::Refused("the controller's registers are not in memory space"));
    }
    let length = usize::try_from(region.len()).map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let base = probe.map_window(region)?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    pci::enable_memory_and_bus_master(config, function.address);
    // SAFETY: BAR 0 of an xHCI controller is its register block, mapped as device memory by
    // `map_window` for as long as the kernel runs; only this driver drives the controller.
    let regs = unsafe { Mmio::new(base, length) };
    let memory = DmaRegion::allocate(xhci::MEMORY_BYTES)
        .map_err(|_| ProbeError::Refused("no memory for the controller's rings"))?;
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let mut controller = Xhci::new(regs, memory, &mut || deadline.keep_waiting()).map_err(|(error, memory)| {
        if let Some(memory) = memory {
            // SAFETY: handed back only when the controller is halted and never had it, or
            // halted after.
            unsafe { memory.free() };
        }
        ProbeError::Refused(error.description())
    })?;
    let mut settle = probe.deadline(SETTLE_MS);
    let mut attach = probe.deadline(ATTACH_TIMEOUT_MS);
    let attached = controller.attach_devices(&mut || settle.keep_waiting(), &mut || attach.keep_waiting());
    if attached == 0 {
        return Err(ProbeError::Refused("no USB keyboard or mouse on its ports"));
    }
    let pointer = controller.has(BootDevice::Mouse);
    let capabilities = Capabilities {
        keyboard: controller.has(BootDevice::Keyboard),
        buttons: pointer,
        relative: pointer,
        wheel: pointer,
        absolute: None,
    };
    let interrupting = super::message_interrupts::connect(probe, id, &crate::input::DEVICE_EVENTS).is_ok();
    if interrupting {
        controller.enable_interrupts();
    }
    let device = Box::new(UsbInput {
        controller,
        capabilities,
    });
    match interrupting {
        true => crate::input::install_interrupting(device),
        false => crate::input::install(device),
    }
    .map_err(|_| ProbeError::Refused("no room for another input device"))
}

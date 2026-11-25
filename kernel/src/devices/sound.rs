//! Sound devices (D773): Intel High Definition Audio controllers on PCI (class 04h, subclass
//! 03h) and the virtio sound device, the first one set up installed as the sound output
//! ([`crate::sound`]), polled.

use super::dma::DmaRegion;
use super::{
    DeviceId, DmaCoherence, DriverDescriptor, Identity, Match, Probe, ProbeError, ProbePass,
};
use crate::sound::SoundOutput;
use alloc::boxed::Box;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::sound::hda::{self, HdAudio, Jack};
use eggos_drivers::sound::PcmFormat;

const MULTIMEDIA: u8 = 0x04;
const AUDIO_DEVICE: u8 = 0x03;
const START_TIMEOUT_MS: u64 = 1_000;

/// High Definition Audio controllers.
pub static HD_AUDIO_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "hd-audio",
    pass: ProbePass::Device,
    matches: &[Match::PciClass {
        class: MULTIMEDIA,
        subclass: AUDIO_DEVICE,
    }],
    probe: probe_hd_audio,
};

struct HdOutput(HdAudio<Mmio, DmaRegion>);

impl SoundOutput for HdOutput {
    fn id(&self) -> &'static str {
        "hda0-out"
    }

    fn name(&self) -> &'static str {
        match self.0.jack() {
            Jack::Headphone => "Headphones",
            Jack::LineOut => "Line out",
            Jack::Speaker => "Built-in speakers",
        }
    }

    fn format(&self) -> PcmFormat {
        self.0.format()
    }

    fn write(&mut self, samples: &[u8]) -> usize {
        self.0.write(samples)
    }
}

fn probe_hd_audio(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    if node.dma != DmaCoherence::Coherent {
        return Err(ProbeError::Refused("the device's DMA is not coherent"));
    }
    if crate::sound::is_present() {
        return Err(ProbeError::Refused("a sound output is already in use"));
    }
    let region = node.regions().next().ok_or(ProbeError::Refused(
        "the controller's registers are not in memory space",
    ))?;
    let length = usize::try_from(region.len())
        .map_err(|_| ProbeError::Refused("the register block is too large"))?;
    let base = probe.map_window(region)?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    pci::enable_memory_and_bus_master(config, function.address);
    // SAFETY: the controller's first memory BAR, mapped as device memory for the kernel's life.
    let regs = unsafe { Mmio::new(base, length) };
    let memory = DmaRegion::allocate(hda::MEMORY_BYTES)
        .map_err(|_| ProbeError::Refused("no memory for the controller's rings"))?;
    let mut deadline = probe.deadline(START_TIMEOUT_MS);
    let audio = HdAudio::new(regs, memory, &mut || deadline.keep_waiting()).map_err(
        |(error, memory)| {
            // SAFETY: the driver hands the memory back with the controller left in reset.
            unsafe { memory.free() };
            ProbeError::Refused(error.description())
        },
    )?;
    crate::sound::install(Box::new(HdOutput(audio)))
        .map_err(|_| ProbeError::Refused("a sound output is already in use"))
}

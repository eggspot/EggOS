//! virtio in the driver model (D110): the virtio-mmio and modern virtio-pci transports as bus
//! drivers, each adding the virtio device it finds as a child, and the polled virtio entropy
//! device and GPU (D112) on top of either.

use super::dma::DmaRegion;
use super::{
    Deadline, DeviceId, DeviceNode, DmaCoherence, DriverDescriptor, Identity, Match, Probe,
    ProbeError, ProbePass,
};
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::virtio::mmio::MmioTransport;
use eggos_drivers::virtio::pci::{self as virtio_pci, PciTransport, VirtioPciCapability};
use eggos_drivers::virtio::{self, QueueAddresses, Transport};

/// Transports the bus drivers can hold for their devices' drivers.
pub const MAX_TRANSPORTS: usize = 8;

/// A virtio device's transport, whichever bus it is on.
#[derive(Debug)]
pub enum AnyTransport {
    Mmio(MmioTransport<Mmio>),
    Pci(PciTransport<Mmio>),
}

macro_rules! each_transport {
    ($self:ident, $transport:ident => $body:expr) => {
        match $self {
            AnyTransport::Mmio($transport) => $body,
            AnyTransport::Pci($transport) => $body,
        }
    };
}

impl Transport for AnyTransport {
    fn device_type(&self) -> u32 {
        each_transport!(self, t => t.device_type())
    }
    fn status(&self) -> u8 {
        each_transport!(self, t => t.status())
    }
    fn set_status(&self, status: u8) {
        each_transport!(self, t => t.set_status(status))
    }
    fn device_features(&self) -> u64 {
        each_transport!(self, t => t.device_features())
    }
    fn set_driver_features(&self, features: u64) {
        each_transport!(self, t => t.set_driver_features(features))
    }
    fn max_queue_size(&self, queue: u16) -> u16 {
        each_transport!(self, t => t.max_queue_size(queue))
    }
    fn set_up_queue(
        &self,
        queue: u16,
        size: u16,
        addresses: &QueueAddresses,
    ) -> Result<(), virtio::Error> {
        each_transport!(self, t => t.set_up_queue(queue, size, addresses))
    }
    fn notify(&self, queue: u16) {
        eggos_hal::memory::doorbell_barrier();
        each_transport!(self, t => t.notify(queue))
    }
    fn read_config8(&self, offset: usize) -> u8 {
        each_transport!(self, t => t.read_config8(offset))
    }
    fn read_config32(&self, offset: usize) -> u32 {
        each_transport!(self, t => t.read_config32(offset))
    }
}

static TRANSPORTS: LeafSpinLock<[Option<Box<AnyTransport>>; MAX_TRANSPORTS]> =
    LeafSpinLock::new_leaf([const { None }; MAX_TRANSPORTS]);

/// virtio-mmio, version 2 (D110): a device-tree `virtio,mmio` slot.
pub static MMIO_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "virtio-mmio",
    pass: ProbePass::Bus,
    matches: &[Match::Compatible(b"virtio,mmio")],
    probe: probe_mmio,
};

/// Modern virtio-pci (D110): virtio's PCI vendor with a modern device id (0x1040 to 0x107f).
pub static PCI_DRIVER: DriverDescriptor = DriverDescriptor {
    name: "virtio-pci",
    pass: ProbePass::Bus,
    matches: &[Match::PciIdRange {
        vendor: virtio_pci::VIRTIO_VENDOR_ID,
        first: *virtio_pci::MODERN_DEVICE_IDS.start(),
        last: *virtio_pci::MODERN_DEVICE_IDS.end(),
    }],
    probe: probe_pci,
};

fn probe_mmio(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let region = node
        .regions()
        .next()
        .ok_or(ProbeError::Refused("the slot has no registers"))?;
    if region.len() < MMIO_REGISTERS_LENGTH {
        return Err(ProbeError::Refused(
            "the virtio-mmio slot is shorter than its registers",
        ));
    }
    let base = probe.map_window(region)?;
    let size = (region.len() as usize).min(virtio::mmio::REGISTER_BLOCK_SIZE);
    // SAFETY: `map_window` mapped the slot's registers as device memory, for good, and only
    // this driver reaches them.
    let regs = unsafe { Mmio::new(base, size) };
    match MmioTransport::new(regs) {
        Ok(transport) => add_device(probe, id, &node, AnyTransport::Mmio(transport)),
        Err(virtio::Error::NoDevice) => Err(ProbeError::Absent),
        Err(error) => Err(ProbeError::Refused(error.description())),
    }
}

fn probe_pci(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let Identity::Pci(function) = node.identity else {
        return Err(ProbeError::Refused("not a PCI function"));
    };
    let device_type =
        virtio_pci::modern_device_type(function.device_id).ok_or(ProbeError::Refused(
            "a legacy or transitional virtio-pci device: only modern ones are supported (QEMU: \
         disable-legacy=on)",
        ))?;
    let config = probe
        .pci_config()
        .ok_or(ProbeError::Refused("no PCI configuration space"))?;
    let mut common = None;
    let mut notify = None;
    let mut device = None;
    for capability in virtio_pci::virtio_capabilities(config, function.address) {
        let slot = match capability.cfg_type {
            virtio_pci::COMMON => &mut common,
            virtio_pci::NOTIFY => &mut notify,
            virtio_pci::DEVICE => &mut device,
            _ => continue,
        };
        slot.get_or_insert(capability);
    }
    let (common, notify) = usable_capabilities(common, notify)?;
    let mut windows = [None; super::MAX_REGIONS];
    let mut regs = |capability: VirtioPciCapability| -> Result<Mmio, ProbeError> {
        let bar = usize::from(capability.bar);
        let region = node.regions[bar];
        let end = u64::from(capability.offset) + u64::from(capability.length);
        if region.is_empty() || end > region.len() {
            return Err(ProbeError::Refused(
                virtio::Error::BarOutOfRange.description(),
            ));
        }
        let base = match windows[bar] {
            Some(base) => base,
            None => *windows[bar].insert(probe.map_window(region)?),
        };
        // SAFETY: the capability lies inside the BAR, which `map_window` mapped as device
        // memory for good; only this driver reaches it.
        Ok(unsafe {
            Mmio::new(
                base + capability.offset as usize,
                capability.length as usize,
            )
        })
    };
    let transport = PciTransport::new(
        device_type,
        regs(common)?,
        regs(notify)?,
        notify.length as usize,
        notify.notify_off_multiplier,
        device.map(&mut regs).transpose()?,
    );
    if let Some(config) = probe.pci_config() {
        pci::enable_memory_and_bus_master(config, function.address);
    }
    add_device(probe, id, &node, AnyTransport::Pci(transport))
}

const MMIO_REGISTERS_LENGTH: u64 = 0x100;
const COMMON_CONFIGURATION_LENGTH: u32 = 0x38;
const NOTIFICATION_LENGTH: u32 = 2;

fn usable_capabilities(
    common: Option<VirtioPciCapability>,
    notify: Option<VirtioPciCapability>,
) -> Result<(VirtioPciCapability, VirtioPciCapability), ProbeError> {
    match (common, notify) {
        (Some(common), Some(notify))
            if common.length >= COMMON_CONFIGURATION_LENGTH
                && notify.length >= NOTIFICATION_LENGTH =>
        {
            Ok((common, notify))
        }
        _ => Err(ProbeError::Refused(
            virtio::Error::BadCapabilities.description(),
        )),
    }
}

fn add_device(
    probe: &mut dyn Probe,
    parent: DeviceId,
    node: &DeviceNode,
    transport: AnyTransport,
) -> Result<(), ProbeError> {
    let device_type = transport.device_type();
    let handle = {
        let mut transports = TRANSPORTS.lock();
        let free = transports
            .iter()
            .position(Option::is_none)
            .ok_or(ProbeError::Refused("no room for another virtio transport"))?;
        transports[free] = Some(Box::new(transport));
        free as u8
    };
    let mut child = DeviceNode::new(Identity::Virtio {
        device_type,
        transport: handle,
    });
    child.parent = Some(parent);
    child.dma = node.dma;
    child.interrupts = node.interrupts;
    probe
        .table()
        .add(child)
        .ok_or(ProbeError::Refused("no memory to record the device"))?;
    Ok(())
}

fn take_transport(identity: &Identity) -> Option<AnyTransport> {
    let Identity::Virtio { transport, .. } = *identity else {
        return None;
    };
    TRANSPORTS
        .lock()
        .get_mut(usize::from(transport))?
        .take()
        .map(|boxed| *boxed)
}

fn give_back_transport(identity: &Identity, transport: AnyTransport) {
    if let Identity::Virtio {
        transport: handle, ..
    } = *identity
        && let Some(slot) = TRANSPORTS.lock().get_mut(usize::from(handle))
    {
        *slot = Some(Box::new(transport));
    }
}

#[cfg(any(test, feature = "virtio-gpu"))]
pub use gpu::GPU_DRIVER;

#[cfg(any(test, feature = "virtio-gpu"))]
mod gpu {
    use super::*;
    use crate::display::{Mode, Rect, Scanout};
    use eggos_drivers::virtio::gpu::{FALLBACK_MODE, MEMORY_BYTES, Released, VirtioGpu};

    type Gpu = VirtioGpu<AnyTransport, DmaRegion>;

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio GPU (virtio 1.2, 5.7), 2D: one framebuffer on scanout 0, installed as
    /// the kernel's display ([`crate::display`]).
    pub static GPU_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-gpu",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::GPU)],
        probe: probe_gpu,
    };

    struct GpuScanout {
        gpu: Gpu,
        mode: Mode,
    }

    impl Scanout for GpuScanout {
        fn mode(&self) -> Mode {
            self.mode
        }

        fn pixels(&mut self) -> &mut [u32] {
            self.gpu.pixels().unwrap_or_default()
        }

        fn flush(
            &mut self,
            rects: &[Rect],
            deadline: &mut Deadline,
        ) -> Result<usize, &'static str> {
            self.gpu
                .flush(rects, &mut || deadline.keep_waiting())
                .map_err(virtio::Error::description)
        }
    }

    fn probe_gpu(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        if crate::display::mode().is_some() {
            return Err(ProbeError::Refused("a display is already in use"));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let control = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queue"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        let mut gpu = match VirtioGpu::new(transport, control, &mut || deadline.keep_waiting()) {
            Ok(gpu) => gpu,
            Err((error, transport, control)) => {
                if let Some(control) = control {
                    // SAFETY: `VirtioGpu::new` hands the memory back only when the device never
                    // had it or was reset after.
                    unsafe { control.free() };
                }
                give_back_transport(&node.identity, transport);
                return Err(ProbeError::Refused(error.description()));
            }
        };
        let shown = attach_framebuffer(&mut gpu, &mut deadline);
        let refused = match shown {
            Ok(mode) => match crate::display::install(GpuScanout { gpu, mode }) {
                Ok(()) => return Ok(()),
                Err(scanout) => (scanout.gpu, "a display is already in use"),
            },
            Err(reason) => (gpu, reason),
        };
        release(
            refused.0,
            &node.identity,
            &mut probe.deadline(ANSWER_TIMEOUT_MS),
        );
        Err(ProbeError::Refused(refused.1))
    }

    fn attach_framebuffer(gpu: &mut Gpu, deadline: &mut Deadline) -> Result<Mode, &'static str> {
        let display = gpu.display_mode();
        let (framebuffer, mode) = [display, FALLBACK_MODE]
            .into_iter()
            .find_map(|mode| Some((DmaRegion::allocate(mode.bytes()).ok()?, mode)))
            .ok_or("no memory for the framebuffer")?;
        gpu.attach_framebuffer(framebuffer, mode, &mut || deadline.keep_waiting())
            .map(|()| mode)
            .map_err(|(error, framebuffer)| {
                if let Some(framebuffer) = framebuffer {
                    // SAFETY: handed back by `attach_framebuffer`, which does so only when the
                    // device was never given it.
                    unsafe { framebuffer.free() };
                }
                error.description()
            })
    }

    fn release(gpu: Gpu, identity: &Identity, deadline: &mut Deadline) {
        match gpu.reset(&mut || deadline.keep_waiting()) {
            Ok(Released {
                transport,
                control,
                framebuffer,
                reset,
            }) => {
                control.free_after_reset(&reset);
                if let Some(framebuffer) = framebuffer {
                    framebuffer.free_after_reset(&reset);
                }
                give_back_transport(identity, transport);
            }
            Err((_, transport)) => give_back_transport(identity, transport),
        }
    }
}

#[cfg(any(test, feature = "virtio-rng"))]
pub use entropy::{RNG_DRIVER, read_random};

#[cfg(any(test, feature = "virtio-rng"))]
pub(crate) use entropy::{put_rng, take_rng};

#[cfg(any(test, feature = "virtio-rng"))]
mod entropy {
    use super::*;
    use eggos_drivers::virtio::rng::{MEMORY_BYTES, VirtioRng};

    pub(crate) type Rng = VirtioRng<AnyTransport, DmaRegion>;

    static RNG: LeafSpinLock<Option<Box<Rng>>> = LeafSpinLock::new_leaf(None);

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio entropy device (virtio 1.2, 5.4).
    pub static RNG_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-rng",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::ENTROPY)],
        probe: probe_rng,
    };

    fn probe_rng(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        if RNG.lock().is_some() {
            return Err(ProbeError::Refused("an entropy device is already in use"));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let memory = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queue"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        match VirtioRng::new(transport, memory, &mut || deadline.keep_waiting()) {
            Ok(rng) => {
                *RNG.lock() = Some(Box::new(rng));
                Ok(())
            }
            Err((error, transport, memory)) => {
                if let Some(memory) = memory {
                    // SAFETY: `VirtioRng::new` hands the memory back only when the device never
                    // had it or was reset after.
                    unsafe { memory.free() };
                }
                give_back_transport(&node.identity, transport);
                Err(ProbeError::Refused(error.description()))
            }
        }
    }

    /// Fills the start of `bytes` with random bytes from the virtio entropy device, polling it
    /// until `deadline`, and returns how many; `None` when there is no such device or it did not
    /// answer in time.
    pub fn read_random(bytes: &mut [u8], deadline: &mut Deadline) -> Option<usize> {
        RNG.lock()
            .as_mut()?
            .read(bytes, &mut || deadline.keep_waiting())
            .ok()
    }

    pub(crate) fn take_rng() -> Option<Rng> {
        RNG.lock().take().map(|boxed| *boxed)
    }

    pub(crate) fn put_rng(rng: Rng) {
        *RNG.lock() = Some(Box::new(rng));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{DeviceTable, Probe};
    use eggos_drivers::pci::ConfigSpace;
    use eggos_hal::PhysicalRange;

    struct NoMapping(DeviceTable);

    impl Probe for NoMapping {
        fn table(&mut self) -> &mut DeviceTable {
            &mut self.0
        }
        fn map_window(&mut self, _: PhysicalRange) -> Result<usize, ProbeError> {
            panic!("a refused device is never mapped")
        }
        fn pci_config(&self) -> Option<&dyn ConfigSpace> {
            None
        }
        fn deadline(&self, _: u64) -> crate::devices::Deadline {
            unreachable!()
        }
    }

    #[test]
    fn a_virtio_mmio_slot_shorter_than_its_registers_is_refused_unmapped() {
        let mut node = DeviceNode::new(Identity::DeviceTree {
            name: b"virtio_mmio@a000000",
            compatible: b"virtio,mmio\0",
        });
        node.regions[0] = PhysicalRange::from_start_length(0xa00_0000, 0x10);
        let mut probe = NoMapping(DeviceTable::new());
        let id = probe.0.add(node).unwrap();
        assert!(matches!(
            probe_mmio(&mut probe, id),
            Err(ProbeError::Refused(_))
        ));
    }

    #[test]
    fn virtio_pci_capabilities_too_short_for_their_registers_are_refused() {
        let capability = |cfg_type, length| VirtioPciCapability {
            cfg_type,
            bar: 4,
            offset: 0,
            length,
            notify_off_multiplier: 4,
        };
        let common = capability(virtio_pci::COMMON, 0x38);
        let notify = capability(virtio_pci::NOTIFY, 2);
        assert!(usable_capabilities(Some(common), Some(notify)).is_ok());
        for (common, notify) in [
            (Some(capability(virtio_pci::COMMON, 0x37)), Some(notify)),
            (Some(common), Some(capability(virtio_pci::NOTIFY, 1))),
            (None, Some(notify)),
            (Some(common), None),
        ] {
            assert!(matches!(
                usable_capabilities(common, notify),
                Err(ProbeError::Refused(_))
            ));
        }
    }
}

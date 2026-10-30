//! virtio in the driver model (D110): the virtio-mmio and modern virtio-pci transports as bus
//! drivers, each adding the virtio device it finds as a child, and the virtio entropy device,
//! GPU (D112), input devices (D113, on interrupts since D184), block device (D130) and console
//! device (D136) on top of either. A device's driver connects its interrupt with
//! [`connect_interrupt`]: the slot's line through the platform's interrupt controller on
//! virtio-mmio, an MSI-X message on virtio-pci.

use super::dma::DmaRegion;
use super::interrupts::{self, DeviceEvents, Handler};
use super::{
    Deadline, DeviceId, DeviceNode, DmaCoherence, DriverDescriptor, Identity, Match, Probe,
    ProbeError, ProbePass,
};
use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use eggos_drivers::io::Mmio;
use eggos_drivers::pci;
use eggos_drivers::pci::msix::MsixTable;
use eggos_drivers::virtio::mmio::MmioTransport;
use eggos_drivers::virtio::pci::{self as virtio_pci, PciTransport, VirtioPciCapability};
use eggos_drivers::virtio::{self, QueueAddresses, Transport};
use eggos_hal::PhysicalRange;

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
    fn write_config8(&self, offset: usize, value: u8) {
        each_transport!(self, t => t.write_config8(offset, value))
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

/// Has the virtio device `id`, whose transport is `transport`, interrupt: its interrupt's handler
/// acknowledges it and raises `events`. On virtio-mmio, the slot's device-tree interrupt routed
/// through the platform's controller; on virtio-pci, MSI-X entry 0 for its configuration and
/// every queue the driver enabled, so call it once the queues are set up. The driver then asks
/// the device for interrupts ([`eggos_drivers::virtio::queue::SplitQueue`]'s notifications) and
/// looks at it once, for what came before. Marks the device as [`super::Delivery::Interrupts`].
pub fn connect_interrupt(
    probe: &mut dyn Probe,
    id: DeviceId,
    transport: &AnyTransport,
    events: &'static DeviceEvents,
) -> Result<(), ProbeError> {
    let node = *probe
        .table()
        .get(id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let parent_id = node.parent.ok_or(ProbeError::Refused(
        "the virtio device has no transport device",
    ))?;
    let parent = *probe
        .table()
        .get(parent_id)
        .ok_or(ProbeError::Refused("no such device"))?;
    let handler = match transport {
        AnyTransport::Mmio(_) => {
            let specifier = node.interrupts[0].ok_or(ProbeError::Refused(
                "the virtio-mmio slot names no interrupt",
            ))?;
            let region = parent
                .regions()
                .next()
                .ok_or(ProbeError::Refused("the slot has no registers"))?;
            let base = probe.map_window(region)?;
            let line = probe.route_interrupt(&specifier)?;
            (
                line,
                Handler {
                    acknowledge: acknowledge_mmio,
                    context: base,
                    events,
                },
            )
        }
        AnyTransport::Pci(pci_transport) => {
            let Identity::Pci(function) = parent.identity else {
                return Err(ProbeError::Refused("the transport is not a PCI function"));
            };
            let config = probe
                .pci_config()
                .ok_or(ProbeError::Refused("no PCI configuration space"))?;
            let msix = pci::msix::find(config, function.address)
                .ok_or(ProbeError::Refused("the function has no MSI-X"))?;
            let bar = parent.regions[usize::from(msix.bar)];
            let table_end = u64::from(msix.table_offset) + msix.table_bytes() as u64;
            if bar.is_empty() || table_end > bar.len() {
                return Err(ProbeError::Refused("the MSI-X table lies outside its BAR"));
            }
            let table = probe.map_window(PhysicalRange::from_start_length(
                bar.start + u64::from(msix.table_offset),
                msix.table_bytes() as u64,
            ))?;
            let (line, message) = probe.message_interrupt()?;
            // SAFETY: `map_window` mapped the table as device memory for good, inside the BAR the
            // capability names; only this driver reaches it.
            let table = MsixTable::new(
                unsafe { Mmio::new(table, msix.table_bytes()) },
                msix.entries,
            );
            table.set(0, message.address, message.data);
            let config = probe
                .pci_config()
                .ok_or(ProbeError::Refused("no PCI configuration space"))?;
            msix.enable(config, function.address);
            if !pci_transport.route_interrupts_to(0) {
                return Err(ProbeError::Refused("the device took no MSI-X vector"));
            }
            (
                line,
                Handler {
                    acknowledge: acknowledge_nothing,
                    context: 0,
                    events,
                },
            )
        }
    };
    interrupts::connect(handler.0, handler.1).map_err(ProbeError::Refused)?;
    if let Some(node) = probe.table().get_mut(id) {
        node.delivery = super::Delivery::Interrupts;
    }
    Ok(())
}

fn acknowledge_mmio(base: usize) {
    // SAFETY: `base` is a virtio-mmio slot's registers, which its bus driver mapped as device
    // memory for good; only the interrupt status and acknowledge registers are reached here,
    // which the device's driver never uses.
    let regs = unsafe { Mmio::new(base, MMIO_REGISTERS_LENGTH as usize) };
    virtio::mmio::acknowledge_interrupt(&regs);
}

fn acknowledge_nothing(_: usize) {}

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
    use eggos_drivers::virtio::gpu::{
        CURSOR_BYTES, CURSOR_SIDE, FALLBACK_MODE, MEMORY_BYTES, Released, VirtioGpu,
    };

    type Gpu = VirtioGpu<AnyTransport, DmaRegion>;

    const _: () = assert!(CURSOR_SIDE == eggos_drivers::desktop::CURSOR_SIDE);

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio GPU (virtio 1.2, 5.7), 2D: one framebuffer on scanout 0, installed as
    /// the kernel's display ([`crate::display`]), and the cursor, which draws the desktop's
    /// pointer when the device has a cursor queue (D186).
    pub static GPU_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-gpu",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::GPU)],
        probe: probe_gpu,
    };

    struct GpuScanout {
        gpu: Gpu,
        mode: Mode,
        cursor: bool,
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

        fn draws_pointer(&self) -> bool {
            self.cursor
        }

        fn pointer_image(&mut self) -> Option<&mut [u32]> {
            self.gpu.cursor_pixels()
        }

        fn show_pointer(
            &mut self,
            hotspot: (u32, u32),
            at: (u32, u32),
            deadline: &mut Deadline,
        ) -> Result<(), &'static str> {
            self.gpu
                .show_cursor(hotspot, at, &mut || deadline.keep_waiting())
                .map_err(virtio::Error::description)
        }

        fn move_pointer(
            &mut self,
            at: (u32, u32),
            deadline: &mut Deadline,
        ) -> Result<(), &'static str> {
            self.gpu
                .move_cursor(at, &mut || deadline.keep_waiting())
                .map_err(virtio::Error::description)
        }

        fn hide_pointer(&mut self, deadline: &mut Deadline) -> Result<(), &'static str> {
            self.gpu
                .hide_cursor(&mut || deadline.keep_waiting())
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
        let cursor = shown.is_ok() && attach_cursor(&mut gpu, &mut deadline);
        let refused = match shown {
            Ok(mode) => match crate::display::install(GpuScanout { gpu, mode, cursor }) {
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

    fn attach_cursor(gpu: &mut Gpu, deadline: &mut Deadline) -> bool {
        if !gpu.has_cursor_queue() {
            return false;
        }
        let Ok(image) = DmaRegion::allocate(CURSOR_BYTES) else {
            return false;
        };
        match gpu.attach_cursor(image, &mut || deadline.keep_waiting()) {
            Ok(()) => true,
            Err((_, image)) => {
                if let Some(image) = image {
                    // SAFETY: handed back by `attach_cursor`, which does so only when the device
                    // was never given it.
                    unsafe { image.free() };
                }
                false
            }
        }
    }

    fn release(gpu: Gpu, identity: &Identity, deadline: &mut Deadline) {
        match gpu.reset(&mut || deadline.keep_waiting()) {
            Ok(Released {
                transport,
                control,
                framebuffer,
                cursor,
                reset,
            }) => {
                control.free_after_reset(&reset);
                if let Some(framebuffer) = framebuffer {
                    framebuffer.free_after_reset(&reset);
                }
                if let Some(cursor) = cursor {
                    cursor.free_after_reset(&reset);
                }
                give_back_transport(identity, transport);
            }
            Err((_, transport)) => give_back_transport(identity, transport),
        }
    }
}

#[cfg(any(test, feature = "virtio-input"))]
pub use input::INPUT_DRIVER;

#[cfg(any(test, feature = "virtio-input"))]
mod input {
    use super::*;
    use crate::input::InputDevice;
    use eggos_drivers::input::{Capabilities, RawEvent};
    use eggos_drivers::virtio::input::{MEMORY_BYTES, VirtioInput};

    type Input = VirtioInput<AnyTransport, DmaRegion>;

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The virtio input device (virtio 1.2, 5.8): a keyboard, a mouse or a tablet, installed as
    /// one of the kernel's input devices ([`crate::input`]), on interrupts where the machine
    /// routes them (D184), polled otherwise.
    pub static INPUT_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-input",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::INPUT)],
        probe: probe_input,
    };

    struct VirtioInputDevice(Input);

    impl InputDevice for VirtioInputDevice {
        fn capabilities(&self) -> Capabilities {
            self.0.capabilities()
        }

        fn poll(&mut self, budget: usize, event: &mut dyn FnMut(RawEvent)) -> usize {
            self.0.poll(budget, &mut |raw| event(raw))
        }

        fn set_caps_lock_light(&mut self, on: bool) {
            self.0.set_caps_lock_light(on);
        }
    }

    fn probe_input(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let memory = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queue"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        let mut input = match VirtioInput::new(transport, memory, &mut || deadline.keep_waiting()) {
            Ok(input) => input,
            Err((error, transport, memory)) => {
                if let Some(memory) = memory {
                    // SAFETY: `VirtioInput::new` hands the memory back only when the device
                    // never had it or was reset after.
                    unsafe { memory.free() };
                }
                give_back_transport(&node.identity, transport);
                return Err(ProbeError::Refused(error.description()));
            }
        };
        let capabilities = input.capabilities();
        let refusal = if !capabilities.keyboard && !capabilities.is_pointer() {
            "neither a keyboard nor a pointer"
        } else if !crate::input::has_room() {
            "no room for another input device"
        } else {
            let interrupting =
                connect_interrupt(probe, id, input.transport(), &crate::input::DEVICE_EVENTS)
                    .is_ok();
            if interrupting {
                input.enable_interrupts();
            }
            let device = Box::new(VirtioInputDevice(input));
            let installed = if interrupting {
                crate::input::install_interrupting(device)
            } else {
                crate::input::install(device)
            };
            return installed.map_err(|_| ProbeError::Refused("no room for another input device"));
        };
        release(
            input,
            &node.identity,
            &mut probe.deadline(ANSWER_TIMEOUT_MS),
        );
        Err(ProbeError::Refused(refusal))
    }

    fn release(input: Input, identity: &Identity, deadline: &mut Deadline) {
        match input.reset(&mut || deadline.keep_waiting()) {
            Ok((transport, memory, reset)) => {
                memory.free_after_reset(&reset);
                give_back_transport(identity, transport);
            }
            Err((_, transport)) => give_back_transport(identity, transport),
        }
    }
}

#[cfg(any(test, feature = "virtio-blk"))]
pub use disk::BLOCK_DRIVER;

#[cfg(any(test, feature = "virtio-blk"))]
mod disk {
    use super::*;
    use crate::block::{BlockDevice, BlockError};
    use eggos_drivers::virtio::block::{MEMORY_BYTES, VirtioBlock};

    type Disk = VirtioBlock<AnyTransport, DmaRegion>;

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio block device (virtio 1.2, 5.2), installed as one of the kernel's
    /// disks ([`crate::block`]).
    pub static BLOCK_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-blk",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::BLOCK)],
        probe: probe_block,
    };

    struct VirtioDisk(Disk);

    fn block_error(error: virtio::Error) -> BlockError {
        match error {
            virtio::Error::ReadOnly => BlockError::ReadOnly,
            virtio::Error::PartialSector | virtio::Error::BeyondCapacity => BlockError::OutOfRange,
            error => BlockError::Device(error.description()),
        }
    }

    impl BlockDevice for VirtioDisk {
        fn sectors(&self) -> u64 {
            self.0.sectors()
        }

        fn is_read_only(&self) -> bool {
            self.0.is_read_only()
        }

        fn read(
            &mut self,
            sector: u64,
            buffer: &mut [u8],
            deadline: &mut Deadline,
        ) -> Result<(), BlockError> {
            self.0
                .read(sector, buffer, &mut || deadline.keep_waiting())
                .map_err(block_error)
        }

        fn write(
            &mut self,
            sector: u64,
            buffer: &[u8],
            deadline: &mut Deadline,
        ) -> Result<(), BlockError> {
            self.0
                .write(sector, buffer, &mut || deadline.keep_waiting())
                .map_err(block_error)
        }

        fn flush(&mut self, deadline: &mut Deadline) -> Result<(), BlockError> {
            self.0
                .flush(&mut || deadline.keep_waiting())
                .map_err(block_error)
        }
    }

    fn probe_block(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        if !crate::block::has_room() {
            return Err(ProbeError::Refused("no room for another disk"));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let memory = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queue"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        let disk = match VirtioBlock::new(transport, memory, &mut || deadline.keep_waiting()) {
            Ok(disk) => disk,
            Err((error, transport, memory)) => {
                if let Some(memory) = memory {
                    // SAFETY: `VirtioBlock::new` hands the memory back only when the device
                    // never had it or was reset after.
                    unsafe { memory.free() };
                }
                give_back_transport(&node.identity, transport);
                return Err(ProbeError::Refused(error.description()));
            }
        };
        crate::block::install(Box::new(VirtioDisk(disk)))
            .map_err(|_| ProbeError::Refused("no room for another disk"))
    }
}

#[cfg(any(test, feature = "virtio-console"))]
pub use agent::CONSOLE_DRIVER;

#[cfg(any(test, feature = "virtio-console"))]
mod agent {
    use super::*;
    use crate::agent_channel::{self, AgentPort};
    use eggos_drivers::virtio::console::{MEMORY_BYTES, VirtioConsole};

    type Console = VirtioConsole<AnyTransport, DmaRegion>;

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio console device (virtio 1.2, 5.3) with multiple ports, whose port
    /// named [`agent_channel::PORT_NAME`] becomes the agent channel (D136).
    pub static CONSOLE_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-console",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::CONSOLE)],
        probe: probe_console,
    };

    struct AgentConsole(Console);

    impl AgentPort for AgentConsole {
        fn has_input(&mut self) -> bool {
            self.0.has_input()
        }
        fn receive(&mut self, bytes: &mut [u8]) -> usize {
            self.0.receive(bytes)
        }
        fn host_connected(&self) -> bool {
            self.0.host_connected()
        }
        fn poll_control(&mut self) -> bool {
            self.0.poll_control()
        }
        fn is_sending(&mut self) -> bool {
            self.0.is_sending()
        }
        fn stage(&mut self, bytes: &[u8]) -> usize {
            self.0.stage(bytes)
        }
        fn send_staged(&mut self) -> Result<(), &'static str> {
            self.0.send_staged().map_err(virtio::Error::description)
        }
    }

    fn probe_console(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        if agent_channel::is_present() {
            return Err(ProbeError::Refused("an agent channel is already in use"));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let memory = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queues"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        let console = match VirtioConsole::new(transport, memory, &mut || deadline.keep_waiting()) {
            Ok(console) => console,
            Err((error, transport, memory)) => {
                if let Some(memory) = memory {
                    // SAFETY: `VirtioConsole::new` hands the memory back only when the device
                    // never had it or was reset after.
                    unsafe { memory.free() };
                }
                give_back_transport(&node.identity, transport);
                return Err(ProbeError::Refused(error.description()));
            }
        };
        if console.name() != agent_channel::PORT_NAME {
            return Err(ProbeError::Refused(
                "its port is not named org.eggos.mcp, the agent channel's name",
            ));
        }
        agent_channel::install(Box::new(AgentConsole(console)))
            .map_err(|_| ProbeError::Refused("an agent channel is already in use"))
    }
}

#[cfg(any(test, feature = "virtio-net"))]
pub use network::NET_DRIVER;

#[cfg(any(test, feature = "virtio-net"))]
mod network {
    use super::*;
    use crate::net::NetDevice;
    use eggos_drivers::virtio::net::{MEMORY_BYTES, VirtioNet};
    use eggos_net::ethernet::MacAddress;

    type Net = VirtioNet<AnyTransport, DmaRegion>;

    const ANSWER_TIMEOUT_MS: u64 = 1000;

    /// The polled virtio network device (virtio 1.2, 5.1), the stack's adapter (D157,
    /// [`crate::net`]).
    pub static NET_DRIVER: DriverDescriptor = DriverDescriptor {
        name: "virtio-net",
        pass: ProbePass::Device,
        matches: &[Match::VirtioId(virtio::device_type::NETWORK)],
        probe: probe_net,
    };

    struct VirtioAdapter(Net);

    impl NetDevice for VirtioAdapter {
        fn mac(&self) -> MacAddress {
            MacAddress(self.0.mac())
        }
        fn mtu(&self) -> usize {
            usize::from(self.0.mtu())
        }
        fn is_link_up(&self) -> bool {
            self.0.is_link_up()
        }
        fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
            self.0.receive(frame)
        }
        fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
            self.0.transmit(frame).map_err(virtio::Error::description)
        }
    }

    fn probe_net(probe: &mut dyn Probe, id: DeviceId) -> Result<(), ProbeError> {
        let node = *probe
            .table()
            .get(id)
            .ok_or(ProbeError::Refused("no such device"))?;
        if node.dma != DmaCoherence::Coherent {
            return Err(ProbeError::Refused(
                "the device's DMA is not coherent, which the kernel does not support yet",
            ));
        }
        if crate::net::is_present() {
            return Err(ProbeError::Refused("a network adapter is already in use"));
        }
        let transport =
            take_transport(&node.identity).ok_or(ProbeError::Refused("no transport"))?;
        let memory = DmaRegion::allocate(MEMORY_BYTES)
            .map_err(|_| ProbeError::Refused("no memory for the device's queues"))?;
        let mut deadline = probe.deadline(ANSWER_TIMEOUT_MS);
        let net = match VirtioNet::new(transport, memory, &mut || deadline.keep_waiting()) {
            Ok(net) => net,
            Err((error, transport, memory)) => {
                if let Some(memory) = memory {
                    // SAFETY: `VirtioNet::new` hands the memory back only when the device
                    // never had it or was reset after.
                    unsafe { memory.free() };
                }
                give_back_transport(&node.identity, transport);
                return Err(ProbeError::Refused(error.description()));
            }
        };
        crate::net::install(Box::new(VirtioAdapter(net)))
            .map_err(|_| ProbeError::Refused("a network adapter is already in use"))
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

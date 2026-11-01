//! virtio devices (virtio 1.2): the transports, the split virtqueue and the device drivers.
//!
//! A device driver is generic over [`Transport`], so it runs over virtio-mmio
//! ([`mmio::MmioTransport`]) and modern virtio-pci ([`pci::PciTransport`]) alike, and against a
//! fake transport in host tests. Only version 1 (non-legacy) devices are driven:
//! [`feature::VERSION_1`] is required.

pub mod block;
pub mod console;
pub mod gpu;
pub mod input;
pub mod mmio;
pub mod net;
pub mod pci;
pub mod queue;
pub mod rng;

#[cfg(test)]
mod tests;

use crate::dma::DmaAddr;

/// Device status bits (virtio 1.2, 2.1).
pub mod status {
    /// The driver has noticed the device.
    pub const ACKNOWLEDGE: u8 = 1;
    /// The driver knows how to drive the device.
    pub const DRIVER: u8 = 2;
    /// The driver is set up and ready to drive the device.
    pub const DRIVER_OK: u8 = 4;
    /// The driver has acknowledged the features it understands.
    pub const FEATURES_OK: u8 = 8;
    /// The device has hit an error it cannot recover from without a reset.
    pub const DEVICE_NEEDS_RESET: u8 = 64;
    /// The driver has given up on the device.
    pub const FAILED: u8 = 128;
}

/// Device-independent feature bits (virtio 1.2, 6).
pub mod feature {
    /// The device complies with virtio 1.0 or later (not legacy).
    pub const VERSION_1: u64 = 1 << 32;
    /// The device reaches memory through the platform's DMA translation (an IOMMU, if any).
    pub const ACCESS_PLATFORM: u64 = 1 << 33;
    /// Accesses are ordered as the platform orders them for real devices.
    pub const ORDER_PLATFORM: u64 = 1 << 36;
}

/// Device types (virtio 1.2, 5).
pub mod device_type {
    pub const NETWORK: u32 = 1;
    pub const BLOCK: u32 = 2;
    pub const CONSOLE: u32 = 3;
    pub const ENTROPY: u32 = 4;
    pub const GPU: u32 = 16;
    pub const INPUT: u32 = 18;
}

/// A short name for virtio device type `device_type`.
pub fn device_type_name(device_type: u32) -> &'static str {
    match device_type {
        device_type::NETWORK => "network",
        device_type::BLOCK => "block",
        device_type::CONSOLE => "console",
        device_type::ENTROPY => "entropy",
        device_type::GPU => "gpu",
        device_type::INPUT => "input",
        _ => "unknown",
    }
}

/// Why a virtio device could not be driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The registers do not start with the virtio-mmio magic value.
    NotVirtio,
    /// A legacy (version 1) virtio-mmio device.
    LegacyVersion,
    /// A virtio-mmio version this driver does not know.
    UnsupportedVersion(u32),
    /// An empty virtio-mmio slot (device id 0).
    NoDevice,
    /// The device's status did not read back 0 after a reset.
    ResetTimedOut,
    /// The device cleared `FEATURES_OK`: it refused the features accepted.
    FeaturesRefused,
    /// The device does not offer [`feature::VERSION_1`].
    MissingFeatures,
    /// The queue does not exist or is already in use.
    QueueUnavailable,
    /// The queue size is not a power of two from 1 to 32768.
    QueueTooSmall,
    /// Not enough free descriptors for the chain.
    QueueFull,
    /// The DMA memory is too small for the queue and buffers.
    MemoryTooSmall,
    /// The device did not answer in time.
    TimedOut,
    /// The PCI capabilities do not describe a usable modern device.
    BadCapabilities,
    /// A register the device names lies outside the BAR region mapped for it.
    BarOutOfRange,
    /// The device is not of the type the driver drives.
    WrongDeviceType,
    /// The device answered a command with this error response type.
    CommandFailed(u32),
    /// The device has no display output (scanout), or the driver has not set one up yet.
    NoScanout,
    /// The display output already shows a framebuffer.
    ScanoutInUse,
    /// A display mode the driver cannot set: empty, or too large for one region of memory.
    UnsupportedMode,
    /// The device refuses writes.
    ReadOnly,
    /// A transfer that is not a whole number of sectors.
    PartialSector,
    /// A transfer past the end of the disk.
    BeyondCapacity,
    /// The console device has no data port the driver can use, or never named it.
    NoPort,
    /// The network device does not say its hardware (MAC) address.
    NoHardwareAddress,
    /// A frame longer than the network device's buffers.
    FrameTooLong,
    /// The GPU has no cursor queue, or the driver has not set its cursor up.
    NoCursor,
}

impl Error {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            Error::NotVirtio => "not a virtio-mmio device (bad magic value)",
            Error::LegacyVersion => {
                "a legacy (version 1) virtio-mmio device: only version 2 is supported \
                 (QEMU: -global virtio-mmio.force-legacy=false)"
            }
            Error::UnsupportedVersion(_) => "an unknown virtio-mmio version",
            Error::NoDevice => "an empty virtio-mmio slot",
            Error::ResetTimedOut => "the device status did not read back 0 after a reset",
            Error::FeaturesRefused => "the device refused the features accepted",
            Error::MissingFeatures => "the device does not offer VIRTIO_F_VERSION_1",
            Error::QueueUnavailable => "the virtqueue does not exist or is in use",
            Error::QueueTooSmall => "the virtqueue size is not a power of two up to 32768",
            Error::QueueFull => "no free virtqueue descriptors",
            Error::MemoryTooSmall => "the DMA memory is too small",
            Error::TimedOut => "the device did not answer in time",
            Error::BadCapabilities => "the virtio-pci capabilities are missing or malformed",
            Error::BarOutOfRange => "a virtio register lies outside its mapped BAR region",
            Error::WrongDeviceType => "the device is not of the type the driver drives",
            Error::CommandFailed(_) => "the device answered a command with an error",
            Error::NoScanout => "the device has no display output, or none is set up",
            Error::ScanoutInUse => "the display output already shows a framebuffer",
            Error::UnsupportedMode => "a display mode the driver cannot set",
            Error::ReadOnly => "the device is read-only",
            Error::PartialSector => "a transfer that is not a whole number of sectors",
            Error::BeyondCapacity => "a transfer past the end of the disk",
            Error::NoPort => "the console device offers no named port with multiport support",
            Error::NoHardwareAddress => "the network device does not say its hardware address",
            Error::FrameTooLong => "a frame longer than the network device's buffers",
            Error::NoCursor => "the GPU has no cursor queue, or its cursor is not set up",
        }
    }
}

/// Where a virtqueue's three areas are, as the device reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueAddresses {
    /// The descriptor table.
    pub descriptors: DmaAddr,
    /// The driver area (the available ring).
    pub driver: DmaAddr,
    /// The device area (the used ring).
    pub device: DmaAddr,
}

/// How a driver reaches a virtio device's common registers, whatever the bus.
pub trait Transport {
    /// The device type (see [`device_type`]).
    fn device_type(&self) -> u32;
    /// The device status (see [`status`]).
    fn status(&self) -> u8;
    /// Writes the device status; 0 resets the device.
    fn set_status(&self, status: u8);
    /// The 64 feature bits the device offers.
    fn device_features(&self) -> u64;
    /// Writes the 64 feature bits the driver accepts.
    fn set_driver_features(&self, features: u64);
    /// The largest size queue `queue` takes; 0 when there is no such queue or it is in use.
    fn max_queue_size(&self, queue: u16) -> u16;
    /// Sets queue `queue` up with `size` entries at `addresses` and enables it.
    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error>;
    /// Tells the device queue `queue` has new available buffers.
    fn notify(&self, queue: u16);
    /// The byte at `offset` of the device-specific configuration.
    fn read_config8(&self, offset: usize) -> u8;
    /// Writes the byte at `offset` of the device-specific configuration.
    fn write_config8(&self, offset: usize, value: u8);
    /// The 32-bit word at `offset` of the device-specific configuration.
    fn read_config32(&self, offset: usize) -> u32;
}

/// Proof that a device's status read back 0 after a reset: it no longer reads or writes the
/// memory it was given, which may now be freed.
#[derive(Debug)]
pub struct ResetDone(());

/// Resets the device: writes status 0 and polls it until it reads 0, while `keep_waiting`
/// says to.
pub fn reset<T: Transport + ?Sized>(
    transport: &T,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<ResetDone, Error> {
    transport.set_status(0);
    loop {
        if transport.status() == 0 {
            return Ok(ResetDone(()));
        }
        if !keep_waiting() {
            return Err(Error::ResetTimedOut);
        }
    }
}

/// Initializes the device up to `FEATURES_OK` (virtio 1.2, 3.1.1): reset, `ACKNOWLEDGE`,
/// `DRIVER`, then accepts the offered features among `wanted`, [`feature::VERSION_1`] (required)
/// and [`feature::ACCESS_PLATFORM`]. Returns the accepted features; on a refusal the device is
/// marked `FAILED`. The caller sets its queues up, then calls [`driver_ok`].
pub fn initialize<T: Transport + ?Sized>(
    transport: &T,
    wanted: u64,
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<u64, Error> {
    reset(transport, keep_waiting)?;
    transport.set_status(status::ACKNOWLEDGE);
    transport.set_status(status::ACKNOWLEDGE | status::DRIVER);
    let offered = transport.device_features();
    if offered & feature::VERSION_1 == 0 {
        fail(transport);
        return Err(Error::MissingFeatures);
    }
    let accepted = offered & (wanted | feature::VERSION_1 | feature::ACCESS_PLATFORM);
    transport.set_driver_features(accepted);
    let features_ok = status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK;
    transport.set_status(features_ok);
    if transport.status() & status::FEATURES_OK == 0 {
        fail(transport);
        return Err(Error::FeaturesRefused);
    }
    Ok(accepted)
}

/// Tells the device the driver is ready (`DRIVER_OK`).
pub fn driver_ok<T: Transport + ?Sized>(transport: &T) {
    transport.set_status(transport.status() | status::DRIVER_OK);
}

/// Marks the device as given up on (`FAILED`).
pub fn fail<T: Transport + ?Sized>(transport: &T) {
    transport.set_status(transport.status() | status::FAILED);
}

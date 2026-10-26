//! The modern virtio-pci transport (virtio 1.2, 4.1): the device's register structures live in
//! its BARs, where vendor-specific PCI capabilities point. Legacy and transitional devices (I/O
//! port registers) are not driven.

use super::{Error, QueueAddresses, Transport};
use crate::io::RegisterIo;
use crate::pci::{self, Address, ConfigSpace};
use core::cell::Cell;

/// The PCI vendor id of virtio devices.
pub const VIRTIO_VENDOR_ID: u16 = 0x1af4;

/// The PCI capability id of vendor-specific capabilities, which virtio's are.
pub const VENDOR_SPECIFIC_CAPABILITY: u8 = 0x09;

/// `cfg_type` of the common configuration structure.
pub const COMMON: u8 = 1;
/// `cfg_type` of the notification structure.
pub const NOTIFY: u8 = 2;
/// `cfg_type` of the ISR status structure.
pub const ISR: u8 = 3;
/// `cfg_type` of the device-specific configuration structure.
pub const DEVICE: u8 = 4;
/// `cfg_type` of the PCI configuration access structure.
pub const PCI: u8 = 5;

/// The PCI device ids of modern (non-transitional) virtio devices: 0x1040 plus the device type.
pub const MODERN_DEVICE_IDS: core::ops::RangeInclusive<u16> = 0x1040..=0x107f;

const DEVICE_FEATURE_SELECT: usize = 0x00;
const DEVICE_FEATURE: usize = 0x04;
const DRIVER_FEATURE_SELECT: usize = 0x08;
const DRIVER_FEATURE: usize = 0x0c;
const NUM_QUEUES: usize = 0x12;
const DEVICE_STATUS: usize = 0x14;
const QUEUE_SELECT: usize = 0x16;
const QUEUE_SIZE: usize = 0x18;
const QUEUE_ENABLE: usize = 0x1c;
const QUEUE_NOTIFY_OFF: usize = 0x1e;
const QUEUE_DESC: usize = 0x20;
const QUEUE_DRIVER: usize = 0x28;
const QUEUE_DEVICE: usize = 0x30;

const REMEMBERED_QUEUES: usize = 8;

/// The virtio device type of a modern virtio-pci device with PCI device id `pci_device_id`;
/// `None` for transitional (legacy) ids and anything else.
pub fn modern_device_type(pci_device_id: u16) -> Option<u32> {
    MODERN_DEVICE_IDS
        .contains(&pci_device_id)
        .then(|| u32::from(pci_device_id - MODERN_DEVICE_IDS.start()))
}

/// One virtio structure a vendor-specific capability locates (virtio 1.2, 4.1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtioPciCapability {
    /// Which structure ([`COMMON`], [`NOTIFY`], [`ISR`], [`DEVICE`] or [`PCI`]).
    pub cfg_type: u8,
    /// The BAR (0-5) that holds it.
    pub bar: u8,
    /// Its offset in the BAR.
    pub offset: u32,
    /// Its length in bytes.
    pub length: u32,
    /// For [`NOTIFY`]: the multiplier of a queue's notify offset; 0 otherwise.
    pub notify_off_multiplier: u32,
}

/// The virtio structures the function at `address` lists, in capability-list order; those in a
/// BAR above 5 are skipped.
pub fn virtio_capabilities<C: ConfigSpace + ?Sized>(
    config: &C,
    address: Address,
) -> impl Iterator<Item = VirtioPciCapability> + '_ {
    pci::capabilities(config, address)
        .filter(|capability| capability.id == VENDOR_SPECIFIC_CAPABILITY)
        .filter_map(move |capability| {
            let at = u16::from(capability.offset);
            let cfg_type = config.read8(address, at + 3);
            let bar = config.read8(address, at + 4);
            let notify_off_multiplier = if cfg_type == NOTIFY {
                config.read32(address, at + 16)
            } else {
                0
            };
            (bar <= 5).then(|| VirtioPciCapability {
                cfg_type,
                bar,
                offset: config.read32(address, at + 8),
                length: config.read32(address, at + 12),
                notify_off_multiplier,
            })
        })
}

/// A modern virtio-pci device, reached through its structures' regions.
#[derive(Debug)]
pub struct PciTransport<R: RegisterIo> {
    device_type: u32,
    common: R,
    notify: R,
    notify_length: usize,
    notify_off_multiplier: u32,
    device: Option<R>,
    notify_offsets: [Cell<Option<usize>>; REMEMBERED_QUEUES],
}

impl<R: RegisterIo> PciTransport<R> {
    /// The device of type `device_type` whose common configuration is `common`, notification
    /// structure `notify` (`notify_length` bytes) with `notify_off_multiplier`, and
    /// device-specific configuration `device`, each region starting at its structure.
    pub fn new(
        device_type: u32,
        common: R,
        notify: R,
        notify_length: usize,
        notify_off_multiplier: u32,
        device: Option<R>,
    ) -> Self {
        Self {
            device_type,
            common,
            notify,
            notify_length,
            notify_off_multiplier,
            device,
            notify_offsets: [const { Cell::new(None) }; REMEMBERED_QUEUES],
        }
    }

    fn write64(&self, offset: usize, value: u64) {
        self.common.write32(offset, value as u32);
        self.common.write32(offset + 4, (value >> 32) as u32);
    }

    fn notify_offset(&self, queue: u16) -> Option<usize> {
        self.common.write16(QUEUE_SELECT, queue);
        let offset = usize::from(self.common.read16(QUEUE_NOTIFY_OFF))
            .checked_mul(self.notify_off_multiplier as usize)?;
        (offset.checked_add(2)? <= self.notify_length).then_some(offset)
    }
}

impl<R: RegisterIo> Transport for PciTransport<R> {
    fn device_type(&self) -> u32 {
        self.device_type
    }

    fn status(&self) -> u8 {
        self.common.read8(DEVICE_STATUS)
    }

    fn set_status(&self, status: u8) {
        self.common.write8(DEVICE_STATUS, status);
    }

    fn device_features(&self) -> u64 {
        self.common.write32(DEVICE_FEATURE_SELECT, 0);
        let low = self.common.read32(DEVICE_FEATURE);
        self.common.write32(DEVICE_FEATURE_SELECT, 1);
        let high = self.common.read32(DEVICE_FEATURE);
        u64::from(high) << 32 | u64::from(low)
    }

    fn set_driver_features(&self, features: u64) {
        self.common.write32(DRIVER_FEATURE_SELECT, 0);
        self.common.write32(DRIVER_FEATURE, features as u32);
        self.common.write32(DRIVER_FEATURE_SELECT, 1);
        self.common.write32(DRIVER_FEATURE, (features >> 32) as u32);
    }

    fn max_queue_size(&self, queue: u16) -> u16 {
        if queue >= self.common.read16(NUM_QUEUES) {
            return 0;
        }
        self.common.write16(QUEUE_SELECT, queue);
        if self.common.read16(QUEUE_ENABLE) != 0 {
            return 0;
        }
        self.common.read16(QUEUE_SIZE)
    }

    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        let notify_offset = self.notify_offset(queue).ok_or(Error::BarOutOfRange)?;
        if let Some(slot) = self.notify_offsets.get(usize::from(queue)) {
            slot.set(Some(notify_offset));
        }
        self.common.write16(QUEUE_SELECT, queue);
        self.common.write16(QUEUE_SIZE, size);
        self.write64(QUEUE_DESC, addresses.descriptors.get());
        self.write64(QUEUE_DRIVER, addresses.driver.get());
        self.write64(QUEUE_DEVICE, addresses.device.get());
        self.common.write16(QUEUE_ENABLE, 1);
        Ok(())
    }

    fn notify(&self, queue: u16) {
        let remembered = self
            .notify_offsets
            .get(usize::from(queue))
            .and_then(Cell::get);
        if let Some(offset) = remembered.or_else(|| self.notify_offset(queue)) {
            self.notify.write16(offset, queue);
        }
    }

    fn read_config8(&self, offset: usize) -> u8 {
        self.device
            .as_ref()
            .map_or(0, |device| device.read8(offset))
    }

    fn write_config8(&self, offset: usize, value: u8) {
        if let Some(device) = self.device.as_ref() {
            device.write8(offset, value);
        }
    }

    fn read_config32(&self, offset: usize) -> u32 {
        self.device
            .as_ref()
            .map_or(0, |device| device.read32(offset))
    }
}

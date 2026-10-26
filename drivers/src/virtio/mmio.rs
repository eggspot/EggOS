//! The virtio-mmio transport, version 2 (virtio 1.2, 4.2): a device's registers in one
//! memory-mapped block, as on QEMU's Arm and RISC-V `virt` machines.

use super::{Error, QueueAddresses, Transport};
use crate::io::RegisterIo;

/// The bytes of one virtio-mmio register block this transport reaches: the registers and the
/// start of the device-specific configuration (QEMU `virt` on Arm spaces its slots 0x200 apart).
pub const REGISTER_BLOCK_SIZE: usize = 0x200;

/// The value of the `MagicValue` register: "virt" in little-endian.
pub const MAGIC: u32 = 0x7472_6976;

const MAGIC_VALUE: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const VENDOR_ID: usize = 0x00c;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;
const CONFIG: usize = 0x100;

/// A version 2 virtio-mmio device.
#[derive(Debug)]
pub struct MmioTransport<R: RegisterIo> {
    regs: R,
    device_type: u32,
}

impl<R: RegisterIo> MmioTransport<R> {
    /// The device behind the register block `regs`. Refuses blocks without the magic value, an
    /// empty slot (device id 0, whatever its version), a legacy (version 1) device and versions
    /// other than 2.
    pub fn new(regs: R) -> Result<Self, Error> {
        if regs.read32(MAGIC_VALUE) != MAGIC {
            return Err(Error::NotVirtio);
        }
        let device_type = regs.read32(DEVICE_ID);
        if device_type == 0 {
            return Err(Error::NoDevice);
        }
        match regs.read32(VERSION) {
            2 => Ok(Self { regs, device_type }),
            1 => Err(Error::LegacyVersion),
            version => Err(Error::UnsupportedVersion(version)),
        }
    }

    /// The device's vendor id.
    pub fn vendor_id(&self) -> u32 {
        self.regs.read32(VENDOR_ID)
    }

    fn write64(&self, low: usize, high: usize, value: u64) {
        self.regs.write32(low, value as u32);
        self.regs.write32(high, (value >> 32) as u32);
    }
}

impl<R: RegisterIo> Transport for MmioTransport<R> {
    fn device_type(&self) -> u32 {
        self.device_type
    }

    fn status(&self) -> u8 {
        self.regs.read32(STATUS) as u8
    }

    fn set_status(&self, status: u8) {
        self.regs.write32(STATUS, status.into());
    }

    fn device_features(&self) -> u64 {
        self.regs.write32(DEVICE_FEATURES_SEL, 0);
        let low = self.regs.read32(DEVICE_FEATURES);
        self.regs.write32(DEVICE_FEATURES_SEL, 1);
        let high = self.regs.read32(DEVICE_FEATURES);
        u64::from(high) << 32 | u64::from(low)
    }

    fn set_driver_features(&self, features: u64) {
        self.regs.write32(DRIVER_FEATURES_SEL, 0);
        self.regs.write32(DRIVER_FEATURES, features as u32);
        self.regs.write32(DRIVER_FEATURES_SEL, 1);
        self.regs.write32(DRIVER_FEATURES, (features >> 32) as u32);
    }

    fn max_queue_size(&self, queue: u16) -> u16 {
        self.regs.write32(QUEUE_SEL, queue.into());
        if self.regs.read32(QUEUE_READY) != 0 {
            return 0;
        }
        self.regs.read32(QUEUE_NUM_MAX).min(u16::MAX.into()) as u16
    }

    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        self.regs.write32(QUEUE_SEL, queue.into());
        self.regs.write32(QUEUE_NUM, size.into());
        self.write64(QUEUE_DESC_LOW, QUEUE_DESC_HIGH, addresses.descriptors.get());
        self.write64(QUEUE_DRIVER_LOW, QUEUE_DRIVER_HIGH, addresses.driver.get());
        self.write64(QUEUE_DEVICE_LOW, QUEUE_DEVICE_HIGH, addresses.device.get());
        self.regs.write32(QUEUE_READY, 1);
        Ok(())
    }

    fn notify(&self, queue: u16) {
        self.regs.write32(QUEUE_NOTIFY, queue.into());
    }

    fn read_config8(&self, offset: usize) -> u8 {
        self.regs.read8(CONFIG + offset)
    }

    fn read_config32(&self, offset: usize) -> u32 {
        self.regs.read32(CONFIG + offset)
    }
}

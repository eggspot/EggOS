//! The virtio-ccw transport, revision 1 (virtio 1.2, 4.3): a virtio device on an s390x I/O
//! subchannel whose control unit is virtio's, reached through channel programs of one command
//! each, and notified through the hypervisor's `DIAGNOSE 0x500`.
//!
//! The transport's user holds it alone: every command shares its one [`ChannelBuffer`].

use super::{Error, QueueAddresses, Transport, status};
use crate::channel::{self, ChannelBuffer, ChannelError, ChannelSubsystem, SubchannelId};
use crate::dma::DmaMemory;
use core::cell::UnsafeCell;

/// The control-unit type of a virtio-ccw device; its model is the virtio device type.
pub const CONTROL_UNIT_TYPE: u16 = 0x3832;

/// The virtio-ccw channel commands.
pub mod command {
    pub const WRITE_FEATURES: u8 = 0x11;
    pub const READ_FEATURES: u8 = 0x12;
    pub const SET_QUEUE: u8 = 0x13;
    pub const WRITE_CONFIG: u8 = 0x21;
    pub const READ_CONFIG: u8 = 0x22;
    pub const WRITE_STATUS: u8 = 0x31;
    pub const READ_QUEUE_CONFIG: u8 = 0x32;
    pub const RESET: u8 = 0x33;
    pub const SET_INDICATORS: u8 = 0x43;
    pub const SET_CONFIG_INDICATOR: u8 = 0x53;
    pub const READ_STATUS: u8 = 0x72;
    pub const SET_ADAPTER_INDICATORS: u8 = 0x73;
    pub const SET_REVISION: u8 = 0x83;
}

/// The transport revision driven: virtio 1 devices, `READ_STATUS` included.
pub const REVISION: u16 = 1;

const FEATURE_WORDS: u8 = 2;
const FEATURE_BYTES: u16 = 5;
const QUEUE_CONFIG_BYTES: u16 = 4;
const QUEUE_INFO_BYTES: u16 = 32;
const REVISION_BYTES: u16 = 4;

impl From<ChannelError> for Error {
    fn from(error: ChannelError) -> Self {
        match error {
            ChannelError::OutOfReach => Error::MemoryTooSmall,
            ChannelError::TimedOut => Error::TimedOut,
            other => Error::Channel(other),
        }
    }
}

/// A revision 1 virtio-ccw device.
#[derive(Debug)]
pub struct CcwTransport<M: DmaMemory> {
    channels: &'static dyn ChannelSubsystem,
    subchannel: SubchannelId,
    device_type: u32,
    buffer: UnsafeCell<ChannelBuffer<M>>,
}

impl<M: DmaMemory> CcwTransport<M> {
    /// The device on `subchannel`, its channel programs in `memory` (at least
    /// [`channel::BUFFER_BYTES`], below 2 GiB). Enables the subchannel, checks the control unit
    /// is virtio's (`SENSE ID`) and sets revision 1; refuses a legacy-only device.
    pub fn new(
        channels: &'static dyn ChannelSubsystem,
        subchannel: SubchannelId,
        memory: M,
    ) -> Result<Self, (Error, M)> {
        let mut buffer = ChannelBuffer::new(memory).map_err(|(error, memory)| (error.into(), memory))?;
        match Self::identify(channels, subchannel, &mut buffer) {
            Ok(device_type) => Ok(Self {
                channels,
                subchannel,
                device_type,
                buffer: UnsafeCell::new(buffer),
            }),
            Err(error) => Err((error, buffer.into_memory())),
        }
    }

    fn identify(
        channels: &dyn ChannelSubsystem,
        subchannel: SubchannelId,
        buffer: &mut ChannelBuffer<M>,
    ) -> Result<u32, Error> {
        channel::enable(channels, subchannel)?;
        let id = channel::sense_id(channels, subchannel, buffer)?;
        if id.control_unit_type != CONTROL_UNIT_TYPE {
            return Err(Error::NotVirtio);
        }
        if id.control_unit_model == 0 {
            return Err(Error::NoDevice);
        }
        let data = buffer.data_mut();
        data[..2].copy_from_slice(&REVISION.to_be_bytes());
        data[2..4].copy_from_slice(&0u16.to_be_bytes());
        match buffer.run(channels, subchannel, command::SET_REVISION, REVISION_BYTES) {
            Ok(_) => Ok(u32::from(id.control_unit_model)),
            Err(ChannelError::UnitCheck) => Err(Error::LegacyVersion),
            Err(error) => Err(error.into()),
        }
    }

    /// The subchannel the device is on.
    pub fn subchannel(&self) -> SubchannelId {
        self.subchannel
    }

    /// The memory back, once the device was reset.
    pub fn into_memory(self) -> M {
        self.buffer.into_inner().into_memory()
    }

    fn with_buffer<R>(&self, use_buffer: impl FnOnce(&mut ChannelBuffer<M>) -> R) -> R {
        // SAFETY: the transport's user holds it alone (the module's contract), and no method
        // calls another while it holds the buffer, so this is the buffer's only reference.
        use_buffer(unsafe { &mut *self.buffer.get() })
    }

    fn run(&self, command: u8, count: u16, fill: impl FnOnce(&mut [u8])) -> Result<[u8; 8], ChannelError> {
        self.with_buffer(|buffer| {
            fill(buffer.data_mut());
            buffer.run(self.channels, self.subchannel, command, count)?;
            let mut first = [0; 8];
            first.copy_from_slice(&buffer.data()[..8]);
            Ok(first)
        })
    }

    fn read_config(&self, offset: usize, bytes: usize, read: impl FnOnce(&[u8])) -> bool {
        let Some(end) = offset.checked_add(bytes).filter(|&end| end <= channel::DATA_BYTES) else {
            return false;
        };
        self.with_buffer(|buffer| {
            buffer.data_mut()[..end].fill(0);
            if buffer.run(self.channels, self.subchannel, command::READ_CONFIG, end as u16).is_err() {
                return false;
            }
            read(&buffer.data()[offset..end]);
            true
        })
    }
}

impl<M: DmaMemory> Transport for CcwTransport<M> {
    fn device_type(&self) -> u32 {
        self.device_type
    }

    fn status(&self) -> u8 {
        match self.run(command::READ_STATUS, 1, |data| data[0] = 0) {
            Ok(data) => data[0],
            Err(_) => status::FAILED,
        }
    }

    fn set_status(&self, status: u8) {
        let _ = if status == 0 {
            self.run(command::RESET, 0, |_| {})
        } else {
            self.run(command::WRITE_STATUS, 1, |data| data[0] = status)
        };
    }

    fn device_features(&self) -> u64 {
        (0..FEATURE_WORDS).fold(0, |features, index| {
            let word = self
                .run(command::READ_FEATURES, FEATURE_BYTES, |data| {
                    data[..4].fill(0);
                    data[4] = index;
                })
                .map_or(0, |data| u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
            features | u64::from(word) << (32 * u32::from(index))
        })
    }

    fn set_driver_features(&self, features: u64) {
        for index in 0..FEATURE_WORDS {
            let word = (features >> (32 * u32::from(index))) as u32;
            let _ = self.run(command::WRITE_FEATURES, FEATURE_BYTES, |data| {
                data[..4].copy_from_slice(&word.to_le_bytes());
                data[4] = index;
            });
        }
    }

    fn max_queue_size(&self, queue: u16) -> u16 {
        self.run(command::READ_QUEUE_CONFIG, QUEUE_CONFIG_BYTES, |data| {
            data[..2].copy_from_slice(&queue.to_be_bytes());
            data[2..4].fill(0);
        })
        .map_or(0, |data| u16::from_be_bytes([data[2], data[3]]))
    }

    fn set_up_queue(&self, queue: u16, size: u16, addresses: &QueueAddresses) -> Result<(), Error> {
        self.with_buffer(|buffer| {
            let data = buffer.data_mut();
            data[..8].copy_from_slice(&addresses.descriptors.get().to_be_bytes());
            data[8..12].fill(0);
            data[12..14].copy_from_slice(&queue.to_be_bytes());
            data[14..16].copy_from_slice(&size.to_be_bytes());
            data[16..24].copy_from_slice(&addresses.driver.get().to_be_bytes());
            data[24..32].copy_from_slice(&addresses.device.get().to_be_bytes());
            buffer
                .run(self.channels, self.subchannel, command::SET_QUEUE, QUEUE_INFO_BYTES)
                .map(|_| ())
                .map_err(|_| Error::QueueUnavailable)
        })
    }

    fn notify(&self, queue: u16) {
        self.channels.notify_virtio_queue(self.subchannel, queue, 0);
    }

    fn read_config8(&self, offset: usize) -> u8 {
        let mut value = 0;
        self.read_config(offset, 1, |bytes| value = bytes[0]);
        value
    }

    fn write_config8(&self, offset: usize, value: u8) {
        let Some(end) = offset.checked_add(1).filter(|&end| end <= channel::DATA_BYTES) else {
            return;
        };
        self.with_buffer(|buffer| {
            if buffer.run(self.channels, self.subchannel, command::READ_CONFIG, end as u16).is_err() {
                return;
            }
            buffer.data_mut()[offset] = value;
            let _ = buffer.run(self.channels, self.subchannel, command::WRITE_CONFIG, end as u16);
        });
    }

    fn read_config32(&self, offset: usize) -> u32 {
        let mut value = 0;
        self.read_config(offset, 4, |bytes| {
            value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        });
        value
    }
}

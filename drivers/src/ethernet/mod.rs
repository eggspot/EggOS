//! Wired Ethernet adapters on PCI (D240): Intel's e1000 and e1000e (82540 to I219), Intel's igc
//! (I225 and I226) and Realtek's RTL8111/8168 and RTL8125. Each keeps a ring of 32 receive and 32
//! transmit descriptors with a 2 KiB buffer each in one block of DMA memory, and is polled.

pub mod e1000;
pub mod igc;
pub mod realtek;

use crate::dma::DmaMemory;
use crate::storage::{copy_in, copy_out};

/// Descriptors per ring.
pub const RING_ENTRIES: usize = 32;
/// Bytes of each frame buffer.
pub const BUFFER_BYTES: usize = 2048;
/// The largest frame the adapters send or take, without its CRC.
pub const MAX_FRAME_BYTES: usize = 1518;

pub(crate) const RECEIVE_RING: usize = 0x0000;
pub(crate) const TRANSMIT_RING: usize = 0x1000;
pub(crate) const RECEIVE_BUFFERS: usize = 0x2000;
pub(crate) const TRANSMIT_BUFFERS: usize = RECEIVE_BUFFERS + RING_ENTRIES * BUFFER_BYTES;
/// DMA memory an adapter needs.
pub const MEMORY_BYTES: usize = TRANSMIT_BUFFERS + RING_ENTRIES * BUFFER_BYTES;

/// Why an adapter could not be used or a frame not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TimedOut,
    NoAddress,
    QueueFull,
    FrameTooLarge,
    Unsupported(&'static str),
}

impl Error {
    pub fn description(self) -> &'static str {
        match self {
            Error::TimedOut => "the network adapter did not answer in time",
            Error::NoAddress => "the network adapter has no hardware address",
            Error::QueueFull => "the network adapter's transmit queue is full",
            Error::FrameTooLarge => "the frame is larger than the network adapter sends",
            Error::Unsupported(reason) => reason,
        }
    }
}

/// Whether `mac` is a usable unicast address.
pub fn is_valid_mac(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac != [0xff; 6] && mac[0] & 1 == 0
}

pub(crate) fn buffer_address<M: DmaMemory>(memory: &M, area: usize, index: usize) -> u64 {
    memory.dma_addr().get() + (area + index * BUFFER_BYTES) as u64
}

pub(crate) fn take_frame<M: DmaMemory>(
    memory: &M,
    index: usize,
    length: usize,
    frame: &mut [u8],
) -> Option<usize> {
    let length = length.min(frame.len()).min(BUFFER_BYTES);
    copy_out(
        memory,
        RECEIVE_BUFFERS + index * BUFFER_BYTES,
        &mut frame[..length],
    );
    Some(length)
}

pub(crate) fn put_frame<M: DmaMemory>(memory: &M, index: usize, frame: &[u8]) -> Result<(), Error> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge);
    }
    copy_in(memory, TRANSMIT_BUFFERS + index * BUFFER_BYTES, frame);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_multicast_and_zero_addresses_are_not_hardware_addresses() {
        assert!(is_valid_mac([0x52, 0x54, 0, 0x12, 0x34, 0x56]));
        assert!(!is_valid_mac([0; 6]));
        assert!(!is_valid_mac([0xff; 6]));
        assert!(!is_valid_mac([0x01, 0, 0x5e, 0, 0, 1]));
    }

    #[test]
    fn the_rings_and_buffers_fit_their_memory() {
        assert!(RING_ENTRIES * 16 <= TRANSMIT_RING - RECEIVE_RING);
        assert_eq!(MEMORY_BYTES, 0x2000 + 2 * RING_ENTRIES * BUFFER_BYTES);
    }
}

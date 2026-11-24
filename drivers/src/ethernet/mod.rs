//! Wired Ethernet adapters on PCI (D240, D615): Intel's e1000 and e1000e (82540 to I219), igb
//! (82575 to I211) and igc (I225 and I226), Intel's ixgbe (82599, X540, X550), Realtek's
//! RTL8111/8168 and RTL8125, and Realtek's RTL8139. Each keeps its rings and 2 KiB buffers in
//! one block of DMA memory, and is polled. The Intel adapters take jumbo frames (D708) by
//! chaining 2 KiB buffers, a frame spanning several descriptors up to its end-of-packet one, so
//! no adapter needs more DMA memory for them; every adapter wakes the machine on a magic packet
//! when asked.

pub mod e1000;
pub mod igc;
pub mod ixgbe;
pub mod realtek;
pub mod rtl8139;

use crate::dma::DmaMemory;
use crate::storage::{copy_in, copy_out};

/// Descriptors per ring.
pub const RING_ENTRIES: usize = 32;
/// Bytes of each frame buffer.
pub const BUFFER_BYTES: usize = 2048;
/// The largest frame the adapters send or take, without its CRC.
pub const MAX_FRAME_BYTES: usize = 1518;
/// The standard Ethernet MTU.
pub const STANDARD_MTU: usize = 1500;
/// The largest MTU with jumbo frames.
pub const MAX_JUMBO_MTU: usize = 9000;
/// The bytes a frame adds to its MTU: the Ethernet header and a VLAN tag.
pub const FRAME_OVERHEAD: usize = MAX_FRAME_BYTES - STANDARD_MTU;

/// The largest frame, without its CRC, an MTU of `mtu` needs.
pub const fn frame_bytes_for(mtu: usize) -> usize {
    mtu + FRAME_OVERHEAD
}

/// How many 2 KiB buffers a frame of `length` bytes takes.
pub const fn buffers_for(length: usize) -> usize {
    if length == 0 { 1 } else { length.div_ceil(BUFFER_BYTES) }
}

/// One received descriptor as a driver reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReceivedPiece {
    /// The adapter is done with it.
    pub done: bool,
    /// It holds the frame's last bytes.
    pub end: bool,
    /// The bytes it holds.
    pub length: usize,
    /// The adapter reported no error for the frame (read on its last descriptor).
    pub good: bool,
}

/// What [`gather`] found from the next descriptor on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gathered {
    /// The next frame is not all there yet.
    Waiting,
    /// A frame over `descriptors` descriptors: its length in `frame`, or `None` when it was
    /// dropped (an error, or longer than `frame`).
    Frame {
        descriptors: usize,
        length: Option<usize>,
    },
}

/// Gathers the frame starting at receive descriptor `next` into `frame`, `piece` reading
/// descriptor `index`; nothing is copied until its end-of-packet descriptor is done, so a frame
/// still arriving is left in the ring. A frame longer than the ring is dropped whole.
pub(crate) fn gather<M: DmaMemory>(
    memory: &M,
    next: usize,
    frame: &mut [u8],
    piece: impl Fn(usize) -> ReceivedPiece,
) -> Gathered {
    let mut descriptors = 0;
    for step in 0..RING_ENTRIES {
        let read = piece((next + step) % RING_ENTRIES);
        if !read.done {
            return Gathered::Waiting;
        }
        if read.end {
            descriptors = step + 1;
            break;
        }
    }
    if descriptors == 0 {
        return Gathered::Frame {
            descriptors: RING_ENTRIES,
            length: None,
        };
    }
    let mut length = 0;
    let mut fits = true;
    for step in 0..descriptors {
        let index = (next + step) % RING_ENTRIES;
        let read = piece(index);
        let bytes = read.length.min(BUFFER_BYTES);
        if step + 1 == descriptors && !read.good {
            fits = false;
        }
        if !fits || length + bytes > frame.len() {
            fits = false;
            continue;
        }
        copy_out(
            memory,
            RECEIVE_BUFFERS + index * BUFFER_BYTES,
            &mut frame[length..length + bytes],
        );
        length += bytes;
    }
    Gathered::Frame {
        descriptors,
        length: fits.then_some(length),
    }
}

/// Copies piece `piece` (from 0) of `frame` into transmit buffer `index`: its length.
pub(crate) fn put_piece<M: DmaMemory>(memory: &M, index: usize, frame: &[u8], piece: usize) -> usize {
    let start = piece * BUFFER_BYTES;
    let end = (start + BUFFER_BYTES).min(frame.len());
    copy_in(
        memory,
        TRANSMIT_BUFFERS + index * BUFFER_BYTES,
        &frame[start..end],
    );
    end - start
}

/// Whether to wake the machine from sleep or soft-off, and on what (D708).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WakeOnLan {
    /// Never.
    #[default]
    Off,
    /// On a magic packet: the adapter's address sixteen times after six `ff` bytes.
    MagicPacket,
}

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
        const { assert!(RING_ENTRIES * 16 <= TRANSMIT_RING - RECEIVE_RING) };
        assert_eq!(MEMORY_BYTES, 0x2000 + 2 * RING_ENTRIES * BUFFER_BYTES);
    }
}

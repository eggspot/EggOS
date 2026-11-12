//! The virtio network device (virtio 1.2, 5.1), polled: one receive queue whose buffers the
//! driver keeps posted, and one transmit queue. Every buffer starts with the device's header
//! (`virtio_net_hdr`, 12 bytes with `VIRTIO_F_VERSION_1`), which, when the driver sends, asks a
//! device that offers it to complete the frame's TCP or UDP checksum (D535), or to cut a TCP
//! segment larger than the MTU into segments of a given size (D541), and is otherwise zero, and
//! which it reads when it receives (D533), then holds one Ethernet frame without its check
//! sequence. A frame cut by the device lies in several contiguous transmit buffers, handed over
//! as one. The hardware address, the link's state and the
//! MTU come from the device's configuration when it offers them.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;

#[cfg(test)]
mod tests;

/// Entries of each queue the driver sets up, at most.
pub const QUEUE_SIZE: u16 = 16;
/// Bytes of the header before each frame.
pub const HEADER_BYTES: usize = 12;
/// Bytes of each buffer: the header and the largest frame (1514 bytes), rounded up.
pub const BUFFER_BYTES: usize = 2048;
/// The longest frame the driver sends or receives whole.
pub const MAX_FRAME_BYTES: usize = BUFFER_BYTES - HEADER_BYTES;
/// Where the transmit queue starts in the driver's DMA memory; the receive queue is at its
/// start.
pub const TRANSMIT_QUEUE_OFFSET: usize = 1024;
/// Where the receive buffers start.
pub const RECEIVE_BUFFERS_OFFSET: usize = 4096;
/// Where the transmit buffers start.
pub const TRANSMIT_BUFFERS_OFFSET: usize =
    RECEIVE_BUFFERS_OFFSET + QUEUE_SIZE as usize * BUFFER_BYTES;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = TRANSMIT_BUFFERS_OFFSET + QUEUE_SIZE as usize * BUFFER_BYTES;
/// The MTU of a device that does not say its own.
pub const DEFAULT_MTU: u16 = 1500;

const RECEIVE_QUEUE: u16 = 0;
const HEADER_FLAGS: usize = 0;
const HEADER_CHECKSUM_START: usize = 6;
const HEADER_CHECKSUM_OFFSET: usize = 8;
const NEEDS_CHECKSUM: u8 = 1;
const HEADER_SEGMENTATION_TYPE: usize = 1;
const HEADER_HEADER_LENGTH: usize = 2;
const HEADER_SEGMENT_SIZE: usize = 4;
const SEGMENTATION_TCPV4: u8 = 1;
const TCP_DATA_OFFSET: usize = 12;
/// The most transmit buffers one segmented frame takes.
pub const SEGMENTED_FRAME_BUFFERS: usize = 8;
/// The longest frame the driver hands the device to cut into segments (D541).
pub const MAX_SEGMENTED_FRAME_BYTES: usize = SEGMENTED_FRAME_BUFFERS * BUFFER_BYTES - HEADER_BYTES;
const ETHERNET_HEADER_BYTES: usize = 14;
const IPV4_ETHERTYPE: [u8; 2] = [0x08, 0x00];
const IPV4_VERSION: u8 = 4;
const IPV4_FRAGMENT_FIELD: usize = 6;
const IPV4_MORE_FRAGMENTS_AND_OFFSET: u16 = 0x3fff;
const IPV4_PROTOCOL_FIELD: usize = 9;
const TCP_PROTOCOL: u8 = 6;
const UDP_PROTOCOL: u8 = 17;
const TCP_CHECKSUM_OFFSET: u16 = 16;
const UDP_CHECKSUM_OFFSET: u16 = 6;

/// Where the device completes the checksum of `frame` (D535): the start of an unfragmented
/// IPv4 packet's TCP or UDP header in the frame and the checksum's offset in that header;
/// `None` for any other frame, or when the field lies past its end.
pub fn checksum_place(frame: &[u8]) -> Option<(u16, u16)> {
    if frame.get(12..ETHERNET_HEADER_BYTES)? != IPV4_ETHERTYPE {
        return None;
    }
    let packet = &frame[ETHERNET_HEADER_BYTES..];
    let first = *packet.first()?;
    if first >> 4 != IPV4_VERSION {
        return None;
    }
    let fragment = packet.get(IPV4_FRAGMENT_FIELD..IPV4_FRAGMENT_FIELD + 2)?;
    if u16::from_be_bytes([fragment[0], fragment[1]]) & IPV4_MORE_FRAGMENTS_AND_OFFSET != 0 {
        return None;
    }
    let offset = match *packet.get(IPV4_PROTOCOL_FIELD)? {
        TCP_PROTOCOL => TCP_CHECKSUM_OFFSET,
        UDP_PROTOCOL => UDP_CHECKSUM_OFFSET,
        _ => return None,
    };
    let start = ETHERNET_HEADER_BYTES + usize::from(first & 0x0f) * 4;
    (start + usize::from(offset) + 2 <= frame.len()).then_some((start as u16, offset))
}

/// The header the driver sends `frame`, a TCP segment longer than the MTU, with for the device
/// to cut it into segments of `segment_size` data bytes (D541): [`transmit_header`]'s checksum
/// request, the TCPv4 type, the length of the frame's headers and the size; `None` for a frame
/// that is not an unfragmented IPv4 TCP segment.
pub fn segmentation_header(frame: &[u8], segment_size: u16) -> Option<[u8; HEADER_BYTES]> {
    let (start, offset) = checksum_place(frame)?;
    if offset != TCP_CHECKSUM_OFFSET {
        return None;
    }
    let data_offset = *frame.get(usize::from(start) + TCP_DATA_OFFSET)?;
    let headers = start + u16::from(data_offset >> 4) * 4;
    let mut header = transmit_header(frame, true);
    header[HEADER_SEGMENTATION_TYPE] = SEGMENTATION_TCPV4;
    header[HEADER_HEADER_LENGTH..HEADER_HEADER_LENGTH + 2].copy_from_slice(&headers.to_le_bytes());
    header[HEADER_SEGMENT_SIZE..HEADER_SEGMENT_SIZE + 2]
        .copy_from_slice(&segment_size.to_le_bytes());
    Some(header)
}

/// The header the driver sends `frame` with: zero, or, when the device completes checksums,
/// [`checksum_place`]'s request for it.
pub fn transmit_header(frame: &[u8], completes_checksums: bool) -> [u8; HEADER_BYTES] {
    let mut header = [0u8; HEADER_BYTES];
    if completes_checksums && let Some((start, offset)) = checksum_place(frame) {
        header[HEADER_FLAGS] = NEEDS_CHECKSUM;
        header[HEADER_CHECKSUM_START..HEADER_CHECKSUM_START + 2]
            .copy_from_slice(&start.to_le_bytes());
        header[HEADER_CHECKSUM_OFFSET..HEADER_CHECKSUM_OFFSET + 2]
            .copy_from_slice(&offset.to_le_bytes());
    }
    header
}

/// Completes the partial checksum of `frame` (D533): the one's complement sum of the bytes from
/// `start` on, the pseudo-header sum already in the field at `start + offset`, stored there
/// inverted; nothing when the field lies outside the frame.
pub fn complete_checksum(frame: &mut [u8], start: usize, offset: usize) {
    let Some(field) = start.checked_add(offset) else {
        return;
    };
    if field + 2 > frame.len() || start >= frame.len() {
        return;
    }
    let mut sum = frame[start..].chunks(2).fold(0u32, |sum, pair| {
        let word = u32::from(pair[0]) << 8 | u32::from(pair.get(1).copied().unwrap_or(0));
        sum + word
    });
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let folded = !(sum as u16);
    frame[field..field + 2].copy_from_slice(&folded.to_be_bytes());
}
const TRANSMIT_QUEUE: u16 = 1;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= TRANSMIT_QUEUE_OFFSET);
const _: () = assert!(
    TRANSMIT_QUEUE_OFFSET + super::queue::memory_size(QUEUE_SIZE) <= RECEIVE_BUFFERS_OFFSET
);
const _: () = assert!(QUEUE_SIZE as usize <= u16::BITS as usize);

/// Feature bits of the network device (virtio 1.2, 5.1.3).
pub mod feature {
    /// The device completes the partial checksums of the frames the driver sends (D535).
    pub const CSUM: u64 = 1 << 0;
    /// The device cuts TCP segments over IPv4 larger than the MTU (D541).
    pub const HOST_TSO4: u64 = 1 << 11;
    /// The driver takes frames whose checksum the device left partial or already checked
    /// (D533).
    pub const GUEST_CSUM: u64 = 1 << 1;
    /// The device says its MTU.
    pub const MTU: u64 = 1 << 3;
    /// The device says its hardware address.
    pub const MAC: u64 = 1 << 5;
    /// The device says whether its link is up.
    pub const STATUS: u64 = 1 << 16;
}

/// The device configuration's layout (`virtio_net_config`).
pub mod config {
    /// The hardware address, six bytes.
    pub const MAC_OFFSET: usize = 0;
    /// The status, 16 bits, little-endian.
    pub const STATUS_OFFSET: usize = 6;
    /// The MTU, 16 bits, little-endian.
    pub const MTU_OFFSET: usize = 10;
    /// The status bit that says the link is up.
    pub const LINK_UP: u16 = 1;
}

/// A virtio network device, set up and polled.
#[derive(Debug)]
pub struct VirtioNet<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    receive: SplitQueue,
    transmit: SplitQueue,
    mac: [u8; 6],
    mtu: u16,
    reports_link: bool,
    completes_checksums: bool,
    segments_tcp: bool,
    receive_buffer_of_head: [u8; QUEUE_SIZE as usize],
    transmit_buffers_of_head: [u16; QUEUE_SIZE as usize],
    free_transmit_buffers: u16,
}

impl<T: Transport, M: DmaMemory> VirtioNet<T, M> {
    /// Sets the network device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more)
    /// for its two queues and their buffers, reads its hardware address, and posts every
    /// receive buffer, polling while `keep_waiting` says to. On an error the transport comes
    /// back, and the memory too unless the device may still use it (a reset that did not read
    /// back 0): then it is leaked.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::NETWORK {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        let wanted = feature::MAC
            | feature::STATUS
            | feature::MTU
            | feature::GUEST_CSUM
            | feature::CSUM
            | feature::HOST_TSO4;
        let accepted = match super::initialize(&transport, wanted, keep_waiting) {
            Ok(accepted) => accepted,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        if accepted & feature::MAC == 0 {
            super::fail(&transport);
            return Err((Error::NoHardwareAddress, transport, Some(memory)));
        }
        let mut mac = [0u8; 6];
        for (index, byte) in mac.iter_mut().enumerate() {
            *byte = transport.read_config8(config::MAC_OFFSET + index);
        }
        let mtu = match accepted & feature::MTU {
            0 => DEFAULT_MTU,
            _ => read_config16(&transport, config::MTU_OFFSET).clamp(68, DEFAULT_MTU),
        };
        let size = |queue: u16| match transport.max_queue_size(queue).min(QUEUE_SIZE) {
            0 => 0,
            max => 1 << max.ilog2(),
        };
        let (receive_size, transmit_size) = (size(RECEIVE_QUEUE), size(TRANSMIT_QUEUE));
        if receive_size == 0 || transmit_size == 0 {
            super::fail(&transport);
            return Err((Error::QueueUnavailable, transport, Some(memory)));
        }
        let base = memory.as_mut_ptr();
        // SAFETY: `memory` holds at least `MEMORY_BYTES`: the receive queue's
        // `memory_size(QUEUE_SIZE)` bytes at its 4096-aligned start, before
        // `TRANSMIT_QUEUE_OFFSET`, owned by this driver, which keeps it while the queue lives;
        // the device reaches it at its DMA address.
        let receive = unsafe { SplitQueue::new(base, memory.dma_addr(), receive_size) };
        let transmit_dma = memory.dma_addr().checked_add(TRANSMIT_QUEUE_OFFSET as u64);
        let transmit = match transmit_dma {
            // SAFETY: as above, for the transmit queue at `TRANSMIT_QUEUE_OFFSET` (1024-aligned),
            // whose bytes end before the receive buffers.
            Some(dma) => unsafe {
                SplitQueue::new(base.wrapping_add(TRANSMIT_QUEUE_OFFSET), dma, transmit_size)
            },
            None => Err(Error::MemoryTooSmall),
        };
        let (mut receive, mut transmit) = match (receive, transmit) {
            (Ok(receive), Ok(transmit)) => (receive, transmit),
            (Err(error), _) | (_, Err(error)) => return Err((error, transport, Some(memory))),
        };
        receive.disable_used_notifications();
        transmit.disable_used_notifications();
        let set_up = transport
            .set_up_queue(RECEIVE_QUEUE, receive_size, &receive.addresses())
            .and_then(|()| {
                transport.set_up_queue(TRANSMIT_QUEUE, transmit_size, &transmit.addresses())
            });
        if let Err(error) = set_up {
            super::fail(&transport);
            let memory = match super::reset(&transport, keep_waiting) {
                Ok(_) => Some(memory),
                Err(_) => {
                    core::mem::forget(memory);
                    None
                }
            };
            return Err((error, transport, memory));
        }
        let mut net = Self {
            transport,
            memory,
            receive,
            transmit,
            mac,
            mtu,
            reports_link: accepted & feature::STATUS != 0,
            completes_checksums: accepted & feature::CSUM != 0,
            segments_tcp: accepted & feature::CSUM != 0 && accepted & feature::HOST_TSO4 != 0,
            receive_buffer_of_head: [0; QUEUE_SIZE as usize],
            transmit_buffers_of_head: [0; QUEUE_SIZE as usize],
            free_transmit_buffers: ((1u32 << transmit_size) - 1) as u16,
        };
        if let Err(error) = (0..receive_size as u8).try_for_each(|buffer| net.post_receive(buffer))
        {
            super::fail(&net.transport);
            return match net.reset(keep_waiting) {
                Ok((transport, memory, _)) => Err((error, transport, Some(memory))),
                Err((_, transport)) => Err((error, transport, None)),
            };
        }
        super::driver_ok(&net.transport);
        if net.receive.should_notify() {
            net.transport.notify(RECEIVE_QUEUE);
        }
        Ok(net)
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Has the device interrupt when it receives frames, for a driver woken by its interrupt
    /// instead of polling (D220); frames received before are found by the next look.
    /// Transmitted frames are taken back when the next one is sent, without an interrupt.
    pub fn enable_interrupts(&mut self) {
        self.receive.enable_used_notifications();
    }

    /// The device's hardware address.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// The largest IP packet the link carries: the device's MTU, or [`DEFAULT_MTU`].
    pub fn mtu(&self) -> u16 {
        self.mtu
    }

    /// Whether the device completes the TCP and UDP checksums of the frames it sends, which then
    /// carry only their pseudo-header's sum (D535).
    pub fn completes_checksums(&self) -> bool {
        self.completes_checksums
    }

    /// The longest frame the device cuts into TCP segments ([`Self::transmit_segmented`],
    /// D541); `None` when it does not.
    pub fn segmentation_limit(&self) -> Option<usize> {
        self.segments_tcp.then_some(MAX_SEGMENTED_FRAME_BYTES)
    }

    /// Whether the link is up; a device that does not report its link is always up.
    pub fn is_link_up(&self) -> bool {
        !self.reports_link
            || read_config16(&self.transport, config::STATUS_OFFSET) & config::LINK_UP != 0
    }

    fn buffer_addr(&self, offset: usize) -> Result<crate::dma::DmaAddr, Error> {
        self.memory
            .dma_addr()
            .checked_add(offset as u64)
            .ok_or(Error::MemoryTooSmall)
    }

    fn post_receive(&mut self, buffer: u8) -> Result<(), Error> {
        let addr = self.buffer_addr(RECEIVE_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES)?;
        let head = self.receive.submit(&[Buffer {
            addr,
            len: BUFFER_BYTES as u32,
            device_writes: true,
        }])?;
        if let Some(slot) = self.receive_buffer_of_head.get_mut(usize::from(head)) {
            *slot = buffer;
        }
        Ok(())
    }

    /// Copies the next frame the device received into the start of `frame` and posts its
    /// buffer again: its length (cut to `frame`'s), or `None` when there is none. With nothing
    /// new, it costs one load of the used ring's index.
    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        loop {
            let used = self.receive.pop_used()?;
            if used.head >= self.receive.size() {
                continue;
            }
            let buffer = self.receive_buffer_of_head[usize::from(used.head)];
            let length = (used.len as usize)
                .min(BUFFER_BYTES)
                .saturating_sub(HEADER_BYTES)
                .min(frame.len());
            let header_at = RECEIVE_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES;
            let start = header_at + HEADER_BYTES;
            let mut header = [0u8; HEADER_BYTES];
            // SAFETY: the header lies at the start of the receive buffer `buffer`, which the
            // device handed back with this used element and does not touch until it is posted
            // again below; `header` is this function's own memory.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.memory.as_mut_ptr().wrapping_add(header_at),
                    header.as_mut_ptr(),
                    HEADER_BYTES,
                )
            };
            // SAFETY: the frame lies inside the receive buffer `buffer` of the driver's DMA
            // memory, which the device wrote and handed back with this used element (its index
            // read with acquire ordering) and does not touch until it is posted again below;
            // `frame` is the caller's own memory, apart from it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.memory.as_mut_ptr().wrapping_add(start),
                    frame.as_mut_ptr(),
                    length,
                )
            };
            if header[HEADER_FLAGS] & NEEDS_CHECKSUM != 0 {
                let field =
                    |at: usize| usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
                let (checksum_start, checksum_offset) =
                    (field(HEADER_CHECKSUM_START), field(HEADER_CHECKSUM_OFFSET));
                complete_checksum(&mut frame[..length], checksum_start, checksum_offset);
            }
            let posted = self.post_receive(buffer);
            if self.receive.should_notify() {
                self.transport.notify(RECEIVE_QUEUE);
            }
            if used.len as usize >= HEADER_BYTES && posted.is_ok() {
                return Some(length);
            }
        }
    }

    fn reclaim_transmitted(&mut self) {
        while let Some(used) = self.transmit.pop_used() {
            if let Some(&buffers) = self.transmit_buffers_of_head.get(usize::from(used.head)) {
                self.free_transmit_buffers |= buffers;
            }
        }
    }

    /// Hands `frame` to the device to send: [`Error::FrameTooLong`] beyond
    /// [`MAX_FRAME_BYTES`], [`Error::QueueFull`] while every transmit buffer is with the device.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLong);
        }
        let header = transmit_header(frame, self.completes_checksums);
        self.hand_over(frame, &header)
    }

    /// Hands `frame`, a TCP segment longer than the MTU, to the device to cut into segments of
    /// `segment_size` data bytes and send (D541): [`Error::FrameTooLong`] when the device does
    /// not cut segments, the frame is not one it cuts, or it is longer than
    /// [`MAX_SEGMENTED_FRAME_BYTES`].
    pub fn transmit_segmented(&mut self, frame: &[u8], segment_size: u16) -> Result<(), Error> {
        if !self.segments_tcp || frame.len() > MAX_SEGMENTED_FRAME_BYTES {
            return Err(Error::FrameTooLong);
        }
        let header = segmentation_header(frame, segment_size).ok_or(Error::FrameTooLong)?;
        self.hand_over(frame, &header)
    }

    /// Copies `header` and `frame` into enough contiguous free transmit buffers and hands them
    /// to the device as one.
    fn hand_over(&mut self, frame: &[u8], header: &[u8; HEADER_BYTES]) -> Result<(), Error> {
        self.reclaim_transmitted();
        let count = (HEADER_BYTES + frame.len()).div_ceil(BUFFER_BYTES).max(1);
        let run = ((1u32 << count) - 1) as u16;
        let first = (0..=QUEUE_SIZE as usize - count)
            .find(|&first| (self.free_transmit_buffers >> first) & run == run)
            .ok_or(Error::QueueFull)?;
        let buffers = run << first;
        let offset = TRANSMIT_BUFFERS_OFFSET + first * BUFFER_BYTES;
        let start = self.memory.as_mut_ptr().wrapping_add(offset);
        // SAFETY: the transmit buffers `first..first + count` of the driver's DMA memory,
        // `BUFFER_BYTES` long each and contiguous, are free: the device handed them back (or
        // never had them), so only the driver touches them; the header fits at their start,
        // and `header` is the caller's own.
        unsafe { core::ptr::copy_nonoverlapping(header.as_ptr(), start, HEADER_BYTES) };
        // SAFETY: the same free buffers, which hold the frame after the header (`count` covers
        // both); `frame` is the caller's own memory, apart from them.
        unsafe {
            core::ptr::copy_nonoverlapping(
                frame.as_ptr(),
                start.wrapping_add(HEADER_BYTES),
                frame.len(),
            )
        };
        let head = self.transmit.submit(&[Buffer {
            addr: self.buffer_addr(offset)?,
            len: (HEADER_BYTES + frame.len()) as u32,
            device_writes: false,
        }])?;
        self.free_transmit_buffers &= !buffers;
        if let Some(slot) = self.transmit_buffers_of_head.get_mut(usize::from(head)) {
            *slot = buffers;
        }
        if self.transmit.should_notify() {
            self.transport.notify(TRANSMIT_QUEUE);
        }
        Ok(())
    }

    /// Resets the device and hands back the transport and the memory, with the proof that the
    /// device no longer uses it. If the reset does not read back 0, the memory is leaked and only
    /// the transport comes back.
    pub fn reset(
        self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(T, M, ResetDone), (Error, T)> {
        let Self {
            transport, memory, ..
        } = self;
        match super::reset(&transport, keep_waiting) {
            Ok(done) => Ok((transport, memory, done)),
            Err(error) => {
                core::mem::forget(memory);
                Err((error, transport))
            }
        }
    }
}

fn read_config16<T: Transport + ?Sized>(transport: &T, offset: usize) -> u16 {
    u16::from_le_bytes([
        transport.read_config8(offset),
        transport.read_config8(offset + 1),
    ])
}

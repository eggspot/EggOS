//! The virtio network device (virtio 1.2, 5.1), polled: one receive queue whose buffers the
//! driver keeps posted, and one transmit queue. Every buffer starts with the device's header
//! (`virtio_net_hdr`, 12 bytes with `VIRTIO_F_VERSION_1`), which, when the driver sends, asks a
//! device that offers it to complete the frame's TCP or UDP checksum (D535), or to cut a TCP
//! segment larger than the MTU into segments of a given size (D541), and is otherwise zero, and
//! which it reads when it receives (D533), then holds one Ethernet frame without its check
//! sequence. A frame cut by the device lies in several contiguous transmit buffers, handed over
//! as one. The hardware address, the link's state and the
//! MTU come from the device's configuration when it offers them.

use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;
use alloc::vec::Vec;
use lane::Lane;

mod lane;
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

const IPV6_ETHERTYPE: [u8; 2] = [0x86, 0xdd];
const IPV6_HEADER_BYTES: usize = 40;
const IPV6_NEXT_HEADER_FIELD: usize = 6;
const SEGMENTATION_TCPV6: u8 = 4;
const SEGMENTATION_UDP: u8 = 5;
const UDP_HEADER_BYTES: u16 = 8;
const HEADER_BUFFERS: usize = 10;

/// Where the device completes the checksum of `frame` (D535): the start of an unfragmented
/// IPv4 packet's, or an IPv6 packet's with no extension header (D566), TCP or UDP header in the
/// frame and the checksum's offset in that header; `None` for any other frame, or when the
/// field lies past its end.
pub fn checksum_place(frame: &[u8]) -> Option<(u16, u16)> {
    if frame.get(12..ETHERNET_HEADER_BYTES)? == IPV6_ETHERTYPE {
        let packet = &frame[ETHERNET_HEADER_BYTES..];
        let offset = match *packet.get(IPV6_NEXT_HEADER_FIELD)? {
            TCP_PROTOCOL => TCP_CHECKSUM_OFFSET,
            UDP_PROTOCOL => UDP_CHECKSUM_OFFSET,
            _ => return None,
        };
        let start = ETHERNET_HEADER_BYTES + IPV6_HEADER_BYTES;
        return (start + usize::from(offset) + 2 <= frame.len()).then_some((start as u16, offset));
    }
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

/// The header the driver sends `frame`, a TCP segment or UDP datagram longer than the MTU,
/// with for the device to cut it into segments of `segment_size` data bytes (D541, D566):
/// [`transmit_header`]'s checksum request, the segmentation type (TCPv4, TCPv6 or UDP), the
/// length of the frame's headers and the size; `None` for a frame that is none of those.
pub fn segmentation_header(frame: &[u8], segment_size: u16) -> Option<[u8; HEADER_BYTES]> {
    let (start, offset) = checksum_place(frame)?;
    let ipv6 = frame.get(12..ETHERNET_HEADER_BYTES)? == IPV6_ETHERTYPE;
    let (kind, headers) = match offset {
        TCP_CHECKSUM_OFFSET => {
            let data_offset = *frame.get(usize::from(start) + TCP_DATA_OFFSET)?;
            let kind = if ipv6 { SEGMENTATION_TCPV6 } else { SEGMENTATION_TCPV4 };
            (kind, start + u16::from(data_offset >> 4) * 4)
        }
        _ => (SEGMENTATION_UDP, start + UDP_HEADER_BYTES),
    };
    let mut header = transmit_header(frame, true);
    header[HEADER_SEGMENTATION_TYPE] = kind;
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
/// Where the control queue lies in the first pair's memory, after the transmit queue (D566).
pub const CONTROL_QUEUE_OFFSET: usize = 2048;
/// Where a control command and its answer lie in the first pair's memory.
pub const CONTROL_COMMAND_OFFSET: usize = 3072;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= TRANSMIT_QUEUE_OFFSET);
const _: () = assert!(
    TRANSMIT_QUEUE_OFFSET + super::queue::memory_size(QUEUE_SIZE) <= RECEIVE_BUFFERS_OFFSET
);
const _: () = assert!(QUEUE_SIZE as usize <= u16::BITS as usize);
const _: () = assert!(
    TRANSMIT_QUEUE_OFFSET + super::queue::memory_size(QUEUE_SIZE) <= CONTROL_QUEUE_OFFSET
);
const _: () = assert!(
    CONTROL_QUEUE_OFFSET + super::queue::memory_size(lane::CONTROL_QUEUE_SIZE)
        <= CONTROL_COMMAND_OFFSET
);

/// Feature bits of the network device (virtio 1.2, 5.1.3).
pub mod feature {
    /// The device completes the partial checksums of the frames the driver sends (D535).
    pub const CSUM: u64 = 1 << 0;
    /// The device cuts TCP segments over IPv4 larger than the MTU (D541).
    pub const HOST_TSO4: u64 = 1 << 11;
    /// The device cuts TCP segments over IPv6 larger than the MTU (D566).
    pub const HOST_TSO6: u64 = 1 << 12;
    /// The device cuts UDP datagrams larger than the MTU (D566).
    pub const HOST_USO: u64 = 1 << 56;
    /// The driver takes TCP segments over IPv4 the device joined (D566).
    pub const GUEST_TSO4: u64 = 1 << 7;
    /// The driver takes TCP segments over IPv6 the device joined (D566).
    pub const GUEST_TSO6: u64 = 1 << 8;
    /// The device spreads a frame over several receive buffers (D566).
    pub const MRG_RXBUF: u64 = 1 << 15;
    /// The driver takes frames whose checksum the device left partial or already checked
    /// (D533).
    pub const GUEST_CSUM: u64 = 1 << 1;
    /// The device says its MTU.
    pub const MTU: u64 = 1 << 3;
    /// The device says its hardware address.
    pub const MAC: u64 = 1 << 5;
    /// The device says whether its link is up.
    pub const STATUS: u64 = 1 << 16;
    /// The device has a control queue (D566).
    pub const CTRL_VQ: u64 = 1 << 17;
    /// The device has several queue pairs, steered by the control queue (D566).
    pub const MQ: u64 = 1 << 22;
}

/// The device configuration's layout (`virtio_net_config`).
pub mod config {
    /// The hardware address, six bytes.
    pub const MAC_OFFSET: usize = 0;
    /// The status, 16 bits, little-endian.
    pub const STATUS_OFFSET: usize = 6;
    /// How many queue pairs the device has at most, 16 bits, little-endian (D566).
    pub const MAX_PAIRS_OFFSET: usize = 8;
    /// The MTU, 16 bits, little-endian.
    pub const MTU_OFFSET: usize = 10;
    /// The status bit that says the link is up.
    pub const LINK_UP: u16 = 1;
}

/// A virtio network device, set up and polled.
#[derive(Debug)]
pub struct VirtioNet<T: Transport, M: DmaMemory> {
    transport: T,
    lanes: Vec<Lane<M>>,
    mac: [u8; 6],
    mtu: u16,
    reports_link: bool,
    completes_checksums: bool,
    segments_tcp: bool,
    segments_tcp6: bool,
    segments_udp: bool,
    choose_lane: fn() -> usize,
    next_receive_lane: usize,
}

fn first_lane() -> usize {
    0
}

impl<T: Transport, M: DmaMemory> VirtioNet<T, M> {
    /// Sets the network device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more)
    /// for its first queue pair and its buffers, and with one more pair for each block of
    /// `more` the device takes (`VIRTIO_NET_F_MQ`, D566: the blocks it does not take are
    /// leaked with it), reads its hardware address, and posts every receive buffer, polling
    /// while `keep_waiting` says to. On an error the transport comes back, and the first
    /// memory too unless the device may still use it (a reset that did not read back 0):
    /// then it is leaked, as the further blocks always are.
    pub fn new(
        transport: T,
        memory: M,
        more: Vec<M>,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::NETWORK {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        let mut wanted = feature::MAC
            | feature::STATUS
            | feature::MTU
            | feature::GUEST_CSUM
            | feature::CSUM
            | feature::HOST_TSO4
            | feature::HOST_TSO6
            | feature::HOST_USO
            | feature::GUEST_TSO4
            | feature::GUEST_TSO6
            | feature::MRG_RXBUF
            | super::feature::RING_EVENT_IDX;
        if !more.is_empty() {
            wanted |= feature::CTRL_VQ | feature::MQ;
        }
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
        let multiqueue = accepted & (feature::CTRL_VQ | feature::MQ) == feature::CTRL_VQ | feature::MQ;
        let offered_pairs = match multiqueue {
            true => read_config16(&transport, config::MAX_PAIRS_OFFSET).max(1),
            false => 1,
        };
        let event_index = accepted & super::feature::RING_EVENT_IDX != 0;
        let joins = accepted & feature::MRG_RXBUF != 0
            && accepted & feature::GUEST_CSUM != 0
            && accepted & (feature::GUEST_TSO4 | feature::GUEST_TSO6) != 0;
        if !joins && accepted & (feature::GUEST_TSO4 | feature::GUEST_TSO6) != 0 {
            super::fail(&transport);
            return Err((Error::FeaturesRefused, transport, Some(memory)));
        }
        let mergeable = accepted & feature::MRG_RXBUF != 0;
        let first = match Lane::new(&transport, memory, 0, event_index, mergeable) {
            Ok(lane) => lane,
            Err((error, memory)) => {
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
        };
        let mut lanes = Vec::with_capacity(1 + more.len());
        lanes.push(first);
        for memory in more {
            let pair = lanes.len() as u16;
            if pair >= offered_pairs {
                core::mem::forget(memory);
                continue;
            }
            match Lane::new(&transport, memory, pair, event_index, mergeable) {
                Ok(lane) => lanes.push(lane),
                Err((_, memory)) => core::mem::forget(memory),
            }
        }
        let control = match multiqueue && lanes.len() > 1 {
            true => lanes[0].control_queue(&transport, offered_pairs),
            false => None,
        };
        let mut net = Self {
            transport,
            lanes,
            mac,
            mtu,
            reports_link: accepted & feature::STATUS != 0,
            completes_checksums: accepted & feature::CSUM != 0,
            segments_tcp: accepted & feature::CSUM != 0 && accepted & feature::HOST_TSO4 != 0,
            segments_tcp6: accepted & feature::CSUM != 0 && accepted & feature::HOST_TSO6 != 0,
            segments_udp: accepted & feature::CSUM != 0 && accepted & feature::HOST_USO != 0,
            choose_lane: first_lane,
            next_receive_lane: 0,
        };
        super::driver_ok(&net.transport);
        let pairs = net.lanes.len() as u16;
        let enabled = match control {
            Some(mut control) => control.set_pairs(&net.transport, &net.lanes[0].memory, pairs, keep_waiting),
            None => false,
        };
        if !enabled {
            net.lanes.truncate(1);
        }
        for index in 0..net.lanes.len() {
            let Self {
                transport, lanes, ..
            } = &mut net;
            if let Err(error) = lanes[index].post_all(transport) {
                super::fail(&net.transport);
                return match net.reset(keep_waiting) {
                    Ok((transport, memory, _)) => Err((error, transport, Some(memory))),
                    Err((_, transport)) => Err((error, transport, None)),
                };
            }
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
        for lane in &mut self.lanes {
            lane.enable_interrupts();
        }
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

    /// Hands `frame` to the device to send: [`Error::FrameTooLong`] beyond
    /// [`MAX_FRAME_BYTES`], [`Error::QueueFull`] while every transmit buffer is with the device.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLong);
        }
        let header = transmit_header(frame, self.completes_checksums);
        let lane = (self.choose_lane)() % self.lanes.len();
        self.lanes[lane].hand_over(&self.transport, frame, &header)
    }

    /// Hands `frame`, a TCP segment longer than the MTU, to the device to cut into segments of
    /// `segment_size` data bytes and send (D541): [`Error::FrameTooLong`] when the device does
    /// not cut segments, the frame is not one it cuts, or it is longer than
    /// [`MAX_SEGMENTED_FRAME_BYTES`].
    pub fn transmit_segmented(&mut self, frame: &[u8], segment_size: u16) -> Result<(), Error> {
        if frame.len() > MAX_SEGMENTED_FRAME_BYTES {
            return Err(Error::FrameTooLong);
        }
        let header = segmentation_header(frame, segment_size).ok_or(Error::FrameTooLong)?;
        let cut = match header[HEADER_SEGMENTATION_TYPE] {
            SEGMENTATION_TCPV4 => self.segments_tcp,
            SEGMENTATION_TCPV6 => self.segments_tcp6,
            _ => self.segments_udp,
        };
        if !cut {
            return Err(Error::FrameTooLong);
        }
        let lane = (self.choose_lane)() % self.lanes.len();
        self.lanes[lane].hand_over(&self.transport, frame, &header)
    }

    /// Copies the next frame any of its queue pairs received into the start of `frame` and
    /// posts its buffer again: its length, or `None` when there is none.
    pub fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        let count = self.lanes.len();
        for step in 0..count {
            let lane = (self.next_receive_lane + step) % count;
            if let Some(length) = self.lanes[lane].receive(&self.transport, frame) {
                self.next_receive_lane = (lane + 1) % count;
                return Some(length);
            }
        }
        None
    }

    /// How many queue pairs it drives (D566): one per CPU it was given memory for, up to what
    /// the device offers.
    pub fn queue_pairs(&self) -> usize {
        self.lanes.len()
    }

    /// Picks the queue pair a send goes through: `choose` (the current CPU's number) modulo
    /// the pairs, so each CPU sends on its own (D566).
    pub fn set_lane_chooser(&mut self, choose: fn() -> usize) {
        self.choose_lane = choose;
    }

    /// Resets the device and hands back the transport and the memory, with the proof that the
    /// device no longer uses it. If the reset does not read back 0, the memory is leaked and only
    /// the transport comes back.
    pub fn reset(
        self,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(T, M, ResetDone), (Error, T)> {
        let Self {
            transport, lanes, ..
        } = self;
        let mut memories = lanes.into_iter().map(|lane| lane.memory);
        let first = memories.next();
        let reset = super::reset(&transport, keep_waiting);
        for memory in memories {
            core::mem::forget(memory);
        }
        match (reset, first) {
            (Ok(done), Some(memory)) => Ok((transport, memory, done)),
            (Ok(_), None) => Err((Error::QueueUnavailable, transport)),
            (Err(error), memory) => {
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

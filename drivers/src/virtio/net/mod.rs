//! The virtio network device (virtio 1.2, 5.1), polled: one receive queue whose buffers the
//! driver keeps posted, and one transmit queue. Every buffer starts with the device's header
//! (`virtio_net_hdr`, 12 bytes with `VIRTIO_F_VERSION_1`), which the driver leaves zero when
//! it sends (no checksum offload, no segmentation) and skips when it receives, then holds one
//! Ethernet frame without its check sequence. The hardware address, the link's state and the
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
const TRANSMIT_QUEUE: u16 = 1;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= TRANSMIT_QUEUE_OFFSET);
const _: () = assert!(
    TRANSMIT_QUEUE_OFFSET + super::queue::memory_size(QUEUE_SIZE) <= RECEIVE_BUFFERS_OFFSET
);
const _: () = assert!(QUEUE_SIZE as usize <= u16::BITS as usize);

/// Feature bits of the network device (virtio 1.2, 5.1.3).
pub mod feature {
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
    receive_buffer_of_head: [u8; QUEUE_SIZE as usize],
    transmit_buffer_of_head: [u8; QUEUE_SIZE as usize],
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
        let wanted = feature::MAC | feature::STATUS | feature::MTU;
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
            receive_buffer_of_head: [0; QUEUE_SIZE as usize],
            transmit_buffer_of_head: [0; QUEUE_SIZE as usize],
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

    /// The device's hardware address.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// The largest IP packet the link carries: the device's MTU, or [`DEFAULT_MTU`].
    pub fn mtu(&self) -> u16 {
        self.mtu
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
            let start = RECEIVE_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES + HEADER_BYTES;
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
            if let Some(&buffer) = self.transmit_buffer_of_head.get(usize::from(used.head)) {
                self.free_transmit_buffers |= 1 << buffer;
            }
        }
    }

    /// Hands `frame` to the device to send: [`Error::FrameTooLong`] beyond
    /// [`MAX_FRAME_BYTES`], [`Error::QueueFull`] while every transmit buffer is with the device.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), Error> {
        if frame.len() > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLong);
        }
        self.reclaim_transmitted();
        if self.free_transmit_buffers == 0 {
            return Err(Error::QueueFull);
        }
        let buffer = self.free_transmit_buffers.trailing_zeros() as u8;
        let offset = TRANSMIT_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES;
        let start = self.memory.as_mut_ptr().wrapping_add(offset);
        // SAFETY: the transmit buffer `buffer` of the driver's DMA memory, `BUFFER_BYTES`
        // long, is free: the device handed it back (or never had it), so only the driver
        // touches it; the header fits at its start.
        unsafe { start.write_bytes(0, HEADER_BYTES) };
        // SAFETY: the same free buffer, which holds the frame after the header
        // (`MAX_FRAME_BYTES` checked above); `frame` is the caller's own memory, apart from it.
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
        self.free_transmit_buffers &= !(1 << buffer);
        if let Some(slot) = self.transmit_buffer_of_head.get_mut(usize::from(head)) {
            *slot = buffer;
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

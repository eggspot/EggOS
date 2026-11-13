//! One queue pair of the virtio network device (D566): a receive queue whose buffers the
//! driver keeps posted and a transmit queue, with their buffers, in one block of DMA memory
//! laid out as [`super::MEMORY_BYTES`] says. The device has one pair, or with
//! `VIRTIO_NET_F_MQ` several, each CPU sending on its own.

use super::super::queue::{Buffer, SplitQueue};
use super::super::{Error, Transport};
use super::{
    BUFFER_BYTES, CONTROL_COMMAND_OFFSET, CONTROL_QUEUE_OFFSET, HEADER_BUFFERS, HEADER_BYTES,
    HEADER_CHECKSUM_OFFSET, HEADER_CHECKSUM_START, HEADER_FLAGS, NEEDS_CHECKSUM, QUEUE_SIZE,
    RECEIVE_BUFFERS_OFFSET, TRANSMIT_BUFFERS_OFFSET, TRANSMIT_QUEUE_OFFSET, complete_checksum,
};
use crate::dma::DmaMemory;

/// Entries of the control queue, at most.
pub const CONTROL_QUEUE_SIZE: u16 = 8;
const CONTROL_MULTIQUEUE: u8 = 4;
const CONTROL_SET_PAIRS: u8 = 0;
const CONTROL_ANSWER_AT: usize = CONTROL_COMMAND_OFFSET + 8;
const CONTROL_OK: u8 = 0;

/// The control queue (D566), in the first pair's memory.
#[derive(Debug)]
pub struct Control {
    queue: SplitQueue,
    index: u16,
}

impl Control {
    /// Asks the device to use `pairs` queue pairs (`VIRTIO_NET_CTRL_MQ_VQ_PAIRS_SET`):
    /// whether it said yes in time.
    pub fn set_pairs<T: Transport + ?Sized, M: DmaMemory>(
        &mut self,
        transport: &T,
        memory: &M,
        pairs: u16,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> bool {
        let command = [CONTROL_MULTIQUEUE, CONTROL_SET_PAIRS, pairs as u8, (pairs >> 8) as u8];
        let base = memory.as_mut_ptr();
        for (index, &byte) in command.iter().enumerate() {
            // SAFETY: the command bytes at `CONTROL_COMMAND_OFFSET` of the first pair's memory,
            // which only the control queue uses and the device does not hold now.
            unsafe { base.wrapping_add(CONTROL_COMMAND_OFFSET + index).write_volatile(byte) };
        }
        // SAFETY: the answer byte, as above.
        unsafe { base.wrapping_add(CONTROL_ANSWER_AT).write_volatile(u8::MAX) };
        memory.prepare_for_device(CONTROL_COMMAND_OFFSET, command.len());
        memory.prepare_for_cpu(CONTROL_ANSWER_AT, 1);
        let at = |offset: usize| memory.dma_addr().checked_add(offset as u64);
        let (Some(command_at), Some(answer_at)) = (at(CONTROL_COMMAND_OFFSET), at(CONTROL_ANSWER_AT))
        else {
            return false;
        };
        let chain = [
            Buffer {
                addr: command_at,
                len: command.len() as u32,
                device_writes: false,
            },
            Buffer {
                addr: answer_at,
                len: 1,
                device_writes: true,
            },
        ];
        if self.queue.submit(&chain).is_err() {
            return false;
        }
        if self.queue.should_notify() {
            transport.notify(self.index);
        }
        while self.queue.pop_used().is_none() {
            if !keep_waiting() {
                return false;
            }
        }
        memory.prepare_for_cpu(CONTROL_ANSWER_AT, 1);
        // SAFETY: the answer byte, which the device wrote and handed back.
        unsafe { base.wrapping_add(CONTROL_ANSWER_AT).read_volatile() == CONTROL_OK }
    }
}

/// One queue pair and its memory.
#[derive(Debug)]
pub struct Lane<M: DmaMemory> {
    pub(super) memory: M,
    pub(super) receive: SplitQueue,
    pub(super) transmit: SplitQueue,
    receive_queue: u16,
    transmit_queue: u16,
    receive_buffer_of_head: [u8; QUEUE_SIZE as usize],
    transmit_buffers_of_head: [u16; QUEUE_SIZE as usize],
    free_transmit_buffers: u16,
    receive_size: u16,
    mergeable: bool,
}

impl<M: DmaMemory> Lane<M> {
    /// Pair `pair` (queues `2·pair` and `2·pair + 1`) in `memory`, its queues of the sizes the
    /// device allows, told to the device, not yet posted.
    pub fn new<T: Transport + ?Sized>(
        transport: &T,
        memory: M,
        pair: u16,
        event_index: bool,
        mergeable: bool,
    ) -> Result<Self, (Error, M)> {
        if memory.len() < super::MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, memory));
        }
        let (receive_queue, transmit_queue) = (2 * pair, 2 * pair + 1);
        let size = |queue: u16| match transport.max_queue_size(queue).min(QUEUE_SIZE) {
            0 => 0,
            max => 1 << max.ilog2(),
        };
        let (receive_size, transmit_size) = (size(receive_queue), size(transmit_queue));
        if receive_size == 0 || transmit_size == 0 {
            return Err((Error::QueueUnavailable, memory));
        }
        let base = memory.as_mut_ptr();
        // SAFETY: `memory` holds at least `MEMORY_BYTES`: the receive queue's
        // `memory_size(QUEUE_SIZE)` bytes at its 4096-aligned start, before
        // `TRANSMIT_QUEUE_OFFSET`, owned by this lane, which keeps it while the queue lives;
        // the device reaches it at its DMA address.
        let receive = unsafe { SplitQueue::new(base, memory.dma_addr(), receive_size) };
        let transmit = match memory.dma_addr().checked_add(TRANSMIT_QUEUE_OFFSET as u64) {
            // SAFETY: as above, for the transmit queue at `TRANSMIT_QUEUE_OFFSET`, whose bytes
            // end before the control queue and the receive buffers.
            Some(dma) => unsafe {
                SplitQueue::new(base.wrapping_add(TRANSMIT_QUEUE_OFFSET), dma, transmit_size)
            },
            None => Err(Error::MemoryTooSmall),
        };
        let (mut receive, mut transmit) = match (receive, transmit) {
            (Ok(receive), Ok(transmit)) => (receive, transmit),
            (Err(error), _) | (_, Err(error)) => return Err((error, memory)),
        };
        receive.disable_used_notifications();
        transmit.disable_used_notifications();
        receive.set_cache_maintenance(memory.cache_maintenance());
        transmit.set_cache_maintenance(memory.cache_maintenance());
        if event_index {
            receive.use_event_index();
            transmit.use_event_index();
        }
        let set_up = transport
            .set_up_queue(receive_queue, receive_size, &receive.addresses())
            .and_then(|()| {
                transport.set_up_queue(transmit_queue, transmit_size, &transmit.addresses())
            });
        if let Err(error) = set_up {
            return Err((error, memory));
        }
        Ok(Self {
            memory,
            receive,
            transmit,
            receive_queue,
            transmit_queue,
            receive_buffer_of_head: [0; QUEUE_SIZE as usize],
            transmit_buffers_of_head: [0; QUEUE_SIZE as usize],
            free_transmit_buffers: ((1u32 << transmit_size) - 1) as u16,
            receive_size,
            mergeable,
        })
    }

    /// Posts every receive buffer and tells the device, once it is driven.
    pub fn post_all<T: Transport + ?Sized>(&mut self, transport: &T) -> Result<(), Error> {
        (0..self.receive_size as u8).try_for_each(|buffer| self.post_receive(buffer))?;
        if self.receive.should_notify() {
            transport.notify(self.receive_queue);
        }
        Ok(())
    }

    /// The control queue, in this (the first) pair's memory, as queue `2·offered_pairs`, told
    /// to the device; `None` when the device has no room for it.
    pub fn control_queue<T: Transport + ?Sized>(
        &self,
        transport: &T,
        offered_pairs: u16,
    ) -> Option<Control> {
        let index = offered_pairs.checked_mul(2)?;
        let size = match transport.max_queue_size(index).min(CONTROL_QUEUE_SIZE) {
            0 => return None,
            max => 1 << max.ilog2(),
        };
        let dma = self
            .memory
            .dma_addr()
            .checked_add(CONTROL_QUEUE_OFFSET as u64)?;
        // SAFETY: the bytes at `CONTROL_QUEUE_OFFSET` of this pair's memory hold the control
        // queue's `memory_size(CONTROL_QUEUE_SIZE)` bytes, 1024-aligned, before the command
        // bytes, and nothing else uses them while the lane, which owns the memory, lives.
        let mut queue = unsafe {
            SplitQueue::new(self.memory.as_mut_ptr().wrapping_add(CONTROL_QUEUE_OFFSET), dma, size)
        }
        .ok()?;
        queue.disable_used_notifications();
        queue.set_cache_maintenance(self.memory.cache_maintenance());
        transport.set_up_queue(index, size, &queue.addresses()).ok()?;
        Some(Control { queue, index })
    }

    /// Has the device interrupt when this pair receives.
    pub fn enable_interrupts(&mut self) {
        self.receive.enable_used_notifications();
    }

    fn buffer_addr(&self, offset: usize) -> Result<crate::dma::DmaAddr, Error> {
        self.memory
            .dma_addr()
            .checked_add(offset as u64)
            .ok_or(Error::MemoryTooSmall)
    }

    pub fn post_receive(&mut self, buffer: u8) -> Result<(), Error> {
        let offset = RECEIVE_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES;
        let addr = self.buffer_addr(offset)?;
        self.memory.prepare_for_cpu(offset, BUFFER_BYTES);
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
    pub fn receive<T: Transport + ?Sized>(
        &mut self,
        transport: &T,
        frame: &mut [u8],
    ) -> Option<usize> {
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
            self.memory
                .prepare_for_cpu(header_at, HEADER_BYTES + length);
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
            let mut length = length;
            if self.mergeable {
                let more = u16::from_le_bytes([header[HEADER_BUFFERS], header[HEADER_BUFFERS + 1]]);
                length = self.join_buffers(transport, frame, length, more.saturating_sub(1));
            }
            if header[HEADER_FLAGS] & NEEDS_CHECKSUM != 0 {
                let field =
                    |at: usize| usize::from(u16::from_le_bytes([header[at], header[at + 1]]));
                let (checksum_start, checksum_offset) =
                    (field(HEADER_CHECKSUM_START), field(HEADER_CHECKSUM_OFFSET));
                complete_checksum(&mut frame[..length], checksum_start, checksum_offset);
            }
            let posted = self.post_receive(buffer);
            if self.receive.should_notify() {
                transport.notify(self.receive_queue);
            }
            if used.len as usize >= HEADER_BYTES && posted.is_ok() {
                return Some(length);
            }
        }
    }

    fn join_buffers<T: Transport + ?Sized>(
        &mut self,
        transport: &T,
        frame: &mut [u8],
        mut length: usize,
        more: u16,
    ) -> usize {
        for _ in 0..more {
            let Some(used) = self.receive.pop_used() else {
                break;
            };
            if used.head >= self.receive.size() {
                continue;
            }
            let buffer = self.receive_buffer_of_head[usize::from(used.head)];
            let at = RECEIVE_BUFFERS_OFFSET + usize::from(buffer) * BUFFER_BYTES;
            let count = (used.len as usize)
                .min(BUFFER_BYTES)
                .min(frame.len().saturating_sub(length));
            self.memory.prepare_for_cpu(at, count);
            // SAFETY: the receive buffer `buffer`, which the device filled and handed back with
            // this used element and does not touch until it is posted again below; `frame` is
            // the caller's own memory, apart from it, with room for `count` more bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.memory.as_mut_ptr().wrapping_add(at),
                    frame.as_mut_ptr().wrapping_add(length),
                    count,
                )
            };
            length += count;
            let _ = self.post_receive(buffer);
        }
        if self.receive.should_notify() {
            transport.notify(self.receive_queue);
        }
        length
    }

    fn reclaim_transmitted(&mut self) {
        while let Some(used) = self.transmit.pop_used() {
            if let Some(&buffers) = self.transmit_buffers_of_head.get(usize::from(used.head)) {
                self.free_transmit_buffers |= buffers;
            }
        }
    }

    pub fn hand_over<T: Transport + ?Sized>(
        &mut self,
        transport: &T,
        frame: &[u8],
        header: &[u8; HEADER_BYTES],
    ) -> Result<(), Error> {
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
        self.memory
            .prepare_for_device(offset, HEADER_BYTES + frame.len());
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
            transport.notify(self.transmit_queue);
        }
        Ok(())
    }

}

//! The virtio entropy device (virtio 1.2, 5.4), polled: one request queue, each request a
//! buffer the device fills with random bytes.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::DmaMemory;

/// Entries of the request queue the driver sets up, at most.
pub const QUEUE_SIZE: u16 = 8;
/// Where the request buffer starts in the driver's DMA memory; the queue is at its start.
pub const BUFFER_OFFSET: usize = 1024;
/// The most bytes one request asks for.
pub const BUFFER_LEN: usize = 1024;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = 4096;

const REQUEST_QUEUE: u16 = 0;

/// A virtio entropy device, set up and polled.
#[derive(Debug)]
pub struct VirtioRng<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queue: SplitQueue,
    request_pending: bool,
}

impl<T: Transport, M: DmaMemory> VirtioRng<T, M> {
    /// Sets the entropy device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more)
    /// for its queue and request buffer, polling while `keep_waiting` says to. On an error the
    /// transport comes back, and the memory too unless the device may still use it (a reset
    /// that did not read back 0): then it is leaked.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::ENTROPY {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        if let Err(error) = super::initialize(&transport, 0, keep_waiting) {
            return Err((error, transport, Some(memory)));
        }
        let max = transport.max_queue_size(REQUEST_QUEUE).min(QUEUE_SIZE);
        if max == 0 {
            super::fail(&transport);
            return Err((Error::QueueUnavailable, transport, Some(memory)));
        }
        let size = 1 << max.ilog2();
        // SAFETY: `memory` holds at least `MEMORY_BYTES`, more than the queue's
        // `memory_size(QUEUE_SIZE)` before `BUFFER_OFFSET`, 4096-aligned, and owned by this
        // driver, which keeps it for as long as the queue lives; the device reaches it at its
        // DMA address.
        let queue = unsafe { SplitQueue::new(memory.as_mut_ptr(), memory.dma_addr(), size) };
        let mut queue = match queue {
            Ok(queue) => queue,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        queue.disable_used_notifications();
        if let Err(error) = transport.set_up_queue(REQUEST_QUEUE, size, &queue.addresses()) {
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
        super::driver_ok(&transport);
        Ok(Self {
            transport,
            memory,
            queue,
            request_pending: false,
        })
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Fills the start of `bytes` with random bytes from the device, at most [`BUFFER_LEN`] in
    /// one call, and returns how many, polling while `keep_waiting` says to. A request the
    /// device has not answered when polling stops stays posted ([`Error::TimedOut`]): the next
    /// call waits for it first, and its bytes are dropped.
    pub fn read(
        &mut self,
        bytes: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<usize, Error> {
        if self.request_pending {
            self.wait_for_answer(keep_waiting)?;
        }
        let len = bytes.len().min(BUFFER_LEN);
        if len == 0 {
            return Ok(0);
        }
        let addr = self
            .memory
            .dma_addr()
            .checked_add(BUFFER_OFFSET as u64)
            .ok_or(Error::MemoryTooSmall)?;
        self.queue.submit(&[Buffer {
            addr,
            len: len as u32,
            device_writes: true,
        }])?;
        self.request_pending = true;
        if self.queue.should_notify() {
            self.transport.notify(REQUEST_QUEUE);
        }
        let written = (self.wait_for_answer(keep_waiting)? as usize).min(len);
        let buffer = self.memory.as_mut_ptr().wrapping_add(BUFFER_OFFSET);
        for (index, byte) in bytes[..written].iter_mut().enumerate() {
            // SAFETY: inside the request buffer of the driver's DMA memory, which the device has
            // handed back.
            *byte = unsafe { buffer.wrapping_add(index).read_volatile() };
        }
        Ok(written)
    }

    fn wait_for_answer(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<u32, Error> {
        loop {
            if let Some(used) = self.queue.pop_used() {
                self.request_pending = false;
                return Ok(used.len);
            }
            if !keep_waiting() {
                return Err(Error::TimedOut);
            }
        }
    }

    /// Resets the device and hands back the transport and the memory, with the proof that the
    /// device no longer uses it. If the reset does not read back 0, the memory is leaked.
    pub fn reset(self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(T, M, ResetDone), Error> {
        let Self {
            transport, memory, ..
        } = self;
        match super::reset(&transport, keep_waiting) {
            Ok(done) => Ok((transport, memory, done)),
            Err(error) => {
                core::mem::forget(memory);
                Err(error)
            }
        }
    }
}

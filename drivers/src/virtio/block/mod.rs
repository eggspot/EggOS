//! The virtio block device (virtio 1.2, 5.2), polled: one request queue, one request at a time,
//! each a header the device reads, the data and a status byte the device writes. Disks are
//! addressed in sectors of [`SECTOR_SIZE`] bytes whatever their own block size; a transfer
//! larger than [`MAX_TRANSFER`] goes as several requests.

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::{DmaAddr, DmaMemory};

#[cfg(test)]
mod tests;

/// The unit requests address the disk in, whatever its own block size.
pub const SECTOR_SIZE: usize = 512;
/// Entries of the request queue the driver sets up, at most: a request takes three.
pub const QUEUE_SIZE: u16 = 8;
/// Where the request header starts in the driver's DMA memory; the queue is at its start.
pub const HEADER_OFFSET: usize = 1024;
/// Where the status byte the device writes is.
pub const STATUS_OFFSET: usize = HEADER_OFFSET + HEADER_BYTES;
/// Where the data of a request starts.
pub const DATA_OFFSET: usize = 4096;
/// The most bytes one request moves.
pub const MAX_TRANSFER: usize = 60 * 1024;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = DATA_OFFSET + MAX_TRANSFER;

const HEADER_BYTES: usize = 16;
const REQUEST_QUEUE: u16 = 0;
const DESCRIPTORS_PER_REQUEST: u16 = 3;
const NO_STATUS: u8 = 0xff;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= HEADER_OFFSET);
const _: () = assert!(STATUS_OFFSET < DATA_OFFSET);
const _: () = assert!(MAX_TRANSFER.is_multiple_of(SECTOR_SIZE));

/// Feature bits of the block device (virtio 1.2, 5.2.3).
pub mod feature {
    /// The disk cannot be written.
    pub const READ_ONLY: u64 = 1 << 5;
}

/// Request types (`VIRTIO_BLK_T_*`).
pub mod request {
    /// Read sectors into the data.
    pub const IN: u32 = 0;
    /// Write the data to sectors.
    pub const OUT: u32 = 1;
}

/// The status byte's values (`VIRTIO_BLK_S_*`).
pub mod status {
    pub const OK: u8 = 0;
    pub const IO_ERROR: u8 = 1;
    pub const UNSUPPORTED: u8 = 2;
}

/// The device configuration's layout (`virtio_blk_config`).
pub mod config {
    /// The disk's size in 512-byte sectors, 64 bits, little-endian.
    pub const CAPACITY_OFFSET: usize = 0;
}

/// A virtio block device, set up and polled.
#[derive(Debug)]
pub struct VirtioBlock<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queue: SplitQueue,
    sectors: u64,
    read_only: bool,
    request_pending: bool,
}

impl<T: Transport, M: DmaMemory> VirtioBlock<T, M> {
    /// Sets the block device behind `transport` up with `memory` ([`MEMORY_BYTES`] or more) for
    /// its queue, request header and data, polling while `keep_waiting` says to. On an error
    /// the transport comes back, and the memory too unless the device may still use it (a
    /// reset that did not read back 0): then it is leaked.
    pub fn new(
        transport: T,
        memory: M,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, (Error, T, Option<M>)> {
        if memory.len() < MEMORY_BYTES {
            return Err((Error::MemoryTooSmall, transport, Some(memory)));
        }
        if transport.device_type() != device_type::BLOCK {
            return Err((Error::WrongDeviceType, transport, Some(memory)));
        }
        let accepted = match super::initialize(&transport, feature::READ_ONLY, keep_waiting) {
            Ok(accepted) => accepted,
            Err(error) => return Err((error, transport, Some(memory))),
        };
        let max = transport.max_queue_size(REQUEST_QUEUE).min(QUEUE_SIZE);
        let size = match max {
            0 => 0,
            max => 1 << max.ilog2(),
        };
        if size < DESCRIPTORS_PER_REQUEST {
            super::fail(&transport);
            return Err((Error::QueueUnavailable, transport, Some(memory)));
        }
        // SAFETY: `memory` holds at least `MEMORY_BYTES`, the queue's `memory_size(QUEUE_SIZE)`
        // before `HEADER_OFFSET`, 4096-aligned, and owned by this driver, which keeps it for as
        // long as the queue lives; the device reaches it at its DMA address.
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
        let sectors = read_capacity(&transport);
        super::driver_ok(&transport);
        Ok(Self {
            transport,
            memory,
            queue,
            sectors,
            read_only: accepted & feature::READ_ONLY != 0,
            request_pending: false,
        })
    }

    /// The transport the device is reached through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The disk's size in [`SECTOR_SIZE`] sectors.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// Whether the device refuses writes.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Reads the sectors from `sector` on into `buffer`, whose length must be a whole number
    /// of sectors, polling while `keep_waiting` says to. A request the device has not answered
    /// when polling stops stays posted ([`Error::TimedOut`]): the next call waits for it first.
    pub fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.check_range(sector, buffer.len())?;
        let mut at = sector;
        for chunk in buffer.chunks_mut(MAX_TRANSFER) {
            self.request(request::IN, at, chunk.len(), keep_waiting)?;
            // SAFETY: the data area of the driver's DMA memory holds `chunk.len()` bytes or more
            // (`MAX_TRANSFER`), which the device wrote and handed back with the request's
            // completion (its used index read with acquire ordering), and does not touch again
            // until the next request; `chunk` is the caller's own memory, apart from it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.memory.as_mut_ptr().wrapping_add(DATA_OFFSET),
                    chunk.as_mut_ptr(),
                    chunk.len(),
                )
            };
            at += (chunk.len() / SECTOR_SIZE) as u64;
        }
        Ok(())
    }

    /// Writes `buffer`, a whole number of sectors, to the sectors from `sector` on, polling
    /// while `keep_waiting` says to; [`Error::ReadOnly`] on a read-only disk.
    pub fn write(
        &mut self,
        sector: u64,
        buffer: &[u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        self.check_range(sector, buffer.len())?;
        let mut at = sector;
        for chunk in buffer.chunks(MAX_TRANSFER) {
            if self.request_pending {
                self.wait_for_answer(keep_waiting)?;
            }
            // SAFETY: the data area of the driver's DMA memory holds `chunk.len()` bytes or more,
            // and no request is posted, so the device does not read or write it now.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    self.memory.as_mut_ptr().wrapping_add(DATA_OFFSET),
                    chunk.len(),
                )
            };
            self.request(request::OUT, at, chunk.len(), keep_waiting)?;
            at += (chunk.len() / SECTOR_SIZE) as u64;
        }
        Ok(())
    }

    fn check_range(&self, sector: u64, bytes: usize) -> Result<(), Error> {
        if !bytes.is_multiple_of(SECTOR_SIZE) {
            return Err(Error::PartialSector);
        }
        let end = sector.checked_add((bytes / SECTOR_SIZE) as u64);
        match end {
            Some(end) if end <= self.sectors => Ok(()),
            _ => Err(Error::BeyondCapacity),
        }
    }

    fn request(
        &mut self,
        kind: u32,
        sector: u64,
        len: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.request_pending {
            self.wait_for_answer(keep_waiting)?;
        }
        let base = self.memory.as_mut_ptr();
        let header = base.wrapping_add(HEADER_OFFSET);
        // SAFETY: the header's type field in the driver's DMA memory, 8-byte aligned; no request
        // is posted, so the device does not read it now.
        unsafe { header.cast::<u32>().write_volatile(kind.to_le()) };
        // SAFETY: the header's reserved field, as above.
        unsafe { header.wrapping_add(4).cast::<u32>().write_volatile(0) };
        // SAFETY: the header's sector field, as above.
        unsafe {
            header
                .wrapping_add(8)
                .cast::<u64>()
                .write_volatile(sector.to_le())
        };
        // SAFETY: the status byte, which the device writes only once the request is posted.
        unsafe { base.wrapping_add(STATUS_OFFSET).write_volatile(NO_STATUS) };
        let at = |offset: usize| -> Result<DmaAddr, Error> {
            self.memory
                .dma_addr()
                .checked_add(offset as u64)
                .ok_or(Error::MemoryTooSmall)
        };
        let chain = [
            Buffer {
                addr: at(HEADER_OFFSET)?,
                len: HEADER_BYTES as u32,
                device_writes: false,
            },
            Buffer {
                addr: at(DATA_OFFSET)?,
                len: len as u32,
                device_writes: kind == request::IN,
            },
            Buffer {
                addr: at(STATUS_OFFSET)?,
                len: 1,
                device_writes: true,
            },
        ];
        self.queue.submit(&chain)?;
        self.request_pending = true;
        if self.queue.should_notify() {
            self.transport.notify(REQUEST_QUEUE);
        }
        self.wait_for_answer(keep_waiting)?;
        // SAFETY: the status byte, which the device wrote and handed back with the request.
        match unsafe { base.wrapping_add(STATUS_OFFSET).read_volatile() } {
            status::OK => Ok(()),
            failed => Err(Error::CommandFailed(failed.into())),
        }
    }

    fn wait_for_answer(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        loop {
            if self.queue.pop_used().is_some() {
                self.request_pending = false;
                return Ok(());
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

const CAPACITY_READS: usize = 4;

fn read_capacity<T: Transport + ?Sized>(transport: &T) -> u64 {
    let word = |offset| u64::from(transport.read_config32(offset));
    let mut capacity = 0;
    for _ in 0..CAPACITY_READS {
        let high = word(config::CAPACITY_OFFSET + 4);
        capacity = high << 32 | word(config::CAPACITY_OFFSET);
        if word(config::CAPACITY_OFFSET + 4) == high {
            break;
        }
    }
    capacity
}

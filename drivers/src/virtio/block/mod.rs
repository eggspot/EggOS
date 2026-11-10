//! The virtio block device (virtio 1.2, 5.2): one request queue with up to
//! [`MAX_IN_FLIGHT`] requests in flight (D220), each a header the device reads, the data (none
//! for a flush) and a status byte the device writes, in a header, a status byte and a run of
//! data slots of its own ([`requests`]). A transfer's requests are posted together and complete
//! in any order; every call returns only once all of its requests did, and a flush is posted
//! alone after every earlier request completed, so a file system's write barriers hold. Disks
//! are addressed in sectors of [`SECTOR_SIZE`] bytes whatever their own block size; a transfer
//! larger than [`MAX_TRANSFER`] goes as several requests. The driver polls the used ring, or,
//! after [`VirtioBlock::enable_interrupts`], the device interrupts as each request completes
//! and the caller's `keep_waiting` sleeps until then.

pub mod requests;

use super::queue::{Buffer, SplitQueue};
use super::{Error, ResetDone, Transport, device_type};
use crate::dma::{DmaAddr, DmaMemory};
pub use requests::{DATA_SLOTS, MAX_IN_FLIGHT, RequestTable, SLOT_BYTES};

#[cfg(test)]
mod tests;

/// The unit requests address the disk in, whatever its own block size.
pub const SECTOR_SIZE: usize = 512;
/// Entries of the request queue the driver sets up, at most: a request takes three.
pub const QUEUE_SIZE: u16 = 64;
/// Where the request headers start in the driver's DMA memory, one per request number; the
/// queue is at its start.
pub const HEADER_OFFSET: usize = 2048;
/// Where the status bytes the device writes are, one per request number.
pub const STATUS_OFFSET: usize = HEADER_OFFSET + MAX_IN_FLIGHT * HEADER_BYTES;
/// Where the data slots start.
pub const DATA_OFFSET: usize = 4096;
/// The most bytes one request moves: every data slot.
pub const MAX_TRANSFER: usize = DATA_SLOTS * SLOT_BYTES;
/// The DMA memory the driver needs.
pub const MEMORY_BYTES: usize = DATA_OFFSET + MAX_TRANSFER;

const HEADER_BYTES: usize = 16;
const REQUEST_QUEUE: u16 = 0;
const DESCRIPTORS_PER_REQUEST: u16 = 3;
const NO_STATUS: u8 = 0xff;

const _: () = assert!(super::queue::memory_size(QUEUE_SIZE) <= HEADER_OFFSET);
const _: () = assert!(STATUS_OFFSET + MAX_IN_FLIGHT <= DATA_OFFSET);
const _: () = assert!(MAX_TRANSFER.is_multiple_of(SECTOR_SIZE));
const _: () = assert!(SLOT_BYTES.is_multiple_of(SECTOR_SIZE));

/// Feature bits of the block device (virtio 1.2, 5.2.3).
pub mod feature {
    /// The disk cannot be written.
    pub const READ_ONLY: u64 = 1 << 5;
    /// The disk has a volatile write cache that a flush request writes back.
    pub const FLUSH: u64 = 1 << 9;
}

/// Request types (`VIRTIO_BLK_T_*`).
pub mod request {
    /// Read sectors into the data.
    pub const IN: u32 = 0;
    /// Write the data to sectors.
    pub const OUT: u32 = 1;
    /// Write the disk's volatile cache back to stable storage; no data.
    pub const FLUSH: u32 = 4;
}

/// The status byte's values (`VIRTIO_BLK_S_*`).
pub mod status {
    /// The request succeeded.
    pub const OK: u8 = 0;
    /// The device failed the transfer.
    pub const IO_ERROR: u8 = 1;
    /// The device does not support the request type.
    pub const UNSUPPORTED: u8 = 2;
}

/// The device configuration's layout (`virtio_blk_config`).
pub mod config {
    /// The disk's size in 512-byte sectors, 64 bits, little-endian.
    pub const CAPACITY_OFFSET: usize = 0;
}

/// A virtio block device, set up.
#[derive(Debug)]
pub struct VirtioBlock<T: Transport, M: DmaMemory> {
    transport: T,
    memory: M,
    queue: SplitQueue,
    sectors: u64,
    read_only: bool,
    flushes: bool,
    requests: RequestTable,
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
        let wanted = feature::READ_ONLY | feature::FLUSH;
        let accepted = match super::initialize(&transport, wanted, keep_waiting) {
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
            flushes: accepted & feature::FLUSH != 0,
            requests: RequestTable::new(),
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

    /// Has the device interrupt as it completes requests, for a driver whose `keep_waiting`
    /// sleeps until an interrupt instead of polling (D220); requests answered before are found
    /// by the next look at the used ring.
    pub fn enable_interrupts(&mut self) {
        self.queue.enable_used_notifications();
    }

    /// How many requests are in flight: posted, and not yet handed back to a caller.
    pub fn in_flight(&self) -> usize {
        self.requests.in_flight()
    }

    /// Reads the sectors from `sector` on into `buffer`, whose length must be a whole number
    /// of sectors, polling while `keep_waiting` says to. Requests the device has not answered
    /// when polling stops stay posted ([`Error::TimedOut`]): the next call waits for them first.
    pub fn read(
        &mut self,
        sector: u64,
        buffer: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.read_many(&mut [(sector, buffer)], keep_waiting)
    }

    /// Reads every one of `reads`, each the sectors from its first one into its buffer (a whole
    /// number of sectors), with as many requests in flight at once as the queue and the data
    /// slots allow, polling while `keep_waiting` says to; refused without a request when one of
    /// them is out of range.
    pub fn read_many(
        &mut self,
        reads: &mut [(u64, &mut [u8])],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        for (sector, buffer) in reads.iter() {
            self.check_range(*sector, buffer.len())?;
        }
        let mut job = ReadJob {
            reads,
            cursor: Cursor::default(),
        };
        self.transfer(request::IN, &mut job, keep_waiting)
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
        let mut job = WriteJob {
            sector,
            buffer,
            cursor: Cursor::default(),
        };
        self.transfer(request::OUT, &mut job, keep_waiting)
    }

    /// Whether the device has a volatile write cache that [`Self::flush`] writes back.
    pub fn has_write_cache(&self) -> bool {
        self.flushes
    }

    /// Returns once every write before it is on stable storage, polling while `keep_waiting`
    /// says to. The flush is posted alone, once every earlier request completed, and nothing is
    /// posted until it completes. A device that did not offer `VIRTIO_BLK_F_FLUSH` has no
    /// volatile cache: its writes are stable when they complete, and there is nothing to do.
    pub fn flush(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        if !self.flushes || self.read_only {
            return Ok(());
        }
        self.transfer(request::FLUSH, &mut FlushJob(false), keep_waiting)
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

    fn transfer(
        &mut self,
        kind: u32,
        job: &mut dyn Job,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        self.drain(keep_waiting)?;
        let mut held: [Option<Piece>; MAX_IN_FLIGHT] = [None; MAX_IN_FLIGHT];
        let mut failure = None;
        let mut next = job.next_piece();
        loop {
            let mut posted_any = false;
            while failure.is_none() {
                let Some(piece) = next else {
                    break;
                };
                match self.post(kind, piece, job)? {
                    Some(request) => {
                        held[request] = Some(piece);
                        next = job.next_piece();
                        posted_any = true;
                    }
                    None => break,
                }
            }
            if posted_any && self.queue.should_notify() {
                self.transport.notify(REQUEST_QUEUE);
            }
            if held.iter().all(Option::is_none) {
                return failure.map_or(Ok(()), Err);
            }
            match self.next_answered() {
                Some(request) => {
                    let piece = held[request].take();
                    let answer = self.finish(request, &mut |data| {
                        if let Some(piece) = piece {
                            job.take(piece, &data[..piece.bytes.min(data.len())]);
                        }
                    });
                    if let Err(error) = answer {
                        failure.get_or_insert(error);
                    }
                }
                None if !keep_waiting() => return Err(Error::TimedOut),
                None => {}
            }
        }
    }

    fn drain(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> Result<(), Error> {
        while self.requests.in_flight() > 0 {
            match self.next_answered() {
                Some(request) => self.requests.release(request),
                None if !keep_waiting() => return Err(Error::TimedOut),
                None => {}
            }
        }
        Ok(())
    }

    fn next_answered(&mut self) -> Option<usize> {
        while let Some(used) = self.queue.pop_used() {
            if let Some(request) = self.requests.answered(used.head) {
                return Some(request);
            }
        }
        None
    }

    fn at(&self, offset: usize) -> Result<DmaAddr, Error> {
        self.memory
            .dma_addr()
            .checked_add(offset as u64)
            .ok_or(Error::MemoryTooSmall)
    }

    fn post(&mut self, kind: u32, piece: Piece, job: &mut dyn Job) -> Result<Option<usize>, Error> {
        let descriptors = match piece.bytes {
            0 => DESCRIPTORS_PER_REQUEST - 1,
            _ => DESCRIPTORS_PER_REQUEST,
        };
        if self.queue.free_descriptors() < descriptors
            || (kind == request::FLUSH && self.requests.in_flight() > 0)
        {
            return Ok(None);
        }
        let Some((request, first_slot)) =
            self.requests.reserve(RequestTable::slots_for(piece.bytes))
        else {
            return Ok(None);
        };
        let header_offset = HEADER_OFFSET + request * HEADER_BYTES;
        let status_offset = STATUS_OFFSET + request;
        let data_offset = DATA_OFFSET + first_slot * SLOT_BYTES;
        let base = self.memory.as_mut_ptr();
        if kind == request::OUT {
            // SAFETY: the request's data slots, `piece.bytes` or more inside the driver's DMA
            // memory ([`MEMORY_BYTES`]), which it reserved just now: the device does not reach
            // them until the request is posted below, and no other request holds them.
            let data = unsafe {
                core::slice::from_raw_parts_mut(base.wrapping_add(data_offset), piece.bytes)
            };
            job.fill(piece, data);
        }
        let header = base.wrapping_add(header_offset);
        // SAFETY: the request's header's type field in the driver's DMA memory, 8-byte aligned;
        // the request is not posted, so the device does not read it now.
        unsafe { header.cast::<u32>().write_volatile(kind.to_le()) };
        // SAFETY: the header's reserved field, as above.
        unsafe { header.wrapping_add(4).cast::<u32>().write_volatile(0) };
        // SAFETY: the header's sector field, as above.
        unsafe {
            header
                .wrapping_add(8)
                .cast::<u64>()
                .write_volatile(piece.sector.to_le())
        };
        // SAFETY: the request's status byte, which the device writes only once it is posted.
        unsafe { base.wrapping_add(status_offset).write_volatile(NO_STATUS) };
        let header = Buffer {
            addr: self.at(header_offset)?,
            len: HEADER_BYTES as u32,
            device_writes: false,
        };
        let status = Buffer {
            addr: self.at(status_offset)?,
            len: 1,
            device_writes: true,
        };
        let data = Buffer {
            addr: self.at(data_offset)?,
            len: piece.bytes as u32,
            device_writes: kind == request::IN,
        };
        let submitted = match piece.bytes {
            0 => self.queue.submit(&[header, status]),
            _ => self.queue.submit(&[header, data, status]),
        };
        match submitted {
            Ok(head) => {
                self.requests.posted(request, head);
                Ok(Some(request))
            }
            Err(error) => {
                self.requests.release(request);
                Err(error)
            }
        }
    }

    fn finish(&mut self, request: usize, take: &mut dyn FnMut(&[u8])) -> Result<(), Error> {
        let Some(in_flight) = self.requests.get(request) else {
            return Ok(());
        };
        let base = self.memory.as_mut_ptr();
        // SAFETY: the request's status byte, which the device wrote and handed back with the
        // request (its used index read with acquire ordering).
        let status = unsafe { base.wrapping_add(STATUS_OFFSET + request).read_volatile() };
        if status == status::OK {
            let length = in_flight.slots * SLOT_BYTES;
            // SAFETY: the request's data slots inside the driver's DMA memory, which the device
            // wrote and handed back with the request and does not touch again until they are
            // posted anew, after the release below.
            let data = unsafe {
                core::slice::from_raw_parts(
                    base.wrapping_add(DATA_OFFSET + in_flight.first_slot * SLOT_BYTES),
                    length,
                )
            };
            take(data);
        }
        self.requests.release(request);
        match status {
            status::OK => Ok(()),
            failed => Err(Error::CommandFailed(failed.into())),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    read: usize,
    offset: usize,
    sector: u64,
    bytes: usize,
}

#[derive(Debug, Default)]
struct Cursor {
    read: usize,
    offset: usize,
}

impl Cursor {
    fn next_piece(&mut self, transfer_at: &dyn Fn(usize) -> Option<(u64, usize)>) -> Option<Piece> {
        loop {
            let (sector, bytes) = transfer_at(self.read)?;
            if self.offset >= bytes {
                self.read += 1;
                self.offset = 0;
                continue;
            }
            let length = (bytes - self.offset).min(MAX_TRANSFER);
            let piece = Piece {
                read: self.read,
                offset: self.offset,
                sector: sector + (self.offset / SECTOR_SIZE) as u64,
                bytes: length,
            };
            self.offset += length;
            return Some(piece);
        }
    }
}

trait Job {
    fn next_piece(&mut self) -> Option<Piece>;
    fn fill(&mut self, _piece: Piece, _data: &mut [u8]) {}
    fn take(&mut self, _piece: Piece, _data: &[u8]) {}
}

struct ReadJob<'r, 'a> {
    reads: &'r mut [(u64, &'a mut [u8])],
    cursor: Cursor,
}

impl Job for ReadJob<'_, '_> {
    fn next_piece(&mut self) -> Option<Piece> {
        let reads = &*self.reads;
        self.cursor.next_piece(&|index| {
            reads
                .get(index)
                .map(|(sector, buffer)| (*sector, buffer.len()))
        })
    }

    fn take(&mut self, piece: Piece, data: &[u8]) {
        if let Some((_, buffer)) = self.reads.get_mut(piece.read)
            && let Some(place) = buffer.get_mut(piece.offset..piece.offset + data.len())
        {
            place.copy_from_slice(data);
        }
    }
}

struct WriteJob<'b> {
    sector: u64,
    buffer: &'b [u8],
    cursor: Cursor,
}

impl Job for WriteJob<'_> {
    fn next_piece(&mut self) -> Option<Piece> {
        let whole = (self.sector, self.buffer.len());
        self.cursor
            .next_piece(&|index| (index == 0).then_some(whole))
    }

    fn fill(&mut self, piece: Piece, data: &mut [u8]) {
        if let Some(source) = self.buffer.get(piece.offset..piece.offset + data.len()) {
            data.copy_from_slice(source);
        }
    }
}

struct FlushJob(bool);

impl Job for FlushJob {
    fn next_piece(&mut self) -> Option<Piece> {
        if self.0 {
            return None;
        }
        self.0 = true;
        Some(Piece {
            read: 0,
            offset: 0,
            sector: 0,
            bytes: 0,
        })
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

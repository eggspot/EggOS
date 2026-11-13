//! The split virtqueue (virtio 1.2, 2.7): a descriptor table, the available ring the driver
//! fills and the used ring the device fills, in memory both reach.
//!
//! Every access to the shared memory goes through raw pointers, volatile or atomic, never a
//! reference: the device writes it behind the compiler's back. Ring indices run free as
//! wrapping `u16`s; the ring slot is the index modulo the queue size (a power of two).

use super::{Error, QueueAddresses};
use crate::dma::{CacheMaintenance, DmaAddr};
use core::sync::atomic::{AtomicU16, Ordering, fence};

/// The largest queue size the split ring allows.
pub const MAX_SIZE: u16 = 32768;

/// Descriptor flag: the chain continues at `next`.
pub const DESCRIPTOR_NEXT: u16 = 1;
/// Descriptor flag: the device writes this buffer (otherwise it reads it).
pub const DESCRIPTOR_WRITE: u16 = 2;
/// Descriptor flag: the buffer is a table of descriptors, the chain itself (D563).
pub const DESCRIPTOR_INDIRECT: u16 = 4;
/// Used ring flag: the device asks not to be notified of new buffers.
pub const USED_NO_NOTIFY: u16 = 1;
/// Available ring flag: the driver asks not to be notified of used buffers (it polls).
pub const AVAIL_NO_INTERRUPT: u16 = 1;

const DESCRIPTOR_SIZE: usize = 16;
const NO_DESCRIPTOR: u16 = u16::MAX;

const fn align_up(offset: usize, alignment: usize) -> usize {
    offset.div_ceil(alignment) * alignment
}

const fn avail_offset(size: u16) -> usize {
    align_up(DESCRIPTOR_SIZE * size as usize, 2)
}

const fn used_offset(size: u16) -> usize {
    align_up(avail_offset(size) + 6 + 2 * size as usize, 4)
}

/// Bytes a queue of `size` entries takes: the descriptor table, then the available ring
/// (2-byte aligned), then the used ring (4-byte aligned).
pub const fn memory_size(size: u16) -> usize {
    used_offset(size) + 6 + 8 * size as usize
}

/// One buffer of a chain the driver hands the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    /// Where the device reaches it.
    pub addr: DmaAddr,
    /// Its length in bytes.
    pub len: u32,
    /// Whether the device writes it (otherwise it reads it).
    pub device_writes: bool,
}

/// A chain the device has finished with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Used {
    /// The chain's first descriptor, as [`SplitQueue::submit`] returned it.
    pub head: u16,
    /// How many bytes the device wrote into the chain's writable buffers.
    pub len: u32,
}

struct Ring {
    memory: *mut u8,
    size: u16,
}

impl Ring {
    fn mask(&self) -> u16 {
        self.size - 1
    }

    fn at<T>(&self, offset: usize) -> *mut T {
        self.memory.wrapping_add(offset).cast()
    }

    fn descriptor(&self, index: u16) -> *mut u8 {
        self.at(DESCRIPTOR_SIZE * usize::from(index))
    }

    fn avail(&self, field: usize) -> *mut u16 {
        self.at(avail_offset(self.size) + field)
    }

    fn avail_slot(&self, index: u16) -> *mut u16 {
        self.avail(4 + 2 * usize::from(index & self.mask()))
    }

    fn used(&self, field: usize) -> *mut u16 {
        self.at(used_offset(self.size) + field)
    }

    fn used_element(&self, index: u16) -> *mut u32 {
        self.at(used_offset(self.size) + 4 + 8 * usize::from(index & self.mask()))
    }

    fn used_event(&self) -> *mut u16 {
        self.avail(4 + 2 * usize::from(self.size))
    }

    fn avail_event(&self) -> *mut u16 {
        self.used(4 + 8 * usize::from(self.size))
    }

    fn index(&self, pointer: *mut u16) -> &AtomicU16 {
        // SAFETY: `pointer` is an index field inside the ring's memory, 2-byte aligned, which
        // the owner of the ring keeps valid while the ring lives; the device updates it, so it
        // is only ever reached atomically or volatilely.
        unsafe { AtomicU16::from_ptr(pointer) }
    }
}

/// A split virtqueue, the driver's side.
#[derive(Debug)]
pub struct SplitQueue {
    ring: Ring,
    dma: DmaAddr,
    free_head: u16,
    never_used: u16,
    free: u16,
    avail_idx: u16,
    last_used: u16,
    event_index: bool,
    interrupts_wanted: bool,
    notified_at: core::cell::Cell<u16>,
    in_flight: InFlight,
    rejected: u64,
    maintenance: Option<CacheMaintenance>,
}

/// Which descriptor heads the device holds: a bit per descriptor, so a used element naming a
/// head the driver never made available, or one already returned, is refused (D563).
#[derive(Debug)]
struct InFlight {
    words: [u64; MAX_SIZE as usize / 64],
}

impl InFlight {
    const fn new() -> Self {
        Self {
            words: [0; MAX_SIZE as usize / 64],
        }
    }

    fn mark(&mut self, head: u16) {
        self.words[usize::from(head) / 64] |= 1 << (head % 64);
    }

    fn take(&mut self, head: u16) -> bool {
        let word = &mut self.words[usize::from(head) / 64];
        let bit = 1 << (head % 64);
        let held = *word & bit != 0;
        *word &= !bit;
        held
    }
}

/// A table of indirect descriptors (virtio 1.2, 2.7.5.3): memory the device reads a chain
/// from, so a chain of any length takes one ring descriptor (D563).
#[derive(Debug)]
pub struct IndirectTable {
    memory: *mut u8,
    dma: DmaAddr,
    capacity: u16,
}

impl IndirectTable {
    /// A table of `capacity` descriptors in the `16 · capacity` bytes at `memory`, which the
    /// device reaches at `dma`.
    ///
    /// # Safety
    /// `memory` must be valid for writes of `16 · capacity` bytes, 16-byte aligned, the device
    /// must reach them at `dma`, and nothing else may use them while a chain written into
    /// them is in the device's hands.
    pub unsafe fn new(memory: *mut u8, dma: DmaAddr, capacity: u16) -> Self {
        Self {
            memory,
            dma,
            capacity,
        }
    }

    /// How many descriptors it holds.
    pub fn capacity(&self) -> u16 {
        self.capacity
    }
}

// SAFETY: the table owns its view of the memory; moving it moves that ownership.
unsafe impl Send for IndirectTable {}

impl core::fmt::Debug for Ring {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ring")
            .field("memory", &self.memory)
            .field("size", &self.size)
            .finish()
    }
}

// SAFETY: the queue owns its view of the ring memory; moving it to another CPU moves that
// ownership, and nothing in it is tied to the CPU that made it.
unsafe impl Send for SplitQueue {}

impl SplitQueue {
    /// A queue of `size` entries in the [`memory_size`]`(size)` bytes at `memory`, which the
    /// device reaches at `dma`. Clears both rings' flags and indices; the descriptor table
    /// needs no clearing. `size` must be a power of two up to [`MAX_SIZE`].
    ///
    /// # Safety
    /// `memory` must be valid for reads and writes of `memory_size(size)` bytes, 16-byte aligned,
    /// for as long as the queue lives, the device must reach the same bytes at `dma`, and
    /// nothing but this queue and the device may use them.
    pub unsafe fn new(memory: *mut u8, dma: DmaAddr, size: u16) -> Result<Self, Error> {
        if !size.is_power_of_two() || size > MAX_SIZE {
            return Err(Error::QueueTooSmall);
        }
        let ring = Ring { memory, size };
        for field in [ring.avail(0), ring.avail(2), ring.used(0), ring.used(2)] {
            // SAFETY: a field of the rings inside the memory the caller vouches for.
            unsafe { field.write_volatile(0) };
        }
        Ok(Self {
            ring,
            dma,
            free_head: NO_DESCRIPTOR,
            never_used: 0,
            free: size,
            avail_idx: 0,
            last_used: 0,
            event_index: false,
            interrupts_wanted: true,
            notified_at: core::cell::Cell::new(0),
            in_flight: InFlight::new(),
            rejected: 0,
            maintenance: None,
        })
    }

    /// Uses the event index fields (`VIRTIO_F_EVENT_IDX`, D563) instead of the flags, once the
    /// device accepted the feature: the driver asks for an interrupt at one used index, and
    /// notifies the device only when it passed the available index the device asked for.
    pub fn use_event_index(&mut self) {
        self.event_index = true;
        let wanted = match self.interrupts_wanted {
            true => self.last_used,
            false => self.last_used.wrapping_sub(1),
        };
        self.set_used_event(wanted);
    }

    /// Keeps the rings in step with a device that does not snoop the CPU's caches (D563):
    /// what the driver writes is written back before it is published, what the device writes
    /// is dropped from the caches before it is read. `None` for a coherent device.
    pub fn set_cache_maintenance(&mut self, maintenance: Option<CacheMaintenance>) {
        self.maintenance = maintenance;
    }

    fn write_back(&self, at: *const u8, length: usize) {
        if let Some(maintenance) = self.maintenance {
            // SAFETY: `at` lies in the ring memory or an indirect table, both valid for the
            // queue's life by `new`'s and `IndirectTable::new`'s contracts.
            unsafe { (maintenance.write_back)(at, length) };
        }
    }

    fn discard(&self, at: *const u8, length: usize) {
        if let Some(maintenance) = self.maintenance {
            // SAFETY: as in `write_back`.
            unsafe { (maintenance.discard)(at, length) };
        }
    }

    fn set_used_event(&self, index: u16) {
        // SAFETY: the available ring's used-event field, which only the driver writes.
        unsafe { self.ring.used_event().write_volatile(index.to_le()) };
        self.write_back(self.ring.used_event().cast(), 2);
    }

    /// How many used elements the device returned that named no chain in its hands.
    pub fn rejected_used(&self) -> u64 {
        self.rejected
    }

    /// Asks the device not to notify the driver (an interrupt) when it uses buffers: for a
    /// driver that polls. The spec makes it a hint the device should follow.
    pub fn disable_used_notifications(&mut self) {
        // SAFETY: the available ring's flags field, which only the driver writes.
        unsafe {
            self.ring
                .avail(0)
                .write_volatile(AVAIL_NO_INTERRUPT.to_le())
        };
        self.interrupts_wanted = false;
        if self.event_index {
            let far = self.last_used.wrapping_sub(1);
            self.set_used_event(far);
        }
    }

    /// Asks the device to notify the driver (an interrupt) when it uses buffers again: for a
    /// driver that waits for interrupts. Buffers used before the flag reached the device raise
    /// none, so the driver looks at the used ring once after.
    pub fn enable_used_notifications(&mut self) {
        // SAFETY: the available ring's flags field, which only the driver writes.
        unsafe { self.ring.avail(0).write_volatile(0) };
        self.interrupts_wanted = true;
        if self.event_index {
            self.set_used_event(self.last_used);
        }
        fence(Ordering::SeqCst);
    }

    /// Where the device finds the queue's three areas.
    pub fn addresses(&self) -> QueueAddresses {
        let at = |offset: usize| DmaAddr::new(self.dma.get().wrapping_add(offset as u64));
        QueueAddresses {
            descriptors: self.dma,
            driver: at(avail_offset(self.ring.size)),
            device: at(used_offset(self.ring.size)),
        }
    }

    /// How many entries the queue has.
    pub fn size(&self) -> u16 {
        self.ring.size
    }

    /// How many descriptors are free for new chains.
    pub fn free_descriptors(&self) -> u16 {
        self.free
    }

    fn allocate(&mut self) -> u16 {
        self.free -= 1;
        if self.free_head == NO_DESCRIPTOR {
            self.never_used += 1;
            return self.never_used - 1;
        }
        let index = self.free_head;
        let next = self.ring.descriptor(index).wrapping_add(14).cast::<u16>();
        // SAFETY: the `next` field of a descriptor inside the table; the device does not use
        // free descriptors, so only this queue reads or writes it.
        self.free_head = u16::from_le(unsafe { next.read_volatile() });
        index
    }

    fn release(&mut self, index: u16) {
        let next = self.ring.descriptor(index).wrapping_add(14).cast::<u16>();
        // SAFETY: as in `allocate`: the descriptor is back in this queue's hands.
        unsafe { next.write_volatile(self.free_head.to_le()) };
        self.free_head = index;
        self.free += 1;
    }

    fn write_descriptor(&self, index: u16, buffer: &Buffer, flags: u16, next: u16) {
        let descriptor = self.ring.descriptor(index);
        // SAFETY: the address field of a descriptor in the table, 8-byte aligned (16-byte
        // entries in a table `new`'s caller aligned to 16 bytes).
        unsafe {
            descriptor
                .cast::<u64>()
                .write_volatile(buffer.addr.get().to_le())
        };
        // SAFETY: the length field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(8)
                .cast::<u32>()
                .write_volatile(buffer.len.to_le())
        };
        // SAFETY: the flags field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(12)
                .cast::<u16>()
                .write_volatile(flags.to_le())
        };
        // SAFETY: the next field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(14)
                .cast::<u16>()
                .write_volatile(next.to_le())
        };
    }

    /// Hands the device `chain`, buffers in order, and returns its head. Fails with
    /// [`Error::QueueFull`] for an empty chain or one longer than the free descriptors. The
    /// descriptors and the ring slot are written before the available index is published.
    pub fn submit(&mut self, chain: &[Buffer]) -> Result<u16, Error> {
        let Some((last, rest)) = chain.split_last() else {
            return Err(Error::QueueFull);
        };
        if chain.len() > usize::from(self.free) {
            return Err(Error::QueueFull);
        }
        let head = self.allocate();
        let mut current = head;
        for buffer in rest {
            let next = self.allocate();
            let flags = DESCRIPTOR_NEXT | write_flag(buffer);
            self.write_descriptor(current, buffer, flags, next);
            current = next;
        }
        self.write_descriptor(current, last, write_flag(last), 0);
        if self.maintenance.is_some() {
            let mut index = head;
            for _ in 0..chain.len() {
                let descriptor = self.ring.descriptor(index);
                self.write_back(descriptor, DESCRIPTOR_SIZE);
                // SAFETY: the next field of a descriptor this call wrote.
                index = u16::from_le(unsafe { descriptor.wrapping_add(14).cast::<u16>().read_volatile() });
            }
        }
        self.publish(head);
        Ok(head)
    }

    /// Hands the device `chain` through `table` (D563): the buffers go into the table and the
    /// ring gets one descriptor, so a long chain does not use the ring up. Fails with
    /// [`Error::QueueFull`] for an empty chain, one longer than the table, or no free
    /// descriptor. The table stays the device's until the head comes back.
    pub fn submit_indirect(&mut self, chain: &[Buffer], table: &IndirectTable) -> Result<u16, Error> {
        let count = chain.len();
        if count == 0 || count > usize::from(table.capacity) || self.free == 0 {
            return Err(Error::QueueFull);
        }
        for (index, buffer) in chain.iter().enumerate() {
            let is_last = index + 1 == count;
            let flags = write_flag(buffer) | if is_last { 0 } else { DESCRIPTOR_NEXT };
            let next = if is_last { 0 } else { index as u16 + 1 };
            let entry = table.memory.wrapping_add(DESCRIPTOR_SIZE * index);
            // SAFETY: entry `index` of the table, inside the `16 · capacity` bytes its maker
            // vouched for, 16-byte aligned; the device does not read it until it is published.
            unsafe { write_descriptor_at(entry, buffer.addr, buffer.len, flags, next) };
        }
        self.write_back(table.memory, DESCRIPTOR_SIZE * count);
        let head = self.allocate();
        let whole = Buffer {
            addr: table.dma,
            len: (DESCRIPTOR_SIZE * count) as u32,
            device_writes: false,
        };
        self.write_descriptor(head, &whole, DESCRIPTOR_INDIRECT, 0);
        self.write_back(self.ring.descriptor(head), DESCRIPTOR_SIZE);
        self.publish(head);
        Ok(head)
    }

    fn publish(&mut self, head: u16) {
        self.in_flight.mark(head);
        let slot = self.ring.avail_slot(self.avail_idx);
        // SAFETY: a slot of the available ring, which only the driver writes.
        unsafe { slot.write_volatile(head.to_le()) };
        self.write_back(slot.cast(), 2);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        fence(Ordering::Release);
        self.ring
            .index(self.ring.avail(2))
            .store(self.avail_idx.to_le(), Ordering::Release);
        self.write_back(self.ring.avail(0).cast(), 4);
    }

    /// Whether the device wants to be told about new buffers: a full fence (the published
    /// index before the flag's read), then the used ring's no-notify flag, or with the event
    /// index whether the available index passed the one the device asked to hear about since
    /// the last call.
    pub fn should_notify(&self) -> bool {
        fence(Ordering::SeqCst);
        self.discard(self.ring.used(0).cast(), 2);
        if self.event_index {
            self.discard(self.ring.avail_event().cast(), 2);
            // SAFETY: the used ring's avail-event field, which the device writes.
            let event = u16::from_le(unsafe { self.ring.avail_event().read_volatile() });
            let previous = self.notified_at.replace(self.avail_idx);
            return needs_event(event, self.avail_idx, previous);
        }
        // SAFETY: the used ring's flags field, which the device writes.
        let flags = u16::from_le(unsafe { self.ring.used(0).read_volatile() });
        flags & USED_NO_NOTIFY == 0
    }

    /// The next chain the device finished with, its descriptors freed; `None` if there is
    /// none yet.
    /// A used element naming a head the device does not hold (out of range, never made
    /// available, or already returned) is skipped and counted ([`Self::rejected_used`],
    /// D563), so a faulty device cannot make the driver free a chain twice.
    pub fn pop_used(&mut self) -> Option<Used> {
        loop {
            self.discard(self.ring.used(2).cast(), 2);
            let used_idx =
                u16::from_le(self.ring.index(self.ring.used(2)).load(Ordering::Acquire));
            if used_idx == self.last_used {
                return None;
            }
            let element = self.ring.used_element(self.last_used);
            self.discard(element.cast(), 8);
            // SAFETY: the id field of a used ring element the device published.
            let id = u32::from_le(unsafe { element.read_volatile() });
            // SAFETY: the length field of the same element.
            let len = u32::from_le(unsafe { element.wrapping_add(1).read_volatile() });
            self.last_used = self.last_used.wrapping_add(1);
            if self.event_index && self.interrupts_wanted {
                self.set_used_event(self.last_used);
            }
            let Ok(head) = u16::try_from(id) else {
                self.rejected += 1;
                continue;
            };
            if head >= self.ring.size || !self.in_flight.take(head) {
                self.rejected += 1;
                continue;
            }
            self.release_chain(head);
            return Some(Used { head, len });
        }
    }

    fn release_chain(&mut self, head: u16) {
        let mut index = head;
        for _ in 0..self.ring.size {
            if self.free == self.ring.size || index >= self.ring.size {
                return;
            }
            let descriptor = self.ring.descriptor(index);
            // SAFETY: the flags field of a descriptor the device handed back.
            let flags =
                u16::from_le(unsafe { descriptor.wrapping_add(12).cast::<u16>().read_volatile() });
            // SAFETY: the next field of the same descriptor.
            let next =
                u16::from_le(unsafe { descriptor.wrapping_add(14).cast::<u16>().read_volatile() });
            self.release(index);
            if flags & DESCRIPTOR_NEXT == 0 {
                return;
            }
            index = next;
        }
    }
}

/// Whether a device that asked to hear about index `event` must be told, now that the index
/// moved from `old` to `new` (virtio 1.2, 2.7.10): `event` lies in `old..new`, wrapping.
pub fn needs_event(event: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(event).wrapping_sub(1) < new.wrapping_sub(old)
}

/// Writes one descriptor at `entry`.
///
/// # Safety
/// `entry` must be valid for writes of 16 bytes, 8-byte aligned.
unsafe fn write_descriptor_at(entry: *mut u8, addr: DmaAddr, len: u32, flags: u16, next: u16) {
    // SAFETY: the address field of the descriptor the caller vouches for.
    unsafe { entry.cast::<u64>().write_volatile(addr.get().to_le()) };
    // SAFETY: the length field of the same descriptor.
    unsafe { entry.wrapping_add(8).cast::<u32>().write_volatile(len.to_le()) };
    // SAFETY: the flags field of the same descriptor.
    unsafe { entry.wrapping_add(12).cast::<u16>().write_volatile(flags.to_le()) };
    // SAFETY: the next field of the same descriptor.
    unsafe { entry.wrapping_add(14).cast::<u16>().write_volatile(next.to_le()) };
}

fn write_flag(buffer: &Buffer) -> u16 {
    if buffer.device_writes {
        DESCRIPTOR_WRITE
    } else {
        0
    }
}

/// The device's side of a split virtqueue: what a device that takes no time does. Host tests
/// and the kernel's benchmarks use it to stand in for a device.
#[derive(Debug)]
pub struct DeviceSide {
    ring: Ring,
    last_avail: u16,
    used_idx: u16,
}

impl DeviceSide {
    /// The device's view of the queue of `size` entries at `memory`, set up by
    /// [`SplitQueue::new`] (both indices 0).
    ///
    /// # Safety
    /// As for [`SplitQueue::new`], and the queue must live as long as this value.
    pub unsafe fn new(memory: *mut u8, size: u16) -> Self {
        Self {
            ring: Ring { memory, size },
            last_avail: 0,
            used_idx: 0,
        }
    }

    /// The head of the next chain the driver made available, if any.
    pub fn next_available(&mut self) -> Option<u16> {
        let avail_idx = u16::from_le(self.ring.index(self.ring.avail(2)).load(Ordering::Acquire));
        if avail_idx == self.last_avail {
            return None;
        }
        let slot = self.ring.avail_slot(self.last_avail);
        self.last_avail = self.last_avail.wrapping_add(1);
        // SAFETY: a slot of the available ring the driver published.
        Some(u16::from_le(unsafe { slot.read_volatile() }))
    }

    /// Descriptor `index`: address, length, flags and next.
    pub fn descriptor(&self, index: u16) -> (DmaAddr, u32, u16, u16) {
        let descriptor = self.ring.descriptor(index % self.ring.size);
        // SAFETY: the address field of a descriptor in the table.
        let addr = u64::from_le(unsafe { descriptor.cast::<u64>().read_volatile() });
        // SAFETY: the length field of the same descriptor.
        let len = u32::from_le(unsafe { descriptor.wrapping_add(8).cast::<u32>().read_volatile() });
        // SAFETY: the flags field of the same descriptor.
        let flags =
            u16::from_le(unsafe { descriptor.wrapping_add(12).cast::<u16>().read_volatile() });
        // SAFETY: the next field of the same descriptor.
        let next =
            u16::from_le(unsafe { descriptor.wrapping_add(14).cast::<u16>().read_volatile() });
        (DmaAddr::new(addr), len, flags, next)
    }

    /// Hands chain `head` back to the driver with `len` bytes written.
    pub fn put_used(&mut self, head: u16, len: u32) {
        let element = self.ring.used_element(self.used_idx);
        // SAFETY: the id field of a used ring element, which only the device writes.
        unsafe { element.write_volatile(u32::from(head).to_le()) };
        // SAFETY: the length field of the same element.
        unsafe { element.wrapping_add(1).write_volatile(len.to_le()) };
        self.used_idx = self.used_idx.wrapping_add(1);
        fence(Ordering::Release);
        self.ring
            .index(self.ring.used(2))
            .store(self.used_idx.to_le(), Ordering::Release);
    }

    /// Whether the driver asked not to be notified of used buffers.
    pub fn used_notifications_disabled(&self) -> bool {
        // SAFETY: the available ring's flags field, which the driver writes.
        let flags = u16::from_le(unsafe { self.ring.avail(0).read_volatile() });
        flags & AVAIL_NO_INTERRUPT != 0
    }

    /// The used index the driver asked to be interrupted at (`VIRTIO_F_EVENT_IDX`).
    pub fn used_event(&self) -> u16 {
        // SAFETY: the available ring's used-event field, which the driver writes.
        u16::from_le(unsafe { self.ring.used_event().read_volatile() })
    }

    /// Asks the driver to notify when its available index passes `index`.
    pub fn set_avail_event(&mut self, index: u16) {
        // SAFETY: the used ring's avail-event field, which only the device writes.
        unsafe { self.ring.avail_event().write_volatile(index.to_le()) };
    }

    /// Asks the driver not to notify (`on`) or to notify again.
    pub fn set_no_notify(&mut self, on: bool) {
        let flags = if on { USED_NO_NOTIFY } else { 0 };
        // SAFETY: the used ring's flags field, which only the device writes.
        unsafe { self.ring.used(0).write_volatile(flags.to_le()) };
    }
}

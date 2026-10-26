//! The split virtqueue (virtio 1.2, 2.7): a descriptor table, the available ring the driver
//! fills and the used ring the device fills, in memory both reach.
//!
//! Every access to the shared memory goes through raw pointers, volatile or atomic, never a
//! reference: the device writes it behind the compiler's back. Ring indices run free as
//! wrapping `u16`s; the ring slot is the index modulo the queue size (a power of two).

use super::{Error, QueueAddresses};
use crate::dma::DmaAddr;
use core::sync::atomic::{AtomicU16, Ordering, fence};

/// The largest queue size the split ring allows.
pub const MAX_SIZE: u16 = 32768;

/// Descriptor flag: the chain continues at `next`.
pub const DESCRIPTOR_NEXT: u16 = 1;
/// Descriptor flag: the device writes this buffer (otherwise it reads it).
pub const DESCRIPTOR_WRITE: u16 = 2;
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
}

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
        })
    }

    /// Asks the device not to notify the driver (an interrupt) when it uses buffers: for a
    /// driver that polls. The spec makes it a hint the device should follow.
    pub fn disable_used_notifications(&mut self) {
        // SAFETY: the available ring's flags field, which only the driver writes.
        unsafe { self.ring.avail(0).write_volatile(AVAIL_NO_INTERRUPT) };
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
        self.free_head = unsafe { next.read_volatile() };
        index
    }

    fn release(&mut self, index: u16) {
        let next = self.ring.descriptor(index).wrapping_add(14).cast::<u16>();
        // SAFETY: as in `allocate`: the descriptor is back in this queue's hands.
        unsafe { next.write_volatile(self.free_head) };
        self.free_head = index;
        self.free += 1;
    }

    fn write_descriptor(&self, index: u16, buffer: &Buffer, flags: u16, next: u16) {
        let descriptor = self.ring.descriptor(index);
        // SAFETY: the address field of a descriptor in the table, 8-byte aligned (16-byte
        // entries in a table `new`'s caller aligned to 16 bytes).
        unsafe { descriptor.cast::<u64>().write_volatile(buffer.addr.get()) };
        // SAFETY: the length field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(8)
                .cast::<u32>()
                .write_volatile(buffer.len)
        };
        // SAFETY: the flags field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(12)
                .cast::<u16>()
                .write_volatile(flags)
        };
        // SAFETY: the next field of the same descriptor.
        unsafe {
            descriptor
                .wrapping_add(14)
                .cast::<u16>()
                .write_volatile(next)
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
        let slot = self.ring.avail_slot(self.avail_idx);
        // SAFETY: a slot of the available ring, which only the driver writes.
        unsafe { slot.write_volatile(head) };
        self.avail_idx = self.avail_idx.wrapping_add(1);
        fence(Ordering::Release);
        self.ring
            .index(self.ring.avail(2))
            .store(self.avail_idx, Ordering::Release);
        Ok(head)
    }

    /// Whether the device wants to be told about new buffers: a full fence (the published
    /// index before the flag's read), then the used ring's no-notify flag.
    pub fn should_notify(&self) -> bool {
        fence(Ordering::SeqCst);
        // SAFETY: the used ring's flags field, which the device writes.
        let flags = unsafe { self.ring.used(0).read_volatile() };
        flags & USED_NO_NOTIFY == 0
    }

    /// The next chain the device finished with, its descriptors freed; `None` if there is
    /// none yet.
    pub fn pop_used(&mut self) -> Option<Used> {
        let used_idx = self.ring.index(self.ring.used(2)).load(Ordering::Acquire);
        if used_idx == self.last_used {
            return None;
        }
        let element = self.ring.used_element(self.last_used);
        // SAFETY: the id field of a used ring element the device published.
        let id = unsafe { element.read_volatile() };
        // SAFETY: the length field of the same element.
        let len = unsafe { element.wrapping_add(1).read_volatile() };
        self.last_used = self.last_used.wrapping_add(1);
        let head = id as u16;
        if id < u32::from(self.ring.size) {
            self.release_chain(head);
        }
        Some(Used { head, len })
    }

    fn release_chain(&mut self, head: u16) {
        let mut index = head;
        for _ in 0..self.ring.size {
            if self.free == self.ring.size || index >= self.ring.size {
                return;
            }
            let descriptor = self.ring.descriptor(index);
            // SAFETY: the flags field of a descriptor the device handed back.
            let flags = unsafe { descriptor.wrapping_add(12).cast::<u16>().read_volatile() };
            // SAFETY: the next field of the same descriptor.
            let next = unsafe { descriptor.wrapping_add(14).cast::<u16>().read_volatile() };
            self.release(index);
            if flags & DESCRIPTOR_NEXT == 0 {
                return;
            }
            index = next;
        }
    }
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
        let avail_idx = self.ring.index(self.ring.avail(2)).load(Ordering::Acquire);
        if avail_idx == self.last_avail {
            return None;
        }
        let slot = self.ring.avail_slot(self.last_avail);
        self.last_avail = self.last_avail.wrapping_add(1);
        // SAFETY: a slot of the available ring the driver published.
        Some(unsafe { slot.read_volatile() })
    }

    /// Descriptor `index`: address, length, flags and next.
    pub fn descriptor(&self, index: u16) -> (DmaAddr, u32, u16, u16) {
        let descriptor = self.ring.descriptor(index % self.ring.size);
        // SAFETY: the address field of a descriptor in the table.
        let addr = unsafe { descriptor.cast::<u64>().read_volatile() };
        // SAFETY: the length field of the same descriptor.
        let len = unsafe { descriptor.wrapping_add(8).cast::<u32>().read_volatile() };
        // SAFETY: the flags field of the same descriptor.
        let flags = unsafe { descriptor.wrapping_add(12).cast::<u16>().read_volatile() };
        // SAFETY: the next field of the same descriptor.
        let next = unsafe { descriptor.wrapping_add(14).cast::<u16>().read_volatile() };
        (DmaAddr::new(addr), len, flags, next)
    }

    /// Hands chain `head` back to the driver with `len` bytes written.
    pub fn put_used(&mut self, head: u16, len: u32) {
        let element = self.ring.used_element(self.used_idx);
        // SAFETY: the id field of a used ring element, which only the device writes.
        unsafe { element.write_volatile(head.into()) };
        // SAFETY: the length field of the same element.
        unsafe { element.wrapping_add(1).write_volatile(len) };
        self.used_idx = self.used_idx.wrapping_add(1);
        fence(Ordering::Release);
        self.ring
            .index(self.ring.used(2))
            .store(self.used_idx, Ordering::Release);
    }

    /// Whether the driver asked not to be notified of used buffers.
    pub fn used_notifications_disabled(&self) -> bool {
        // SAFETY: the available ring's flags field, which the driver writes.
        let flags = unsafe { self.ring.avail(0).read_volatile() };
        flags & AVAIL_NO_INTERRUPT != 0
    }

    /// Asks the driver not to notify (`on`) or to notify again.
    pub fn set_no_notify(&mut self, on: bool) {
        let flags = if on { USED_NO_NOTIFY } else { 0 };
        // SAFETY: the used ring's flags field, which only the device writes.
        unsafe { self.ring.used(0).write_volatile(flags) };
    }
}

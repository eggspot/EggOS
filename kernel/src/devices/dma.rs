//! DMA regions (D110): zeroed blocks of contiguous frames from the buddy allocator (D31) that a
//! device reads and writes, reached by the CPU through the direct map.
//!
//! The device address is the physical address for now ([`DmaAddr`] keeps the two apart, so an
//! IOMMU changes how regions are made, not the drivers), and only coherent devices are given
//! regions: nothing here cleans or invalidates caches. A region goes back to the allocator only
//! with the proof that its device was reset ([`DmaRegion::free_after_reset`]); one dropped
//! without it is leaked, since its device may still write it.

use crate::memory::FRAMES;
use eggos_drivers::dma::{DmaAddr, DmaMemory};
use eggos_drivers::virtio::ResetDone;
use eggos_hal::memory::FRAME_SIZE;

/// Why a region could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    /// No run of free contiguous frames that large, or no frame allocator.
    OutOfMemory,
    /// Larger than the buddy allocator's largest block.
    TooLarge,
}

/// Memory one device reaches by DMA: whole frames, zeroed when made.
#[derive(Debug)]
pub struct DmaRegion {
    first: u64,
    order: u32,
    virt: usize,
}

impl DmaRegion {
    /// A zeroed region of at least `bytes` bytes: the smallest power-of-two number of frames
    /// that holds them, aligned to its size.
    pub fn allocate(bytes: usize) -> Result<Self, DmaError> {
        let frames = (bytes as u64).div_ceil(FRAME_SIZE).max(1);
        let order = frames.next_power_of_two().ilog2();
        if order > crate::memory::MAX_ORDER {
            return Err(DmaError::TooLarge);
        }
        let mut allocator = FRAMES.lock();
        let allocator = allocator.as_mut().ok_or(DmaError::OutOfMemory)?;
        let first = allocator
            .allocate_contiguous(order)
            .ok_or(DmaError::OutOfMemory)?;
        let virt = (first as usize).wrapping_add(allocator.physical_memory_offset());
        let region = Self { first, order, virt };
        // SAFETY: the block was just allocated, so nothing else uses it, and the direct map
        // reaches all of it at `virt`.
        unsafe { eggos_hal::memory::zero(virt as *mut u8, region.len()) };
        Ok(region)
    }

    /// Gives the frames back; `_reset` proves the device that had the region no longer reaches
    /// it.
    pub fn free_after_reset(self, _reset: &ResetDone) {
        // SAFETY: the device was reset, so nothing but this region uses its frames.
        unsafe { self.free() }
    }

    /// Gives the frames back.
    ///
    /// # Safety
    /// No device may read or write the region any more: it was never given to one, or that
    /// device was reset (its driver's contract says which).
    pub unsafe fn free(self) {
        let Self { first, order, .. } = self;
        if let Some(allocator) = FRAMES.lock().as_mut() {
            // SAFETY: the block came from `allocate_contiguous` with this order and is freed
            // once, as `self` is gone; the caller guarantees no device uses it.
            unsafe { allocator.free_contiguous(first, order) };
        }
    }
}

// SAFETY: the frames are owned by the region from `allocate` until `free`, reached at `virt`
// through the direct map (normal, cached memory, aligned to at least a frame); the device reaches
// them at their physical address, which is the device address while there is no IOMMU; and only
// coherent devices are given regions.
unsafe impl DmaMemory for DmaRegion {
    fn as_mut_ptr(&self) -> *mut u8 {
        self.virt as *mut u8
    }
    fn dma_addr(&self) -> DmaAddr {
        DmaAddr::new(self.first)
    }
    fn len(&self) -> usize {
        (FRAME_SIZE as usize) << self.order
    }
}

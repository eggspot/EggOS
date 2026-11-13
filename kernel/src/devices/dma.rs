//! DMA regions (D110): zeroed blocks of contiguous frames from the buddy allocator (D31) that a
//! device reads and writes, reached by the CPU through the direct map.
//!
//! The device address is the physical address for now ([`DmaAddr`] keeps the two apart, so an
//! IOMMU changes how regions are made, not the drivers). A region for a device that does not
//! snoop the CPU's caches ([`DmaRegion::allocate_for`], D563) has its drivers write lines back
//! and drop them around the device's use. A region goes back to the allocator only
//! with the proof that its device was reset ([`DmaRegion::free_after_reset`]); one dropped
//! without it is leaked, since its device may still write it.

use crate::memory::FRAMES;
use crate::sync::LeafSpinLock;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use eggos_drivers::dma::{CacheMaintenance, DmaAddr, DmaMemory};
use eggos_drivers::virtio::ResetDone;
use eggos_hal::atomic::AtomicU64;
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
    coherent: bool,
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
        let (first, virt) = {
            let mut allocator = FRAMES.lock();
            let allocator = allocator.as_mut().ok_or(DmaError::OutOfMemory)?;
            let first = allocator
                .allocate_contiguous(order)
                .ok_or(DmaError::OutOfMemory)?;
            let virt = (first as usize).wrapping_add(allocator.physical_memory_offset());
            (first, virt)
        };
        let region = Self {
            first,
            order,
            virt,
            coherent: true,
        };
        // SAFETY: the block was just allocated, so nothing else uses it, and the direct map
        // reaches all of it at `virt`.
        unsafe { eggos_hal::memory::zero(virt as *mut u8, region.len()) };
        LIVE.lock().push((first, region.len() as u64));
        let translation = *TRANSLATION.lock();
        if let Some(translation) = translation {
            translation.map(first, region.len() as u64);
        }
        Ok(region)
    }

    /// A region as [`Self::allocate`] makes, for a device whose DMA is `coherence` (D563): a
    /// non-coherent device's region has the drivers write back and drop the CPU's cache lines
    /// ([`DmaMemory::cache_maintenance`]). `None` when the device is not coherent and this CPU
    /// cannot keep its caches in step with it.
    pub fn allocate_for(
        bytes: usize,
        coherence: super::DmaCoherence,
    ) -> Option<Result<Self, DmaError>> {
        let coherent = coherence == super::DmaCoherence::Coherent;
        if !coherent && !eggos_hal::cache::can_maintain_for_devices() {
            return None;
        }
        Some(Self::allocate(bytes).map(|region| Self { coherent, ..region }))
    }

    /// The physical address of its first frame, which the CPU maps it at (the device's address
    /// may differ, [`DmaMemory::dma_addr`]).
    pub fn physical(&self) -> u64 {
        self.first
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
        let length = self.len() as u64;
        let Self { first, order, .. } = self;
        LIVE.lock().retain(|&(start, _)| start != first);
        let translation = *TRANSLATION.lock();
        if let Some(translation) = translation {
            translation.unmap(first, length);
        }
        if let Some(allocator) = FRAMES.lock().as_mut() {
            // SAFETY: the block came from `allocate_contiguous` with this order and is freed
            // once, as `self` is gone; the caller guarantees no device uses it.
            unsafe { allocator.free_contiguous(first, order) };
        }
    }
}

static DEVICE_OFFSET: AtomicU64 = AtomicU64::new(0);

/// The regions given out and not freed: their first frame and length, which an IOMMU that
/// starts translating maps first (D563).
static LIVE: LeafSpinLock<Vec<(u64, u64)>> = LeafSpinLock::new_leaf(Vec::new());

static TRANSLATION: LeafSpinLock<Option<&'static dyn DmaTranslation>> =
    LeafSpinLock::new_leaf(None);

/// An IOMMU that maps every DMA region into the domain its devices are in (D563): a region is
/// reached at its physical address, as without one, but a device reaches nothing else.
pub trait DmaTranslation: Sync {
    /// Maps the `length` bytes at physical address `start` at the same device address.
    fn map(&self, start: u64, length: u64);
    /// Unmaps what [`Self::map`] mapped there.
    fn unmap(&self, start: u64, length: u64);
}

/// Has `translation` map every region from now on, mapping the regions already given out
/// first, so their devices keep working once their endpoints are attached.
pub fn translate_through(translation: &'static dyn DmaTranslation) {
    let live = LIVE.lock().clone();
    for (start, length) in live {
        translation.map(start, length);
    }
    *TRANSLATION.lock() = Some(translation);
}

/// Sets [`DEVICE_OFFSET`], once, when the PCI host that needs it is discovered.
pub(super) fn set_device_offset(offset: u64) {
    DEVICE_OFFSET.store(offset, Ordering::Relaxed);
}

// SAFETY: the frames are owned by the region from `allocate` until `free`, reached at `virt`
// through the direct map (normal, cached memory, aligned to at least a frame); the device reaches
// them at their physical address plus `DEVICE_OFFSET` (a window without translation: identity
// with no IOMMU, POWER's bypass window, D312); and a region made for a non-coherent device
// names the cache maintenance its drivers do (D563).
unsafe impl DmaMemory for DmaRegion {
    fn as_mut_ptr(&self) -> *mut u8 {
        self.virt as *mut u8
    }
    fn dma_addr(&self) -> DmaAddr {
        DmaAddr::new(self.first + DEVICE_OFFSET.load(Ordering::Relaxed))
    }
    fn len(&self) -> usize {
        (FRAME_SIZE as usize) << self.order
    }
    fn cache_maintenance(&self) -> Option<CacheMaintenance> {
        (!self.coherent).then_some(CacheMaintenance {
            write_back: eggos_hal::cache::write_back_for_device,
            discard: eggos_hal::cache::discard_for_cpu,
        })
    }
}

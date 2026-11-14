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

/// Memory one device reaches by DMA: whole frames, zeroed when made. A large one
/// ([`DmaRegion::allocate_mapped`], D580) is several blocks the CPU reaches contiguously
/// through a mapping of its own and the device piece by piece.
#[derive(Debug)]
pub struct DmaRegion {
    first: u64,
    order: u32,
    virt: usize,
    coherent: bool,
    more: Vec<(u64, u32)>,
}

fn allocate_block(order: u32) -> Result<(u64, usize), DmaError> {
    let mut allocator = FRAMES.lock();
    let allocator = allocator.as_mut().ok_or(DmaError::OutOfMemory)?;
    let first = allocator
        .allocate_contiguous(order)
        .ok_or(DmaError::OutOfMemory)?;
    Ok((first, (first as usize).wrapping_add(allocator.physical_memory_offset())))
}

fn free_block(first: u64, order: u32) {
    let length = (FRAME_SIZE as usize) << order;
    LIVE.lock().retain(|&(start, _)| start != first);
    let translation = *TRANSLATION.lock();
    if let Some(translation) = translation {
        translation.unmap(first, length as u64);
    }
    if let Some(allocator) = FRAMES.lock().as_mut() {
        // SAFETY: the block came from `allocate_contiguous` with this order and is freed once;
        // the caller guarantees no device uses it.
        unsafe { allocator.free_contiguous(first, order) };
    }
}

fn live_block(first: u64, virt: usize, order: u32) {
    let length = (FRAME_SIZE as usize) << order;
    // SAFETY: the block was just allocated, so nothing else uses it, and the direct map
    // reaches all of it at `virt`.
    unsafe { eggos_hal::memory::zero(virt as *mut u8, length) };
    LIVE.lock().push((first, length as u64));
    let translation = *TRANSLATION.lock();
    if let Some(translation) = translation {
        translation.map(first, length as u64);
    }
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
        let (first, virt) = allocate_block(order)?;
        live_block(first, virt, order);
        Ok(Self {
            first,
            order,
            virt,
            coherent: true,
            more: Vec::new(),
        })
    }

    /// A zeroed region of at least `bytes` bytes for a device that takes its memory in pieces
    /// (D580): one block as [`Self::allocate`] makes when one fits, else blocks of the largest
    /// size the allocator gives, which `map` (given each block's physical address and length,
    /// in order) maps contiguously for the CPU and returns the address of. Such a region is
    /// never given back to the allocator ([`Self::free`] keeps it), since its mapping is the
    /// kernel's for good.
    pub fn allocate_mapped(
        bytes: usize,
        map: impl FnOnce(&[(u64, u64)]) -> Option<usize>,
    ) -> Result<Self, DmaError> {
        let largest = (FRAME_SIZE as usize) << crate::memory::MAX_ORDER;
        if bytes <= largest {
            return Self::allocate(bytes);
        }
        let mut blocks: Vec<(u64, u32, usize)> = Vec::new();
        let mut covered = 0usize;
        while covered < bytes {
            let left = bytes - covered;
            let frames = (left as u64).div_ceil(FRAME_SIZE).max(1);
            let order = frames
                .next_power_of_two()
                .ilog2()
                .min(crate::memory::MAX_ORDER);
            match allocate_block(order) {
                Ok((first, virt)) => {
                    live_block(first, virt, order);
                    blocks.push((first, order, virt));
                    covered += (FRAME_SIZE as usize) << order;
                }
                Err(error) => {
                    for &(first, order, _) in &blocks {
                        free_block(first, order);
                    }
                    return Err(error);
                }
            }
        }
        let ranges: Vec<(u64, u64)> = blocks
            .iter()
            .map(|&(first, order, _)| (first, (FRAME_SIZE << order)))
            .collect();
        let Some(virt) = map(&ranges) else {
            for &(first, order, _) in &blocks {
                free_block(first, order);
            }
            return Err(DmaError::OutOfMemory);
        };
        let (first, order, _) = blocks[0];
        Ok(Self {
            first,
            order,
            virt,
            coherent: true,
            more: blocks[1..].iter().map(|&(first, order, _)| (first, order)).collect(),
        })
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
        if !self.more.is_empty() {
            return;
        }
        free_block(self.first, self.order);
    }

    /// Whether it is a large region of several blocks, which is kept rather than freed.
    pub fn is_mapped(&self) -> bool {
        !self.more.is_empty()
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
// through the direct map (normal, cached memory, aligned to at least a frame), or for a large
// region through its own mapping of its blocks in order, never unmapped; the device reaches
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
        self.more
            .iter()
            .fold((FRAME_SIZE as usize) << self.order, |total, &(_, order)| {
                total + ((FRAME_SIZE as usize) << order)
            })
    }
    fn each_piece(&self, piece: &mut dyn FnMut(DmaAddr, usize)) {
        let offset = DEVICE_OFFSET.load(Ordering::Relaxed);
        piece(DmaAddr::new(self.first + offset), (FRAME_SIZE as usize) << self.order);
        for &(first, order) in &self.more {
            piece(DmaAddr::new(first + offset), (FRAME_SIZE as usize) << order);
        }
    }
    fn cache_maintenance(&self) -> Option<CacheMaintenance> {
        (!self.coherent).then_some(CacheMaintenance {
            write_back: eggos_hal::cache::write_back_for_device,
            discard: eggos_hal::cache::discard_for_cpu,
        })
    }
}

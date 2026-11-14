//! Memory that devices read and write themselves (direct memory access).
//!
//! A device sees memory at its own addresses, [`DmaAddr`], which are not the CPU's virtual
//! addresses and need not be its physical ones: with an IOMMU they are whatever the IOMMU maps.
//! Drivers only ever hand devices `DmaAddr`s taken from a [`DmaMemory`], so a kernel that adds an
//! IOMMU changes how regions are made, not the drivers.

/// An address in a device's view of memory: what a driver writes into a descriptor or a
/// register for the device to read or write through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DmaAddr(u64);

impl DmaAddr {
    /// The device address `address`.
    pub const fn new(address: u64) -> Self {
        Self(address)
    }

    /// The address as the device takes it.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The address `bytes` further on, if it does not wrap.
    pub const fn checked_add(self, bytes: u64) -> Option<Self> {
        match self.0.checked_add(bytes) {
            Some(address) => Some(Self(address)),
            None => None,
        }
    }
}

/// A block of memory a device may read and write: the CPU reaches it at [`Self::as_mut_ptr`],
/// the device at [`Self::dma_addr`], both for [`Self::len`] bytes.
///
/// # Safety
/// Implementations guarantee that `as_mut_ptr()..as_mut_ptr() + len()` is memory the
/// implementation owns, valid for reads and writes (volatile and atomic ones included) and
/// aligned to at least 4096 bytes, for as long as the value lives; that the device reaches the
/// same bytes at `dma_addr()..dma_addr() + len()`; and that CPU and device see each other's
/// writes, without cache maintenance (coherent DMA) when [`Self::cache_maintenance`] is
/// `None`, or once a driver called [`Self::prepare_for_device`] and [`Self::prepare_for_cpu`]
/// around the device's use otherwise (D563).
pub unsafe trait DmaMemory {
    /// Where the CPU reaches the memory.
    fn as_mut_ptr(&self) -> *mut u8;
    /// Where the device reaches the memory.
    fn dma_addr(&self) -> DmaAddr;
    /// How many bytes it holds.
    fn len(&self) -> usize;
    /// Whether it holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The memory as the device sees it, piece by piece in order: each piece's device address
    /// and length (D580). One piece, [`Self::dma_addr`] and [`Self::len`], for memory that is
    /// contiguous to the device (the default); a large framebuffer made of several blocks
    /// that the CPU reaches contiguously has several.
    fn each_piece(&self, piece: &mut dyn FnMut(DmaAddr, usize)) {
        piece(self.dma_addr(), self.len());
    }
    /// The cache maintenance a device that does not snoop the CPU's caches needs, `None` for
    /// coherent memory (the default).
    fn cache_maintenance(&self) -> Option<CacheMaintenance> {
        None
    }
    /// Makes the CPU's writes to bytes `offset..offset + len` visible to the device: before
    /// handing them over. Nothing for coherent memory; a range past the end is cut.
    fn prepare_for_device(&self, offset: usize, len: usize) {
        if let Some(maintenance) = self.cache_maintenance() {
            let len = len.min(self.len().saturating_sub(offset));
            // SAFETY: the range lies inside the memory the implementation owns.
            unsafe { (maintenance.write_back)(self.as_mut_ptr().wrapping_add(offset), len) };
        }
    }
    /// Makes the device's writes to bytes `offset..offset + len` visible to the CPU: before
    /// handing a buffer the device will write over (so no dirty line lands on it later) and
    /// after it handed it back. Nothing for coherent memory; a range past the end is cut.
    fn prepare_for_cpu(&self, offset: usize, len: usize) {
        if let Some(maintenance) = self.cache_maintenance() {
            let len = len.min(self.len().saturating_sub(offset));
            // SAFETY: as in `prepare_for_device`.
            unsafe { (maintenance.discard)(self.as_mut_ptr().wrapping_add(offset), len) };
        }
    }
}

/// How the CPU's caches are kept in step with memory a device reaches without snooping them
/// (D563): the kernel's architecture code provides both.
#[derive(Debug, Clone, Copy)]
pub struct CacheMaintenance {
    /// Writes the lines holding the bytes back to memory.
    pub write_back: unsafe fn(*const u8, usize),
    /// Writes the lines holding the bytes back and drops them, so the CPU reads memory next.
    pub discard: unsafe fn(*const u8, usize),
}

/// Hands a driver blocks of DMA memory of the sizes it asks for, while it runs: a firmware
/// image's sections, each a block of its own.
pub trait DmaAllocator {
    /// The blocks it hands out.
    type Memory: DmaMemory;
    /// A zeroed block of at least `bytes`, aligned to 4096 bytes; `None` when there is no
    /// memory for it.
    fn allocate(&mut self, bytes: usize) -> Option<Self::Memory>;
}

#[cfg(test)]
pub(crate) mod fake {
    use super::{DmaAddr, DmaMemory};
    use std::alloc::{Layout, alloc_zeroed, dealloc};

    #[derive(Debug)]
    pub struct HostDma {
        memory: *mut u8,
        layout: Layout,
    }

    impl HostDma {
        pub fn zeroed(pages: usize) -> Self {
            let layout = Layout::from_size_align(pages * 4096, 4096).unwrap();
            // SAFETY: the layout has a non-zero size for every caller's `pages`.
            let memory = unsafe { alloc_zeroed(layout) };
            assert!(!memory.is_null());
            Self { memory, layout }
        }
    }

    impl Drop for HostDma {
        fn drop(&mut self) {
            // SAFETY: allocated in `zeroed` with this layout and freed only here.
            unsafe { dealloc(self.memory, self.layout) }
        }
    }

    // SAFETY: the memory is owned, 4096-aligned host memory, and a host test's "device" uses
    // the CPU's addresses, so the identity is its device address.
    unsafe impl DmaMemory for HostDma {
        fn as_mut_ptr(&self) -> *mut u8 {
            self.memory
        }
        fn dma_addr(&self) -> DmaAddr {
            DmaAddr::new(self.memory as u64)
        }
        fn len(&self) -> usize {
            self.layout.size()
        }
    }
}

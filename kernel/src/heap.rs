//! The kernel heap: `alloc` for the kernel (D30).
//!
//! Small allocations come from power-of-two size classes of 16 to 2048 bytes. Each class
//! carves 4 KiB frames into blocks of its size, reached through the direct map, so a block is
//! aligned to its size and a class needs no metadata beyond three words: the most recently
//! freed block (freed blocks are linked through their first word) and the never-used rest of
//! the frame being carved. Allocating and freeing a block are each a few instructions and take
//! no frame allocator call; a block never handed out is never touched. Larger allocations take
//! 2^order contiguous frames from the frame allocator (its buddy allocator, D31), up to
//! 2^[`MAX_ORDER`] frames (4 MiB); larger ones fail (the caller gets null, and `alloc`
//! panics).
//!
//! Frames a size class took stay with that class when its blocks are freed: giving empty slabs
//! back needs a count of live blocks per frame (D30 follow-up).
//!
//! Interrupt and exception handlers must not allocate: the heap's lock does not mask
//! interrupts (see [`LeafSpinLock`]), so a handler allocating while the code it interrupted holds
//! the lock would wait forever. Threads on other CPUs wait for it (D43).

use crate::memory::{self, FrameAllocator, MAX_ORDER};
use crate::sync::{LeafSpinLock, LeafSpinLockGuard};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use eggos_hal::memory::FRAME_SIZE;

/// The smallest block: room for the free-list link, and the alignment `alloc` callers most
/// often need.
const MIN_BLOCK: usize = 16;
/// The largest block a size class serves; larger allocations take whole frames.
pub const MAX_BLOCK: usize = 2048;
/// Size classes: 16, 32, ..., [`MAX_BLOCK`] bytes.
const CLASSES: usize = (MAX_BLOCK.trailing_zeros() - MIN_BLOCK.trailing_zeros() + 1) as usize;

/// The size class of sizes `16 * i + 1 ..= 16 * (i + 1)`, at index `i`: the smallest class
/// whose blocks, `16 << class` bytes, hold them. A table, because a power-of-two round-up costs
/// dozens of instructions on CPUs without bit-manipulation instructions (RISC-V without Zbb).
const CLASS_OF: [u8; MAX_BLOCK / MIN_BLOCK] = {
    let mut table = [0; MAX_BLOCK / MIN_BLOCK];
    let mut i = 0;
    while i < table.len() {
        table[i] = (i + 1).next_power_of_two().trailing_zeros() as u8;
        i += 1;
    }
    table
};

/// Marks an empty free list. Never a block address: blocks are at least 16-byte aligned.
const NO_BLOCK: usize = usize::MAX;

/// Where the heap gets frames, as the virtual addresses the kernel reaches them at.
pub trait FrameSource {
    /// The address of 2^`order` contiguous free frames, now owned by the heap; `None` when
    /// there are none.
    fn allocate_frames(&mut self, order: u32) -> Option<usize>;

    /// Gives back the 2^`order` frames at `address`.
    ///
    /// # Safety
    /// They must have come from one [`Self::allocate_frames`] call with this `order` on this
    /// source, not have been freed since, and nothing may use them any more.
    unsafe fn free_frames(&mut self, address: usize, order: u32);
}

/// A [`FrameAllocator`] the caller holds, as a [`FrameSource`].
#[derive(Debug)]
pub struct Frames<'a>(pub &'a mut FrameAllocator);

impl FrameSource for Frames<'_> {
    fn allocate_frames(&mut self, order: u32) -> Option<usize> {
        // Blocks are aligned physically; they are aligned virtually (as `Layout`s need) only if
        // the offset is, which every platform's direct map is, to far more than 4 MiB.
        debug_assert!(
            self.0
                .physical_memory_offset()
                .is_multiple_of((FRAME_SIZE as usize) << order),
            "the direct map is not aligned to the block size"
        );
        let first = self.0.allocate_contiguous(order)?;
        Some((first as usize).wrapping_add(self.0.physical_memory_offset()))
    }

    unsafe fn free_frames(&mut self, address: usize, order: u32) {
        let first = address.wrapping_sub(self.0.physical_memory_offset()) as u64;
        // SAFETY: the caller guarantees the frames came from this allocator (through
        // `allocate_frames`, so `allocate_contiguous` with this order) and are unused.
        unsafe { self.0.free_contiguous(first, order) };
    }
}

/// The kernel's frame allocator, [`memory::FRAMES`], locked for each call. Nothing is
/// allocated before [`crate::paging::enable`] puts the allocator there.
#[derive(Debug)]
pub struct GlobalFrames;

impl FrameSource for GlobalFrames {
    fn allocate_frames(&mut self, order: u32) -> Option<usize> {
        Frames(memory::FRAMES.lock().as_mut()?).allocate_frames(order)
    }

    unsafe fn free_frames(&mut self, address: usize, order: u32) {
        let mut frames = memory::FRAMES.lock();
        let Some(frames) = frames.as_mut() else {
            debug_assert!(false, "freeing frames before there is a frame allocator");
            return;
        };
        // SAFETY: the frames came from `allocate_frames` on this source with this order, which
        // took them from the same global allocator, as the caller guarantees.
        unsafe { Frames(frames).free_frames(address, order) };
    }
}

/// One size class's free blocks.
#[derive(Debug, Clone, Copy)]
struct Class {
    /// Virtual address of the most recently freed block, or [`NO_BLOCK`].
    free: usize,
    /// The never-used part of the frame being carved: `next..end` (empty when equal).
    next: usize,
    end: usize,
}

/// The heap's state. Each call is given the [`FrameSource`] it may take frames from, which
/// must be the same one every time.
#[derive(Debug)]
pub struct Heap {
    classes: [Class; CLASSES],
    slab_frames: u64,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

/// What serves a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The size class with this index.
    Block(usize),
    /// 2^order contiguous frames, aligned to their size.
    Frames(u32),
}

impl Kind {
    /// `None` when the layout needs more than 2^[`MAX_ORDER`] frames.
    #[inline]
    fn of(layout: Layout) -> Option<Self> {
        // At least 1: alignments are.
        let size = layout.size().max(layout.align());
        if size <= MAX_BLOCK {
            return Some(Kind::Block(usize::from(CLASS_OF[(size - 1) / MIN_BLOCK])));
        }
        let frames = (size as u64).div_ceil(FRAME_SIZE);
        let order = frames.next_power_of_two().trailing_zeros();
        (order <= MAX_ORDER).then_some(Kind::Frames(order))
    }
}

impl Heap {
    pub const fn new() -> Self {
        Self {
            classes: [Class {
                free: NO_BLOCK,
                next: 0,
                end: 0,
            }; CLASSES],
            slab_frames: 0,
        }
    }

    /// How many frames the size classes have taken to carve into blocks, which they keep for
    /// good (D30).
    pub fn slab_frames(&self) -> u64 {
        self.slab_frames
    }

    /// Memory for `layout`, or `None` when memory is exhausted or the layout is larger than
    /// 2^[`MAX_ORDER`] frames. A zero-sized layout gets a real block, so every pointer returned
    /// is unique.
    #[inline]
    pub fn allocate(
        &mut self,
        layout: Layout,
        frames: &mut impl FrameSource,
    ) -> Option<NonNull<u8>> {
        let address = match Kind::of(layout)? {
            Kind::Block(class) => self.allocate_block(class, frames)?,
            Kind::Frames(order) => frames.allocate_frames(order)?,
        };
        NonNull::new(address as *mut u8)
    }

    /// Gives back memory from [`Self::allocate`].
    ///
    /// # Safety
    /// `pointer` must have come from [`Self::allocate`] on this heap with this `layout` and the
    /// same `frames`, not have been given back since, and nothing may use it any more.
    #[inline]
    pub unsafe fn deallocate(
        &mut self,
        pointer: NonNull<u8>,
        layout: Layout,
        frames: &mut impl FrameSource,
    ) {
        let address = pointer.as_ptr() as usize;
        match Kind::of(layout) {
            Some(Kind::Block(class)) => {
                let class = &mut self.classes[class];
                // SAFETY: the caller gives the block up, so its first word (blocks are at
                // least 16 bytes and 16-byte aligned) is the heap's to write.
                unsafe { (address as *mut usize).write(class.free) };
                class.free = address;
            }
            // SAFETY: the caller gives the allocation up, and `allocate` took these frames from
            // `frames` with this order for this layout.
            Some(Kind::Frames(order)) => unsafe { frames.free_frames(address, order) },
            None => debug_assert!(false, "deallocating a layout the heap never serves"),
        }
    }

    /// Whether memory from [`Self::allocate`] for `old` also serves `new` as it is: both in the
    /// same size class, or the same number of frames.
    #[inline]
    pub fn fits_in_place(old: Layout, new: Layout) -> bool {
        Kind::of(old).is_some() && Kind::of(old) == Kind::of(new)
    }

    /// A block of size class `class`.
    #[inline]
    fn allocate_block(&mut self, class: usize, frames: &mut impl FrameSource) -> Option<usize> {
        let state = &mut self.classes[class];
        if state.free != NO_BLOCK {
            let block = state.free;
            // SAFETY: `block` is on the free list, so `deallocate` wrote the next link into its
            // first word, which nothing else owns; it is aligned for a `usize`.
            state.free = unsafe { (block as *const usize).read() };
            return Some(block);
        }
        if state.next == state.end {
            Self::refill(state, &mut self.slab_frames, frames)?;
        }
        let block = state.next;
        state.next += MIN_BLOCK << class;
        Some(block)
    }

    /// Gives a size class a new frame to carve; `None` when there is none. Out of line, so
    /// the block path stays short.
    #[cold]
    #[inline(never)]
    fn refill(
        class: &mut Class,
        slab_frames: &mut u64,
        frames: &mut impl FrameSource,
    ) -> Option<()> {
        let frame = frames.allocate_frames(0)?;
        *slab_frames += 1;
        class.next = frame;
        class.end = frame + FRAME_SIZE as usize;
        Some(())
    }
}

/// The kernel's `alloc` allocator: a [`Heap`] behind a lock, taking frames from the global
/// frame allocator. Lock order: this heap's lock, then [`memory::FRAMES`]; never the reverse.
#[derive(Debug)]
pub struct KernelHeap {
    heap: LeafSpinLock<Heap>,
}

impl Default for KernelHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            heap: LeafSpinLock::new_leaf(Heap::new()),
        }
    }

    /// The heap, locked, once the CPU using it (or the host thread, in tests) is done. Every
    /// CPU allocates since threads run on all of them (D43), so a held lock no longer means an
    /// interrupt handler broke the rule in the module documentation, as it did on one CPU.
    #[inline]
    fn lock(&self) -> LeafSpinLockGuard<'_, Heap> {
        self.heap.lock()
    }

    /// The frame allocator's free frames plus [`Heap::slab_frames`], read together; `None`
    /// before there is a frame allocator. A size class taking a frame leaves the sum as it is,
    /// so code that gives back every frame it took leaves it unchanged even if a class grew.
    pub fn free_and_slab_frames(&self) -> Option<u64> {
        let heap = self.lock();
        let free = memory::FRAMES.lock().as_ref()?.free_frames();
        Some(free + heap.slab_frames())
    }
}

// SAFETY: `alloc` returns memory from `Heap::allocate`, which is at least `layout.size()`
// bytes aligned to `layout.align()` (a block of a size class at least that large, aligned to
// its size; or 2^order frames aligned to their size, at least `max(size, align)`, through a
// direct map aligned to more than that), owned by the caller until `dealloc`, or null.
// `dealloc` gives it back with the same layout, as `GlobalAlloc` callers guarantee. The lock
// serialises every use of the heap state, and the heap only ever takes frames from the global
// frame allocator, under its lock.
unsafe impl GlobalAlloc for KernelHeap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.lock()
            .allocate(layout, &mut GlobalFrames)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    #[inline]
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let Some(pointer) = NonNull::new(pointer) else {
            return;
        };
        // SAFETY: `GlobalAlloc` callers give back memory `alloc` returned for `layout`, which
        // came from this heap with `GlobalFrames`, and do not use it any more.
        unsafe { self.lock().deallocate(pointer, layout, &mut GlobalFrames) };
    }

    #[inline]
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // `GlobalAlloc` callers pass a `new_size` that is valid with `layout`'s alignment.
        let Ok(new) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };
        if Heap::fits_in_place(layout, new) {
            return pointer;
        }
        // SAFETY: `new` has a non-zero size (the caller's `new_size` is non-zero) and valid
        // alignment.
        let moved = unsafe { self.alloc(new) };
        if !moved.is_null() {
            // SAFETY: both blocks are at least `min(old, new)` bytes long, `pointer` is the
            // caller's live allocation and `moved` a fresh one, so they do not overlap.
            unsafe { ptr::copy_nonoverlapping(pointer, moved, layout.size().min(new_size)) };
            // SAFETY: `pointer` came from `alloc` with `layout` and its contents were copied;
            // the caller stops using it once `realloc` succeeds.
            unsafe { self.dealloc(pointer, layout) };
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_hal::{MemoryMap, PhysicalRange};

    const K: u64 = FRAME_SIZE;

    /// Host memory standing in for `count` frames of RAM at physical 0x10_0000, and an
    /// allocator over them.
    /// The physical memory offset is aligned to the largest buddy block, as the real direct
    /// maps are, so blocks aligned physically are aligned virtually.
    fn frames(count: u64) -> FrameAllocator {
        const BASE: u64 = 0x40_0000;
        let block = (K << MAX_ORDER) as usize;
        let memory = vec![0u8; (count * K) as usize + block].leak();
        let start = (memory.as_mut_ptr() as usize).next_multiple_of(block);
        let mut map = MemoryMap::new();
        map.add_ram(PhysicalRange::from_start_length(BASE, count * K));
        FrameAllocator::new(&map, start.wrapping_sub(BASE as usize))
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn layouts_pick_the_smallest_class_that_fits_size_and_alignment() {
        let class = |size, align| match Kind::of(layout(size, align)) {
            Some(Kind::Block(class)) => Some(MIN_BLOCK << class),
            _ => None,
        };
        assert_eq!(class(0, 1), Some(16));
        assert_eq!(class(1, 1), Some(16));
        assert_eq!(class(17, 8), Some(32));
        assert_eq!(class(24, 64), Some(64));
        assert_eq!(class(2048, 8), Some(2048));
        assert_eq!(class(8, 2048), Some(2048));
        assert_eq!(class(2049, 8), None);
        assert_eq!(Kind::of(layout(2049, 8)), Some(Kind::Frames(0)));
        assert_eq!(Kind::of(layout(8, 4096)), Some(Kind::Frames(0)));
        assert_eq!(Kind::of(layout(4096, 4096)), Some(Kind::Frames(0)));
        assert_eq!(Kind::of(layout(4097, 8)), Some(Kind::Frames(1)));
        assert_eq!(Kind::of(layout(8, 8192)), Some(Kind::Frames(1)));
        assert_eq!(Kind::of(layout(3 * 4096, 8)), Some(Kind::Frames(2)));
        assert_eq!(Kind::of(layout(4 << 20, 8)), Some(Kind::Frames(MAX_ORDER)));
        assert_eq!(Kind::of(layout((4 << 20) + 1, 8)), None);
    }

    #[test]
    fn blocks_are_aligned_distinct_and_reused_last_in_first_out() {
        let mut allocator = frames(64);
        let mut frames = Frames(&mut allocator);
        let mut heap = Heap::new();
        let mut got = Vec::new();
        for size in [1, 16, 17, 100, 1000, 2048] {
            for _ in 0..40 {
                let l = layout(size, 1);
                let p = heap.allocate(l, &mut frames).unwrap();
                let block = size.max(MIN_BLOCK).next_power_of_two();
                assert_eq!(p.as_ptr() as usize % block, 0, "size {size}");
                // SAFETY: the block was just allocated with at least `size` bytes.
                unsafe { p.as_ptr().write_bytes(0xa5, size) };
                got.push((p, l));
            }
        }
        let mut addresses: Vec<usize> = got.iter().map(|(p, _)| p.as_ptr() as usize).collect();
        addresses.sort_unstable();
        addresses.dedup();
        assert_eq!(addresses.len(), got.len(), "a block was handed out twice");
        let (last, l) = got.pop().unwrap();
        let (before, l2) = got.pop().unwrap();
        // SAFETY: both came from this heap with these layouts and are not used any more.
        unsafe { heap.deallocate(before, l2, &mut frames) };
        // SAFETY: as above.
        unsafe { heap.deallocate(last, l, &mut frames) };
        assert_eq!(heap.allocate(l, &mut frames), Some(last));
        assert_eq!(heap.allocate(l2, &mut frames), Some(before));
    }

    #[test]
    fn a_class_takes_a_frame_only_when_its_current_one_is_used_up() {
        let mut allocator = frames(4);
        let mut frames = Frames(&mut allocator);
        let mut heap = Heap::new();
        let free = frames.0.free_frames();
        let l = layout(64, 8);
        for _ in 0..K as usize / 64 {
            heap.allocate(l, &mut frames).unwrap();
        }
        assert_eq!(frames.0.free_frames(), free - 1);
        assert_eq!(heap.slab_frames(), 1);
        heap.allocate(l, &mut frames).unwrap();
        assert_eq!(frames.0.free_frames(), free - 2);
        assert_eq!(heap.slab_frames(), 2);
    }

    #[test]
    fn free_and_slab_frames_count_a_class_s_new_frame_but_not_a_frame_allocation() {
        let _frames = crate::thread::host::frames_for_test();
        let heap = KernelHeap::new();
        let before = heap.free_and_slab_frames().unwrap();
        let small = layout(64, 8);
        let page = layout(4096, 4096);
        // SAFETY: the layout has a non-zero size.
        let block = unsafe { heap.alloc(small) };
        // SAFETY: as above.
        let frame = unsafe { heap.alloc(page) };
        assert!(!block.is_null() && !frame.is_null());
        assert_eq!(heap.lock().slab_frames(), 1);
        assert_eq!(heap.free_and_slab_frames(), Some(before - 1));
        // SAFETY: it came from this heap with this layout and is not used any more.
        unsafe { heap.dealloc(frame, page) };
        // SAFETY: as above.
        unsafe { heap.dealloc(block, small) };
        assert_eq!(heap.free_and_slab_frames(), Some(before));
    }

    #[test]
    fn realloc_stays_in_place_only_within_a_class_or_a_frame() {
        assert!(Heap::fits_in_place(layout(20, 8), layout(32, 8)));
        assert!(!Heap::fits_in_place(layout(32, 8), layout(33, 8)));
        assert!(!Heap::fits_in_place(layout(32, 8), layout(8, 8)));
        assert!(Heap::fits_in_place(layout(2049, 8), layout(4096, 8)));
        assert!(!Heap::fits_in_place(layout(4096, 8), layout(4097, 8)));
        assert!(Heap::fits_in_place(layout(5000, 8), layout(8192, 8)));
        assert!(!Heap::fits_in_place(layout(8192, 8), layout(8193, 8)));
        assert!(!Heap::fits_in_place(layout(8 << 20, 8), layout(8 << 20, 8)));
    }

    #[test]
    fn frame_sized_allocations_go_back_to_the_frame_allocator() {
        let mut allocator = frames(8);
        let mut frames = Frames(&mut allocator);
        let mut heap = Heap::new();
        let free = frames.0.free_frames();
        let l = layout(3000, 8);
        let p = heap.allocate(l, &mut frames).unwrap();
        assert_eq!(p.as_ptr() as usize % K as usize, 0);
        assert_eq!(frames.0.free_frames(), free - 1);
        // SAFETY: from this heap with this layout, not used any more.
        unsafe { heap.deallocate(p, l, &mut frames) };
        assert_eq!(frames.0.free_frames(), free);
    }

    #[test]
    fn multi_frame_allocations_are_contiguous_aligned_and_come_back() {
        let mut allocator = frames(64);
        let mut frames = Frames(&mut allocator);
        let mut heap = Heap::new();
        let free = frames.0.free_frames();
        let l = layout(3 * K as usize, 8);
        let p = heap.allocate(l, &mut frames).unwrap();
        assert_eq!(
            p.as_ptr() as usize % (4 * K as usize),
            0,
            "aligned to its 4 frames"
        );
        // SAFETY: the allocation is 3 frames long and unused.
        unsafe { p.as_ptr().write_bytes(0x5a, 3 * K as usize) };
        assert_eq!(frames.0.free_frames(), free - 4);
        // SAFETY: from this heap with this layout, not used any more.
        unsafe { heap.deallocate(p, l, &mut frames) };
        assert_eq!(frames.0.free_frames(), free);
        assert_eq!(
            heap.allocate(l, &mut frames),
            Some(p),
            "the freed block is reused"
        );
    }

    #[test]
    fn exhausted_memory_and_layouts_larger_than_the_largest_block_return_none() {
        // One of the 4 frames holds the buddy allocator's metadata.
        let mut allocator = frames(4);
        let mut frames = Frames(&mut allocator);
        let mut heap = Heap::new();
        assert!(
            heap.allocate(layout((4 << 20) + 1, 8), &mut frames)
                .is_none()
        );
        assert_eq!(frames.0.free_frames(), 3);
        for _ in 0..3 {
            heap.allocate(layout(4096, 8), &mut frames).unwrap();
        }
        assert!(heap.allocate(layout(2048, 8), &mut frames).is_none());
        assert!(heap.allocate(layout(16, 8), &mut frames).is_none());
        assert!(heap.allocate(layout(8192, 8), &mut frames).is_none());
    }
}

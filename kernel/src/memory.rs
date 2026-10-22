//! Physical memory: the frame allocator.
//!
//! Single frames come from a bump allocator over the free ranges of the boot memory map plus a
//! stack of freed frames (D28). Building it costs nothing per frame, so boot time does not grow
//! with RAM size; allocating and freeing a frame are each a handful of instructions. A frame
//! never handed out is never touched. Freed frames are linked through their own first word,
//! which the kernel reaches through the platform's physical memory offset. Blocks of
//! contiguous frames (heap allocations over a frame, DMA, huge pages) come from a buddy
//! allocator behind those fast paths, whose one bit of metadata per frame is written only once
//! it is first used (D31).

use crate::paging::PageTables;
use crate::sync::SpinLock;
use eggos_hal::memory::{FRAME_SIZE, MAX_RANGES};
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// The kernel's memory once its page tables are active: the tables, reaching physical memory
/// through the direct map (the frame allocator is [`FRAMES`]).
#[derive(Debug)]
pub struct Memory<P: Platform> {
    pub page_tables: PageTables<P::PageTableFormat>,
}

/// The kernel's frame allocator once its page tables are active: [`crate::paging::enable`]
/// puts it here, and the kernel heap and everything else that needs frames take them from here.
/// Lock order: the heap's lock ([`crate::heap::KernelHeap`]) before this one, never the reverse.
pub static FRAMES: SpinLock<Option<FrameAllocator>> = SpinLock::new(None);

/// Marks the end of the free stack. Never a frame address: frames are aligned to
/// [`FRAME_SIZE`].
const NO_FRAME: u64 = u64::MAX;

/// Hands out and takes back 4 KiB physical frames.
#[derive(Debug)]
pub struct FrameAllocator {
    /// The never-allocated range being consumed, from the top down. Empty only when all
    /// untouched memory is gone.
    current: PhysicalRange,
    /// The other never-allocated ranges, in ascending order; the highest is consumed next, so
    /// low memory (which some devices' DMA is limited to) goes last.
    untouched: [PhysicalRange; MAX_RANGES],
    untouched_ranges: usize,
    /// Physical address of the most recently freed frame, or [`NO_FRAME`].
    free_stack: u64,
    /// Frames that [`Self::allocate`] can still return.
    free_frames: u64,
    physical_memory_offset: usize,
    /// Free blocks of contiguous frames (D31).
    buddy: Buddy,
}

impl FrameAllocator {
    /// An allocator owning every frame in `map`, which the kernel reaches at physical address +
    /// `physical_memory_offset`.
    pub fn new(map: &MemoryMap, physical_memory_offset: usize) -> Self {
        let (current, rest) = match map.ranges() {
            [rest @ .., last] => (*last, rest),
            [] => (PhysicalRange::default(), &[][..]),
        };
        let mut untouched = [PhysicalRange::default(); MAX_RANGES];
        untouched[..rest.len()].copy_from_slice(rest);
        let mut allocator = Self {
            current,
            untouched,
            untouched_ranges: rest.len(),
            free_stack: NO_FRAME,
            free_frames: map.total() / FRAME_SIZE,
            physical_memory_offset,
            buddy: Buddy::new(PhysicalRange::default(), 0),
        };
        // The buddy allocator's bitmap covers every frame from the lowest to the highest one
        // in the map. It is taken from the top of the highest range that keeps a frame besides
        // it, but not written until the buddy allocator is first used; without such a range,
        // there is no buddy allocator (only single frames).
        if let ([first, ..], [.., last]) = (map.ranges(), map.ranges()) {
            let span = PhysicalRange {
                start: first.start,
                end: last.end,
            };
            let bytes = Buddy::bitmap_bytes(span);
            if let Some(bitmap) = allocator.take_from_untouched(bytes) {
                allocator.buddy = Buddy::new(span, bitmap);
                allocator.free_frames -= bytes / FRAME_SIZE;
            }
        }
        allocator
    }

    /// Takes `bytes` (whole frames) from the top of the highest untouched range longer than
    /// that, before anything is allocated.
    fn take_from_untouched(&mut self, bytes: u64) -> Option<u64> {
        if self.current.len() > bytes {
            self.current.end -= bytes;
            return Some(self.current.end);
        }
        let range = self.untouched[..self.untouched_ranges]
            .iter_mut()
            .rev()
            .find(|range| range.len() > bytes)?;
        range.end -= bytes;
        Some(range.end)
    }

    /// Where this allocator reaches physical memory: frame `p` at virtual `p + offset`.
    pub fn physical_memory_offset(&self) -> usize {
        self.physical_memory_offset
    }

    /// Frames that [`Self::allocate`] can still return.
    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    /// The physical address of a free frame, now owned by the caller; `None` when memory is
    /// exhausted. The frame's contents are undefined.
    #[inline]
    pub fn allocate(&mut self) -> Option<u64> {
        if self.free_stack != NO_FRAME {
            let frame = self.free_stack;
            // SAFETY: `frame` is on the free stack, so `free` wrote the next link into its
            // first word, which nothing else owns; it is reached through the physical memory
            // offset and is aligned for a `u64` (frames are 4 KiB aligned).
            self.free_stack = unsafe { self.link(frame).read() };
            self.free_frames -= 1;
            return Some(frame);
        }
        if self.current.is_empty() {
            self.refill_current()?;
        }
        self.current.end -= FRAME_SIZE;
        self.free_frames -= 1;
        Some(self.current.end)
    }

    /// Gives [`Self::allocate`] a non-empty range to take from once the free stack and the
    /// untouched range being consumed are both empty: the next untouched range, else a frame
    /// from the buddy allocator (as a one-frame range, so the fast path stays as it is);
    /// `None` when memory is exhausted.
    #[cold]
    fn refill_current(&mut self) -> Option<()> {
        if let Some(below) = self.untouched_ranges.checked_sub(1) {
            self.untouched_ranges = below;
            self.current = self.untouched[below];
        } else {
            let frame = self.allocate_from_buddy(0)?;
            self.current = PhysicalRange::from_start_length(frame, FRAME_SIZE);
        }
        Some(())
    }

    /// The physical address of the first of 2^`order` contiguous free frames, aligned to their
    /// size, now owned by the caller (who gives them back with [`Self::free_contiguous`]);
    /// `None` when there is no such run or `order` exceeds [`MAX_ORDER`]. The contents are
    /// undefined.
    ///
    /// Not a hot path: it tries the buddy allocator's free blocks, then carves the block from
    /// untouched memory, then moves frames from the free stack into the buddy allocator, where
    /// freed neighbours merge, until a block of `order` forms.
    pub fn allocate_contiguous(&mut self, order: u32) -> Option<u64> {
        if order == 0 {
            return self.allocate();
        }
        if order > MAX_ORDER || !self.buddy.exists() {
            return None;
        }
        let block = match self.allocate_from_buddy(order) {
            Some(block) => block,
            None => match self.carve_untouched(order) {
                Some(block) => block,
                None => {
                    self.drain_free_stack(order);
                    self.allocate_from_buddy(order)?
                }
            },
        };
        self.free_frames -= 1 << order;
        Some(block)
    }

    /// Gives back 2^`order` frames starting at `first`.
    ///
    /// # Safety
    /// They must have come from one [`Self::allocate_contiguous`] call with this `order` on
    /// this allocator, not have been freed since, and nothing may use them any more.
    pub unsafe fn free_contiguous(&mut self, first: u64, order: u32) {
        if order == 0 {
            // SAFETY: an order-0 block is a frame from `allocate`, as the caller guarantees.
            return unsafe { self.free(first) };
        }
        debug_assert!(
            first.is_multiple_of(FRAME_SIZE << order),
            "freeing an unaligned block"
        );
        self.buddy_memory().insert(first, order);
        self.free_frames += 1 << order;
    }

    /// A block of 2^`order` frames from the buddy allocator's free lists, splitting a larger
    /// one if needed. Does not count it in [`Self::free_frames`].
    fn allocate_from_buddy(&mut self, order: u32) -> Option<u64> {
        if !self.buddy.exists() {
            return None;
        }
        self.buddy_memory().allocate(order)
    }

    /// Takes an aligned block of 2^`order` frames from the top of an untouched range, the
    /// highest that has one; the frames above it in that range go to the buddy allocator. Does
    /// not count the block in [`Self::free_frames`].
    fn carve_untouched(&mut self, order: u32) -> Option<u64> {
        let size = FRAME_SIZE << order;
        let fits = |range: &PhysicalRange| {
            let start = range.end.checked_sub(size)? / size * size;
            (start >= range.start).then_some(start)
        };
        // `None` is `current`, `Some(i)` the untouched range `i`; highest first.
        let (index, block) = core::iter::once(None)
            .chain((0..self.untouched_ranges).rev().map(Some))
            .find_map(|index| {
                let range = index.map_or(&self.current, |i| &self.untouched[i]);
                Some((index, fits(range)?))
            })?;
        let range = match index {
            None => &mut self.current,
            Some(i) => &mut self.untouched[i],
        };
        let above = PhysicalRange {
            start: block + size,
            end: range.end,
        };
        range.end = block;
        if let Some(i) = index
            && self.untouched[i].is_empty()
        {
            // Only `current` may be empty: `allocate` moves on to the next range without
            // checking it.
            self.untouched.copy_within(i + 1..self.untouched_ranges, i);
            self.untouched_ranges -= 1;
        }
        let mut buddy = self.buddy_memory();
        let mut start = above.start;
        while start < above.end {
            // The largest block that starts here and fits.
            let order = (start.trailing_zeros() - FRAME_SIZE.trailing_zeros())
                .min(MAX_ORDER)
                .min((above.end - start).ilog2() - FRAME_SIZE.trailing_zeros());
            buddy.insert(start, order);
            start += FRAME_SIZE << order;
        }
        Some(block)
    }

    /// Moves frames from the free stack into the buddy allocator, where freed neighbours merge
    /// into larger blocks, until a block of `order` forms or the stack is empty.
    fn drain_free_stack(&mut self, order: u32) {
        while self.free_stack != NO_FRAME {
            let frame = self.free_stack;
            // SAFETY: as in `allocate`: `frame` is on the free stack, its first word the link.
            self.free_stack = unsafe { self.link(frame).read() };
            if self.buddy_memory().insert(frame, 0) >= order {
                return;
            }
        }
    }

    /// The buddy allocator with access to the memory it keeps its lists and metadata in.
    fn buddy_memory(&mut self) -> BuddyMemory<'_> {
        BuddyMemory {
            buddy: &mut self.buddy,
            offset: self.physical_memory_offset,
        }
    }

    /// Gives `frame` back.
    ///
    /// # Safety
    /// `frame` must have come from [`Self::allocate`] on this allocator and not been freed
    /// since, and nothing may use the frame any more.
    #[inline]
    pub unsafe fn free(&mut self, frame: u64) {
        debug_assert!(
            frame.is_multiple_of(FRAME_SIZE),
            "freeing an unaligned frame"
        );
        // SAFETY: the caller gives up the frame, which this allocator handed out, so its first
        // word is ours to write; it is reached through the physical memory offset and aligned.
        unsafe { self.link(frame).write(self.free_stack) };
        self.free_stack = frame;
        self.free_frames += 1;
    }

    /// Where the kernel reaches the first word of `frame`, the free-stack link.
    fn link(&self, frame: u64) -> *mut u64 {
        (frame as usize).wrapping_add(self.physical_memory_offset) as *mut u64
    }
}

/// Largest block the buddy allocator manages: 2^`MAX_ORDER` frames (4 MiB).
pub const MAX_ORDER: u32 = 10;

/// The buddy allocator's state (D31): free blocks of 2^order contiguous frames, each aligned
/// to its size, in one doubly linked list per order. A free block's first three words hold its
/// links (next, previous) and its order. One bit per frame of the span says whether the frame
/// starts a free block, so freeing a block finds out in one load whether its buddy is free to
/// merge with (and then reads the buddy's order from the buddy itself).
#[derive(Debug)]
struct Buddy {
    /// The frames the bitmap covers.
    span: PhysicalRange,
    /// Physical address of the bitmap, one bit per frame of `span`.
    bitmap: u64,
    /// Whether the bitmap has been zeroed yet; it is, on the buddy allocator's first use.
    zeroed: bool,
    /// The first free block of each order, or [`NO_FRAME`].
    heads: [u64; MAX_ORDER as usize + 1],
}

/// Words at the start of a free buddy block.
const NEXT: usize = 0;
const PREVIOUS: usize = 1;
const ORDER: usize = 2;

impl Buddy {
    /// A buddy allocator over `span` with its bitmap at `bitmap`; none for an empty span.
    const fn new(span: PhysicalRange, bitmap: u64) -> Self {
        Self {
            span,
            bitmap,
            zeroed: false,
            heads: [NO_FRAME; MAX_ORDER as usize + 1],
        }
    }

    const fn exists(&self) -> bool {
        !self.span.is_empty()
    }

    /// Bytes the bitmap for `span` takes, in whole frames.
    const fn bitmap_bytes(span: PhysicalRange) -> u64 {
        (span.len() / FRAME_SIZE)
            .div_ceil(8)
            .next_multiple_of(FRAME_SIZE)
    }
}

/// A [`Buddy`] with the physical memory offset that reaches its blocks and bitmap.
struct BuddyMemory<'a> {
    buddy: &'a mut Buddy,
    offset: usize,
}

impl BuddyMemory<'_> {
    /// Removes and returns a block of 2^`order` frames, splitting a larger free block if
    /// needed (its upper halves stay free).
    fn allocate(&mut self, order: u32) -> Option<u64> {
        self.zero_bitmap();
        let mut found = (order..=MAX_ORDER).find(|&o| self.buddy.heads[o as usize] != NO_FRAME)?;
        let block = self.buddy.heads[found as usize];
        self.unlink(block, found);
        while found > order {
            found -= 1;
            self.push(block + (FRAME_SIZE << found), found);
        }
        Some(block)
    }

    /// Adds the free block of 2^`order` frames at `block`, merging it with its buddy as long as
    /// that is free too. Returns the order of the block it ends up in.
    fn insert(&mut self, mut block: u64, mut order: u32) -> u32 {
        self.zero_bitmap();
        while order < MAX_ORDER {
            let buddy = block ^ (FRAME_SIZE << order);
            if self.free_block_order(buddy) != Some(order) {
                break;
            }
            self.unlink(buddy, order);
            block = block.min(buddy);
            order += 1;
        }
        self.push(block, order);
        order
    }

    /// The order of the free block starting at `frame`, if one does.
    fn free_block_order(&self, frame: u64) -> Option<u32> {
        if frame < self.buddy.span.start || frame >= self.buddy.span.end {
            return None;
        }
        let (byte, bit) = self.bitmap_bit(frame);
        // SAFETY: `frame` is in the span, so its bit is inside the bitmap, which the allocator
        // owns and zeroed before any use.
        if unsafe { byte.read() } & bit == 0 {
            return None;
        }
        // SAFETY: the bit says a free block starts at `frame`, whose third word `push` set to
        // its order.
        Some(unsafe { self.word(frame, ORDER).read() } as u32)
    }

    /// Pushes the free block at `block` onto the list of `order`.
    fn push(&mut self, block: u64, order: u32) {
        let head = &mut self.buddy.heads[order as usize];
        let next = *head;
        *head = block;
        // SAFETY: `block` is free and owned by the allocator, reached through the physical
        // memory offset; its first three words are its header.
        unsafe { self.word(block, NEXT).write(next) };
        // SAFETY: as above.
        unsafe { self.word(block, PREVIOUS).write(NO_FRAME) };
        // SAFETY: as above.
        unsafe { self.word(block, ORDER).write(u64::from(order)) };
        if next != NO_FRAME {
            // SAFETY: `next` was the list's head, a free block with a header.
            unsafe { self.word(next, PREVIOUS).write(block) };
        }
        self.set_free_bit(block, true);
    }

    /// Takes the free block at `block` off the list of `order`.
    fn unlink(&mut self, block: u64, order: u32) {
        // SAFETY: `block` is a free block on the list, so `push` wrote its header.
        let next = unsafe { self.word(block, NEXT).read() };
        // SAFETY: as above.
        let previous = unsafe { self.word(block, PREVIOUS).read() };
        if previous == NO_FRAME {
            self.buddy.heads[order as usize] = next;
        } else {
            // SAFETY: `previous` is the block before it on the list, a free block.
            unsafe { self.word(previous, NEXT).write(next) };
        }
        if next != NO_FRAME {
            // SAFETY: `next` is the block after it on the list, a free block.
            unsafe { self.word(next, PREVIOUS).write(previous) };
        }
        self.set_free_bit(block, false);
    }

    fn set_free_bit(&mut self, frame: u64, free: bool) {
        let (byte, bit) = self.bitmap_bit(frame);
        // SAFETY: only blocks inside the span are pushed or unlinked, so the byte is inside the
        // bitmap, which the allocator owns.
        let value = unsafe { byte.read() };
        let value = if free { value | bit } else { value & !bit };
        // SAFETY: as above.
        unsafe { byte.write(value) };
    }

    /// Zeroes the bitmap, the first time the buddy allocator is used.
    fn zero_bitmap(&mut self) {
        if !self.buddy.zeroed {
            let bytes = (self.buddy.span.len() / FRAME_SIZE).div_ceil(8);
            let bitmap = self.virt(self.buddy.bitmap) as *mut u8;
            // SAFETY: the bitmap frames were taken out of the free memory for this, are reached
            // through the physical memory offset and hold one bit per frame of the span.
            unsafe { bitmap.write_bytes(0, bytes as usize) };
            self.buddy.zeroed = true;
        }
    }

    /// The bitmap byte holding `frame`'s bit, and the bit.
    fn bitmap_bit(&self, frame: u64) -> (*mut u8, u8) {
        let index = (frame - self.buddy.span.start) / FRAME_SIZE;
        let byte = self.virt(self.buddy.bitmap + index / 8) as *mut u8;
        (byte, 1 << (index % 8))
    }

    /// Word `index` of the block at physical `block`.
    fn word(&self, block: u64, index: usize) -> *mut u64 {
        (self.virt(block) as *mut u64).wrapping_add(index)
    }

    fn virt(&self, physical: u64) -> usize {
        (physical as usize).wrapping_add(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: u64 = FRAME_SIZE;

    /// RAM for host tests: `frames` frames of real memory, placed at physical `base`.
    struct Arena {
        memory: Vec<u8>,
        base: u64,
        frames: u64,
    }

    impl Arena {
        fn new(base: u64, frames: u64) -> Self {
            // One extra frame, so an aligned run of `frames` frames fits.
            let memory = vec![0u8; ((frames + 1) * K) as usize];
            Self {
                memory,
                base,
                frames,
            }
        }
        fn aligned_start(&mut self) -> usize {
            (self.memory.as_mut_ptr() as usize).next_multiple_of(K as usize)
        }
        fn offset(&mut self) -> usize {
            self.aligned_start().wrapping_sub(self.base as usize)
        }
        fn map(&self, ranges: &[(u64, u64)]) -> MemoryMap {
            let mut map = MemoryMap::new();
            for &(start, end) in ranges {
                assert!(start >= self.base && end <= self.base + self.frames * K);
                map.add_ram(PhysicalRange { start, end });
            }
            map
        }
    }

    #[test]
    fn hands_out_every_frame_once_top_down_then_runs_out() {
        let mut arena = Arena::new(0x10_0000, 16);
        let b = arena.base;
        let map = arena.map(&[(b, b + 4 * K), (b + 8 * K, b + 10 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        // The top frame holds the buddy allocator's bitmap.
        assert_eq!(frames.free_frames(), 5);
        let got: Vec<u64> = core::iter::from_fn(|| frames.allocate()).collect();
        assert_eq!(
            got,
            [8, 3, 2, 1, 0].map(|i| b + i * K),
            "highest frames first"
        );
        assert_eq!(frames.free_frames(), 0);
        assert_eq!(frames.allocate(), None);
    }

    #[test]
    fn freed_frames_come_back_last_in_first_out() {
        let mut arena = Arena::new(0x10_0000, 8);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 8 * K)]), arena.offset());
        let first = frames.allocate().unwrap();
        let second = frames.allocate().unwrap();
        // SAFETY: both frames came from this allocator and are not used any more.
        unsafe {
            frames.free(first);
        }
        // SAFETY: as above.
        unsafe {
            frames.free(second);
        }
        // The top frame holds the buddy allocator's bitmap.
        assert_eq!(frames.free_frames(), 7);
        assert_eq!(frames.allocate(), Some(second));
        assert_eq!(frames.allocate(), Some(first));
        // Then untouched memory again.
        assert_eq!(frames.allocate(), Some(b + 4 * K));
        assert_eq!(frames.free_frames(), 4);
    }

    /// Writes `block`'s frames through the arena and checks none is the buddy bitmap.
    fn touch(frames: &FrameAllocator, block: u64, order: u32) {
        let bitmap = frames.buddy.bitmap;
        assert!(
            bitmap < block || bitmap >= block + (K << order),
            "bitmap handed out"
        );
        let start = (block as usize).wrapping_add(frames.physical_memory_offset()) as *mut u8;
        // SAFETY: the block was just allocated from the arena, which backs it in full.
        unsafe { start.write_bytes(0xa5, (K << order) as usize) };
    }

    #[test]
    fn contiguous_blocks_of_every_order_are_aligned_and_merge_back() {
        // A base aligned to 1 MiB only, so 4 MiB alignment needs real carving.
        let mut arena = Arena::new(0x10_0000, 2100);
        let b = arena.base;
        let map = arena.map(&[(b, b + 2100 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        let total = frames.free_frames();
        let mut owned = Vec::new();
        // Largest first: the other way round, the small blocks carved from the top would leave
        // no aligned 4 MiB run in 8 MiB of RAM.
        for order in (0..=MAX_ORDER).rev() {
            let block = frames.allocate_contiguous(order).unwrap();
            assert_eq!(block % (K << order), 0, "order {order} block unaligned");
            touch(&frames, block, order);
            for &(other, other_order) in &owned {
                assert!(block + (K << order) <= other || other + (K << other_order) <= block);
            }
            owned.push((block, order));
        }
        assert_eq!(frames.allocate_contiguous(MAX_ORDER + 1), None);
        let used: u64 = owned.iter().map(|&(_, order)| 1 << order).sum();
        assert_eq!(frames.free_frames(), total - used);
        for (block, order) in owned {
            // SAFETY: allocated above with this order and no longer used.
            unsafe { frames.free_contiguous(block, order) };
        }
        assert_eq!(frames.free_frames(), total);
        // The freed order-10 block is whole again.
        let block = frames.allocate_contiguous(MAX_ORDER).unwrap();
        assert_eq!(block % (K << MAX_ORDER), 0);
    }

    #[test]
    fn a_block_and_its_buddy_merge() {
        let mut arena = Arena::new(0x40_0000, 64);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 64 * K)]), arena.offset());
        // Carved top down from frames 0..63 (the bitmap took frame 63): 56, 52, 48. The first
        // block's buddy, 60, is partly the carving's leftovers; 52 and 48 are buddies.
        let blocks = [0; 3].map(|_| frames.allocate_contiguous(2).unwrap());
        assert_eq!(blocks, [56, 52, 48].map(|i| b + i * K));
        // SAFETY: both came from this allocator with order 2 and are unused.
        unsafe { frames.free_contiguous(blocks[1], 2) };
        // SAFETY: as above.
        unsafe { frames.free_contiguous(blocks[2], 2) };
        assert_eq!(frames.allocate_contiguous(3), Some(blocks[2]));
    }

    #[test]
    fn contiguous_blocks_form_again_after_every_frame_went_through_the_free_stack() {
        let mut arena = Arena::new(0x40_0000, 64);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 64 * K)]), arena.offset());
        let total = frames.free_frames();
        let all: Vec<u64> = core::iter::from_fn(|| frames.allocate()).collect();
        assert_eq!(all.len() as u64, total);
        assert_eq!(frames.allocate_contiguous(1), None);
        for frame in all {
            // SAFETY: every frame came from this allocator and is unused.
            unsafe { frames.free(frame) };
        }
        let block = frames.allocate_contiguous(4).unwrap();
        assert_eq!(block % (16 * K), 0);
        touch(&frames, block, 4);
        assert_eq!(frames.free_frames(), total - 16);
        // Single frames still come from what is left, the stack and then the buddy allocator.
        let rest: Vec<u64> = core::iter::from_fn(|| frames.allocate()).collect();
        assert_eq!(rest.len() as u64, total - 16);
        assert!(rest.iter().all(|&f| f < block || f >= block + 16 * K));
    }

    #[test]
    fn the_bitmap_goes_where_it_fits_or_there_is_no_buddy_allocator() {
        let mut arena = Arena::new(0x10_0000, 16);
        let b = arena.base;
        // The highest range is a single frame: the bitmap goes to the top of the one below.
        let map = arena.map(&[(b, b + 8 * K), (b + 12 * K, b + 13 * K)]);
        let frames = FrameAllocator::new(&map, arena.offset());
        assert_eq!(frames.buddy.bitmap, b + 7 * K);
        assert_eq!(frames.free_frames(), 8);
        // No range keeps a frame besides the bitmap: single frames only.
        let map = arena.map(&[(b, b + K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        assert_eq!(frames.allocate_contiguous(1), None);
        assert_eq!(frames.allocate(), Some(b));
        assert_eq!(frames.allocate(), None);
    }

    #[test]
    fn random_single_and_contiguous_allocations_never_overlap() {
        let mut arena = Arena::new(0x4000_0000, 256);
        let b = arena.base;
        let map = arena.map(&[(b, b + 100 * K), (b + 104 * K, b + 256 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        let total = frames.free_frames();
        let mut owned: Vec<(u64, u32)> = Vec::new();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if state % 5 < 3 {
                let order = ((state >> 8) % 5) as u32;
                let order = if order < 3 { 0 } else { order - 2 };
                if let Some(block) = frames.allocate_contiguous(order) {
                    assert_eq!(block % (K << order), 0);
                    assert!(
                        map.ranges()
                            .iter()
                            .any(|r| r.start <= block && block + (K << order) <= r.end)
                    );
                    touch(&frames, block, order);
                    for &(other, other_order) in &owned {
                        assert!(
                            block + (K << order) <= other || other + (K << other_order) <= block,
                            "{block:#x} overlaps {other:#x}"
                        );
                    }
                    owned.push((block, order));
                }
            } else if !owned.is_empty() {
                let (block, order) = owned.swap_remove((state as usize >> 8) % owned.len());
                // SAFETY: allocated from this allocator with this order, no longer used.
                unsafe { frames.free_contiguous(block, order) };
            }
            let used: u64 = owned.iter().map(|&(_, order)| 1 << order).sum();
            assert_eq!(frames.free_frames() + used, total);
        }
    }

    #[test]
    fn empty_map_allocates_nothing() {
        let mut frames = FrameAllocator::new(&MemoryMap::new(), 0);
        assert_eq!(frames.free_frames(), 0);
        assert_eq!(frames.allocate(), None);
    }

    #[test]
    fn random_allocate_and_free_never_hands_out_a_frame_twice() {
        let mut arena = Arena::new(0x4000_0000, 64);
        let b = arena.base;
        let map = arena.map(&[(b, b + 20 * K), (b + 24 * K, b + 64 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        let total = frames.free_frames();
        assert_eq!(
            total,
            map.total() / K - 1,
            "one frame holds the buddy bitmap"
        );
        let mut owned: Vec<u64> = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if state % 5 < 3 {
                if let Some(frame) = frames.allocate() {
                    assert!(
                        map.ranges()
                            .iter()
                            .any(|r| r.start <= frame && frame < r.end)
                    );
                    assert_eq!(frame % K, 0);
                    assert!(!owned.contains(&frame), "{frame:#x} handed out twice");
                    owned.push(frame);
                } else {
                    assert_eq!(owned.len() as u64, total);
                }
            } else if !owned.is_empty() {
                let frame = owned.swap_remove((state as usize >> 8) % owned.len());
                // SAFETY: `frame` came from this allocator and the test no longer uses it.
                unsafe { frames.free(frame) };
            }
            assert_eq!(frames.free_frames() + owned.len() as u64, total);
        }
    }
}

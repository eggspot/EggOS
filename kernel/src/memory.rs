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
//!
//! Allocations a program drives (its pages at fault time, shared memory files, window buffers,
//! socket and pipe buffers, file contents in memory, new threads) leave the kernel a reserve
//! of free frames ([`user_reserve_for`], D155): they fail with `ENOMEM` (or end the program,
//! at a page fault) once free frames would drop below it, so the kernel's own allocations,
//! which cannot fail, always find memory.

use crate::paging::PageTables;
use crate::sync::LeafSpinLock;
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
pub static FRAMES: LeafSpinLock<Option<FrameAllocator>> = LeafSpinLock::new_leaf(None);

static TOTAL_FRAMES: eggos_hal::atomic::AtomicU64 = eggos_hal::atomic::AtomicU64::new(0);

/// Records how many frames the allocator had once the kernel's page tables were built: the
/// memory programs can have, as `/proc/meminfo`'s `MemTotal` reports it.
pub fn record_total_frames(frames: u64) {
    TOTAL_FRAMES.store(frames, core::sync::atomic::Ordering::Relaxed);
}

/// The frames [`record_total_frames`] recorded; 0 before.
pub fn total_frames() -> u64 {
    TOTAL_FRAMES.load(core::sync::atomic::Ordering::Relaxed)
}

/// `1 / divisor` of the memory programs can have ([`total_frames`]), in bytes: what the global
/// caps on window buffers and socket buffers are; no cap (`u64::MAX`) before the total is
/// recorded, and on the host, where the frame allocator is not the machine's memory.
pub fn share_of_memory(divisor: u64) -> u64 {
    match total_frames() {
        frames if frames > 0 && cfg!(eggos_kernel) => frames * FRAME_SIZE / divisor.max(1),
        _ => u64::MAX,
    }
}

/// The part of memory the reserve is: 1/32 (D155).
pub const USER_RESERVE_SHARE: u64 = 32;
/// The smallest reserve: 512 frames, 2 MiB (D155).
pub const MIN_USER_RESERVE_FRAMES: u64 = 512;
/// The largest reserve: 4096 frames, 16 MiB (D155).
pub const MAX_USER_RESERVE_FRAMES: u64 = 4096;

/// The frames allocations programs drive leave free for the kernel, on a machine whose
/// programs can have `total` frames: [`USER_RESERVE_SHARE`] of them, at least
/// [`MIN_USER_RESERVE_FRAMES`] and at most [`MAX_USER_RESERVE_FRAMES`], but never more than a
/// quarter of `total` (D155).
pub const fn user_reserve_for(total: u64) -> u64 {
    let share = total / USER_RESERVE_SHARE;
    let clamped = if share < MIN_USER_RESERVE_FRAMES {
        MIN_USER_RESERVE_FRAMES
    } else if share > MAX_USER_RESERVE_FRAMES {
        MAX_USER_RESERVE_FRAMES
    } else {
        share
    };
    let quarter = total / 4;
    if clamped < quarter { clamped } else { quarter }
}

/// Whether a program may take `frames` more frames now: the free frames stay at or above the
/// reserve after it. True before there is a frame allocator.
pub fn user_may_take_frames(frames: u64) -> bool {
    FRAMES
        .lock()
        .as_ref()
        .is_none_or(|allocator| allocator.user_may_take(frames))
}

/// Marks the end of the free stack. Never a frame address: frames are aligned to
/// [`FRAME_SIZE`].
const NO_FRAME: u64 = u64::MAX;

/// The second word of a frame on the free stack in debug builds, mixed with the frame's address
/// ([`FrameAllocator::free`]'s double-free check).
const STACKED_MARK: u64 = 0x5eed_f4ee_57ac_4ed0;

/// The free stack's high watermark is this share of the machine's frames (D551)...
const STACK_SHARE: u64 = 8;
/// ...and never below this many frames (16 MiB).
const MIN_STACK_HIGH_WATERMARK: u64 = 4096;
/// Frames [`FrameAllocator::free`] moves into the buddy allocator at a time past the watermark.
const STACK_TRIM_BATCH: u32 = 256;

/// Frames a CPU keeps for programs' page faults, so most faults take a frame without the frame
/// allocator's lock (D551). Counted as allocated while they sit here; the allocator takes them
/// back ([`drain_frame_caches`]) before it reports memory exhausted.
#[derive(Debug)]
pub struct FrameCache {
    frames: [u64; FRAME_CACHE_SIZE],
    count: usize,
}

/// Frames a [`FrameCache`] holds at most.
pub const FRAME_CACHE_SIZE: usize = 32;
/// Frames a [`FrameCache`] takes from the allocator at once when it runs dry.
const FRAME_CACHE_BATCH: usize = 16;

impl FrameCache {
    pub const fn new() -> Self {
        Self {
            frames: [NO_FRAME; FRAME_CACHE_SIZE],
            count: 0,
        }
    }

    fn pop(&mut self) -> Option<u64> {
        self.count = self.count.checked_sub(1)?;
        Some(self.frames[self.count])
    }

    fn push(&mut self, frame: u64) -> bool {
        let Some(slot) = self.frames.get_mut(self.count) else {
            return false;
        };
        *slot = frame;
        self.count += 1;
        true
    }

    /// Refills it from `allocator` with frames programs may take (above the reserve).
    fn refill(&mut self, allocator: &mut FrameAllocator) {
        for _ in 0..FRAME_CACHE_BATCH {
            let Some(frame) = allocator.allocate_for_user() else {
                break;
            };
            if !self.push(frame) {
                // SAFETY: the frame was just allocated from `allocator` and is unused.
                unsafe { allocator.free(frame) };
                break;
            }
        }
    }

    /// Gives every frame back to `allocator`.
    fn empty_into(&mut self, allocator: &mut FrameAllocator) {
        while let Some(frame) = self.pop() {
            // SAFETY: the cache owned the frame, which came from this allocator, unused.
            unsafe { allocator.free(frame) };
        }
    }
}

impl Default for FrameCache {
    fn default() -> Self {
        Self::new()
    }
}

/// A frame for a program's page, from this CPU's [`FrameCache`] when it has one, else from the
/// frame allocator (taking a batch for the cache under one lock); `None` once free frames are
/// down to the reserve, after every CPU's cache was given back.
pub fn allocate_user_frame<P: Platform>() -> Option<u64> {
    let cpu = crate::cpu::current::<P>();
    {
        let mut cache = cpu.frame_cache.lock();
        if let Some(frame) = cache.pop() {
            return Some(frame);
        }
        let mut frames = FRAMES.lock();
        let allocator = frames.as_mut()?;
        cache.refill(allocator);
        if let Some(frame) = cache.pop() {
            return Some(frame);
        }
    }
    drain_frame_caches();
    if let Some(frame) = FRAMES.lock().as_mut()?.allocate_for_user() {
        return Some(frame);
    }
    if cfg!(eggos_kernel) && crate::HEAP.release_empty_slabs() > 0 {
        return FRAMES.lock().as_mut()?.allocate_for_user();
    }
    None
}

/// Gives back a frame a program's page used: into this CPU's [`FrameCache`] while it has room,
/// else to the frame allocator.
///
/// # Safety
/// As [`FrameAllocator::free`]: the frame came from the frame allocator, is not freed yet, and
/// nothing uses it any more.
pub unsafe fn free_user_frame<P: Platform>(frame: u64) {
    let cpu = crate::cpu::current::<P>();
    let mut cache = cpu.frame_cache.lock();
    if cache.push(frame) {
        return;
    }
    let mut frames = FRAMES.lock();
    if let Some(allocator) = frames.as_mut() {
        // SAFETY: the caller's promise.
        unsafe { allocator.free(frame) };
    }
}

/// Gives every CPU's cached frames back to the frame allocator: before an allocation reports
/// memory exhausted, and for `/proc/meminfo`'s exact count.
pub fn drain_frame_caches() {
    for cpu in crate::cpu::CPUS.all() {
        let mut cache = cpu.frame_cache.lock();
        if cache.count == 0 {
            continue;
        }
        let mut frames = FRAMES.lock();
        if let Some(allocator) = frames.as_mut() {
            cache.empty_into(allocator);
        }
    }
}

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
    user_reserve: u64,
    physical_memory_offset: usize,
    /// Frames on the free stack.
    stacked_frames: u64,
    /// Past this many frames on the free stack, [`Self::free`] moves a batch of them into the
    /// buddy allocator, where they merge back into contiguous blocks (D551).
    stack_high_watermark: u64,
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
            user_reserve: 0,
            physical_memory_offset,
            stacked_frames: 0,
            stack_high_watermark: (map.total() / FRAME_SIZE / STACK_SHARE).max(MIN_STACK_HIGH_WATERMARK),
            buddy: Buddy::none(),
        };
        if !map.ranges().is_empty() {
            let bytes = Buddy::bitmap_bytes(map);
            if let Some(bitmap) = allocator.take_from_untouched(bytes) {
                allocator.buddy = Buddy::new(map, bitmap);
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

    /// The memory it never handed out (the buddy allocator's bitmap included), while every
    /// frame it handed out came from untouched memory: none freed back and the buddy
    /// allocator never used. `None` otherwise. The kernel builds its page tables with an
    /// allocator over the memory the boot direct map reaches, then makes one over this and the
    /// rest of RAM ([`crate::paging::enable`]).
    pub fn untouched_memory(&self) -> Option<MemoryMap> {
        if self.free_stack != NO_FRAME || self.buddy.zeroed {
            return None;
        }
        let mut map = MemoryMap::new();
        for range in self.untouched[..self.untouched_ranges]
            .iter()
            .chain([&self.current])
        {
            map.add_ram(*range);
        }
        if self.buddy.exists() {
            map.add_ram(PhysicalRange::from_start_length(
                self.buddy.bitmap,
                self.buddy.bitmap_bytes,
            ));
        }
        Some(map)
    }

    /// Where this allocator reaches physical memory: frame `p` at virtual `p + offset`.
    pub fn physical_memory_offset(&self) -> usize {
        self.physical_memory_offset
    }

    /// Frames that [`Self::allocate`] can still return.
    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    /// Makes the `*_for_user` allocations leave `frames` free frames (0 at first:
    /// [`crate::paging::enable`] sets [`user_reserve_for`] the machine's frames).
    pub fn set_user_reserve(&mut self, frames: u64) {
        self.user_reserve = frames;
    }

    /// The frames the `*_for_user` allocations leave free.
    pub fn user_reserve(&self) -> u64 {
        self.user_reserve
    }

    /// Whether `frames` more frames can go to a program: the free frames stay at or above the
    /// reserve after it.
    pub fn user_may_take(&self, frames: u64) -> bool {
        self.free_frames >= self.user_reserve.saturating_add(frames)
    }

    /// As [`Self::allocate`], for memory a program drives: `None` too when the free frames are
    /// down to the reserve.
    #[inline]
    pub fn allocate_for_user(&mut self) -> Option<u64> {
        if self.free_frames <= self.user_reserve {
            return None;
        }
        self.allocate()
    }

    /// As [`Self::allocate_contiguous`], for memory a program drives: `None` too when the
    /// block would take the free frames below the reserve.
    pub fn allocate_contiguous_for_user(&mut self, order: u32) -> Option<u64> {
        if order > MAX_ORDER || !self.user_may_take(1 << order) {
            return None;
        }
        self.allocate_contiguous_of_kind(order, BlockKind::User)
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
            self.stacked_frames -= 1;
            if cfg!(debug_assertions) {
                // SAFETY: the frame is now the caller's, reached through the offset and aligned;
                // its second word loses the free-stack mark.
                unsafe { self.link(frame).add(1).write(0) };
            }
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
            let frame = self.allocate_from_buddy(0, BlockKind::Kernel)?;
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
        self.allocate_contiguous_of_kind(order, BlockKind::Kernel)
    }

    /// As [`Self::allocate_contiguous`], grouped with the other blocks of `kind`.
    ///
    /// When nothing else gives a block, it hands every untouched range and the whole free
    /// stack to the buddy allocator before it gives up, so a free run that straddles untouched
    /// memory and freed frames still merges into the block (D551). Single frames then come
    /// from the buddy allocator once the free stack is empty: near exhaustion, where a
    /// contiguous block matters more than the bump allocator's speed.
    pub fn allocate_contiguous_of_kind(&mut self, order: u32, kind: BlockKind) -> Option<u64> {
        if order == 0 {
            return self.allocate();
        }
        if order > MAX_ORDER || !self.buddy.exists() {
            return None;
        }
        let block = match self.allocate_from_buddy(order, kind) {
            Some(block) => block,
            None => match self.carve_untouched(order) {
                Some(block) => block,
                None => {
                    self.drain_free_stack(order);
                    match self.allocate_from_buddy(order, kind) {
                        Some(block) => block,
                        None => self.allocate_after_compacting(order, kind)?,
                    }
                }
            },
        };
        self.free_frames -= 1 << order;
        Some(block)
    }

    /// The last try for a contiguous block: every untouched range and every stacked frame go
    /// to the buddy allocator, where they merge with each other.
    #[cold]
    fn allocate_after_compacting(&mut self, order: u32, kind: BlockKind) -> Option<u64> {
        let ranges = self.untouched_ranges;
        for index in 0..ranges {
            let range = self.untouched[index];
            self.give_range_to_buddy(range);
        }
        self.untouched_ranges = 0;
        let current = core::mem::take(&mut self.current);
        self.give_range_to_buddy(current);
        self.drain_free_stack(MAX_ORDER + 1);
        self.allocate_from_buddy(order, kind)
    }

    /// Inserts `range`'s frames into the buddy allocator as the largest aligned blocks that fit.
    fn give_range_to_buddy(&mut self, range: PhysicalRange) {
        let mut buddy = self.buddy_memory();
        let mut start = range.start;
        while start < range.end {
            let order = (start.trailing_zeros() - FRAME_SIZE.trailing_zeros())
                .min(MAX_ORDER)
                .min((range.end - start).ilog2() - FRAME_SIZE.trailing_zeros());
            buddy.insert(start, order);
            start += FRAME_SIZE << order;
        }
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
    fn allocate_from_buddy(&mut self, order: u32, kind: BlockKind) -> Option<u64> {
        if !self.buddy.exists() {
            return None;
        }
        self.buddy_memory().allocate(order, kind)
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
            self.untouched.copy_within(i + 1..self.untouched_ranges, i);
            self.untouched_ranges -= 1;
        }
        self.give_range_to_buddy(above);
        Some(block)
    }

    /// Moves frames from the free stack into the buddy allocator, where freed neighbours merge
    /// into larger blocks, until a block of `order` forms or the stack is empty.
    fn drain_free_stack(&mut self, order: u32) {
        while self.free_stack != NO_FRAME {
            let frame = self.free_stack;
            // SAFETY: as in `allocate`: `frame` is on the free stack, its first word the link.
            self.free_stack = unsafe { self.link(frame).read() };
            self.stacked_frames -= 1;
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
        if cfg!(debug_assertions) {
            self.check_not_stacked(frame);
        }
        // SAFETY: the caller gives up the frame, which this allocator handed out, so its first
        // word is ours to write; it is reached through the physical memory offset and aligned.
        unsafe { self.link(frame).write(self.free_stack) };
        self.free_stack = frame;
        self.free_frames += 1;
        self.stacked_frames += 1;
        if self.stacked_frames > self.stack_high_watermark {
            self.trim_free_stack();
        }
    }

    /// Debug builds: panics when `frame` is already on the free stack (freed twice), and marks
    /// it as stacked. The mark is its second word, [`STACKED_MARK`] mixed with its address; a
    /// frame in use that happens to hold it costs a walk of the stack, never a false report.
    fn check_not_stacked(&mut self, frame: u64) {
        let mark = self.link(frame).wrapping_add(1);
        // SAFETY: the caller of `free` gives the frame up, so its second word is ours to read
        // and write; it is reached through the physical memory offset and aligned.
        if unsafe { mark.read() } == STACKED_MARK ^ frame {
            let mut at = self.free_stack;
            while at != NO_FRAME {
                assert_ne!(at, frame, "frame {frame:#x} freed twice");
                // SAFETY: `at` is on the free stack; its first word is the next link.
                at = unsafe { self.link(at).read() };
            }
        }
        // SAFETY: as above.
        unsafe { mark.write(STACKED_MARK ^ frame) };
    }

    /// Moves [`STACK_TRIM_BATCH`] frames from the top of the free stack into the buddy
    /// allocator, where they merge with their free neighbours, once the stack passed its high
    /// watermark.
    #[cold]
    fn trim_free_stack(&mut self) {
        if !self.buddy.exists() {
            return;
        }
        for _ in 0..STACK_TRIM_BATCH {
            let frame = self.free_stack;
            if frame == NO_FRAME {
                break;
            }
            // SAFETY: as in `allocate`: `frame` is on the free stack, its first word the link.
            self.free_stack = unsafe { self.link(frame).read() };
            self.stacked_frames -= 1;
            self.buddy_memory().insert(frame, 0);
        }
    }

    /// Gives back every frame in `chain` at once: the chain joins the free stack whole, in
    /// constant time, so the frame allocator's lock is held no longer for a million frames than
    /// for one ([`FramesToFree::chain`] walked it before the lock was taken).
    ///
    /// # Safety
    /// Every frame in `chain` must have come from [`Self::allocate`] on this allocator and not
    /// been freed since, nothing may use them any more, and the chain must have been linked
    /// through this allocator's physical memory offset ([`FramesToFree::push`]).
    pub unsafe fn free_chain(&mut self, chain: FrameChain) {
        if chain.count == 0 {
            return;
        }
        // SAFETY: `chain.last` is the chain's last frame, linked through this allocator's
        // offset; the caller gives every frame up, so its link is ours to write.
        unsafe { self.link(chain.last).write(self.free_stack) };
        self.free_stack = chain.first;
        self.free_frames += chain.count;
        self.stacked_frames += chain.count;
        if self.stacked_frames > self.stack_high_watermark {
            self.trim_free_stack();
        }
    }

    /// Where the kernel reaches the first word of `frame`, the free-stack link.
    fn link(&self, frame: u64) -> *mut u64 {
        (frame as usize).wrapping_add(self.physical_memory_offset) as *mut u64
    }
}

/// Frames to give back to the frame allocator together and later, once nothing can reach them:
/// `munmap` collects them while it clears their entries and frees them after the CPU's
/// translations are dropped ([`Self::chain`], then [`FrameAllocator::free_chain`]). Linked
/// through their first words, as the free stack is, so collecting them allocates nothing and
/// costs no lock, and only the chain's first frame is kept here: one register in the page-table
/// walk that collects them. Dropped without being given back, they leak.
#[derive(Debug)]
#[must_use = "the frames leak unless they go to `FrameAllocator::free_chain`"]
pub struct FramesToFree {
    /// The frame pushed last, or [`NO_FRAME`].
    first: u64,
}

impl FramesToFree {
    /// No frames yet.
    pub const fn new() -> Self {
        Self { first: NO_FRAME }
    }

    /// Adds `frame`, which the kernel reaches at physical address + `physical_memory_offset`
    /// (the frame allocator's, [`FrameAllocator::physical_memory_offset`]).
    ///
    /// # Safety
    /// `frame` must be a frame of the frame allocator's that is in use, owned by the caller
    /// and in no other `FramesToFree`, and nothing may write it any more: its first word is
    /// overwritten now.
    #[inline]
    pub unsafe fn push(&mut self, frame: u64, physical_memory_offset: usize) {
        debug_assert!(
            frame.is_multiple_of(FRAME_SIZE),
            "freeing an unaligned frame"
        );
        let link = (frame as usize).wrapping_add(physical_memory_offset) as *mut u64;
        // SAFETY: the caller gives up `frame`, so its first word is ours to write; it is reached
        // through the allocator's physical memory offset and aligned.
        unsafe { link.write(self.first) };
        self.first = frame;
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.first == NO_FRAME
    }

    /// The frames as a chain [`FrameAllocator::free_chain`] takes in constant time: a walk down
    /// the links, whose frames were just written, to count them and find the last one, done
    /// here so that no lock is held meanwhile.
    ///
    /// # Safety
    /// `physical_memory_offset` must be the one every frame was given to [`Self::push`] with.
    pub unsafe fn chain(self, physical_memory_offset: usize) -> FrameChain {
        if self.first == NO_FRAME {
            return FrameChain {
                first: NO_FRAME,
                last: NO_FRAME,
                count: 0,
            };
        }
        let mut last = self.first;
        let mut count = 1;
        loop {
            let link = (last as usize).wrapping_add(physical_memory_offset) as *const u64;
            // SAFETY: `last` is in the chain, which `push` linked through the frames' first
            // words at this offset (the caller's promise), and whose frames nothing else uses
            // any more (`push`'s contract).
            let next = unsafe { link.read() };
            if next == NO_FRAME {
                break;
            }
            last = next;
            count += 1;
        }
        FrameChain {
            first: self.first,
            last,
            count,
        }
    }
}

impl Default for FramesToFree {
    fn default() -> Self {
        Self::new()
    }
}

/// Frames linked through their first words, with both ends and the count known
/// ([`FramesToFree::chain`]), for [`FrameAllocator::free_chain`].
#[derive(Debug)]
#[must_use = "the frames leak unless they go to `FrameAllocator::free_chain`"]
pub struct FrameChain {
    /// The chain's first frame, or [`NO_FRAME`].
    first: u64,
    /// Its last frame, whose link is [`NO_FRAME`], or [`NO_FRAME`].
    last: u64,
    /// How many frames it has.
    count: u64,
}

/// Largest block the buddy allocator manages: 2^`MAX_ORDER` frames (4 MiB).
pub const MAX_ORDER: u32 = 10;

/// A pageblock: 2^`PAGEBLOCK_ORDER` frames (2 MiB) that the buddy allocator keeps for one
/// [`BlockKind`] of allocation, so the kernel's long-lived blocks and programs' blocks do not
/// interleave and break up each other's large runs (anti-fragmentation grouping).
pub const PAGEBLOCK_ORDER: u32 = 9;

/// Who a buddy block is for: the pageblock it lies in is grouped with others of its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// The kernel's own memory: heap blocks over a frame, DMA buffers, page tables.
    Kernel = 0,
    /// Memory a program drives: huge pages, window buffers, pipe and socket rings.
    User = 1,
}

const KINDS: usize = 2;

impl BlockKind {
    fn other(self) -> Self {
        match self {
            Self::Kernel => Self::User,
            Self::User => Self::Kernel,
        }
    }

    fn from_bit(bit: bool) -> Self {
        if bit { Self::User } else { Self::Kernel }
    }
}

/// One range of the memory map as the buddy allocator sees it: its frames and where its part of
/// the metadata starts (one bit per frame saying whether a free block starts there, then one
/// bit per pageblock it touches saying whether that pageblock is kept for programs).
#[derive(Clone, Copy, Debug, Default)]
struct BuddyRange {
    range: PhysicalRange,
    /// Physical address of the range's first free-block bit's byte.
    free_bits: u64,
    /// Physical address of the range's first pageblock bit's byte.
    kind_bits: u64,
}

impl BuddyRange {
    fn frames(range: PhysicalRange) -> u64 {
        range.len() / FRAME_SIZE
    }

    fn pageblocks(range: PhysicalRange) -> u64 {
        if range.is_empty() {
            return 0;
        }
        let shift = FRAME_SIZE.trailing_zeros() + PAGEBLOCK_ORDER;
        ((range.end - 1) >> shift) - (range.start >> shift) + 1
    }

    fn metadata_bytes(range: PhysicalRange) -> u64 {
        Self::frames(range).div_ceil(8) + Self::pageblocks(range).div_ceil(8)
    }
}

/// The buddy allocator's state (D31, D551): free blocks of 2^order contiguous frames, each
/// aligned to its size, in one doubly linked list per order and per [`BlockKind`]. A free block's
/// first three words hold its links (next, previous) and its order with its list's kind. Each
/// range of the memory map has its own metadata, so a sparse machine pays nothing for its
/// holes: one bit per frame says whether the frame starts a free block, so freeing a block finds
/// out in one load whether its buddy is free to merge with (and then reads the buddy's order
/// from the buddy itself), and one bit per pageblock says which kind it is kept for.
#[derive(Debug)]
struct Buddy {
    ranges: [BuddyRange; MAX_RANGES],
    range_count: usize,
    /// Physical address of the metadata, every range's in one run of whole frames.
    bitmap: u64,
    /// The metadata's length in bytes, whole frames.
    bitmap_bytes: u64,
    /// Whether the metadata has been zeroed yet; it is, on the buddy allocator's first use.
    zeroed: bool,
    /// The first free block of each order, per kind, or [`NO_FRAME`].
    heads: [[u64; MAX_ORDER as usize + 1]; KINDS],
}

/// Words at the start of a free buddy block.
const NEXT: usize = 0;
const PREVIOUS: usize = 1;
const ORDER: usize = 2;
/// The bit of the [`ORDER`] word that says the block is on the [`BlockKind::User`] lists.
const USER_LIST: u64 = 1 << 8;

impl Buddy {
    /// No buddy allocator: single frames only.
    const fn none() -> Self {
        Self {
            ranges: [BuddyRange {
                range: PhysicalRange { start: 0, end: 0 },
                free_bits: 0,
                kind_bits: 0,
            }; MAX_RANGES],
            range_count: 0,
            bitmap: 0,
            bitmap_bytes: 0,
            zeroed: false,
            heads: [[NO_FRAME; MAX_ORDER as usize + 1]; KINDS],
        }
    }

    /// A buddy allocator over the ranges of `map`, with their metadata at `bitmap`, which is
    /// [`Self::bitmap_bytes`] of `map` long.
    fn new(map: &MemoryMap, bitmap: u64) -> Self {
        let mut buddy = Self::none();
        let mut at = bitmap;
        for (slot, &range) in buddy.ranges.iter_mut().zip(map.ranges()) {
            let free_bits = at;
            let kind_bits = free_bits + BuddyRange::frames(range).div_ceil(8);
            at = kind_bits + BuddyRange::pageblocks(range).div_ceil(8);
            *slot = BuddyRange {
                range,
                free_bits,
                kind_bits,
            };
        }
        buddy.range_count = map.ranges().len();
        buddy.bitmap = bitmap;
        buddy.bitmap_bytes = Self::bitmap_bytes(map);
        buddy
    }

    fn exists(&self) -> bool {
        self.range_count > 0
    }

    /// Bytes the metadata for every range of `map` takes, in whole frames.
    fn bitmap_bytes(map: &MemoryMap) -> u64 {
        map.ranges()
            .iter()
            .map(|&range| BuddyRange::metadata_bytes(range))
            .sum::<u64>()
            .next_multiple_of(FRAME_SIZE)
    }

    /// The range holding `frame`, if any.
    fn range_of(&self, frame: u64) -> Option<&BuddyRange> {
        let ranges = &self.ranges[..self.range_count];
        let index = ranges.partition_point(|r| r.range.end <= frame);
        ranges.get(index).filter(|r| r.range.start <= frame)
    }
}

/// A [`Buddy`] with the physical memory offset that reaches its blocks and metadata.
struct BuddyMemory<'a> {
    buddy: &'a mut Buddy,
    offset: usize,
}

impl BuddyMemory<'_> {
    /// Removes and returns a block of 2^`order` frames for `kind`, splitting a larger free block
    /// if needed (its other parts stay free, on the lists of the pageblocks they lie in). When
    /// `kind`'s lists have nothing large enough, it takes the largest block of the other kind's
    /// and keeps the pageblocks that block covers for `kind` from then on, so the next
    /// allocations of `kind` find their frames together (Linux's fallback, D551).
    fn allocate(&mut self, order: u32, kind: BlockKind) -> Option<u64> {
        self.zero_bitmap();
        let own = (order..=MAX_ORDER).find(|&o| self.head(kind, o) != NO_FRAME);
        let (mut found, block) = match own {
            Some(found) => (found, self.head(kind, found)),
            None => {
                let other = kind.other();
                let found = (order..=MAX_ORDER)
                    .rev()
                    .find(|&o| self.head(other, o) != NO_FRAME)?;
                let block = self.head(other, found);
                (found, block)
            }
        };
        self.unlink(block, found);
        if own.is_none() {
            self.claim(block, found, kind);
        }
        while found > order {
            found -= 1;
            self.push(block + (FRAME_SIZE << found), found);
        }
        Some(block)
    }

    fn head(&self, kind: BlockKind, order: u32) -> u64 {
        self.buddy.heads[kind as usize][order as usize]
    }

    /// Keeps the pageblocks under the block at `block` of `order` for `kind`: every pageblock it
    /// covers, or, for a block smaller than a pageblock, the one it lies in.
    fn claim(&mut self, block: u64, order: u32, kind: BlockKind) {
        let size = FRAME_SIZE << order.max(PAGEBLOCK_ORDER);
        let first = block / size * size;
        let mut pageblock = first;
        while pageblock < first + size {
            self.set_kind(pageblock.max(block), kind);
            pageblock += FRAME_SIZE << PAGEBLOCK_ORDER;
        }
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
        let (byte, bit) = self.free_bit(frame)?;
        // SAFETY: the byte is in the metadata of the range holding `frame`, which the allocator
        // owns and zeroed before any use.
        if unsafe { byte.read() } & bit == 0 {
            return None;
        }
        // SAFETY: the bit says a free block starts at `frame`, whose third word `push` set to
        // its order and list.
        Some((unsafe { self.word(frame, ORDER).read() } & 0xff) as u32)
    }

    /// Pushes the free block at `block` onto the list of `order` of the kind its pageblock is
    /// kept for.
    fn push(&mut self, block: u64, order: u32) {
        let kind = self.kind_of(block);
        let head = &mut self.buddy.heads[kind as usize][order as usize];
        let next = *head;
        *head = block;
        let list = if kind == BlockKind::User { USER_LIST } else { 0 };
        // SAFETY: `block` is free and owned by the allocator, reached through the physical
        // memory offset; its first three words are its header.
        unsafe { self.word(block, NEXT).write(next) };
        // SAFETY: as above.
        unsafe { self.word(block, PREVIOUS).write(NO_FRAME) };
        // SAFETY: as above.
        unsafe { self.word(block, ORDER).write(u64::from(order) | list) };
        if next != NO_FRAME {
            // SAFETY: `next` was the list's head, a free block with a header.
            unsafe { self.word(next, PREVIOUS).write(block) };
        }
        self.set_free_bit(block, true);
    }

    /// Takes the free block at `block` off the list of `order` it was pushed onto (its header
    /// names the kind, which its pageblock may have changed since).
    fn unlink(&mut self, block: u64, order: u32) {
        // SAFETY: `block` is a free block on a list, so `push` wrote its header.
        let next = unsafe { self.word(block, NEXT).read() };
        // SAFETY: as above.
        let previous = unsafe { self.word(block, PREVIOUS).read() };
        // SAFETY: as above.
        let list = unsafe { self.word(block, ORDER).read() } & USER_LIST;
        let kind = BlockKind::from_bit(list != 0);
        if previous == NO_FRAME {
            self.buddy.heads[kind as usize][order as usize] = next;
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
        let Some((byte, bit)) = self.free_bit(frame) else {
            return;
        };
        // SAFETY: the byte is in the metadata of the range holding `frame`, which the allocator
        // owns.
        let value = unsafe { byte.read() };
        let value = if free { value | bit } else { value & !bit };
        // SAFETY: as above.
        unsafe { byte.write(value) };
    }

    /// The kind the pageblock holding `frame` is kept for: the kernel's until claimed.
    fn kind_of(&self, frame: u64) -> BlockKind {
        let Some((byte, bit)) = self.kind_bit(frame) else {
            return BlockKind::Kernel;
        };
        // SAFETY: the byte is in the metadata of the range holding `frame`.
        BlockKind::from_bit(unsafe { byte.read() } & bit != 0)
    }

    fn set_kind(&mut self, frame: u64, kind: BlockKind) {
        let Some((byte, bit)) = self.kind_bit(frame) else {
            return;
        };
        // SAFETY: the byte is in the metadata of the range holding `frame`, which the allocator
        // owns.
        let value = unsafe { byte.read() };
        let value = match kind {
            BlockKind::User => value | bit,
            BlockKind::Kernel => value & !bit,
        };
        // SAFETY: as above.
        unsafe { byte.write(value) };
    }

    /// Zeroes the metadata, the first time the buddy allocator is used.
    fn zero_bitmap(&mut self) {
        if !self.buddy.zeroed {
            let bitmap = self.virt(self.buddy.bitmap) as *mut u8;
            // SAFETY: the metadata frames were taken out of the free memory for this and are
            // reached through the physical memory offset.
            unsafe { bitmap.write_bytes(0, self.buddy.bitmap_bytes as usize) };
            self.buddy.zeroed = true;
        }
    }

    /// The metadata byte holding `frame`'s free-block bit, and the bit; `None` outside every
    /// range.
    fn free_bit(&self, frame: u64) -> Option<(*mut u8, u8)> {
        let range = self.buddy.range_of(frame)?;
        let index = (frame - range.range.start) / FRAME_SIZE;
        let byte = self.virt(range.free_bits + index / 8) as *mut u8;
        Some((byte, 1 << (index % 8)))
    }

    /// The metadata byte holding the kind bit of `frame`'s pageblock, and the bit; `None`
    /// outside every range.
    fn kind_bit(&self, frame: u64) -> Option<(*mut u8, u8)> {
        let range = self.buddy.range_of(frame)?;
        let shift = FRAME_SIZE.trailing_zeros() + PAGEBLOCK_ORDER;
        let index = (frame >> shift) - (range.range.start >> shift);
        let byte = self.virt(range.kind_bits + index / 8) as *mut u8;
        Some((byte, 1 << (index % 8)))
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
    fn untouched_memory_is_what_was_never_handed_out_until_a_frame_comes_back() {
        let mut arena = Arena::new(0x10_0000, 16);
        let b = arena.base;
        let map = arena.map(&[(b, b + 4 * K), (b + 8 * K, b + 10 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        let untouched = frames.untouched_memory().unwrap();
        assert_eq!(untouched.ranges(), map.ranges());
        let first = frames.allocate().unwrap();
        let second = frames.allocate().unwrap();
        assert_eq!((first, second), (b + 8 * K, b + 3 * K));
        let left = frames.untouched_memory().unwrap();
        assert_eq!(
            left.ranges(),
            [
                PhysicalRange::from_start_length(b, 3 * K),
                PhysicalRange::from_start_length(b + 9 * K, K)
            ],
            "the bitmap's frame stays"
        );
        assert_eq!(left.total(), map.total() - 2 * K);
        // SAFETY: `second` came from this allocator and nothing uses it.
        unsafe { frames.free(second) };
        assert!(frames.untouched_memory().is_none());
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
        assert_eq!(frames.free_frames(), 7);
        assert_eq!(frames.allocate(), Some(second));
        assert_eq!(frames.allocate(), Some(first));
        assert_eq!(frames.allocate(), Some(b + 4 * K));
        assert_eq!(frames.free_frames(), 4);
    }

    #[test]
    fn frames_freed_together_join_the_free_stack_at_once() {
        let mut arena = Arena::new(0x10_0000, 8);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 8 * K)]), arena.offset());
        let taken: [u64; 3] = core::array::from_fn(|_| frames.allocate().unwrap());
        let kept = frames.allocate().unwrap();
        // SAFETY: the frame came from this allocator and is not used any more.
        unsafe { frames.free(kept) };
        let free = frames.free_frames();
        // SAFETY: an empty chain links nothing.
        let empty = unsafe { FramesToFree::new().chain(arena.offset()) };
        // SAFETY: nothing is freed.
        unsafe { frames.free_chain(empty) };
        assert_eq!(frames.free_frames(), free);
        let mut freed = FramesToFree::new();
        for frame in taken {
            // SAFETY: the frame came from this allocator, is in use and is not used any more.
            unsafe { freed.push(frame, arena.offset()) };
        }
        assert!(!freed.is_empty());
        // SAFETY: every frame was pushed with the arena's offset.
        let chain = unsafe { freed.chain(arena.offset()) };
        // SAFETY: every frame in `chain` came from this allocator and is not used any more.
        unsafe { frames.free_chain(chain) };
        assert_eq!(frames.free_frames(), free + 3);
        assert_eq!(frames.allocate(), Some(taken[2]));
        assert_eq!(frames.allocate(), Some(taken[1]));
        assert_eq!(frames.allocate(), Some(taken[0]));
        assert_eq!(frames.allocate(), Some(kept));
        assert_eq!(frames.allocate(), Some(b + 2 * K));
    }

    /// Writes `block`'s frames through the arena and checks none is the buddy bitmap.
    fn touch(frames: &FrameAllocator, block: u64, order: u32) {
        let bitmap = frames.buddy.bitmap;
        assert!(
            bitmap + frames.buddy.bitmap_bytes <= block || bitmap >= block + (K << order),
            "bitmap handed out"
        );
        let start = (block as usize).wrapping_add(frames.physical_memory_offset()) as *mut u8;
        // SAFETY: the block was just allocated from the arena, which backs it in full.
        unsafe { start.write_bytes(0xa5, (K << order) as usize) };
    }

    #[test]
    fn contiguous_blocks_of_every_order_are_aligned_and_merge_back() {
        let mut arena = Arena::new(0x10_0000, 2100);
        let b = arena.base;
        let map = arena.map(&[(b, b + 2100 * K)]);
        let mut frames = FrameAllocator::new(&map, arena.offset());
        let total = frames.free_frames();
        let mut owned = Vec::new();
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
        let block = frames.allocate_contiguous(MAX_ORDER).unwrap();
        assert_eq!(block % (K << MAX_ORDER), 0);
    }

    #[test]
    fn a_block_and_its_buddy_merge() {
        let mut arena = Arena::new(0x40_0000, 64);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 64 * K)]), arena.offset());
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
        let rest: Vec<u64> = core::iter::from_fn(|| frames.allocate()).collect();
        assert_eq!(rest.len() as u64, total - 16);
        assert!(rest.iter().all(|&f| f < block || f >= block + 16 * K));
    }

    #[test]
    fn the_bitmap_goes_where_it_fits_or_there_is_no_buddy_allocator() {
        let mut arena = Arena::new(0x10_0000, 16);
        let b = arena.base;
        let map = arena.map(&[(b, b + 8 * K), (b + 12 * K, b + 13 * K)]);
        let frames = FrameAllocator::new(&map, arena.offset());
        assert_eq!(frames.buddy.bitmap, b + 7 * K);
        assert_eq!(frames.free_frames(), 8);
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
    fn allocations_for_programs_stop_at_the_reserve_and_the_kernel_s_do_not() {
        let mut arena = Arena::new(0x40_0000, 64);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 64 * K)]), arena.offset());
        let total = frames.free_frames();
        frames.set_user_reserve(total - 6);
        assert_eq!(frames.user_reserve(), total - 6);
        assert!(frames.user_may_take(6));
        assert!(!frames.user_may_take(7));
        assert_eq!(frames.allocate_contiguous_for_user(3), None);
        let block = frames.allocate_contiguous_for_user(2).unwrap();
        assert_eq!(block % (4 * K), 0);
        let user: Vec<u64> = core::iter::from_fn(|| frames.allocate_for_user()).collect();
        assert_eq!(user.len(), 2);
        assert_eq!(frames.free_frames(), frames.user_reserve());
        assert_eq!(frames.allocate_contiguous_for_user(MAX_ORDER + 1), None);
        let kernel = frames.allocate().unwrap();
        assert_eq!(frames.free_frames(), frames.user_reserve() - 1);
        assert_eq!(frames.allocate_for_user(), None);
        for frame in user.into_iter().chain([kernel]) {
            // SAFETY: every frame came from this allocator and is not used any more.
            unsafe { frames.free(frame) };
        }
        // SAFETY: allocated above with order 2, not used any more.
        unsafe { frames.free_contiguous(block, 2) };
        assert_eq!(frames.free_frames(), total);
        frames.set_user_reserve(0);
        assert_eq!(
            core::iter::from_fn(|| frames.allocate_for_user()).count() as u64,
            total
        );
    }

    #[test]
    fn the_reserve_is_a_thirty_second_of_memory_within_its_bounds() {
        assert_eq!(user_reserve_for(32_768), 1024);
        assert_eq!(user_reserve_for(30_000), 937);
        assert_eq!(user_reserve_for(8192), MIN_USER_RESERVE_FRAMES);
        assert_eq!(user_reserve_for(1 << 22), MAX_USER_RESERVE_FRAMES);
        assert_eq!(user_reserve_for(1000), 250, "never more than a quarter");
        assert_eq!(user_reserve_for(0), 0);
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

    #[test]
    #[should_panic(expected = "freed twice")]
    fn freeing_a_frame_twice_panics_in_debug_builds() {
        let mut arena = Arena::new(0x40_0000, 16);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 16 * K)]), arena.offset());
        let frame = frames.allocate().unwrap();
        // SAFETY: from this allocator, unused.
        unsafe { frames.free(frame) };
        // SAFETY: deliberately wrong: the check under test catches it.
        unsafe { frames.free(frame) };
    }

    #[test]
    fn programs_blocks_and_the_kernels_keep_to_their_own_pageblocks() {
        let mut arena = Arena::new(0x40_0000, 2048);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 2048 * K)]), arena.offset());
        let kernel = frames.allocate_contiguous_of_kind(1, BlockKind::Kernel).unwrap();
        let user = frames.allocate_contiguous_of_kind(1, BlockKind::User).unwrap();
        let pageblock = |frame: u64| frame >> (FRAME_SIZE.trailing_zeros() + PAGEBLOCK_ORDER);
        assert_ne!(pageblock(kernel), pageblock(user));
        let second = frames.allocate_contiguous_of_kind(1, BlockKind::User).unwrap();
        assert_eq!(pageblock(second), pageblock(user));
    }

    #[test]
    fn a_run_across_untouched_memory_and_freed_frames_still_forms_a_block() {
        let mut arena = Arena::new(0x40_0000, 64);
        let b = arena.base;
        let mut frames = FrameAllocator::new(&arena.map(&[(b, b + 64 * K)]), arena.offset());
        let total = frames.free_frames();
        let taken: Vec<u64> = (0..40).map(|_| frames.allocate().unwrap()).collect();
        for &frame in &taken {
            // SAFETY: from this allocator, unused.
            unsafe { frames.free(frame) };
        }
        let block = frames.allocate_contiguous(5).unwrap();
        assert_eq!(block % (32 * K), 0);
        assert_eq!(frames.free_frames(), total - 32);
    }

    #[test]
    fn a_sparse_map_keeps_metadata_for_its_ranges_only() {
        let mut map = MemoryMap::new();
        map.add_ram(PhysicalRange::from_start_length(0x10_0000, 16 * K));
        map.add_ram(PhysicalRange::from_start_length(0x100_0000_0000, 16 * K));
        assert_eq!(Buddy::bitmap_bytes(&map), K);
    }
}

//! Physical memory: the frame allocator.
//!
//! The first allocator is a bump allocator over the free ranges of the boot memory map plus a
//! stack of freed frames (D28). Building it costs nothing per frame, so boot time does not grow
//! with RAM size and no metadata memory is spent; allocating and freeing a frame are each a
//! handful of instructions. A frame never handed out is never touched. Freed frames are linked
//! through their own first word, which the kernel reaches through the platform's physical
//! memory offset. Contiguous multi-frame allocation (DMA, huge pages) comes with the buddy
//! allocator that replaces this one; the interface stays.

use crate::paging::PageTables;
use eggos_hal::memory::{FRAME_SIZE, MAX_RANGES};
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// The kernel's memory once its page tables are active: the frame allocator and the tables,
/// both reaching physical memory through the direct map.
#[derive(Debug)]
pub struct Memory<P: Platform> {
    pub frames: FrameAllocator,
    pub page_tables: PageTables<P::PageTableFormat>,
}

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
        Self {
            current,
            untouched,
            untouched_ranges: rest.len(),
            free_stack: NO_FRAME,
            free_frames: map.total() / FRAME_SIZE,
            physical_memory_offset,
        }
    }

    /// Where this allocator reaches physical memory: frame `p` at virtual `p + offset`.
    pub fn physical_memory_offset(&self) -> usize {
        self.physical_memory_offset
    }

    /// Reaches physical memory at `offset` from now on, as when the kernel switches to its own
    /// page tables: the free stack links hold physical addresses, so they stay valid as long as
    /// the new offset reaches the same memory.
    pub fn set_physical_memory_offset(&mut self, offset: usize) {
        self.physical_memory_offset = offset;
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
            self.next_untouched_range()?;
        }
        self.current.end -= FRAME_SIZE;
        self.free_frames -= 1;
        Some(self.current.end)
    }

    /// Moves on to the next untouched range; `None` when there is none.
    #[cold]
    fn next_untouched_range(&mut self) -> Option<()> {
        self.untouched_ranges = self.untouched_ranges.checked_sub(1)?;
        self.current = self.untouched[self.untouched_ranges];
        Some(())
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
        assert_eq!(frames.free_frames(), 6);
        let got: Vec<u64> = core::iter::from_fn(|| frames.allocate()).collect();
        assert_eq!(
            got,
            [9, 8, 3, 2, 1, 0].map(|i| b + i * K),
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
        assert_eq!(frames.free_frames(), 8);
        assert_eq!(frames.allocate(), Some(second));
        assert_eq!(frames.allocate(), Some(first));
        // Then untouched memory again.
        assert_eq!(frames.allocate(), Some(b + 5 * K));
        assert_eq!(frames.free_frames(), 5);
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
        let total = map.total() / K;
        let mut frames = FrameAllocator::new(&map, arena.offset());
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

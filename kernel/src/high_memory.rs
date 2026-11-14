//! High memory (D600): RAM beyond what the direct map reaches, on 32-bit machines (RV32 and
//! 32-bit Arm, whose direct map holds 896 MiB and 768 MiB). Only programs' anonymous pages use
//! it, so the kernel's own memory stays in the direct map. A high frame cannot hold a free-list
//! link the kernel reaches, so free high frames are a bitmap in low memory ([`HighFrames`]); the
//! kernel reaches a high frame only through a page of a per-CPU temporary window
//! ([`with_frame`]). Where the direct map covers all RAM nothing here runs: [`is_high`] is one
//! relaxed load and a compare.

use crate::paging::{MapError, PageTables};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::paging::{Attributes, Format};
use eggos_hal::{MemoryMap, Platform};

/// Temporary pages each CPU has: as deep as kernel code nests [`with_frame`] (a copy holds two).
pub const SLOTS_PER_CPU: usize = 4;
/// CPUs with temporary pages of their own; the others share one group, one CPU at a time.
pub const OWN_GROUPS_MAX: usize = 255;

const NO_OWNER: usize = usize::MAX;
const FRAME_BITS: u64 = 64;

static HIGH_START: AtomicU64 = AtomicU64::new(u64::MAX);
static WINDOW_START: AtomicU64 = AtomicU64::new(0);
static WINDOW_TABLE: AtomicUsize = AtomicUsize::new(0);
static OWN_GROUPS: AtomicUsize = AtomicUsize::new(0);
static DEPTHS: [AtomicU8; OWN_GROUPS_MAX] = [const { AtomicU8::new(0) }; OWN_GROUPS_MAX];
static SHARED_OWNER: AtomicUsize = AtomicUsize::new(NO_OWNER);
static SHARED_DEPTH: AtomicU8 = AtomicU8::new(0);
static SHARED_TAKEN: AtomicBool = AtomicBool::new(false);

/// Whether `frame` is high memory, reached only through [`with_frame`].
#[inline(always)]
pub fn is_high(frame: u64) -> bool {
    frame >= HIGH_START.load(Ordering::Relaxed)
}

/// Free high frames: one bit a frame from the lowest high frame to the highest, set while the
/// frame is free, in low memory the allocator took for good.
#[derive(Debug)]
pub struct HighFrames {
    start: u64,
    end: u64,
    bitmap: usize,
    words: usize,
    free: u64,
    total: u64,
    hint: usize,
}

impl HighFrames {
    /// The 64-bit words the bitmap over `map`'s frames takes.
    pub fn bitmap_words(map: &MemoryMap) -> usize {
        match (map.ranges().first(), map.ranges().last()) {
            (Some(first), Some(last)) => {
                ((last.end - first.start) / FRAME_SIZE).div_ceil(FRAME_BITS) as usize
            }
            _ => 0,
        }
    }

    /// Every frame of `map` free, its bitmap the [`Self::bitmap_words`] words at virtual
    /// `bitmap`.
    ///
    /// # Safety
    /// `bitmap` must be writable, aligned memory of that many words that nothing else uses,
    /// ever.
    pub unsafe fn new(map: &MemoryMap, bitmap: usize) -> Self {
        let words = Self::bitmap_words(map);
        let start = map.ranges().first().map_or(0, |range| range.start);
        let end = map.ranges().last().map_or(0, |range| range.end);
        let mut high = Self {
            start,
            end,
            bitmap,
            words,
            free: 0,
            total: 0,
            hint: 0,
        };
        for index in 0..words {
            *high.word(index) = 0;
        }
        for range in map.ranges() {
            let mut frame = range.start.next_multiple_of(FRAME_SIZE);
            while frame + FRAME_SIZE <= range.end {
                high.mark_free(frame);
                frame += FRAME_SIZE;
            }
        }
        high.total = high.free;
        high
    }

    fn word(&mut self, index: usize) -> &mut u64 {
        debug_assert!(index < self.words);
        // SAFETY: `new`'s caller gave the bitmap's words to this value for good, aligned and
        // writable; `index` is below their count.
        unsafe { &mut *(self.bitmap as *mut u64).add(index) }
    }

    fn mark_free(&mut self, frame: u64) {
        let bit = (frame - self.start) / FRAME_SIZE;
        *self.word((bit / FRAME_BITS) as usize) |= 1 << (bit % FRAME_BITS);
        self.free += 1;
    }

    /// Whether `frame` is one of its frames.
    pub fn contains(&self, frame: u64) -> bool {
        self.start <= frame && frame < self.end
    }

    /// A free frame, now the caller's; `None` when none is left.
    pub fn allocate(&mut self) -> Option<u64> {
        if self.free == 0 {
            return None;
        }
        for step in 0..self.words {
            let index = (self.hint + step) % self.words;
            let word = *self.word(index);
            if word == 0 {
                continue;
            }
            let bit = word.trailing_zeros() as u64;
            *self.word(index) = word & !(1 << bit);
            self.free -= 1;
            self.hint = index;
            return Some(self.start + (index as u64 * FRAME_BITS + bit) * FRAME_SIZE);
        }
        None
    }

    /// Gives `frame` back.
    ///
    /// # Safety
    /// `frame` must have come from [`Self::allocate`] and not been freed since, and nothing may
    /// use it any more.
    pub unsafe fn free(&mut self, frame: u64) {
        debug_assert!(self.contains(frame) && frame.is_multiple_of(FRAME_SIZE));
        let bit = (frame - self.start) / FRAME_SIZE;
        let index = (bit / FRAME_BITS) as usize;
        debug_assert!(*self.word(index) & (1 << (bit % FRAME_BITS)) == 0, "freed twice");
        self.mark_free(frame);
        self.hint = self.hint.min(index);
    }

    /// Its free frames.
    pub fn free_frames(&self) -> u64 {
        self.free
    }

    /// All its frames.
    pub fn total_frames(&self) -> u64 {
        self.total
    }
}

/// Publishes high memory from `start` on, for [`is_high`].
pub(crate) fn publish_start(start: u64) {
    HIGH_START.store(start, Ordering::Relaxed);
}

/// Sets up the temporary window at `virt` in `window`, the kernel area just below the device
/// window: one last-level table of `tables`, every entry empty, the window aligned to what that
/// table maps. Run on the boot CPU before any other starts, before any user address space
/// exists and before any high frame is handed out.
pub fn set_up_window<P: Platform>(
    tables: &mut PageTables<P::PageTableFormat>,
    window: &core::ops::Range<u64>,
    virt: u64,
    frames: &mut crate::memory::FrameAllocator,
) -> Result<(), MapError> {
    let table = tables.window_leaf_table(window, virt, frames)?;
    let entries = <P::PageTableFormat as Format>::entries(0);
    let groups = (entries / SLOTS_PER_CPU).saturating_sub(1).min(OWN_GROUPS_MAX);
    WINDOW_TABLE.store(
        (table as usize).wrapping_add(frames.physical_memory_offset()),
        Ordering::Relaxed,
    );
    WINDOW_START.store(virt, Ordering::Relaxed);
    OWN_GROUPS.store(groups, Ordering::Release);
    Ok(())
}

/// The bytes of the temporary window: one last-level table's reach.
pub fn window_bytes<P: Platform>() -> u64 {
    <P::PageTableFormat as Format>::leaf_bytes(1)
}

/// Calls `use_frame` with the address frame `frame` is reached at: the direct map's, or, for
/// high memory, a temporary page of this CPU's mapped to it for the call. `use_frame` must not
/// sleep or switch threads (the kernel runs with interrupts masked, D91, so nothing else on this
/// CPU takes the page meanwhile), and may nest at most [`SLOTS_PER_CPU`] deep.
#[inline(always)]
pub fn with_frame<P: Platform, R>(
    frame: u64,
    direct_map_offset: usize,
    use_frame: impl FnOnce(*mut u8) -> R,
) -> R {
    if !is_high(frame) {
        return use_frame((frame as usize).wrapping_add(direct_map_offset) as *mut u8);
    }
    with_high_frame::<P, R>(frame, use_frame)
}

/// As [`with_frame`] with two frames at once: a copy's source and destination.
#[inline(always)]
pub fn with_frames<P: Platform, R>(
    first: u64,
    second: u64,
    direct_map_offset: usize,
    use_frames: impl FnOnce(*mut u8, *mut u8) -> R,
) -> R {
    with_frame::<P, R>(first, direct_map_offset, |first| {
        with_frame::<P, R>(second, direct_map_offset, |second| use_frames(first, second))
    })
}

#[cold]
#[inline(never)]
fn with_high_frame<P: Platform, R>(frame: u64, use_frame: impl FnOnce(*mut u8) -> R) -> R {
    let groups = OWN_GROUPS.load(Ordering::Acquire);
    assert!(groups > 0, "a high frame before the temporary window exists");
    let cpu = crate::cpu::current::<P>().index();
    let (group, depth) = match cpu < groups {
        true => (cpu, &DEPTHS[cpu]),
        false => {
            take_shared_group(cpu);
            (groups, &SHARED_DEPTH)
        }
    };
    let level = depth.load(Ordering::Relaxed) as usize;
    assert!(level < SLOTS_PER_CPU, "temporary pages nested too deep");
    depth.store(level as u8 + 1, Ordering::Relaxed);
    let index = group * SLOTS_PER_CPU + level;
    let virt = WINDOW_START.load(Ordering::Relaxed) + index as u64 * FRAME_SIZE;
    set_entry::<P>(index, <P::PageTableFormat as Format>::leaf(frame, 0, Attributes::READ_WRITE));
    P::drop_kernel_translations(virt, FRAME_SIZE);
    let result = use_frame(virt as usize as *mut u8);
    set_entry::<P>(index, <P::PageTableFormat as Format>::empty(0));
    P::drop_kernel_translations(virt, FRAME_SIZE);
    depth.store(level as u8, Ordering::Relaxed);
    if cpu >= groups && level == 0 {
        SHARED_OWNER.store(NO_OWNER, Ordering::Relaxed);
        SHARED_TAKEN.store(false, Ordering::Release);
    }
    result
}

fn take_shared_group(cpu: usize) {
    if SHARED_OWNER.load(Ordering::Relaxed) == cpu {
        return;
    }
    while SHARED_TAKEN
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    SHARED_OWNER.store(cpu, Ordering::Relaxed);
}

fn set_entry<P: Platform>(index: usize, value: u64) {
    let table = WINDOW_TABLE.load(Ordering::Relaxed);
    let bytes = <P::PageTableFormat as Format>::ENTRY_BYTES;
    let place = table.wrapping_add(index * bytes);
    match bytes {
        // SAFETY: the window's last-level table (`set_up_window`) is the kernel's for good,
        // reached through the direct map; entry `index` is this CPU's slot, which nothing else
        // writes while this CPU holds it (its depth count, or the shared group's flag).
        // Volatile: the page-table walker reads it behind the compiler's back.
        4 => unsafe { (place as *mut u32).write_volatile(value as u32) },
        // SAFETY: as for the narrow entry.
        _ => unsafe { (place as *mut u64).write_volatile(value) },
    }
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_hal::PhysicalRange;

    fn high(map: &MemoryMap) -> HighFrames {
        let words = HighFrames::bitmap_words(map);
        let bitmap = alloc::vec![0u64; words.max(1)].leak();
        // SAFETY: the leaked words are this value's for good.
        unsafe { HighFrames::new(map, bitmap.as_mut_ptr() as usize) }
    }

    fn map(ranges: &[(u64, u64)]) -> MemoryMap {
        let mut map = MemoryMap::new();
        for &(start, end) in ranges {
            map.add_ram(PhysicalRange { start, end });
        }
        map
    }

    #[test]
    fn every_frame_of_the_ranges_is_free_and_nothing_between() {
        let map = map(&[(0x3800_0000, 0x3801_0000), (0x4000_0000, 0x4000_2000)]);
        let mut frames = high(&map);
        assert_eq!(frames.total_frames(), 18);
        let mut taken = alloc::vec::Vec::new();
        while let Some(frame) = frames.allocate() {
            assert!(
                (0x3800_0000..0x3801_0000).contains(&frame)
                    || (0x4000_0000..0x4000_2000).contains(&frame)
            );
            taken.push(frame);
        }
        assert_eq!(taken.len(), 18);
        assert_eq!(frames.free_frames(), 0);
        // SAFETY: the frame came from `allocate` and nothing uses it.
        unsafe { frames.free(taken[3]) };
        assert_eq!(frames.allocate(), Some(taken[3]));
    }

    #[test]
    fn no_high_memory_gives_no_frame() {
        let mut frames = high(&MemoryMap::new());
        assert_eq!(frames.total_frames(), 0);
        assert_eq!(frames.allocate(), None);
    }
}

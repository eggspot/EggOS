//! The physical memory map: which RAM the kernel may hand out.
//!
//! Each architecture module builds a [`MemoryMap`] at boot from what its firmware or loader
//! describes (the device tree on Arm and RISC-V, the PVH memory map on x86_64): it adds the
//! RAM ranges, then reserves everything that is in use (the kernel image and boot stack, the
//! firmware, the boot data the kernel still reads). What remains is free for the kernel's frame
//! allocator. The map has a fixed capacity and needs no allocator, because it is built before
//! there is one.

use core::fmt;

/// Size of the smallest page every supported architecture maps: the frame allocator's unit.
pub const FRAME_SIZE: u64 = 4096;

/// Most ranges a [`MemoryMap`] holds. Real machines describe a few to a few dozen RAM ranges;
/// when a reservation would need more, the map drops RAM rather than a reservation (see
/// [`MemoryMap::reserve`]), so running out only costs memory, never correctness.
pub const MAX_RANGES: usize = 64;

/// Physical addresses `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalRange {
    pub start: u64,
    pub end: u64,
}

impl PhysicalRange {
    /// `start..start + length`, saturating at the top of the address space.
    pub const fn from_start_length(start: u64, length: u64) -> Self {
        Self {
            start,
            end: start.saturating_add(length),
        }
    }

    pub const fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    /// The whole frames inside this range: start rounded up, end rounded down.
    const fn whole_frames(self) -> Self {
        let start = match self.start.checked_next_multiple_of(FRAME_SIZE) {
            Some(start) => start,
            None => u64::MAX,
        };
        Self {
            start,
            end: self.end - self.end % FRAME_SIZE,
        }
    }

    /// Every frame touching this range: start rounded down, end rounded up. An end in the last,
    /// partial frame of the address space rounds down instead, which loses nothing: a map holds
    /// whole frames only, so it has nothing there.
    pub const fn covering_frames(self) -> Self {
        let end = match self.end.checked_next_multiple_of(FRAME_SIZE) {
            Some(end) => end,
            None => u64::MAX - (FRAME_SIZE - 1),
        };
        Self {
            start: self.start - self.start % FRAME_SIZE,
            end,
        }
    }
}

impl fmt::Display for PhysicalRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}..{:#x}", self.start, self.end)
    }
}

/// Free physical memory: sorted, non-overlapping, non-adjacent, frame-aligned ranges.
#[derive(Clone)]
pub struct MemoryMap {
    ranges: [PhysicalRange; MAX_RANGES],
    len: usize,
}

impl fmt::Debug for MemoryMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ranges()).finish()
    }
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryMap {
    pub const fn new() -> Self {
        Self {
            ranges: [PhysicalRange { start: 0, end: 0 }; MAX_RANGES],
            len: 0,
        }
    }

    /// The free ranges, in ascending address order.
    pub fn ranges(&self) -> &[PhysicalRange] {
        &self.ranges[..self.len]
    }

    /// Total free bytes.
    pub fn total(&self) -> u64 {
        self.ranges().iter().map(PhysicalRange::len).sum()
    }

    /// Adds RAM. Only whole frames are kept; overlaps with RAM already added are merged. Add all
    /// RAM before reserving anything: a reservation only removes what is in the map when it is
    /// made. When the map is full, the smallest range (the new one included) is dropped.
    pub fn add_ram(&mut self, range: PhysicalRange) {
        let range = range.whole_frames();
        if range.is_empty() {
            return;
        }
        // Ranges that overlap or touch `range` merge into it.
        let first = self.ranges().partition_point(|r| r.end < range.start);
        let last = self.ranges().partition_point(|r| r.start <= range.end);
        if first < last {
            let merged = PhysicalRange {
                start: range.start.min(self.ranges[first].start),
                end: range.end.max(self.ranges[last - 1].end),
            };
            self.ranges[first] = merged;
            self.remove(first + 1..last);
            return;
        }
        if self.len == MAX_RANGES {
            let smallest = self.smallest();
            if self.ranges[smallest].len() >= range.len() {
                return;
            }
            self.remove(smallest..smallest + 1);
            return self.add_ram(range);
        }
        self.insert(first, range);
    }

    /// Removes every frame that `range` touches (rounded out to whole frames). When splitting a
    /// range would need more room than the map has, the smaller piece is dropped as well: the
    /// map may lose free memory but never hands out reserved memory.
    pub fn reserve(&mut self, range: PhysicalRange) {
        if range.is_empty() {
            return;
        }
        let range = range.covering_frames();
        let first = self.ranges().partition_point(|r| r.end <= range.start);
        let last = self.ranges().partition_point(|r| r.start < range.end);
        if first >= last {
            return;
        }
        let below = PhysicalRange {
            start: self.ranges[first].start,
            end: range.start,
        };
        let above = PhysicalRange {
            start: range.end,
            end: self.ranges[last - 1].end,
        };
        self.remove(first..last);
        let mut at = first;
        for piece in [below, above] {
            if piece.is_empty() {
                continue;
            }
            if self.len == MAX_RANGES {
                let smallest = self.smallest();
                if self.ranges[smallest].len() >= piece.len() {
                    continue;
                }
                self.remove(smallest..smallest + 1);
                if smallest < at {
                    at -= 1;
                }
            }
            self.insert(at, piece);
            at += 1;
        }
    }

    /// Reserves everything outside `allowed` (for example, memory the kernel cannot reach yet).
    pub fn retain_within(&mut self, allowed: PhysicalRange) {
        self.reserve(PhysicalRange {
            start: 0,
            end: allowed.start,
        });
        self.reserve(PhysicalRange {
            start: allowed.end,
            end: u64::MAX,
        });
    }

    fn smallest(&self) -> usize {
        let mut smallest = 0;
        for (i, range) in self.ranges().iter().enumerate() {
            if range.len() < self.ranges[smallest].len() {
                smallest = i;
            }
        }
        smallest
    }

    fn insert(&mut self, at: usize, range: PhysicalRange) {
        debug_assert!(self.len < MAX_RANGES);
        self.ranges.copy_within(at..self.len, at + 1);
        self.ranges[at] = range;
        self.len += 1;
    }

    fn remove(&mut self, indices: core::ops::Range<usize>) {
        self.ranges
            .copy_within(indices.end..self.len, indices.start);
        self.len -= indices.len();
    }
}

/// Zeroes the `length` bytes at `start` the fastest way the CPU has (D56): frames and page
/// tables the kernel hands out are zeroed with it. On x86_64, `rep stosq` (the compiler's
/// `memset`); elsewhere, [`zero_by_lines`], where the compiler's `memset` is a loop of single
/// word stores.
///
/// # Safety
/// The range must be valid for writes and not used by anything else meanwhile.
#[inline]
pub unsafe fn zero(start: *mut u8, length: usize) {
    // SAFETY: guaranteed by the caller.
    #[cfg(target_os = "none")]
    unsafe {
        crate::arch::zero(start, length)
    };
    // SAFETY: guaranteed by the caller.
    #[cfg(not(target_os = "none"))]
    unsafe {
        zero_by_lines(start, length)
    };
}

/// Zeroes the `length` bytes at `start` a 64-byte line at a time, with eight 8-byte stores,
/// where the range is so aligned, and the ragged ends byte by byte: a frame costs about a
/// quarter of what a loop of single word stores does. The stores are volatile only so that
/// the compiler cannot turn the loop back into a `memset` call.
///
/// # Safety
/// As for [`zero`].
pub unsafe fn zero_by_lines(start: *mut u8, length: usize) {
    const LINE: usize = 64;
    let end = start.wrapping_add(length);
    let head = (start.addr().next_multiple_of(LINE) - start.addr()).min(length);
    // SAFETY: the first `head` bytes of the range, which the caller lets this code write.
    unsafe { start.write_bytes(0, head) };
    let mut line = start.wrapping_add(head);
    while end.addr() - line.addr() >= LINE {
        let words = line.cast::<u64>();
        for word in 0..LINE / 8 {
            // SAFETY: a word of a whole 64-byte line of the range, aligned to 64 bytes, so the
            // word is aligned and written by this code alone.
            unsafe { words.wrapping_add(word).write_volatile(0) };
        }
        line = line.wrapping_add(LINE);
    }
    // SAFETY: the bytes after the last whole line, the rest of the range.
    unsafe { line.write_bytes(0, end.addr() - line.addr()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_by_lines_clears_exactly_its_range() {
        for (start, length) in [
            (0, 4096),
            (0, 0),
            (5, 3),
            (1, 63),
            (3, 200),
            (64, 128),
            (60, 4000),
        ] {
            let mut buffer = vec![0xa5u8; 4096 + 128];
            // From a 64-byte boundary inside the buffer, so `start` sets the alignment.
            let base = buffer.as_ptr().addr().next_multiple_of(64) - buffer.as_ptr().addr();
            // SAFETY: the range is inside `buffer`, which nothing else uses.
            unsafe { zero_by_lines(buffer.as_mut_ptr().wrapping_add(base + start), length) };
            for (index, &byte) in buffer.iter().enumerate() {
                let inside = (base + start..base + start + length).contains(&index);
                assert_eq!(
                    byte,
                    if inside { 0 } else { 0xa5 },
                    "{start}+{length} at {index}"
                );
            }
        }
    }

    const K: u64 = FRAME_SIZE;

    fn range(start: u64, end: u64) -> PhysicalRange {
        PhysicalRange { start, end }
    }

    fn map_of(ram: &[PhysicalRange]) -> MemoryMap {
        let mut map = MemoryMap::new();
        for &r in ram {
            map.add_ram(r);
        }
        map
    }

    #[test]
    fn ram_is_trimmed_to_whole_frames_and_sorted() {
        // Less than one whole frame: nothing is kept.
        let map = map_of(&[range(10 * K, 20 * K), range(K + 1, 3 * K - 1)]);
        assert_eq!(map.ranges(), &[range(10 * K, 20 * K)]);
        let map = map_of(&[range(10 * K, 20 * K), range(K - 1, 3 * K + 1)]);
        assert_eq!(map.ranges(), &[range(K, 3 * K), range(10 * K, 20 * K)]);
        assert_eq!(map.total(), 12 * K);
    }

    #[test]
    fn overlapping_and_touching_ram_merges() {
        let map = map_of(&[
            range(0, 4 * K),
            range(8 * K, 12 * K),
            range(4 * K, 5 * K),
            range(3 * K, 9 * K),
        ]);
        assert_eq!(map.ranges(), &[range(0, 12 * K)]);
    }

    #[test]
    fn reservation_is_rounded_out_and_splits_a_range() {
        let mut map = map_of(&[range(0, 16 * K)]);
        map.reserve(range(4 * K + 1, 6 * K - 1));
        assert_eq!(map.ranges(), &[range(0, 4 * K), range(6 * K, 16 * K)]);
    }

    #[test]
    fn reservation_spanning_several_ranges_trims_both_ends() {
        let mut map = map_of(&[range(0, 4 * K), range(8 * K, 12 * K), range(16 * K, 20 * K)]);
        map.reserve(range(2 * K, 17 * K));
        assert_eq!(map.ranges(), &[range(0, 2 * K), range(17 * K, 20 * K)]);
    }

    #[test]
    fn reservation_outside_ram_or_empty_changes_nothing() {
        let mut map = map_of(&[range(8 * K, 12 * K)]);
        map.reserve(range(0, 8 * K));
        map.reserve(range(12 * K, u64::MAX));
        map.reserve(range(9 * K, 9 * K));
        assert_eq!(map.ranges(), &[range(8 * K, 12 * K)]);
    }

    #[test]
    fn reservation_at_the_top_of_the_address_space_does_not_overflow() {
        let top = u64::MAX - u64::MAX % K;
        let mut map = map_of(&[range(top - 4 * K, u64::MAX)]);
        assert_eq!(map.ranges(), &[range(top - 4 * K, top)]);
        map.reserve(range(top - K, u64::MAX));
        assert_eq!(map.ranges(), &[range(top - 4 * K, top - K)]);
    }

    #[test]
    fn retain_within_drops_everything_outside() {
        let mut map = map_of(&[range(0, 4 * K), range(8 * K, 16 * K)]);
        map.retain_within(range(K, 10 * K));
        assert_eq!(map.ranges(), &[range(K, 4 * K), range(8 * K, 10 * K)]);
    }

    #[test]
    fn full_map_drops_smallest_ram_not_reservations() {
        let mut map = MemoryMap::new();
        // MAX_RANGES ranges of one frame each, two frames apart, then one big range.
        for i in 0..MAX_RANGES as u64 - 1 {
            map.add_ram(range(2 * i * K, (2 * i + 1) * K));
        }
        let big = range(1 << 40, (1 << 40) + 100 * K);
        map.add_ram(big);
        assert_eq!(map.ranges().len(), MAX_RANGES);
        // A tiny range is not worth evicting anything for.
        map.add_ram(range(1 << 30, (1 << 30) + K));
        assert_eq!(map.ranges().len(), MAX_RANGES);
        assert!(!map.ranges().contains(&range(1 << 30, (1 << 30) + K)));
        // Splitting the big range needs one more slot: a one-frame range goes instead.
        map.reserve(range(big.start + 10 * K, big.start + 11 * K));
        assert_eq!(map.ranges().len(), MAX_RANGES);
        assert!(map.ranges().contains(&range(big.start, big.start + 10 * K)));
        assert!(map.ranges().contains(&range(big.start + 11 * K, big.end)));
        assert!(map.ranges().windows(2).all(|w| w[0].end < w[1].start));
        // Every reservation is honoured, even when the map is full.
        for r in map.ranges() {
            assert!(r.end <= big.start + 10 * K || r.start >= big.start + 11 * K);
        }
    }

    #[test]
    fn random_operations_keep_the_invariants() {
        // A small xorshift generator, so the test is deterministic without a crate.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut map = MemoryMap::new();
        for _ in 0..200 {
            let start = next() % (1 << 30);
            map.add_ram(PhysicalRange::from_start_length(start, next() % (1 << 24)));
        }
        let mut reserved = Vec::new();
        for _ in 0..2000 {
            let start = next() % (1 << 30);
            let r = PhysicalRange::from_start_length(start, next() % (1 << 16));
            map.reserve(r);
            reserved.push(r);
            let ranges = map.ranges();
            assert!(
                ranges
                    .iter()
                    .all(|r| !r.is_empty() && r.start % FRAME_SIZE == 0 && r.end % FRAME_SIZE == 0)
            );
            assert!(ranges.windows(2).all(|w| w[0].end < w[1].start));
            for res in reserved.iter().filter(|r| !r.is_empty()) {
                assert!(
                    ranges
                        .iter()
                        .all(|r| r.end <= res.start || r.start >= res.end)
                );
            }
        }
    }
}

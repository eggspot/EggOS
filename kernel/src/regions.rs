//! The regions of a process's user space its program may use (D52): what Linux calls its
//! memory mappings (`/proc/<pid>/maps`), each a page-aligned range with the access the
//! program has to it.
//!
//! A region says what may be mapped, not what is: its pages are mapped as the program first
//! touches them (demand paging), each a zeroed frame of its own, unless they were mapped up
//! front (a program's segments, the stack's arguments). A page fault in a region the access is
//! allowed in maps the page; anywhere else it ends the program.
//!
//! [`Regions`] keeps them sorted and disjoint in one `Vec`: a static program has a handful,
//! and a binary search over contiguous memory beats a tree at that size (decided by research,
//! D52; revisit past about a thousand, as JVMs and glibc arenas reach).
//!
//! The memory calls (D53) change them while the program runs: [`Regions::remove`] takes a
//! range out (`munmap`), splitting the regions at its ends, [`Regions::protect`] gives a
//! range other attributes (`mprotect`), and [`Regions::free_below`] finds room for a new one
//! (`mmap`). A region whose pages the program may not touch at all (`PROT_NONE`) has no
//! attributes.
//!
//! Regions know what backs them (D167): a part of user space whose pages come from a file is a
//! [`FileRange`] in the process's [`FileRanges`], kept apart from the regions, so the regions'
//! hot paths stay as they were. A page there is read from the file when first touched, and
//! again after `MADV_DONTNEED`; every other page of a region is zero when first touched.

use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_hal::memory::FRAME_SIZE;
pub use eggos_hal::paging::Access;
use eggos_hal::paging::Attributes;

/// One region: user addresses `start..end`, page-aligned, whose pages are mapped with
/// `attributes`, or, with `None`, may not be touched by the program at all (`PROT_NONE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub attributes: Option<Attributes>,
}

/// Why a region could not be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    /// The range is empty, not page-aligned or not user attributes.
    Invalid,
    /// Part of the range is in another region already.
    Overlaps,
    /// Part of the range is in no region ([`Regions::protect`]).
    NotCovered,
    /// No range of addresses free and large enough ([`Regions::free_below`]).
    NoRoom,
    /// No frame for a page table (`Process::remap_region`).
    OutOfMemory,
    /// The range holds frames lent read-only, which cannot be made writable (`mprotect` of a
    /// private memfd mapping, D139).
    NotWritable,
}

/// A process's regions, sorted and disjoint. See the module documentation.
#[derive(Debug, Default)]
pub struct Regions {
    regions: Vec<Region>,
}

impl Regions {
    pub const fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// Adds `start..end` with `attributes`, joined to a neighbour it touches with the same
    /// attributes (as Linux merges them), so the list stays short.
    #[inline(always)]
    pub fn insert(
        &mut self,
        start: u64,
        end: u64,
        attributes: Option<Attributes>,
    ) -> Result<(), RegionError> {
        if !is_page_range(start, end) || attributes.is_some_and(|attributes| !attributes.user) {
            return Err(RegionError::Invalid);
        }
        // The first region that ends after `start`: the only one that could overlap from
        // below, and where the new one goes.
        let at = self.regions.partition_point(|region| region.end <= start);
        if self.regions.get(at).is_some_and(|next| next.start < end) {
            return Err(RegionError::Overlaps);
        }
        let joins_previous = at > 0 && {
            let previous = &self.regions[at - 1];
            previous.end == start && previous.attributes == attributes
        };
        let joins_next = self
            .regions
            .get(at)
            .is_some_and(|next| next.start == end && next.attributes == attributes);
        match (joins_previous, joins_next) {
            (true, true) => {
                self.regions[at - 1].end = self.regions[at].end;
                self.regions.remove(at);
            }
            (true, false) => self.regions[at - 1].end = end,
            (false, true) => self.regions[at].start = start,
            (false, false) => self.regions.insert(
                at,
                Region {
                    start,
                    end,
                    attributes,
                },
            ),
        }
        Ok(())
    }

    /// The region that holds `address`, if any.
    #[inline]
    pub fn find(&self, address: u64) -> Option<&Region> {
        let at = self.regions.partition_point(|region| region.end <= address);
        self.regions
            .get(at)
            .filter(|region| region.start <= address)
    }

    /// A copy of every region, for a new process's memory (`fork`); `None` without memory for
    /// it.
    pub fn try_copy(&self) -> Option<Self> {
        let mut regions = Vec::new();
        regions.try_reserve_exact(self.regions.len()).ok()?;
        regions.extend_from_slice(&self.regions);
        Some(Self { regions })
    }

    /// Every region, in address order.
    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    /// The regions that hold part of `start..end`, in address order.
    pub fn overlapping(&self, start: u64, end: u64) -> &[Region] {
        let first = self.regions.partition_point(|region| region.end <= start);
        let last = self.regions.partition_point(|region| region.start < end);
        &self.regions[first..last.max(first)]
    }

    /// Whether no region holds any of `start..end`.
    pub fn is_free(&self, start: u64, end: u64) -> bool {
        let at = self.regions.partition_point(|region| region.end <= start);
        self.regions.get(at).is_none_or(|next| next.start >= end)
    }

    /// Takes `start..end`, page-aligned, out of every region: a region it covers goes, one it
    /// cuts keeps what lies outside it (two regions, if it cuts one in the middle). Hands each
    /// part taken out to `removed`, in address order. Parts of the range in no region are
    /// fine, as `munmap` allows; an empty or unaligned range is [`RegionError::Invalid`].
    pub fn remove(
        &mut self,
        start: u64,
        end: u64,
        mut removed: impl FnMut(Region),
    ) -> Result<(), RegionError> {
        if !is_page_range(start, end) {
            return Err(RegionError::Invalid);
        }
        let first = self.regions.partition_point(|region| region.end <= start);
        let last = self.regions.partition_point(|region| region.start < end);
        if first == last {
            return Ok(());
        }
        for region in &self.regions[first..last] {
            removed(Region {
                start: region.start.max(start),
                end: region.end.min(end),
                attributes: region.attributes,
            });
        }
        // What survives at either end: the first region's part below the range and the last
        // one's above it (the same region's two parts, if the range is inside one).
        let below = Region {
            end: start,
            ..self.regions[first]
        };
        let above = Region {
            start: end,
            ..self.regions[last - 1]
        };
        // Written out rather than a `splice`, whose general machinery costs more than the
        // whole of `munmap`'s usual case (one region removed whole).
        let keeps_below = below.start < below.end;
        let keeps_above = above.start < above.end;
        let kept_from = first + usize::from(keeps_below);
        let kept_to = last - usize::from(keeps_above);
        if keeps_below {
            self.regions[first] = below;
        }
        if kept_from <= kept_to {
            if keeps_above {
                self.regions[last - 1] = above;
            }
            self.regions.drain(kept_from..kept_to);
        } else {
            // The range is inside one region, which leaves two parts: the region now holds
            // the lower one.
            self.regions.insert(last, above);
        }
        Ok(())
    }

    /// Gives `start..end`, page-aligned, `attributes` (see [`Region`]), splitting the regions
    /// at its ends and joining what then touches with the same attributes. Hands each part as
    /// it was to `changed`, in address order. If part of the range is in no region, only the
    /// part before that hole changes (none, if `start` is in none) and the error is
    /// [`RegionError::NotCovered`], as Linux's `mprotect` fails with `ENOMEM`.
    pub fn protect(
        &mut self,
        start: u64,
        end: u64,
        attributes: Option<Attributes>,
        changed: impl FnMut(Region),
    ) -> Result<(), RegionError> {
        if !is_page_range(start, end) || attributes.is_some_and(|attributes| !attributes.user) {
            return Err(RegionError::Invalid);
        }
        let first = self.regions.partition_point(|region| region.end <= start);
        let mut covered = start;
        for region in &self.regions[first..] {
            if region.start > covered || covered >= end {
                break;
            }
            covered = region.end;
        }
        let changed_end = covered.min(end);
        if changed_end > start {
            self.remove(start, changed_end, changed)?;
            self.insert(start, changed_end, attributes)?;
        }
        if changed_end < end {
            return Err(RegionError::NotCovered);
        }
        Ok(())
    }

    /// The highest page-aligned start of `length` bytes of addresses no region holds, at or
    /// above `floor` and ending at or below `top`: where `mmap` places a mapping it is free to
    /// place, top-down as Linux does. `None` if there is no such gap.
    pub fn free_below(&self, top: u64, floor: u64, length: u64) -> Option<u64> {
        let top = top & !(FRAME_SIZE - 1);
        let floor = floor.next_multiple_of(FRAME_SIZE);
        let fits = |gap_start: u64, gap_end: u64| {
            let gap_end = gap_end.min(top);
            let gap_start = gap_start.max(floor);
            (gap_end >= gap_start && gap_end - gap_start >= length).then(|| gap_end - length)
        };
        // Gaps from the top down: above the last region below `top`, then between regions.
        let below_top = self.regions.partition_point(|region| region.start < top);
        let mut gap_end = top;
        for region in self.regions[..below_top].iter().rev() {
            if let Some(start) = fits(region.end, gap_end) {
                return Some(start);
            }
            gap_end = region.start;
            if gap_end <= floor {
                return None;
            }
        }
        fits(floor, gap_end)
    }
}

/// Whether `start..end` is a non-empty, page-aligned range.
pub fn is_page_range(start: u64, end: u64) -> bool {
    start < end && (start | end).is_multiple_of(FRAME_SIZE)
}

/// A file that a [`FileRange`]'s pages are read from: a node of a file system (D167).
pub trait FilePages: Send + Sync {
    /// Copies the file's bytes from `offset` into `buffer`: how many, fewer at its end and 0
    /// past it; `None` if the storage under the file failed.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Option<usize>;
}

/// [`FileRange::file_bytes_end`] of a range that maps the whole file: its pages past the
/// file's end are not the file's (`SIGBUS`), as an `mmap` of a file has them.
pub const WHOLE_FILE: u64 = u64::MAX;

/// User addresses `start..end`, page-aligned, whose pages hold `file` from `offset`, a page
/// boundary of the file: a private file mapping (`mmap`) or a program's segment (`execve`).
#[derive(Clone)]
pub struct FileRange {
    pub start: u64,
    pub end: u64,
    /// Where `start` is in the file.
    pub offset: u64,
    /// Where the file's bytes stop counting: bytes at or past this offset of the file read as
    /// zero, and no page reaches past the file's end (a segment's file bytes, then its
    /// `.bss`); [`WHOLE_FILE`] for an `mmap`.
    pub file_bytes_end: u64,
    /// Whether the program may make the pages writable (`mprotect`): `false` for a shared
    /// mapping, whose pages are the file's as it was read.
    pub writable: bool,
    pub file: Arc<dyn FilePages>,
}

impl FileRange {
    /// Where page `page` of the range is in the file.
    pub fn offset_of(&self, page: u64) -> u64 {
        self.offset + (page - self.start)
    }

    /// How many bytes of page `page` of the range come from the file: a page's worth, fewer
    /// where [`Self::file_bytes_end`] cuts it.
    pub fn file_bytes_in(&self, page: u64) -> u64 {
        self.file_bytes_end
            .saturating_sub(self.offset_of(page))
            .min(FRAME_SIZE)
    }

    fn clipped(&self, start: u64, end: u64) -> Self {
        let from = self.start.max(start);
        Self {
            start: from,
            end: self.end.min(end),
            offset: self.offset_of(from),
            ..self.clone()
        }
    }
}

impl core::fmt::Debug for FileRange {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FileRange")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("offset", &self.offset)
            .field("file_bytes_end", &self.file_bytes_end)
            .field("writable", &self.writable)
            .finish_non_exhaustive()
    }
}

/// A process's [`FileRange`]s, sorted and disjoint, each within its regions. Removing,
/// moving and copying them follows what the regions' calls do to the same addresses.
#[derive(Debug, Default)]
pub struct FileRanges {
    ranges: Vec<FileRange>,
}

impl FileRanges {
    pub const fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Every range, in address order.
    pub fn iter(&self) -> impl Iterator<Item = &FileRange> {
        self.ranges.iter()
    }

    /// The range that holds `address`, if any.
    pub fn find(&self, address: u64) -> Option<&FileRange> {
        let at = self.ranges.partition_point(|range| range.end <= address);
        self.ranges.get(at).filter(|range| range.start <= address)
    }

    fn bounds(&self, start: u64, end: u64) -> (usize, usize) {
        let first = self.ranges.partition_point(|range| range.end <= start);
        let last = self.ranges.partition_point(|range| range.start < end);
        (first, last.max(first))
    }

    /// Whether any range holds part of `start..end`.
    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        let (first, last) = self.bounds(start, end);
        first < last
    }

    /// Whether a range that may not be made writable holds part of `start..end`.
    pub fn read_only_in(&self, start: u64, end: u64) -> bool {
        let (first, last) = self.bounds(start, end);
        self.ranges[first..last].iter().any(|range| !range.writable)
    }

    /// Adds `range`, page-aligned, where no range is: [`RegionError::Invalid`] for an empty or
    /// unaligned one, [`RegionError::Overlaps`] where one is, [`RegionError::OutOfMemory`]
    /// without memory for it.
    pub fn insert(&mut self, range: FileRange) -> Result<(), RegionError> {
        if !is_page_range(range.start, range.end) || !range.offset.is_multiple_of(FRAME_SIZE) {
            return Err(RegionError::Invalid);
        }
        let at = self
            .ranges
            .partition_point(|other| other.end <= range.start);
        if self
            .ranges
            .get(at)
            .is_some_and(|next| next.start < range.end)
        {
            return Err(RegionError::Overlaps);
        }
        self.ranges
            .try_reserve(1)
            .map_err(|_| RegionError::OutOfMemory)?;
        self.ranges.insert(at, range);
        Ok(())
    }

    /// Takes `start..end`, page-aligned, out of every range, keeping what lies outside it of
    /// the ranges it cuts. Nothing changes unless it all can: [`RegionError::Invalid`] for an
    /// empty or unaligned range, [`RegionError::OutOfMemory`] when a cut needs memory there is
    /// none of.
    pub fn remove(&mut self, start: u64, end: u64) -> Result<(), RegionError> {
        if !is_page_range(start, end) {
            return Err(RegionError::Invalid);
        }
        let (first, last) = self.bounds(start, end);
        if first == last {
            return Ok(());
        }
        let lowest = &self.ranges[first];
        let highest = &self.ranges[last - 1];
        let below = (lowest.start < start).then(|| lowest.clipped(lowest.start, start));
        let above = (highest.end > end).then(|| highest.clipped(end, highest.end));
        let kept = usize::from(below.is_some()) + usize::from(above.is_some());
        if kept > last - first {
            self.ranges
                .try_reserve(kept - (last - first))
                .map_err(|_| RegionError::OutOfMemory)?;
        }
        self.ranges
            .splice(first..last, below.into_iter().chain(above));
        Ok(())
    }

    /// A copy of every range, for a new process's memory (`fork`); `None` without memory.
    pub fn try_copy(&self) -> Option<Self> {
        let mut ranges = Vec::new();
        ranges.try_reserve_exact(self.ranges.len()).ok()?;
        ranges.extend(self.ranges.iter().cloned());
        Some(Self { ranges })
    }

    /// The first half of moving `old_length` bytes from `start` to `to` with `new_length`
    /// bytes there (`mremap`, the [`crate::process::Process::remap_region`] that goes with it):
    /// the parts of the ranges in the old range, moved, the one that reached its end grown to
    /// the new end, as a file mapping that grows maps more of its file. Takes the memory
    /// [`Self::finish_move`] needs, so that cannot run out; changes nothing.
    pub fn moved_parts(
        &mut self,
        start: u64,
        old_length: u64,
        to: u64,
        new_length: u64,
    ) -> Result<Vec<FileRange>, RegionError> {
        let old_end = start + old_length;
        let (first, last) = self.bounds(start, old_end);
        let mut moved = Vec::new();
        moved
            .try_reserve_exact(last - first)
            .map_err(|_| RegionError::OutOfMemory)?;
        for range in &self.ranges[first..last] {
            let mut part = range.clipped(start, old_end);
            let reaches_end = part.end == old_end;
            part.start = part.start - start + to;
            part.end = if reaches_end {
                to + new_length
            } else {
                part.end - start + to
            };
            moved.push(part);
        }
        self.ranges
            .try_reserve(moved.len() + 1)
            .map_err(|_| RegionError::OutOfMemory)?;
        Ok(moved)
    }

    /// The second half of a move ([`Self::moved_parts`]): takes the old range out unless
    /// `keep_source`, then adds `moved` where it went.
    pub fn finish_move(
        &mut self,
        start: u64,
        old_end: u64,
        moved: Vec<FileRange>,
        keep_source: bool,
    ) -> Result<(), RegionError> {
        if !keep_source {
            self.remove(start, old_end)?;
        }
        for part in moved {
            self.insert(part)?;
        }
        Ok(())
    }
}

/// A file held in memory, for tests of what reads [`FilePages`].
#[cfg(test)]
#[derive(Debug)]
pub struct BytesFile(pub Vec<u8>);

#[cfg(test)]
impl FilePages for BytesFile {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Option<usize> {
        let from = usize::try_from(offset).ok()?.min(self.0.len());
        let count = buffer.len().min(self.0.len() - from);
        buffer[..count].copy_from_slice(&self.0[from..from + count]);
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u64 = FRAME_SIZE;

    fn ranges(regions: &Regions) -> Vec<(u64, u64)> {
        regions
            .iter()
            .map(|region| (region.start, region.end))
            .collect()
    }

    #[test]
    fn regions_stay_sorted_and_join_neighbours_with_the_same_attributes() {
        let mut regions = Regions::new();
        let data = Some(Attributes::USER_READ_WRITE);
        let text = Some(Attributes::USER_TEXT);
        regions.insert(10 * PAGE, 12 * PAGE, data).unwrap();
        regions.insert(2 * PAGE, 4 * PAGE, text).unwrap();
        regions.insert(20 * PAGE, 21 * PAGE, data).unwrap();
        assert_eq!(
            ranges(&regions),
            [
                (2 * PAGE, 4 * PAGE),
                (10 * PAGE, 12 * PAGE),
                (20 * PAGE, 21 * PAGE)
            ]
        );
        // Touching a neighbour with other attributes: kept apart.
        regions.insert(4 * PAGE, 5 * PAGE, data).unwrap();
        // Touching both with the same: one region.
        regions.insert(12 * PAGE, 20 * PAGE, data).unwrap();
        // Touching the previous only, then the next only.
        regions.insert(5 * PAGE, 6 * PAGE, data).unwrap();
        regions.insert(PAGE, 2 * PAGE, text).unwrap();
        assert_eq!(
            ranges(&regions),
            [
                (PAGE, 4 * PAGE),
                (4 * PAGE, 6 * PAGE),
                (10 * PAGE, 21 * PAGE)
            ]
        );
        assert_eq!(regions.find(3 * PAGE + 5).unwrap().attributes, text);
        assert_eq!(regions.find(4 * PAGE).unwrap().attributes, data);
        assert_eq!(regions.find(21 * PAGE - 1).unwrap().start, 10 * PAGE);
        for unmapped in [0, PAGE - 1, 6 * PAGE, 10 * PAGE - 1, 21 * PAGE, u64::MAX] {
            assert_eq!(regions.find(unmapped), None, "{unmapped:#x}");
        }
    }

    #[test]
    fn overlapping_empty_unaligned_and_kernel_regions_are_refused() {
        let mut regions = Regions::new();
        let data = Some(Attributes::USER_READ_WRITE);
        regions.insert(4 * PAGE, 8 * PAGE, data).unwrap();
        for (start, end) in [
            (3 * PAGE, 5 * PAGE),
            (7 * PAGE, 9 * PAGE),
            (5 * PAGE, 6 * PAGE),
            (0, 10 * PAGE),
        ] {
            assert_eq!(
                regions.insert(start, end, data),
                Err(RegionError::Overlaps),
                "{start:#x}..{end:#x}"
            );
        }
        for (start, end) in [(PAGE, PAGE), (2 * PAGE, PAGE), (PAGE + 1, 2 * PAGE)] {
            assert_eq!(regions.insert(start, end, data), Err(RegionError::Invalid));
        }
        assert_eq!(
            regions.insert(PAGE, 2 * PAGE, Some(Attributes::READ_WRITE)),
            Err(RegionError::Invalid)
        );
        assert_eq!(ranges(&regions), [(4 * PAGE, 8 * PAGE)]);
    }

    #[test]
    fn accesses_follow_the_attributes() {
        for (attributes, write, execute) in [
            (Attributes::USER_READ_ONLY, false, false),
            (Attributes::USER_READ_WRITE, true, false),
            (Attributes::USER_TEXT, false, true),
        ] {
            assert!(Access::Read.allowed_by(attributes));
            assert_eq!(Access::Write.allowed_by(attributes), write);
            assert_eq!(Access::Execute.allowed_by(attributes), execute);
        }
    }

    /// Regions as (start page, end page, attributes), for short expectations.
    fn layout(regions: &Regions) -> Vec<(u64, u64, Option<Attributes>)> {
        regions
            .iter()
            .map(|region| (region.start / PAGE, region.end / PAGE, region.attributes))
            .collect()
    }

    #[test]
    fn removing_a_range_splits_the_regions_it_cuts_and_reports_what_went() {
        let data = Some(Attributes::USER_READ_WRITE);
        let text = Some(Attributes::USER_TEXT);
        let mut regions = Regions::new();
        regions.insert(2 * PAGE, 6 * PAGE, text).unwrap();
        regions.insert(8 * PAGE, 16 * PAGE, data).unwrap();
        regions.insert(20 * PAGE, 22 * PAGE, None).unwrap();
        let mut removed = Vec::new();
        // The middle of one region: two parts left.
        regions
            .remove(10 * PAGE, 12 * PAGE, |part| removed.push(part))
            .unwrap();
        assert_eq!(
            layout(&regions),
            [(2, 6, text), (8, 10, data), (12, 16, data), (20, 22, None)]
        );
        assert_eq!(
            removed,
            [Region {
                start: 10 * PAGE,
                end: 12 * PAGE,
                attributes: data
            }]
        );
        // Across several, with gaps: the top of one, two whole, the bottom of the last.
        removed.clear();
        regions
            .remove(4 * PAGE, 21 * PAGE, |part| removed.push(part))
            .unwrap();
        assert_eq!(layout(&regions), [(2, 4, text), (21, 22, None)]);
        assert_eq!(
            removed
                .iter()
                .map(|part| (part.start / PAGE, part.end / PAGE))
                .collect::<Vec<_>>(),
            [(4, 6), (8, 10), (12, 16), (20, 21)]
        );
        // Nothing there: fine, nothing reported.
        removed.clear();
        regions
            .remove(30 * PAGE, 40 * PAGE, |part| removed.push(part))
            .unwrap();
        assert!(removed.is_empty());
        // One region's bottom, one whole, one's top, then everything.
        regions.remove(2 * PAGE, 3 * PAGE, |_| ()).unwrap();
        assert_eq!(layout(&regions), [(3, 4, text), (21, 22, None)]);
        regions.remove(21 * PAGE, 22 * PAGE, |_| ()).unwrap();
        assert_eq!(layout(&regions), [(3, 4, text)]);
        regions.insert(6 * PAGE, 10 * PAGE, data).unwrap();
        regions.remove(9 * PAGE, 10 * PAGE, |_| ()).unwrap();
        assert_eq!(layout(&regions), [(3, 4, text), (6, 9, data)]);
        regions.remove(PAGE, 40 * PAGE, |_| ()).unwrap();
        assert!(layout(&regions).is_empty());
        for (start, end) in [(PAGE, PAGE), (PAGE + 1, 2 * PAGE), (3 * PAGE, PAGE)] {
            assert_eq!(
                regions.remove(start, end, |_| ()),
                Err(RegionError::Invalid)
            );
        }
        // All of it.
        regions.remove(0, 64 * PAGE, |_| ()).unwrap();
        assert!(layout(&regions).is_empty());
    }

    #[test]
    fn protecting_a_range_splits_rejoins_and_needs_every_page_in_a_region() {
        let data = Some(Attributes::USER_READ_WRITE);
        let read_only = Some(Attributes::USER_READ_ONLY);
        let mut regions = Regions::new();
        regions.insert(4 * PAGE, 12 * PAGE, data).unwrap();
        regions.insert(12 * PAGE, 14 * PAGE, read_only).unwrap();
        regions.insert(20 * PAGE, 22 * PAGE, data).unwrap();
        let mut changed = Vec::new();
        regions
            .protect(6 * PAGE, 8 * PAGE, None, |part| changed.push(part))
            .unwrap();
        assert_eq!(
            layout(&regions),
            [
                (4, 6, data),
                (6, 8, None),
                (8, 12, data),
                (12, 14, read_only),
                (20, 22, data)
            ]
        );
        assert_eq!(changed.len(), 1);
        // Back as it was: one region again.
        regions.protect(6 * PAGE, 8 * PAGE, data, |_| ()).unwrap();
        assert_eq!(
            layout(&regions),
            [(4, 12, data), (12, 14, read_only), (20, 22, data)]
        );
        // Across two touching regions: joined with the one whose attributes it takes.
        changed.clear();
        regions
            .protect(10 * PAGE, 14 * PAGE, read_only, |part| changed.push(part))
            .unwrap();
        assert_eq!(
            layout(&regions),
            [(4, 10, data), (10, 14, read_only), (20, 22, data)]
        );
        assert_eq!(changed.len(), 2);
        // A hole in the range at its start: nothing changes.
        for (start, end) in [(2 * PAGE, 6 * PAGE), (30 * PAGE, 31 * PAGE)] {
            assert_eq!(
                regions.protect(start, end, None, |_| panic!("changed")),
                Err(RegionError::NotCovered),
                "{start:#x}..{end:#x}"
            );
        }
        assert_eq!(
            layout(&regions),
            [(4, 10, data), (10, 14, read_only), (20, 22, data)]
        );
        // A hole later: what comes before it changes, as on Linux.
        assert_eq!(
            regions.protect(12 * PAGE, 21 * PAGE, data, |_| ()),
            Err(RegionError::NotCovered)
        );
        assert_eq!(
            layout(&regions),
            [
                (4, 10, data),
                (10, 12, read_only),
                (12, 14, data),
                (20, 22, data)
            ]
        );
        assert_eq!(
            regions.protect(4 * PAGE, 5 * PAGE, Some(Attributes::READ_WRITE), |_| ()),
            Err(RegionError::Invalid)
        );
    }

    #[test]
    fn free_space_is_found_from_the_top_down_between_floor_and_top() {
        let data = Some(Attributes::USER_READ_WRITE);
        let mut regions = Regions::new();
        assert_eq!(
            regions.free_below(100 * PAGE, 10 * PAGE, 4 * PAGE),
            Some(96 * PAGE)
        );
        regions.insert(90 * PAGE, 110 * PAGE, data).unwrap();
        regions.insert(80 * PAGE, 87 * PAGE, data).unwrap();
        regions.insert(20 * PAGE, 30 * PAGE, data).unwrap();
        // Above the top region's start is not free; the 3-page gap at 87 is too small for 4.
        assert_eq!(
            regions.free_below(100 * PAGE, 10 * PAGE, 4 * PAGE),
            Some(76 * PAGE)
        );
        assert_eq!(
            regions.free_below(100 * PAGE, 10 * PAGE, 3 * PAGE),
            Some(87 * PAGE)
        );
        // 50 pages fit between 30 and 80, 51 nowhere; then down to the floor.
        assert_eq!(regions.free_below(100 * PAGE, 10 * PAGE, 51 * PAGE), None);
        assert_eq!(
            regions.free_below(100 * PAGE, 10 * PAGE, 50 * PAGE),
            Some(30 * PAGE)
        );
        assert_eq!(
            regions.free_below(100 * PAGE, 10 * PAGE, 10 * PAGE),
            Some(70 * PAGE)
        );
        assert_eq!(
            regions.free_below(25 * PAGE, 10 * PAGE, 10 * PAGE),
            Some(10 * PAGE)
        );
        assert_eq!(regions.free_below(25 * PAGE, 11 * PAGE, 10 * PAGE), None);
        // A top inside a page counts from the page below it.
        assert_eq!(regions.free_below(20 * PAGE + 5, 0, PAGE), Some(19 * PAGE));
        assert!(regions.is_free(30 * PAGE, 80 * PAGE));
        assert!(!regions.is_free(29 * PAGE, 31 * PAGE));
        assert!(!regions.is_free(0, 200 * PAGE));
    }

    fn file_range(start: u64, end: u64, offset: u64, file: &Arc<BytesFile>) -> FileRange {
        FileRange {
            start: start * PAGE,
            end: end * PAGE,
            offset: offset * PAGE,
            file_bytes_end: WHOLE_FILE,
            writable: true,
            file: Arc::clone(file) as Arc<dyn FilePages>,
        }
    }

    /// File ranges as (start page, end page, file offset in pages).
    fn files(ranges: &FileRanges) -> Vec<(u64, u64, u64)> {
        ranges
            .iter()
            .map(|range| (range.start / PAGE, range.end / PAGE, range.offset / PAGE))
            .collect()
    }

    #[test]
    fn file_ranges_are_cut_and_found_with_their_offsets() {
        let file = Arc::new(BytesFile(Vec::new()));
        let mut ranges = FileRanges::new();
        ranges.insert(file_range(10, 20, 0, &file)).unwrap();
        ranges.insert(file_range(30, 34, 8, &file)).unwrap();
        assert_eq!(
            ranges.insert(file_range(19, 21, 0, &file)).unwrap_err(),
            RegionError::Overlaps
        );
        assert_eq!(
            ranges.insert(file_range(40, 40, 0, &file)).unwrap_err(),
            RegionError::Invalid
        );
        let found = ranges.find(15 * PAGE + 7).unwrap();
        assert_eq!(found.offset_of(15 * PAGE), 5 * PAGE);
        assert!(ranges.find(20 * PAGE).is_none());
        ranges.remove(12 * PAGE, 14 * PAGE).unwrap();
        assert_eq!(files(&ranges), [(10, 12, 0), (14, 20, 4), (30, 34, 8)]);
        ranges.remove(16 * PAGE, 31 * PAGE).unwrap();
        assert_eq!(files(&ranges), [(10, 12, 0), (14, 16, 4), (31, 34, 9)]);
        assert!(ranges.overlaps(11 * PAGE, 13 * PAGE));
        assert!(!ranges.overlaps(12 * PAGE, 14 * PAGE));
        ranges.remove(0, 64 * PAGE).unwrap();
        assert!(ranges.is_empty());
        assert_eq!(ranges.remove(PAGE, PAGE + 1), Err(RegionError::Invalid));
    }

    #[test]
    fn a_file_range_reads_its_bytes_up_to_where_they_end() {
        let file = Arc::new(BytesFile(Vec::new()));
        let segment = FileRange {
            file_bytes_end: 2 * PAGE + 100,
            ..file_range(4, 8, 1, &file)
        };
        assert_eq!(segment.file_bytes_in(4 * PAGE), PAGE);
        assert_eq!(segment.file_bytes_in(5 * PAGE), 100);
        assert_eq!(
            segment.file_bytes_in(7 * PAGE),
            0,
            "a segment's .bss is zeros"
        );
        let mapping = file_range(4, 8, 1, &file);
        assert_eq!(mapping.file_bytes_in(7 * PAGE), PAGE);
    }

    #[test]
    fn moving_file_ranges_keeps_their_offsets_and_grows_the_last_one() {
        let file = Arc::new(BytesFile(Vec::new()));
        let mut ranges = FileRanges::new();
        ranges.insert(file_range(10, 14, 2, &file)).unwrap();
        ranges.insert(file_range(14, 16, 9, &file)).unwrap();
        let moved = ranges
            .moved_parts(11 * PAGE, 5 * PAGE, 40 * PAGE, 8 * PAGE)
            .unwrap();
        assert_eq!(ranges.iter().count(), 2, "nothing changes yet");
        ranges
            .finish_move(11 * PAGE, 16 * PAGE, moved, false)
            .unwrap();
        assert_eq!(files(&ranges), [(10, 11, 2), (40, 43, 3), (43, 48, 9)]);
        let moved = ranges
            .moved_parts(10 * PAGE, PAGE, 10 * PAGE, 3 * PAGE)
            .unwrap();
        ranges
            .finish_move(10 * PAGE, 11 * PAGE, moved, false)
            .unwrap();
        assert_eq!(files(&ranges), [(10, 13, 2), (40, 43, 3), (43, 48, 9)]);
        let moved = ranges
            .moved_parts(40 * PAGE, 3 * PAGE, 60 * PAGE, 3 * PAGE)
            .unwrap();
        ranges
            .finish_move(40 * PAGE, 43 * PAGE, moved, true)
            .unwrap();
        assert_eq!(
            files(&ranges),
            [(10, 13, 2), (40, 43, 3), (43, 48, 9), (60, 63, 3)]
        );
        let copy = ranges.try_copy().unwrap();
        assert_eq!(files(&copy), files(&ranges));
    }

    #[test]
    fn a_file_in_memory_reads_fewer_bytes_at_its_end() {
        let file = BytesFile(b"0123456789".to_vec());
        let mut buffer = [0u8; 8];
        assert_eq!(file.read_at(6, &mut buffer), Some(4));
        assert_eq!(&buffer[..4], b"6789");
        assert_eq!(file.read_at(10, &mut buffer), Some(0));
        assert_eq!(file.read_at(u64::MAX, &mut buffer), Some(0));
    }
}

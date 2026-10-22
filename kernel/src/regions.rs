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

use alloc::vec::Vec;
use eggos_hal::memory::FRAME_SIZE;
pub use eggos_hal::paging::Access;
use eggos_hal::paging::Attributes;

/// One region: user addresses `start..end`, page-aligned, whose pages are mapped with
/// `attributes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub attributes: Attributes,
}

/// Why a region could not be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    /// The range is empty, not page-aligned or not user attributes.
    Invalid,
    /// Part of the range is in another region already.
    Overlaps,
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
    pub fn insert(
        &mut self,
        start: u64,
        end: u64,
        attributes: Attributes,
    ) -> Result<(), RegionError> {
        if start >= end || !(start | end).is_multiple_of(FRAME_SIZE) || !attributes.user {
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

    /// Every region, in address order.
    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
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
        let data = Attributes::USER_READ_WRITE;
        let text = Attributes::USER_TEXT;
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
        let data = Attributes::USER_READ_WRITE;
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
            regions.insert(PAGE, 2 * PAGE, Attributes::READ_WRITE),
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
}

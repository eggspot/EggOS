//! Memory a Windows program maps writable and executable at once (`PAGE_EXECUTE_READWRITE`,
//! `PAGE_EXECUTE_WRITECOPY`), which EggOS never maps (W^X): the ranges it lies in, and which of
//! the two a page of it becomes after the program faults on it (D213).

use crate::fault::access;

const PAGE_SIZE: usize = 4096;
const RECENT_FETCHES: usize = 4;

/// What a page of a writable and executable range becomes after a fault on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flip {
    /// Readable and executable: the program fetched an instruction from it.
    Executable,
    /// Readable and writable: the program wrote to it.
    Writable,
}

/// Up to `N` writable and executable ranges, and the pages instructions were last fetched from
/// after a flip.
#[derive(Clone, Copy)]
pub struct WriteExecutePages<const N: usize> {
    ranges: [(usize, usize); N],
    recent_fetches: [(usize, usize); RECENT_FETCHES],
    next_fetch: usize,
}

fn page_of(address: usize) -> usize {
    address & !(PAGE_SIZE - 1)
}

impl<const N: usize> WriteExecutePages<N> {
    pub const fn new() -> Self {
        Self {
            ranges: [(0, 0); N],
            recent_fetches: [(0, 0); RECENT_FETCHES],
            next_fetch: 0,
        }
    }

    /// Whether `address` lies in one of the ranges.
    pub fn holds(&self, address: usize) -> bool {
        self.ranges
            .iter()
            .any(|&(start, end)| end != 0 && (start..end).contains(&address))
    }

    /// The flip a fault with `access` (`fault::access`) at `address`, raised by the instruction
    /// at `pc`, calls for. None when `address` is in no range, and for a write to a page that
    /// holds the faulting instruction itself, which no flip completes: the instruction cannot
    /// run from a page that is writable, nor write to one that is executable.
    pub fn flip(&mut self, address: usize, access: u64, pc: usize) -> Option<Flip> {
        if !self.holds(address) {
            return None;
        }
        let page = page_of(address);
        match access {
            access::EXECUTE => {
                self.recent_fetches[self.next_fetch] = (pc, page);
                self.next_fetch = (self.next_fetch + 1) % RECENT_FETCHES;
                Some(Flip::Executable)
            }
            access::WRITE if page == page_of(pc) || self.recent_fetches.contains(&(pc, page)) => {
                None
            }
            access::WRITE => Some(Flip::Writable),
            _ => None,
        }
    }

    fn put(&mut self, start: usize, end: usize) -> bool {
        match self.ranges.iter_mut().find(|range| range.1 == 0) {
            Some(free) => {
                *free = (start, end);
                true
            }
            None => false,
        }
    }

    /// Records `start..end`, after taking it out of every range it overlaps. False, with
    /// nothing changed, when no slot is left for it.
    pub fn add(&mut self, start: usize, end: usize) -> bool {
        let mut changed = *self;
        let added = changed.remove(start, end) && (start >= end || changed.put(start, end));
        if added {
            *self = changed;
        }
        added
    }

    /// Takes `start..end` out of every range, keeping the parts of a range on either side of
    /// it. False, with nothing changed, when no slot is left for a part.
    pub fn remove(&mut self, start: usize, end: usize) -> bool {
        let mut changed = *self;
        let mut kept = true;
        for index in 0..N {
            let (range_start, range_end) = changed.ranges[index];
            if range_end == 0 || range_end <= start || end <= range_start {
                continue;
            }
            changed.ranges[index] = (0, 0);
            if range_start < start {
                kept &= changed.put(range_start, start);
            }
            if end < range_end {
                kept &= changed.put(end, range_end);
            }
        }
        if kept {
            *self = changed;
        }
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 0x1000;

    #[test]
    fn a_fetch_makes_a_page_executable_and_a_write_makes_it_writable() {
        let mut pages = WriteExecutePages::<4>::new();
        let code = 0x50_0000;
        assert!(pages.add(0x10_0000, 0x10_0000 + 2 * PAGE));
        assert_eq!(
            pages.flip(0x10_0000, access::EXECUTE, 0x10_0000),
            Some(Flip::Executable)
        );
        assert_eq!(
            pages.flip(0x10_0000 + PAGE + 8, access::WRITE, code),
            Some(Flip::Writable)
        );
        assert_eq!(
            pages.flip(0x10_0000, access::WRITE, code),
            Some(Flip::Writable)
        );
        assert_eq!(pages.flip(0x10_0000, access::READ, code), None);
        assert_eq!(
            pages.flip(0x10_0000 + 2 * PAGE, access::EXECUTE, code),
            None
        );
        assert_eq!(pages.flip(0x0f_ffff, access::WRITE, code), None);
    }

    #[test]
    fn a_write_to_the_page_of_the_writing_instruction_is_not_flipped() {
        let mut pages = WriteExecutePages::<4>::new();
        assert!(pages.add(0x10_0000, 0x10_0000 + 2 * PAGE));
        let pc = 0x10_0000 + 0x40;
        assert_eq!(pages.flip(pc, access::EXECUTE, pc), Some(Flip::Executable));
        assert_eq!(pages.flip(0x10_0000 + 0x800, access::WRITE, pc), None);
        assert_eq!(
            pages.flip(0x10_0000 + PAGE, access::WRITE, pc),
            Some(Flip::Writable)
        );
    }

    #[test]
    fn a_write_to_a_page_the_writing_instruction_runs_into_is_not_flipped() {
        let mut pages = WriteExecutePages::<4>::new();
        assert!(pages.add(0x10_0000, 0x10_0000 + 2 * PAGE));
        let pc = 0x10_0000 + PAGE - 3;
        let next_page = 0x10_0000 + PAGE;
        assert_eq!(
            pages.flip(next_page + 0x10, access::WRITE, pc),
            Some(Flip::Writable)
        );
        assert_eq!(
            pages.flip(next_page, access::EXECUTE, pc),
            Some(Flip::Executable)
        );
        assert_eq!(pages.flip(next_page + 0x10, access::WRITE, pc), None);
        assert_eq!(pages.flip(next_page + 0x10, access::WRITE, pc + 3), None);
        assert_eq!(
            pages.flip(next_page + 0x10, access::WRITE, 0x50_0000),
            Some(Flip::Writable)
        );
    }

    #[test]
    fn removing_the_middle_of_a_range_keeps_both_sides() {
        let mut pages = WriteExecutePages::<4>::new();
        assert!(pages.add(0x20_0000, 0x20_0000 + 4 * PAGE));
        assert!(pages.remove(0x20_0000 + PAGE, 0x20_0000 + 2 * PAGE));
        assert!(pages.holds(0x20_0000));
        assert!(!pages.holds(0x20_0000 + PAGE));
        assert!(pages.holds(0x20_0000 + 2 * PAGE));
        assert!(pages.holds(0x20_0000 + 4 * PAGE - 1));
        assert!(pages.remove(0x1f_0000, 0x30_0000));
        assert!(!pages.holds(0x20_0000));
        assert!(!pages.holds(0x20_0000 + 3 * PAGE));
    }

    #[test]
    fn adding_over_a_range_replaces_the_overlap_and_a_full_table_changes_nothing() {
        let mut pages = WriteExecutePages::<3>::new();
        assert!(pages.add(0x30_0000, 0x30_0000 + 2 * PAGE));
        assert!(pages.add(0x30_0000 + PAGE, 0x30_0000 + 3 * PAGE));
        assert!(pages.holds(0x30_0000));
        assert!(pages.holds(0x30_0000 + 3 * PAGE - 1));
        assert!(!pages.holds(0x30_0000 + 3 * PAGE));
        assert!(pages.add(0x40_0000, 0x40_0000 + PAGE));
        assert!(!pages.add(0x50_0000, 0x50_0000 + PAGE));
        assert!(!pages.holds(0x50_0000));
        assert!(!pages.remove(0x30_0000 + PAGE + 8, 0x30_0000 + PAGE + 16));
        assert!(pages.holds(0x30_0000 + PAGE + 8));
        assert!(pages.holds(0x40_0000));
    }
}

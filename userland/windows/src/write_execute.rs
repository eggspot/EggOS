//! Memory a Windows program asks to be writable and executable at once
//! (`PAGE_EXECUTE_READWRITE`, `PAGE_EXECUTE_WRITECOPY`), which EggOS never maps (W^X): the
//! layer maps it writable and records it here, and when the program faults on one of its pages
//! the page flips: an instruction fetch makes it readable and executable, a write makes it
//! readable and writable again, as the kernel allows a page to change from one to the other.
//! The fault handler reads the ranges without a lock, so a fault in the middle of a change never
//! waits; the program's calls that change them take turns through their caller's lock.

use core::sync::atomic::{AtomicUsize, Ordering};

/// What a page of a writable and executable range becomes after a fault on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flip {
    /// Readable and executable: the program fetched an instruction from it.
    Executable,
    /// Readable and writable: the program wrote to it.
    Writable,
}

/// The flip a fault with `access` (`fault::access`) at `address` calls for, if `address` lies
/// in one of `ranges`.
pub fn flip_for<const N: usize>(ranges: &Ranges<N>, address: usize, access: u64) -> Option<Flip> {
    if !ranges.holds(address) {
        return None;
    }
    match access {
        crate::fault::access::EXECUTE => Some(Flip::Executable),
        crate::fault::access::WRITE => Some(Flip::Writable),
        _ => None,
    }
}

/// Up to `N` address ranges, each its start and its end (zero when the slot is free).
pub struct Ranges<const N: usize> {
    slots: [(AtomicUsize, AtomicUsize); N],
}

impl<const N: usize> Ranges<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { (AtomicUsize::new(0), AtomicUsize::new(0)) }; N],
        }
    }

    /// Whether `address` lies in one of the ranges.
    pub fn holds(&self, address: usize) -> bool {
        self.slots.iter().any(|(start, end)| {
            let end = end.load(Ordering::Acquire);
            end != 0 && (start.load(Ordering::Relaxed)..end).contains(&address)
        })
    }

    fn put(&self, start: usize, end: usize) -> bool {
        let Some(free) = self
            .slots
            .iter()
            .find(|slot| slot.1.load(Ordering::Relaxed) == 0)
        else {
            return false;
        };
        free.0.store(start, Ordering::Relaxed);
        free.1.store(end, Ordering::Release);
        true
    }

    /// Records `start..end`, after taking it out of every range it overlaps. False when no slot
    /// is left for it.
    pub fn add(&self, start: usize, end: usize) -> bool {
        self.remove(start, end) && (start >= end || self.put(start, end))
    }

    /// Takes `start..end` out of every range, keeping the parts of a range on either side of
    /// it. False when no slot is left for a part, which is then no longer recorded.
    pub fn remove(&self, start: usize, end: usize) -> bool {
        let mut kept = true;
        for slot in &self.slots {
            let slot_end = slot.1.load(Ordering::Relaxed);
            let slot_start = slot.0.load(Ordering::Relaxed);
            if slot_end == 0 || slot_end <= start || end <= slot_start {
                continue;
            }
            slot.1.store(0, Ordering::Release);
            if slot_start < start {
                kept &= self.put(slot_start, start);
            }
            if end < slot_end {
                kept &= self.put(end, slot_end);
            }
        }
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault::access;

    const PAGE: usize = 0x1000;

    #[test]
    fn a_fetch_makes_a_page_executable_and_a_write_makes_it_writable() {
        let ranges = Ranges::<4>::new();
        assert!(ranges.add(0x10_0000, 0x10_0000 + 2 * PAGE));
        assert_eq!(
            flip_for(&ranges, 0x10_0000, access::EXECUTE),
            Some(Flip::Executable)
        );
        assert_eq!(
            flip_for(&ranges, 0x10_0000 + PAGE + 8, access::WRITE),
            Some(Flip::Writable)
        );
        assert_eq!(flip_for(&ranges, 0x10_0000, access::READ), None);
        assert_eq!(
            flip_for(&ranges, 0x10_0000 + 2 * PAGE, access::EXECUTE),
            None
        );
        assert_eq!(flip_for(&ranges, 0x0f_ffff, access::WRITE), None);
    }

    #[test]
    fn removing_the_middle_of_a_range_keeps_both_sides() {
        let ranges = Ranges::<4>::new();
        assert!(ranges.add(0x20_0000, 0x20_0000 + 4 * PAGE));
        assert!(ranges.remove(0x20_0000 + PAGE, 0x20_0000 + 2 * PAGE));
        assert!(ranges.holds(0x20_0000));
        assert!(!ranges.holds(0x20_0000 + PAGE));
        assert!(ranges.holds(0x20_0000 + 2 * PAGE));
        assert!(ranges.holds(0x20_0000 + 4 * PAGE - 1));
        assert!(ranges.remove(0x1f_0000, 0x30_0000));
        assert!(!ranges.holds(0x20_0000));
        assert!(!ranges.holds(0x20_0000 + 3 * PAGE));
    }

    #[test]
    fn adding_over_a_range_replaces_the_overlap_and_a_full_table_says_so() {
        let ranges = Ranges::<2>::new();
        assert!(ranges.add(0x30_0000, 0x30_0000 + 2 * PAGE));
        assert!(ranges.add(0x30_0000 + PAGE, 0x30_0000 + 3 * PAGE));
        assert!(ranges.holds(0x30_0000));
        assert!(ranges.holds(0x30_0000 + 3 * PAGE - 1));
        assert!(!ranges.holds(0x30_0000 + 3 * PAGE));
        assert!(!ranges.add(0x40_0000, 0x40_0000 + PAGE));
        assert!(!ranges.holds(0x40_0000));
    }
}

//! Method caches: per class, a power-of-two table of `(SEL, IMP)` pairs that `objc_msgSend`
//! probes without a lock. The probe starts at `(SEL << 4) & mask_bytes` and walks forward
//! until it finds the selector or an empty slot. A table is filled under the runtime lock and
//! never more than three quarters full, so every probe ends; each slot is written once, its IMP
//! before its SEL, so a reader that sees the SEL either sees the IMP or a 0, which it treats as
//! a miss. A fuller table is replaced by a new one, published with one release store; old
//! tables are never freed, so a reader still probing one is never left with freed memory.

use core::sync::atomic::{AtomicUsize, Ordering};

/// One slot.
#[repr(C)]
#[derive(Debug)]
pub struct Entry {
    pub sel: AtomicUsize,
    pub imp: AtomicUsize,
}

/// A cache table's header; its `capacity` entries follow it.
#[repr(C)]
#[derive(Debug)]
pub struct Cache {
    pub mask_bytes: usize,
    pub occupied: AtomicUsize,
    pub first: Entry,
}

/// The cache every class starts with, `__objc_empty_cache`: one empty slot, so every probe
/// misses at once.
pub static EMPTY: Cache = Cache {
    mask_bytes: 0,
    occupied: AtomicUsize::new(0),
    first: Entry {
        sel: AtomicUsize::new(0),
        imp: AtomicUsize::new(0),
    },
};

pub const HEADER_BYTES: usize = 16;
pub const ENTRY_BYTES: usize = 16;
pub const FIRST_CAPACITY: usize = 8;

/// The bytes a table of `capacity` slots takes.
pub const fn bytes_for(capacity: usize) -> usize {
    HEADER_BYTES + capacity * ENTRY_BYTES
}

impl Cache {
    /// How many slots the table has.
    pub fn capacity(&self) -> usize {
        self.mask_bytes / ENTRY_BYTES + 1
    }

    fn entry(&self, index: usize) -> &Entry {
        let first = &raw const self.first;
        // SAFETY: every table is allocated with `capacity()` entries after its header, and
        // `index` is masked to below it.
        unsafe { &*first.wrapping_add(index & (self.capacity() - 1)) }
    }

    /// The table's IMP for `sel`, probing as `objc_msgSend` does.
    #[cfg(test)]
    pub fn lookup(&self, sel: usize) -> Option<usize> {
        let capacity = self.capacity();
        let mut index = sel & (capacity - 1);
        for _ in 0..capacity {
            let entry = self.entry(index);
            match entry.sel.load(Ordering::Acquire) {
                0 => return None,
                found if found == sel => {
                    let imp = entry.imp.load(Ordering::Relaxed);
                    return (imp != 0).then_some(imp);
                }
                _ => index += 1,
            }
        }
        None
    }

    /// Whether one more selector fits without passing three quarters full.
    pub fn has_room(&self) -> bool {
        (self.occupied.load(Ordering::Relaxed) + 1) * 4 <= self.capacity() * 3
    }

    /// Puts `sel` with `imp` in its first free slot; the caller holds the runtime lock and has
    /// checked [`Self::has_room`]. A selector already there is left as it is.
    pub fn insert(&self, sel: usize, imp: usize) {
        let mut index = sel & (self.capacity() - 1);
        loop {
            let entry = self.entry(index);
            match entry.sel.load(Ordering::Relaxed) {
                0 => {
                    entry.imp.store(imp, Ordering::Relaxed);
                    entry.sel.store(sel, Ordering::Release);
                    self.occupied.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                found if found == sel => return,
                _ => index += 1,
            }
        }
    }

    /// Copies every filled slot of `self` into `into`, which must have room for them.
    pub fn copy_into(&self, into: &Cache) {
        for index in 0..self.capacity() {
            let entry = self.entry(index);
            let sel = entry.sel.load(Ordering::Acquire);
            let imp = entry.imp.load(Ordering::Relaxed);
            if sel != 0 && imp != 0 {
                into.insert(sel, imp);
            }
        }
    }
}

/// Makes the zeroed `bytes_for(capacity)` bytes at `memory` an empty table of `capacity`
/// slots, a power of two.
///
/// # Safety
/// `memory` must be zeroed, writable, 16-byte aligned and `bytes_for(capacity)` long, and
/// stay allocated for as long as the program runs.
pub unsafe fn initialize<'a>(memory: *mut u8, capacity: usize) -> &'a Cache {
    let cache = memory.cast::<Cache>();
    // SAFETY: the caller's promise: the header is writable and nothing else uses it yet.
    let mask = unsafe { &raw mut (*cache).mask_bytes };
    // SAFETY: as above.
    unsafe { mask.write((capacity - 1) * ENTRY_BYTES) };
    // SAFETY: as above; the zeroed entries are valid atomics.
    unsafe { &*cache }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(capacity: usize) -> &'static Cache {
        let memory = vec![0u128; bytes_for(capacity) / 16].leak();
        // SAFETY: a leaked, zeroed, aligned block of the right size.
        unsafe { initialize(memory.as_mut_ptr().cast(), capacity) }
    }

    #[test]
    fn the_empty_cache_misses_everything() {
        assert_eq!(EMPTY.capacity(), 1);
        assert_eq!(EMPTY.lookup(0x1234), None);
        assert!(!EMPTY.has_room());
    }

    #[test]
    fn inserted_selectors_are_found_and_others_miss() {
        let cache = table(FIRST_CAPACITY);
        cache.insert(0x1000, 0xa);
        cache.insert(0x1008, 0xb);
        cache.insert(0x1000, 0xc);
        assert_eq!(cache.lookup(0x1000), Some(0xa));
        assert_eq!(cache.lookup(0x1008), Some(0xb));
        assert_eq!(cache.lookup(0x1010), None);
        assert_eq!(cache.occupied.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_table_at_its_fill_limit_still_ends_every_probe() {
        let cache = table(FIRST_CAPACITY);
        let mut sel = 0x2000;
        while cache.has_room() {
            cache.insert(sel, sel + 1);
            sel += 8;
        }
        assert_eq!(cache.occupied.load(Ordering::Relaxed), 6);
        for missing in (0x9000..0x9100).step_by(1) {
            assert_eq!(cache.lookup(missing), None);
        }
        let empty = (0..cache.capacity())
            .filter(|&index| cache.entry(index).sel.load(Ordering::Relaxed) == 0)
            .count();
        assert_eq!(empty, 2);
    }

    #[test]
    fn a_grown_table_keeps_every_entry() {
        let small = table(FIRST_CAPACITY);
        for sel in [0x10, 0x11, 0x12, 0x13, 0x14, 0x15] {
            small.insert(sel, sel * 2);
        }
        let large = table(FIRST_CAPACITY * 2);
        small.copy_into(large);
        for sel in [0x10, 0x11, 0x12, 0x13, 0x14, 0x15] {
            assert_eq!(large.lookup(sel), Some(sel * 2));
        }
        assert!(large.has_room());
    }

    #[test]
    fn the_probe_matches_the_assembly() {
        let cache = table(16);
        assert_eq!(cache.mask_bytes, 15 * 16);
        assert_eq!((0x1235usize << 4) & cache.mask_bytes, (0x1235 & 15) * 16);
        assert_eq!(core::mem::offset_of!(Cache, first), HEADER_BYTES);
    }
}

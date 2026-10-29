//! Frames that more than one address space maps: copy-on-write `fork` (D169).
//!
//! A `fork` gives the child the parent's frames instead of copies: both map each one read-only,
//! and the first write on either side copies the page ([`crate::process::Process`]'s fault
//! path). Each such frame has a count of the address spaces that map it beyond the first, kept
//! here: [`share`] adds one as a `fork` maps a frame in the child, [`release`] takes one away as
//! an address space unmaps the frame or goes, and tells the last one to free it, and
//! [`is_shared`] tells a write fault whether it must copy or may take the frame as it is.
//!
//! The counts live in a radix tree indexed by the frame's number, made the first time a frame
//! is shared and never freed: a leaf of 2048 two-byte counts covers 8 MiB of memory, so the
//! tree costs at most 1/2048 of the memory that forks ever shared, and nothing before the
//! first `fork`. Every operation is a few loads and one atomic update, with no lock, and a
//! count of the frames shared now lets the paths that free frames ([`release`]) pay one relaxed
//! load whenever no frame is shared, as after every forked child has exited or exec'd. A
//! frame the tree cannot count (its node could not be allocated, its address is beyond
//! [`MAX_PHYSICAL_ADDRESS`], or its count is full) is copied by `fork` instead.

use crate::heap;
use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU16, AtomicUsize, Ordering};
use eggos_hal::memory::FRAME_SIZE;

const LEAF_BITS: u32 = 11;
const LEAF_ENTRIES: usize = 1 << LEAF_BITS;
const BRANCH_BITS: u32 = 9;
const BRANCH_ENTRIES: usize = 1 << BRANCH_BITS;
const FRAME_BITS: u32 = FRAME_SIZE.trailing_zeros();

/// The highest physical address whose frame the tree counts, exclusive: 2 TiB.
pub const MAX_PHYSICAL_ADDRESS: u64 = 1 << (FRAME_BITS + LEAF_BITS + 2 * BRANCH_BITS);

struct Leaf {
    counts: [AtomicU16; LEAF_ENTRIES],
}

struct Middle {
    leaves: [AtomicPtr<Leaf>; BRANCH_ENTRIES],
}

struct Top {
    middles: [AtomicPtr<Middle>; BRANCH_ENTRIES],
}

static TOP: AtomicPtr<Top> = AtomicPtr::new(ptr::null_mut());

static SHARED_NOW: AtomicUsize = AtomicUsize::new(0);

fn allocate_zeroed<T>() -> *mut T {
    let layout = Layout::new::<T>();
    if !heap::may_grow_for_user(layout.size()) {
        return ptr::null_mut();
    }
    // SAFETY: the layout is a struct's, never zero-sized; every such struct here is atomics,
    // for which all zero bytes are a valid value.
    unsafe { alloc_zeroed(layout) }.cast()
}

fn node<T>(slot: &AtomicPtr<T>, create: bool) -> *mut T {
    let current = slot.load(Ordering::Acquire);
    if !current.is_null() || !create {
        return current;
    }
    let made = allocate_zeroed::<T>();
    if made.is_null() {
        return made;
    }
    match slot.compare_exchange(ptr::null_mut(), made, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(installed) => {
            // SAFETY: `made` came from `alloc_zeroed` with this layout just above and was never
            // published, so nothing else refers to it.
            unsafe { dealloc(made.cast(), Layout::new::<T>()) };
            installed
        }
    }
}

#[inline]
fn count_of(frame: u64, create: bool) -> Option<&'static AtomicU16> {
    if frame >= MAX_PHYSICAL_ADDRESS {
        return None;
    }
    let number = (frame >> FRAME_BITS) as usize;
    let top = node(&TOP, create);
    if top.is_null() {
        return None;
    }
    // SAFETY: every node in the tree is installed once, zeroed, and never freed or moved, so a
    // pointer read from its slot stays valid for the kernel's lifetime.
    let top = unsafe { &*top };
    let middle = node(&top.middles[number >> (LEAF_BITS + BRANCH_BITS)], create);
    if middle.is_null() {
        return None;
    }
    // SAFETY: as above.
    let middle = unsafe { &*middle };
    let leaf = node(
        &middle.leaves[(number >> LEAF_BITS) & (BRANCH_ENTRIES - 1)],
        create,
    );
    if leaf.is_null() {
        return None;
    }
    // SAFETY: as above.
    let leaf = unsafe { &*leaf };
    Some(&leaf.counts[number & (LEAF_ENTRIES - 1)])
}

/// Records that one more address space maps `frame`, a frame one already maps: `false` if it
/// cannot be counted, and the caller must give the new one a copy instead.
pub fn share(frame: u64) -> bool {
    let Some(count) = count_of(frame, true) else {
        return false;
    };
    match count.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |sharers| {
        sharers.checked_add(1).filter(|&sharers| sharers < u16::MAX)
    }) {
        Ok(0) => {
            SHARED_NOW.fetch_add(1, Ordering::Relaxed);
            true
        }
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Whether an address space other than the caller's maps `frame` too.
#[inline]
pub fn is_shared(frame: u64) -> bool {
    if SHARED_NOW.load(Ordering::Relaxed) == 0 {
        return false;
    }
    count_of(frame, false).is_some_and(|count| count.load(Ordering::Acquire) > 0)
}

/// Records that the caller's address space no longer maps `frame`: `true` if no other one does,
/// and the caller frees it; `false` if another one still maps it and keeps it.
#[inline]
pub fn release(frame: u64) -> bool {
    if SHARED_NOW.load(Ordering::Relaxed) == 0 {
        return true;
    }
    release_counted(frame)
}

#[cold]
#[inline(never)]
fn release_counted(frame: u64) -> bool {
    let Some(count) = count_of(frame, false) else {
        return true;
    };
    match count.fetch_update(Ordering::AcqRel, Ordering::Acquire, |sharers| {
        sharers.checked_sub(1)
    }) {
        Ok(1) => {
            SHARED_NOW.fetch_sub(1, Ordering::Relaxed);
            false
        }
        Ok(_) => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: u64 = 0x7654_3000;

    #[test]
    fn a_shared_frame_is_freed_by_the_last_address_space_that_releases_it() {
        let frame = FIRST;
        assert!(!is_shared(frame));
        assert!(share(frame));
        assert!(share(frame));
        assert!(is_shared(frame));
        assert!(!release(frame));
        assert!(is_shared(frame));
        assert!(!release(frame));
        assert!(!is_shared(frame));
        assert!(release(frame));
    }

    #[test]
    fn frames_the_tree_cannot_reach_are_never_shared_and_always_freed() {
        let beyond = MAX_PHYSICAL_ADDRESS;
        assert!(!share(beyond));
        assert!(!is_shared(beyond));
        assert!(release(beyond));
        assert!(release(FIRST + 0x100_0000));
    }

    #[test]
    fn a_full_count_refuses_one_more_sharer() {
        let frame = FIRST + 2 * FRAME_SIZE;
        let count = count_of(frame, true).unwrap();
        count.store(u16::MAX - 1, Ordering::Relaxed);
        assert!(!share(frame));
        count.store(0, Ordering::Relaxed);
        assert!(release(frame));
    }
}

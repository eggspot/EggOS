//! A native Swift object's reference count, the word after its metadata pointer (D180), in
//! Swift's 64-bit inline layout: bit 0 marks pure-Swift deallocation, bits 1-31 count unowned
//! references (one for the strong references together), bit 32 is set once deinitialization
//! starts, bits 33-62 count the strong references beyond the first. An object whose low 32 bits
//! are all set is immortal (statics, the empty singletons): counting leaves it alone. The top bit
//! of the unowned count, which the runtime never counts that high, marks an object a weak
//! reference names, so its last release clears those references. Pure logic over an atomic
//! word with host tests.

use core::sync::atomic::{AtomicU64, Ordering};

/// A new object's count: one strong reference, one unowned.
pub const INITIAL: u64 = 0b11;
/// The count of an object that is never freed.
pub const IMMORTAL: u64 = u64::MAX;
const STRONG_ONE: u64 = 1 << 33;
const STRONG_MASK: u64 = ((1 << 30) - 1) << 33;
/// Set once the object's deinitialization starts.
pub const DEINITING: u64 = 1 << 32;
const IMMORTAL_BITS: u64 = 0xffff_ffff;
/// Set once a weak reference names the object.
pub const WEAKLY_REFERENCED: u64 = 1 << 31;

pub fn is_immortal(bits: u64) -> bool {
    bits & IMMORTAL_BITS == IMMORTAL_BITS
}

/// The strong references `bits` counts (immortal objects report the most).
pub fn strong_count(bits: u64) -> u64 {
    if is_immortal(bits) {
        return u64::MAX;
    }
    ((bits & STRONG_MASK) >> 33) + 1
}

/// Adds `count` strong references.
pub fn retain(word: &AtomicU64, count: u32) {
    if is_immortal(word.load(Ordering::Relaxed)) {
        return;
    }
    word.fetch_add(STRONG_ONE * u64::from(count), Ordering::Relaxed);
}

/// Drops `count` strong references; true when they were the last, so deinitialization has
/// started and the caller destroys the object.
pub fn release(word: &AtomicU64, count: u32) -> bool {
    let mut bits = word.load(Ordering::Relaxed);
    loop {
        if is_immortal(bits) || bits & DEINITING != 0 {
            return false;
        }
        let extra = (bits & STRONG_MASK) >> 33;
        let (next, last) = if extra < u64::from(count) {
            ((bits & !STRONG_MASK) | DEINITING, true)
        } else {
            (bits - STRONG_ONE * u64::from(count), false)
        };
        match word.compare_exchange_weak(bits, next, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => {
                if last {
                    core::sync::atomic::fence(Ordering::Acquire);
                }
                return last;
            }
            Err(now) => bits = now,
        }
    }
}

/// Whether only one strong reference holds the object (never for an immortal one, so the
/// shared empty storages are copied before they are written).
pub fn is_unique(bits: u64) -> bool {
    !is_immortal(bits) && bits & (STRONG_MASK | DEINITING) == 0
}

/// Adds one strong reference unless deinitialization has started; false then. Immortal objects
/// always succeed.
pub fn try_retain(word: &AtomicU64) -> bool {
    let mut bits = word.load(Ordering::Relaxed);
    loop {
        if is_immortal(bits) {
            return true;
        }
        if bits & DEINITING != 0 {
            return false;
        }
        match word.compare_exchange_weak(
            bits,
            bits + STRONG_ONE,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(now) => bits = now,
        }
    }
}

/// Whether deinitialization of a mortal object has started.
pub fn is_deinitializing(bits: u64) -> bool {
    !is_immortal(bits) && bits & DEINITING != 0
}

/// Marks a mortal object as named by a weak reference.
pub fn mark_weakly_referenced(word: &AtomicU64) {
    if !is_immortal(word.load(Ordering::Relaxed)) {
        word.fetch_or(WEAKLY_REFERENCED, Ordering::Relaxed);
    }
}

/// Whether a weak reference names the mortal object.
pub fn is_weakly_referenced(bits: u64) -> bool {
    !is_immortal(bits) && bits & WEAKLY_REFERENCED != 0
}

/// Marks the object as deinitializing without counting.
pub fn set_deinitializing(word: &AtomicU64) {
    let bits = word.load(Ordering::Relaxed);
    if !is_immortal(bits) {
        word.fetch_or(DEINITING, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_match_swifts_layout() {
        let word = AtomicU64::new(INITIAL);
        assert!(is_unique(word.load(Ordering::Relaxed)));
        retain(&word, 1);
        assert_eq!(word.load(Ordering::Relaxed), 0x2_0000_0003);
        assert!(!is_unique(word.load(Ordering::Relaxed)));
        assert_eq!(strong_count(word.load(Ordering::Relaxed)), 2);
        assert!(!release(&word, 1));
        assert_eq!(word.load(Ordering::Relaxed), INITIAL);
        assert!(release(&word, 1));
        assert_eq!(word.load(Ordering::Relaxed), INITIAL | DEINITING);
        assert!(!release(&word, 1));
    }

    #[test]
    fn several_references_go_at_once() {
        let word = AtomicU64::new(INITIAL);
        retain(&word, 3);
        assert_eq!(strong_count(word.load(Ordering::Relaxed)), 4);
        assert!(!release(&word, 3));
        assert!(release(&word, 2));
    }

    #[test]
    fn immortal_objects_are_never_released_nor_unique() {
        for bits in [IMMORTAL, 0x8000_0004_ffff_ffff] {
            let word = AtomicU64::new(bits);
            retain(&word, 1);
            assert!(!release(&word, 1));
            assert!(!is_unique(word.load(Ordering::Relaxed)));
            set_deinitializing(&word);
            assert!(is_immortal(word.load(Ordering::Relaxed)));
        }
    }

    #[test]
    fn weak_loads_retain_only_live_objects() {
        let word = AtomicU64::new(INITIAL);
        mark_weakly_referenced(&word);
        assert!(is_weakly_referenced(word.load(Ordering::Relaxed)));
        assert!(!is_immortal(word.load(Ordering::Relaxed)));
        assert!(is_unique(word.load(Ordering::Relaxed)));
        assert!(try_retain(&word));
        assert_eq!(strong_count(word.load(Ordering::Relaxed)), 2);
        assert!(!release(&word, 1));
        assert!(release(&word, 1));
        assert!(is_deinitializing(word.load(Ordering::Relaxed)));
        assert!(is_weakly_referenced(word.load(Ordering::Relaxed)));
        assert!(!try_retain(&word));
    }

    #[test]
    fn immortal_objects_are_never_weakly_marked() {
        let word = AtomicU64::new(IMMORTAL);
        mark_weakly_referenced(&word);
        assert_eq!(word.load(Ordering::Relaxed), IMMORTAL);
        assert!(!is_weakly_referenced(IMMORTAL));
        assert!(!is_deinitializing(IMMORTAL));
        assert!(try_retain(&word));
    }
}

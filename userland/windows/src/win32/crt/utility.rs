//! The utility and time functions: `rand` and `srand` (our own xorshift generator, so values
//! are in `0..=RAND_MAX` but not Microsoft's sequence), `abs` and its family, `qsort` (a heap
//! sort, in place and without allocating) and `bsearch`, both calling the program's comparison
//! function, and `_time64`.

use super::super::call_program;
use crate::library::Export;
use crate::linux::{self, CLOCK_REALTIME};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

const RAND_MAX: u32 = 0x7fff;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

static RANDOM_STATE: AtomicU32 = AtomicU32::new(1);

fn next_random(state: u32) -> u32 {
    let mut value = if state == 0 { 1 } else { state };
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    value
}

extern "efiapi" fn rand() -> i32 {
    let mut state = RANDOM_STATE.load(Ordering::Relaxed);
    loop {
        let next = next_random(state);
        match RANDOM_STATE.compare_exchange_weak(state, next, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return ((next >> 16) & RAND_MAX) as i32,
            Err(current) => state = current,
        }
    }
}

extern "efiapi" fn srand(seed: u32) {
    RANDOM_STATE.store(seed, Ordering::Relaxed);
}

extern "efiapi" fn abs(value: i32) -> i32 {
    value.wrapping_abs()
}

extern "efiapi" fn abs64(value: i64) -> i64 {
    value.wrapping_abs()
}

struct Elements {
    base: *mut u8,
    size: usize,
    compare: usize,
}

impl Elements {
    fn at(&self, index: usize) -> *mut u8 {
        self.base.wrapping_add(index * self.size)
    }

    unsafe fn order(&self, a: *const u8, b: *const u8) -> i32 {
        let arguments = [a as usize, b as usize, 0, 0];
        // SAFETY: the caller's duty: `compare` is the program's
        // `int (*)(const void *, const void *)` and both point to elements.
        unsafe { call_program(self.compare, arguments) as u32 as i32 }
    }

    unsafe fn less(&self, a: usize, b: usize) -> bool {
        // SAFETY: the caller's duty: both indexes are inside the array.
        unsafe { self.order(self.at(a), self.at(b)) < 0 }
    }

    unsafe fn swap(&self, a: usize, b: usize) {
        if a != b {
            // SAFETY: the caller's duty: two different elements of `size` bytes each.
            unsafe { ptr::swap_nonoverlapping(self.at(a), self.at(b), self.size) };
        }
    }

    unsafe fn sift_down(&self, mut root: usize, end: usize) {
        loop {
            let mut child = 2 * root + 1;
            if child >= end {
                return;
            }
            // SAFETY: the caller's duty: `end` elements are inside the array.
            if child + 1 < end && unsafe { self.less(child, child + 1) } {
                child += 1;
            }
            // SAFETY: as above.
            if !unsafe { self.less(root, child) } {
                return;
            }
            // SAFETY: as above.
            unsafe { self.swap(root, child) };
            root = child;
        }
    }
}

unsafe extern "efiapi" fn qsort(base: *mut u8, count: usize, size: usize, compare: usize) {
    if count < 2 || size == 0 || base.is_null() {
        return;
    }
    let elements = Elements {
        base,
        size,
        compare,
    };
    for start in (0..count / 2).rev() {
        // SAFETY: `qsort`'s contract: `count` elements of `size` bytes at `base`.
        unsafe { elements.sift_down(start, count) };
    }
    for end in (1..count).rev() {
        // SAFETY: as above.
        unsafe { elements.swap(0, end) };
        // SAFETY: as above.
        unsafe { elements.sift_down(0, end) };
    }
}

unsafe extern "efiapi" fn bsearch(
    key: *const u8,
    base: *mut u8,
    count: usize,
    size: usize,
    compare: usize,
) -> *mut u8 {
    let elements = Elements {
        base,
        size,
        compare,
    };
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        // SAFETY: `bsearch`'s contract: a sorted array of `count` elements and the program's
        // comparison function.
        let order = unsafe { elements.order(key, elements.at(middle)) };
        match order {
            0 => return elements.at(middle),
            order if order < 0 => high = middle,
            _ => low = middle + 1,
        }
    }
    ptr::null_mut()
}

unsafe extern "efiapi" fn time64(out: *mut i64) -> i64 {
    let seconds = (linux::clock_nanoseconds(CLOCK_REALTIME) / NANOSECONDS_PER_SECOND) as i64;
    if !out.is_null() {
        // SAFETY: `_time64`'s contract: null or a writable `__time64_t`.
        unsafe { out.write_unaligned(seconds) };
    }
    seconds
}

pub(super) const EXPORTS: &[Export] = exports![
    b"rand" => rand,
    b"srand" => srand,
    b"abs" => abs,
    b"labs" => abs,
    b"llabs" => abs64,
    b"_abs64" => abs64,
    b"qsort" => qsort,
    b"bsearch" => bsearch,
    b"_time64" => time64,
];

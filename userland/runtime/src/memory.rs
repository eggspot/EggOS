//! The memory and string functions the compiler and `core` call, which a C library provides
//! on Linux targets.

use core::ffi::{c_char, c_int, c_void};

#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    let (to, from) = (destination.cast::<u8>(), source.cast::<u8>());
    for index in 0..count {
        // SAFETY: the caller's `count` readable bytes at `source`.
        let byte = unsafe { from.wrapping_add(index).read() };
        // SAFETY: the caller's `count` writable bytes at `destination`.
        unsafe { to.wrapping_add(index).write(byte) };
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    if destination.addr() <= source.addr() {
        // SAFETY: copying upwards, each byte is read before a write can reach it.
        return unsafe { memcpy(destination, source, count) };
    }
    let (to, from) = (destination.cast::<u8>(), source.cast::<u8>());
    for index in (0..count).rev() {
        // SAFETY: the caller's `count` readable bytes at `source`.
        let byte = unsafe { from.wrapping_add(index).read() };
        // SAFETY: the caller's `count` writable bytes at `destination`.
        unsafe { to.wrapping_add(index).write(byte) };
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memset(destination: *mut c_void, value: c_int, count: usize) -> *mut c_void {
    let to = destination.cast::<u8>();
    for index in 0..count {
        // SAFETY: the caller's `count` writable bytes at `destination`.
        unsafe { to.wrapping_add(index).write(value as u8) };
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int {
    let (left, right) = (left.cast::<u8>(), right.cast::<u8>());
    for index in 0..count {
        // SAFETY: the caller's `count` readable bytes at `left`.
        let a = unsafe { left.wrapping_add(index).read() };
        // SAFETY: the caller's `count` readable bytes at `right`.
        let b = unsafe { right.wrapping_add(index).read() };
        if a != b {
            return c_int::from(a) - c_int::from(b);
        }
    }
    0
}

#[unsafe(no_mangle)]
unsafe extern "C" fn bcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int {
    // SAFETY: the caller's `count` readable bytes at each.
    unsafe { memcmp(left, right, count) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn strlen(string: *const c_char) -> usize {
    let mut length = 0;
    // SAFETY: the caller's null-ended string: every byte up to its end marker is readable.
    while unsafe { string.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    length
}

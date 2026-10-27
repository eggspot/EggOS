//! `<string.h>`, `<strings.h>` and the `_FORTIFY_SOURCE` checks clang emits around them
//! (`__memcpy_chk` and its kin), which abort when a copy would overflow its destination.

use super::{c_string, heap, process};
use core::ffi::{c_char, c_int, c_void};

const UNKNOWN_SIZE: usize = usize::MAX;

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn strlen(text: *const c_char) -> usize {
    // SAFETY: the caller's promise.
    unsafe { c_string(text) }.len()
}

/// # Safety
/// `text` must be readable for `limit` bytes or up to its NUL.
pub unsafe extern "C" fn strnlen(text: *const c_char, limit: usize) -> usize {
    let mut length = 0;
    // SAFETY: the caller's promise.
    while length < limit && unsafe { text.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    length
}

/// # Safety
/// Both must be NUL-terminated strings.
pub unsafe extern "C" fn strcmp(left: *const c_char, right: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { strncmp(left, right, usize::MAX) }
}

/// # Safety
/// Both must be readable for `count` bytes or up to their NUL.
pub unsafe extern "C" fn strncmp(left: *const c_char, right: *const c_char, count: usize) -> c_int {
    for index in 0..count {
        // SAFETY: the caller's promise.
        let a = unsafe { left.cast::<u8>().wrapping_add(index).read() };
        // SAFETY: as above.
        let b = unsafe { right.cast::<u8>().wrapping_add(index).read() };
        if a != b || a == 0 {
            return c_int::from(a) - c_int::from(b);
        }
    }
    0
}

/// # Safety
/// Both must be NUL-terminated strings.
pub unsafe extern "C" fn strcasecmp(left: *const c_char, right: *const c_char) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { strncasecmp(left, right, usize::MAX) }
}

/// # Safety
/// Both must be readable for `count` bytes or up to their NUL.
pub unsafe extern "C" fn strncasecmp(
    left: *const c_char,
    right: *const c_char,
    count: usize,
) -> c_int {
    for index in 0..count {
        // SAFETY: the caller's promise.
        let a = unsafe { left.cast::<u8>().wrapping_add(index).read() };
        // SAFETY: as above.
        let b = unsafe { right.cast::<u8>().wrapping_add(index).read() };
        let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
        if a != b || a == 0 {
            return c_int::from(a) - c_int::from(b);
        }
    }
    0
}

/// # Safety
/// `destination` must hold `source`'s length plus its NUL; they must not overlap.
pub unsafe extern "C" fn strcpy(destination: *mut c_char, source: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { strlen(source) };
    // SAFETY: as above.
    unsafe { destination.copy_from_nonoverlapping(source, length + 1) };
    destination
}

/// # Safety
/// `destination` must hold `count` bytes; `source` readable up to `count` bytes or its NUL.
pub unsafe extern "C" fn strncpy(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { strnlen(source, count) };
    // SAFETY: as above.
    unsafe { destination.copy_from_nonoverlapping(source, length) };
    // SAFETY: as above.
    unsafe {
        destination
            .wrapping_add(length)
            .write_bytes(0, count - length)
    };
    destination
}

/// # Safety
/// `destination` must be a NUL-terminated string with room for `source` after it.
pub unsafe extern "C" fn strcat(destination: *mut c_char, source: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { strlen(destination) };
    // SAFETY: as above.
    unsafe { strcpy(destination.wrapping_add(length), source) };
    destination
}

/// # Safety
/// As `strcat`, for at most `count` bytes of `source`.
pub unsafe extern "C" fn strncat(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
) -> *mut c_char {
    // SAFETY: the caller's promise.
    let end = destination.wrapping_add(unsafe { strlen(destination) });
    // SAFETY: as above.
    let length = unsafe { strnlen(source, count) };
    // SAFETY: as above.
    unsafe { end.copy_from_nonoverlapping(source, length) };
    // SAFETY: as above.
    unsafe { end.wrapping_add(length).write(0) };
    destination
}

/// # Safety
/// `destination` must hold `size` bytes; `source` must be a NUL-terminated string.
pub unsafe extern "C" fn strlcpy(
    destination: *mut c_char,
    source: *const c_char,
    size: usize,
) -> usize {
    // SAFETY: the caller's promise.
    let length = unsafe { strlen(source) };
    if size > 0 {
        let copied = length.min(size - 1);
        // SAFETY: as above.
        unsafe { destination.copy_from_nonoverlapping(source, copied) };
        // SAFETY: as above.
        unsafe { destination.wrapping_add(copied).write(0) };
    }
    length
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn strchr(text: *const c_char, character: c_int) -> *mut c_char {
    let wanted = character as u8;
    let mut at = text;
    loop {
        // SAFETY: the caller's promise; the walk stops at the NUL.
        let byte = unsafe { at.cast::<u8>().read() };
        if byte == wanted {
            return at.cast_mut();
        }
        if byte == 0 {
            return core::ptr::null_mut();
        }
        at = at.wrapping_add(1);
    }
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn strrchr(text: *const c_char, character: c_int) -> *mut c_char {
    // SAFETY: the caller's promise.
    let bytes = unsafe { c_string(text) };
    let wanted = character as u8;
    if wanted == 0 {
        return text.wrapping_add(bytes.len()).cast_mut();
    }
    bytes
        .iter()
        .rposition(|&b| b == wanted)
        .map_or(core::ptr::null_mut(), |index| {
            text.wrapping_add(index).cast_mut()
        })
}

/// # Safety
/// Both must be NUL-terminated strings.
pub unsafe extern "C" fn strstr(text: *const c_char, needle: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    let haystack = unsafe { c_string(text) };
    // SAFETY: as above.
    let needle = unsafe { c_string(needle) };
    if needle.is_empty() {
        return text.cast_mut();
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .map_or(core::ptr::null_mut(), |index| {
            text.wrapping_add(index).cast_mut()
        })
}

/// # Safety
/// `text` must be a NUL-terminated string.
pub unsafe extern "C" fn strdup(text: *const c_char) -> *mut c_char {
    // SAFETY: the caller's promise.
    unsafe { strndup(text, usize::MAX) }
}

/// # Safety
/// `text` must be readable up to `limit` bytes or its NUL.
pub unsafe extern "C" fn strndup(text: *const c_char, limit: usize) -> *mut c_char {
    // SAFETY: the caller's promise.
    let length = unsafe { strnlen(text, limit) };
    let copy = heap::allocate(length + 1, false).cast::<c_char>();
    if !copy.is_null() {
        // SAFETY: `copy` holds `length + 1` bytes.
        unsafe { copy.copy_from_nonoverlapping(text, length) };
        // SAFETY: as above.
        unsafe { copy.wrapping_add(length).write(0) };
    }
    copy
}

/// # Safety
/// `bytes` must be readable for `count` bytes.
pub unsafe extern "C" fn memchr(
    bytes: *const c_void,
    character: c_int,
    count: usize,
) -> *mut c_void {
    // SAFETY: the caller's promise.
    let slice = unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), count) };
    slice
        .iter()
        .position(|&b| b == character as u8)
        .map_or(core::ptr::null_mut(), |index| {
            bytes.wrapping_byte_add(index).cast_mut()
        })
}

/// # Safety
/// Both must be readable for `count` bytes.
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> c_int {
    // SAFETY: the caller's promise.
    let a = unsafe { core::slice::from_raw_parts(left.cast::<u8>(), count) };
    // SAFETY: as above.
    let b = unsafe { core::slice::from_raw_parts(right.cast::<u8>(), count) };
    a.iter()
        .zip(b)
        .find(|(x, y)| x != y)
        .map_or(0, |(x, y)| c_int::from(*x) - c_int::from(*y))
}

/// # Safety
/// `destination` must be writable and `source` readable for `count` bytes, not overlapping.
pub unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    // SAFETY: the caller's promise.
    unsafe {
        destination
            .cast::<u8>()
            .copy_from_nonoverlapping(source.cast(), count)
    };
    destination
}

/// # Safety
/// `destination` must be writable and `source` readable for `count` bytes.
pub unsafe extern "C" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    // SAFETY: the caller's promise.
    unsafe { destination.cast::<u8>().copy_from(source.cast(), count) };
    destination
}

/// # Safety
/// `destination` must be writable for `count` bytes.
pub unsafe extern "C" fn memset(
    destination: *mut c_void,
    value: c_int,
    count: usize,
) -> *mut c_void {
    // SAFETY: the caller's promise.
    unsafe { destination.cast::<u8>().write_bytes(value as u8, count) };
    destination
}

/// # Safety
/// `destination` must be writable for `count` bytes.
pub unsafe extern "C" fn bzero(destination: *mut c_void, count: usize) {
    // SAFETY: the caller's promise.
    unsafe { destination.cast::<u8>().write_bytes(0, count) };
}

fn overflow() -> ! {
    process::fail(b"detected buffer overflow")
}

/// # Safety
/// As `memcpy`.
pub unsafe extern "C" fn memcpy_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    size: usize,
) -> *mut c_void {
    if size != UNKNOWN_SIZE && count > size {
        overflow();
    }
    // SAFETY: the caller's promise, checked against the destination's size.
    unsafe { memcpy(destination, source, count) }
}

/// # Safety
/// As `memmove`.
pub unsafe extern "C" fn memmove_chk(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
    size: usize,
) -> *mut c_void {
    if size != UNKNOWN_SIZE && count > size {
        overflow();
    }
    // SAFETY: as above.
    unsafe { memmove(destination, source, count) }
}

/// # Safety
/// As `memset`.
pub unsafe extern "C" fn memset_chk(
    destination: *mut c_void,
    value: c_int,
    count: usize,
    size: usize,
) -> *mut c_void {
    if size != UNKNOWN_SIZE && count > size {
        overflow();
    }
    // SAFETY: as above.
    unsafe { memset(destination, value, count) }
}

/// # Safety
/// As `strcpy`.
pub unsafe extern "C" fn strcpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    size: usize,
) -> *mut c_char {
    // SAFETY: the caller's promise.
    if size != UNKNOWN_SIZE && unsafe { strlen(source) } >= size {
        overflow();
    }
    // SAFETY: as above.
    unsafe { strcpy(destination, source) }
}

/// # Safety
/// As `strncpy`.
pub unsafe extern "C" fn strncpy_chk(
    destination: *mut c_char,
    source: *const c_char,
    count: usize,
    size: usize,
) -> *mut c_char {
    if size != UNKNOWN_SIZE && count > size {
        overflow();
    }
    // SAFETY: as above.
    unsafe { strncpy(destination, source, count) }
}

/// # Safety
/// As `strcat`.
pub unsafe extern "C" fn strcat_chk(
    destination: *mut c_char,
    source: *const c_char,
    size: usize,
) -> *mut c_char {
    // SAFETY: the caller's promise.
    if size != UNKNOWN_SIZE && unsafe { strlen(destination) } + unsafe { strlen(source) } >= size {
        overflow();
    }
    // SAFETY: as above.
    unsafe { strcat(destination, source) }
}

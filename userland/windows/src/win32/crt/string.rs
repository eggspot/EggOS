//! Strings and characters: the narrow (`str*`, bytes in the ANSI code page, UTF-8 here) and
//! wide (`wcs*`, UTF-16 units) functions share one generic implementation each; the secure
//! `_s` forms return `EINVAL` or `ERANGE` and empty the destination, as Microsoft documents
//! for when the invalid-parameter handler lets the program go on; the character classes are
//! the C locale's; and the `strtol` family follows `c_text`.

use super::super::heap as process_heap;
use super::runtime::{errno, set_errno};
use crate::c_text::{self, Integer};
use crate::library::Export;
use crate::text;
use core::ptr;

trait Unit: Copy + PartialEq + Default + Into<u32> {}

impl Unit for u8 {}
impl Unit for u16 {}

unsafe fn length<T: Unit>(text: *const T) -> usize {
    let mut length = 0;
    // SAFETY: the caller's string is NUL-terminated, so every unit up to the NUL is readable.
    while unsafe { text.wrapping_add(length).read() } != T::default() {
        length += 1;
    }
    length
}

unsafe fn bounded_length<T: Unit>(text: *const T, limit: usize) -> usize {
    let mut length = 0;
    // SAFETY: the caller's string has a NUL or `limit` readable units.
    while length < limit && unsafe { text.wrapping_add(length).read() } != T::default() {
        length += 1;
    }
    length
}

unsafe fn units<'a, T: Unit>(text: *const T) -> &'a [T] {
    // SAFETY: the caller's NUL-terminated string.
    let length = unsafe { length(text) };
    // SAFETY: as above: the units before its NUL.
    unsafe { core::slice::from_raw_parts(text, length) }
}

fn folded(unit: u32, fold: bool) -> u32 {
    match fold && (u32::from(b'A')..=u32::from(b'Z')).contains(&unit) {
        true => unit + 32,
        false => unit,
    }
}

unsafe fn compare<T: Unit>(a: *const T, b: *const T, limit: usize, fold: bool) -> i32 {
    for at in 0..limit {
        // SAFETY: the caller's strings are NUL-terminated; neither is read past its NUL.
        let x: u32 = unsafe { a.wrapping_add(at).read() }.into();
        // SAFETY: as above.
        let y: u32 = unsafe { b.wrapping_add(at).read() }.into();
        let (x, y) = (folded(x, fold), folded(y, fold));
        if x != y {
            return if x < y { -1 } else { 1 };
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

unsafe fn copy<T: Unit>(destination: *mut T, source: *const T) -> *mut T {
    // SAFETY: the caller's NUL-terminated source.
    let units = unsafe { length(source) } + 1;
    // SAFETY: the caller's duty: `destination` holds the source and its NUL.
    unsafe { ptr::copy(source, destination, units) };
    destination
}

unsafe fn copy_at_most<T: Unit>(destination: *mut T, source: *const T, count: usize) -> *mut T {
    // SAFETY: the caller's source has a NUL or `count` readable units.
    let taken = unsafe { bounded_length(source, count) };
    // SAFETY: the caller's duty: `destination` holds `count` units.
    unsafe { ptr::copy(source, destination, taken) };
    // SAFETY: as above; the rest of the `count` units.
    unsafe {
        destination
            .wrapping_add(taken)
            .write_bytes(0, count - taken)
    };
    destination
}

unsafe fn append<T: Unit>(destination: *mut T, source: *const T, limit: usize) -> *mut T {
    // SAFETY: the caller's NUL-terminated destination.
    let end = destination.wrapping_add(unsafe { length(destination) });
    // SAFETY: the caller's source has a NUL or `limit` readable units.
    let taken = unsafe { bounded_length(source, limit) };
    // SAFETY: the caller's duty: `destination` holds both strings and a NUL.
    unsafe { ptr::copy(source, end, taken) };
    // SAFETY: as above.
    unsafe { end.wrapping_add(taken).write(T::default()) };
    destination
}

unsafe fn find<T: Unit>(text: *const T, wanted: T) -> *mut T {
    let mut at = text;
    loop {
        // SAFETY: the caller's NUL-terminated string; the search stops at its NUL.
        let unit = unsafe { at.read() };
        if unit == wanted {
            return at.cast_mut();
        }
        if unit == T::default() {
            return ptr::null_mut();
        }
        at = at.wrapping_add(1);
    }
}

unsafe fn find_last<T: Unit>(text: *const T, wanted: T) -> *mut T {
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe { length(text) } + 1;
    // SAFETY: as above, with its NUL.
    let whole = unsafe { core::slice::from_raw_parts(text, units) };
    whole
        .iter()
        .rposition(|unit| *unit == wanted)
        .map_or(ptr::null_mut(), |at| text.wrapping_add(at).cast_mut())
}

unsafe fn find_text<T: Unit>(haystack: *const T, needle: *const T) -> *mut T {
    // SAFETY: the caller's NUL-terminated strings.
    let whole = unsafe { units(haystack) };
    // SAFETY: as above.
    let wanted = unsafe { units(needle) };
    if wanted.is_empty() {
        return haystack.cast_mut();
    }
    whole
        .windows(wanted.len())
        .position(|window| window == wanted)
        .map_or(ptr::null_mut(), |at| haystack.wrapping_add(at).cast_mut())
}

unsafe fn duplicate<T: Unit>(text: *const T) -> *mut T {
    if text.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe { length(text) } + 1;
    let copy = process_heap::allocate_with(units * core::mem::size_of::<T>(), false).cast::<T>();
    if copy.is_null() {
        set_errno(errno::ENOMEM);
        return copy;
    }
    // SAFETY: a fresh block of `units` units.
    unsafe { ptr::copy_nonoverlapping(text, copy, units) };
    copy
}

unsafe fn copy_checked<T: Unit>(destination: *mut T, size: usize, source: *const T) -> i32 {
    if destination.is_null() || size == 0 {
        return refused(errno::EINVAL);
    }
    if source.is_null() {
        // SAFETY: `destination` holds `size` units, at least one.
        unsafe { destination.write(T::default()) };
        return refused(errno::EINVAL);
    }
    // SAFETY: the caller's NUL-terminated source.
    let needed = unsafe { length(source) } + 1;
    if needed > size {
        // SAFETY: as above.
        unsafe { destination.write(T::default()) };
        return refused(errno::ERANGE);
    }
    // SAFETY: `destination` holds `size` units, enough for the source and its NUL.
    unsafe { ptr::copy(source, destination, needed) };
    0
}

unsafe fn append_checked<T: Unit>(destination: *mut T, size: usize, source: *const T) -> i32 {
    if destination.is_null() || size == 0 {
        return refused(errno::EINVAL);
    }
    // SAFETY: `destination` holds `size` units.
    let used = unsafe { bounded_length(destination, size) };
    if source.is_null() || used == size {
        // SAFETY: as above.
        unsafe { destination.write(T::default()) };
        return refused(errno::EINVAL);
    }
    // SAFETY: the caller's NUL-terminated source.
    let needed = unsafe { length(source) } + 1;
    if used + needed > size {
        // SAFETY: as above.
        unsafe { destination.write(T::default()) };
        return refused(errno::ERANGE);
    }
    // SAFETY: the destination's room holds the source and its NUL.
    unsafe { ptr::copy(source, destination.wrapping_add(used), needed) };
    0
}

fn refused(code: i32) -> i32 {
    set_errno(code);
    code
}

unsafe fn integer<T: Unit>(text: *const T, end: *mut *mut T, base: i32) -> Option<Integer> {
    if text.is_null() {
        set_errno(errno::EINVAL);
        return None;
    }
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe { units(text) };
    let parsed = u32::try_from(base)
        .ok()
        .and_then(|base| c_text::parse_integer(units, base));
    if parsed.is_none() {
        set_errno(errno::EINVAL);
    }
    if !end.is_null() {
        let consumed = parsed.map_or(0, |parsed| parsed.consumed);
        // SAFETY: the caller passes a writable pointer for the end.
        unsafe { end.write(text.wrapping_add(consumed).cast_mut()) };
    }
    parsed
}

fn signed(parsed: Option<Integer>, bits: u32) -> i64 {
    let Some(parsed) = parsed else {
        return 0;
    };
    let (value, out_of_range) = parsed.signed(bits);
    if out_of_range {
        set_errno(errno::ERANGE);
    }
    value
}

fn unsigned(parsed: Option<Integer>, bits: u32) -> u64 {
    let Some(parsed) = parsed else {
        return 0;
    };
    let (value, out_of_range) = parsed.unsigned(bits);
    if out_of_range {
        set_errno(errno::ERANGE);
    }
    value
}

unsafe extern "efiapi" fn memchr(block: *const u8, value: i32, count: usize) -> *mut u8 {
    // SAFETY: `memchr`'s contract: `count` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(block, count) };
    bytes
        .iter()
        .position(|byte| *byte == value as u8)
        .map_or(ptr::null_mut(), |at| block.wrapping_add(at).cast_mut())
}

unsafe extern "efiapi" fn strchr(text: *const u8, value: i32) -> *mut u8 {
    // SAFETY: `strchr`'s contract: a NUL-terminated string.
    unsafe { find(text, value as u8) }
}

unsafe extern "efiapi" fn strrchr(text: *const u8, value: i32) -> *mut u8 {
    // SAFETY: `strrchr`'s contract: a NUL-terminated string.
    unsafe { find_last(text, value as u8) }
}

unsafe extern "efiapi" fn strstr(haystack: *const u8, needle: *const u8) -> *mut u8 {
    // SAFETY: `strstr`'s contract: NUL-terminated strings.
    unsafe { find_text(haystack, needle) }
}

unsafe extern "efiapi" fn wcschr(text: *const u16, value: u16) -> *mut u16 {
    // SAFETY: `wcschr`'s contract: a NUL-terminated string.
    unsafe { find(text, value) }
}

unsafe extern "efiapi" fn wcsrchr(text: *const u16, value: u16) -> *mut u16 {
    // SAFETY: `wcsrchr`'s contract: a NUL-terminated string.
    unsafe { find_last(text, value) }
}

unsafe extern "efiapi" fn wcsstr(haystack: *const u16, needle: *const u16) -> *mut u16 {
    // SAFETY: `wcsstr`'s contract: NUL-terminated strings.
    unsafe { find_text(haystack, needle) }
}

unsafe extern "efiapi" fn strcmp(a: *const u8, b: *const u8) -> i32 {
    // SAFETY: `strcmp`'s contract: NUL-terminated strings.
    unsafe { compare(a, b, usize::MAX, false) }
}

unsafe extern "efiapi" fn strncmp(a: *const u8, b: *const u8, count: usize) -> i32 {
    // SAFETY: `strncmp`'s contract.
    unsafe { compare(a, b, count, false) }
}

unsafe extern "efiapi" fn stricmp(a: *const u8, b: *const u8) -> i32 {
    // SAFETY: `_stricmp`'s contract: NUL-terminated strings.
    unsafe { compare(a, b, usize::MAX, true) }
}

unsafe extern "efiapi" fn strnicmp(a: *const u8, b: *const u8, count: usize) -> i32 {
    // SAFETY: `_strnicmp`'s contract.
    unsafe { compare(a, b, count, true) }
}

unsafe extern "efiapi" fn strcpy(destination: *mut u8, source: *const u8) -> *mut u8 {
    // SAFETY: `strcpy`'s contract.
    unsafe { copy(destination, source) }
}

unsafe extern "efiapi" fn strncpy(
    destination: *mut u8,
    source: *const u8,
    count: usize,
) -> *mut u8 {
    // SAFETY: `strncpy`'s contract.
    unsafe { copy_at_most(destination, source, count) }
}

unsafe extern "efiapi" fn strcat(destination: *mut u8, source: *const u8) -> *mut u8 {
    // SAFETY: `strcat`'s contract.
    unsafe { append(destination, source, usize::MAX) }
}

unsafe extern "efiapi" fn strncat(
    destination: *mut u8,
    source: *const u8,
    count: usize,
) -> *mut u8 {
    // SAFETY: `strncat`'s contract.
    unsafe { append(destination, source, count) }
}

unsafe extern "efiapi" fn strnlen(text: *const u8, limit: usize) -> usize {
    // SAFETY: `strnlen`'s contract.
    unsafe { bounded_length(text, limit) }
}

unsafe extern "efiapi" fn strdup(text: *const u8) -> *mut u8 {
    // SAFETY: `_strdup`'s contract.
    unsafe { duplicate(text) }
}

unsafe extern "efiapi" fn strcpy_s(destination: *mut u8, size: usize, source: *const u8) -> i32 {
    // SAFETY: `strcpy_s`'s contract.
    unsafe { copy_checked(destination, size, source) }
}

unsafe extern "efiapi" fn strcat_s(destination: *mut u8, size: usize, source: *const u8) -> i32 {
    // SAFETY: `strcat_s`'s contract.
    unsafe { append_checked(destination, size, source) }
}

unsafe extern "efiapi" fn wcslen(text: *const u16) -> usize {
    // SAFETY: `wcslen`'s contract.
    unsafe { length(text) }
}

unsafe extern "efiapi" fn wcsnlen(text: *const u16, limit: usize) -> usize {
    // SAFETY: `wcsnlen`'s contract.
    unsafe { bounded_length(text, limit) }
}

unsafe extern "efiapi" fn wcscmp(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `wcscmp`'s contract.
    unsafe { compare(a, b, usize::MAX, false) }
}

unsafe extern "efiapi" fn wcsncmp(a: *const u16, b: *const u16, count: usize) -> i32 {
    // SAFETY: `wcsncmp`'s contract.
    unsafe { compare(a, b, count, false) }
}

unsafe extern "efiapi" fn wcsicmp(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `_wcsicmp`'s contract.
    unsafe { compare(a, b, usize::MAX, true) }
}

unsafe extern "efiapi" fn wcscpy(destination: *mut u16, source: *const u16) -> *mut u16 {
    // SAFETY: `wcscpy`'s contract.
    unsafe { copy(destination, source) }
}

unsafe extern "efiapi" fn wcsncpy(
    destination: *mut u16,
    source: *const u16,
    count: usize,
) -> *mut u16 {
    // SAFETY: `wcsncpy`'s contract.
    unsafe { copy_at_most(destination, source, count) }
}

unsafe extern "efiapi" fn wcscat(destination: *mut u16, source: *const u16) -> *mut u16 {
    // SAFETY: `wcscat`'s contract.
    unsafe { append(destination, source, usize::MAX) }
}

unsafe extern "efiapi" fn wcsdup(text: *const u16) -> *mut u16 {
    // SAFETY: `_wcsdup`'s contract.
    unsafe { duplicate(text) }
}

unsafe extern "efiapi" fn wcscpy_s(destination: *mut u16, size: usize, source: *const u16) -> i32 {
    // SAFETY: `wcscpy_s`'s contract.
    unsafe { copy_checked(destination, size, source) }
}

unsafe extern "efiapi" fn wcscat_s(destination: *mut u16, size: usize, source: *const u16) -> i32 {
    // SAFETY: `wcscat_s`'s contract.
    unsafe { append_checked(destination, size, source) }
}

fn ascii(character: i32) -> Option<u8> {
    u8::try_from(character).ok().filter(u8::is_ascii)
}

fn class(character: i32, test: fn(&u8) -> bool) -> i32 {
    i32::from(ascii(character).is_some_and(|byte| test(&byte)))
}

extern "efiapi" fn isalpha(character: i32) -> i32 {
    class(character, u8::is_ascii_alphabetic)
}

extern "efiapi" fn isdigit(character: i32) -> i32 {
    class(character, u8::is_ascii_digit)
}

extern "efiapi" fn isalnum(character: i32) -> i32 {
    class(character, u8::is_ascii_alphanumeric)
}

extern "efiapi" fn isspace(character: i32) -> i32 {
    i32::from(u32::try_from(character).is_ok_and(c_text::is_space))
}

extern "efiapi" fn isupper(character: i32) -> i32 {
    class(character, u8::is_ascii_uppercase)
}

extern "efiapi" fn islower(character: i32) -> i32 {
    class(character, u8::is_ascii_lowercase)
}

extern "efiapi" fn isxdigit(character: i32) -> i32 {
    class(character, u8::is_ascii_hexdigit)
}

extern "efiapi" fn ispunct(character: i32) -> i32 {
    class(character, u8::is_ascii_punctuation)
}

extern "efiapi" fn iscntrl(character: i32) -> i32 {
    class(character, u8::is_ascii_control)
}

extern "efiapi" fn isprint(character: i32) -> i32 {
    class(character, |byte| (0x20..0x7f).contains(byte))
}

extern "efiapi" fn tolower(character: i32) -> i32 {
    ascii(character).map_or(character, |byte| i32::from(byte.to_ascii_lowercase()))
}

extern "efiapi" fn toupper(character: i32) -> i32 {
    ascii(character).map_or(character, |byte| i32::from(byte.to_ascii_uppercase()))
}

extern "efiapi" fn towlower(character: u16) -> u16 {
    text::lower(character)
}

extern "efiapi" fn towupper(character: u16) -> u16 {
    text::upper(character)
}

extern "efiapi" fn iswspace(character: u16) -> i32 {
    i32::from(c_text::is_space(u32::from(character)))
}

extern "efiapi" fn iswalpha(character: u16) -> i32 {
    i32::from(char::from_u32(u32::from(character)).is_some_and(char::is_alphabetic))
}

extern "efiapi" fn iswdigit(character: u16) -> i32 {
    i32::from((u16::from(b'0')..=u16::from(b'9')).contains(&character))
}

unsafe extern "efiapi" fn strtol(text: *const u8, end: *mut *mut u8, base: i32) -> i32 {
    // SAFETY: `strtol`'s contract.
    signed(unsafe { integer(text, end, base) }, 32) as i32
}

unsafe extern "efiapi" fn strtoul(text: *const u8, end: *mut *mut u8, base: i32) -> u32 {
    // SAFETY: `strtoul`'s contract.
    unsigned(unsafe { integer(text, end, base) }, 32) as u32
}

unsafe extern "efiapi" fn strtoll(text: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    // SAFETY: `strtoll`'s contract.
    signed(unsafe { integer(text, end, base) }, 64)
}

unsafe extern "efiapi" fn strtoull(text: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    // SAFETY: `strtoull`'s contract.
    unsigned(unsafe { integer(text, end, base) }, 64)
}

unsafe extern "efiapi" fn atoi(text: *const u8) -> i32 {
    // SAFETY: `atoi`'s contract: a NUL-terminated string.
    signed(unsafe { integer(text, ptr::null_mut(), 10) }, 32) as i32
}

unsafe extern "efiapi" fn atoll(text: *const u8) -> i64 {
    // SAFETY: `atoll`'s contract: a NUL-terminated string.
    signed(unsafe { integer(text, ptr::null_mut(), 10) }, 64)
}

unsafe extern "efiapi" fn wcstol(text: *const u16, end: *mut *mut u16, base: i32) -> i32 {
    // SAFETY: `wcstol`'s contract.
    signed(unsafe { integer(text, end, base) }, 32) as i32
}

unsafe extern "efiapi" fn wcstoul(text: *const u16, end: *mut *mut u16, base: i32) -> u32 {
    // SAFETY: `wcstoul`'s contract.
    unsigned(unsafe { integer(text, end, base) }, 32) as u32
}

unsafe extern "efiapi" fn wtoi(text: *const u16) -> i32 {
    // SAFETY: `_wtoi`'s contract: a NUL-terminated string.
    signed(unsafe { integer(text, ptr::null_mut(), 10) }, 32) as i32
}

pub(super) const VCRUNTIME_EXPORTS: &[Export] = exports![
    b"memchr" => memchr,
    b"strchr" => strchr,
    b"strrchr" => strrchr,
    b"strstr" => strstr,
    b"wcschr" => wcschr,
    b"wcsrchr" => wcsrchr,
    b"wcsstr" => wcsstr,
];

pub(super) const EXPORTS: &[Export] = exports![
    b"strcmp" => strcmp,
    b"strncmp" => strncmp,
    b"_stricmp" => stricmp,
    b"_strnicmp" => strnicmp,
    b"strcpy" => strcpy,
    b"strncpy" => strncpy,
    b"strcat" => strcat,
    b"strncat" => strncat,
    b"strnlen" => strnlen,
    b"_strdup" => strdup,
    b"strcpy_s" => strcpy_s,
    b"strcat_s" => strcat_s,
    b"wcslen" => wcslen,
    b"wcsnlen" => wcsnlen,
    b"wcscmp" => wcscmp,
    b"wcsncmp" => wcsncmp,
    b"_wcsicmp" => wcsicmp,
    b"wcscpy" => wcscpy,
    b"wcsncpy" => wcsncpy,
    b"wcscat" => wcscat,
    b"_wcsdup" => wcsdup,
    b"wcscpy_s" => wcscpy_s,
    b"wcscat_s" => wcscat_s,
    b"isalpha" => isalpha,
    b"isdigit" => isdigit,
    b"isalnum" => isalnum,
    b"isspace" => isspace,
    b"isupper" => isupper,
    b"islower" => islower,
    b"isxdigit" => isxdigit,
    b"ispunct" => ispunct,
    b"iscntrl" => iscntrl,
    b"isprint" => isprint,
    b"tolower" => tolower,
    b"toupper" => toupper,
    b"towlower" => towlower,
    b"towupper" => towupper,
    b"iswspace" => iswspace,
    b"iswalpha" => iswalpha,
    b"iswdigit" => iswdigit,
    b"strtol" => strtol,
    b"strtoul" => strtoul,
    b"strtoll" => strtoll,
    b"strtoull" => strtoull,
    b"atoi" => atoi,
    b"atol" => atoi,
    b"atoll" => atoll,
    b"wcstol" => wcstol,
    b"wcstoul" => wcstoul,
    b"_wtoi" => wtoi,
    b"_wtol" => wtoi,
];

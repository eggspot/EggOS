//! shlwapi (D178): `StrCmpLogicalW`, the order Explorer sorts names in: runs of digits compare
//! as numbers, the rest ignoring case (`crate::text::compare_logical`); and `StrRetToBufW` and
//! `StrRetToStrW` (D399), which read the `STRRET` a shell folder's `GetDisplayNameOf` fills, its
//! narrow forms taken as UTF-8 like every narrow string of the layer.

use super::heap;
use crate::library::Export;
use crate::text;
use core::cmp::Ordering;

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000e_u32 as i32;
const STRRET_WSTR: u32 = 0;
const STRRET_OFFSET: u32 = 1;
const STRRET_CSTR: u32 = 2;
const STRRET_UNION: usize = 8;
const STRRET_TEXT_BYTES: usize = 260;
const LONGEST_TEXT: usize = 0x8000;

unsafe fn terminated<'a>(text: *const u16) -> &'a [u16] {
    match text.is_null() {
        true => &[],
        // SAFETY: the caller's duty: a NUL-terminated string, or null for an empty one.
        false => unsafe { super::file::wide_string(text) },
    }
}

unsafe extern "efiapi" fn str_cmp_logical_w(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `StrCmpLogicalW`'s contract: NUL-terminated strings.
    let a = unsafe { terminated(a) };
    // SAFETY: as above.
    let b = unsafe { terminated(b) };
    match crate::text::compare_logical(a, b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// The narrow text at `start` up to its NUL or `longest` bytes.
///
/// # Safety
/// `start` must hold `longest` readable bytes, or a NUL before them.
unsafe fn narrow_text<'a>(start: *const u8, longest: usize) -> &'a [u8] {
    let mut length = 0;
    // SAFETY: the caller's duty.
    while length < longest && unsafe { start.add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: as above: the bytes before the NUL.
    unsafe { core::slice::from_raw_parts(start, length) }
}

/// The text of the `STRRET` at `record`, written into `out` as UTF-16: the units written, and
/// for `STRRET_WSTR` the string, which the caller frees.
///
/// # Safety
/// `record` must be a readable `STRRET`, and `list` the item list it came with when it is
/// `STRRET_OFFSET`.
unsafe fn str_ret_text(
    record: *const u8,
    list: *const u8,
    out: &mut [u16],
) -> Result<(usize, *mut u16), i32> {
    // SAFETY: the caller's duty: the record's kind.
    let kind = unsafe { record.cast::<u32>().read_unaligned() };
    let union = record.wrapping_add(STRRET_UNION);
    match kind {
        STRRET_WSTR => {
            // SAFETY: as above: the string's pointer.
            let string = unsafe { union.cast::<*mut u16>().read_unaligned() };
            if string.is_null() {
                return Err(E_INVALIDARG);
            }
            // SAFETY: a NUL-terminated string the shell folder allocated.
            let units = unsafe { super::file::wide_string(string) };
            let length = units.len().min(out.len());
            out[..length].copy_from_slice(&units[..length]);
            Ok((length, string))
        }
        STRRET_OFFSET if !list.is_null() => {
            // SAFETY: as above: the offset into the list.
            let offset = unsafe { union.cast::<u32>().read_unaligned() } as usize;
            // SAFETY: as above: the list holds the NUL-terminated text at the offset.
            let bytes = unsafe { narrow_text(list.wrapping_add(offset), LONGEST_TEXT) };
            Ok((text::write_utf16(bytes, out), core::ptr::null_mut()))
        }
        STRRET_CSTR => {
            // SAFETY: as above: the record's text field.
            let bytes = unsafe { narrow_text(union, STRRET_TEXT_BYTES) };
            Ok((text::write_utf16(bytes, out), core::ptr::null_mut()))
        }
        _ => Err(E_INVALIDARG),
    }
}

unsafe extern "efiapi" fn str_ret_to_buf_w(
    record: *mut u8,
    list: *const u8,
    out: *mut u16,
    capacity: u32,
) -> i32 {
    if record.is_null() || out.is_null() || capacity == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: `StrRetToBufW`'s contract: a buffer of `capacity` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(out, capacity as usize) };
    let last = out.len() - 1;
    // SAFETY: as above: a readable `STRRET` and its list.
    let found = unsafe { str_ret_text(record, list, &mut out[..last]) };
    match found {
        Ok((length, string)) => {
            out[length] = 0;
            // SAFETY: the record's own string, which `StrRetToBufW` frees.
            unsafe { heap::free(string.cast()) };
            S_OK
        }
        Err(failure) => {
            out[0] = 0;
            failure
        }
    }
}

unsafe extern "efiapi" fn str_ret_to_str_w(
    record: *mut u8,
    list: *const u8,
    out: *mut *mut u16,
) -> i32 {
    if record.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller passes a writable string pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    // SAFETY: `StrRetToStrW`'s contract: a readable `STRRET` and its list.
    let kind = unsafe { record.cast::<u32>().read_unaligned() };
    if kind == STRRET_WSTR {
        // SAFETY: as above: the string's pointer, which passes to the caller.
        let string = unsafe { record.add(STRRET_UNION).cast::<*mut u16>().read_unaligned() };
        // SAFETY: as above.
        unsafe { out.write(string) };
        return if string.is_null() { E_INVALIDARG } else { S_OK };
    }
    let mut units = [0u16; STRRET_TEXT_BYTES * 2];
    // SAFETY: as above.
    let found = unsafe { str_ret_text(record, list, &mut units) };
    let Ok((length, _)) = found else {
        return E_INVALIDARG;
    };
    let block = heap::allocate((length + 1) * 2).cast::<u16>();
    if block.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh, zeroed block of `length + 1` units: the text and its NUL.
    unsafe { block.copy_from_nonoverlapping(units.as_ptr(), length) };
    // SAFETY: as above.
    unsafe { out.write(block) };
    S_OK
}

pub(super) const EXPORTS: &[Export] = exports![
    b"StrCmpLogicalW" => str_cmp_logical_w,
    b"StrRetToBufW" => str_ret_to_buf_w,
    b"StrRetToStrW" => str_ret_to_str_w,
];

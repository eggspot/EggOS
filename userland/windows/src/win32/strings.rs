//! Strings: conversions between UTF-16 and the code pages (UTF-8 for `CP_UTF8` and, since the
//! layer's ANSI and OEM code pages are UTF-8, for those too), lengths, ordinal comparisons,
//! upper and lower case, user32's character classes (`IsCharAlphaW` and its companions, by
//! Unicode's properties, D178), and `FormatMessage` for the system's messages.

use super::{Bool, FALSE, error, fail, heap};
use crate::errors;
use crate::library::Export;
use crate::text;
use core::cmp::Ordering;

const CP_ACP: u32 = 0;
const CP_OEMCP: u32 = 1;
const CP_THREAD_ACP: u32 = 3;
const CP_UTF8: u32 = 65001;
const MB_ERR_INVALID_CHARS: u32 = 0x8;
const WC_ERR_INVALID_CHARS: u32 = 0x80;
const NORM_IGNORECASE: u32 = 0x1;
const LINGUISTIC_IGNORECASE: u32 = 0x10;
const CSTR_LESS_THAN: i32 = 1;
const CSTR_EQUAL: i32 = 2;
const CSTR_GREATER_THAN: i32 = 3;
const FORMAT_MESSAGE_ALLOCATE_BUFFER: u32 = 0x100;
const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x200;
const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x1000;
const FORMAT_MESSAGE_MAX_WIDTH_MASK: u32 = 0xff;
const ERROR_MR_MID_NOT_FOUND: u32 = 317;

fn is_utf8(code_page: u32) -> bool {
    matches!(code_page, CP_ACP | CP_OEMCP | CP_THREAD_ACP | CP_UTF8)
}

unsafe fn wide_input<'a>(text: *const u16, length: i32) -> Option<&'a [u16]> {
    if text.is_null() {
        return None;
    }
    let length = match length {
        -1 => {
            // SAFETY: the caller's duty: a NUL-terminated string.
            unsafe { super::file::wide_string(text) }.len() + 1
        }
        0.. => length as usize,
        _ => return None,
    };
    // SAFETY: the caller's duty.
    Some(unsafe { core::slice::from_raw_parts(text, length) })
}

unsafe extern "efiapi" fn multi_byte_to_wide_char(
    code_page: u32,
    flags: u32,
    bytes: *const u8,
    byte_count: i32,
    out: *mut u16,
    out_count: i32,
) -> i32 {
    if !is_utf8(code_page) || bytes.is_null() || byte_count == 0 || out_count < 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let length = match byte_count {
        -1 => {
            // SAFETY: `MultiByteToWideChar`'s contract: with -1, a NUL-terminated string.
            unsafe { core::ffi::CStr::from_ptr(bytes.cast()) }
                .to_bytes()
                .len()
                + 1
        }
        1.. => byte_count as usize,
        _ => return fail(error::INVALID_PARAMETER, 0),
    };
    // SAFETY: as above, or `bytes` holds `byte_count` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(bytes, length) };
    if flags & MB_ERR_INVALID_CHARS != 0 && core::str::from_utf8(bytes).is_err() {
        return fail(error::NO_UNICODE_TRANSLATION, 0);
    }
    let needed = text::utf16_length(bytes);
    if out_count == 0 {
        return needed as i32;
    }
    if needed > out_count as usize || out.is_null() {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller passes `out_count` writable units, at least `needed`.
    let out = unsafe { core::slice::from_raw_parts_mut(out, needed) };
    text::write_utf16(bytes, out) as i32
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn wide_char_to_multi_byte(
    code_page: u32,
    flags: u32,
    units: *const u16,
    unit_count: i32,
    out: *mut u8,
    out_count: i32,
    default_char: *const u8,
    used_default_char: *mut i32,
) -> i32 {
    let defaults_given = !default_char.is_null() || !used_default_char.is_null();
    if !is_utf8(code_page) || unit_count == 0 || out_count < 0 {
        return fail(error::INVALID_PARAMETER, 0);
    }
    if code_page == CP_UTF8 && defaults_given {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `WideCharToMultiByte`'s contract: `units` holds `unit_count` units, or is
    // NUL-terminated with -1.
    let Some(units) = (unsafe { wide_input(units, unit_count) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if flags & WC_ERR_INVALID_CHARS != 0 && text::has_unpaired_surrogate(units) {
        return fail(error::NO_UNICODE_TRANSLATION, 0);
    }
    if !used_default_char.is_null() {
        // SAFETY: the caller passes a writable `BOOL`.
        unsafe { used_default_char.write(FALSE) };
    }
    let needed = text::utf8_length(units);
    if out_count == 0 {
        return needed as i32;
    }
    if needed > out_count as usize || out.is_null() {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller passes `out_count` writable bytes, at least `needed`.
    let out = unsafe { core::slice::from_raw_parts_mut(out, needed) };
    text::write_utf8(units, out).0 as i32
}

unsafe extern "efiapi" fn lstrlen_w(text: *const u16) -> i32 {
    if text.is_null() {
        return 0;
    }
    // SAFETY: `lstrlenW`'s contract: a NUL-terminated string.
    unsafe { super::file::wide_string(text) }.len() as i32
}

unsafe extern "efiapi" fn lstrlen_a(text: *const u8) -> i32 {
    if text.is_null() {
        return 0;
    }
    // SAFETY: `lstrlenA`'s contract: a NUL-terminated string.
    unsafe { core::ffi::CStr::from_ptr(text.cast()) }
        .to_bytes()
        .len() as i32
}

fn sign(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn compare_result(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => CSTR_LESS_THAN,
        Ordering::Equal => CSTR_EQUAL,
        Ordering::Greater => CSTR_GREATER_THAN,
    }
}

unsafe fn compare_terminated(a: *const u16, b: *const u16, ignore_case: bool) -> i32 {
    let terminated = |text: *const u16| match text.is_null() {
        true => &[][..],
        // SAFETY: the callers' contract: NUL-terminated strings, or null for an empty one.
        false => unsafe { super::file::wide_string(text) },
    };
    sign(text::compare_ordinal(
        terminated(a),
        terminated(b),
        ignore_case,
    ))
}

unsafe extern "efiapi" fn lstrcmp_w(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `lstrcmpW`'s contract.
    unsafe { compare_terminated(a, b, false) }
}

unsafe extern "efiapi" fn lstrcmpi_w(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: `lstrcmpiW`'s contract.
    unsafe { compare_terminated(a, b, true) }
}

unsafe fn compare(a: *const u16, a_count: i32, b: *const u16, b_count: i32, flags: u32) -> i32 {
    // SAFETY: the caller's duty.
    let (Some(a), Some(b)) = (unsafe { wide_input(a, a_count) }, unsafe {
        wide_input(b, b_count)
    }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let a = if a_count == -1 { &a[..a.len() - 1] } else { a };
    let b = if b_count == -1 { &b[..b.len() - 1] } else { b };
    let ignore_case = flags & (NORM_IGNORECASE | LINGUISTIC_IGNORECASE) != 0;
    compare_result(text::compare_ordinal(a, b, ignore_case))
}

unsafe extern "efiapi" fn compare_string_ordinal(
    a: *const u16,
    a_count: i32,
    b: *const u16,
    b_count: i32,
    ignore_case: i32,
) -> i32 {
    let flags = if ignore_case != FALSE {
        NORM_IGNORECASE
    } else {
        0
    };
    // SAFETY: `CompareStringOrdinal`'s contract.
    unsafe { compare(a, a_count, b, b_count, flags) }
}

unsafe extern "efiapi" fn compare_string_w(
    _locale: u32,
    flags: u32,
    a: *const u16,
    a_count: i32,
    b: *const u16,
    b_count: i32,
) -> i32 {
    // SAFETY: `CompareStringW`'s contract.
    unsafe { compare(a, a_count, b, b_count, flags) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn compare_string_ex(
    _locale: *const u16,
    flags: u32,
    a: *const u16,
    a_count: i32,
    b: *const u16,
    b_count: i32,
    _version: usize,
    _reserved: usize,
    _parameter: usize,
) -> i32 {
    // SAFETY: `CompareStringEx`'s contract.
    unsafe { compare(a, a_count, b, b_count, flags) }
}

unsafe fn map_case(text: *mut u16, map: fn(u16) -> u16) -> *mut u16 {
    let address = text as usize;
    if address >> 16 == 0 {
        return map(address as u16) as usize as *mut u16;
    }
    let mut at = text;
    loop {
        // SAFETY: the callers' contract: a NUL-terminated, writable string.
        let unit = unsafe { at.read() };
        if unit == 0 {
            return text;
        }
        // SAFETY: as above.
        unsafe { at.write(map(unit)) };
        at = at.wrapping_add(1);
    }
}

unsafe extern "efiapi" fn char_upper_w(text: *mut u16) -> *mut u16 {
    // SAFETY: `CharUpperW`'s contract: one character in the low word, or a string.
    unsafe { map_case(text, text::upper) }
}

unsafe extern "efiapi" fn char_lower_w(text: *mut u16) -> *mut u16 {
    // SAFETY: `CharLowerW`'s contract.
    unsafe { map_case(text, text::lower) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn format_message_w(
    flags: u32,
    _source: usize,
    message_id: u32,
    _language: u32,
    buffer: *mut u16,
    size: u32,
    _arguments: usize,
) -> u32 {
    let known = FORMAT_MESSAGE_ALLOCATE_BUFFER
        | FORMAT_MESSAGE_IGNORE_INSERTS
        | FORMAT_MESSAGE_FROM_SYSTEM
        | FORMAT_MESSAGE_MAX_WIDTH_MASK;
    if flags & FORMAT_MESSAGE_FROM_SYSTEM == 0 || flags & !known != 0 || buffer.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let Some(message) = errors::message(message_id) else {
        return fail(ERROR_MR_MID_NOT_FOUND, 0);
    };
    let line_end = if flags & FORMAT_MESSAGE_MAX_WIDTH_MASK == 0 {
        "\r\n"
    } else {
        ""
    };
    let length = message.len() + line_end.len();
    let out = if flags & FORMAT_MESSAGE_ALLOCATE_BUFFER != 0 {
        let block = heap::allocate((length.max(size as usize) + 1) * 2).cast::<u16>();
        if block.is_null() {
            return fail(error::NOT_ENOUGH_MEMORY, 0);
        }
        // SAFETY: with `FORMAT_MESSAGE_ALLOCATE_BUFFER`, `buffer` is a writable `LPWSTR*`
        // that receives the block, which the caller frees with `LocalFree`.
        unsafe { buffer.cast::<*mut u16>().write(block) };
        block
    } else if length < size as usize {
        buffer
    } else {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    };
    let units = message
        .encode_utf16()
        .chain(line_end.encode_utf16())
        .chain([0]);
    for (at, unit) in units.enumerate() {
        // SAFETY: `out` holds more than `length` units (the message is ASCII).
        unsafe { out.wrapping_add(at).write(unit) };
    }
    super::thread_environment::set_last_error(error::SUCCESS);
    length as u32
}

unsafe extern "efiapi" fn format_message_a(
    flags: u32,
    _source: usize,
    message_id: u32,
    _language: u32,
    buffer: *mut u8,
    size: u32,
    _arguments: usize,
) -> u32 {
    let known = FORMAT_MESSAGE_ALLOCATE_BUFFER
        | FORMAT_MESSAGE_IGNORE_INSERTS
        | FORMAT_MESSAGE_FROM_SYSTEM
        | FORMAT_MESSAGE_MAX_WIDTH_MASK;
    if flags & FORMAT_MESSAGE_FROM_SYSTEM == 0 || flags & !known != 0 || buffer.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let Some(message) = errors::message(message_id) else {
        return fail(ERROR_MR_MID_NOT_FOUND, 0);
    };
    let line_end = if flags & FORMAT_MESSAGE_MAX_WIDTH_MASK == 0 {
        "\r\n"
    } else {
        ""
    };
    let length = message.len() + line_end.len();
    let out = if flags & FORMAT_MESSAGE_ALLOCATE_BUFFER != 0 {
        let block = heap::allocate(length.max(size as usize) + 1);
        if block.is_null() {
            return fail(error::NOT_ENOUGH_MEMORY, 0);
        }
        // SAFETY: with `FORMAT_MESSAGE_ALLOCATE_BUFFER`, `buffer` is a writable `LPSTR*` that
        // receives the block, which the caller frees with `LocalFree`.
        unsafe { buffer.cast::<*mut u8>().write(block) };
        block
    } else if length < size as usize {
        buffer
    } else {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    };
    let bytes = message.bytes().chain(line_end.bytes()).chain([0]);
    for (at, byte) in bytes.enumerate() {
        // SAFETY: `out` holds more than `length` bytes.
        unsafe { out.wrapping_add(at).write(byte) };
    }
    super::thread_environment::set_last_error(error::SUCCESS);
    length as u32
}

pub(super) const EXPORTS: &[Export] = exports![
    b"FormatMessageA" => format_message_a,
    b"MultiByteToWideChar" => multi_byte_to_wide_char,
    b"WideCharToMultiByte" => wide_char_to_multi_byte,
    b"lstrlenW" => lstrlen_w,
    b"lstrlenA" => lstrlen_a,
    b"lstrcmpW" => lstrcmp_w,
    b"lstrcmpiW" => lstrcmpi_w,
    b"CompareStringOrdinal" => compare_string_ordinal,
    b"CompareStringW" => compare_string_w,
    b"CompareStringEx" => compare_string_ex [two_stack_arguments],
    b"FormatMessageW" => format_message_w,
];

fn is_character(unit: u16, test: fn(char) -> bool) -> Bool {
    Bool::from(char::from_u32(u32::from(unit)).is_some_and(test))
}

extern "efiapi" fn is_char_alpha_w(unit: u16) -> Bool {
    is_character(unit, char::is_alphabetic)
}

extern "efiapi" fn is_char_alpha_numeric_w(unit: u16) -> Bool {
    is_character(unit, char::is_alphanumeric)
}

extern "efiapi" fn is_char_upper_w(unit: u16) -> Bool {
    is_character(unit, char::is_uppercase)
}

extern "efiapi" fn is_char_lower_w(unit: u16) -> Bool {
    is_character(unit, char::is_lowercase)
}

pub(super) const USER32_EXPORTS: &[Export] = exports![
    b"CharUpperW" => char_upper_w,
    b"CharLowerW" => char_lower_w,
    b"IsCharAlphaW" => is_char_alpha_w,
    b"IsCharAlphaNumericW" => is_char_alpha_numeric_w,
    b"IsCharUpperW" => is_char_upper_w,
    b"IsCharLowerW" => is_char_lower_w,
];

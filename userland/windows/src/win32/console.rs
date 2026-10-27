//! The console: the standard handles are descriptors 0 to 2, `WriteConsoleW` writes UTF-8 and
//! `ReadConsoleW` reads it, converted a stack buffer at a time, and the console's code pages are
//! UTF-8 (65001).

use super::file::{fd_of, handle_of, read_into, write_bytes};
use super::{Bool, FALSE, INVALID_HANDLE_VALUE, TRUE, error, fail};
use crate::library::Export;
use crate::linux;
use crate::text;
use core::ffi::c_void;

const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const CODE_PAGE_UTF8: u32 = 65001;
const INPUT_MODE: u32 = 0x1 | 0x2 | 0x4;
const OUTPUT_MODE: u32 = 0x1 | 0x2 | 0x4;
const CHUNK: usize = 1024;

extern "win64" fn get_std_handle(which: u32) -> usize {
    match which {
        STD_INPUT_HANDLE => handle_of(0),
        STD_OUTPUT_HANDLE => handle_of(1),
        STD_ERROR_HANDLE => handle_of(2),
        _ => fail(error::INVALID_HANDLE, INVALID_HANDLE_VALUE),
    }
}

fn console_fd(handle: usize) -> Option<usize> {
    fd_of(handle).filter(|fd| linux::is_terminal(*fd))
}

unsafe extern "win64" fn get_console_mode(handle: usize, mode: *mut u32) -> Bool {
    let Some(fd) = console_fd(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let value = if fd == 0 { INPUT_MODE } else { OUTPUT_MODE };
    // SAFETY: the caller passes a writable `DWORD`.
    unsafe { mode.write(value) };
    TRUE
}

extern "win64" fn set_console_mode(handle: usize, _mode: u32) -> Bool {
    match console_fd(handle) {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "win64" fn get_code_page() -> u32 {
    CODE_PAGE_UTF8
}

extern "win64" fn set_code_page(page: u32) -> Bool {
    match page {
        CODE_PAGE_UTF8 => TRUE,
        _ => fail(error::INVALID_PARAMETER, FALSE),
    }
}

unsafe extern "win64" fn write_console_w(
    handle: usize,
    buffer: *const u16,
    length: u32,
    written: *mut u32,
    _reserved: *mut c_void,
) -> Bool {
    // SAFETY: `WriteConsoleW`'s contract: `buffer` holds `length` UTF-16 units.
    let mut units = unsafe { core::slice::from_raw_parts(buffer, length as usize) };
    let mut bytes = [0u8; CHUNK];
    while !units.is_empty() {
        let (filled, consumed) = text::write_utf8(units, &mut bytes);
        if !write_bytes(handle, &bytes[..filled]) {
            return FALSE;
        }
        units = &units[consumed.max(1).min(units.len())..];
    }
    if !written.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the count.
        unsafe { written.write(length) };
    }
    TRUE
}

unsafe extern "win64" fn write_console_a(
    handle: usize,
    buffer: *const u8,
    length: u32,
    written: *mut u32,
    _reserved: *mut c_void,
) -> Bool {
    // SAFETY: `WriteConsoleA`'s contract: `buffer` holds `length` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(buffer, length as usize) };
    if !write_bytes(handle, bytes) {
        return FALSE;
    }
    if !written.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the count.
        unsafe { written.write(length) };
    }
    TRUE
}

unsafe extern "win64" fn read_console_w(
    handle: usize,
    buffer: *mut u16,
    length: u32,
    read: *mut u32,
    _control: *mut c_void,
) -> Bool {
    let mut bytes = [0u8; CHUNK];
    let wanted = (length as usize).min(CHUNK);
    // SAFETY: `bytes` holds `wanted` writable bytes.
    let Some(count) = (unsafe { read_into(handle, bytes.as_mut_ptr(), wanted) }) else {
        return FALSE;
    };
    // SAFETY: `ReadConsoleW`'s contract: `buffer` holds `length` writable units; UTF-8 never
    // takes fewer bytes than UTF-16 units, so `count` bytes fit.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, length as usize) };
    let units = text::write_utf16(&bytes[..count], out);
    if !read.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the count.
        unsafe { read.write(units as u32) };
    }
    TRUE
}

unsafe extern "win64" fn read_console_a(
    handle: usize,
    buffer: *mut u8,
    length: u32,
    read: *mut u32,
    _control: *mut c_void,
) -> Bool {
    // SAFETY: `ReadConsoleA`'s contract: `buffer` holds `length` writable bytes.
    let Some(count) = (unsafe { read_into(handle, buffer, length as usize) }) else {
        return FALSE;
    };
    if !read.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the count.
        unsafe { read.write(count as u32) };
    }
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetStdHandle" => get_std_handle,
    b"GetConsoleMode" => get_console_mode,
    b"SetConsoleMode" => set_console_mode,
    b"GetConsoleCP" => get_code_page,
    b"GetConsoleOutputCP" => get_code_page,
    b"GetACP" => get_code_page,
    b"GetOEMCP" => get_code_page,
    b"SetConsoleCP" => set_code_page,
    b"SetConsoleOutputCP" => set_code_page,
    b"WriteConsoleW" => write_console_w,
    b"WriteConsoleA" => write_console_a,
    b"ReadConsoleW" => read_console_w,
    b"ReadConsoleA" => read_console_a,
];

//! The console: the standard handles are descriptors 0 to 2, `WriteConsoleW` writes UTF-8 and
//! `ReadConsoleW` reads it, converted a stack buffer at a time, and the console's code pages are
//! UTF-8 (65001). A GUI program has no console, as on Windows: its standard handles are null
//! where they would be the terminal, and the files or pipes the shell redirected them to
//! otherwise (D129). `SetStdHandle` replaces what `GetStdHandle` answers (D178).

use super::file::{fd_of, handle_of, read_into, write_bytes};
use super::{Bool, FALSE, INVALID_HANDLE_VALUE, TRUE, error, fail};
use crate::library::Export;
use crate::linux;
use crate::text;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const CODE_PAGE_UTF8: u32 = 65001;
const INPUT_MODE: u32 = 0x1 | 0x2 | 0x4;
const OUTPUT_MODE: u32 = 0x1 | 0x2 | 0x4;
const CHUNK: usize = 1024;

static GUI_PROGRAM: AtomicBool = AtomicBool::new(false);

/// Marks the program as a GUI program (subsystem `WINDOWS`), which has no console.
pub fn set_gui_program() {
    GUI_PROGRAM.store(true, Ordering::Relaxed);
}

pub(super) fn standard_handle(fd: usize) -> usize {
    match GUI_PROGRAM.load(Ordering::Relaxed) && linux::is_terminal(fd) {
        true => 0,
        false => handle_of(fd),
    }
}

static REPLACED: [AtomicBool; 3] = [const { AtomicBool::new(false) }; 3];
static REPLACEMENTS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

fn standard_index(which: u32) -> Option<usize> {
    match which {
        STD_INPUT_HANDLE => Some(0),
        STD_OUTPUT_HANDLE => Some(1),
        STD_ERROR_HANDLE => Some(2),
        _ => None,
    }
}

extern "efiapi" fn get_std_handle(which: u32) -> usize {
    match standard_index(which) {
        Some(index) if REPLACED[index].load(Ordering::Acquire) => {
            REPLACEMENTS[index].load(Ordering::Relaxed)
        }
        Some(index) => standard_handle(index),
        None => fail(error::INVALID_HANDLE, INVALID_HANDLE_VALUE),
    }
}

extern "efiapi" fn set_std_handle(which: u32, handle: usize) -> Bool {
    match standard_index(which) {
        Some(index) => {
            REPLACEMENTS[index].store(handle, Ordering::Relaxed);
            REPLACED[index].store(true, Ordering::Release);
            TRUE
        }
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn console_fd(handle: usize) -> Option<usize> {
    fd_of(handle).filter(|fd| linux::is_terminal(*fd))
}

unsafe extern "efiapi" fn get_console_mode(handle: usize, mode: *mut u32) -> Bool {
    let Some(fd) = console_fd(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let value = if fd == 0 { INPUT_MODE } else { OUTPUT_MODE };
    // SAFETY: the caller passes a writable `DWORD`.
    unsafe { mode.write(value) };
    TRUE
}

extern "efiapi" fn set_console_mode(handle: usize, _mode: u32) -> Bool {
    match console_fd(handle) {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

extern "efiapi" fn get_code_page() -> u32 {
    CODE_PAGE_UTF8
}

extern "efiapi" fn set_code_page(page: u32) -> Bool {
    match page {
        CODE_PAGE_UTF8 => TRUE,
        _ => fail(error::INVALID_PARAMETER, FALSE),
    }
}

unsafe extern "efiapi" fn write_console_w(
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

unsafe extern "efiapi" fn write_console_a(
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

unsafe extern "efiapi" fn read_console_w(
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

unsafe extern "efiapi" fn read_console_a(
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
    b"SetStdHandle" => set_std_handle,
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

//! Modules: the program's own (`GetModuleHandle(NULL)`, its image base) and the libraries the
//! layer provides, which `LoadLibrary` and `GetModuleHandle` find by name and `GetProcAddress`
//! searches. Other DLLs cannot be loaded yet.

use super::process::process;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::{self, Export, Library};
use crate::text;

static KERNEL32_MODULE: u64 = 1;
static NTDLL_MODULE: u64 = 2;
static SHELL32_MODULE: u64 = 3;
static USER32_MODULE: u64 = 4;
static GDI32_MODULE: u64 = 5;
static COMDLG32_MODULE: u64 = 6;

fn module_handle(library: Library) -> usize {
    match library {
        Library::Kernel32 => &raw const KERNEL32_MODULE as usize,
        Library::Ntdll => &raw const NTDLL_MODULE as usize,
        Library::Shell32 => &raw const SHELL32_MODULE as usize,
        Library::User32 => &raw const USER32_MODULE as usize,
        Library::Gdi32 => &raw const GDI32_MODULE as usize,
        Library::Comdlg32 => &raw const COMDLG32_MODULE as usize,
    }
}

fn library_of(module: usize) -> Option<Library> {
    [
        Library::Kernel32,
        Library::Ntdll,
        Library::Shell32,
        Library::User32,
        Library::Gdi32,
        Library::Comdlg32,
    ]
    .into_iter()
    .find(|library| module_handle(*library) == module)
}

fn find_library(name: &[u8]) -> usize {
    match library::library(name) {
        Some(library) => module_handle(library),
        None => fail(error::MOD_NOT_FOUND, 0),
    }
}

unsafe fn find_library_wide(name: *const u16) -> usize {
    let mut bytes = [0u8; 256];
    // SAFETY: the caller's duty: a NUL-terminated UTF-16 name.
    let name = unsafe { super::file::wide_string(name) };
    let (length, consumed) = text::write_utf8(name, &mut bytes);
    if consumed < name.len() {
        return fail(error::MOD_NOT_FOUND, 0);
    }
    find_library(&bytes[..length])
}

unsafe fn find_library_ansi(name: *const u8) -> usize {
    // SAFETY: the caller's duty: a NUL-terminated name.
    find_library(unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes())
}

unsafe extern "efiapi" fn get_module_handle_w(name: *const u16) -> usize {
    match name.is_null() {
        true => process().image_base as usize,
        // SAFETY: `GetModuleHandleW`'s contract: a NUL-terminated UTF-16 name.
        false => unsafe { find_library_wide(name) },
    }
}

unsafe extern "efiapi" fn get_module_handle_a(name: *const u8) -> usize {
    match name.is_null() {
        true => process().image_base as usize,
        // SAFETY: `GetModuleHandleA`'s contract: a NUL-terminated name.
        false => unsafe { find_library_ansi(name) },
    }
}

unsafe extern "efiapi" fn load_library_w(name: *const u16) -> usize {
    if name.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `LoadLibraryW`'s contract: a NUL-terminated UTF-16 name.
    unsafe { find_library_wide(name) }
}

unsafe extern "efiapi" fn load_library_a(name: *const u8) -> usize {
    if name.is_null() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `LoadLibraryA`'s contract: a NUL-terminated name.
    unsafe { find_library_ansi(name) }
}

unsafe extern "efiapi" fn load_library_ex_w(name: *const u16, _file: usize, _flags: u32) -> usize {
    // SAFETY: the same contract as `LoadLibraryW`'s.
    unsafe { load_library_w(name) }
}

unsafe extern "efiapi" fn load_library_ex_a(name: *const u8, _file: usize, _flags: u32) -> usize {
    // SAFETY: the same contract as `LoadLibraryA`'s.
    unsafe { load_library_a(name) }
}

extern "efiapi" fn free_library(module: usize) -> Bool {
    match library_of(module) {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

unsafe extern "efiapi" fn get_proc_address(module: usize, name: *const u8) -> usize {
    let Some(library) = library_of(module) else {
        return fail(error::PROC_NOT_FOUND, 0);
    };
    if (name as usize) < 0x1_0000 {
        return fail(error::PROC_NOT_FOUND, 0);
    }
    // SAFETY: `GetProcAddress`'s contract: a NUL-terminated name (ordinals are below 64 Ki).
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    let name_of = |library| match library {
        Library::Kernel32 => &b"kernel32"[..],
        Library::Ntdll => b"ntdll",
        Library::Shell32 => b"shell32",
        Library::User32 => b"user32",
        Library::Gdi32 => b"gdi32",
        Library::Comdlg32 => b"comdlg32",
    };
    match super::resolve(name_of(library), name) {
        Some(address) => address as usize,
        None => fail(error::PROC_NOT_FOUND, 0),
    }
}

unsafe extern "efiapi" fn get_module_file_name_w(
    module: usize,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    if module != 0 && module != process().image_base as usize {
        return fail(error::MOD_NOT_FOUND, 0);
    }
    if size == 0 {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: `GetModuleFileNameW`'s contract: `buffer` holds `size` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    let program = process().program_windows;
    let written = program.len().min(out.len() - 1);
    out[..written].copy_from_slice(&program[..written]);
    out[written] = 0;
    match written < program.len() {
        true => fail(error::INSUFFICIENT_BUFFER, size),
        false => {
            super::thread_environment::set_last_error(error::SUCCESS);
            written as u32
        }
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetModuleHandleW" => get_module_handle_w,
    b"GetModuleHandleA" => get_module_handle_a,
    b"LoadLibraryW" => load_library_w,
    b"LoadLibraryA" => load_library_a,
    b"LoadLibraryExW" => load_library_ex_w,
    b"LoadLibraryExA" => load_library_ex_a,
    b"FreeLibrary" => free_library,
    b"GetProcAddress" => get_proc_address,
    b"GetModuleFileNameW" => get_module_file_name_w,
];

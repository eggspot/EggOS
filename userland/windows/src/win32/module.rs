//! Modules: the program's own (`GetModuleHandle(NULL)`, its image base) and the libraries the
//! layer provides, which `LoadLibrary` and `GetModuleHandle` find by name and `GetProcAddress`
//! searches. Other DLLs cannot be loaded yet.

use super::process::process;
use super::{Bool, FALSE, TRUE, error, fail};
use crate::library::{self, Export, Library};
use crate::text;

static MODULES: [u64; Library::ALL.len()] = [0; Library::ALL.len()];

fn module_handle(library: Library) -> usize {
    let index = Library::ALL
        .iter()
        .position(|known| *known == library)
        .unwrap_or(0);
    &raw const MODULES[index] as usize
}

fn library_of(module: usize) -> Option<Library> {
    Library::ALL
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

const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x1;
const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
const SIZE_OF_IMAGE: usize = 24 + 56;

fn module_at(address: usize) -> usize {
    let base = process().image_base as usize;
    let headers = base as *const u8;
    // SAFETY: the program's headers stay mapped and readable at its base: the PE header's
    // offset is at 0x3c.
    let header = unsafe { headers.wrapping_add(0x3c).cast::<u32>().read_unaligned() } as usize;
    // SAFETY: as above: `SizeOfImage` is in the PE header's optional header.
    let size = unsafe {
        headers
            .wrapping_add(header + SIZE_OF_IMAGE)
            .cast::<u32>()
            .read_unaligned()
    } as usize;
    if (base..base + size).contains(&address) {
        return base;
    }
    Library::ALL
        .into_iter()
        .find(|library| {
            super::exports(*library)
                .iter()
                .flat_map(|list| list.iter())
                .any(|export| export.address as usize == address)
        })
        .map_or(0, module_handle)
}

unsafe extern "efiapi" fn get_module_handle_ex_w(
    flags: u32,
    name: *const u16,
    module: *mut usize,
) -> Bool {
    let known = GET_MODULE_HANDLE_EX_FLAG_PIN
        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT
        | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS;
    let both = GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if module.is_null() || flags & !known != 0 || flags & both == both {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let found = match (
        flags & GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS != 0,
        name.is_null(),
    ) {
        (true, _) => match module_at(name as usize) {
            0 => fail(error::MOD_NOT_FOUND, 0),
            found => found,
        },
        (false, true) => process().image_base as usize,
        // SAFETY: `GetModuleHandleExW`'s contract: a NUL-terminated UTF-16 name.
        (false, false) => unsafe { find_library_wide(name) },
    };
    // SAFETY: the caller passes a writable `HMODULE`.
    unsafe { module.write(found) };
    Bool::from(found != 0)
}

extern "efiapi" fn set_dll_directory_w(_path: *const u16) -> Bool {
    TRUE
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
        return match super::resolve_ordinal(library.name(), name as usize as u16) {
            Some(address) => address as usize,
            None => fail(error::PROC_NOT_FOUND, 0),
        };
    }
    // SAFETY: `GetProcAddress`'s contract: a NUL-terminated name (ordinals are below 64 Ki).
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    match super::resolve(library.name(), name) {
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

unsafe extern "efiapi" fn get_module_file_name_a(module: usize, buffer: *mut u8, size: u32) -> u32 {
    if module != 0 && module != process().image_base as usize {
        return fail(error::MOD_NOT_FOUND, 0);
    }
    if size == 0 {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: `GetModuleFileNameA`'s contract: `buffer` holds `size` writable bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    let program = process().program_windows;
    let room = out.len() - 1;
    let (written, consumed) = crate::text::write_utf8(program, &mut out[..room]);
    out[written] = 0;
    match consumed < program.len() {
        true => fail(error::INSUFFICIENT_BUFFER, size),
        false => {
            super::thread_environment::set_last_error(error::SUCCESS);
            written as u32
        }
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetModuleFileNameA" => get_module_file_name_a,
    b"GetModuleHandleW" => get_module_handle_w,
    b"GetModuleHandleA" => get_module_handle_a,
    b"GetModuleHandleExW" => get_module_handle_ex_w,
    b"SetDllDirectoryW" => set_dll_directory_w,
    b"LoadLibraryW" => load_library_w,
    b"LoadLibraryA" => load_library_a,
    b"LoadLibraryExW" => load_library_ex_w,
    b"LoadLibraryExA" => load_library_ex_a,
    b"FreeLibrary" => free_library,
    b"GetProcAddress" => get_proc_address,
    b"GetModuleFileNameW" => get_module_file_name_w,
];

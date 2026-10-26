//! Handles and files: a file handle names a Linux descriptor (`(fd + 1) * 4`, so handles are
//! multiples of four and never null, as Windows' are), and `CreateFile`, `ReadFile`,
//! `WriteFile`, `CloseHandle`, `GetFileSize(Ex)`, `SetFilePointer(Ex)` and `GetFileType` are
//! the matching Linux calls. Paths are UTF-16 or ANSI (UTF-8) and may use `\`, a drive letter
//! (`C:\` is `/`) or the `\\?\` prefix.

use super::{Bool, FALSE, INVALID_HANDLE_VALUE, TRUE, error, error_for, fail};
use crate::library::Export;
use crate::linux::{self, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
use crate::text;
use core::ffi::c_void;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const TRUNCATE_EXISTING: u32 = 5;
const FILE_TYPE_UNKNOWN: u32 = 0;
const FILE_TYPE_DISK: u32 = 1;
const FILE_TYPE_CHAR: u32 = 2;
const FILE_TYPE_PIPE: u32 = 3;
const INVALID_SET_FILE_POINTER: u32 = u32::MAX;
const MAX_FD: usize = 1 << 20;

/// The descriptor a file handle names.
pub fn fd_of(handle: usize) -> Option<usize> {
    (handle != 0 && handle.is_multiple_of(4))
        .then(|| handle / 4 - 1)
        .filter(|fd| *fd < MAX_FD)
}

/// The handle that names descriptor `fd`.
pub fn handle_of(fd: usize) -> usize {
    (fd + 1) * 4
}

/// Reads up to `length` bytes into `buffer` from `handle`'s descriptor; the count read, or
/// `None` with the last error set.
///
/// # Safety
/// `buffer` must hold `length` writable bytes.
pub unsafe fn read_into(handle: usize, buffer: *mut u8, length: usize) -> Option<usize> {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, None);
    };
    // SAFETY: the caller's duty.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, length) };
    match linux::read(fd, buffer) {
        Ok(read) => Some(read),
        Err(errno) => fail(error_for(errno), None),
    }
}

/// Writes all of `bytes` to `handle`'s descriptor; `false` with the last error set if it fails.
pub fn write_bytes(handle: usize, bytes: &[u8]) -> bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, false);
    };
    match linux::write_all(fd, bytes) {
        Ok(()) => true,
        Err(errno) => fail(error_for(errno), false),
    }
}

unsafe extern "win64" fn read_file(
    handle: usize,
    buffer: *mut u8,
    length: u32,
    read: *mut u32,
    overlapped: *mut c_void,
) -> Bool {
    if !overlapped.is_null() {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    // SAFETY: `ReadFile`'s contract: `buffer` holds `length` writable bytes.
    let Some(count) = (unsafe { read_into(handle, buffer, length as usize) }) else {
        return FALSE;
    };
    if !read.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the count.
        unsafe { read.write(count as u32) };
    }
    TRUE
}

unsafe extern "win64" fn write_file(
    handle: usize,
    buffer: *const u8,
    length: u32,
    written: *mut u32,
    overlapped: *mut c_void,
) -> Bool {
    if !overlapped.is_null() {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    // SAFETY: `WriteFile`'s contract: `buffer` holds `length` readable bytes.
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

extern "win64" fn close_handle(handle: usize) -> Bool {
    if handle >= usize::MAX - 1 {
        return TRUE;
    }
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    match linux::close(fd) {
        Ok(_) => TRUE,
        Err(_) => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn open_flags(access: u32, disposition: u32) -> Option<usize> {
    let read = access & (GENERIC_READ | GENERIC_ALL) != 0 || access & 0x1 != 0;
    let write = access & (GENERIC_WRITE | GENERIC_ALL) != 0 || access & 0x6 != 0;
    let mode = match (read, write) {
        (_, false) => O_RDONLY,
        (false, true) => O_WRONLY,
        (true, true) => O_RDWR,
    };
    let creation = match disposition {
        CREATE_NEW => O_CREAT | O_EXCL,
        CREATE_ALWAYS => O_CREAT | O_TRUNC,
        OPEN_EXISTING => 0,
        OPEN_ALWAYS => O_CREAT,
        TRUNCATE_EXISTING => O_TRUNC,
        _ => return None,
    };
    Some(mode | creation)
}

fn create_file(path: &[u8], access: u32, disposition: u32) -> usize {
    let Some(flags) = open_flags(access, disposition) else {
        return fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    };
    let fd = match linux::open(path, flags) {
        Ok(fd) => fd,
        Err(errno) => return fail(error_for(errno), INVALID_HANDLE_VALUE),
    };
    let directory = linux::fstat(fd)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY);
    if directory {
        let _ = linux::close(fd);
        return fail(error::ACCESS_DENIED, INVALID_HANDLE_VALUE);
    }
    handle_of(fd)
}

unsafe extern "win64" fn create_file_w(
    name: *const u16,
    access: u32,
    _share: u32,
    _security: *mut c_void,
    disposition: u32,
    _flags: u32,
    _template: usize,
) -> usize {
    // SAFETY: `CreateFileW`'s contract: a NUL-terminated UTF-16 name.
    let name = unsafe { wide_string(name) };
    let mut path = [0u8; text::PATH_CAPACITY];
    match text::eggos_path(text::utf16_chars(name), &mut path) {
        Some(length) => create_file(&path[..length], access, disposition),
        None => fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE),
    }
}

unsafe extern "win64" fn create_file_a(
    name: *const u8,
    access: u32,
    _share: u32,
    _security: *mut c_void,
    disposition: u32,
    _flags: u32,
    _template: usize,
) -> usize {
    // SAFETY: `CreateFileA`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    let mut path = [0u8; text::PATH_CAPACITY];
    match text::eggos_path(text::utf8_chars(name), &mut path) {
        Some(length) => create_file(&path[..length], access, disposition),
        None => fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE),
    }
}

/// The NUL-terminated UTF-16 string at `text`, without its NUL.
///
/// # Safety
/// `text` must point to a NUL-terminated UTF-16 string that outlives the result's use.
pub unsafe fn wide_string<'a>(text: *const u16) -> &'a [u16] {
    let mut length = 0;
    // SAFETY: the caller's string is NUL-terminated, so every unit up to the NUL is readable.
    while unsafe { text.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: as above.
    unsafe { core::slice::from_raw_parts(text, length) }
}

fn file_size(handle: usize) -> Option<u64> {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, None);
    };
    match linux::fstat(fd) {
        Ok(words) => Some(words[linux::STAT_SIZE_WORD]),
        Err(errno) => fail(error_for(errno), None),
    }
}

unsafe extern "win64" fn get_file_size(handle: usize, high: *mut u32) -> u32 {
    let Some(size) = file_size(handle) else {
        return u32::MAX;
    };
    if !high.is_null() {
        // SAFETY: the caller passes a writable `DWORD` for the high half.
        unsafe { high.write((size >> 32) as u32) };
    }
    set_success();
    size as u32
}

unsafe extern "win64" fn get_file_size_ex(handle: usize, size: *mut i64) -> Bool {
    let Some(value) = file_size(handle) else {
        return FALSE;
    };
    // SAFETY: the caller passes a writable `LARGE_INTEGER`.
    unsafe { size.write(value as i64) };
    TRUE
}

fn seek(handle: usize, distance: i64, method: u32) -> Option<u64> {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, None);
    };
    if method > 2 {
        return fail(error::INVALID_PARAMETER, None);
    }
    match linux::lseek(fd, distance, method as usize) {
        Ok(position) => Some(position as u64),
        Err(errno) if errno.0 == 22 => fail(error::NEGATIVE_SEEK, None),
        Err(errno) => fail(error_for(errno), None),
    }
}

unsafe extern "win64" fn set_file_pointer(
    handle: usize,
    low: i32,
    high: *mut i32,
    method: u32,
) -> u32 {
    let distance = match high.is_null() {
        true => i64::from(low),
        // SAFETY: the caller passes a readable and writable `LONG` for the high half.
        false => i64::from(unsafe { high.read() }) << 32 | i64::from(low as u32),
    };
    let Some(position) = seek(handle, distance, method) else {
        return INVALID_SET_FILE_POINTER;
    };
    if !high.is_null() {
        // SAFETY: as above.
        unsafe { high.write((position >> 32) as i32) };
    }
    set_success();
    position as u32
}

unsafe extern "win64" fn set_file_pointer_ex(
    handle: usize,
    distance: i64,
    position: *mut i64,
    method: u32,
) -> Bool {
    let Some(new) = seek(handle, distance, method) else {
        return FALSE;
    };
    if !position.is_null() {
        // SAFETY: the caller passes a writable `LARGE_INTEGER`.
        unsafe { position.write(new as i64) };
    }
    TRUE
}

extern "win64" fn get_file_type(handle: usize) -> u32 {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FILE_TYPE_UNKNOWN);
    };
    let Ok(words) = linux::fstat(fd) else {
        return fail(error::INVALID_HANDLE, FILE_TYPE_UNKNOWN);
    };
    set_success();
    match linux::file_mode(&words) & linux::FILE_TYPE {
        linux::REGULAR_FILE | linux::DIRECTORY => FILE_TYPE_DISK,
        linux::CHARACTER_DEVICE => FILE_TYPE_CHAR,
        linux::FIFO => FILE_TYPE_PIPE,
        _ => FILE_TYPE_UNKNOWN,
    }
}

extern "win64" fn flush_file_buffers(handle: usize) -> Bool {
    match fd_of(handle) {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn set_success() {
    super::thread_environment::set_last_error(error::SUCCESS);
}

pub(super) const EXPORTS: &[Export] = exports![
    b"ReadFile" => read_file,
    b"WriteFile" => write_file,
    b"CloseHandle" => close_handle,
    b"CreateFileW" => create_file_w,
    b"CreateFileA" => create_file_a,
    b"GetFileSize" => get_file_size,
    b"GetFileSizeEx" => get_file_size_ex,
    b"SetFilePointer" => set_file_pointer,
    b"SetFilePointerEx" => set_file_pointer_ex,
    b"GetFileType" => get_file_type,
    b"FlushFileBuffers" => flush_file_buffers,
];

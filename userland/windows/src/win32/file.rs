//! Handles and files: a file handle names a Linux descriptor (`(fd + 1) * 4`, so handles are
//! multiples of four and never null, as Windows' are), and `CreateFile`, `ReadFile`,
//! `WriteFile`, `CloseHandle`, `GetFileSize(Ex)`, `SetFilePointer(Ex)` and `GetFileType` are
//! the matching Linux calls, as are `CopyFileW`, `SetEndOfFile`, `SetFileTime` (the
//! modification time: EggOS keeps no creation time) and `SetFileAttributesW` (the read-only
//! attribute is the write permission; the others have no place on EggOS's files, D178). Paths
//! are UTF-16 or ANSI (UTF-8) and may use `\`, a drive letter (`C:\` is `/`) or the `\\?\`
//! prefix. A file is inherited by children only when its `SECURITY_ATTRIBUTES` ask for it, and
//! `ReadFile` at a pipe's end fails with `ERROR_BROKEN_PIPE` and `WriteFile` to a pipe nobody
//! reads with `ERROR_NO_DATA`, as on Windows (D189).

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
const EPIPE: u16 = 32;

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
    let peeked = super::pipe::take_peeked(fd, buffer);
    if peeked != 0 {
        return Some(peeked);
    }
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
        Err(errno) if errno.0 == EPIPE => fail(error::NO_DATA, false),
        Err(errno) => fail(error_for(errno), false),
    }
}

unsafe extern "efiapi" fn read_file(
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
    let pipe_ended = count == 0 && length != 0 && fd_of(handle).is_some_and(super::pipe::is_pipe);
    if pipe_ended {
        return fail(error::BROKEN_PIPE, FALSE);
    }
    TRUE
}

unsafe extern "efiapi" fn write_file(
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

pub(super) extern "efiapi" fn close_handle(handle: usize) -> Bool {
    if handle >= super::CURRENT_THREAD {
        return TRUE;
    }
    if super::sync::is_object_handle(handle) {
        return super::sync::close(handle);
    }
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    if !super::pipe::closing(fd) {
        return fail(error::INVALID_HANDLE, FALSE);
    }
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

fn create_file(path: &[u8], access: u32, disposition: u32, inherit: bool) -> usize {
    let Some(flags) = open_flags(access, disposition) else {
        return fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    };
    let fd = match linux::open(path, flags | super::pipe::close_on_exec_unless(inherit)) {
        Ok(fd) => fd,
        Err(errno) => {
            return fail(
                super::directory::path_error(path, errno),
                INVALID_HANDLE_VALUE,
            );
        }
    };
    let directory = linux::fstat(fd)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY);
    if directory {
        let _ = linux::close(fd);
        return fail(error::ACCESS_DENIED, INVALID_HANDLE_VALUE);
    }
    handle_of(fd)
}

unsafe extern "efiapi" fn create_file_w(
    name: *const u16,
    access: u32,
    _share: u32,
    security: *const u8,
    disposition: u32,
    _flags: u32,
    _template: usize,
) -> usize {
    // SAFETY: `CreateFileW`'s contract: a NUL-terminated UTF-16 name.
    let name = unsafe { wide_string(name) };
    // SAFETY: `CreateFileW`'s contract: null or a readable `SECURITY_ATTRIBUTES`.
    let inherit = unsafe { super::pipe::inherits(security) };
    let mut path = [0u8; text::PATH_CAPACITY];
    match text::eggos_path(text::utf16_chars(name), &mut path) {
        Some(length) => create_file(&path[..length], access, disposition, inherit),
        None => fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE),
    }
}

unsafe extern "efiapi" fn create_file_a(
    name: *const u8,
    access: u32,
    _share: u32,
    security: *const u8,
    disposition: u32,
    _flags: u32,
    _template: usize,
) -> usize {
    // SAFETY: `CreateFileA`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    // SAFETY: `CreateFileA`'s contract: null or a readable `SECURITY_ATTRIBUTES`.
    let inherit = unsafe { super::pipe::inherits(security) };
    let mut path = [0u8; text::PATH_CAPACITY];
    match text::eggos_path(text::utf8_chars(name), &mut path) {
        Some(length) => create_file(&path[..length], access, disposition, inherit),
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

/// The caller's buffer of `size` UTF-16 units at `buffer`, empty when it is null.
///
/// # Safety
/// A non-null `buffer` must hold `size` writable units that outlive the result's use.
pub unsafe fn wide_buffer<'a>(buffer: *mut u16, size: usize) -> &'a mut [u16] {
    match buffer.is_null() {
        true => &mut [],
        // SAFETY: the caller's duty.
        false => unsafe { core::slice::from_raw_parts_mut(buffer, size) },
    }
}

/// Copies `units` and a NUL into `out`: the units copied (no NUL), or, when they do not fit,
/// the units needed with the NUL.
pub fn copy_terminated(units: &[u16], out: &mut [u16]) -> Result<usize, usize> {
    if units.len() >= out.len() {
        return Err(units.len() + 1);
    }
    out[..units.len()].copy_from_slice(units);
    out[units.len()] = 0;
    Ok(units.len())
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

unsafe extern "efiapi" fn get_file_size(handle: usize, high: *mut u32) -> u32 {
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

unsafe extern "efiapi" fn get_file_size_ex(handle: usize, size: *mut i64) -> Bool {
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

unsafe extern "efiapi" fn set_file_pointer(
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

unsafe extern "efiapi" fn set_file_pointer_ex(
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

extern "efiapi" fn get_file_type(handle: usize) -> u32 {
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

extern "efiapi" fn flush_file_buffers(handle: usize) -> Bool {
    match fd_of(handle) {
        Some(_) => TRUE,
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

fn set_success() {
    super::thread_environment::set_last_error(error::SUCCESS);
}

const COPY_CHUNK: usize = 16 << 10;
const SEEK_CURRENT: usize = 1;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const WRITE_BITS: u32 = 0o222;
const OWNER_WRITE: u32 = 0o200;
const PRESERVE_TIME: u64 = u64::MAX;

/// Copies the regular file at `from` to `to` (both NUL-terminated EggOS paths), replacing what
/// `to` names only when `replace`, and gives the copy the original's modification time.
pub(super) fn copy_file_at(from: &[u8], to: &[u8], replace: bool) -> Result<(), u32> {
    let source =
        linux::open(from, O_RDONLY).map_err(|errno| super::directory::path_error(from, errno))?;
    let copied = copy_from(source, to, replace);
    let _ = linux::close(source);
    copied
}

fn copy_from(source: usize, to: &[u8], replace: bool) -> Result<(), u32> {
    let words = linux::fstat(source).map_err(error_for)?;
    if linux::file_mode(&words) & linux::FILE_TYPE != linux::REGULAR_FILE {
        return Err(error::ACCESS_DENIED);
    }
    let creation = if replace {
        O_CREAT | O_TRUNC
    } else {
        O_CREAT | O_EXCL
    };
    let target = linux::open(to, O_WRONLY | creation).map_err(|errno| match errno.0 {
        17 => error::FILE_EXISTS,
        _ => super::directory::path_error(to, errno),
    })?;
    let mut chunk = [0u8; COPY_CHUNK];
    let result = loop {
        match linux::read(source, &mut chunk) {
            Ok(0) => break Ok(()),
            Ok(read) => {
                if let Err(errno) = linux::write_all(target, &chunk[..read]) {
                    break Err(error_for(errno));
                }
            }
            Err(errno) => break Err(error_for(errno)),
        }
    };
    if result.is_ok() {
        let modified = [
            [0, linux::UTIME_OMIT],
            [
                words[linux::STAT_MODIFY_TIME_WORD] as i64,
                words[linux::STAT_MODIFY_TIME_WORD + 1] as i64,
            ],
        ];
        let _ = linux::set_file_times(target, &modified);
    }
    let _ = linux::close(target);
    result
}

unsafe extern "efiapi" fn copy_file_w(
    from: *const u16,
    to: *const u16,
    fail_if_exists: Bool,
) -> Bool {
    let (mut old, mut new) = ([0u8; text::PATH_CAPACITY], [0u8; text::PATH_CAPACITY]);
    // SAFETY: `CopyFileW`'s contract: two NUL-terminated names.
    let old_length = unsafe { super::directory::eggos_path(from, &mut old) };
    // SAFETY: as above.
    let new_length = unsafe { super::directory::eggos_path(to, &mut new) };
    let (Some(old_length), Some(new_length)) = (old_length, new_length) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    match copy_file_at(
        &old[..old_length],
        &new[..new_length],
        fail_if_exists == FALSE,
    ) {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn set_file_attributes_w(name: *const u16, attributes: u32) -> Bool {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: `SetFileAttributesW`'s contract: a NUL-terminated name.
    let Some(length) = (unsafe { super::directory::eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let path = &path[..length];
    let words = match linux::stat_at(linux::AT_FDCWD, path) {
        Ok(words) => words,
        Err(errno) => return fail(super::directory::path_error(path, errno), FALSE),
    };
    let mode = linux::file_mode(&words) & 0o7777;
    let mode = match attributes & FILE_ATTRIBUTE_READONLY != 0 {
        true => mode & !WRITE_BITS,
        false => mode | OWNER_WRITE,
    };
    match linux::change_mode(path, mode) {
        Ok(_) => TRUE,
        Err(errno) => fail(error_for(errno), FALSE),
    }
}

unsafe fn unix_time(file_time: *const u64) -> [i64; 2] {
    let value = match file_time.is_null() {
        true => 0,
        // SAFETY: the caller's duty: a readable `FILETIME`, 4-byte aligned.
        false => unsafe { file_time.read_unaligned() },
    };
    let epoch = crate::calendar::UNIX_EPOCH_SECONDS * crate::calendar::INTERVALS_PER_SECOND;
    if value == 0 || value == PRESERVE_TIME || value < epoch {
        return [0, linux::UTIME_OMIT];
    }
    let since = value - epoch;
    [
        (since / crate::calendar::INTERVALS_PER_SECOND) as i64,
        (since % crate::calendar::INTERVALS_PER_SECOND * 100) as i64,
    ]
}

unsafe extern "efiapi" fn set_file_time(
    handle: usize,
    _creation: *const u64,
    access: *const u64,
    write: *const u64,
) -> Bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    // SAFETY: `SetFileTime`'s contract: readable `FILETIME`s or nulls.
    let accessed = unsafe { unix_time(access) };
    // SAFETY: as above.
    let modified = unsafe { unix_time(write) };
    let times = [accessed, modified];
    match linux::set_file_times(fd, &times) {
        Ok(_) => TRUE,
        Err(errno) => fail(error_for(errno), FALSE),
    }
}

extern "efiapi" fn set_end_of_file(handle: usize) -> Bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let truncated =
        linux::lseek(fd, 0, SEEK_CURRENT).and_then(|position| linux::truncate(fd, position as u64));
    match truncated {
        Ok(_) => TRUE,
        Err(errno) => fail(error_for(errno), FALSE),
    }
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
    b"CopyFileW" => copy_file_w,
    b"SetFileAttributesW" => set_file_attributes_w,
    b"SetFileTime" => set_file_time,
    b"SetEndOfFile" => set_end_of_file,
];

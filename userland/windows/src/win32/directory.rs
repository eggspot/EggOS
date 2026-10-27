//! Directories and file attributes: `FindFirstFileW` lists a directory through `getdents64`
//! and matches each name against the search's wildcard pattern, attributes and times come from
//! `stat`, and the working directory and full paths are shown in Windows' form (`C:\` is `/`).
//! A missing file whose directory is missing too is `ERROR_PATH_NOT_FOUND`, as on Windows.

use super::{Bool, FALSE, INVALID_HANDLE_VALUE, SpinLock, TRUE, error, error_for, fail, heap};
use crate::calendar;
use crate::find::{self, Entry};
use crate::library::Export;
use crate::linux::{self, AT_FDCWD, O_DIRECTORY, O_RDONLY, STAT_WORDS};
use crate::text;
use eggos_runtime::errno::Errno;

const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const ENTRIES_SIZE: usize = 4096;
const PATTERN_CAPACITY: usize = 1024;
const MAX_SEARCHES: usize = 64;
const DIRENT_NAME: usize = 19;
const DIRENT_LENGTH: usize = 16;
const ENOENT: u16 = 2;
const ENOTDIR: u16 = 20;
const ENOSYS: u16 = 38;
const EROFS: u16 = 30;

struct Search {
    fd: usize,
    filled: usize,
    at: usize,
    pattern_length: usize,
    pattern: [u8; PATTERN_CAPACITY],
    entries: [u8; ENTRIES_SIZE],
}

static SEARCHES: SpinLock<[usize; MAX_SEARCHES]> = SpinLock::new([0; MAX_SEARCHES]);

/// The NUL-terminated EggOS path of the NUL-terminated UTF-16 Windows path `name`.
///
/// # Safety
/// `name` must be a NUL-terminated UTF-16 string.
pub unsafe fn eggos_path(name: *const u16, out: &mut [u8; text::PATH_CAPACITY]) -> Option<usize> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the caller's duty.
    let name = unsafe { super::file::wide_string(name) };
    text::eggos_path(text::utf16_chars(name), out)
}

/// The Win32 error for a failed path operation on `path` (NUL-terminated): `ENOENT` is
/// `ERROR_PATH_NOT_FOUND` when the directory holding it is missing too.
pub fn path_error(path: &[u8], errno: Errno) -> u32 {
    if errno.0 != ENOENT {
        return error_for(errno);
    }
    let path = &path[..path.len().saturating_sub(1)];
    let (directory, _) = find::split(path);
    let mut parent = [0u8; text::PATH_CAPACITY];
    parent[..directory.len()].copy_from_slice(directory);
    match linux::stat_at(AT_FDCWD, &parent[..directory.len() + 1]) {
        Ok(words) if linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY => {
            error::FILE_NOT_FOUND
        }
        _ => error::PATH_NOT_FOUND,
    }
}

fn time_word(words: &[u64; STAT_WORDS], at: usize) -> u64 {
    calendar::file_time_from_unix(words[at] as i64, words[at + 1] as u32)
}

/// What `stat`'s words say about a file, in Windows' terms.
pub fn entry(words: &[u64; STAT_WORDS]) -> Entry {
    Entry {
        attributes: find::attributes(linux::file_mode(words)),
        creation_time: time_word(words, linux::STAT_CHANGE_TIME_WORD),
        last_access_time: time_word(words, linux::STAT_ACCESS_TIME_WORD),
        last_write_time: time_word(words, linux::STAT_MODIFY_TIME_WORD),
        size: words[linux::STAT_SIZE_WORD],
        volume: words[linux::STAT_DEVICE_WORD] as u32,
        links: linux::file_links(words),
        index: words[linux::STAT_INODE_WORD],
    }
}

fn next_match(search: &mut Search, data: &mut [u8; find::FIND_DATA_SIZE]) -> bool {
    loop {
        if search.at >= search.filled {
            match linux::directory_entries(search.fd, &mut search.entries) {
                Ok(0) | Err(_) => return false,
                Ok(filled) => {
                    search.filled = filled.min(ENTRIES_SIZE);
                    search.at = 0;
                }
            }
        }
        let record = &search.entries[search.at..search.filled];
        let length = match record.get(DIRENT_LENGTH..DIRENT_LENGTH + 2) {
            Some(bytes) => usize::from(u16::from_ne_bytes([bytes[0], bytes[1]])),
            None => return false,
        };
        if length <= DIRENT_NAME || length > record.len() {
            return false;
        }
        search.at += length;
        let name_field = &record[DIRENT_NAME..length];
        let Some(end) = name_field.iter().position(|byte| *byte == 0) else {
            continue;
        };
        let name = &name_field[..end];
        if !find::matches(&search.pattern[..search.pattern_length], name) {
            continue;
        }
        let Ok(words) = linux::stat_at(search.fd, &name_field[..=end]) else {
            continue;
        };
        if find::fill_find_data(data, &entry(&words), name) {
            return true;
        }
    }
}

unsafe extern "efiapi" fn find_first_file_w(name: *const u16, data: *mut u8) -> usize {
    if data.is_null() {
        return fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    }
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: `FindFirstFileW`'s contract: a NUL-terminated name.
    let Some(length) = (unsafe { eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, INVALID_HANDLE_VALUE);
    };
    let (directory, pattern) = find::split(&path[..length - 1]);
    if pattern.is_empty() || pattern.len() > PATTERN_CAPACITY {
        return fail(error::FILE_NOT_FOUND, INVALID_HANDLE_VALUE);
    }
    let mut directory_path = [0u8; text::PATH_CAPACITY];
    directory_path[..directory.len()].copy_from_slice(directory);
    let directory_path = &directory_path[..directory.len() + 1];
    let fd = match linux::open(directory_path, O_RDONLY | O_DIRECTORY) {
        Ok(fd) => fd,
        Err(errno) if matches!(errno.0, ENOENT | ENOTDIR) => {
            return fail(error::PATH_NOT_FOUND, INVALID_HANDLE_VALUE);
        }
        Err(errno) => return fail(error_for(errno), INVALID_HANDLE_VALUE),
    };
    let search = heap::allocate(core::mem::size_of::<Search>()).cast::<Search>();
    if search.is_null() {
        let _ = linux::close(fd);
        return fail(error::NOT_ENOUGH_MEMORY, INVALID_HANDLE_VALUE);
    }
    // SAFETY: a fresh, zeroed, 16-byte aligned heap block for one `Search`, whose fields are
    // all integers and byte arrays, for which zeros are valid.
    let state = unsafe { &mut *search };
    state.fd = fd;
    state.pattern_length = pattern.len();
    state.pattern[..pattern.len()].copy_from_slice(pattern);
    // SAFETY: `FindFirstFileW`'s contract: `data` is a writable `WIN32_FIND_DATAW`.
    let data = unsafe { &mut *data.cast::<[u8; find::FIND_DATA_SIZE]>() };
    if !next_match(state, data) {
        close_search(search as usize);
        return fail(error::FILE_NOT_FOUND, INVALID_HANDLE_VALUE);
    }
    let registered = SEARCHES.with(|searches| {
        let slot = searches.iter_mut().find(|slot| **slot == 0)?;
        *slot = search as usize;
        Some(())
    });
    if registered.is_none() {
        close_search(search as usize);
        return fail(error::TOO_MANY_OPEN_FILES, INVALID_HANDLE_VALUE);
    }
    super::thread_environment::set_last_error(error::SUCCESS);
    search as usize
}

unsafe extern "efiapi" fn find_first_file_ex_w(
    name: *const u16,
    _level: u32,
    data: *mut u8,
    _search_op: u32,
    _filter: usize,
    _flags: u32,
) -> usize {
    // SAFETY: the same contract as `FindFirstFileW`'s (both information levels fill the
    // fields this layer fills).
    unsafe { find_first_file_w(name, data) }
}

fn close_search(search: usize) {
    // SAFETY: `search` is a `Search` block this module allocated and nothing else uses.
    let fd = unsafe { (*(search as *const Search)).fd };
    let _ = linux::close(fd);
    // SAFETY: as above.
    unsafe { heap::free(search as *mut u8) };
}

fn registered(handle: usize) -> bool {
    handle != 0 && SEARCHES.with(|searches| searches.contains(&handle))
}

unsafe extern "efiapi" fn find_next_file_w(handle: usize, data: *mut u8) -> Bool {
    if !registered(handle) || data.is_null() {
        return fail(error::INVALID_HANDLE, FALSE);
    }
    // SAFETY: a registered handle is a live `Search`; a search handle is used by one thread at
    // a time, as Windows requires.
    let state = unsafe { &mut *(handle as *mut Search) };
    // SAFETY: `FindNextFileW`'s contract: `data` is a writable `WIN32_FIND_DATAW`.
    let data = unsafe { &mut *data.cast::<[u8; find::FIND_DATA_SIZE]>() };
    match next_match(state, data) {
        true => TRUE,
        false => fail(error::NO_MORE_FILES, FALSE),
    }
}

extern "efiapi" fn find_close(handle: usize) -> Bool {
    let removed = SEARCHES.with(|searches| {
        let slot = searches
            .iter_mut()
            .find(|slot| **slot == handle && handle != 0)?;
        *slot = 0;
        Some(())
    });
    match removed {
        Some(()) => {
            close_search(handle);
            TRUE
        }
        None => fail(error::INVALID_HANDLE, FALSE),
    }
}

unsafe fn stat_path(name: *const u16) -> Result<[u64; STAT_WORDS], u32> {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: the callers' contract: a NUL-terminated name.
    let length = unsafe { eggos_path(name, &mut path) }.ok_or(error::INVALID_PARAMETER)?;
    linux::stat_at(AT_FDCWD, &path[..length]).map_err(|errno| path_error(&path[..length], errno))
}

unsafe extern "efiapi" fn get_file_attributes_w(name: *const u16) -> u32 {
    // SAFETY: `GetFileAttributesW`'s contract: a NUL-terminated name.
    match unsafe { stat_path(name) } {
        Ok(words) => find::attributes(linux::file_mode(&words)),
        Err(code) => fail(code, INVALID_FILE_ATTRIBUTES),
    }
}

unsafe extern "efiapi" fn get_file_attributes_ex_w(
    name: *const u16,
    level: u32,
    out: *mut u8,
) -> Bool {
    if level != 0 || out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `GetFileAttributesExW`'s contract: a NUL-terminated name.
    match unsafe { stat_path(name) } {
        Ok(words) => {
            // SAFETY: `GetFileAttributesExW`'s contract: a writable
            // `WIN32_FILE_ATTRIBUTE_DATA` for the standard level.
            let out = unsafe { &mut *out.cast::<[u8; find::ATTRIBUTE_DATA_SIZE]>() };
            find::fill_attribute_data(out, &entry(&words));
            TRUE
        }
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_file_information_by_handle(handle: usize, out: *mut u8) -> Bool {
    let Some(fd) = super::file::fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    if out.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    match linux::fstat(fd) {
        Ok(words) => {
            // SAFETY: the caller passes a writable `BY_HANDLE_FILE_INFORMATION`.
            let out = unsafe { &mut *out.cast::<[u8; find::FILE_INFORMATION_SIZE]>() };
            find::fill_file_information(out, &entry(&words));
            TRUE
        }
        Err(errno) => fail(error_for(errno), FALSE),
    }
}

fn read_only_error(errno: Errno) -> u32 {
    match errno.0 {
        ENOSYS => error::ACCESS_DENIED,
        EROFS => error::WRITE_PROTECT,
        _ => error_for(errno),
    }
}

unsafe extern "efiapi" fn create_directory_w(name: *const u16, _security: usize) -> Bool {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: `CreateDirectoryW`'s contract: a NUL-terminated name.
    let Some(length) = (unsafe { eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let path = &path[..length];
    match linux::stat_at(AT_FDCWD, path) {
        Ok(_) => return fail(error::ALREADY_EXISTS, FALSE),
        Err(errno) if errno.0 == ENOENT => {
            if path_error(path, errno) == error::PATH_NOT_FOUND {
                return fail(error::PATH_NOT_FOUND, FALSE);
            }
        }
        Err(errno) => return fail(error_for(errno), FALSE),
    }
    match linux::make_directory(path) {
        Ok(_) => TRUE,
        Err(errno) => fail(read_only_error(errno), FALSE),
    }
}

unsafe fn remove(name: *const u16, directory: bool) -> Bool {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: the callers' contract: a NUL-terminated name.
    let Some(length) = (unsafe { eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    let path = &path[..length];
    let is_directory = match linux::stat_at(AT_FDCWD, path) {
        Ok(words) => linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY,
        Err(errno) => return fail(path_error(path, errno), FALSE),
    };
    match (directory, is_directory) {
        (true, false) => return fail(error::DIRECTORY, FALSE),
        (false, true) => return fail(error::ACCESS_DENIED, FALSE),
        _ => {}
    }
    let flags = if directory { linux::AT_REMOVEDIR } else { 0 };
    match linux::remove(path, flags) {
        Ok(_) => TRUE,
        Err(errno) => fail(read_only_error(errno), FALSE),
    }
}

unsafe extern "efiapi" fn delete_file_w(name: *const u16) -> Bool {
    // SAFETY: `DeleteFileW`'s contract: a NUL-terminated name.
    unsafe { remove(name, false) }
}

unsafe extern "efiapi" fn remove_directory_w(name: *const u16) -> Bool {
    // SAFETY: `RemoveDirectoryW`'s contract: a NUL-terminated name.
    unsafe { remove(name, true) }
}

fn write_windows_path(path: &[u8], out: &mut [u16]) -> Result<usize, usize> {
    let mut units = 0;
    text::windows_path(path, &mut |c| units += c.len_utf16());
    if units >= out.len() {
        return Err(units + 1);
    }
    let mut at = 0;
    text::windows_path(path, &mut |c| at += c.encode_utf16(&mut out[at..]).len());
    out[at] = 0;
    Ok(at)
}

fn working_directory(out: &mut [u8; text::PATH_CAPACITY]) -> Option<usize> {
    let length = linux::working_directory(out).ok()?;
    Some(length.saturating_sub(1))
}

unsafe extern "efiapi" fn get_current_directory_w(size: u32, buffer: *mut u16) -> u32 {
    let mut directory = [0u8; text::PATH_CAPACITY];
    let Some(length) = working_directory(&mut directory) else {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let out = match buffer.is_null() || size == 0 {
        true => &mut [][..],
        // SAFETY: `GetCurrentDirectoryW`'s contract: `buffer` holds `size` writable units.
        false => unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) },
    };
    match write_windows_path(&directory[..length], out) {
        Ok(written) => written as u32,
        Err(needed) => needed as u32,
    }
}

unsafe extern "efiapi" fn set_current_directory_w(name: *const u16) -> Bool {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: `SetCurrentDirectoryW`'s contract: a NUL-terminated name.
    let Some(length) = (unsafe { eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    match linux::change_directory(&path[..length]) {
        Ok(_) => TRUE,
        Err(errno) if errno.0 == ENOTDIR => fail(error::DIRECTORY, FALSE),
        Err(errno) => fail(path_error(&path[..length], errno), FALSE),
    }
}

unsafe extern "efiapi" fn get_full_path_name_w(
    name: *const u16,
    size: u32,
    buffer: *mut u16,
    file_part: *mut *mut u16,
) -> u32 {
    let mut path = [0u8; text::PATH_CAPACITY];
    // SAFETY: `GetFullPathNameW`'s contract: a NUL-terminated name.
    let Some(length) = (unsafe { eggos_path(name, &mut path) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let mut directory = [0u8; text::PATH_CAPACITY];
    let Some(directory_length) = working_directory(&mut directory) else {
        return fail(error::NOT_ENOUGH_MEMORY, 0);
    };
    let mut full = [0u8; text::PATH_CAPACITY];
    let Some(full_length) = text::absolute_path(
        &directory[..directory_length],
        &path[..length - 1],
        &mut full,
    ) else {
        return fail(error::FILENAME_EXCED_RANGE, 0);
    };
    let out = match buffer.is_null() || size == 0 {
        true => &mut [][..],
        // SAFETY: `GetFullPathNameW`'s contract: `buffer` holds `size` writable units.
        false => unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) },
    };
    match write_windows_path(&full[..full_length], out) {
        Ok(written) => {
            if !file_part.is_null() {
                let last = out[..written]
                    .iter()
                    .rposition(|unit| *unit == u16::from(b'\\'))
                    .map_or(0, |at| at + 1);
                let part = match last < written {
                    true => buffer.wrapping_add(last),
                    false => core::ptr::null_mut(),
                };
                // SAFETY: the caller passes a writable `LPWSTR*`.
                unsafe { file_part.write(part) };
            }
            written as u32
        }
        Err(needed) => needed as u32,
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"FindFirstFileW" => find_first_file_w,
    b"FindFirstFileExW" => find_first_file_ex_w,
    b"FindNextFileW" => find_next_file_w,
    b"FindClose" => find_close,
    b"GetFileAttributesW" => get_file_attributes_w,
    b"GetFileAttributesExW" => get_file_attributes_ex_w,
    b"GetFileInformationByHandle" => get_file_information_by_handle,
    b"CreateDirectoryW" => create_directory_w,
    b"DeleteFileW" => delete_file_w,
    b"RemoveDirectoryW" => remove_directory_w,
    b"GetCurrentDirectoryW" => get_current_directory_w,
    b"SetCurrentDirectoryW" => set_current_directory_w,
    b"GetFullPathNameW" => get_full_path_name_w,
];

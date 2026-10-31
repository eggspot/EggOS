//! version.dll (D178): a file's version information is the `RT_VERSION` resource its PE file
//! holds, read from the file (`crate::version_info`), and `VerQueryValueW` finds a value in
//! the block a program got, by its path.

use super::{Bool, FALSE, TRUE, error, error_for, fail, heap};
use crate::library::Export;
use crate::linux::{self, O_RDONLY};
use crate::text::PATH_CAPACITY;
use crate::version_info;

const MAX_FILE_SIZE: u64 = 256 << 20;

struct FileBytes {
    block: *mut u8,
    length: usize,
}

impl FileBytes {
    fn bytes(&self) -> &[u8] {
        // SAFETY: the block holds `length` bytes read from the file.
        unsafe { core::slice::from_raw_parts(self.block, self.length) }
    }
}

impl Drop for FileBytes {
    fn drop(&mut self) {
        // SAFETY: the block is this value's alone.
        unsafe { heap::free(self.block) };
    }
}

fn read_all(fd: usize) -> Result<FileBytes, u32> {
    let size = linux::fstat(fd).map_err(error_for)?[linux::STAT_SIZE_WORD];
    if size > MAX_FILE_SIZE {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    let block = heap::allocate(size as usize + 1);
    if block.is_null() {
        return Err(error::NOT_ENOUGH_MEMORY);
    }
    let mut file = FileBytes { block, length: 0 };
    // SAFETY: the block holds `size + 1` bytes.
    let buffer = unsafe { core::slice::from_raw_parts_mut(block, size as usize) };
    while file.length < buffer.len() {
        match linux::read(fd, &mut buffer[file.length..]) {
            Ok(0) => break,
            Ok(count) => file.length += count,
            Err(errno) => return Err(error_for(errno)),
        }
    }
    Ok(file)
}

unsafe fn with_version_block<R>(name: *const u16, work: impl FnOnce(&[u8]) -> R) -> Result<R, u32> {
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: the callers' contract: a NUL-terminated file name.
    let length =
        unsafe { super::directory::eggos_path(name, &mut path) }.ok_or(error::INVALID_PARAMETER)?;
    let path = &path[..length];
    let fd =
        linux::open(path, O_RDONLY).map_err(|errno| super::directory::path_error(path, errno))?;
    let file = read_all(fd);
    let _ = linux::close(fd);
    let file = file?;
    let block =
        version_info::version_resource(file.bytes()).ok_or(error::RESOURCE_TYPE_NOT_FOUND)?;
    Ok(work(block))
}

unsafe extern "efiapi" fn get_file_version_info_size_w(name: *const u16, handle: *mut u32) -> u32 {
    if !handle.is_null() {
        // SAFETY: the caller passes a writable `DWORD`, or null.
        unsafe { handle.write(0) };
    }
    // SAFETY: `GetFileVersionInfoSizeW`'s contract: a NUL-terminated file name.
    match unsafe { with_version_block(name, |block| block.len()) } {
        Ok(size) => size as u32,
        Err(code) => fail(code, 0),
    }
}

unsafe extern "efiapi" fn get_file_version_info_w(
    name: *const u16,
    _handle: u32,
    length: u32,
    data: *mut u8,
) -> Bool {
    if data.is_null() || length == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `GetFileVersionInfoW`'s contract: `length` writable bytes at `data`.
    let out = unsafe { core::slice::from_raw_parts_mut(data, length as usize) };
    // SAFETY: as above: a NUL-terminated file name.
    let copied = unsafe {
        with_version_block(name, |block| {
            let count = block.len().min(out.len());
            out[..count].copy_from_slice(&block[..count]);
        })
    };
    match copied {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn ver_query_value_w(
    block: *const u8,
    path: *const u16,
    value: *mut *const u8,
    length: *mut u32,
) -> Bool {
    if block.is_null() || path.is_null() || value.is_null() || length.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `VerQueryValueW`'s contract: a block `GetFileVersionInfoW` filled, which starts
    // with its own length.
    let size = usize::from(unsafe { block.cast::<u16>().read_unaligned() });
    // SAFETY: as above: the block holds `size` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(block, size) };
    // SAFETY: as above: a NUL-terminated path.
    let path = unsafe { super::file::wide_string(path) };
    match version_info::query(bytes, path) {
        Some((offset, count, _)) => {
            // SAFETY: the caller passes a writable pointer and a writable `UINT`.
            unsafe { value.write(block.wrapping_add(offset)) };
            // SAFETY: as above.
            unsafe { length.write(count as u32) };
            TRUE
        }
        None => fail(error::RESOURCE_TYPE_NOT_FOUND, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetFileVersionInfoSizeW" => get_file_version_info_size_w,
    b"GetFileVersionInfoW" => get_file_version_info_w,
    b"VerQueryValueW" => ver_query_value_w,
];

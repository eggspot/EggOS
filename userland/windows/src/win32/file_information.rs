//! A file handle's information by class (D189): `GetFileInformationByHandleEx` gives the
//! basic (times and attributes), standard (sizes, links, directory) and attribute-tag classes
//! from the descriptor's `fstat`, and `SetFileInformationByHandle` sets the end of the file
//! and its allocation (`ftruncate`), as Rust's standard library does to truncate and resize
//! files.

use super::{Bool, FALSE, TRUE, error, error_for, fail};
use crate::find;
use crate::library::Export;
use crate::linux;

const FILE_BASIC_INFO: u32 = 0;
const FILE_STANDARD_INFO: u32 = 1;
const FILE_ALLOCATION_INFO: u32 = 5;
const FILE_END_OF_FILE_INFO: u32 = 6;
const FILE_ATTRIBUTE_TAG_INFO: u32 = 9;

fn descriptor(handle: usize) -> Result<usize, u32> {
    super::file::fd_of(handle).ok_or(error::INVALID_HANDLE)
}

unsafe fn fill(fd: usize, class: u32, out: *mut u8, size: usize) -> Result<(), u32> {
    let needed = match class {
        FILE_BASIC_INFO => find::BASIC_INFORMATION_SIZE,
        FILE_STANDARD_INFO => find::STANDARD_INFORMATION_SIZE,
        FILE_ATTRIBUTE_TAG_INFO => find::ATTRIBUTE_TAG_INFORMATION_SIZE,
        _ => return Err(error::INVALID_PARAMETER),
    };
    if out.is_null() || size < needed {
        return Err(error::BAD_LENGTH);
    }
    let words = linux::fstat(fd).map_err(error_for)?;
    let entry = super::directory::entry(&words);
    // SAFETY: the caller's duty: `size` writable bytes, at least `needed`.
    let out = unsafe { core::slice::from_raw_parts_mut(out, needed) };
    match class {
        FILE_BASIC_INFO => {
            find::fill_basic_information(out.try_into().map_err(|_| error::BAD_LENGTH)?, &entry)
        }
        FILE_STANDARD_INFO => {
            find::fill_standard_information(out.try_into().map_err(|_| error::BAD_LENGTH)?, &entry)
        }
        _ => find::fill_attribute_tag_information(
            out.try_into().map_err(|_| error::BAD_LENGTH)?,
            &entry,
        ),
    }
    Ok(())
}

unsafe extern "efiapi" fn get_file_information_by_handle_ex(
    handle: usize,
    class: u32,
    out: *mut u8,
    size: u32,
) -> Bool {
    let filled = descriptor(handle).and_then(|fd| {
        // SAFETY: `GetFileInformationByHandleEx`'s contract: `size` writable bytes at `out`.
        unsafe { fill(fd, class, out, size as usize) }
    });
    match filled {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe fn set(fd: usize, class: u32, information: *const u8, size: usize) -> Result<(), u32> {
    if !matches!(class, FILE_ALLOCATION_INFO | FILE_END_OF_FILE_INFO) {
        return Err(error::INVALID_PARAMETER);
    }
    if information.is_null() || size < 8 {
        return Err(error::BAD_LENGTH);
    }
    // SAFETY: the caller's duty: the class' one `LARGE_INTEGER`.
    let length = unsafe { information.cast::<i64>().read_unaligned() };
    let length = u64::try_from(length).map_err(|_| error::INVALID_PARAMETER)?;
    let current = linux::fstat(fd).map_err(error_for)?[linux::STAT_SIZE_WORD];
    let shrinks_only = class == FILE_ALLOCATION_INFO;
    if shrinks_only && length >= current {
        return Ok(());
    }
    linux::truncate(fd, length).map(drop).map_err(error_for)
}

unsafe extern "efiapi" fn set_file_information_by_handle(
    handle: usize,
    class: u32,
    information: *const u8,
    size: u32,
) -> Bool {
    let done = descriptor(handle).and_then(|fd| {
        // SAFETY: `SetFileInformationByHandle`'s contract: `size` readable bytes.
        unsafe { set(fd, class, information, size as usize) }
    });
    match done {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetFileInformationByHandleEx" => get_file_information_by_handle_ex,
    b"SetFileInformationByHandle" => set_file_information_by_handle,
];

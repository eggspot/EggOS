//! Memory mappings: `mmap` with Darwin's flags, `munmap`, `mprotect`, and `madvise`, whose
//! advice is only a hint and so is accepted and ignored (Darwin's `MADV_DONTNEED` does not
//! discard the pages, unlike Linux's).

use super::{errno_from, set_errno};
use crate::darwin::{self, EINVAL};
use crate::linux;
use core::ffi::{c_int, c_void};

const MAP_FAILED: usize = usize::MAX;
const PROTECTION: c_int = 7;

/// # Safety
/// With `MAP_FIXED`, the range must hold nothing the program still uses.
pub unsafe extern "C" fn mmap(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> usize {
    let Some(linux_flags) = darwin::linux_map_flags(flags as u32) else {
        set_errno(EINVAL);
        return MAP_FAILED;
    };
    if protection & !PROTECTION != 0 {
        set_errno(EINVAL);
        return MAP_FAILED;
    }
    // SAFETY: the caller's promise covers a fixed mapping; any other replaces nothing.
    match unsafe {
        linux::syscall(
            linux::number::MMAP,
            [
                address as usize,
                length,
                protection as usize,
                linux_flags,
                fd as isize as usize,
                offset as usize,
            ],
        )
    } {
        Ok(mapped) => mapped,
        Err(errno) => errno_from(errno, MAP_FAILED),
    }
}

/// # Safety
/// The range must hold nothing the program still uses.
pub unsafe extern "C" fn munmap(address: *mut c_void, length: usize) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { linux::unmap(address as usize, length) }
        .map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

/// # Safety
/// The program must not use the range in a way the new protection forbids.
pub unsafe extern "C" fn mprotect(address: *mut c_void, length: usize, protection: c_int) -> c_int {
    if protection & !PROTECTION != 0 {
        set_errno(EINVAL);
        return -1;
    }
    // SAFETY: the caller's promise.
    unsafe { linux::protect(address as usize, length, protection as usize) }
        .map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

pub extern "C" fn madvise(address: *mut c_void, _: usize, _: c_int) -> c_int {
    if !(address as usize).is_multiple_of(linux::PAGE_SIZE) {
        set_errno(EINVAL);
        return -1;
    }
    0
}

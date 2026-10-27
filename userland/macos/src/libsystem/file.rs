//! Files and directories: `open` (Darwin's flags), `read`, `write`, `close`, `lseek`, `stat`,
//! `fstat` and `lstat` (Darwin's `struct stat`), `opendir` and `readdir` (Darwin's
//! `struct dirent`), the working directory, `unlink`, `mkdir`, `rmdir`, `access` and `isatty`.

use super::variadic::{RawVaList, VaList, variadic};
use super::{c_string_with_nul, errno_from, heap, set_errno};
use crate::darwin::{self, EBADF, EINVAL, ERANGE, LinuxLayout, Status};
use crate::format::Arguments;
use crate::linux::{self, AT_FDCWD};
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

const DIRECTORY_BUFFER: usize = 4096;
const MODE_MASK: usize = 0o7777;

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe fn open_with_mode(path: *const c_char, flags: c_int, mode: usize) -> c_int {
    let Some(linux_flags) = darwin::linux_open_flags(flags as u32, LinuxLayout::HOST) else {
        set_errno(EINVAL);
        return -1;
    };
    // SAFETY: the caller's promise.
    let path = unsafe { c_string_with_nul(path) };
    match linux::open(path, linux_flags as usize, mode & MODE_MASK) {
        Ok(fd) => fd as c_int,
        Err(errno) => errno_from(errno, -1),
    }
}

unsafe extern "C" fn open_with(path: *const c_char, flags: c_int, arguments: RawVaList) -> c_int {
    let mode = if flags as u32 & darwin::open::CREAT != 0 {
        // SAFETY: with `O_CREAT`, the program passes the mode as the variadic argument.
        unsafe { VaList::new(arguments) }.integer() as usize
    } else {
        0
    };
    // SAFETY: the program passes a NUL-terminated path.
    unsafe { open_with_mode(path, flags, mode) }
}

variadic!(eggos_macos_open => open_with, 2, "x2", "rdx");
pub use eggos_macos_open as open;

pub extern "C" fn close(fd: c_int) -> c_int {
    linux::close(fd as usize).map_or_else(|errno| errno_from(errno, -1), |_| 0)
}

/// # Safety
/// `buffer` must be writable for `count` bytes.
pub unsafe extern "C" fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize {
    // SAFETY: `read` writes at most `count` bytes into the caller's buffer.
    match unsafe {
        linux::syscall(
            linux::number::READ,
            [fd as usize, buffer as usize, count, 0, 0, 0],
        )
    } {
        Ok(read) => read as isize,
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `bytes` must be readable for `count` bytes.
pub unsafe extern "C" fn write(fd: c_int, bytes: *const c_void, count: usize) -> isize {
    if fd == 1 || fd == 2 {
        super::stdio::flush_all();
    }
    // SAFETY: `write` reads at most `count` bytes of the caller's buffer.
    match unsafe {
        linux::syscall(
            linux::number::WRITE,
            [fd as usize, bytes as usize, count, 0, 0, 0],
        )
    } {
        Ok(written) => written as isize,
        Err(errno) => errno_from(errno, -1),
    }
}

pub extern "C" fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
    match linux::lseek(fd as usize, offset, whence as usize) {
        Ok(position) => position as i64,
        Err(errno) => errno_from(errno, -1),
    }
}

fn write_status(raw: &[u8; darwin::LINUX_STAT_SIZE], out: *mut u8) {
    let darwin = Status::from_linux(raw, LinuxLayout::HOST).to_darwin();
    // SAFETY: the caller of the public function promised room for Darwin's `struct stat`.
    unsafe { out.copy_from_nonoverlapping(darwin.as_ptr(), darwin.len()) };
}

/// # Safety
/// `path` must be a NUL-terminated string and `out` hold Darwin's `struct stat`.
pub unsafe extern "C" fn stat(path: *const c_char, out: *mut c_void) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stat_with(path, out, 0) }
}

/// # Safety
/// As `stat`.
pub unsafe extern "C" fn lstat(path: *const c_char, out: *mut c_void) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { stat_with(path, out, linux::AT_SYMLINK_NOFOLLOW) }
}

unsafe fn stat_with(path: *const c_char, out: *mut c_void, flags: usize) -> c_int {
    let mut raw = [0u8; darwin::LINUX_STAT_SIZE];
    // SAFETY: the caller's promise.
    let path = unsafe { c_string_with_nul(path) };
    match linux::stat_at(path, flags, &mut raw) {
        Ok(_) => {
            write_status(&raw, out.cast());
            0
        }
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `out` must hold Darwin's `struct stat`.
pub unsafe extern "C" fn fstat(fd: c_int, out: *mut c_void) -> c_int {
    let mut raw = [0u8; darwin::LINUX_STAT_SIZE];
    match linux::fstat(fd as usize, &mut raw) {
        Ok(_) => {
            write_status(&raw, out.cast());
            0
        }
        Err(errno) => errno_from(errno, -1),
    }
}

pub extern "C" fn isatty(fd: c_int) -> c_int {
    if linux::is_terminal(fd as usize) {
        1
    } else {
        set_errno(25);
        0
    }
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn access(path: *const c_char, mode: c_int) -> c_int {
    // SAFETY: the caller's promise.
    let path = unsafe { c_string_with_nul(path) };
    // SAFETY: `faccessat` reads the NUL-terminated path.
    match unsafe {
        linux::syscall(
            linux::number::FACCESSAT,
            [AT_FDCWD, path.as_ptr() as usize, mode as usize, 0, 0, 0],
        )
    } {
        Ok(_) => 0,
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `buffer` must be null or writable for `size` bytes.
pub unsafe extern "C" fn getcwd(buffer: *mut c_char, size: usize) -> *mut c_char {
    let (target, room) = if buffer.is_null() {
        let room = if size == 0 { 1024 } else { size };
        (heap::allocate(room, false).cast::<c_char>(), room)
    } else {
        (buffer, size)
    };
    if target.is_null() {
        return ptr::null_mut();
    }
    if room == 0 {
        set_errno(EINVAL);
        return ptr::null_mut();
    }
    // SAFETY: `getcwd` writes at most `room` bytes into the buffer.
    match unsafe { linux::syscall(linux::number::GETCWD, [target as usize, room, 0, 0, 0, 0]) } {
        Ok(_) => target,
        Err(errno) => {
            if buffer.is_null() {
                heap::release(target.cast());
            }
            if errno.0 == 34 {
                set_errno(ERANGE);
                return ptr::null_mut();
            }
            errno_from(errno, ptr::null_mut())
        }
    }
}

fn path_call(number: usize, path: *const c_char, third: usize, fourth: usize) -> c_int {
    // SAFETY: the public functions' callers promise a NUL-terminated path.
    let path = unsafe { c_string_with_nul(path) };
    let arguments = if number == linux::number::CHDIR {
        [path.as_ptr() as usize, 0, 0, 0, 0, 0]
    } else {
        [AT_FDCWD, path.as_ptr() as usize, third, fourth, 0, 0]
    };
    // SAFETY: each of these calls reads only the NUL-terminated path.
    match unsafe { linux::syscall(number, arguments) } {
        Ok(_) => 0,
        Err(errno) => errno_from(errno, -1),
    }
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn chdir(path: *const c_char) -> c_int {
    path_call(linux::number::CHDIR, path, 0, 0)
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    path_call(linux::number::UNLINKAT, path, 0, 0)
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn rmdir(path: *const c_char) -> c_int {
    path_call(linux::number::UNLINKAT, path, linux::AT_REMOVEDIR, 0)
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn mkdir(path: *const c_char, mode: u16) -> c_int {
    path_call(
        linux::number::MKDIRAT,
        path,
        usize::from(mode) & MODE_MASK,
        0,
    )
}

/// A `DIR`: the directory's descriptor, a buffer of Linux `getdents64` entries and the Darwin
/// `struct dirent` `readdir` last returned.
struct Directory {
    fd: c_int,
    at: usize,
    end: usize,
    buffer: [u8; DIRECTORY_BUFFER],
    entry: [u8; darwin::DARWIN_DIRENT_SIZE],
}

/// # Safety
/// `path` must be a NUL-terminated string.
pub unsafe extern "C" fn opendir(path: *const c_char) -> *mut c_void {
    let flags = darwin::open::DIRECTORY | darwin::open::CLOEXEC;
    // SAFETY: the caller's promise.
    let fd = unsafe { open_with_mode(path, flags as c_int, 0) };
    if fd < 0 {
        return ptr::null_mut();
    }
    let directory = heap::allocate(size_of::<Directory>(), false).cast::<Directory>();
    if directory.is_null() {
        let _ = linux::close(fd as usize);
        return ptr::null_mut();
    }
    // SAFETY: a fresh block of the right size.
    unsafe {
        (&raw mut (*directory).fd).write(fd);
        (&raw mut (*directory).at).write(0);
        (&raw mut (*directory).end).write(0);
    }
    directory.cast()
}

/// # Safety
/// `directory` must be a `DIR` from `opendir`.
pub unsafe extern "C" fn readdir(directory: *mut c_void) -> *mut c_void {
    // SAFETY: the caller's promise.
    let Some(directory) = (unsafe { directory.cast::<Directory>().as_mut() }) else {
        set_errno(EBADF);
        return ptr::null_mut();
    };
    if directory.at >= directory.end {
        // SAFETY: `getdents64` writes at most the buffer's size into it.
        match unsafe {
            linux::syscall(
                linux::number::GETDENTS64,
                [
                    directory.fd as usize,
                    directory.buffer.as_mut_ptr() as usize,
                    DIRECTORY_BUFFER,
                    0,
                    0,
                    0,
                ],
            )
        } {
            Ok(0) => return ptr::null_mut(),
            Ok(read) => {
                directory.at = 0;
                directory.end = read.min(DIRECTORY_BUFFER);
            }
            Err(errno) => return errno_from(errno, ptr::null_mut()),
        }
    }
    let Some((inode, offset, kind, name, record)) =
        darwin::linux_dirent(&directory.buffer[directory.at..directory.end])
    else {
        directory.at = directory.end;
        return ptr::null_mut();
    };
    darwin::darwin_dirent(inode, offset, kind, name, &mut directory.entry);
    directory.at += record;
    directory.entry.as_mut_ptr().cast()
}

/// # Safety
/// `directory` must be a `DIR` from `opendir`, not used after this.
pub unsafe extern "C" fn closedir(directory: *mut c_void) -> c_int {
    // SAFETY: the caller's promise.
    let Some(open) = (unsafe { directory.cast::<Directory>().as_mut() }) else {
        set_errno(EBADF);
        return -1;
    };
    let result = close(open.fd);
    heap::release(directory.cast());
    result
}

/// # Safety
/// `directory` must be a `DIR` from `opendir`.
pub unsafe extern "C" fn dirfd(directory: *mut c_void) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { directory.cast::<Directory>().as_ref() }.map_or(-1, |directory| directory.fd)
}

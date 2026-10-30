//! The C runtime's low-level I/O (`io.h`, D179): `_open`, `_wopen`, `_sopen_s`, `_wsopen_s`,
//! `_close`, `_read`, `_write`, `_lseek` and `_lseeki64` on the Linux descriptors the streams
//! and handles use, so `_fileno`, `_fdopen` and `_open_osfhandle` name the same descriptors.
//! Paths are Windows paths, as `fopen`'s. The flags are `crate::low_level_io`'s. Bytes are read
//! and written as they are, in text mode too: line endings are not translated, so `_setmode`
//! (D189) only records a descriptor's mode and reports the one it had.

use super::runtime::{errno, errno_for_linux, errno_value, set_errno};
use crate::library::Export;
use crate::linux;
use crate::low_level_io::{is_sharing_mode, open_flags, whence};
use crate::text;
use core::ffi::CStr;
use core::sync::atomic::{AtomicI32, Ordering};

const FAILED: i32 = -1;

fn refused(linux_errno: u16) -> i32 {
    set_errno(errno_for_linux(linux_errno));
    FAILED
}

fn open_path(path: impl Iterator<Item = char>, flags: i32) -> i32 {
    let Some((linux_flags, temporary)) = open_flags(flags) else {
        set_errno(errno::EINVAL);
        return FAILED;
    };
    let mut eggos_path = [0u8; text::PATH_CAPACITY];
    let Some(length) = text::eggos_path(path, &mut eggos_path) else {
        set_errno(errno::EINVAL);
        return FAILED;
    };
    let fd = match linux::open(&eggos_path[..length], linux_flags) {
        Ok(fd) => fd,
        Err(error) => return refused(error.0),
    };
    if temporary && let Err(error) = linux::remove(&eggos_path[..length], 0) {
        let _ = linux::close(fd);
        return refused(error.0);
    }
    match i32::try_from(fd) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = linux::close(fd);
            set_errno(errno::EMFILE);
            FAILED
        }
    }
}

unsafe fn narrow_path(path: *const u8) -> Option<&'static [u8]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!path.is_null()).then(|| unsafe { CStr::from_ptr(path.cast()) }.to_bytes())
}

unsafe fn wide_path(path: *const u16) -> Option<&'static [u16]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!path.is_null()).then(|| unsafe { super::super::file::wide_string(path) })
}

unsafe extern "efiapi" fn open(path: *const u8, flags: i32, _permissions: i32) -> i32 {
    // SAFETY: `_open`'s contract: a NUL-terminated path.
    match unsafe { narrow_path(path) } {
        Some(path) => open_path(text::utf8_chars(path), flags),
        None => {
            set_errno(errno::EINVAL);
            FAILED
        }
    }
}

unsafe extern "efiapi" fn wopen(path: *const u16, flags: i32, _permissions: i32) -> i32 {
    // SAFETY: `_wopen`'s contract: a NUL-terminated path.
    match unsafe { wide_path(path) } {
        Some(path) => open_path(text::utf16_chars(path), flags),
        None => {
            set_errno(errno::EINVAL);
            FAILED
        }
    }
}

unsafe fn opened_into(out: *mut i32, sharing: i32, open: impl FnOnce() -> i32) -> i32 {
    if out.is_null() {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    // SAFETY: `_sopen_s`'s contract: `out` is a writable `int`.
    unsafe { out.write(FAILED) };
    if !is_sharing_mode(sharing) {
        set_errno(errno::EINVAL);
        return errno::EINVAL;
    }
    let fd = open();
    // SAFETY: as above.
    unsafe { out.write(fd) };
    match fd {
        FAILED => errno_value(),
        _ => 0,
    }
}

unsafe extern "efiapi" fn sopen_s(
    out: *mut i32,
    path: *const u8,
    flags: i32,
    sharing: i32,
    _permissions: i32,
) -> i32 {
    // SAFETY: `_sopen_s`'s contract: a writable `int` and a NUL-terminated path.
    unsafe {
        opened_into(out, sharing, || match narrow_path(path) {
            Some(path) => open_path(text::utf8_chars(path), flags),
            None => {
                set_errno(errno::EINVAL);
                FAILED
            }
        })
    }
}

unsafe extern "efiapi" fn wsopen_s(
    out: *mut i32,
    path: *const u16,
    flags: i32,
    sharing: i32,
    _permissions: i32,
) -> i32 {
    // SAFETY: `_wsopen_s`'s contract: a writable `int` and a NUL-terminated path.
    unsafe {
        opened_into(out, sharing, || match wide_path(path) {
            Some(path) => open_path(text::utf16_chars(path), flags),
            None => {
                set_errno(errno::EINVAL);
                FAILED
            }
        })
    }
}

fn descriptor(fd: i32) -> Option<usize> {
    let fd = usize::try_from(fd).ok();
    if fd.is_none() {
        set_errno(errno::EBADF);
    }
    fd
}

extern "efiapi" fn close(fd: i32) -> i32 {
    let Some(fd) = descriptor(fd) else {
        return FAILED;
    };
    match linux::close(fd) {
        Ok(_) => 0,
        Err(error) => refused(error.0),
    }
}

unsafe extern "efiapi" fn read(fd: i32, buffer: *mut u8, count: u32) -> i32 {
    let Some(fd) = descriptor(fd) else {
        return FAILED;
    };
    if buffer.is_null() && count > 0 {
        set_errno(errno::EINVAL);
        return FAILED;
    }
    let count = (count as usize).min(i32::MAX as usize);
    let mut empty = [0u8; 0];
    let bytes = match count {
        0 => &mut empty[..],
        // SAFETY: `_read`'s contract: `count` writable bytes at `buffer`.
        _ => unsafe { core::slice::from_raw_parts_mut(buffer, count) },
    };
    match linux::read(fd, bytes) {
        Ok(read) => read as i32,
        Err(error) => refused(error.0),
    }
}

unsafe extern "efiapi" fn write(fd: i32, buffer: *const u8, count: u32) -> i32 {
    let Some(fd) = descriptor(fd) else {
        return FAILED;
    };
    if buffer.is_null() && count > 0 {
        set_errno(errno::EINVAL);
        return FAILED;
    }
    let count = (count as usize).min(i32::MAX as usize);
    let bytes = match count {
        0 => &[][..],
        // SAFETY: `_write`'s contract: `count` readable bytes at `buffer`.
        _ => unsafe { core::slice::from_raw_parts(buffer, count) },
    };
    match linux::write(fd, bytes) {
        Ok(written) => written as i32,
        Err(error) => refused(error.0),
    }
}

fn seek(fd: i32, offset: i64, origin: i32) -> i64 {
    let Some(fd) = descriptor(fd) else {
        return i64::from(FAILED);
    };
    let Some(whence) = whence(origin) else {
        set_errno(errno::EINVAL);
        return i64::from(FAILED);
    };
    match linux::lseek(fd, offset, whence) {
        Ok(position) => position as i64,
        Err(error) => i64::from(refused(error.0)),
    }
}

extern "efiapi" fn lseek(fd: i32, offset: i32, origin: i32) -> i32 {
    let position = seek(fd, i64::from(offset), origin);
    match i32::try_from(position) {
        Ok(position) => position,
        Err(_) => {
            set_errno(errno::EINVAL);
            FAILED
        }
    }
}

extern "efiapi" fn lseeki64(fd: i32, offset: i64, origin: i32) -> i64 {
    seek(fd, offset, origin)
}

const TEXT_MODE: i32 = 0x4000;
const TRANSLATION_MODES: [i32; 5] = [TEXT_MODE, 0x8000, 0x1_0000, 0x2_0000, 0x4_0000];
const TRACKED_DESCRIPTORS: usize = 64;

static TRANSLATION: [AtomicI32; TRACKED_DESCRIPTORS] =
    [const { AtomicI32::new(TEXT_MODE) }; TRACKED_DESCRIPTORS];

extern "efiapi" fn set_mode(fd: i32, mode: i32) -> i32 {
    if !TRANSLATION_MODES.contains(&mode) {
        set_errno(errno::EINVAL);
        return FAILED;
    }
    let Ok(descriptor) = usize::try_from(fd) else {
        set_errno(errno::EBADF);
        return FAILED;
    };
    if let Err(error) = linux::fstat(descriptor) {
        return refused(error.0);
    }
    match TRANSLATION.get(descriptor) {
        Some(slot) => slot.swap(mode, Ordering::AcqRel),
        None => TEXT_MODE,
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"_open" => open,
    b"_wopen" => wopen,
    b"_sopen_s" => sopen_s,
    b"_wsopen_s" => wsopen_s,
    b"_close" => close,
    b"_read" => read,
    b"_write" => write,
    b"_lseek" => lseek,
    b"_lseeki64" => lseeki64,
    b"_setmode" => set_mode,
];

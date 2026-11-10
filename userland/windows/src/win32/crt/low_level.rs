//! The C runtime's low-level I/O (`io.h`, D179): `_open`, `_wopen`, `_sopen_s`, `_wsopen_s`,
//! `_close`, `_read`, `_write`, `_lseek` and `_lseeki64` on the Linux descriptors the streams
//! and handles use, so `_fileno`, `_fdopen` and `_open_osfhandle` name the same descriptors.
//! Paths are Windows paths, as `fopen`'s. The flags are `crate::low_level_io`'s. Each
//! descriptor has a translation mode (D229), from `_open`'s flags or `_fmode`, which `_setmode`
//! changes: `_write` in text mode writes `\n` as `\r\n` and in the wide modes writes the
//! program's UTF-16 as UTF-16 or UTF-8; `_read` in text mode takes `\r\n` as `\n` and stops at
//! Ctrl+Z, seeking back to it, and a last `\r` waits for the next byte, which goes back to the
//! file, or for a descriptor that cannot seek to the next `_read`. The wide modes read bytes as
//! they are.

use super::runtime::{binary_by_default, errno, errno_for_linux, errno_value, set_errno};
use crate::library::Export;
use crate::linux;
use crate::low_level_io::{
    SMALLEST_TRANSLATION, TRANSLATION_MODES, Translation, is_sharing_mode, open_flags, open_mode,
    take_text, translate_written, whence, windows_flag,
};
use crate::text;
use core::ffi::CStr;
use core::sync::atomic::{AtomicI32, Ordering};

const FAILED: i32 = -1;
const SEEK_CURRENT: usize = 1;
const NO_BYTE: i32 = -1;
const TRANSLATION_CHUNK: usize = 1024;
const TRACKED_DESCRIPTORS: usize = 64;

const _: () = assert!(TRANSLATION_CHUNK >= SMALLEST_TRANSLATION);

static TRANSLATION: [AtomicI32; TRACKED_DESCRIPTORS] =
    [const { AtomicI32::new(windows_flag::TEXT) }; TRACKED_DESCRIPTORS];
static PENDING_BYTE: [AtomicI32; TRACKED_DESCRIPTORS] =
    [const { AtomicI32::new(NO_BYTE) }; TRACKED_DESCRIPTORS];

/// The translation mode (`_O_TEXT`, ...) of descriptor `fd`: text for one the runtime does
/// not track.
pub(super) fn translation_mode(fd: usize) -> i32 {
    TRANSLATION
        .get(fd)
        .map_or(windows_flag::TEXT, |slot| slot.load(Ordering::Acquire))
}

/// Forgets what the runtime kept for descriptor `fd`, which is closing: its translation
/// mode and a byte waiting to be read.
pub(super) fn forget_descriptor(fd: usize) {
    if let Some(slot) = TRANSLATION.get(fd) {
        slot.store(windows_flag::TEXT, Ordering::Release);
    }
    if let Some(slot) = PENDING_BYTE.get(fd) {
        slot.store(NO_BYTE, Ordering::Release);
    }
}

fn default_mode() -> i32 {
    match binary_by_default() {
        true => windows_flag::BINARY,
        false => windows_flag::TEXT,
    }
}

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
    if let Some(slot) = TRANSLATION.get(fd) {
        slot.store(open_mode(flags, default_mode()), Ordering::Release);
    }
    if let Some(slot) = PENDING_BYTE.get(fd) {
        slot.store(NO_BYTE, Ordering::Release);
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
    let open = || {
        // SAFETY: `_sopen_s`'s contract: a NUL-terminated path.
        match unsafe { narrow_path(path) } {
            Some(path) => open_path(text::utf8_chars(path), flags),
            None => {
                set_errno(errno::EINVAL);
                FAILED
            }
        }
    };
    // SAFETY: `_sopen_s`'s contract: a writable `int`.
    unsafe { opened_into(out, sharing, open) }
}

unsafe extern "efiapi" fn wsopen_s(
    out: *mut i32,
    path: *const u16,
    flags: i32,
    sharing: i32,
    _permissions: i32,
) -> i32 {
    let open = || {
        // SAFETY: `_wsopen_s`'s contract: a NUL-terminated path.
        match unsafe { wide_path(path) } {
            Some(path) => open_path(text::utf16_chars(path), flags),
            None => {
                set_errno(errno::EINVAL);
                FAILED
            }
        }
    };
    // SAFETY: `_wsopen_s`'s contract: a writable `int`.
    unsafe { opened_into(out, sharing, open) }
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
    forget_descriptor(fd);
    match super::super::file::close_descriptor(fd) {
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
    let read = match Translation::of_mode(translation_mode(fd)) {
        Translation::Text if !bytes.is_empty() => read_text(fd, bytes),
        _ => read_raw(fd, bytes),
    };
    match read {
        Ok(read) => read as i32,
        Err(error) => refused(error),
    }
}

fn read_raw(fd: usize, bytes: &mut [u8]) -> Result<usize, u16> {
    let peeked = super::super::pipe::take_peeked(fd, bytes);
    if peeked != 0 {
        return Ok(peeked);
    }
    linux::read(fd, bytes).map_err(|error| error.0)
}

fn take_pending_byte(fd: usize) -> Option<u8> {
    let slot = PENDING_BYTE.get(fd)?;
    u8::try_from(slot.swap(NO_BYTE, Ordering::AcqRel)).ok()
}

fn give_back(fd: usize, byte: u8) {
    if linux::lseek(fd, -1, SEEK_CURRENT).is_err()
        && let Some(slot) = PENDING_BYTE.get(fd)
    {
        slot.store(i32::from(byte), Ordering::Release);
    }
}

fn read_text(fd: usize, bytes: &mut [u8]) -> Result<usize, u16> {
    let read = match take_pending_byte(fd) {
        Some(byte) => {
            bytes[0] = byte;
            1
        }
        None => read_raw(fd, bytes)?,
    };
    let text = take_text(&mut bytes[..read]);
    if let Some(at) = text.stopped_at {
        let _ = linux::lseek(fd, at as i64 - read as i64, SEEK_CURRENT);
        return Ok(text.kept);
    }
    if !text.return_pending {
        return Ok(text.kept);
    }
    let mut next = [0u8; 1];
    bytes[text.kept] = match read_raw(fd, &mut next) {
        Ok(1) if next[0] == b'\n' => b'\n',
        Ok(1) => {
            give_back(fd, next[0]);
            b'\r'
        }
        _ => b'\r',
    };
    Ok(text.kept + 1)
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
    match Translation::of_mode(translation_mode(fd)) {
        Translation::Binary => match linux::write(fd, bytes) {
            Ok(written) => written as i32,
            Err(error) => refused(error.0),
        },
        translation => write_translated(fd, translation, bytes),
    }
}

fn write_translated(fd: usize, translation: Translation, bytes: &[u8]) -> i32 {
    if translation.is_wide() && !bytes.len().is_multiple_of(2) {
        set_errno(errno::EINVAL);
        return FAILED;
    }
    let terminal = linux::is_terminal(fd);
    let mut out = [0u8; TRANSLATION_CHUNK];
    let mut taken = 0;
    while taken < bytes.len() {
        let (used, put) = translate_written(translation, terminal, &bytes[taken..], &mut out);
        if let Err(error) = linux::write_all(fd, &out[..put]) {
            return match taken {
                0 => refused(error.0),
                _ => taken as i32,
            };
        }
        if used == 0 {
            break;
        }
        taken += used;
    }
    taken as i32
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
        None => windows_flag::TEXT,
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

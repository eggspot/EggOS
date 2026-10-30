//! Anonymous pipes and handle flags (D189): `CreatePipe` makes a Linux pipe whose ends are file
//! handles (`file.rs`), inherited by the programs `CreateProcess` starts when its
//! `SECURITY_ATTRIBUTES` say so; `PeekNamedPipe` tells how many bytes wait and copies them
//! without taking them from the reader, keeping what it had to read until `ReadFile` takes it
//! (`peeked_pipes.rs`); `SetHandleInformation` and `GetHandleInformation` read and change a
//! handle's inheritance (its descriptor's `FD_CLOEXEC`) and its protection from `CloseHandle`
//! (`handle_flags.rs`). Kernel objects (`sync.rs`) live in the layer's memory and are never
//! inherited: their flags are kept for the handle alone, and protect it from closing.

use super::file::{fd_of, handle_of};
use super::sync;
use super::{Bool, FALSE, SpinLock, TRUE, error, error_for, fail};
use crate::handle_flags::{self, ProtectedDescriptors};
use crate::library::Export;
use crate::linux;
use crate::peeked_pipes::{self, PeekedPipes};
use core::sync::atomic::{AtomicBool, Ordering};

const SECURITY_ATTRIBUTES_INHERIT: usize = 2 * core::mem::size_of::<usize>();

static PROTECTED: SpinLock<ProtectedDescriptors> = SpinLock::new(ProtectedDescriptors::new());
static ANY_PROTECTED: AtomicBool = AtomicBool::new(false);
static PEEKED: SpinLock<PeekedPipes> = SpinLock::new(PeekedPipes::new());
static ANY_PEEKED: AtomicBool = AtomicBool::new(false);

/// Whether the `SECURITY_ATTRIBUTES` at `attributes` ask for an inherited handle; `false` for
/// none.
///
/// # Safety
/// `attributes` must be null or a readable `SECURITY_ATTRIBUTES`.
pub(super) unsafe fn inherits(attributes: *const u8) -> bool {
    if attributes.is_null() {
        return false;
    }
    // SAFETY: the caller's duty: `bInheritHandle` follows the length and the descriptor
    // pointer.
    let inherit = unsafe {
        attributes
            .wrapping_add(SECURITY_ATTRIBUTES_INHERIT)
            .cast::<i32>()
            .read_unaligned()
    };
    inherit != 0
}

/// The `open` flag a new descriptor takes so that children inherit it only when `inherit`.
pub(super) fn close_on_exec_unless(inherit: bool) -> usize {
    match inherit {
        true => 0,
        false => linux::O_CLOEXEC,
    }
}

/// Moves the bytes `PeekNamedPipe` kept for `fd` into `buffer`, as many as fit; how many.
pub(super) fn take_peeked(fd: usize, buffer: &mut [u8]) -> usize {
    if !ANY_PEEKED.load(Ordering::Acquire) {
        return 0;
    }
    PEEKED.with(|pipes| {
        let count = pipes.take(fd, buffer);
        ANY_PEEKED.store(!pipes.is_empty(), Ordering::Release);
        count
    })
}

/// Readies `fd` for `CloseHandle`: drops the bytes kept for it; `false`, and nothing done,
/// when its handle is protected from closing.
pub(super) fn closing(fd: usize) -> bool {
    let protected =
        ANY_PROTECTED.load(Ordering::Acquire) && PROTECTED.with(|protected| protected.contains(fd));
    if protected {
        return false;
    }
    if ANY_PEEKED.load(Ordering::Acquire) {
        PEEKED.with(|pipes| {
            pipes.forget(fd);
            ANY_PEEKED.store(!pipes.is_empty(), Ordering::Release);
        });
    }
    true
}

unsafe extern "efiapi" fn create_pipe(
    read: *mut usize,
    write: *mut usize,
    attributes: *const u8,
    _size: u32,
) -> Bool {
    if read.is_null() || write.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `CreatePipe`'s contract: null or a readable `SECURITY_ATTRIBUTES`.
    let inherit = unsafe { inherits(attributes) };
    let (read_end, write_end) = match linux::pipe_with(close_on_exec_unless(inherit)) {
        Ok(ends) => ends,
        Err(errno) => return fail(error_for(errno), FALSE),
    };
    // SAFETY: the caller passes a writable `HANDLE` for the read end.
    unsafe { read.write(handle_of(read_end)) };
    // SAFETY: and another for the write end.
    unsafe { write.write(handle_of(write_end)) };
    TRUE
}

/// Whether `fd` is a pipe.
pub(super) fn is_pipe(fd: usize) -> bool {
    linux::fstat(fd).is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::FIFO)
}

/// The bytes `fd` keeps after reading up to `wanted` of the `waiting` ones, copied into
/// `buffer`: how many were copied and how many wait in all.
///
/// # Safety
/// `buffer` must hold `wanted` writable bytes.
unsafe fn peek(
    fd: usize,
    buffer: *mut u8,
    wanted: usize,
    waiting: usize,
) -> Result<[usize; 2], u32> {
    PEEKED.with(|pipes| {
        let kept = pipes.kept(fd).len();
        let more = wanted.saturating_sub(kept).min(waiting).min(pipes.room(fd));
        let mut fresh = [0u8; peeked_pipes::CAPACITY];
        let read = match more {
            0 => 0,
            _ => linux::read(fd, &mut fresh[..more]).map_err(error_for)?,
        };
        pipes.keep(fd, &fresh[..read]);
        ANY_PEEKED.store(!pipes.is_empty(), Ordering::Release);
        let kept = pipes.kept(fd);
        let copied = kept.len().min(wanted);
        if copied != 0 {
            // SAFETY: the caller's duty: `buffer` holds `wanted` bytes, at least `copied`.
            unsafe { buffer.copy_from_nonoverlapping(kept.as_ptr(), copied) };
        }
        Ok([copied, kept.len() + waiting - read])
    })
}

unsafe extern "efiapi" fn peek_named_pipe(
    handle: usize,
    buffer: *mut u8,
    size: u32,
    read: *mut u32,
    available: *mut u32,
    left_in_message: *mut u32,
) -> Bool {
    let Some(fd) = fd_of(handle).filter(|fd| is_pipe(*fd)) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let waiting = match linux::bytes_to_read(fd) {
        Ok(waiting) => waiting,
        Err(errno) => return fail(error_for(errno), FALSE),
    };
    let wanted = match buffer.is_null() {
        true => 0,
        false => size as usize,
    };
    // SAFETY: `PeekNamedPipe`'s contract: `buffer`, when not null, holds `size` writable bytes.
    let [copied, total] = match unsafe { peek(fd, buffer, wanted, waiting) } {
        Ok(counts) => counts,
        Err(code) => return fail(code, FALSE),
    };
    let ended =
        total == 0 && linux::wait_readable([fd as i32, -1], Some(0)).is_ok_and(|ready| ready != 0);
    if ended {
        return fail(error::BROKEN_PIPE, FALSE);
    }
    let counts = [(read, copied), (available, total), (left_in_message, 0)];
    for (out, count) in counts {
        if !out.is_null() {
            // SAFETY: the caller passes a writable `DWORD` for each count it asks for.
            unsafe { out.write(count.min(u32::MAX as usize) as u32) };
        }
    }
    TRUE
}

/// The descriptor `handle` names and its Linux descriptor flags.
fn descriptor_of(handle: usize) -> Result<(usize, usize), u32> {
    let fd = fd_of(handle).ok_or(error::INVALID_HANDLE)?;
    let flags = linux::control(fd, linux::F_GETFD, 0).map_err(error_for)?;
    Ok((fd, flags))
}

extern "efiapi" fn set_handle_information(handle: usize, mask: u32, flags: u32) -> Bool {
    if sync::is_object_handle(handle) {
        let Some(current) = sync::flags_of(handle) else {
            return fail(error::INVALID_HANDLE, FALSE);
        };
        sync::set_flags(handle, handle_flags::updated(current, mask, flags));
        return TRUE;
    }
    let (fd, descriptor_flags) = match descriptor_of(handle) {
        Ok(found) => found,
        Err(code) => return fail(code, FALSE),
    };
    let protected = PROTECTED.with(|protected| protected.contains(fd));
    let current = handle_flags::of_descriptor(descriptor_flags, protected);
    let new = handle_flags::updated(current, mask, flags);
    let changed = handle_flags::descriptor_flags(descriptor_flags, new);
    if changed != descriptor_flags
        && let Err(errno) = linux::control(fd, linux::F_SETFD, changed)
    {
        return fail(error_for(errno), FALSE);
    }
    let protect = new & handle_flags::PROTECT_FROM_CLOSE != 0;
    if !PROTECTED.with(|protected| protected.set(fd, protect)) {
        return fail(error::NOT_SUPPORTED, FALSE);
    }
    if protect {
        ANY_PROTECTED.store(true, Ordering::Release);
    }
    TRUE
}

unsafe extern "efiapi" fn get_handle_information(handle: usize, flags: *mut u32) -> Bool {
    if flags.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let value = match sync::is_object_handle(handle) {
        true => match sync::flags_of(handle) {
            Some(value) => value,
            None => return fail(error::INVALID_HANDLE, FALSE),
        },
        false => match descriptor_of(handle) {
            Ok((fd, descriptor_flags)) => {
                let protected = PROTECTED.with(|protected| protected.contains(fd));
                handle_flags::of_descriptor(descriptor_flags, protected)
            }
            Err(code) => return fail(code, FALSE),
        },
    };
    // SAFETY: the caller passes a writable `DWORD` for the flags.
    unsafe { flags.write(value) };
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"CreatePipe" => create_pipe,
    b"PeekNamedPipe" => peek_named_pipe,
    b"SetHandleInformation" => set_handle_information,
    b"GetHandleInformation" => get_handle_information,
];

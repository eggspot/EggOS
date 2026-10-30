//! ntdll's file functions that Rust's standard library calls (D189). `NtReadFile` and
//! `NtWriteFile` read and write a file handle's descriptor at once, at the given offset or the
//! file's position, report the result in the `IO_STATUS_BLOCK` and an NT status, signal the
//! event and queue the APC routine they were given; the end of a file is
//! `STATUS_END_OF_FILE` and of a pipe `STATUS_PIPE_BROKEN`. Anonymous pipes come in two steps:
//! `NtOpenFile` of the pipe file system (`\??\PIPE\`), then `NtCreateNamedPipeFile` with no name
//! under it, which makes a Linux pipe and returns the end the access asks for, and `NtOpenFile`
//! with no name under that end, which returns the other end; each end is inherited when its
//! `OBJECT_ATTRIBUTES` say `OBJ_INHERIT`. `RtlNtStatusToDosError` turns a status into its Win32
//! error (`nt_status.rs`).

use super::file::{fd_of, handle_of};
use super::{SpinLock, error_for, sync};
use crate::library::Export;
use crate::linux;
use crate::nt_status;
use core::sync::atomic::{AtomicBool, Ordering};

const POINTER: usize = core::mem::size_of::<usize>();
const ROOT_DIRECTORY: usize = POINTER;
const OBJECT_NAME: usize = 2 * POINTER;
const OBJECT_ATTRIBUTES: usize = 3 * POINTER;
const OBJ_INHERIT: u32 = 2;
const UNICODE_STRING_BUFFER: usize = POINTER;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_READ_DATA: u32 = 1;
const USE_FILE_POSITION: i64 = -2;
const PIPE_FILE_SYSTEM_NAME: &[u8] = b"\\??\\PIPE\\";
const PIPE_FILE_SYSTEM_PATH: &[u8] = b"/\0";
const UNCLAIMED_ENDS: usize = 8;
const EPIPE: u16 = 32;

static UNCLAIMED: SpinLock<[(usize, usize); UNCLAIMED_ENDS]> =
    SpinLock::new([(usize::MAX, usize::MAX); UNCLAIMED_ENDS]);
static ANY_UNCLAIMED: AtomicBool = AtomicBool::new(false);

/// The name and root directory of the `OBJECT_ATTRIBUTES` at `attributes`, and whether they
/// ask for an inherited handle.
///
/// # Safety
/// `attributes` must be a readable `OBJECT_ATTRIBUTES` whose name is null or a readable
/// `UNICODE_STRING`.
unsafe fn object<'a>(attributes: *const u8) -> (&'a [u16], usize, bool) {
    // SAFETY: the caller's duty: the root follows the length, the name the root, and the
    // attributes the name.
    let (root, name, flags) = unsafe {
        (
            attributes
                .wrapping_add(ROOT_DIRECTORY)
                .cast::<usize>()
                .read_unaligned(),
            attributes
                .wrapping_add(OBJECT_NAME)
                .cast::<*const u8>()
                .read_unaligned(),
            attributes
                .wrapping_add(OBJECT_ATTRIBUTES)
                .cast::<u32>()
                .read_unaligned(),
        )
    };
    let units: &[u16] = match name.is_null() {
        true => &[],
        false => {
            // SAFETY: the caller's duty: a `UNICODE_STRING` is a byte length and a buffer.
            let (length, buffer) = unsafe {
                (
                    name.cast::<u16>().read_unaligned() as usize / 2,
                    name.wrapping_add(UNICODE_STRING_BUFFER)
                        .cast::<*const u16>()
                        .read_unaligned(),
                )
            };
            match buffer.is_null() || length == 0 {
                true => &[],
                // SAFETY: the caller's duty: the buffer holds `length` units.
                false => unsafe { core::slice::from_raw_parts(buffer, length) },
            }
        }
    };
    (units, root, flags & OBJ_INHERIT != 0)
}

/// Writes `status` and `information` into the `IO_STATUS_BLOCK` at `block`.
///
/// # Safety
/// `block` must be null or a writable `IO_STATUS_BLOCK`.
unsafe fn report(block: *mut u8, status: u32, information: usize) -> u32 {
    if !block.is_null() {
        // SAFETY: the caller's duty: the status word, then the information word.
        unsafe {
            block.cast::<usize>().write_unaligned(status as usize);
            block
                .wrapping_add(POINTER)
                .cast::<usize>()
                .write_unaligned(information);
        }
    }
    status
}

fn names_pipe_file_system(name: &[u16]) -> bool {
    name.len() == PIPE_FILE_SYSTEM_NAME.len()
        && name.iter().zip(PIPE_FILE_SYSTEM_NAME).all(|(unit, byte)| {
            u8::try_from(*unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(byte))
        })
}

fn set_inherited(fd: usize, inherit: bool) {
    if inherit {
        let _ = linux::control(fd, linux::F_SETFD, 0);
    }
}

fn status_for(errno: eggos_runtime::errno::Errno) -> u32 {
    match errno.0 {
        EPIPE => nt_status::PIPE_CLOSING,
        _ => nt_status::for_dos_error(error_for(errno)),
    }
}

/// Forgets the other end kept for pipe end `fd` as it closes, closing it if nobody opened it.
pub(super) fn closing(fd: usize) {
    if !ANY_UNCLAIMED.load(Ordering::Acquire) {
        return;
    }
    let other = UNCLAIMED.with(|ends| {
        let slot = ends.iter_mut().find(|(end, _)| *end == fd)?;
        let other = slot.1;
        *slot = (usize::MAX, usize::MAX);
        ANY_UNCLAIMED.store(
            ends.iter().any(|(end, _)| *end != usize::MAX),
            Ordering::Release,
        );
        Some(other)
    });
    if let Some(other) = other {
        let _ = linux::close(other);
    }
}

unsafe extern "efiapi" fn nt_open_file(
    handle: *mut usize,
    _access: u32,
    attributes: *const u8,
    io_status: *mut u8,
    _share: u32,
    _options: u32,
) -> u32 {
    if handle.is_null() || attributes.is_null() {
        return nt_status::INVALID_PARAMETER;
    }
    // SAFETY: `NtOpenFile`'s contract: a readable `OBJECT_ATTRIBUTES`.
    let (name, root, inherit) = unsafe { object(attributes) };
    let opened = if names_pipe_file_system(name) {
        match linux::open(PIPE_FILE_SYSTEM_PATH, linux::O_RDONLY | linux::O_CLOEXEC) {
            Ok(fd) => fd,
            Err(errno) => return status_for(errno),
        }
    } else if name.is_empty() && root != 0 {
        let Some(end) = fd_of(root) else {
            return nt_status::INVALID_HANDLE;
        };
        let other = UNCLAIMED.with(|ends| {
            let slot = ends.iter_mut().find(|(kept, _)| *kept == end)?;
            let other = slot.1;
            *slot = (usize::MAX, usize::MAX);
            Some(other)
        });
        match other {
            Some(other) => other,
            None => return nt_status::OBJECT_NAME_NOT_FOUND,
        }
    } else {
        return nt_status::NOT_SUPPORTED;
    };
    set_inherited(opened, inherit);
    // SAFETY: `NtOpenFile`'s contract: a writable handle.
    unsafe { handle.write(handle_of(opened)) };
    // SAFETY: `NtOpenFile`'s contract: a writable `IO_STATUS_BLOCK`.
    unsafe { report(io_status, nt_status::SUCCESS, 0) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn nt_create_named_pipe_file(
    handle: *mut usize,
    access: u32,
    attributes: *const u8,
    io_status: *mut u8,
    _share: u32,
    _disposition: u32,
    _options: u32,
    _pipe_type: u32,
    _read_mode: u32,
    _completion_mode: u32,
    _instances: u32,
    _inbound_quota: u32,
    _outbound_quota: u32,
    _timeout: *const i64,
) -> u32 {
    if handle.is_null() || attributes.is_null() {
        return nt_status::INVALID_PARAMETER;
    }
    // SAFETY: `NtCreateNamedPipeFile`'s contract: a readable `OBJECT_ATTRIBUTES`.
    let (name, _root, inherit) = unsafe { object(attributes) };
    if !name.is_empty() {
        return nt_status::NOT_SUPPORTED;
    }
    let (read_end, write_end) = match linux::pipe_with(linux::O_CLOEXEC) {
        Ok(ends) => ends,
        Err(errno) => return status_for(errno),
    };
    let (ours, other) = match access & (GENERIC_READ | FILE_READ_DATA) != 0 {
        true => (read_end, write_end),
        false => (write_end, read_end),
    };
    let kept = UNCLAIMED.with(|ends| {
        let slot = ends.iter_mut().find(|(end, _)| *end == usize::MAX)?;
        *slot = (ours, other);
        Some(())
    });
    if kept.is_none() {
        let _ = linux::close(read_end);
        let _ = linux::close(write_end);
        return nt_status::TOO_MANY_OPENED_FILES;
    }
    ANY_UNCLAIMED.store(true, Ordering::Release);
    set_inherited(ours, inherit);
    // SAFETY: `NtCreateNamedPipeFile`'s contract: a writable handle.
    unsafe { handle.write(handle_of(ours)) };
    // SAFETY: `NtCreateNamedPipeFile`'s contract: a writable `IO_STATUS_BLOCK`.
    unsafe { report(io_status, nt_status::SUCCESS, 0) }
}

/// The byte offset an I/O call names, `None` for the file's position.
///
/// # Safety
/// `offset` must be null or a readable `LARGE_INTEGER`.
unsafe fn offset(offset: *const i64) -> Option<u64> {
    if offset.is_null() {
        return None;
    }
    // SAFETY: the caller's duty.
    let value = unsafe { offset.read_unaligned() };
    (value != USE_FILE_POSITION && value >= 0).then_some(value as u64)
}

/// Signals `event` and queues `routine(context, io_status, 0)` after an I/O call finished.
fn after_io(event: usize, routine: usize, context: usize, io_status: *mut u8) {
    if event != 0 {
        sync::set_event_quietly(event, true);
    }
    if routine != 0 {
        let _ = super::overlapped::queue_call(routine, [context, io_status as usize, 0]);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn nt_read_file(
    handle: usize,
    event: usize,
    routine: usize,
    context: usize,
    io_status: *mut u8,
    buffer: *mut u8,
    length: u32,
    byte_offset: *const i64,
    _key: *const u32,
) -> u32 {
    let Some(fd) = fd_of(handle) else {
        return nt_status::INVALID_HANDLE;
    };
    let length = length as usize;
    // SAFETY: `NtReadFile`'s contract: `buffer` holds `length` writable bytes.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, length) };
    // SAFETY: `NtReadFile`'s contract: null or a readable offset.
    let result = match unsafe { offset(byte_offset) } {
        Some(at) => linux::read_at(fd, buffer, at),
        None => match super::pipe::take_peeked(fd, buffer) {
            0 => linux::read(fd, buffer),
            peeked => Ok(peeked),
        },
    };
    let (status, count) = match result {
        Ok(0) if length != 0 && super::pipe::is_pipe(fd) => (nt_status::PIPE_BROKEN, 0),
        Ok(0) if length != 0 => (nt_status::END_OF_FILE, 0),
        Ok(count) => (nt_status::SUCCESS, count),
        Err(errno) => (status_for(errno), 0),
    };
    // SAFETY: `NtReadFile`'s contract: a writable `IO_STATUS_BLOCK`.
    unsafe { report(io_status, status, count) };
    after_io(event, routine, context, io_status);
    status
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn nt_write_file(
    handle: usize,
    event: usize,
    routine: usize,
    context: usize,
    io_status: *mut u8,
    buffer: *const u8,
    length: u32,
    byte_offset: *const i64,
    _key: *const u32,
) -> u32 {
    let Some(fd) = fd_of(handle) else {
        return nt_status::INVALID_HANDLE;
    };
    // SAFETY: `NtWriteFile`'s contract: `buffer` holds `length` readable bytes.
    let bytes = unsafe { core::slice::from_raw_parts(buffer, length as usize) };
    // SAFETY: `NtWriteFile`'s contract: null or a readable offset.
    let result = match unsafe { offset(byte_offset) } {
        Some(at) => linux::write_all_at(fd, bytes, at),
        None => linux::write_all(fd, bytes),
    };
    let (status, count) = match result {
        Ok(()) => (nt_status::SUCCESS, bytes.len()),
        Err(errno) => (status_for(errno), 0),
    };
    // SAFETY: `NtWriteFile`'s contract: a writable `IO_STATUS_BLOCK`.
    unsafe { report(io_status, status, count) };
    after_io(event, routine, context, io_status);
    status
}

extern "efiapi" fn rtl_nt_status_to_dos_error(status: u32) -> u32 {
    nt_status::dos_error(status)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"NtOpenFile" => nt_open_file,
    b"NtCreateNamedPipeFile" => nt_create_named_pipe_file [six_stack_arguments],
    b"NtReadFile" => nt_read_file [two_stack_arguments],
    b"NtWriteFile" => nt_write_file [two_stack_arguments],
    b"RtlNtStatusToDosError" => rtl_nt_status_to_dos_error,
];

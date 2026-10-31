//! Overlapped I/O and completion routines (D189), as Rust's standard library uses them for the
//! pipes to the programs it starts. An overlapped `ReadFile` on a pipe reads at once when the
//! pipe has bytes or has ended, and otherwise returns `ERROR_IO_PENDING` and keeps the read
//! (`pending_reads.rs`): a wait on its event looks at the pipe every 10 ms and finishes the read
//! once it can, `GetOverlappedResult` finishes it (waiting when asked to) and `CancelIo` cancels
//! it. On files, overlapped reads and writes happen at once at the `OVERLAPPED`'s offset.
//! `ReadFileEx` and `WriteFileEx` do the I/O at once and queue the completion routine for the
//! thread's next alertable wait (`queued_calls.rs`), which runs it and returns
//! `WAIT_IO_COMPLETION`.

use super::file::{fd_of, handle_of};
use super::{Bool, FALSE, SpinLock, TRUE, error, error_for, fail, sync, thread_environment};
use crate::library::Export;
use crate::linux;
use crate::nt_status;
use crate::pending_reads::{PendingRead, PendingReads};
use crate::queued_calls::{QueuedCall, QueuedCalls};
use core::sync::atomic::{AtomicBool, Ordering};

const POINTER: usize = core::mem::size_of::<usize>();
const INTERNAL: usize = 0;
const INTERNAL_HIGH: usize = POINTER;
const OFFSET: usize = 2 * POINTER;
const EVENT: usize = 2 * POINTER + 8;
/// What alertable waits return after running queued calls.
pub(super) const WAIT_IO_COMPLETION: u32 = 0xC0;

static PENDING: SpinLock<PendingReads> = SpinLock::new(PendingReads::new());
static ANY_PENDING: AtomicBool = AtomicBool::new(false);
static QUEUED: SpinLock<QueuedCalls> = SpinLock::new(QueuedCalls::new());
static ANY_QUEUED: AtomicBool = AtomicBool::new(false);

/// Queues `routine(arguments)` for the calling thread's next alertable wait; `false` when the
/// queue is full.
pub(super) fn queue_call(routine: usize, arguments: [usize; 3]) -> bool {
    queue_call_for(thread_environment::thread_id(), routine, arguments)
}

fn queue_call_for(thread: u32, routine: usize, arguments: [usize; 3]) -> bool {
    let call = QueuedCall {
        thread,
        routine,
        arguments,
    };
    let queued = QUEUED.with(|queue| {
        let queued = queue.push(call);
        ANY_QUEUED.store(!queue.is_empty(), Ordering::Release);
        queued
    });
    if queued {
        sync::wake_waiters();
    }
    queued
}

/// Drops the calls still queued for the calling thread, which is ending.
pub(super) fn forget_queued_calls() {
    if !ANY_QUEUED.load(Ordering::Acquire) {
        return;
    }
    let thread = thread_environment::thread_id();
    QUEUED.with(|queue| {
        while queue.take_for(thread).is_some() {}
        ANY_QUEUED.store(!queue.is_empty(), Ordering::Release);
    });
}

extern "efiapi" fn queue_user_apc(routine: usize, thread: usize, data: usize) -> u32 {
    let target = match thread {
        super::CURRENT_THREAD => Some((sync::STILL_ACTIVE, thread_environment::thread_id())),
        handle => sync::thread_state(handle),
    };
    match target {
        _ if routine == 0 => fail(error::INVALID_PARAMETER, 0),
        Some((sync::STILL_ACTIVE, id)) => match queue_call_for(id, routine, [data, 0, 0]) {
            true => 1,
            false => fail(error::NOT_ENOUGH_MEMORY, 0),
        },
        _ => fail(error::INVALID_HANDLE, 0),
    }
}

/// Runs the calls queued for the calling thread, oldest first; whether there were any.
pub(super) fn run_queued_calls() -> bool {
    if !ANY_QUEUED.load(Ordering::Acquire) {
        return false;
    }
    let thread = thread_environment::thread_id();
    let mut ran = false;
    while let Some(call) = QUEUED.with(|queue| {
        let call = queue.take_for(thread);
        ANY_QUEUED.store(!queue.is_empty(), Ordering::Release);
        call
    }) {
        let [first, second, third] = call.arguments;
        // SAFETY: the program gave this routine for exactly these three arguments, to run on
        // this thread, whose TEB register is set.
        unsafe { super::call_program(call.routine, [first, second, third, 0]) };
        ran = true;
    }
    ran
}

/// Finishes the reads in progress whose events are among `handles` and whose pipes can be read
/// now; whether one of them is still in progress, so that the wait looks again.
pub(super) fn finish_ready_reads(handles: &[usize]) -> bool {
    if !ANY_PENDING.load(Ordering::Acquire) {
        return false;
    }
    let mut waiting = false;
    for handle in handles {
        let Some(read) = PENDING.with(|reads| reads.take_with_event(*handle)) else {
            continue;
        };
        if readable(read.fd) {
            finish_pending(&read);
        } else {
            waiting = true;
            keep(read);
        }
    }
    refresh_any_pending();
    waiting
}

fn refresh_any_pending() {
    PENDING.with(|reads| ANY_PENDING.store(!reads.is_empty(), Ordering::Release));
}

fn keep(read: PendingRead) -> bool {
    let kept = PENDING.with(|reads| reads.insert(read));
    ANY_PENDING.store(true, Ordering::Release);
    kept
}

fn readable(fd: usize) -> bool {
    matches!(linux::wait_readable([fd as i32, -1], Some(0)), Ok(ready) if ready > 0)
}

unsafe fn record(overlapped: usize, status: u32, count: usize) {
    let overlapped = overlapped as *mut u8;
    // SAFETY: the caller's duty: `Internal` and `InternalHigh` are its first two words.
    unsafe {
        overlapped
            .wrapping_add(INTERNAL)
            .cast::<usize>()
            .write_unaligned(status as usize)
    };
    // SAFETY: as above.
    unsafe {
        overlapped
            .wrapping_add(INTERNAL_HIGH)
            .cast::<usize>()
            .write_unaligned(count)
    };
}

unsafe fn event_of(overlapped: usize) -> usize {
    // SAFETY: the caller's duty: `hEvent` follows the offset words.
    unsafe {
        (overlapped as *const u8)
            .wrapping_add(EVENT)
            .cast::<usize>()
            .read_unaligned()
    }
}

unsafe fn offset_of(overlapped: usize) -> u64 {
    // SAFETY: the caller's duty: `Offset` and `OffsetHigh` follow the two status words.
    let [low, high] = unsafe {
        (overlapped as *const u8)
            .wrapping_add(OFFSET)
            .cast::<[u32; 2]>()
            .read_unaligned()
    };
    u64::from(low) | u64::from(high) << 32
}

fn set_event(event: usize, signaled: bool) {
    if event != 0 {
        sync::set_event_quietly(event, signaled);
    }
}

fn read_now(read: &PendingRead) -> (u32, usize) {
    // SAFETY: the program keeps the buffer of a read in progress writable until it finishes.
    let buffer = unsafe { core::slice::from_raw_parts_mut(read.buffer as *mut u8, read.length) };
    let peeked = super::pipe::take_peeked(read.fd, buffer);
    if peeked != 0 {
        return (nt_status::SUCCESS, peeked);
    }
    match linux::read(read.fd, buffer) {
        Ok(0) if read.length != 0 => (nt_status::PIPE_BROKEN, 0),
        Ok(count) => (nt_status::SUCCESS, count),
        Err(errno) => (nt_status::for_dos_error(error_for(errno)), 0),
    }
}

fn finish_pending(read: &PendingRead) {
    let (status, count) = read_now(read);
    // SAFETY: the program keeps the `OVERLAPPED` of a read in progress until it finishes.
    unsafe { record(read.overlapped, status, count) };
    set_event(read.event, true);
}

unsafe fn read_file_at(
    fd: usize,
    buffer: *mut u8,
    length: usize,
    overlapped: usize,
) -> (u32, usize) {
    // SAFETY: the caller's duty.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, length) };
    // SAFETY: as above.
    let offset = unsafe { offset_of(overlapped) };
    match linux::read_at(fd, buffer, offset) {
        Ok(0) if length != 0 => (nt_status::END_OF_FILE, 0),
        Ok(count) => (nt_status::SUCCESS, count),
        Err(errno) => (nt_status::for_dos_error(error_for(errno)), 0),
    }
}

unsafe fn finished(
    overlapped: usize,
    status: u32,
    count: usize,
    counted: *mut u32,
    signal: bool,
) -> Bool {
    // SAFETY: the caller's duty.
    unsafe { record(overlapped, status, count) };
    if signal {
        // SAFETY: the caller's duty.
        set_event(unsafe { event_of(overlapped) }, true);
    }
    if !counted.is_null() {
        // SAFETY: the caller's duty.
        unsafe { counted.write(count as u32) };
    }
    match status {
        nt_status::SUCCESS => TRUE,
        _ => fail(nt_status::dos_error(status), FALSE),
    }
}

/// `ReadFile` with an `OVERLAPPED`.
///
/// # Safety
/// `ReadFile`'s contract: `buffer` holds `length` writable bytes and stays so, with the
/// `OVERLAPPED`, until the read finishes; `read` is null or a writable `DWORD`.
pub(super) unsafe fn read_file(
    handle: usize,
    buffer: *mut u8,
    length: u32,
    read: *mut u32,
    overlapped: usize,
) -> Bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    let length = length as usize;
    if !super::pipe::is_pipe(fd) {
        // SAFETY: `ReadFile`'s contract.
        let (status, count) = unsafe { read_file_at(fd, buffer, length, overlapped) };
        // SAFETY: as above.
        return unsafe { finished(overlapped, status, count, read, true) };
    }
    let pending = PendingRead {
        fd,
        buffer: buffer as usize,
        length,
        overlapped,
        // SAFETY: `ReadFile`'s contract: a readable `OVERLAPPED`.
        event: unsafe { event_of(overlapped) },
    };
    if readable(fd) || super::pipe::has_peeked(fd) {
        let (status, count) = read_now(&pending);
        // SAFETY: `ReadFile`'s contract.
        return unsafe { finished(overlapped, status, count, read, true) };
    }
    if !keep(pending) {
        refresh_any_pending();
        return fail(error::NOT_ENOUGH_MEMORY, FALSE);
    }
    // SAFETY: `ReadFile`'s contract: a writable `OVERLAPPED`.
    unsafe { record(overlapped, nt_status::PENDING, 0) };
    set_event(pending.event, false);
    fail(error::IO_PENDING, FALSE)
}

/// `WriteFile` with an `OVERLAPPED`: the write happens at once, at the `OVERLAPPED`'s offset on
/// a file.
///
/// # Safety
/// `WriteFile`'s contract: `buffer` holds `length` readable bytes, `overlapped` is a writable
/// `OVERLAPPED` and `written` null or a writable `DWORD`.
pub(super) unsafe fn write_file(
    handle: usize,
    buffer: *const u8,
    length: u32,
    written: *mut u32,
    overlapped: usize,
) -> Bool {
    // SAFETY: `WriteFile`'s contract.
    let status = unsafe { write_now(handle, buffer, length as usize, overlapped) };
    let count = match status {
        nt_status::SUCCESS => length as usize,
        _ => 0,
    };
    // SAFETY: as above.
    unsafe { finished(overlapped, status, count, written, true) }
}

unsafe fn write_now(handle: usize, buffer: *const u8, length: usize, overlapped: usize) -> u32 {
    let Some(fd) = fd_of(handle) else {
        return nt_status::INVALID_HANDLE;
    };
    // SAFETY: the caller's duty.
    let bytes = unsafe { core::slice::from_raw_parts(buffer, length) };
    let regular = linux::fstat(fd)
        .is_ok_and(|words| linux::file_mode(&words) & linux::FILE_TYPE == linux::REGULAR_FILE);
    let result = match regular {
        // SAFETY: the caller's duty.
        true => linux::write_all_at(fd, bytes, unsafe { offset_of(overlapped) }),
        false => linux::write_all(fd, bytes),
    };
    match result {
        Ok(()) => nt_status::SUCCESS,
        Err(errno) if errno.0 == EPIPE => nt_status::PIPE_CLOSING,
        Err(errno) => nt_status::for_dos_error(error_for(errno)),
    }
}

const EPIPE: u16 = 32;

unsafe extern "efiapi" fn get_overlapped_result(
    _handle: usize,
    overlapped: usize,
    transferred: *mut u32,
    wait: Bool,
) -> Bool {
    if overlapped == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    if ANY_PENDING.load(Ordering::Acquire)
        && let Some(read) = PENDING.with(|reads| reads.take(overlapped))
    {
        if wait != FALSE || readable(read.fd) {
            finish_pending(&read);
            refresh_any_pending();
        } else {
            keep(read);
            return fail(error::IO_INCOMPLETE, FALSE);
        }
    }
    let operation = overlapped as *const u8;
    // SAFETY: `GetOverlappedResult`'s contract: the `OVERLAPPED` of an operation.
    let status = unsafe {
        operation
            .wrapping_add(INTERNAL)
            .cast::<usize>()
            .read_unaligned()
    } as u32;
    // SAFETY: as above.
    let count = unsafe {
        operation
            .wrapping_add(INTERNAL_HIGH)
            .cast::<usize>()
            .read_unaligned()
    };
    if !transferred.is_null() {
        // SAFETY: `GetOverlappedResult`'s contract: a writable `DWORD`.
        unsafe { transferred.write(count as u32) };
    }
    match status {
        nt_status::SUCCESS => TRUE,
        _ => fail(nt_status::dos_error(status), FALSE),
    }
}

extern "efiapi" fn cancel_io(handle: usize) -> Bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    if ANY_PENDING.load(Ordering::Acquire) {
        while let Some(read) = PENDING.with(|reads| reads.take_from(fd)) {
            // SAFETY: the program keeps the `OVERLAPPED` of a read in progress until it ends.
            unsafe { record(read.overlapped, nt_status::CANCELLED, 0) };
            set_event(read.event, true);
        }
        refresh_any_pending();
    }
    TRUE
}

/// Ends the reads in progress from `fd` as it closes, as cancelled.
pub(super) fn closing(fd: usize) {
    if ANY_PENDING.load(Ordering::Acquire) {
        let _ = cancel_io(handle_of(fd));
    }
}

fn complete_later(routine: usize, status: u32, count: usize, overlapped: usize) -> Bool {
    // SAFETY: `ReadFileEx`' and `WriteFileEx`' contract: a writable `OVERLAPPED`.
    unsafe { record(overlapped, status, count) };
    let error = nt_status::dos_error(status) as usize;
    match routine == 0 || queue_call(routine, [error, count, overlapped]) {
        true => TRUE,
        false => fail(error::NOT_ENOUGH_MEMORY, FALSE),
    }
}

unsafe extern "efiapi" fn read_file_ex(
    handle: usize,
    buffer: *mut u8,
    length: u32,
    overlapped: usize,
    routine: usize,
) -> Bool {
    let Some(fd) = fd_of(handle) else {
        return fail(error::INVALID_HANDLE, FALSE);
    };
    if overlapped == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let length = length as usize;
    let (status, count) = match super::pipe::is_pipe(fd) {
        true => read_now(&PendingRead {
            fd,
            buffer: buffer as usize,
            length,
            overlapped,
            event: 0,
        }),
        // SAFETY: `ReadFileEx`' contract: `buffer` holds `length` writable bytes.
        false => unsafe { read_file_at(fd, buffer, length, overlapped) },
    };
    match status {
        nt_status::SUCCESS => complete_later(routine, status, count, overlapped),
        _ => fail(nt_status::dos_error(status), FALSE),
    }
}

unsafe extern "efiapi" fn write_file_ex(
    handle: usize,
    buffer: *const u8,
    length: u32,
    overlapped: usize,
    routine: usize,
) -> Bool {
    if overlapped == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `WriteFileEx`' contract: `buffer` holds `length` readable bytes.
    let status = unsafe { write_now(handle, buffer, length as usize, overlapped) };
    match status {
        nt_status::SUCCESS => complete_later(routine, status, length as usize, overlapped),
        _ => fail(nt_status::dos_error(status), FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetOverlappedResult" => get_overlapped_result,
    b"CancelIo" => cancel_io,
    b"ReadFileEx" => read_file_ex,
    b"WriteFileEx" => write_file_ex,
    b"QueueUserAPC" => queue_user_apc,
];

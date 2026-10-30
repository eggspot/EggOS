//! Time: the tick counts and performance counter from the monotonic clock (the counter at
//! 10 MHz, the frequency Windows reports), the system time as a `FILETIME` (100 ns intervals
//! since 1601) from the real-time clock, its calendar form (`SYSTEMTIME`, in UTC: local time is
//! UTC until EggOS has time zones), `Sleep`, and `SleepEx`, which runs the calls
//! queued for the thread when alertable (D189). Waitable timers are not there yet:
//! `CreateWaitableTimerExW` fails with `ERROR_NOT_SUPPORTED`, so Rust's `thread::sleep` uses
//! `Sleep`.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::calendar::{self, SystemTime};
use crate::library::Export;
use crate::linux::{self, CLOCK_MONOTONIC, CLOCK_REALTIME};

const PERFORMANCE_FREQUENCY: i64 = 10_000_000;
const NANOSECONDS_PER_TICK: u64 = 100;
const FILETIME_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;
const INFINITE: u32 = u32::MAX;

extern "efiapi" fn get_tick_count64() -> u64 {
    linux::clock_nanoseconds(CLOCK_MONOTONIC) / 1_000_000
}

extern "efiapi" fn get_tick_count() -> u32 {
    get_tick_count64() as u32
}

unsafe extern "efiapi" fn query_performance_counter(counter: *mut i64) -> Bool {
    let ticks = linux::clock_nanoseconds(CLOCK_MONOTONIC) / NANOSECONDS_PER_TICK;
    // SAFETY: the caller passes a writable `LARGE_INTEGER`.
    unsafe { counter.write(ticks as i64) };
    TRUE
}

unsafe extern "efiapi" fn query_performance_frequency(frequency: *mut i64) -> Bool {
    // SAFETY: the caller passes a writable `LARGE_INTEGER`.
    unsafe { frequency.write(PERFORMANCE_FREQUENCY) };
    TRUE
}

unsafe extern "efiapi" fn get_system_time_as_file_time(time: *mut u64) {
    let intervals = now();
    // SAFETY: the caller passes a writable `FILETIME`: two `DWORD`s, low first, which is a
    // little-endian 64-bit value at 4-byte alignment.
    unsafe { time.write_unaligned(intervals) };
}

fn now() -> u64 {
    let nanoseconds = linux::clock_nanoseconds(CLOCK_REALTIME);
    nanoseconds / NANOSECONDS_PER_TICK + FILETIME_EPOCH_OFFSET_SECONDS * 10_000_000
}

unsafe extern "efiapi" fn get_system_time(time: *mut u8) {
    let calendar = calendar::system_time(now()).unwrap_or_default();
    // SAFETY: the caller passes a writable `SYSTEMTIME` (16 bytes).
    unsafe { time.copy_from_nonoverlapping(calendar.to_bytes().as_ptr(), 16) };
}

unsafe extern "efiapi" fn file_time_to_system_time(file_time: *const u64, time: *mut u8) -> Bool {
    // SAFETY: the caller passes a readable `FILETIME` (4-byte aligned).
    let value = unsafe { file_time.read_unaligned() };
    let Some(calendar) = calendar::system_time(value).filter(|_| value <= i64::MAX as u64) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    // SAFETY: the caller passes a writable `SYSTEMTIME`.
    unsafe { time.copy_from_nonoverlapping(calendar.to_bytes().as_ptr(), 16) };
    TRUE
}

unsafe extern "efiapi" fn system_time_to_file_time(time: *const u8, file_time: *mut u64) -> Bool {
    let mut bytes = [0u8; 16];
    // SAFETY: the caller passes a readable `SYSTEMTIME`.
    unsafe { bytes.as_mut_ptr().copy_from_nonoverlapping(time, 16) };
    let Some(value) = calendar::file_time(&SystemTime::from_bytes(bytes)) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    // SAFETY: the caller passes a writable `FILETIME`.
    unsafe { file_time.write_unaligned(value) };
    TRUE
}

unsafe extern "efiapi" fn file_time_to_local_file_time(from: *const u64, to: *mut u64) -> Bool {
    // SAFETY: the caller passes a readable `FILETIME`.
    let time = unsafe { from.read_unaligned() };
    // SAFETY: and a writable one.
    unsafe { to.write_unaligned(time) };
    TRUE
}

extern "efiapi" fn sleep(milliseconds: u32) {
    match milliseconds {
        0 => linux::yield_now(),
        INFINITE => loop {
            linux::sleep_nanoseconds(u64::MAX / 2);
        },
        _ => linux::sleep_nanoseconds(u64::from(milliseconds) * 1_000_000),
    }
}

extern "efiapi" fn sleep_ex(milliseconds: u32, alertable: Bool) -> u32 {
    if alertable != FALSE && super::overlapped::run_queued_calls() {
        return super::overlapped::WAIT_IO_COMPLETION;
    }
    sleep(milliseconds);
    0
}

extern "efiapi" fn create_waitable_timer_ex_w(
    _attributes: usize,
    _name: *const u16,
    _flags: u32,
    _access: u32,
) -> usize {
    fail(error::NOT_SUPPORTED, 0)
}

extern "efiapi" fn set_waitable_timer(
    _timer: usize,
    _due: *const i64,
    _period: i32,
    _routine: usize,
    _argument: usize,
    _resume: Bool,
) -> Bool {
    fail(error::INVALID_HANDLE, FALSE)
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetTickCount64" => get_tick_count64,
    b"CreateWaitableTimerExW" => create_waitable_timer_ex_w,
    b"SetWaitableTimer" => set_waitable_timer,
    b"GetTickCount" => get_tick_count,
    b"QueryPerformanceCounter" => query_performance_counter,
    b"QueryPerformanceFrequency" => query_performance_frequency,
    b"GetSystemTimeAsFileTime" => get_system_time_as_file_time,
    b"GetSystemTimePreciseAsFileTime" => get_system_time_as_file_time,
    b"GetSystemTime" => get_system_time,
    b"GetLocalTime" => get_system_time,
    b"FileTimeToSystemTime" => file_time_to_system_time,
    b"SystemTimeToFileTime" => system_time_to_file_time,
    b"FileTimeToLocalFileTime" => file_time_to_local_file_time,
    b"LocalFileTimeToFileTime" => file_time_to_local_file_time,
    b"Sleep" => sleep,
    b"SleepEx" => sleep_ex,
];

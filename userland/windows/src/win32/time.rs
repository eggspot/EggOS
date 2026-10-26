//! Time: the tick counts and performance counter from the monotonic clock (the counter at
//! 10 MHz, the frequency Windows reports), the system time as a `FILETIME` (100 ns intervals
//! since 1601) from the real-time clock, and `Sleep`.

use super::{Bool, TRUE};
use crate::library::Export;
use crate::linux::{self, CLOCK_MONOTONIC, CLOCK_REALTIME};

const PERFORMANCE_FREQUENCY: i64 = 10_000_000;
const NANOSECONDS_PER_TICK: u64 = 100;
const FILETIME_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;
const INFINITE: u32 = u32::MAX;

extern "win64" fn get_tick_count64() -> u64 {
    linux::clock_nanoseconds(CLOCK_MONOTONIC) / 1_000_000
}

extern "win64" fn get_tick_count() -> u32 {
    get_tick_count64() as u32
}

unsafe extern "win64" fn query_performance_counter(counter: *mut i64) -> Bool {
    let ticks = linux::clock_nanoseconds(CLOCK_MONOTONIC) / NANOSECONDS_PER_TICK;
    // SAFETY: the caller passes a writable `LARGE_INTEGER`.
    unsafe { counter.write(ticks as i64) };
    TRUE
}

unsafe extern "win64" fn query_performance_frequency(frequency: *mut i64) -> Bool {
    // SAFETY: the caller passes a writable `LARGE_INTEGER`.
    unsafe { frequency.write(PERFORMANCE_FREQUENCY) };
    TRUE
}

unsafe extern "win64" fn get_system_time_as_file_time(time: *mut u64) {
    let nanoseconds = linux::clock_nanoseconds(CLOCK_REALTIME);
    let intervals = nanoseconds / NANOSECONDS_PER_TICK + FILETIME_EPOCH_OFFSET_SECONDS * 10_000_000;
    // SAFETY: the caller passes a writable `FILETIME`: two `DWORD`s, low first, which is a
    // little-endian 64-bit value at 4-byte alignment.
    unsafe { time.write_unaligned(intervals) };
}

extern "win64" fn sleep(milliseconds: u32) {
    match milliseconds {
        0 => linux::yield_now(),
        INFINITE => loop {
            linux::sleep_nanoseconds(u64::MAX / 2);
        },
        _ => linux::sleep_nanoseconds(u64::from(milliseconds) * 1_000_000),
    }
}

extern "win64" fn sleep_ex(milliseconds: u32, _alertable: Bool) -> u32 {
    sleep(milliseconds);
    0
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetTickCount64" => get_tick_count64,
    b"GetTickCount" => get_tick_count,
    b"QueryPerformanceCounter" => query_performance_counter,
    b"QueryPerformanceFrequency" => query_performance_frequency,
    b"GetSystemTimeAsFileTime" => get_system_time_as_file_time,
    b"GetSystemTimePreciseAsFileTime" => get_system_time_as_file_time,
    b"Sleep" => sleep,
    b"SleepEx" => sleep_ex,
];

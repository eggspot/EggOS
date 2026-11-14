//! Time: the tick counts and performance counter from the monotonic clock (the counter at
//! 10 MHz, the frequency Windows reports), the system time as a `FILETIME` (100 ns intervals
//! since 1601) from the real-time clock, its calendar form (`SYSTEMTIME`), local time in the
//! zone `TZ` names or else the `time.zone` setting, with its past from its TZif file when
//! there (`GetLocalTime`, `FileTimeToLocalFileTime`, `GetTimeZoneInformation`, D194, D579),
//! `Sleep`, and `SleepEx`, which runs the calls
//! queued for the thread when alertable (D189). Waitable timers are not there yet:
//! `CreateWaitableTimerExW` fails with `ERROR_NOT_SUPPORTED`, so Rust's `thread::sleep` uses
//! `Sleep`.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::calendar::{self, SystemTime};
use crate::library::Export;
use crate::linux::{self, CLOCK_MONOTONIC, CLOCK_REALTIME};
use eggos_calendar::zone::{self, MonthRule, Rule, UTC, Zone};

pub(super) const PERFORMANCE_FREQUENCY: i64 = 10_000_000;
pub(super) const NANOSECONDS_PER_TICK: u64 = 100;
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

/// The time now as a `FILETIME`: 100 ns intervals since 1601.
pub(super) fn now() -> u64 {
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

const SETTINGS: &[u8] = b"/home/root/.config/eggos/settings\0";
const SETTINGS_BYTES: usize = 4096;
const INTERVALS_PER_SECOND: i64 = 10_000_000;
const TIME_ZONE_INFORMATION_BYTES: usize = 172;
const TIME_ZONE_ID_UNKNOWN: u32 = 0;
const TIME_ZONE_ID_STANDARD: u32 = 1;
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

const TZIF_BYTES: usize = 16 * 1024;

static LOCAL_ZONE: super::SpinLock<Option<Zone>> = super::SpinLock::new(None);

fn read_all(path: &[u8], out: &mut [u8]) -> Option<usize> {
    let descriptor = linux::open(path, 0).ok()?;
    let mut length = 0;
    while let Some(free) = out.get_mut(length..).filter(|free| !free.is_empty()) {
        match linux::read(descriptor, free) {
            Ok(0) | Err(_) => break,
            Ok(count) => length += count,
        }
    }
    let _ = linux::close(descriptor);
    Some(length)
}

fn zone_named(text: &str) -> Option<Zone> {
    let rule = zone::zone(text)?;
    let mut path = [0u8; 64];
    let Some(path) = zone::tzif_path(text, &mut path) else {
        return Some(Zone::of_rule(rule));
    };
    let mut file = [0u8; TZIF_BYTES];
    match read_all(path, &mut file) {
        Some(length) if length > 0 => Some(Zone::with_history(rule, &file[..length])),
        _ => Some(Zone::of_rule(rule)),
    }
}

fn zone_named_by_settings() -> Option<Zone> {
    let mut file = [0u8; SETTINGS_BYTES];
    let length = read_all(SETTINGS, &mut file)?;
    let name = eggos_settings_file::value(file.get(..length)?, eggos_settings_file::TIME_ZONE)?;
    zone_named(core::str::from_utf8(name).ok()?)
}

fn load_local_zone() -> Zone {
    let mut text = [0u8; 64];
    match super::environment::variable(b"TZ", &mut text) {
        Some(length) => text
            .get(..length)
            .and_then(|text| core::str::from_utf8(text).ok())
            .filter(|text| !text.is_empty())
            .and_then(zone_named)
            .unwrap_or(Zone::of_rule(UTC)),
        None => zone_named_by_settings().unwrap_or(Zone::of_rule(UTC)),
    }
}

/// Runs `work` with the process's local zone: `TZ`, else the `time.zone` setting, with the
/// zone's past when its TZif file is there (D579), read once and again after `_tzset`.
pub(super) fn with_local_zone<R>(work: impl FnOnce(&Zone) -> R) -> R {
    LOCAL_ZONE.with(|zone| work(zone.get_or_insert_with(load_local_zone)))
}

/// Reads the local zone again (`_tzset`).
pub(super) fn reload_local_zone() {
    let zone = load_local_zone();
    LOCAL_ZONE.with(|kept| *kept = Some(zone));
}

fn seconds_of(intervals: u64) -> i64 {
    (intervals / INTERVALS_PER_SECOND as u64) as i64 - FILETIME_EPOCH_OFFSET_SECONDS as i64
}

fn shifted(intervals: u64, seconds: i64) -> u64 {
    intervals.saturating_add_signed(seconds.saturating_mul(INTERVALS_PER_SECOND))
}

unsafe extern "efiapi" fn get_local_time(time: *mut u8) {
    let now = now();
    let local = shifted(now, with_local_zone(|zone| zone.at(seconds_of(now)).offset));
    let calendar = calendar::system_time(local).unwrap_or_default();
    // SAFETY: the caller passes a writable `SYSTEMTIME` (16 bytes).
    unsafe { time.copy_from_nonoverlapping(calendar.to_bytes().as_ptr(), 16) };
}

unsafe extern "efiapi" fn file_time_to_local_file_time(from: *const u64, to: *mut u64) -> Bool {
    // SAFETY: the caller passes a readable `FILETIME`.
    let time = unsafe { from.read_unaligned() };
    let local = shifted(time, with_local_zone(|zone| zone.at(seconds_of(time)).offset));
    // SAFETY: and a writable one.
    unsafe { to.write_unaligned(local) };
    TRUE
}

unsafe extern "efiapi" fn local_file_time_to_file_time(from: *const u64, to: *mut u64) -> Bool {
    // SAFETY: the caller passes a readable `FILETIME`.
    let time = unsafe { from.read_unaligned() };
    let local = seconds_of(time);
    let utc = shifted(time, with_local_zone(|zone| zone.to_utc(local)) - local);
    // SAFETY: and a writable one.
    unsafe { to.write_unaligned(utc) };
    TRUE
}

fn transition_date(rule: &MonthRule) -> [u8; 16] {
    let fields = [
        0,
        u16::from(rule.month),
        u16::from(rule.weekday),
        u16::from(rule.week),
        (rule.seconds.rem_euclid(86_400) / 3600) as u16,
        (rule.seconds.rem_euclid(3600) / 60) as u16,
        (rule.seconds.rem_euclid(60)) as u16,
        0,
    ];
    let mut bytes = [0u8; 16];
    for (place, field) in bytes.as_chunks_mut::<2>().0.iter_mut().zip(fields) {
        *place = field.to_le_bytes();
    }
    bytes
}

fn put_name(bytes: &mut [u8], name: &str) {
    for (place, unit) in bytes
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(name.encode_utf16().take(31))
    {
        *place = unit.to_le_bytes();
    }
}

/// `TIME_ZONE_INFORMATION` for `rule` at `now`, and the zone id Windows answers with.
pub fn time_zone_information(rule: &Rule, now: i64) -> ([u8; TIME_ZONE_INFORMATION_BYTES], u32) {
    let mut bytes = [0u8; TIME_ZONE_INFORMATION_BYTES];
    let bias = -(rule.standard_offset() / 60) as i32;
    bytes[..4].copy_from_slice(&bias.to_le_bytes());
    put_name(&mut bytes[4..68], rule.standard_abbreviation().as_str());
    let Some(rules) = rule.month_rules() else {
        return (bytes, TIME_ZONE_ID_UNKNOWN);
    };
    bytes[68..84].copy_from_slice(&transition_date(&rules.end));
    put_name(&mut bytes[88..152], rules.daylight_abbreviation.as_str());
    bytes[152..168].copy_from_slice(&transition_date(&rules.start));
    let daylight_bias = -((rules.daylight_offset - rule.standard_offset()) / 60) as i32;
    bytes[168..172].copy_from_slice(&daylight_bias.to_le_bytes());
    let id = match rule.at(now).daylight {
        true => TIME_ZONE_ID_DAYLIGHT,
        false => TIME_ZONE_ID_STANDARD,
    };
    (bytes, id)
}

unsafe extern "efiapi" fn get_time_zone_information(information: *mut u8) -> u32 {
    let (bytes, id) =
        with_local_zone(|zone| time_zone_information(zone.rule(), seconds_of(now())));
    // SAFETY: the caller passes a writable `TIME_ZONE_INFORMATION` (172 bytes).
    unsafe { information.copy_from_nonoverlapping(bytes.as_ptr(), TIME_ZONE_INFORMATION_BYTES) };
    id
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
    if alertable != FALSE {
        return super::sync::sleep_alertably(milliseconds);
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
    b"GetLocalTime" => get_local_time,
    b"GetTimeZoneInformation" => get_time_zone_information,
    b"FileTimeToSystemTime" => file_time_to_system_time,
    b"SystemTimeToFileTime" => system_time_to_file_time,
    b"FileTimeToLocalFileTime" => file_time_to_local_file_time,
    b"LocalFileTimeToFileTime" => local_file_time_to_file_time,
    b"Sleep" => sleep,
    b"SleepEx" => sleep_ex,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shift_stays_within_a_file_time_s_range() {
        assert_eq!(shifted(0, -3600), 0);
        assert_eq!(shifted(5 * INTERVALS_PER_SECOND as u64, -3600), 0);
        assert_eq!(shifted(u64::MAX - 1, 3600), u64::MAX);
        assert_eq!(
            shifted(u64::MAX, -1),
            u64::MAX - INTERVALS_PER_SECOND as u64
        );
        assert_eq!(
            shifted(7200 * INTERVALS_PER_SECOND as u64, -3600),
            3600 * INTERVALS_PER_SECOND as u64
        );
    }
}

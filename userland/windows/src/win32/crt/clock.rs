//! The C runtime's calendar time (D579): `_tzset` and the zone it reads (`TZ`, else the
//! `time.zone` setting, with the zone's past when its TZif file is there), `_get_timezone`,
//! `_get_daylight`, `_get_dstbias` and `_get_tzname` with the UCRT's `__timezone`,
//! `__daylight`, `__dstbias` and `__tzname`, and `struct tm` both ways: `_localtime64`,
//! `_gmtime64`, their `_s` and 32-bit forms, `_mktime64` and `_mkgmtime64`. The non-`_s`
//! functions share one `struct tm` for the process, where Microsoft's keep one per thread.

use super::super::time::{reload_local_zone, with_local_zone};
use crate::library::Export;
use core::cell::UnsafeCell;
use eggos_calendar::{SECONDS_PER_DAY, civil_from_days, days_from_civil};

const TM_FIELDS: usize = 9;
const EINVAL: i32 = 22;
const ERANGE: i32 = 34;
const NAME_BYTES: usize = 64;
const LATEST_TIME64: i64 = 32_535_215_999;
const LATEST_TIME32: i64 = i32::MAX as i64;

struct Shared<T>(UnsafeCell<T>);

// SAFETY: the C runtime's contract for these globals: the program does not call the
// functions that change them from two threads at once, as with Microsoft's own.
unsafe impl<T> Sync for Shared<T> {}

static TM: Shared<[i32; TM_FIELDS]> = Shared(UnsafeCell::new([0; TM_FIELDS]));
static TIMEZONE: Shared<i32> = Shared(UnsafeCell::new(0));
static DAYLIGHT: Shared<i32> = Shared(UnsafeCell::new(0));
static DSTBIAS: Shared<i32> = Shared(UnsafeCell::new(-3600));
static NAMES: Shared<[[u8; NAME_BYTES]; 2]> = Shared(UnsafeCell::new([[0; NAME_BYTES]; 2]));
static NAME_POINTERS: Shared<[*mut u8; 2]> = Shared(UnsafeCell::new([core::ptr::null_mut(); 2]));

fn broken_down(seconds: i64, offset: i64, daylight: bool) -> Option<[i32; TM_FIELDS]> {
    let local = seconds.checked_add(offset)?;
    let days = local.div_euclid(SECONDS_PER_DAY);
    let rest = local.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let year_day = days - days_from_civil(year, 1, 1);
    Some([
        (rest % 60) as i32,
        (rest / 60 % 60) as i32,
        (rest / 3600) as i32,
        day as i32,
        month as i32 - 1,
        i32::try_from(year - 1900).ok()?,
        (days + 4).rem_euclid(7) as i32,
        year_day as i32,
        i32::from(daylight),
    ])
}

fn seconds_named(tm: &[i32; TM_FIELDS]) -> Option<i64> {
    let months = i64::from(tm[5]) * 12 + i64::from(tm[4]);
    let year = 1900 + months.div_euclid(12);
    let month = months.rem_euclid(12) as u32 + 1;
    let days = days_from_civil(year, month, 1).checked_add(i64::from(tm[3]) - 1)?;
    days.checked_mul(SECONDS_PER_DAY)?
        .checked_add(i64::from(tm[2]) * 3600 + i64::from(tm[1]) * 60 + i64::from(tm[0]))
}

fn local(seconds: i64) -> Option<[i32; TM_FIELDS]> {
    let local = with_local_zone(|zone| zone.at(seconds));
    broken_down(seconds, local.offset, local.daylight)
}

fn store(tm: *mut i32, fields: [i32; TM_FIELDS]) {
    for (index, value) in fields.into_iter().enumerate() {
        // SAFETY: callers pass a writable `struct tm`, nine `int`s.
        unsafe { tm.add(index).write_unaligned(value) };
    }
}

fn load(tm: *const i32) -> [i32; TM_FIELDS] {
    // SAFETY: callers pass a readable `struct tm`, nine `int`s.
    core::array::from_fn(|index| unsafe { tm.add(index).read_unaligned() })
}

fn shared_tm(fields: Option<[i32; TM_FIELDS]>) -> *mut i32 {
    let Some(fields) = fields else {
        return core::ptr::null_mut();
    };
    let tm = TM.0.get().cast::<i32>();
    store(tm, fields);
    tm
}

fn update_globals() {
    with_local_zone(|zone| {
        let rule = zone.rule();
        let names = NAMES.0.get();
        // SAFETY: see `Shared`; the names are written here only.
        let names = unsafe { &mut *names };
        *names = [[0; NAME_BYTES]; 2];
        let standard = rule.standard_abbreviation();
        let standard = standard.as_str().as_bytes();
        names[0][..standard.len()].copy_from_slice(standard);
        let daylight = rule.month_rules();
        if let Some(rules) = &daylight {
            let name = rules.daylight_abbreviation.as_str().as_bytes();
            names[1][..name.len()].copy_from_slice(name);
        }
        // SAFETY: see `Shared`.
        unsafe {
            *TIMEZONE.0.get() = -(rule.standard_offset() as i32);
            *DAYLIGHT.0.get() = i32::from(rule.has_daylight());
            *DSTBIAS.0.get() = daylight.map_or(-3600, |rules| {
                -((rules.daylight_offset - rule.standard_offset()) as i32)
            });
            *NAME_POINTERS.0.get() = [names[0].as_mut_ptr(), names[1].as_mut_ptr()];
        }
    });
}

extern "efiapi" fn tzset() {
    reload_local_zone();
    update_globals();
}

unsafe extern "efiapi" fn get_timezone(seconds: *mut i32) -> i32 {
    if seconds.is_null() {
        return EINVAL;
    }
    update_globals();
    // SAFETY: a writable `long`; see `Shared`.
    unsafe { seconds.write_unaligned(*TIMEZONE.0.get()) };
    0
}

unsafe extern "efiapi" fn get_daylight(hours: *mut i32) -> i32 {
    if hours.is_null() {
        return EINVAL;
    }
    update_globals();
    // SAFETY: a writable `int`; see `Shared`.
    unsafe { hours.write_unaligned(*DAYLIGHT.0.get()) };
    0
}

unsafe extern "efiapi" fn get_dstbias(seconds: *mut i32) -> i32 {
    if seconds.is_null() {
        return EINVAL;
    }
    update_globals();
    // SAFETY: a writable `long`; see `Shared`.
    unsafe { seconds.write_unaligned(*DSTBIAS.0.get()) };
    0
}

unsafe extern "efiapi" fn get_tzname(returned: *mut usize, buffer: *mut u8, size: usize, index: i32) -> i32 {
    if !(0..=1).contains(&index) || (buffer.is_null() && size != 0) {
        return EINVAL;
    }
    update_globals();
    // SAFETY: see `Shared`.
    let name = unsafe { &(*NAMES.0.get())[index as usize] };
    let length = name.iter().position(|&byte| byte == 0).unwrap_or(NAME_BYTES) + 1;
    if !returned.is_null() {
        // SAFETY: a writable `size_t`.
        unsafe { returned.write_unaligned(length) };
    }
    if buffer.is_null() {
        return 0;
    }
    if size < length {
        return ERANGE;
    }
    // SAFETY: `buffer` holds `size` bytes, at least `length`.
    unsafe { buffer.copy_from_nonoverlapping(name.as_ptr(), length) };
    0
}

extern "efiapi" fn timezone_address() -> *mut i32 {
    update_globals();
    TIMEZONE.0.get()
}

extern "efiapi" fn daylight_address() -> *mut i32 {
    update_globals();
    DAYLIGHT.0.get()
}

extern "efiapi" fn dstbias_address() -> *mut i32 {
    update_globals();
    DSTBIAS.0.get()
}

extern "efiapi" fn tzname_address() -> *mut *mut u8 {
    update_globals();
    NAME_POINTERS.0.get().cast::<*mut u8>()
}

fn read_time(time: *const i64, latest: i64) -> Option<i64> {
    if time.is_null() {
        return None;
    }
    // SAFETY: a readable `__time64_t`.
    let seconds = unsafe { time.read_unaligned() };
    (0..=latest).contains(&seconds).then_some(seconds)
}

fn read_time32(time: *const i32) -> Option<i64> {
    if time.is_null() {
        return None;
    }
    // SAFETY: a readable `__time32_t`.
    let seconds = i64::from(unsafe { time.read_unaligned() });
    (0..=LATEST_TIME32).contains(&seconds).then_some(seconds)
}

unsafe extern "efiapi" fn localtime64(time: *const i64) -> *mut i32 {
    shared_tm(read_time(time, LATEST_TIME64).and_then(local))
}

unsafe extern "efiapi" fn gmtime64(time: *const i64) -> *mut i32 {
    shared_tm(read_time(time, LATEST_TIME64).and_then(|seconds| broken_down(seconds, 0, false)))
}

unsafe extern "efiapi" fn localtime32(time: *const i32) -> *mut i32 {
    shared_tm(read_time32(time).and_then(local))
}

unsafe extern "efiapi" fn gmtime32(time: *const i32) -> *mut i32 {
    shared_tm(read_time32(time).and_then(|seconds| broken_down(seconds, 0, false)))
}

fn into_tm(tm: *mut i32, fields: Option<[i32; TM_FIELDS]>) -> i32 {
    if tm.is_null() {
        return EINVAL;
    }
    match fields {
        Some(fields) => {
            store(tm, fields);
            0
        }
        None => {
            store(tm, [-1; TM_FIELDS]);
            EINVAL
        }
    }
}

unsafe extern "efiapi" fn localtime64_s(tm: *mut i32, time: *const i64) -> i32 {
    into_tm(tm, read_time(time, LATEST_TIME64).and_then(local))
}

unsafe extern "efiapi" fn gmtime64_s(tm: *mut i32, time: *const i64) -> i32 {
    into_tm(tm, read_time(time, LATEST_TIME64).and_then(|seconds| broken_down(seconds, 0, false)))
}

unsafe extern "efiapi" fn localtime32_s(tm: *mut i32, time: *const i32) -> i32 {
    into_tm(tm, read_time32(time).and_then(local))
}

unsafe extern "efiapi" fn gmtime32_s(tm: *mut i32, time: *const i32) -> i32 {
    into_tm(tm, read_time32(time).and_then(|seconds| broken_down(seconds, 0, false)))
}

fn make_time(tm: *mut i32, local_zone: bool, latest: i64) -> i64 {
    if tm.is_null() {
        return -1;
    }
    let Some(named) = seconds_named(&load(tm)) else {
        return -1;
    };
    let seconds = match local_zone {
        true => with_local_zone(|zone| zone.to_utc(named)),
        false => named,
    };
    if !(0..=latest).contains(&seconds) {
        return -1;
    }
    let fields = match local_zone {
        true => local(seconds),
        false => broken_down(seconds, 0, false),
    };
    match fields {
        Some(fields) => {
            store(tm, fields);
            seconds
        }
        None => -1,
    }
}

unsafe extern "efiapi" fn mktime64(tm: *mut i32) -> i64 {
    make_time(tm, true, LATEST_TIME64)
}

unsafe extern "efiapi" fn mkgmtime64(tm: *mut i32) -> i64 {
    make_time(tm, false, LATEST_TIME64)
}

unsafe extern "efiapi" fn mktime32(tm: *mut i32) -> i32 {
    make_time(tm, true, LATEST_TIME32) as i32
}

unsafe extern "efiapi" fn mkgmtime32(tm: *mut i32) -> i32 {
    make_time(tm, false, LATEST_TIME32) as i32
}

pub(super) const EXPORTS: &[Export] = exports![
    b"_tzset" => tzset,
    b"_get_timezone" => get_timezone,
    b"_get_daylight" => get_daylight,
    b"_get_dstbias" => get_dstbias,
    b"_get_tzname" => get_tzname,
    b"__timezone" => timezone_address,
    b"__daylight" => daylight_address,
    b"__dstbias" => dstbias_address,
    b"__tzname" => tzname_address,
    b"_localtime64" => localtime64,
    b"_localtime64_s" => localtime64_s,
    b"_localtime32" => localtime32,
    b"_localtime32_s" => localtime32_s,
    b"_gmtime64" => gmtime64,
    b"_gmtime64_s" => gmtime64_s,
    b"_gmtime32" => gmtime32,
    b"_gmtime32_s" => gmtime32_s,
    b"_mktime64" => mktime64,
    b"_mkgmtime64" => mkgmtime64,
    b"_mktime32" => mktime32,
    b"_mkgmtime32" => mkgmtime32,
];

/// The old C runtime's names for the 64-bit forms (`time_t` is 64 bits on x64).
pub(super) const MSVCRT_EXPORTS: &[Export] = exports![
    b"localtime" => localtime64,
    b"gmtime" => gmtime64,
    b"mktime" => mktime64,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_breaks_down_and_comes_back() {
        let tm = broken_down(1_700_000_000, 3600, false).unwrap();
        assert_eq!(tm, [20, 13, 23, 14, 10, 123, 2, 317, 0]);
        assert_eq!(seconds_named(&tm), Some(1_700_000_000 + 3600));
    }

    #[test]
    fn out_of_range_fields_are_normalised() {
        let tm = [0, 0, 0, 32, 11, 99, 0, 0, 0];
        assert_eq!(seconds_named(&tm), Some(946_684_800));
        let tm = [-1, 0, 0, 1, 0, 70, 0, 0, 0];
        assert_eq!(seconds_named(&tm), Some(-1));
    }
}

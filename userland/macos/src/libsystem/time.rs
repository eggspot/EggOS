//! Calendar time: `gmtime_r` and `timegm` in UTC, `localtime_r`, `mktime` and `tzset` in the
//! zone `TZ` names or else the `time.zone` setting (D194), and `strftime` and `strptime` in the
//! `LC_TIME` locale, on Darwin's `struct tm`.

use super::{Exclusive, set_errno};
use crate::calendar::{self, BrokenDown};
use core::ffi::{c_char, c_int, c_long};
use eggos_calendar::zone::{self, Local, Rule, UTC};

const EOVERFLOW: c_int = 84;

/// Darwin's `struct tm`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Tm {
    second: c_int,
    minute: c_int,
    hour: c_int,
    day: c_int,
    month: c_int,
    year: c_int,
    weekday: c_int,
    year_day: c_int,
    daylight_saving: c_int,
    offset: c_long,
    zone: *const c_char,
}

impl Tm {
    fn from(time: BrokenDown) -> Self {
        Self {
            second: time.second,
            minute: time.minute,
            hour: time.hour,
            day: time.day,
            month: time.month,
            year: time.year,
            weekday: time.weekday,
            year_day: time.year_day,
            daylight_saving: 0,
            offset: 0,
            zone: c"UTC".as_ptr(),
        }
    }

    fn with_broken_down(self, time: BrokenDown) -> Self {
        Self {
            second: time.second,
            minute: time.minute,
            hour: time.hour,
            day: time.day,
            month: time.month,
            year: time.year,
            weekday: time.weekday,
            year_day: time.year_day,
            ..self
        }
    }

    fn broken_down(&self) -> BrokenDown {
        BrokenDown {
            second: self.second,
            minute: self.minute,
            hour: self.hour,
            day: self.day,
            month: self.month,
            year: self.year,
            weekday: self.weekday,
            year_day: self.year_day,
        }
    }
}

static mut SHARED: Tm = Tm {
    second: 0,
    minute: 0,
    hour: 0,
    day: 0,
    month: 0,
    year: 0,
    weekday: 0,
    year_day: 0,
    daylight_saving: 0,
    offset: 0,
    zone: core::ptr::null(),
};

/// # Safety
/// `seconds` must be readable and `out` writable.
pub unsafe extern "C" fn gmtime_r(seconds: *const i64, out: *mut Tm) -> *mut Tm {
    // SAFETY: the caller's promise.
    let Some(time) = calendar::from_seconds(unsafe { seconds.read() }) else {
        set_errno(EOVERFLOW);
        return core::ptr::null_mut();
    };
    // SAFETY: as above.
    unsafe { out.write(Tm::from(time)) };
    out
}

/// # Safety
/// `seconds` must be readable.
pub unsafe extern "C" fn gmtime(seconds: *const i64) -> *mut Tm {
    // SAFETY: the caller's promise; `gmtime` returns a shared block it may overwrite.
    unsafe { gmtime_r(seconds, &raw mut SHARED) }
}

/// # Safety
/// `time` must be readable and writable.
pub unsafe extern "C" fn timegm(time: *mut Tm) -> i64 {
    // SAFETY: the caller's promise.
    let seconds = calendar::to_seconds(&unsafe { time.read() }.broken_down());
    match calendar::from_seconds(seconds) {
        Some(normalized) => {
            // SAFETY: as above.
            unsafe { time.write(Tm::from(normalized)) };
            seconds
        }
        None => {
            set_errno(EOVERFLOW);
            -1
        }
    }
}

/// # Safety
/// `buffer` must be writable for `size` bytes, `format` NUL-terminated and `time` readable.
pub unsafe extern "C" fn strftime(
    buffer: *mut c_char,
    size: usize,
    format: *const c_char,
    time: *const Tm,
) -> usize {
    if size == 0 {
        return 0;
    }
    // SAFETY: the caller's promise.
    let format = unsafe { super::c_string(format) };
    // SAFETY: as above.
    let time = unsafe { time.read() }.broken_down();
    // SAFETY: as above.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer.cast::<u8>(), size) };
    match calendar::format_in(super::locale::time_formats(), format, &time, out) {
        Some(length) => {
            out[length] = 0;
            length
        }
        None => {
            out[0] = 0;
            0
        }
    }
}

const SETTINGS: &[u8] = b"/home/root/.config/eggos/settings\0";
const SETTINGS_BYTES: usize = 4096;
const NAME_BYTES: usize = zone::MAX_ABBREVIATION_BYTES + 1;

struct LoadedZone {
    rule: Rule,
    names: [[u8; NAME_BYTES]; 2],
}

static ZONE: Exclusive<Option<LoadedZone>> = Exclusive::new(None);

fn zone_named_by_settings() -> Option<Rule> {
    let descriptor = crate::linux::open(SETTINGS, 0, 0).ok()?;
    let mut file = [0u8; SETTINGS_BYTES];
    let mut length = 0;
    while let Some(free) = file.get_mut(length..).filter(|free| !free.is_empty()) {
        match crate::linux::read(descriptor, free) {
            Ok(0) | Err(_) => break,
            Ok(count) => length += count,
        }
    }
    let _ = crate::linux::close(descriptor);
    let name = eggos_settings_file::value(file.get(..length)?, eggos_settings_file::TIME_ZONE)?;
    zone::zone(core::str::from_utf8(name).ok()?)
}

fn zone_now() -> Rule {
    // SAFETY: `getenv` reads the NUL-terminated name and returns null or a NUL-terminated value.
    let variable = unsafe { super::process::getenv(c"TZ".as_ptr()) };
    if variable.is_null() {
        return zone_named_by_settings().unwrap_or(UTC);
    }
    // SAFETY: as above.
    let text = unsafe { super::c_string(variable) };
    core::str::from_utf8(text)
        .ok()
        .filter(|text| !text.is_empty())
        .and_then(zone::zone)
        .unwrap_or(UTC)
}

fn loaded<R>(work: impl FnOnce(&LoadedZone) -> R) -> R {
    ZONE.with(|zone| {
        let loaded = zone.get_or_insert_with(|| LoadedZone {
            rule: zone_now(),
            names: [[0; NAME_BYTES]; 2],
        });
        work(loaded)
    })
}

fn local_tm(seconds: i64, loaded: &mut LoadedZone) -> Option<Tm> {
    let local: Local = loaded.rule.at(seconds);
    let time = calendar::from_seconds(seconds.checked_add(local.offset)?)?;
    let slot = usize::from(local.daylight);
    let name = local.abbreviation.as_str().as_bytes();
    loaded.names[slot] = [0; NAME_BYTES];
    loaded.names[slot][..name.len()].copy_from_slice(name);
    Some(Tm {
        daylight_saving: c_int::from(local.daylight),
        offset: local.offset as c_long,
        zone: loaded.names[slot].as_ptr().cast(),
        ..Tm::from(time)
    })
}

/// # Safety
/// `seconds` must be readable and `out` writable.
pub unsafe extern "C" fn localtime_r(seconds: *const i64, out: *mut Tm) -> *mut Tm {
    // SAFETY: the caller's promise.
    let seconds = unsafe { seconds.read() };
    let shown = ZONE.with(|zone| {
        let loaded = zone.get_or_insert_with(|| LoadedZone {
            rule: zone_now(),
            names: [[0; NAME_BYTES]; 2],
        });
        local_tm(seconds, loaded)
    });
    let Some(shown) = shown else {
        set_errno(EOVERFLOW);
        return core::ptr::null_mut();
    };
    // SAFETY: as above.
    unsafe { out.write(shown) };
    out
}

/// # Safety
/// `seconds` must be readable.
pub unsafe extern "C" fn localtime(seconds: *const i64) -> *mut Tm {
    // SAFETY: the caller's promise; `localtime` returns a shared block it may overwrite.
    unsafe { localtime_r(seconds, &raw mut SHARED) }
}

/// # Safety
/// `time` must be readable and writable.
pub unsafe extern "C" fn mktime(time: *mut Tm) -> i64 {
    // SAFETY: the caller's promise.
    let local = calendar::to_seconds(&unsafe { time.read() }.broken_down());
    let seconds = loaded(|loaded| loaded.rule.to_utc(local));
    // SAFETY: as above; `localtime_r` writes the normalised local time back.
    match unsafe { localtime_r(&seconds, time) }.is_null() {
        true => -1,
        false => seconds,
    }
}

pub extern "C" fn tzset() {
    let rule = zone_now();
    ZONE.with(|zone| {
        *zone = Some(LoadedZone {
            rule,
            names: [[0; NAME_BYTES]; 2],
        })
    });
}

/// # Safety
/// `text` and `format` must be NUL-terminated and `time` readable and writable.
pub unsafe extern "C" fn strptime(
    text: *const c_char,
    format: *const c_char,
    time: *mut Tm,
) -> *mut c_char {
    // SAFETY: the caller's promise.
    let input = unsafe { super::c_string(text) };
    // SAFETY: as above.
    let format = unsafe { super::c_string(format) };
    // SAFETY: as above.
    let mut result = unsafe { time.read() };
    let mut broken_down = result.broken_down();
    let formats = super::locale::time_formats();
    let Some(parsed) = calendar::parse(input, format, formats, &mut broken_down) else {
        return core::ptr::null_mut();
    };
    result = result.with_broken_down(broken_down);
    if parsed.standard_time {
        result.daylight_saving = 0;
    }
    if let Some(offset) = parsed.offset {
        result.offset = offset as c_long;
    }
    // SAFETY: as above.
    unsafe { time.write(result) };
    text.wrapping_add(parsed.used).cast_mut()
}

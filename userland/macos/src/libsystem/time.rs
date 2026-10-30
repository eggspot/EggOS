//! Calendar time in UTC (the only time zone yet): `gmtime_r`, `localtime_r`, `timegm`,
//! `mktime`, and `strftime` and `strptime` in the `LC_TIME` locale, on Darwin's `struct tm`.

use super::set_errno;
use crate::calendar::{self, BrokenDown};
use core::ffi::{c_char, c_int, c_long};

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

pub extern "C" fn tzset() {}

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

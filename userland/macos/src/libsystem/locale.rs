//! Locales: only the C locale exists. `setlocale` accepts `"C"`, `"POSIX"` and `""` (the
//! environment's locale, which is C) and refuses others with a null pointer; `nl_langinfo`
//! and `localeconv` answer for the C locale; one byte is the longest character, which
//! `MB_CUR_MAX` reads from `__mb_cur_max`, a variable (`___mb_cur_max()` is the function the
//! extended-locale headers call).

use super::c_string;
use core::ffi::{c_char, c_int};

const CHAR_MAX: c_char = 127;
const ITEM_COUNT: usize = 58;

/// Darwin's `struct lconv`.
#[repr(C)]
#[derive(Debug)]
pub struct Conventions {
    strings: [*const c_char; 10],
    numbers: [c_char; 14],
}

// SAFETY: the conventions are constant pointers to constant strings.
unsafe impl Sync for Conventions {}

static C_CONVENTIONS: Conventions = Conventions {
    strings: [
        c".".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
        c"".as_ptr(),
    ],
    numbers: [CHAR_MAX; 14],
};

/// # Safety
/// `locale` must be null or NUL-terminated.
pub unsafe extern "C" fn setlocale(category: c_int, locale: *const c_char) -> *const c_char {
    if !(0..=6).contains(&category) {
        return core::ptr::null();
    }
    if locale.is_null() {
        return c"C".as_ptr();
    }
    // SAFETY: the caller's promise.
    match unsafe { c_string(locale) } {
        b"" | b"C" | b"POSIX" => c"C".as_ptr(),
        _ => core::ptr::null(),
    }
}

fn item(item: usize) -> &'static core::ffi::CStr {
    const DAYS: [&core::ffi::CStr; 7] = [
        c"Sunday",
        c"Monday",
        c"Tuesday",
        c"Wednesday",
        c"Thursday",
        c"Friday",
        c"Saturday",
    ];
    const SHORT_DAYS: [&core::ffi::CStr; 7] =
        [c"Sun", c"Mon", c"Tue", c"Wed", c"Thu", c"Fri", c"Sat"];
    const MONTHS: [&core::ffi::CStr; 12] = [
        c"January",
        c"February",
        c"March",
        c"April",
        c"May",
        c"June",
        c"July",
        c"August",
        c"September",
        c"October",
        c"November",
        c"December",
    ];
    const SHORT_MONTHS: [&core::ffi::CStr; 12] = [
        c"Jan", c"Feb", c"Mar", c"Apr", c"May", c"Jun", c"Jul", c"Aug", c"Sep", c"Oct", c"Nov",
        c"Dec",
    ];
    match item {
        0 => c"US-ASCII",
        1 => c"%a %b %e %H:%M:%S %Y",
        2 => c"%m/%d/%y",
        3 => c"%H:%M:%S",
        4 => c"%I:%M:%S %p",
        5 => c"AM",
        6 => c"PM",
        7..=13 => DAYS[item - 7],
        14..=20 => SHORT_DAYS[item - 14],
        21..=32 => MONTHS[item - 21],
        33..=44 => SHORT_MONTHS[item - 33],
        50 => c".",
        52 => c"^[yY]",
        53 => c"^[nN]",
        57 => c"md",
        _ => c"",
    }
}

pub extern "C" fn nl_langinfo(which: c_int) -> *const c_char {
    let which = usize::try_from(which).unwrap_or(ITEM_COUNT);
    item(which).as_ptr()
}

pub extern "C" fn localeconv() -> *const Conventions {
    &raw const C_CONVENTIONS
}

/// `__mb_cur_max`: the longest character in bytes.
pub static MB_CUR_MAX: c_int = 1;

pub extern "C" fn mb_cur_max() -> c_int {
    MB_CUR_MAX
}

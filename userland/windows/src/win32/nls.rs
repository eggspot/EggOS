//! National language support (D178): the code page every program gets (UTF-8, 65001, D118),
//! dates and times formatted for `en-US` or the invariant locale (`crate::locale`), character
//! types and `LCMapStringW`'s case mappings, byte reversal and sort keys, which order as the
//! layer's comparisons do (by unit, case folded when asked). Other locales are not there yet:
//! asking for one is `ERROR_INVALID_PARAMETER`.

use super::{Bool, FALSE, TRUE, error, fail};
use crate::calendar::{self, SystemTime};
use crate::library::Export;
use crate::linux;
use crate::locale::{self, Locale, Standard, character_type, time_flags};
use crate::text;

const CP_ACP: u32 = 0;
const CP_OEMCP: u32 = 1;
const CP_THREAD_ACP: u32 = 3;
const CP_UTF8: u32 = 65001;
const UTF8_LONGEST_CHARACTER: u32 = 4;
const LOCALE_NEUTRAL: u32 = 0x0000;
const LOCALE_INVARIANT: u32 = 0x007f;
const LOCALE_USER_DEFAULT: u32 = 0x0400;
const LOCALE_SYSTEM_DEFAULT: u32 = 0x0800;
const LOCALE_CUSTOM_DEFAULT: u32 = 0x0c00;
const LOCALE_NOUSEROVERRIDE: u32 = 0x8000_0000;
const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;
const DATE_SHORTDATE: u32 = 0x1;
const DATE_LONGDATE: u32 = 0x2;
const DATE_YEARMONTH: u32 = 0x8;
const DATE_MONTHDAY: u32 = 0x80;
const DATE_READING_ORDER: u32 = 0x10 | 0x20 | 0x40;
const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;
const LCMAP_TITLECASE: u32 = 0x300;
const LCMAP_SORTKEY: u32 = 0x400;
const LCMAP_BYTEREV: u32 = 0x800;
const LCMAP_LINGUISTIC_CASING: u32 = 0x0100_0000;
const LCMAP_UNSUPPORTED: u32 = 0x0010_0000 | 0x0020_0000 | 0x0040_0000 | 0x0080_0000 | 0x0600_0000;
const NORM_IGNORECASE: u32 = 0x1;
const NORM_FLAGS: u32 = 0x0003_1007;
const PICTURE_CAPACITY: usize = 32;

fn locale_of_id(id: u32) -> Option<Locale> {
    match id & 0xffff {
        LOCALE_NEUTRAL | LOCALE_USER_DEFAULT | LOCALE_SYSTEM_DEFAULT | LOCALE_CUSTOM_DEFAULT => {
            Some(Locale::UnitedStates)
        }
        locale::UNITED_STATES => Some(Locale::UnitedStates),
        LOCALE_INVARIANT => Some(Locale::Invariant),
        _ => None,
    }
}

unsafe fn locale_of_name(name: *const u16) -> Option<Locale> {
    if name.is_null() {
        return Some(Locale::UnitedStates);
    }
    // SAFETY: the callers' contract: a NUL-terminated name.
    let name = unsafe { super::file::wide_string(name) };
    let is = |wanted: &str| {
        name.len() == wanted.len()
            && name
                .iter()
                .zip(wanted.bytes())
                .all(|(unit, byte)| text::upper(*unit) == u16::from(byte.to_ascii_uppercase()))
    };
    match name {
        [] => Some(Locale::Invariant),
        _ if is("en-US") || is("!x-sys-default-locale") => Some(Locale::UnitedStates),
        _ => None,
    }
}

fn is_utf8(code_page: u32) -> bool {
    matches!(code_page, CP_ACP | CP_OEMCP | CP_THREAD_ACP | CP_UTF8)
}

unsafe extern "efiapi" fn get_cp_info(code_page: u32, info: *mut u8) -> Bool {
    if !is_utf8(code_page) || info.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&UTF8_LONGEST_CHARACTER.to_le_bytes());
    out[4] = b'?';
    // SAFETY: the caller passes a writable `CPINFO`: the longest character, the default
    // character's two bytes and twelve lead bytes, none for UTF-8.
    unsafe { info.copy_from_nonoverlapping(out.as_ptr(), out.len()) };
    TRUE
}

extern "efiapi" fn is_valid_code_page(code_page: u32) -> Bool {
    Bool::from(code_page == CP_UTF8)
}

extern "efiapi" fn get_system_default_ui_language() -> u16 {
    locale::UNITED_STATES as u16
}

unsafe fn source<'a>(units: *const u16, count: i32) -> Option<&'a [u16]> {
    if units.is_null() {
        return None;
    }
    let length = match count {
        // SAFETY: the callers' contract: with -1, a NUL-terminated string, the NUL included.
        -1 => unsafe { super::file::wide_string(units) }.len() + 1,
        1.. => count as usize,
        _ => return None,
    };
    // SAFETY: as above, or `count` readable units.
    Some(unsafe { core::slice::from_raw_parts(units, length) })
}

unsafe extern "efiapi" fn get_string_type_w(
    information: u32,
    units: *const u16,
    count: i32,
    types: *mut u16,
) -> Bool {
    // SAFETY: `GetStringTypeW`'s contract: `count` units, or a NUL-terminated string.
    let Some(units) = (unsafe { source(units, count) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    if types.is_null() || character_type(information, 0).is_none() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    for (at, unit) in units.iter().enumerate() {
        let bits = character_type(information, *unit).unwrap_or(0);
        // SAFETY: the caller passes one writable `WORD` per unit.
        unsafe { types.wrapping_add(at).write(bits) };
    }
    TRUE
}

unsafe extern "efiapi" fn get_string_type_ex_w(
    _locale: u32,
    information: u32,
    units: *const u16,
    count: i32,
    types: *mut u16,
) -> Bool {
    // SAFETY: the same contract as `GetStringTypeW`'s.
    unsafe { get_string_type_w(information, units, count, types) }
}

fn mapped(unit: u16, previous: Option<u16>, case: u32) -> u16 {
    match case {
        LCMAP_LOWERCASE => text::lower(unit),
        LCMAP_UPPERCASE => text::upper(unit),
        LCMAP_TITLECASE => {
            let letter =
                |unit: u16| char::from_u32(u32::from(unit)).is_some_and(char::is_alphabetic);
            match previous.is_some_and(letter) {
                true => text::lower(unit),
                false => text::upper(unit),
            }
        }
        _ => unit,
    }
}

unsafe extern "efiapi" fn lc_map_string_w(
    locale_id: u32,
    flags: u32,
    units: *const u16,
    count: i32,
    out: *mut u16,
    out_count: i32,
) -> i32 {
    if locale_of_id(locale_id).is_none() {
        return fail(error::INVALID_PARAMETER, 0);
    }
    // SAFETY: `LCMapStringW`'s contract: `count` units, or a NUL-terminated string.
    let Some(units) = (unsafe { source(units, count) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    if out_count < 0 || (out_count > 0 && out.is_null()) {
        return fail(error::INVALID_PARAMETER, 0);
    }
    if flags & LCMAP_UNSUPPORTED != 0 {
        return fail(error::NOT_SUPPORTED, 0);
    }
    if flags & LCMAP_SORTKEY != 0 {
        if flags & !(LCMAP_SORTKEY | NORM_FLAGS) != 0 {
            return fail(error::INVALID_FLAGS, 0);
        }
        let units = match count {
            -1 => &units[..units.len() - 1],
            _ => units,
        };
        let needed = units.len() * 3 + 1;
        if out_count == 0 {
            return needed as i32;
        }
        if needed > out_count as usize {
            return fail(error::INSUFFICIENT_BUFFER, 0);
        }
        // SAFETY: with `LCMAP_SORTKEY`, `out` is `out_count` writable bytes, at least `needed`.
        let bytes = unsafe { core::slice::from_raw_parts_mut(out.cast::<u8>(), needed) };
        for (key, unit) in bytes.as_chunks_mut::<3>().0.iter_mut().zip(units) {
            let unit = match flags & NORM_IGNORECASE != 0 {
                true => text::upper(*unit),
                false => *unit,
            };
            *key = locale::sort_key_bytes(unit);
        }
        bytes[needed - 1] = 0;
        return needed as i32;
    }
    let case = flags & LCMAP_TITLECASE;
    if flags & !(LCMAP_TITLECASE | LCMAP_BYTEREV | LCMAP_LINGUISTIC_CASING) != 0
        || (case == 0 && flags & LCMAP_BYTEREV == 0)
    {
        return fail(error::INVALID_FLAGS, 0);
    }
    if out_count == 0 {
        return units.len() as i32;
    }
    if units.len() > out_count as usize {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    let mut previous = None;
    for (at, unit) in units.iter().enumerate() {
        let mut value = mapped(*unit, previous, case);
        previous = Some(*unit);
        if flags & LCMAP_BYTEREV != 0 {
            value = value.swap_bytes();
        }
        // SAFETY: the caller passes `out_count` writable units, at least as many as the source;
        // `out` may be `units` itself, which is read before it is written.
        unsafe { out.wrapping_add(at).write(value) };
    }
    units.len() as i32
}

fn now() -> SystemTime {
    let nanoseconds = linux::clock_nanoseconds(linux::CLOCK_REALTIME);
    let file_time =
        nanoseconds / 100 + calendar::UNIX_EPOCH_SECONDS * calendar::INTERVALS_PER_SECOND;
    calendar::system_time(file_time).unwrap_or_default()
}

unsafe fn date_of(date: *const u8) -> Option<SystemTime> {
    if date.is_null() {
        return Some(now());
    }
    let mut bytes = [0u8; 16];
    // SAFETY: the callers' contract: a readable `SYSTEMTIME`.
    unsafe { bytes.as_mut_ptr().copy_from_nonoverlapping(date, 16) };
    let given = SystemTime::from_bytes(bytes);
    calendar::system_time(calendar::file_time(&given)?)
}

unsafe fn time_of(time: *const u8) -> Option<SystemTime> {
    if time.is_null() {
        return Some(now());
    }
    let mut bytes = [0u8; 16];
    // SAFETY: the callers' contract: a readable `SYSTEMTIME`.
    unsafe { bytes.as_mut_ptr().copy_from_nonoverlapping(time, 16) };
    let given = SystemTime::from_bytes(bytes);
    (given.hour < 24 && given.minute < 60 && given.second < 60 && given.milliseconds < 1000)
        .then_some(given)
}

unsafe fn deliver(buffer: *mut u16, size: i32, produce: impl Fn(&mut dyn FnMut(char))) -> i32 {
    if size < 0 || (size > 0 && buffer.is_null()) {
        return fail(error::INVALID_PARAMETER, 0);
    }
    let mut needed = 1usize;
    produce(&mut |c| needed += c.len_utf16());
    if size == 0 {
        return needed as i32;
    }
    if needed > size as usize {
        return fail(error::INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller passes `size` writable units, at least `needed`.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, needed) };
    let mut at = 0;
    produce(&mut |c| at += c.encode_utf16(&mut out[at..]).len());
    out[at] = 0;
    needed as i32
}

unsafe fn picture<'a>(
    format: *const u16,
    standard: &str,
    buffer: &'a mut [u16; PICTURE_CAPACITY],
) -> &'a [u16] {
    if !format.is_null() {
        // SAFETY: the callers' contract: a NUL-terminated picture.
        return unsafe { super::file::wide_string(format) };
    }
    let mut length = 0;
    for unit in standard.encode_utf16() {
        buffer[length] = unit;
        length += 1;
    }
    &buffer[..length]
}

unsafe fn format_date(
    locale: Option<Locale>,
    flags: u32,
    date: *const u8,
    format: *const u16,
    buffer: *mut u16,
    size: i32,
) -> i32 {
    let Some(locale) = locale else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let kinds = [
        (DATE_SHORTDATE, Standard::ShortDate),
        (DATE_LONGDATE, Standard::LongDate),
        (DATE_YEARMONTH, Standard::YearMonth),
        (DATE_MONTHDAY, Standard::MonthDay),
    ];
    let chosen = kinds.iter().filter(|(flag, _)| flags & flag != 0).count();
    let known = DATE_SHORTDATE
        | DATE_LONGDATE
        | DATE_YEARMONTH
        | DATE_MONTHDAY
        | DATE_READING_ORDER
        | LOCALE_NOUSEROVERRIDE
        | LOCALE_USE_CP_ACP;
    if flags & !known != 0 || chosen > 1 || (chosen == 1 && !format.is_null()) {
        return fail(error::INVALID_FLAGS, 0);
    }
    let standard = kinds
        .iter()
        .find(|(flag, _)| flags & flag != 0)
        .map_or(Standard::ShortDate, |(_, standard)| *standard);
    // SAFETY: the callers' contract: a readable `SYSTEMTIME`, or null.
    let Some(date) = (unsafe { date_of(date) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let mut standard_units = [0u16; PICTURE_CAPACITY];
    // SAFETY: as above: a NUL-terminated picture, or null.
    let picture = unsafe { picture(format, locale.picture(standard), &mut standard_units) };
    // SAFETY: the callers' contract: `size` writable units at `buffer`.
    unsafe {
        deliver(buffer, size, |out| {
            locale::format_date(text::utf16_chars(picture), &date, &mut |c| out(c))
        })
    }
}

unsafe extern "efiapi" fn get_date_format_w(
    locale_id: u32,
    flags: u32,
    date: *const u8,
    format: *const u16,
    buffer: *mut u16,
    size: i32,
) -> i32 {
    // SAFETY: `GetDateFormatW`'s contract.
    unsafe { format_date(locale_of_id(locale_id), flags, date, format, buffer, size) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "efiapi" fn get_date_format_ex(
    locale_name: *const u16,
    flags: u32,
    date: *const u8,
    format: *const u16,
    buffer: *mut u16,
    size: i32,
    _calendar: *const u16,
) -> i32 {
    // SAFETY: `GetDateFormatEx`'s contract: a NUL-terminated locale name, or null.
    let locale = unsafe { locale_of_name(locale_name) };
    // SAFETY: `GetDateFormatEx`'s contract.
    unsafe { format_date(locale, flags, date, format, buffer, size) }
}

unsafe extern "efiapi" fn get_time_format_w(
    locale_id: u32,
    flags: u32,
    time: *const u8,
    format: *const u16,
    buffer: *mut u16,
    size: i32,
) -> i32 {
    let Some(locale) = locale_of_id(locale_id) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let known = time_flags::NO_MINUTES_OR_SECONDS
        | time_flags::NO_SECONDS
        | time_flags::NO_TIME_MARKER
        | time_flags::FORCE_24_HOUR
        | LOCALE_NOUSEROVERRIDE
        | LOCALE_USE_CP_ACP;
    if flags & !known != 0 {
        return fail(error::INVALID_FLAGS, 0);
    }
    // SAFETY: `GetTimeFormatW`'s contract: a readable `SYSTEMTIME`, or null.
    let Some(time) = (unsafe { time_of(time) }) else {
        return fail(error::INVALID_PARAMETER, 0);
    };
    let mut standard_units = [0u16; PICTURE_CAPACITY];
    // SAFETY: as above: a NUL-terminated picture, or null.
    let picture = unsafe { picture(format, locale.picture(Standard::Time), &mut standard_units) };
    // SAFETY: `GetTimeFormatW`'s contract: `size` writable units at `buffer`.
    unsafe {
        deliver(buffer, size, |out| {
            locale::format_time(text::utf16_chars(picture), &time, flags, &mut |c| out(c))
        })
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetCPInfo" => get_cp_info,
    b"IsValidCodePage" => is_valid_code_page,
    b"GetSystemDefaultUILanguage" => get_system_default_ui_language,
    b"GetStringTypeW" => get_string_type_w,
    b"GetStringTypeExW" => get_string_type_ex_w,
    b"LCMapStringW" => lc_map_string_w,
    b"GetDateFormatW" => get_date_format_w,
    b"GetDateFormatEx" => get_date_format_ex,
    b"GetTimeFormatW" => get_time_format_w,
];

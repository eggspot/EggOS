//! Locales (D190): C, POSIX, C.UTF-8 and en_US.UTF-8 for every category and UTF-8 for
//! character types, chosen per category by `setlocale` from a name, a composite name or the
//! environment (`darwin_locale::settings`). `nl_langinfo` and `localeconv` answer for the
//! categories' locales; `MB_CUR_MAX` reads `__mb_cur_max`, a variable that follows `LC_CTYPE`
//! (`___mb_cur_max()` is the function the extended-locale headers call); and `isalpha`'s family,
//! `toupper` and `towupper` read `__DefaultRuneLocale` for ASCII and call `___maskrune`,
//! `___toupper` and `___tolower` for the rest.

use super::{Exclusive, c_string, process};
use crate::calendar::TimeFormats;
use crate::darwin_locale::ctype;
use crate::darwin_locale::multibyte::Encoding;
use crate::darwin_locale::settings::{
    self, CTYPE, Locale, MESSAGES, MONETARY, NAME_CAPACITY, NUMERIC, Settings, TIME,
};
use core::ffi::{CStr, c_char, c_int, c_ulong};
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

const CHAR_MAX: c_char = 127;
const LONGEST_VARIABLE: usize = 16;

static SETTINGS: AtomicU32 = AtomicU32::new(Settings::C.bits());
static COMPOSITE_NAME: Exclusive<[u8; NAME_CAPACITY]> = Exclusive::new([0; NAME_CAPACITY]);

/// `__mb_cur_max`: the most bytes a character of `LC_CTYPE`'s encoding takes.
pub static MB_CUR_MAX: AtomicI32 = AtomicI32::new(1);

fn settings() -> Settings {
    Settings::from_bits(SETTINGS.load(Ordering::Relaxed))
}

/// The `LC_CTYPE` locale, whose classes and encoding the character functions use.
pub fn character_locale() -> Locale {
    settings().get(CTYPE)
}

/// The `LC_TIME` locale's `%c` and `%x`.
pub fn time_formats() -> TimeFormats {
    formats_of(settings())
}

fn formats_of(settings: Settings) -> TimeFormats {
    if settings.get(TIME).is_united_states() {
        TimeFormats::UNITED_STATES
    } else {
        TimeFormats::C
    }
}

fn environment(name: &[u8]) -> Option<&'static [u8]> {
    let mut variable = [0u8; LONGEST_VARIABLE];
    if name.len() >= variable.len() {
        return None;
    }
    variable[..name.len()].copy_from_slice(name);
    // SAFETY: `variable` is NUL-terminated.
    let value = unsafe { process::getenv(variable.as_ptr().cast()) };
    if value.is_null() {
        return None;
    }
    // SAFETY: an environment string is NUL-terminated and the program keeps it.
    Some(unsafe { c_string(value) })
}

/// # Safety
/// `locale` must be null or NUL-terminated.
pub unsafe extern "C" fn setlocale(category: c_int, locale: *const c_char) -> *const c_char {
    let mut current = settings();
    if !locale.is_null() {
        // SAFETY: the caller's promise.
        let name = unsafe { c_string(locale) };
        let Some(chosen) = current.set(category, name, environment) else {
            return core::ptr::null();
        };
        SETTINGS.store(chosen.bits(), Ordering::Relaxed);
        let longest = Encoding::of(chosen.get(CTYPE)).longest();
        MB_CUR_MAX.store(longest, Ordering::Relaxed);
        current = chosen;
    } else if !(settings::ALL..=MESSAGES).contains(&category) {
        return core::ptr::null();
    }
    match current.single(category) {
        Some(locale) => locale.name().as_ptr(),
        None => COMPOSITE_NAME.with(|name| {
            current.composite(name);
            name.as_ptr().cast::<c_char>()
        }),
    }
}

fn item(settings: Settings, item: usize) -> &'static CStr {
    const DAYS: [&CStr; 7] = [
        c"Sunday",
        c"Monday",
        c"Tuesday",
        c"Wednesday",
        c"Thursday",
        c"Friday",
        c"Saturday",
    ];
    const SHORT_DAYS: [&CStr; 7] = [c"Sun", c"Mon", c"Tue", c"Wed", c"Thu", c"Fri", c"Sat"];
    const MONTHS: [&CStr; 12] = [
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
    const SHORT_MONTHS: [&CStr; 12] = [
        c"Jan", c"Feb", c"Mar", c"Apr", c"May", c"Jun", c"Jul", c"Aug", c"Sep", c"Oct", c"Nov",
        c"Dec",
    ];
    let united = |category| settings.get(category).is_united_states();
    let pick =
        |category, us: &'static CStr, c: &'static CStr| if united(category) { us } else { c };
    let formats = formats_of(settings);
    match item {
        0 if settings.get(CTYPE).is_utf8() => c"UTF-8",
        0 => c"US-ASCII",
        1 => formats.date_time,
        2 => formats.date,
        3 => c"%H:%M:%S",
        4 => c"%I:%M:%S %p",
        5 => c"AM",
        6 => c"PM",
        7..=13 => DAYS[item - 7],
        14..=20 => SHORT_DAYS[item - 14],
        21..=32 => MONTHS[item - 21],
        33..=44 => SHORT_MONTHS[item - 33],
        50 => c".",
        51 => pick(NUMERIC, c",", c""),
        52 => pick(MESSAGES, c"^(([yY]([eE][sS])?)|([yY]))", c"^[yY]"),
        53 => pick(MESSAGES, c"^(([nN]([oO])?)|([nN]))", c"^[nN]"),
        54 => pick(MESSAGES, c"yes:y:YES:Y", c"yes"),
        55 => pick(MESSAGES, c"no:n:NO:N", c"no"),
        56 => pick(MONETARY, c"-$", c""),
        57 => c"md",
        _ => c"",
    }
}

pub extern "C" fn nl_langinfo(which: c_int) -> *const c_char {
    let which = usize::try_from(which).unwrap_or(usize::MAX);
    item(settings(), which).as_ptr()
}

/// Darwin's `struct lconv`.
#[repr(C)]
#[derive(Debug)]
pub struct Conventions {
    strings: [*const c_char; 10],
    numbers: [c_char; 14],
}

// SAFETY: the conventions are constant pointers to constant strings.
unsafe impl Sync for Conventions {}

const fn conventions(united_numbers: bool, united_money: bool) -> Conventions {
    let (thousands, grouping) = if united_numbers {
        (c",".as_ptr(), c"\x03".as_ptr())
    } else {
        (c"".as_ptr(), c"".as_ptr())
    };
    let money: [*const c_char; 7] = if united_money {
        [
            c"USD ".as_ptr(),
            c"$".as_ptr(),
            c".".as_ptr(),
            c",".as_ptr(),
            c"\x03".as_ptr(),
            c"".as_ptr(),
            c"-".as_ptr(),
        ]
    } else {
        [c"".as_ptr(); 7]
    };
    let numbers = if united_money {
        [2, 2, 1, 0, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1]
    } else {
        [CHAR_MAX; 14]
    };
    Conventions {
        strings: [
            c".".as_ptr(),
            thousands,
            grouping,
            money[0],
            money[1],
            money[2],
            money[3],
            money[4],
            money[5],
            money[6],
        ],
        numbers,
    }
}

static CONVENTIONS: [Conventions; 4] = [
    conventions(false, false),
    conventions(false, true),
    conventions(true, false),
    conventions(true, true),
];

pub extern "C" fn localeconv() -> *const Conventions {
    let settings = settings();
    let numbers = usize::from(settings.get(NUMERIC).is_united_states());
    let money = usize::from(settings.get(MONETARY).is_united_states());
    &raw const CONVENTIONS[numbers * 2 + money]
}

pub extern "C" fn mb_cur_max() -> c_int {
    MB_CUR_MAX.load(Ordering::Relaxed)
}

/// Darwin's `_RuneLocale`, of which programs read the classes and case mappings of bytes.
#[repr(C)]
#[derive(Debug)]
pub struct RuneLocale {
    magic: [u8; 8],
    encoding: [u8; 32],
    get_rune: usize,
    put_rune: usize,
    invalid_rune: i32,
    classes: [u32; 256],
    lower: [i32; 256],
    upper: [i32; 256],
    ranges: [[usize; 2]; 3],
    variable: usize,
    variable_length: i32,
    class_count: i32,
    class_names: usize,
}

const _: () = assert!(core::mem::size_of::<RuneLocale>() == 3208);
const _: () = assert!(core::mem::offset_of!(RuneLocale, classes) == 60);
const _: () = assert!(core::mem::offset_of!(RuneLocale, upper) == 2108);
const _: () = assert!(core::mem::offset_of!(RuneLocale, variable) == 3184);

const fn encoding_name() -> [u8; 32] {
    let mut name = [0; 32];
    let none = b"NONE";
    let mut index = 0;
    while index < none.len() {
        name[index] = none[index];
        index += 1;
    }
    name
}

/// `_DefaultRuneLocale`: the C locale's tables, which the ctype macros read for ASCII in every
/// locale.
pub static DEFAULT_RUNE_LOCALE: RuneLocale = RuneLocale {
    magic: *b"RuneMagA",
    encoding: encoding_name(),
    get_rune: 0,
    put_rune: 0,
    invalid_rune: 0xfffd,
    classes: ctype::DEFAULT_CLASSES,
    lower: ctype::DEFAULT_LOWER,
    upper: ctype::DEFAULT_UPPER,
    ranges: [[0; 2]; 3],
    variable: 0,
    variable_length: 0,
    class_count: 0,
    class_names: 0,
};

/// `___maskrune`: the classes of `rune` in the `LC_CTYPE` locale that `mask` selects.
pub extern "C" fn maskrune(rune: c_int, mask: c_ulong) -> c_int {
    (c_ulong::from(ctype::classes(character_locale(), rune)) & mask) as c_int
}

/// `___runetype`: all the classes of `rune` in the `LC_CTYPE` locale.
pub extern "C" fn runetype(rune: c_int) -> c_ulong {
    c_ulong::from(ctype::classes(character_locale(), rune))
}

/// `toupper`, `towupper` and `___toupper`.
pub extern "C" fn to_upper(rune: c_int) -> c_int {
    ctype::to_upper(character_locale(), rune)
}

/// `tolower`, `towlower` and `___tolower`.
pub extern "C" fn to_lower(rune: c_int) -> c_int {
    ctype::to_lower(character_locale(), rune)
}

/// # Safety
/// `name` must be NUL-terminated.
pub unsafe extern "C" fn wctype(name: *const c_char) -> u32 {
    // SAFETY: the caller's promise.
    match unsafe { c_string(name) } {
        b"alnum" => ctype::ALPHA | ctype::DIGIT,
        b"alpha" => ctype::ALPHA,
        b"blank" => ctype::BLANK,
        b"cntrl" => ctype::CONTROL,
        b"digit" => ctype::DIGIT,
        b"graph" => ctype::GRAPH,
        b"lower" => ctype::LOWER,
        b"print" => ctype::PRINT,
        b"punct" => ctype::PUNCT,
        b"space" => ctype::SPACE,
        b"upper" => ctype::UPPER,
        b"xdigit" => ctype::HEX_DIGIT,
        _ => 0,
    }
}

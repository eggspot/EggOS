//! The C runtime's locales (D229): `setlocale`, `_wsetlocale`, `localeconv`, `_create_locale`
//! and the functions behind `MB_CUR_MAX` and `_pctype`, over `crate::locale`'s runtime
//! locales, "C" and English (United States) with UTF-8. One locale serves the whole process
//! (`_configthreadlocale` answers that per-thread locales are off); `setlocale`'s answer is a
//! buffer of the runtime's that the next call rewrites, as C allows. A `_locale_t` points to
//! the public head of its locale data (`_pctype`, `MB_CUR_MAX`, the code page) as the UCRT's
//! inline functions read it.

use super::super::SpinLock;
use super::super::file::wide_string;
use super::super::heap as process_heap;
use super::runtime::{errno, set_errno};
use crate::library::Export;
use crate::locale::{
    RuntimeLocales, category, classic_character_types, united_states_conventions as us,
};
use crate::text;
use core::ffi::CStr;
use core::ptr;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

const NAME_CAPACITY: usize = 256;
const CONVENTION_TEXT: usize = 8;
const CHAR_MAX: u8 = 127;
const MULTIBYTE_DATA_WORDS: usize = 64;

struct Global {
    locales: RuntimeLocales,
    narrow_name: [u8; NAME_CAPACITY],
    wide_name: [u16; NAME_CAPACITY],
}

static GLOBAL: SpinLock<Global> = SpinLock::new(Global {
    locales: RuntimeLocales::C,
    narrow_name: [0; NAME_CAPACITY],
    wide_name: [0; NAME_CAPACITY],
});
static CODE_PAGE: AtomicU32 = AtomicU32::new(0);
static LONGEST_CHARACTER: AtomicI32 = AtomicI32::new(1);
static CHARACTER_TYPES: [u16; 257] = classic_character_types();
static MULTIBYTE_DATA: [u64; MULTIBYTE_DATA_WORDS] = [0; MULTIBYTE_DATA_WORDS];

fn utf8_name(name: impl Iterator<Item = char>, out: &mut [u8; NAME_CAPACITY]) -> Option<&str> {
    let mut length = 0;
    for c in name {
        let mut units = [0; 4];
        let encoded = c.encode_utf8(&mut units).as_bytes();
        out.get_mut(length..length + encoded.len())?
            .copy_from_slice(encoded);
        length += encoded.len();
    }
    core::str::from_utf8(&out[..length]).ok()
}

fn known_category(category: i32) -> bool {
    category == category::ALL || RuntimeLocales::C.of(category).is_some()
}

fn set_or_describe(category: i32, name: Option<&str>) -> Option<(*const u8, *const u16)> {
    if !known_category(category) {
        set_errno(errno::EINVAL);
        return None;
    }
    GLOBAL.with(|global| {
        if let Some(name) = name {
            if !global.locales.set(category, name) {
                return None;
            }
            CODE_PAGE.store(global.locales.code_page(), Ordering::Release);
            LONGEST_CHARACTER.store(global.locales.longest_character(), Ordering::Release);
        }
        let Global {
            locales,
            narrow_name,
            wide_name,
        } = global;
        let mut length = 0;
        locales.describe(category, &mut |c| {
            if length + 1 < NAME_CAPACITY {
                narrow_name[length] = c as u8;
                wide_name[length] = c as u16;
                length += 1;
            }
        });
        narrow_name[length] = 0;
        wide_name[length] = 0;
        Some((narrow_name.as_ptr(), wide_name.as_ptr()))
    })
}

unsafe fn narrow_text<'a>(text: *const u8) -> Option<&'a [u8]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text.cast()) }.to_bytes())
}

unsafe fn wide_text<'a>(text: *const u16) -> Option<&'a [u16]> {
    // SAFETY: the caller's duty: null or a NUL-terminated string.
    (!text.is_null()).then(|| unsafe { wide_string(text) })
}

unsafe extern "efiapi" fn set_locale(category: i32, name: *const u8) -> *const u8 {
    let mut buffer = [0; NAME_CAPACITY];
    // SAFETY: `setlocale`'s contract: null or a NUL-terminated name.
    let name = match unsafe { narrow_text(name) } {
        Some(name) => match utf8_name(text::utf8_chars(name), &mut buffer) {
            Some(name) => Some(name),
            None => return ptr::null(),
        },
        None => None,
    };
    set_or_describe(category, name).map_or(ptr::null(), |(narrow, _)| narrow)
}

unsafe extern "efiapi" fn wide_set_locale(category: i32, name: *const u16) -> *const u16 {
    let mut buffer = [0; NAME_CAPACITY];
    // SAFETY: `_wsetlocale`'s contract: null or a NUL-terminated name.
    let name = match unsafe { wide_text(name) } {
        Some(name) => match utf8_name(text::utf16_chars(name), &mut buffer) {
            Some(name) => Some(name),
            None => return ptr::null(),
        },
        None => None,
    };
    set_or_describe(category, name).map_or(ptr::null(), |(_, wide)| wide)
}

struct ConventionTexts {
    narrow: [[u8; CONVENTION_TEXT]; 10],
    wide: [[u16; CONVENTION_TEXT]; 8],
    numbers: [u8; 8],
}

const fn narrow(text: &str) -> [u8; CONVENTION_TEXT] {
    let bytes = text.as_bytes();
    let mut out = [0; CONVENTION_TEXT];
    let mut index = 0;
    while index < bytes.len() {
        out[index] = bytes[index];
        index += 1;
    }
    out
}

const fn wide(text: &str) -> [u16; CONVENTION_TEXT] {
    let bytes = text.as_bytes();
    let mut out = [0; CONVENTION_TEXT];
    let mut index = 0;
    while index < bytes.len() {
        out[index] = bytes[index] as u16;
        index += 1;
    }
    out
}

static C_TEXTS: ConventionTexts = ConventionTexts {
    narrow: [
        narrow("."),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
        narrow(""),
    ],
    wide: [
        wide("."),
        wide(""),
        wide(""),
        wide(""),
        wide(""),
        wide(""),
        wide(""),
        wide(""),
    ],
    numbers: [CHAR_MAX; 8],
};

static UNITED_STATES_TEXTS: ConventionTexts = ConventionTexts {
    narrow: [
        narrow(us::DECIMAL_POINT),
        narrow(us::THOUSANDS_SEPARATOR),
        narrow(us::GROUPING),
        narrow(us::INTERNATIONAL_CURRENCY_SYMBOL),
        narrow(us::CURRENCY_SYMBOL),
        narrow(us::MONETARY_DECIMAL_POINT),
        narrow(us::MONETARY_THOUSANDS_SEPARATOR),
        narrow(us::MONETARY_GROUPING),
        narrow(us::POSITIVE_SIGN),
        narrow(us::NEGATIVE_SIGN),
    ],
    wide: [
        wide(us::DECIMAL_POINT),
        wide(us::THOUSANDS_SEPARATOR),
        wide(us::INTERNATIONAL_CURRENCY_SYMBOL),
        wide(us::CURRENCY_SYMBOL),
        wide(us::MONETARY_DECIMAL_POINT),
        wide(us::MONETARY_THOUSANDS_SEPARATOR),
        wide(us::POSITIVE_SIGN),
        wide(us::NEGATIVE_SIGN),
    ],
    numbers: us::MONETARY_NUMBERS,
};

const NUMERIC_NARROW_TEXTS: usize = 3;
const NUMERIC_WIDE_TEXTS: usize = 2;

/// `struct lconv`: the old runtime's fields, then the UCRT's wide ones.
#[repr(C)]
struct LocaleConventions {
    texts: [*const u8; 10],
    numbers: [u8; 8],
    wide_texts: [*const u16; 8],
}

// SAFETY: the conventions are read-only for the program's life.
unsafe impl Sync for LocaleConventions {}

const fn conventions(
    numeric: &'static ConventionTexts,
    monetary: &'static ConventionTexts,
) -> LocaleConventions {
    let mut texts = [ptr::null(); 10];
    let mut index = 0;
    while index < texts.len() {
        texts[index] = match index < NUMERIC_NARROW_TEXTS {
            true => numeric.narrow[index].as_ptr(),
            false => monetary.narrow[index].as_ptr(),
        };
        index += 1;
    }
    let mut wide_texts = [ptr::null(); 8];
    let mut index = 0;
    while index < wide_texts.len() {
        wide_texts[index] = match index < NUMERIC_WIDE_TEXTS {
            true => numeric.wide[index].as_ptr(),
            false => monetary.wide[index].as_ptr(),
        };
        index += 1;
    }
    LocaleConventions {
        texts,
        numbers: monetary.numbers,
        wide_texts,
    }
}

static CONVENTIONS: [LocaleConventions; 4] = [
    conventions(&UNITED_STATES_TEXTS, &UNITED_STATES_TEXTS),
    conventions(&UNITED_STATES_TEXTS, &C_TEXTS),
    conventions(&C_TEXTS, &UNITED_STATES_TEXTS),
    conventions(&C_TEXTS, &C_TEXTS),
];

extern "efiapi" fn locale_conventions() -> *const LocaleConventions {
    let (numeric_c, monetary_c) = GLOBAL.with(|global| global.locales.conventions_are_c());
    &raw const CONVENTIONS[usize::from(numeric_c) * 2 + usize::from(monetary_c)]
}

#[repr(C)]
struct LocaleData {
    character_types: *const u16,
    longest_character: i32,
    code_page: u32,
    locales: RuntimeLocales,
}

/// A `_locale_t`'s object: `__crt_locale_pointers`, then the data its first pointer names.
#[repr(C)]
struct LocaleObject {
    data_pointer: *const LocaleData,
    multibyte_data_pointer: *const u64,
    data: LocaleData,
}

fn character_types_at_zero() -> *const u16 {
    CHARACTER_TYPES.as_ptr().wrapping_add(1)
}

fn locale_object(locales: RuntimeLocales) -> *mut LocaleObject {
    let block = process_heap::allocate_with(core::mem::size_of::<LocaleObject>(), false)
        .cast::<LocaleObject>();
    if block.is_null() {
        set_errno(errno::ENOMEM);
        return block;
    }
    let object = LocaleObject {
        data_pointer: ptr::null(),
        multibyte_data_pointer: MULTIBYTE_DATA.as_ptr(),
        data: LocaleData {
            character_types: character_types_at_zero(),
            longest_character: locales.longest_character(),
            code_page: locales.code_page(),
            locales,
        },
    };
    // SAFETY: a fresh heap block, 16-byte aligned, for one locale object.
    unsafe { block.write(object) };
    // SAFETY: as above; the object now holds its data.
    unsafe { (*block).data_pointer = &raw const (*block).data };
    block
}

fn create(category: i32, name: Option<&str>) -> *mut LocaleObject {
    let Some(name) = name.filter(|_| known_category(category)) else {
        set_errno(errno::EINVAL);
        return ptr::null_mut();
    };
    let mut locales = RuntimeLocales::C;
    if !locales.set(category, name) {
        return ptr::null_mut();
    }
    locale_object(locales)
}

unsafe extern "efiapi" fn create_locale(category: i32, name: *const u8) -> *mut LocaleObject {
    let mut buffer = [0; NAME_CAPACITY];
    // SAFETY: `_create_locale`'s contract: a NUL-terminated name.
    let name = unsafe { narrow_text(name) }
        .and_then(|name| utf8_name(text::utf8_chars(name), &mut buffer));
    create(category, name)
}

unsafe extern "efiapi" fn wide_create_locale(category: i32, name: *const u16) -> *mut LocaleObject {
    let mut buffer = [0; NAME_CAPACITY];
    // SAFETY: `_wcreate_locale`'s contract: a NUL-terminated name.
    let name =
        unsafe { wide_text(name) }.and_then(|name| utf8_name(text::utf16_chars(name), &mut buffer));
    create(category, name)
}

extern "efiapi" fn current_locale() -> *mut LocaleObject {
    locale_object(GLOBAL.with(|global| global.locales))
}

unsafe extern "efiapi" fn free_locale(locale: *mut LocaleObject) {
    if !locale.is_null() {
        // SAFETY: `_free_locale`'s contract: a locale `_create_locale` or `_get_current_locale`
        // made, freed once.
        unsafe { process_heap::free(locale.cast()) };
    }
}

extern "efiapi" fn code_page() -> u32 {
    CODE_PAGE.load(Ordering::Acquire)
}

extern "efiapi" fn longest_character() -> i32 {
    LONGEST_CHARACTER.load(Ordering::Acquire)
}

extern "efiapi" fn longest_character_variable() -> *mut i32 {
    LONGEST_CHARACTER.as_ptr()
}

extern "efiapi" fn character_types() -> *const u16 {
    character_types_at_zero()
}

/// `locale.h`'s functions and the locale queries `ctype.h` and `stdlib.h` call, in both
/// runtimes.
pub(super) const EXPORTS: &[Export] = exports![
    b"setlocale" => set_locale,
    b"_wsetlocale" => wide_set_locale,
    b"localeconv" => locale_conventions,
    b"_create_locale" => create_locale,
    b"_wcreate_locale" => wide_create_locale,
    b"_get_current_locale" => current_locale,
    b"_free_locale" => free_locale,
    b"___lc_codepage_func" => code_page,
    b"___mb_cur_max_func" => longest_character,
    b"__p___mb_cur_max" => longest_character_variable,
    b"__pctype_func" => character_types,
];

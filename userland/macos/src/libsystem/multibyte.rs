//! The multibyte and wide-character conversions in the `LC_CTYPE` locale's encoding
//! (`darwin_locale::multibyte`): `mbrtowc`, `mbrlen`, `wcrtomb`, `mbsrtowcs`, `wcsrtombs` and
//! `mbsinit`, which keep a partial character in the program's `mbstate_t` (or their own when it
//! passes none), and `mbtowc`, `mblen`, `wctomb`, `mbstowcs`, `wcstombs`, `btowc` and `wctob`.
//! A wide character is Darwin's 32-bit `wchar_t`.

use super::locale::character_locale;
use super::set_errno;
use crate::darwin_locale::multibyte::{self, Converted, Decoder, Encoding, Step};
use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicU64, Ordering};

const EILSEQ: c_int = 92;
const INVALID: usize = usize::MAX;
const INCOMPLETE: usize = usize::MAX - 1;
const END_OF_FILE: c_int = -1;
const LONGEST_SLICE: usize = isize::MAX as usize / 4;

static MBRTOWC_STATE: AtomicU64 = AtomicU64::new(0);
static MBRLEN_STATE: AtomicU64 = AtomicU64::new(0);
static MBSRTOWCS_STATE: AtomicU64 = AtomicU64::new(0);

fn encoding() -> Encoding {
    Encoding::of(character_locale())
}

fn with_decoder<R>(state: *mut u64, own: &AtomicU64, work: impl FnOnce(&mut Decoder) -> R) -> R {
    let mut decoder = if state.is_null() {
        Decoder::from_bits(own.load(Ordering::Relaxed))
    } else {
        // SAFETY: a program's `mbstate_t` is 128 bytes; the decoder keeps the first eight.
        Decoder::from_bits(unsafe { state.read_unaligned() })
    };
    let result = work(&mut decoder);
    if state.is_null() {
        own.store(decoder.bits(), Ordering::Relaxed);
    } else {
        // SAFETY: as above.
        unsafe { state.write_unaligned(decoder.bits()) };
    }
    result
}

unsafe fn decode_one(
    decoder: &mut Decoder,
    wide: *mut i32,
    text: *const c_char,
    length: usize,
) -> usize {
    let encoding = encoding();
    let mut index = 0;
    while index < length {
        // SAFETY: the caller's promise; the walk stops at the first whole character.
        let byte = unsafe { text.cast::<u8>().wrapping_add(index).read() };
        index += 1;
        match decoder.decode(encoding, &[byte]) {
            Step::Incomplete => {}
            Step::Invalid => {
                set_errno(EILSEQ);
                return INVALID;
            }
            Step::Character { scalar, .. } => {
                if !wide.is_null() {
                    // SAFETY: the caller's promise.
                    unsafe { wide.write(scalar as i32) };
                }
                return if scalar == 0 { 0 } else { index };
            }
        }
    }
    INCOMPLETE
}

/// # Safety
/// `wide` must be null or writable, `text` null or readable for `length` bytes, and `state`
/// null or an `mbstate_t`.
pub unsafe extern "C" fn mbrtowc(
    wide: *mut i32,
    text: *const c_char,
    length: usize,
    state: *mut u64,
) -> usize {
    let (wide, text, length) = if text.is_null() {
        (core::ptr::null_mut(), c"".as_ptr(), 1)
    } else {
        (wide, text, length)
    };
    with_decoder(state, &MBRTOWC_STATE, |decoder| {
        // SAFETY: the caller's promise.
        unsafe { decode_one(decoder, wide, text, length) }
    })
}

/// # Safety
/// As [`mbrtowc`]'s.
pub unsafe extern "C" fn mbrlen(text: *const c_char, length: usize, state: *mut u64) -> usize {
    let (text, length) = if text.is_null() {
        (c"".as_ptr(), 1)
    } else {
        (text, length)
    };
    with_decoder(state, &MBRLEN_STATE, |decoder| {
        // SAFETY: the caller's promise.
        unsafe { decode_one(decoder, core::ptr::null_mut(), text, length) }
    })
}

/// # Safety
/// `state` must be null or an `mbstate_t`.
pub unsafe extern "C" fn mbsinit(state: *const u64) -> c_int {
    if state.is_null() {
        return 1;
    }
    // SAFETY: the caller's promise.
    let bits = unsafe { state.read_unaligned() };
    c_int::from(Decoder::from_bits(bits).is_initial())
}

/// # Safety
/// `wide` must be null or writable and `text` null or readable for `length` bytes.
pub unsafe extern "C" fn mbtowc(wide: *mut i32, text: *const c_char, length: usize) -> c_int {
    if text.is_null() {
        return 0;
    }
    let mut decoder = Decoder::default();
    // SAFETY: the caller's promise.
    match unsafe { decode_one(&mut decoder, wide, text, length) } {
        INVALID | INCOMPLETE => {
            set_errno(EILSEQ);
            -1
        }
        used => used as c_int,
    }
}

/// # Safety
/// `text` must be null or readable for `length` bytes.
pub unsafe extern "C" fn mblen(text: *const c_char, length: usize) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { mbtowc(core::ptr::null_mut(), text, length) }
}

unsafe fn encode_one(text: *mut c_char, wide: i32) -> usize {
    let mut bytes = [0; 4];
    let Some(length) = encoding().encode(wide as u32, &mut bytes) else {
        set_errno(EILSEQ);
        return INVALID;
    };
    // SAFETY: the caller's promise: room for `MB_CUR_MAX` bytes.
    unsafe {
        text.cast::<u8>()
            .copy_from_nonoverlapping(bytes.as_ptr(), length)
    };
    length
}

/// # Safety
/// `text` must be null or writable for `MB_CUR_MAX` bytes and `state` null or an `mbstate_t`.
pub unsafe extern "C" fn wcrtomb(text: *mut c_char, wide: i32, state: *mut u64) -> usize {
    if text.is_null() {
        if !state.is_null() {
            // SAFETY: the caller's promise.
            unsafe { state.write_unaligned(Decoder::default().bits()) };
        }
        return 1;
    }
    // SAFETY: the caller's promise.
    unsafe { encode_one(text, wide) }
}

/// # Safety
/// `text` must be null or writable for `MB_CUR_MAX` bytes.
pub unsafe extern "C" fn wctomb(text: *mut c_char, wide: i32) -> c_int {
    if text.is_null() {
        return 0;
    }
    // SAFETY: the caller's promise.
    match unsafe { encode_one(text, wide) } {
        INVALID => -1,
        length => length as c_int,
    }
}

pub extern "C" fn btowc(byte: c_int) -> c_int {
    match (u8::try_from(byte), encoding()) {
        (Ok(byte), Encoding::Byte) => c_int::from(byte),
        (Ok(byte @ 0..=0x7f), Encoding::Utf8) => c_int::from(byte),
        _ => END_OF_FILE,
    }
}

pub extern "C" fn wctob(wide: c_int) -> c_int {
    let mut bytes = [0; 4];
    match encoding().encode(wide as u32, &mut bytes) {
        Some(1) => c_int::from(bytes[0]),
        _ => END_OF_FILE,
    }
}

unsafe fn decode_from(
    decoder: &mut Decoder,
    text: *const c_char,
    destination: *mut i32,
    limit: usize,
) -> Option<Converted> {
    let byte_at = |index: usize| {
        // SAFETY: the caller's promise; the walk stops at the NUL, the limit or a bad sequence.
        unsafe { text.cast::<u8>().wrapping_add(index).read() }
    };
    let out = (!destination.is_null()).then(|| {
        // SAFETY: the caller's promise.
        unsafe {
            core::slice::from_raw_parts_mut(destination.cast::<u32>(), limit.min(LONGEST_SLICE))
        }
    });
    multibyte::decode_text(encoding(), decoder, byte_at, out)
}

unsafe fn encode_from(
    text: *const i32,
    destination: *mut c_char,
    limit: usize,
) -> Option<Converted> {
    let wide_at = |index: usize| {
        // SAFETY: the caller's promise; the walk stops at the NUL, the limit or a bad character.
        unsafe { text.wrapping_add(index).read() as u32 }
    };
    let out = (!destination.is_null()).then(|| {
        // SAFETY: the caller's promise.
        unsafe {
            core::slice::from_raw_parts_mut(destination.cast::<u8>(), limit.min(LONGEST_SLICE))
        }
    });
    multibyte::encode_text(encoding(), wide_at, out)
}

fn counted(converted: Option<Converted>) -> usize {
    match converted {
        Some(converted) => converted.count,
        None => {
            set_errno(EILSEQ);
            INVALID
        }
    }
}

/// # Safety
/// `source` must point at a NUL-terminated string, `destination` be null or writable for
/// `limit` wide characters and `state` null or an `mbstate_t`.
pub unsafe extern "C" fn mbsrtowcs(
    destination: *mut i32,
    source: *mut *const c_char,
    limit: usize,
    state: *mut u64,
) -> usize {
    // SAFETY: the caller's promise.
    let text = unsafe { source.read() };
    let converted = with_decoder(state, &MBSRTOWCS_STATE, |decoder| {
        // SAFETY: the caller's promise.
        unsafe { decode_from(decoder, text, destination, limit) }
    });
    if let (Some(done), false) = (converted, destination.is_null()) {
        let next = if done.finished {
            core::ptr::null()
        } else {
            text.wrapping_add(done.read)
        };
        // SAFETY: the caller's promise.
        unsafe { source.write(next) };
    }
    counted(converted)
}

/// # Safety
/// `source` must point at a NUL-terminated wide string and `destination` be null or writable
/// for `limit` bytes.
pub unsafe extern "C" fn wcsrtombs(
    destination: *mut c_char,
    source: *mut *const i32,
    limit: usize,
    _state: *mut u64,
) -> usize {
    // SAFETY: the caller's promise.
    let text = unsafe { source.read() };
    // SAFETY: the caller's promise.
    let converted = unsafe { encode_from(text, destination, limit) };
    if let (Some(done), false) = (converted, destination.is_null()) {
        let next = if done.finished {
            core::ptr::null()
        } else {
            text.wrapping_add(done.read)
        };
        // SAFETY: the caller's promise.
        unsafe { source.write(next) };
    }
    counted(converted)
}

/// # Safety
/// `text` must be NUL-terminated and `destination` null or writable for `limit` wide
/// characters.
pub unsafe extern "C" fn mbstowcs(
    destination: *mut i32,
    text: *const c_char,
    limit: usize,
) -> usize {
    // SAFETY: the caller's promise.
    counted(unsafe { decode_from(&mut Decoder::default(), text, destination, limit) })
}

/// # Safety
/// `text` must be a NUL-terminated wide string and `destination` null or writable for `limit`
/// bytes.
pub unsafe extern "C" fn wcstombs(
    destination: *mut c_char,
    text: *const i32,
    limit: usize,
) -> usize {
    // SAFETY: the caller's promise.
    counted(unsafe { encode_from(text, destination, limit) })
}

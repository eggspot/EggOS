//! `scanf`, `fscanf`, `sscanf` and their `va_list` forms, on the shared scanner
//! (`eggos_c_format::scan`, Darwin's sizes): a stream is read a byte at a time through its
//! buffer, and what a number does not use goes back to the stream, so the next read sees it.

use super::heap;
use super::stdio::{Stream, read_byte, standard_input, stream};
use super::variadic::{RawVaList, VaList, variadic};
use super::{c_string, set_errno};
use crate::darwin::EBADF;
use crate::darwin_locale::multibyte::{Decoder, Encoding, Step};
use crate::format::Arguments;
use crate::format::scan::{self, Bytes, Field, Precision, Rules, Size, Source};
use core::ffi::{c_char, c_int};

fn rules() -> Rules {
    Rules {
        dialect: crate::format::Dialect::Darwin,
        utf8: super::locale::character_locale().is_utf8(),
    }
}

struct StreamSource<'s> {
    stream: &'s mut Stream,
    peeked: Option<u8>,
    taken: *mut u8,
    capacity: usize,
    length: usize,
}

impl<'s> StreamSource<'s> {
    fn new(stream: &'s mut Stream) -> Self {
        Self {
            stream,
            peeked: None,
            taken: core::ptr::null_mut(),
            capacity: 0,
            length: 0,
        }
    }

    fn keep(&mut self, byte: u8) -> bool {
        if self.length == self.capacity {
            let capacity = (self.capacity * 2).max(64);
            let grown = heap::reallocate(self.taken, capacity);
            if grown.is_null() {
                return false;
            }
            self.taken = grown;
            self.capacity = capacity;
        }
        // SAFETY: `length` is below the block's `capacity` bytes.
        unsafe { self.taken.wrapping_add(self.length).write(byte) };
        self.length += 1;
        true
    }
}

impl Source for StreamSource<'_> {
    fn peek(&mut self) -> Option<u8> {
        if self.peeked.is_none() {
            self.peeked = read_byte(self.stream);
        }
        self.peeked
    }

    fn advance(&mut self) {
        self.peeked = None;
    }

    fn take_while(&mut self, limit: usize, accept: &mut dyn FnMut(u8) -> bool) -> &[u8] {
        self.length = 0;
        while self.length < limit {
            match self.peek() {
                Some(byte) if accept(byte) && self.keep(byte) => self.peeked = None,
                _ => break,
            }
        }
        if self.length == 0 {
            return &[];
        }
        // SAFETY: the block holds the `length` bytes just kept.
        unsafe { core::slice::from_raw_parts(self.taken, self.length) }
    }

    fn give_back(&mut self, count: usize) {
        if let Some(byte) = self.peeked.take() {
            self.stream.push_back(byte);
        }
        let count = count.min(self.length);
        for index in (self.length - count..self.length).rev() {
            // SAFETY: `index` is below the `length` bytes kept.
            let byte = unsafe { self.taken.wrapping_add(index).read() };
            self.stream.push_back(byte);
        }
        self.length -= count;
    }
}

impl Drop for StreamSource<'_> {
    fn drop(&mut self) {
        if let Some(byte) = self.peeked.take() {
            self.stream.push_back(byte);
        }
        if !self.taken.is_null() {
            heap::release(self.taken);
        }
    }
}

fn write_integer(target: usize, value: u64, size: Size) {
    match size {
        // SAFETY: the program's argument for the conversion points at an integer of its size.
        Size::Byte => unsafe { (target as *mut u8).write_unaligned(value as u8) },
        // SAFETY: as above.
        Size::Short => unsafe { (target as *mut u16).write_unaligned(value as u16) },
        // SAFETY: as above.
        Size::Int => unsafe { (target as *mut u32).write_unaligned(value as u32) },
        // SAFETY: as above.
        Size::Wide => unsafe { (target as *mut u64).write_unaligned(value) },
    }
}

fn write_long_double(target: usize, value: f64) {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: the argument for `%Lf` points at a `long double`, a double on arm64.
        unsafe { (target as *mut f64).write_unaligned(value) };
    }
    #[cfg(target_arch = "x86_64")]
    {
        let (significand, sign_exponent) = crate::format::double_to_extended(value);
        let target = target as *mut u8;
        // SAFETY: the argument for `%Lf` points at a 16-byte x87 `long double`.
        unsafe { target.cast::<u64>().write_unaligned(significand) };
        // SAFETY: as above; byte 8 is inside it.
        let exponent_at = unsafe { target.add(8) };
        // SAFETY: as above; bytes 8 and 9 hold the sign and exponent.
        unsafe { exponent_at.cast::<u16>().write_unaligned(sign_exponent) };
        // SAFETY: as above; byte 10 is inside it.
        let padding_at = unsafe { target.add(10) };
        // SAFETY: as above; bytes 10 to 15 are its padding.
        unsafe { padding_at.write_bytes(0, 6) };
    }
}

fn write_bytes(target: usize, bytes: &[u8], nul: bool) {
    let target = target as *mut u8;
    // SAFETY: the argument for `%c`, `%s` or `%[` points at room for the characters.
    unsafe { target.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
    if nul {
        // SAFETY: for `%s` and `%[` the room holds a NUL after the characters.
        let end = unsafe { target.add(bytes.len()) };
        // SAFETY: as above; `end` is that NUL's byte.
        unsafe { end.write(0) };
    }
}

fn write_wide(target: usize, bytes: &[u8], nul: bool, encoding: Encoding) {
    let target = target as *mut u32;
    let mut decoder = Decoder::default();
    let mut count = 0;
    for &byte in bytes {
        let scalar = match decoder.decode(encoding, &[byte]) {
            Step::Character { scalar, .. } => scalar,
            Step::Incomplete => continue,
            Step::Invalid => u32::from(byte),
        };
        // SAFETY: the argument for `%lc`, `%ls` or `%l[` points at room for a `wchar_t` per
        // character and, for the last two, a NUL.
        unsafe { target.wrapping_add(count).write_unaligned(scalar) };
        count += 1;
    }
    if nul {
        // SAFETY: as above.
        unsafe { target.wrapping_add(count).write_unaligned(0) };
    }
}

fn store(field: Field<'_>, arguments: &mut VaList, encoding: Encoding) -> bool {
    let target = arguments.integer() as usize;
    if target == 0 {
        return false;
    }
    match field {
        Field::Signed(value, size) => write_integer(target, value as u64, size),
        Field::Unsigned(value, size) => write_integer(target, value, size),
        Field::Count(count, size) => write_integer(target, count as u64, size),
        // SAFETY: the argument for `%f` and its family points at a `float`.
        Field::Float(value, Precision::Single) => unsafe {
            (target as *mut f32).write_unaligned(value as f32)
        },
        // SAFETY: the argument for `%lf` and its family points at a `double`.
        Field::Float(value, Precision::Double) => unsafe {
            (target as *mut f64).write_unaligned(value)
        },
        Field::Float(value, Precision::Extended) => write_long_double(target, value),
        Field::Characters(bytes) => write_bytes(target, bytes, false),
        Field::Text(bytes) => write_bytes(target, bytes, true),
        Field::WideCharacters(bytes) => write_wide(target, bytes, false, encoding),
        Field::WideText(bytes) => write_wide(target, bytes, true, encoding),
    }
    true
}

fn scanned(source: &mut dyn Source, format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: the public callers' callers promise a NUL-terminated format.
    let format = unsafe { c_string(format) };
    // SAFETY: and the `va_list` with a pointer for each conversion it stores.
    let mut arguments = unsafe { VaList::new(arguments) };
    let rules = rules();
    let encoding = if rules.utf8 {
        Encoding::Utf8
    } else {
        Encoding::Byte
    };
    scan::scan(source, format, rules, &mut |field| {
        store(field, &mut arguments, encoding)
    })
}

/// # Safety
/// `text` and `format` must be NUL-terminated and `arguments` the `va_list` for the format.
pub unsafe extern "C" fn vsscanf(
    text: *const c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: the caller's promise.
    let mut source = Bytes::new(unsafe { c_string(text) });
    scanned(&mut source, format, arguments)
}

/// # Safety
/// `file` must be a stream, `format` NUL-terminated and `arguments` its `va_list`.
pub unsafe extern "C" fn vfscanf(
    file: *mut Stream,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: the caller's promise.
    let Some(stream) = (unsafe { stream(file) }) else {
        set_errno(EBADF);
        return -1;
    };
    let mut source = StreamSource::new(stream);
    scanned(&mut source, format, arguments)
}

/// # Safety
/// `format` must be NUL-terminated and `arguments` its `va_list`.
pub unsafe extern "C" fn vscanf(format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: standard input is a stream; the rest is the caller's promise.
    unsafe { vfscanf(standard_input(), format, arguments) }
}

unsafe extern "C" fn scanf_with(format: *const c_char, arguments: RawVaList) -> c_int {
    // SAFETY: the variadic entry passes the program's format and arguments.
    unsafe { vscanf(format, arguments) }
}

unsafe extern "C" fn fscanf_with(
    file: *mut Stream,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vfscanf(file, format, arguments) }
}

unsafe extern "C" fn sscanf_with(
    text: *const c_char,
    format: *const c_char,
    arguments: RawVaList,
) -> c_int {
    // SAFETY: as above.
    unsafe { vsscanf(text, format, arguments) }
}

variadic!(eggos_macos_scanf => scanf_with, 1, "x1", "rsi");
variadic!(eggos_macos_fscanf => fscanf_with, 2, "x2", "rdx");
variadic!(eggos_macos_sscanf => sscanf_with, 2, "x2", "rdx");

pub use self::{
    eggos_macos_fscanf as fscanf, eggos_macos_scanf as scanf, eggos_macos_sscanf as sscanf,
};

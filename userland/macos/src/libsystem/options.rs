//! `getopt` and `getopt_long` over the program's `argv`, with BSD's variables `optarg`,
//! `optind`, `opterr`, `optopt` and `optreset`, and its messages on standard error.

use super::{c_string, process, stdio};
use crate::getopt::{self, Cursor, Found, Takes};
use crate::linux;
use core::ffi::{c_char, c_int};
use core::ptr;

/// `optarg`: the current option's argument.
pub static mut OPTARG: *mut c_char = ptr::null_mut();
/// `optind`: the index of the next word to parse.
pub static mut OPTIND: c_int = 1;
/// `opterr`: whether to print messages.
pub static mut OPTERR: c_int = 1;
/// `optopt`: the option character of the last error.
pub static mut OPTOPT: c_int = 0;
/// `optreset`: set to restart parsing.
pub static mut OPTRESET: c_int = 0;
static mut CURSOR: Cursor = Cursor {
    index: 1,
    offset: 0,
};

/// `struct option`.
#[repr(C)]
#[derive(Debug)]
pub struct LongOption {
    name: *const c_char,
    has_arg: c_int,
    flag: *mut c_int,
    value: c_int,
}

fn message(parts: &[&[u8]]) {
    stdio::flush_all();
    // SAFETY: `_NSGetProgname`'s pointer is set before the program runs.
    let program = unsafe { *process::ns_get_progname() };
    if !program.is_null() {
        // SAFETY: a NUL-terminated argument.
        let _ = linux::write_all(2, unsafe { c_string(program) });
        let _ = linux::write_all(2, b": ");
    }
    for part in parts {
        let _ = linux::write_all(2, part);
    }
    let _ = linux::write_all(2, b"\n");
}

unsafe fn parse(
    count: c_int,
    arguments: *const *mut c_char,
    spec: *const c_char,
    long: *const LongOption,
    long_index: *mut c_int,
) -> c_int {
    // SAFETY: the caller's promise: a NUL-terminated option string.
    let spec = unsafe { c_string(spec) };
    let count = count.max(0) as usize;
    let word = |index: usize| {
        (index < count).then(|| {
            // SAFETY: the caller's promise: `count` NUL-terminated words.
            let pointer = unsafe { arguments.add(index).read() };
            // SAFETY: as above.
            unsafe { c_string(pointer) }
        })
    };
    let long_count = if long.is_null() {
        0
    } else {
        // SAFETY: the caller's promise: an array ended by an entry with a null name.
        (0..).take_while(|&index| !unsafe { (*long.add(index)).name }.is_null()).count()
    };
    let longs = (0..long_count).map(|index| {
        // SAFETY: as above.
        let option = unsafe { &*long.add(index) };
        // SAFETY: as above.
        (unsafe { c_string(option.name) }, Takes::from_c(option.has_arg))
    });
    // SAFETY: the program's options state, which a program parses from one thread.
    let (index, reset) = unsafe { (OPTIND, OPTRESET) };
    // SAFETY: as above.
    let cursor = unsafe { &mut *&raw mut CURSOR };
    if reset != 0 || index <= 0 || index as usize != cursor.index {
        *cursor = Cursor {
            index: index.max(1) as usize,
            offset: 0,
        };
        // SAFETY: as above.
        unsafe { OPTRESET = 0 };
    }
    let found = getopt::next(word, spec, longs.clone(), !long.is_null(), cursor);
    // SAFETY: as above.
    unsafe { OPTIND = cursor.index as c_int };
    let value_pointer = |value: getopt::Value| match value {
        // SAFETY: the word index is below `count`, and the offset inside the word.
        Some((index, offset)) => unsafe { arguments.add(index).read().add(offset) },
        None => ptr::null_mut(),
    };
    let quiet = getopt::quiet(spec);
    // SAFETY: as above.
    let report = !quiet && unsafe { OPTERR } != 0;
    let missing = if quiet { b':' as c_int } else { b'?' as c_int };
    let set_error = |option: c_int| {
        // SAFETY: as above.
        unsafe { OPTOPT = option };
    };
    match found {
        Found::Done => -1,
        Found::Short(option, value) => {
            // SAFETY: as above.
            unsafe { OPTARG = value_pointer(value) };
            c_int::from(option)
        }
        Found::Long(which, value) => {
            // SAFETY: as above.
            unsafe { OPTARG = value_pointer(value) };
            if !long_index.is_null() {
                // SAFETY: the caller's promise.
                unsafe { long_index.write(which as c_int) };
            }
            // SAFETY: `which` is below `long_count`.
            let option = unsafe { &*long.add(which) };
            if option.flag.is_null() {
                option.value
            } else {
                // SAFETY: the program's `flag` points to an `int`.
                unsafe { option.flag.write(option.value) };
                0
            }
        }
        Found::UnknownShort(option) => {
            set_error(c_int::from(option));
            if report {
                message(&[b"illegal option -- ", &[option]]);
            }
            b'?' as c_int
        }
        Found::MissingShort(option) => {
            set_error(c_int::from(option));
            if report {
                message(&[b"option requires an argument -- ", &[option]]);
            }
            missing
        }
        Found::UnknownLong(at) | Found::AmbiguousLong(at) => {
            set_error(0);
            if report {
                let what: &[u8] = if matches!(found, Found::AmbiguousLong(_)) {
                    b"ambiguous option -- "
                } else {
                    b"unrecognized option `"
                };
                let name = word(at).unwrap_or_default();
                let name = name.split(|&b| b == b'=').next().unwrap_or_default();
                message(&[what, name, if what[0] == b'u' { b"'" } else { b"" }]);
            }
            b'?' as c_int
        }
        Found::MissingLong(which) | Found::UnexpectedLong(which) => {
            // SAFETY: `which` is below `long_count`.
            let option = unsafe { &*long.add(which) };
            set_error(if option.flag.is_null() { option.value } else { 0 });
            if report {
                // SAFETY: as above.
                let name = unsafe { c_string(option.name) };
                if matches!(found, Found::MissingLong(_)) {
                    message(&[b"option requires an argument -- ", name]);
                } else {
                    message(&[b"option doesn't take an argument -- ", name]);
                }
            }
            if matches!(found, Found::MissingLong(_)) {
                missing
            } else {
                b'?' as c_int
            }
        }
    }
}

/// # Safety
/// `arguments` must hold `count` NUL-terminated words and `spec` be NUL-terminated.
pub unsafe extern "C" fn getopt(
    count: c_int,
    arguments: *const *mut c_char,
    spec: *const c_char,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { parse(count, arguments, spec, ptr::null(), ptr::null_mut()) }
}

/// # Safety
/// As [`getopt`]; `long` must be an array of `struct option` ended by a null name, and
/// `long_index` null or writable.
pub unsafe extern "C" fn getopt_long(
    count: c_int,
    arguments: *const *mut c_char,
    spec: *const c_char,
    long: *const LongOption,
    long_index: *mut c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { parse(count, arguments, spec, long, long_index) }
}

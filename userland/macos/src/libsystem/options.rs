//! `getopt`, `getopt_long` and `getopt_long_only` over the program's `argv`, with BSD's
//! variables `optarg`, `optind`, `opterr`, `optopt` and `optreset`, and macOS's messages on
//! standard error. The long forms reorder `argv` as macOS's do (D190).

use super::{c_string, process, stdio};
use crate::getopt::{self, Cursor, Found, Kind, Long, Rules, Takes, Typed, Words};
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
static mut CURSOR: Cursor = Cursor::at(1);

/// `struct option`.
#[repr(C)]
#[derive(Debug)]
pub struct LongOption {
    name: *const c_char,
    has_arg: c_int,
    flag: *mut c_int,
    value: c_int,
}

struct Arguments {
    vector: *mut *mut c_char,
    count: usize,
}

impl Words for Arguments {
    fn count(&self) -> usize {
        self.count
    }

    fn word(&self, index: usize) -> &[u8] {
        // SAFETY: `parse`'s caller promised `count` NUL-terminated words.
        let pointer = unsafe { self.vector.wrapping_add(index).read() };
        // SAFETY: as above.
        unsafe { c_string(pointer) }
    }

    fn rotate(&mut self, start: usize, middle: usize, end: usize) {
        // SAFETY: as above; the long forms reorder `argv` as macOS's do, though it is declared
        // `char *const *`.
        let words = unsafe { core::slice::from_raw_parts_mut(self.vector, self.count) };
        words[start..end].rotate_left(middle - start);
    }
}

fn program_name() -> &'static [u8] {
    // SAFETY: `_NSGetProgname`'s pointer is set before the program runs.
    let program = unsafe { *process::ns_get_progname() };
    if program.is_null() {
        return b"";
    }
    // SAFETY: a NUL-terminated argument that lives as long as the program.
    unsafe { c_string(program) }
}

fn message(program: &[u8], parts: &[&[u8]]) {
    stdio::flush_all();
    if !program.is_empty() {
        let _ = linux::write_all(2, program);
        let _ = linux::write_all(2, b": ");
    }
    for part in parts {
        let _ = linux::write_all(2, part);
    }
    let _ = linux::write_all(2, b"\n");
}

fn typed_parts(words: &Arguments, typed: Typed) -> [&[u8]; 2] {
    let dashes: &[u8] = if typed.dashes == 2 { b"--" } else { b"-" };
    let name = words
        .word(typed.index)
        .get(typed.start..typed.end)
        .unwrap_or_default();
    [dashes, name]
}

unsafe fn parse(
    count: c_int,
    arguments: *const *mut c_char,
    spec: *const c_char,
    long: *const LongOption,
    long_index: *mut c_int,
    kind: Kind,
) -> c_int {
    // SAFETY: the caller's promise: a NUL-terminated option string.
    let spec = unsafe { c_string(spec) };
    let posixly_correct = kind != Kind::Short && {
        // SAFETY: a NUL-terminated name.
        let value = unsafe { process::getenv(c"POSIXLY_CORRECT".as_ptr()) };
        !value.is_null()
    };
    let rules = Rules::new(kind, spec, posixly_correct);
    let mut words = Arguments {
        vector: arguments.cast_mut(),
        count: count.max(0) as usize,
    };
    let long_count = if long.is_null() {
        0
    } else {
        // SAFETY: the caller's promise: an array ended by an entry with a null name.
        (0..)
            .take_while(|&index| !unsafe { (*long.wrapping_add(index)).name }.is_null())
            .count()
    };
    let longs = (0..long_count).map(|index| {
        // SAFETY: as above.
        let option = unsafe { &*long.wrapping_add(index) };
        Long {
            // SAFETY: as above.
            name: unsafe { c_string(option.name) },
            takes: Takes::from_c(option.has_arg),
            gives: (option.flag as usize, option.value),
        }
    });
    // SAFETY: the program's options state, which a program parses from one thread.
    let index = unsafe { OPTIND };
    // SAFETY: as above.
    let reset = unsafe { OPTRESET };
    let cursor = &raw mut CURSOR;
    // SAFETY: as above.
    let cursor = unsafe { &mut *cursor };
    if reset != 0 || index <= 0 || index as usize != cursor.index {
        *cursor = Cursor::at(index.max(1) as usize);
        // SAFETY: as above.
        unsafe { OPTRESET = 0 };
    }
    let found = getopt::next(&mut words, rules, longs, cursor);
    // SAFETY: as above.
    unsafe { OPTIND = cursor.index as c_int };
    let value_pointer = |value: getopt::Value| match value {
        Some((index, offset)) => {
            // SAFETY: the word index is below `count`.
            let word = unsafe { arguments.wrapping_add(index).read() };
            word.wrapping_add(offset)
        }
        None => ptr::null_mut(),
    };
    // SAFETY: as above.
    let report = !rules.quiet && unsafe { OPTERR } != 0;
    let missing = if rules.quiet {
        b':' as c_int
    } else {
        b'?' as c_int
    };
    let set_error = |option: c_int| {
        // SAFETY: as above.
        unsafe { OPTOPT = option };
    };
    let program: &[u8] = if kind == Kind::Short {
        if words.count > 0 { words.word(0) } else { b"" }
    } else {
        program_name()
    };
    let error_of = |which: usize| {
        // SAFETY: `which` is below `long_count`.
        let option = unsafe { &*long.wrapping_add(which) };
        if option.flag.is_null() {
            option.value
        } else {
            0
        }
    };
    match found {
        Found::Done => -1,
        Found::Operand(at) => {
            // SAFETY: as above.
            unsafe { OPTARG = value_pointer(Some((at, 0))) };
            1
        }
        Found::Short(option, value) => {
            if kind == Kind::Short {
                set_error(c_int::from(option));
            }
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
            let option = unsafe { &*long.wrapping_add(which) };
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
                let what: &[u8] = if kind == Kind::Short || posixly_correct {
                    b"illegal option -- "
                } else {
                    b"invalid option -- "
                };
                message(program, &[what, &[option]]);
            }
            b'?' as c_int
        }
        Found::MissingShort(option) => {
            set_error(c_int::from(option));
            if report {
                message(program, &[b"option requires an argument -- ", &[option]]);
            }
            missing
        }
        Found::UnknownLong(typed) | Found::AmbiguousLong(typed) => {
            set_error(0);
            if report {
                let [dashes, name] = typed_parts(&words, typed);
                if matches!(found, Found::UnknownLong(_)) {
                    message(program, &[b"unrecognized option `", dashes, name, b"'"]);
                } else {
                    message(program, &[b"option `", dashes, name, b"' is ambiguous"]);
                }
            }
            b'?' as c_int
        }
        Found::MissingLong(which, typed) => {
            set_error(error_of(which));
            if report {
                let [dashes, name] = typed_parts(&words, typed);
                message(
                    program,
                    &[b"option `", dashes, name, b"' requires an argument"],
                );
            }
            missing
        }
        Found::UnexpectedLong(which, typed) => {
            set_error(error_of(which));
            if report {
                let [dashes, name] = typed_parts(&words, typed);
                message(
                    program,
                    &[b"option `", dashes, name, b"' doesn't allow an argument"],
                );
            }
            b'?' as c_int
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
    unsafe {
        parse(
            count,
            arguments,
            spec,
            ptr::null(),
            ptr::null_mut(),
            Kind::Short,
        )
    }
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
    unsafe { parse(count, arguments, spec, long, long_index, Kind::Long) }
}

/// # Safety
/// As [`getopt_long`].
pub unsafe extern "C" fn getopt_long_only(
    count: c_int,
    arguments: *const *mut c_char,
    spec: *const c_char,
    long: *const LongOption,
    long_index: *mut c_int,
) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { parse(count, arguments, spec, long, long_index, Kind::LongOnly) }
}

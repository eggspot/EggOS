//! The standard library's failure reports a program calls, and its random bytes (D180):
//! `_assertionFailure` (the message after its file and line, as Swift prints a precondition or
//! a forced unwrap of `nil`), a call of a deleted method, an initializer a class does not
//! implement, then `abort` as the layer's other fatal errors; `swift_stdlib_random` from the
//! system's random source.

use super::grapheme;
use super::number_text;
use super::string::bytes_of;
use super::string_object::RawString;
use crate::libsystem::{abort, stdio, write_error};

fn report(parts: &[&[u8]]) -> ! {
    stdio::flush_all();
    for part in parts {
        write_error(part);
    }
    write_error(b"\n");
    abort()
}

const UNICODE_SCALAR: usize = 1;

fn static_string(start: usize, count: usize, flags: usize, scalar: &mut [u8; 4]) -> &[u8] {
    if flags & UNICODE_SCALAR != 0 {
        let length = grapheme::encode(start as u32, scalar);
        return &scalar[..length];
    }
    if start == 0 {
        return &[];
    }
    // SAFETY: a `StaticString` with a pointer names `count` bytes of the program's constants.
    unsafe { core::slice::from_raw_parts(start as *const u8, count) }
}

fn message_parts<'a>(prefix: &'a [u8], message: &'a [u8]) -> [&'a [u8]; 3] {
    if message.is_empty() {
        [prefix, b"", b""]
    } else {
        [prefix, b": ", message]
    }
}

#[allow(clippy::too_many_arguments)]
extern "C" fn assertion_failure_in_file(
    prefix_start: usize,
    prefix_count: usize,
    prefix_flags: usize,
    message_count_and_flags: u64,
    message_object: u64,
    file_start: usize,
    file_count: usize,
    file_flags: usize,
    line: usize,
    _flags: u32,
) -> ! {
    let (mut prefix_scalar, mut file_scalar) = ([0u8; 4], [0u8; 4]);
    let prefix = static_string(prefix_start, prefix_count, prefix_flags, &mut prefix_scalar);
    let file = static_string(file_start, file_count, file_flags, &mut file_scalar);
    let message = RawString {
        count_and_flags: message_count_and_flags,
        object: message_object,
    };
    let mut scratch = [0u8; 16];
    let message = bytes_of(message, &mut scratch);
    let line = number_text::unsigned(line as u64);
    let [prefix, separator, message] = message_parts(prefix, message);
    report(&[
        file,
        b":",
        line.as_bytes(),
        b": ",
        prefix,
        separator,
        message,
    ])
}

extern "C" fn assertion_failure(
    prefix_start: usize,
    prefix_count: usize,
    prefix_flags: usize,
    message_count_and_flags: u64,
    message_object: u64,
    _flags: u32,
) -> ! {
    let mut prefix_scalar = [0u8; 4];
    let prefix = static_string(prefix_start, prefix_count, prefix_flags, &mut prefix_scalar);
    let message = RawString {
        count_and_flags: message_count_and_flags,
        object: message_object,
    };
    let mut scratch = [0u8; 16];
    let message = bytes_of(message, &mut scratch);
    let [prefix, separator, message] = message_parts(prefix, message);
    report(&[prefix, separator, message])
}

extern "C" fn deleted_method_error() -> ! {
    super::fatal_error(&[b"Call of deleted method"])
}

fn counted(start: *const u8, length: i32) -> &'static [u8] {
    if start.is_null() || length <= 0 {
        return &[];
    }
    // SAFETY: the names Swift passes are `length` bytes of the program's constants.
    unsafe { core::slice::from_raw_parts(start, length as usize) }
}

extern "C" fn report_unimplemented_initializer(
    class_name: *const u8,
    class_length: i32,
    initializer: *const u8,
    initializer_length: i32,
) -> ! {
    super::fatal_error(&[
        b"Use of unimplemented initializer '",
        counted(initializer, initializer_length),
        b"' for class '",
        counted(class_name, class_length),
        b"'",
    ])
}

extern "C" fn stdlib_random(buffer: *mut u8, count: usize) {
    // SAFETY: the program's buffer of `count` bytes.
    unsafe { crate::libsystem::process::arc4random_buf(buffer.cast(), count) };
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_$ss17_assertionFailure__4file4line5flagss5NeverOs12StaticStringV_SSAHSus6UInt32VtF" => {
            assertion_failure_in_file as *const () as usize
        }
        b"_$ss17_assertionFailure__5flagss5NeverOs12StaticStringV_SSs6UInt32VtF" => {
            assertion_failure as *const () as usize
        }
        b"_swift_deletedMethodError" => deleted_method_error as *const () as usize,
        b"__swift_stdlib_reportUnimplementedInitializer"
        | b"__swift_stdlib_reportUnimplementedInitializerInFile" => {
            report_unimplemented_initializer as *const () as usize
        }
        b"_swift_stdlib_random" => stdlib_random as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

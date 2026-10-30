//! `print(_:separator:terminator:)` (D180): each item, an `Any`, is written by its type
//! (strings as they are, the integers, `Double` and `Bool` by their descriptions), separated and
//! terminated, to the standard output stream the C library's `printf` uses, so their output
//! interleaves in order; and `CommandLine.argc`.

use super::metadata::{self, Metadata, address_of, kind, project_box, witnesses, word};
use super::number_text;
use super::storage;
use super::string::bytes_of;
use super::string_object::RawString;
use crate::libsystem::stdio;

const ANY_SIZE: usize = 32;
const ANY_TYPE: usize = 24;

fn write(bytes: &[u8]) {
    stdio::write_standard_output(bytes);
}

fn write_string(raw: RawString) {
    let mut scratch = [0u8; 16];
    write(bytes_of(raw, &mut scratch));
}

fn read<T: Copy>(value: usize) -> T {
    // SAFETY: the value an `Any` holds, of the type its metadata names.
    unsafe { (value as *const T).read_unaligned() }
}

fn write_value(mut value: usize, mut stored: Metadata) {
    while metadata::kind_of(stored) == kind::EXISTENTIAL
        && word(stored + 8) as u32 & metadata::OPAQUE_EXISTENTIAL != 0
    {
        let inner = word(value + ANY_TYPE);
        value = contents(value, inner);
        stored = inner;
    }
    let is = |known: &'static metadata::StructMetadata| stored == address_of(known);
    if is(&metadata::STRING) {
        write_string(read(value));
    } else if is(&metadata::INT) || is(&metadata::INT64) {
        write(number_text::signed(read(value)).as_bytes());
    } else if is(&metadata::INT32) {
        write(number_text::signed(i64::from(read::<i32>(value))).as_bytes());
    } else if is(&metadata::INT16) {
        write(number_text::signed(i64::from(read::<i16>(value))).as_bytes());
    } else if is(&metadata::INT8) {
        write(number_text::signed(i64::from(read::<i8>(value))).as_bytes());
    } else if is(&metadata::UINT) || is(&metadata::UINT64) {
        write(number_text::unsigned(read(value)).as_bytes());
    } else if is(&metadata::UINT32) {
        write(number_text::unsigned(u64::from(read::<u32>(value))).as_bytes());
    } else if is(&metadata::UINT16) {
        write(number_text::unsigned(u64::from(read::<u16>(value))).as_bytes());
    } else if is(&metadata::UINT8) {
        write(number_text::unsigned(u64::from(read::<u8>(value))).as_bytes());
    } else if is(&metadata::DOUBLE) {
        write(number_text::double(read(value)).as_bytes());
    } else if is(&metadata::BOOL) {
        write(if read::<u8>(value) & 1 != 0 {
            b"true"
        } else {
            b"false"
        });
    } else {
        super::unimplemented(&[
            b"printing a value of a type other than String, Bool and the numbers",
        ]);
    }
}

fn contents(container: usize, stored: Metadata) -> usize {
    let table = witnesses(stored);
    if table.is_inline() {
        container
    } else {
        project_box(word(container), table.alignment_mask())
    }
}

extern "C" fn print(
    items: usize,
    separator_count_and_flags: u64,
    separator_object: u64,
    terminator_count_and_flags: u64,
    terminator_object: u64,
) {
    let separator = RawString {
        count_and_flags: separator_count_and_flags,
        object: separator_object,
    };
    let terminator = RawString {
        count_and_flags: terminator_count_and_flags,
        object: terminator_object,
    };
    let storage = storage::array_storage(items);
    let start = storage + 32;
    for index in 0..storage::count(storage) {
        if index > 0 {
            write_string(separator);
        }
        let container = start + index * ANY_SIZE;
        let stored = word(container + ANY_TYPE);
        write_value(contents(container, stored), stored);
    }
    write_string(terminator);
}

extern "C" fn argument_count() -> i32 {
    crate::libsystem::process::arguments().0 as i32
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    match name {
        b"_$ss5print_9separator10terminatoryypd_S2StF" => Some(print as *const () as u64),
        b"_$ss11CommandLineO4argcs5Int32VvgZ" => Some(argument_count as *const () as u64),
        _ => None,
    }
}

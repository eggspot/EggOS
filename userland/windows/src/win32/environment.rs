//! The environment: variables by name (`GetEnvironmentVariableW`/`A`, names compared ignoring
//! ASCII case as Windows compares them) and the whole block (`GetEnvironmentStringsW`/`A`),
//! both read from the environment the layer was started with.

use super::process::process;
use super::{Bool, TRUE, error, fail};
use crate::library::Export;
use crate::text;

fn value_of<'a>(name: impl Iterator<Item = u8> + Clone) -> Option<&'a [u8]> {
    process().startup.environment().find_map(|entry| {
        let entry = entry.to_bytes();
        let equals = entry.iter().position(|byte| *byte == b'=')?;
        text::equal_ignoring_ascii_case(entry[..equals].iter().copied(), name.clone())
            .then(|| &entry[equals + 1..])
    })
}

unsafe extern "win64" fn get_environment_variable_w(
    name: *const u16,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    // SAFETY: `GetEnvironmentVariableW`'s contract: a NUL-terminated UTF-16 name.
    let name = unsafe { super::file::wide_string(name) };
    let ascii = name.iter().map(|unit| u8::try_from(*unit).unwrap_or(0xff));
    let Some(value) = value_of(ascii) else {
        return fail(error::ENVVAR_NOT_FOUND, 0);
    };
    let length = text::utf16_length(value);
    if length >= size as usize || buffer.is_null() {
        return length as u32 + 1;
    }
    // SAFETY: the caller passes `size` writable units, more than the value and its NUL take.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    let written = text::write_utf16(value, out);
    out[written] = 0;
    super::thread_environment::set_last_error(error::SUCCESS);
    written as u32
}

unsafe extern "win64" fn get_environment_variable_a(
    name: *const u8,
    buffer: *mut u8,
    size: u32,
) -> u32 {
    // SAFETY: `GetEnvironmentVariableA`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    let Some(value) = value_of(name.iter().copied()) else {
        return fail(error::ENVVAR_NOT_FOUND, 0);
    };
    if value.len() >= size as usize || buffer.is_null() {
        return value.len() as u32 + 1;
    }
    // SAFETY: the caller passes `size` writable bytes, more than the value and its NUL take.
    let out = unsafe { core::slice::from_raw_parts_mut(buffer, size as usize) };
    out[..value.len()].copy_from_slice(value);
    out[value.len()] = 0;
    super::thread_environment::set_last_error(error::SUCCESS);
    value.len() as u32
}

extern "win64" fn get_environment_strings_w() -> *const u16 {
    process().environment_wide.as_ptr()
}

extern "win64" fn get_environment_strings_a() -> *const u8 {
    process().environment_ansi.as_ptr()
}

extern "win64" fn free_environment_strings(_block: usize) -> Bool {
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetEnvironmentVariableW" => get_environment_variable_w,
    b"GetEnvironmentVariableA" => get_environment_variable_a,
    b"GetEnvironmentStringsW" => get_environment_strings_w,
    b"GetEnvironmentStringsA" => get_environment_strings_a,
    b"GetEnvironmentStrings" => get_environment_strings_a,
    b"FreeEnvironmentStringsW" => free_environment_strings,
    b"FreeEnvironmentStringsA" => free_environment_strings,
];

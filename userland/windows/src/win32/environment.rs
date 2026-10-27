//! The environment: variables by name (`GetEnvironmentVariableW`/`A`, names compared ignoring
//! ASCII case as Windows compares them), changed with `SetEnvironmentVariableW`/`A`, and the
//! whole block (`GetEnvironmentStringsW`/`A`, a copy the caller frees). It starts as the
//! environment the layer was started with; each changed variable is a heap block
//! `NAME=value` in a table the process's threads share.

use super::process::process;
use super::{Bool, FALSE, SpinLock, TRUE, error, fail, heap};
use crate::library::Export;
use crate::text;

const MAX_VARIABLES: usize = 512;
const NAME_CAPACITY: usize = 1024;

struct Variables {
    loaded: bool,
    count: usize,
    entries: [(usize, usize); MAX_VARIABLES],
}

// SAFETY: the entries are heap blocks reached only under the table's lock.
unsafe impl Send for Variables {}

static VARIABLES: SpinLock<Variables> = SpinLock::new(Variables {
    loaded: false,
    count: 0,
    entries: [(0, 0); MAX_VARIABLES],
});

fn copy(bytes: &[u8]) -> Option<(usize, usize)> {
    let block = heap::allocate(bytes.len().max(1));
    if block.is_null() {
        return None;
    }
    // SAFETY: a fresh heap block of at least `bytes.len()` bytes.
    unsafe { block.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
    Some((block as usize, bytes.len()))
}

fn entry_bytes<'a>(&(address, length): &(usize, usize)) -> &'a [u8] {
    // SAFETY: each entry is a live heap block of `length` bytes, which only changes under the
    // table's lock that the caller holds.
    unsafe { core::slice::from_raw_parts(address as *const u8, length) }
}

fn with_variables<R>(work: impl FnOnce(&mut Variables) -> R) -> R {
    VARIABLES.with(|variables| {
        if !variables.loaded {
            variables.loaded = true;
            for entry in process().startup.environment() {
                if variables.count == MAX_VARIABLES {
                    break;
                }
                if let Some(entry) = copy(entry.to_bytes()) {
                    variables.entries[variables.count] = entry;
                    variables.count += 1;
                }
            }
        }
        work(variables)
    })
}

fn position(variables: &Variables, name: &[u8]) -> Option<usize> {
    variables.entries[..variables.count]
        .iter()
        .position(|entry| {
            let entry = entry_bytes(entry);
            let equals = entry.iter().position(|byte| *byte == b'=');
            equals.is_some_and(|equals| entry[..equals].eq_ignore_ascii_case(name))
        })
}

fn value_of<R>(name: &[u8], work: impl FnOnce(Option<&[u8]>) -> R) -> R {
    with_variables(|variables| {
        let value = position(variables, name).map(|index| {
            let entry = entry_bytes(&variables.entries[index]);
            let equals = entry.iter().position(|byte| *byte == b'=').unwrap_or(0);
            &entry[equals + 1..]
        });
        work(value)
    })
}

fn set(name: &[u8], value: Option<&[u8]>) -> Bool {
    if name.is_empty() || name.contains(&b'=') {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let new = match value {
        Some(value) => {
            let mut entry = [0u8; NAME_CAPACITY * 4];
            let length = name.len() + 1 + value.len();
            if length > entry.len() {
                return fail(error::NOT_ENOUGH_MEMORY, FALSE);
            }
            entry[..name.len()].copy_from_slice(name);
            entry[name.len()] = b'=';
            entry[name.len() + 1..length].copy_from_slice(value);
            match copy(&entry[..length]) {
                Some(entry) => Some(entry),
                None => return fail(error::NOT_ENOUGH_MEMORY, FALSE),
            }
        }
        None => None,
    };
    let old = with_variables(|variables| {
        let index = position(variables, name);
        match (index, new) {
            (Some(index), Some(new)) => {
                Some(core::mem::replace(&mut variables.entries[index], new))
            }
            (Some(index), None) => {
                variables.count -= 1;
                let last = variables.entries[variables.count];
                Some(core::mem::replace(&mut variables.entries[index], last))
            }
            (None, Some(new)) if variables.count < MAX_VARIABLES => {
                variables.entries[variables.count] = new;
                variables.count += 1;
                None
            }
            (None, Some(new)) => Some(new),
            (None, None) => None,
        }
    });
    if let Some((address, _)) = old {
        // SAFETY: the entry left the table (or never entered it), so nothing reaches it.
        unsafe { heap::free(address as *mut u8) };
    }
    TRUE
}

unsafe fn utf8_name(name: *const u16, out: &mut [u8; NAME_CAPACITY]) -> Option<usize> {
    if name.is_null() {
        return None;
    }
    // SAFETY: the caller's duty.
    let name = unsafe { super::file::wide_string(name) };
    let (written, consumed) = text::write_utf8(name, out);
    (consumed == name.len()).then_some(written)
}

unsafe extern "efiapi" fn get_environment_variable_w(
    name: *const u16,
    buffer: *mut u16,
    size: u32,
) -> u32 {
    let mut bytes = [0u8; NAME_CAPACITY];
    // SAFETY: `GetEnvironmentVariableW`'s contract: a NUL-terminated UTF-16 name.
    let Some(length) = (unsafe { utf8_name(name, &mut bytes) }) else {
        return fail(error::ENVVAR_NOT_FOUND, 0);
    };
    value_of(&bytes[..length], |value| {
        let Some(value) = value else {
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
    })
}

unsafe extern "efiapi" fn get_environment_variable_a(
    name: *const u8,
    buffer: *mut u8,
    size: u32,
) -> u32 {
    // SAFETY: `GetEnvironmentVariableA`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    value_of(name, |value| {
        let Some(value) = value else {
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
    })
}

unsafe extern "efiapi" fn set_environment_variable_w(name: *const u16, value: *const u16) -> Bool {
    let mut name_bytes = [0u8; NAME_CAPACITY];
    // SAFETY: `SetEnvironmentVariableW`'s contract: a NUL-terminated UTF-16 name.
    let Some(name_length) = (unsafe { utf8_name(name, &mut name_bytes) }) else {
        return fail(error::INVALID_PARAMETER, FALSE);
    };
    if value.is_null() {
        return set(&name_bytes[..name_length], None);
    }
    // SAFETY: as above: a NUL-terminated UTF-16 value.
    let value = unsafe { super::file::wide_string(value) };
    let mut bytes = [0u8; NAME_CAPACITY * 3];
    let (written, consumed) = text::write_utf8(value, &mut bytes);
    if consumed < value.len() {
        return fail(error::NOT_ENOUGH_MEMORY, FALSE);
    }
    set(&name_bytes[..name_length], Some(&bytes[..written]))
}

unsafe extern "efiapi" fn set_environment_variable_a(name: *const u8, value: *const u8) -> Bool {
    if name.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `SetEnvironmentVariableA`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    let value = match value.is_null() {
        true => None,
        // SAFETY: as above: a NUL-terminated value.
        false => Some(unsafe { core::ffi::CStr::from_ptr(value.cast()) }.to_bytes()),
    };
    set(name, value)
}

fn environment_block<T: Copy + Default>(
    length_of: fn(&[u8]) -> usize,
    write: fn(&[u8], &mut [T]) -> usize,
) -> *mut T {
    with_variables(|variables| {
        let entries = &variables.entries[..variables.count];
        let units: usize = entries
            .iter()
            .map(|entry| length_of(entry_bytes(entry)) + 1)
            .sum::<usize>()
            + 1;
        let block = heap::allocate(units * core::mem::size_of::<T>()).cast::<T>();
        if block.is_null() {
            return fail(error::NOT_ENOUGH_MEMORY, block);
        }
        // SAFETY: a fresh, zeroed heap block of `units` `T`s (bytes or UTF-16 units).
        let out = unsafe { core::slice::from_raw_parts_mut(block, units) };
        let mut at = 0;
        for entry in entries {
            at += write(entry_bytes(entry), &mut out[at..]) + 1;
        }
        block
    })
}

fn write_bytes(bytes: &[u8], out: &mut [u8]) -> usize {
    out[..bytes.len()].copy_from_slice(bytes);
    bytes.len()
}

extern "efiapi" fn get_environment_strings_w() -> *mut u16 {
    environment_block(text::utf16_length, text::write_utf16)
}

extern "efiapi" fn get_environment_strings_a() -> *mut u8 {
    environment_block(<[u8]>::len, write_bytes)
}

unsafe extern "efiapi" fn free_environment_strings(block: *mut u8) -> Bool {
    // SAFETY: `FreeEnvironmentStrings`' contract: a block `GetEnvironmentStrings` returned.
    unsafe { heap::free(block) };
    TRUE
}

pub(super) const EXPORTS: &[Export] = exports![
    b"GetEnvironmentVariableW" => get_environment_variable_w,
    b"GetEnvironmentVariableA" => get_environment_variable_a,
    b"SetEnvironmentVariableW" => set_environment_variable_w,
    b"SetEnvironmentVariableA" => set_environment_variable_a,
    b"GetEnvironmentStringsW" => get_environment_strings_w,
    b"GetEnvironmentStringsA" => get_environment_strings_a,
    b"GetEnvironmentStrings" => get_environment_strings_a,
    b"FreeEnvironmentStringsW" => free_environment_strings,
    b"FreeEnvironmentStringsA" => free_environment_strings,
];

//! `NSLog` and the class and selector name functions. `NSLog` writes one line to standard
//! error, `YYYY-MM-DD HH:MM:SS.mmm name[pid:tid] message`, in UTC because the layer has no time
//! zones yet (D123); each call formats into its own buffer, since a `description` it calls
//! may log too.

use super::string::{Formatted, format_with};
use super::{string_from_utf8, with_utf8};
use crate::calendar;
use crate::libsystem::variadic::{RawVaList, variadic};
use crate::libsystem::{process, stdio, thread, write_error};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::class;

fn two(out: &mut Buffer<u8>, value: i64, digits: usize) {
    let mut text = [0u8; 24];
    let decimal = crate::format::decimal(value.unsigned_abs(), &mut text);
    for _ in decimal.len()..digits {
        out.push(b'0');
    }
    for &byte in decimal {
        out.push(byte);
    }
}

fn prefix(out: &mut Buffer<u8>) {
    let [seconds, nanoseconds] = crate::linux::clock_time(0).unwrap_or([0, 0]);
    if let Some(time) = calendar::from_seconds(seconds) {
        two(out, i64::from(time.year) + 1900, 4);
        out.push(b'-');
        two(out, i64::from(time.month) + 1, 2);
        out.push(b'-');
        two(out, i64::from(time.day), 2);
        out.push(b' ');
        two(out, i64::from(time.hour), 2);
        out.push(b':');
        two(out, i64::from(time.minute), 2);
        out.push(b':');
        two(out, i64::from(time.second), 2);
        out.push(b'.');
        two(out, nanoseconds / 1_000_000, 3);
        out.push(b' ');
    }
    // SAFETY: `_NSGetProgname`'s variable, set before the program started.
    let name = unsafe { process::ns_get_progname().read() };
    if !name.is_null() {
        // SAFETY: the program's name, NUL-terminated.
        for &byte in unsafe { crate::objc::selector::name_bytes(name.cast()) } {
            out.push(byte);
        }
    }
    out.push(b'[');
    two(out, crate::linux::process_id() as i64, 1);
    out.push(b':');
    two(out, i64::from(thread::current_id()), 1);
    out.push(b']');
    out.push(b' ');
}

extern "C" fn ns_logv(format: Id, arguments: RawVaList) {
    let Formatted(mut message) = format_with(format, arguments);
    let mut line = Buffer::new();
    prefix(&mut line);
    line.reserve(message.len() + 1);
    for &byte in message.as_slice() {
        line.push(byte);
    }
    if message.as_slice().last() != Some(&b'\n') {
        line.push(b'\n');
    }
    stdio::flush_all();
    write_error(line.as_slice());
    line.free();
    message.free();
}

variadic!(eggos_ns_log => ns_logv, 1, "x1", "rsi");

extern "C" fn ns_string_from_class(class: *mut ClassObject) -> Id {
    match class::record_of(class) {
        Some(record) => string_from_utf8(record.name()),
        None => core::ptr::null_mut(),
    }
}

extern "C" fn ns_class_from_string(name: Id) -> *mut ClassObject {
    if name.is_null() {
        return core::ptr::null_mut();
    }
    with_utf8(name, crate::objc::class_named)
}

extern "C" fn ns_string_from_selector(sel: Sel) -> Id {
    if sel.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: a selector is its canonical, permanent name.
    string_from_utf8(unsafe { class::abi_name(sel) })
}

extern "C" fn ns_selector_from_string(name: Id) -> Sel {
    if name.is_null() {
        return core::ptr::null();
    }
    with_utf8(name, |bytes| {
        crate::objc::selector_named(bytes).unwrap_or(core::ptr::null())
    })
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_NSLog" => eggos_ns_log as *const () as usize,
        b"_NSLogv" => ns_logv as *const () as usize,
        b"_NSStringFromClass" => ns_string_from_class as *const () as usize,
        b"_NSClassFromString" => ns_class_from_string as *const () as usize,
        b"_NSStringFromSelector" => ns_string_from_selector as *const () as usize,
        b"_NSSelectorFromString" => ns_selector_from_string as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

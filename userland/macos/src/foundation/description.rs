//! `description` of arrays and dictionaries, laid out as macOS lays them out: at nesting level
//! `n`, a collection starts with `4n` spaces and its entries are indented `4(n + 1)`, a nested
//! collection coming after its entry's indentation; other objects are their `description`,
//! quoted as `string_text` says; a dictionary whose keys are all strings lists them in order.
//! Also NSObject's `description`, which needs a string: `<Class: 0x...>`.

use super::string_text::{self, Text};
use super::{is_array, is_dictionary, is_string, string_from_utf8, text_of};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::root::{self, NSOBJECT};
use crate::objc::sel;
use crate::objc::{Runtime, class, hexadecimal, methods, send};

fn write(out: &mut Buffer<u8>, bytes: &[u8]) {
    if out.reserve(bytes.len()) {
        for &byte in bytes {
            out.push(byte);
        }
    }
}

fn pad(out: &mut Buffer<u8>, level: usize) {
    for _ in 0..level {
        write(out, b"    ");
    }
}

fn leaf(out: &mut Buffer<u8>, object: Id) {
    let text: Text<'static> = if is_string(object) {
        text_of(object)
    } else {
        text_of(send(object, sel!("description")))
    };
    string_text::write_quoted(&text, &mut |piece| write(out, piece));
}

fn entry(out: &mut Buffer<u8>, object: Id, level: usize) {
    if is_array(object) || is_dictionary(object) {
        collection(out, object, level);
    } else {
        leaf(out, object);
    }
}

fn collection(out: &mut Buffer<u8>, object: Id, level: usize) {
    pad(out, level);
    if is_array(object) {
        write(out, b"(\n");
        let mut index = 0;
        while index < super::array::count_of(object) {
            pad(out, level + 1);
            entry(out, super::array::item(object, index), level + 1);
            index += 1;
            if index < super::array::count_of(object) {
                write(out, b",");
            }
            write(out, b"\n");
        }
        pad(out, level);
        write(out, b")");
        return;
    }
    write(out, b"{\n");
    let count = super::dictionary::count_of(object);
    let mut order: Buffer<usize> = Buffer::new();
    for index in 0..count {
        order.push(index);
    }
    let all_strings = (0..count).all(|index| is_string(super::dictionary::key(object, index)));
    if all_strings {
        let order = order.as_mut_slice();
        for next in 1..order.len() {
            let mut at = next;
            while at > 0 {
                let left = text_of(super::dictionary::key(object, order[at - 1]));
                let right = text_of(super::dictionary::key(object, order[at]));
                if left.compare(&right) != core::cmp::Ordering::Greater {
                    break;
                }
                order.swap(at - 1, at);
                at -= 1;
            }
        }
    }
    for &index in order.as_slice() {
        if index >= super::dictionary::count_of(object) {
            continue;
        }
        pad(out, level + 1);
        leaf(out, super::dictionary::key(object, index));
        write(out, b" = ");
        if index >= super::dictionary::count_of(object) {
            write(out, b"(removed);\n");
            continue;
        }
        entry(out, super::dictionary::value(object, index), level + 1);
        write(out, b";\n");
    }
    order.free();
    pad(out, level);
    write(out, b"}");
}

/// The description of the array or dictionary `object` at nesting level `level`, autoreleased.
pub fn describe(object: Id, level: usize) -> Id {
    let mut out = Buffer::new();
    collection(&mut out, object, level);
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn object_description(this: Id, _: Sel) -> Id {
    let mut out = Buffer::new();
    let mut digits = [0u8; 18];
    write(&mut out, b"<");
    write(&mut out, root::class_name(this));
    write(&mut out, b": ");
    write(&mut out, hexadecimal(this as usize, &mut digits));
    write(&mut out, b">");
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn debug_description(this: Id, _: Sel) -> Id {
    send(this, sel!("description"))
}

extern "C" fn class_description(this: Id, _: Sel) -> Id {
    let name = class::record_of(this.cast()).map_or(&b"?"[..], |record| record.name());
    string_from_utf8(name)
}

/// Adds Foundation's NSObject methods: `description` and `debugDescription`.
pub fn extend_object(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "description" => object_description,
        "debugDescription" => debug_description,
    ];
    let class_methods = methods![
        "description" => class_description,
        "debugDescription" => class_description,
    ];
    class::extend(runtime, &NSOBJECT, &instance, &class_methods)
}

//! `_typeName(_:qualified:)` (D180), which `print(type(of: x))` and the cast failure messages
//! use: a nominal type's name from its descriptor, prefixed by the types it is nested in and,
//! qualified, its module (`Swift.Int`), with its generic arguments (`Optional<Int>`); an
//! Objective-C class's own name; `Any`, `AnyObject` and protocol compositions; tuples.

use super::cast::is_swift_class;
use super::metadata::{
    self, ARRAY_DESCRIPTOR, CLASS_DESCRIPTION, DICTIONARY_DESCRIPTOR, Metadata, OPAQUE_EXISTENTIAL,
    OPTIONAL_DESCRIPTOR, SET_DESCRIPTOR, context, kind, relative, word, word32,
};
use super::string::make;
use super::string_object::RawString;
use crate::objc::abi::ClassObject;
use crate::objc::class;

const LONGEST: usize = 256;

/// A type's name, as much of it as fits.
#[derive(Debug)]
pub struct Name {
    bytes: [u8; LONGEST],
    length: usize,
}

impl Name {
    fn push(&mut self, text: &[u8]) {
        let taken = text.len().min(LONGEST - self.length);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text[..taken]);
        self.length += taken;
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn c_string(address: usize) -> &'static [u8] {
    let start = address as *const u8;
    let mut length = 0;
    // SAFETY: descriptor names are NUL-terminated strings in the program's or the runtime's
    // constant data; the scan stops at the longest name kept.
    while length < LONGEST && unsafe { start.wrapping_add(length).read() } != 0 {
        length += 1;
    }
    // SAFETY: the bytes just scanned.
    unsafe { core::slice::from_raw_parts(start, length) }
}

fn parent_of(descriptor: usize) -> Option<usize> {
    if word32(descriptor + 4) == 0 {
        return None;
    }
    let target = relative(descriptor + 4);
    Some(if target & 1 != 0 {
        word(target & !1)
    } else {
        target
    })
}

fn write_context(name: &mut Name, descriptor: usize, qualified: bool) -> bool {
    let kind = word32(descriptor) & context::KIND_MASK;
    if kind == context::MODULE {
        if qualified {
            name.push(c_string(relative(descriptor + 8)));
        }
        return qualified;
    }
    let wrote = parent_of(descriptor).is_some_and(|parent| write_context(name, parent, qualified));
    if !matches!(
        kind,
        context::PROTOCOL | context::CLASS | context::STRUCT | context::ENUM
    ) {
        return wrote;
    }
    if wrote {
        name.push(b".");
    }
    name.push(c_string(relative(descriptor + 8)));
    true
}

const GENERIC_PARAMETER_COUNT: usize = 36;

fn generic_argument_count(descriptor: usize) -> usize {
    if descriptor == DICTIONARY_DESCRIPTOR.address() {
        return 2;
    }
    let ours = [
        OPTIONAL_DESCRIPTOR.address(),
        ARRAY_DESCRIPTOR.address(),
        SET_DESCRIPTOR.address(),
    ];
    if ours.contains(&descriptor) {
        return 1;
    }
    if word32(descriptor) & context::GENERIC == 0 {
        return 0;
    }
    (word32(descriptor + GENERIC_PARAMETER_COUNT) & 0xffff) as usize
}

fn write_nominal(name: &mut Name, metadata: Metadata, qualified: bool) {
    let descriptor = word(metadata + 8);
    write_context(name, descriptor, qualified);
    let count = generic_argument_count(descriptor);
    if count == 0 {
        return;
    }
    name.push(b"<");
    for index in 0..count {
        if index > 0 {
            name.push(b", ");
        }
        write_type(name, word(metadata + 16 + 8 * index), qualified);
    }
    name.push(b">");
}

fn write_objc_class(name: &mut Name, class: usize) {
    let record = class::record_of(class as *const ClassObject);
    name.push(record.map_or(b"?", |record| record.name()));
}

fn write_existential(name: &mut Name, metadata: Metadata, qualified: bool) {
    let flags = word32(metadata + 8);
    let protocols = word32(metadata + 12) as usize;
    if protocols == 0 {
        name.push(if flags & OPAQUE_EXISTENTIAL != 0 {
            b"Any"
        } else {
            b"AnyObject"
        });
        return;
    }
    for index in 0..protocols {
        if index > 0 {
            name.push(b" & ");
        }
        let protocol = word(metadata + 16 + 8 * index);
        if protocol & 1 != 0 {
            super::unimplemented(&[b"names of Objective-C protocol compositions"]);
        }
        write_context(name, protocol, qualified);
    }
}

fn write_tuple(name: &mut Name, metadata: Metadata, qualified: bool) {
    name.push(b"(");
    for index in 0..word(metadata + 8) {
        if index > 0 {
            name.push(b", ");
        }
        write_type(name, word(metadata + 24 + 16 * index), qualified);
    }
    name.push(b")");
}

fn write_type(name: &mut Name, metadata: Metadata, qualified: bool) {
    match metadata::kind_of(metadata) {
        kind::CLASS if is_swift_class(metadata) => {
            write_context(name, word(metadata + CLASS_DESCRIPTION), qualified);
        }
        kind::CLASS => write_objc_class(name, metadata),
        kind::OBJC_CLASS_WRAPPER => write_objc_class(name, word(metadata + 8)),
        kind::STRUCT | kind::ENUM | kind::OPTIONAL | kind::FOREIGN_CLASS => {
            write_nominal(name, metadata, qualified);
        }
        kind::EXISTENTIAL => write_existential(name, metadata, qualified),
        kind::TUPLE => write_tuple(name, metadata, qualified),
        _ => super::unimplemented(&[b"names of function types and metatypes"]),
    }
}

/// The name of the type `metadata`, with its module when `qualified`.
pub fn name_of(metadata: Metadata, qualified: bool) -> Name {
    let mut name = Name {
        bytes: [0; LONGEST],
        length: 0,
    };
    write_type(&mut name, metadata, qualified);
    name
}

extern "C" fn type_name(metadata: Metadata, qualified: u32) -> RawString {
    make(name_of(metadata, qualified & 1 != 0).as_bytes())
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    (name == b"_$ss9_typeName_9qualifiedSSypXp_SbtF").then_some(type_name as *const () as u64)
}

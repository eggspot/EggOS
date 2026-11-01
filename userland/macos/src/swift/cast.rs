//! Dynamic casts and the metadata of classes Swift did not define (D180). `swift_dynamicCast`
//! opens existentials and optionals on the way in, wraps optionals and `Any` and `AnyObject`
//! on the way out, checks class instances against a class's ancestry, and bridges Foundation's
//! NSNumber and NSString to Swift's numbers, `Bool` and `String` as `as?` does (a number only
//! when the value fits exactly). An Objective-C class's Swift metadata is a wrapper made once
//! per class; a Core Foundation type's is the program's own candidate.

use super::generic;
use super::heap_object::unknown_object_retain;
use super::metadata::{
    self, ANY_SWIFT_CLASS, CGFLOAT, Metadata, OPAQUE_EXISTENTIAL, UNKNOWN_OBJECT_WITNESSES,
    address_of, copy_value, destroy_value, kind, project_box, set_word, witnesses, word, word32,
};
use super::string::make;
use super::string_object::RawString;
use super::type_lookup::Response;
use super::type_name;
use crate::foundation::{self, Exact};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::{ClassObject, Id};
use crate::objc::buffer::Buffer;
use crate::objc::{class, root};

/// Whether `class` (an Objective-C class object) is a Swift class.
pub fn is_swift_class(class: usize) -> bool {
    word(class + 32) & ANY_SWIFT_CLASS != 0
}

static WRAPPERS: Exclusive<Buffer<(usize, Metadata)>> = Exclusive::new(Buffer::new());

fn objc_wrapper(class: usize) -> Metadata {
    WRAPPERS.with(|wrappers| {
        if let Some(&(_, found)) = wrappers.as_slice().iter().find(|(seen, _)| *seen == class) {
            return found;
        }
        let block = heap::allocate(24, true) as usize;
        if block == 0 {
            super::fatal_error(&[b"out of memory for Swift metadata"]);
        }
        set_word(block, &raw const UNKNOWN_OBJECT_WITNESSES as usize);
        set_word(block + 8, kind::OBJC_CLASS_WRAPPER);
        set_word(block + 16, class);
        if !wrappers.push((class, block + 8)) {
            super::fatal_error(&[b"out of memory for Swift metadata"]);
        }
        block + 8
    })
}

/// `swift_getObjCClassMetadata`: a Swift class's own metadata, else the class's wrapper.
pub extern "C" fn get_objc_class_metadata(class: usize) -> Metadata {
    if class == 0 || is_swift_class(class) {
        class
    } else {
        objc_wrapper(class)
    }
}

extern "C" fn get_objc_class_from_metadata(metadata: Metadata) -> usize {
    class_of_metadata(metadata).unwrap_or(0)
}

/// `swift_getObjectType`: the dynamic type of the object `object`.
pub extern "C" fn get_object_type(object: Id) -> Metadata {
    get_objc_class_metadata(class::class_of(object) as usize)
}

fn class_of_metadata(metadata: Metadata) -> Option<usize> {
    match metadata::kind_of(metadata) {
        kind::CLASS => Some(metadata),
        kind::OBJC_CLASS_WRAPPER => Some(word(metadata + 8)),
        _ => None,
    }
}

fn is_instance(object: Id, class: usize) -> bool {
    !object.is_null() && root::descends_from(class::class_of(object), class as *mut ClassObject)
}

fn opaque_value(container: *mut u8) -> (*mut u8, Metadata) {
    let stored = word(container as usize + 24);
    let table = witnesses(stored);
    if table.is_inline() {
        (container, stored)
    } else {
        let boxed = word(container as usize);
        (
            project_box(boxed, table.alignment_mask()) as *mut u8,
            stored,
        )
    }
}

const SPECIAL_PROTOCOL: u32 = 0x3f00_0000;
const HAS_SUPERCLASS: u32 = 0x4000_0000;

fn existential_parts(existential: Metadata) -> (u32, u32) {
    (word32(existential + 8), word32(existential + 12))
}

fn open(container: *mut u8, existential: Metadata) -> (*mut u8, Metadata) {
    let (flags, _) = existential_parts(existential);
    if flags & SPECIAL_PROTOCOL != 0 {
        super::unimplemented(&[b"casts from Error existentials"]);
    }
    if flags & OPAQUE_EXISTENTIAL != 0 {
        opaque_value(container)
    } else {
        (container, get_object_type(word(container as usize) as Id))
    }
}

fn is_none(value: *mut u8, optional: Metadata) -> bool {
    let payload = generic::wrapped_of(optional);
    // SAFETY: the payload's witness reads an initialized `Optional` of it at `value`.
    unsafe { (witnesses(payload).get_enum_tag_single_payload)(value, 1, payload) != 0 }
}

fn store_case(value: *mut u8, case: u32, payload: Metadata) {
    // SAFETY: the payload's witness writes the `Optional` case at `value`, whose payload is
    // initialized for case 0.
    unsafe { (witnesses(payload).store_enum_tag_single_payload)(value, case, 1, payload) };
}

fn into_existential(destination: *mut u8, source: *mut u8, from: Metadata, to: Metadata) -> bool {
    let (flags, protocols) = existential_parts(to);
    if protocols != 0 || flags & (SPECIAL_PROTOCOL | HAS_SUPERCLASS) != 0 {
        super::unimplemented(&[b"casts to protocol existentials"]);
    }
    if flags & OPAQUE_EXISTENTIAL == 0 {
        if !metadata::is_class_type(from) {
            return false;
        }
        let object = word(source as usize);
        unknown_object_retain(object as *mut u8);
        set_word(destination as usize, object);
        return true;
    }
    if !witnesses(from).is_inline() {
        super::unimplemented(&[b"casts of values larger than three words into Any"]);
    }
    copy_value(destination, source, from);
    set_word(destination as usize + 24, from);
    true
}

fn into_class(destination: *mut u8, source: *mut u8, from: Metadata, to: Metadata) -> bool {
    let Some(class) = class_of_metadata(to) else {
        return false;
    };
    if !metadata::is_class_type(from) {
        return false;
    }
    let object = word(source as usize) as Id;
    if !is_instance(object, class) {
        return false;
    }
    unknown_object_retain(object.cast());
    set_word(destination as usize, object as usize);
    true
}

fn write_bytes(destination: *mut u8, bytes: &[u8]) -> bool {
    metadata::copy_bytes(destination, bytes.as_ptr(), bytes.len());
    true
}

fn integer_of(value: Exact) -> Option<i128> {
    match value {
        Exact::Integer(integer) => Some(integer),
        Exact::Real(real) => {
            let integer = real as i128;
            (integer as f64 == real).then_some(integer)
        }
    }
}

fn number_into(destination: *mut u8, value: Exact, to: Metadata) -> bool {
    let is = |target: &'static metadata::StructMetadata| to == address_of(target);
    if is(&metadata::DOUBLE) || is(&CGFLOAT) {
        let double = match value {
            Exact::Integer(integer) if (integer as f64) as i128 == integer => integer as f64,
            Exact::Integer(_) => return false,
            Exact::Real(real) => real,
        };
        return write_bytes(destination, &double.to_le_bytes());
    }
    if is(&metadata::FLOAT) {
        let single = match value {
            Exact::Integer(integer) if (integer as f32) as i128 == integer => integer as f32,
            Exact::Real(real) if f64::from(real as f32) == real || real.is_nan() => real as f32,
            _ => return false,
        };
        return write_bytes(destination, &single.to_le_bytes());
    }
    let (minimum, maximum, size) = if is(&metadata::BOOL) {
        (0, 1, 1)
    } else if is(&metadata::INT) || is(&metadata::INT64) {
        (i128::from(i64::MIN), i128::from(i64::MAX), 8)
    } else if is(&metadata::INT32) {
        (i128::from(i32::MIN), i128::from(i32::MAX), 4)
    } else if is(&metadata::INT16) {
        (i128::from(i16::MIN), i128::from(i16::MAX), 2)
    } else if is(&metadata::INT8) {
        (i128::from(i8::MIN), i128::from(i8::MAX), 1)
    } else if is(&metadata::UINT) || is(&metadata::UINT64) {
        (0, i128::from(u64::MAX), 8)
    } else if is(&metadata::UINT32) {
        (0, i128::from(u32::MAX), 4)
    } else if is(&metadata::UINT16) {
        (0, i128::from(u16::MAX), 2)
    } else if is(&metadata::UINT8) {
        (0, i128::from(u8::MAX), 1)
    } else {
        return false;
    };
    match integer_of(value) {
        Some(integer) if (minimum..=maximum).contains(&integer) => {
            write_bytes(destination, &(integer as u64).to_le_bytes()[..size])
        }
        _ => false,
    }
}

fn from_objective_c(destination: *mut u8, source: *mut u8, from: Metadata, to: Metadata) -> bool {
    if !metadata::is_class_type(from) {
        return false;
    }
    let object = word(source as usize) as Id;
    if object.is_null() {
        return false;
    }
    if foundation::is_string(object) {
        if to != address_of(&metadata::STRING) {
            return false;
        }
        let text = foundation::with_utf8(object, make);
        // SAFETY: the caller's destination: room for a value of the target type, `String`.
        unsafe { destination.cast::<RawString>().write(text) };
        return true;
    }
    if foundation::is_number(object) {
        return number_into(destination, foundation::exact_value(object), to);
    }
    false
}

fn cast_into(destination: *mut u8, source: *mut u8, from: Metadata, to: Metadata) -> bool {
    if from == to {
        copy_value(destination, source, to);
        return true;
    }
    let from_kind = metadata::kind_of(from);
    if from_kind == kind::EXISTENTIAL {
        let (value, stored) = open(source, from);
        return cast_into(destination, value, stored, to);
    }
    let to_kind = metadata::kind_of(to);
    if to_kind == kind::OPTIONAL {
        let payload = generic::wrapped_of(to);
        if from_kind == kind::OPTIONAL && is_none(source, from) {
            store_case(destination, 1, payload);
            return true;
        }
        if !cast_into(destination, source, from, payload) {
            return false;
        }
        store_case(destination, 0, payload);
        return true;
    }
    if from_kind == kind::OPTIONAL {
        if is_none(source, from) {
            return false;
        }
        return cast_into(destination, source, generic::wrapped_of(from), to);
    }
    match to_kind {
        kind::EXISTENTIAL => into_existential(destination, source, from, to),
        kind::CLASS | kind::OBJC_CLASS_WRAPPER => into_class(destination, source, from, to),
        kind::FOREIGN_CLASS => super::unimplemented(&[b"casts to Core Foundation types"]),
        _ => from_objective_c(destination, source, from, to),
    }
}

const UNCONDITIONAL: usize = 1;
const TAKE_ON_SUCCESS: usize = 2;
const DESTROY_ON_FAILURE: usize = 4;

fn report_failure(from: Metadata, to: Metadata) -> ! {
    let from = type_name::name_of(from, true);
    let to = type_name::name_of(to, true);
    super::fatal_error(&[
        b"Could not cast value of type '",
        from.as_bytes(),
        b"' to '",
        to.as_bytes(),
        b"'.",
    ])
}

extern "C" fn dynamic_cast(
    destination: *mut u8,
    source: *mut u8,
    from: Metadata,
    to: Metadata,
    flags: usize,
) -> bool {
    let cast = cast_into(destination, source, from, to);
    if cast {
        if flags & TAKE_ON_SUCCESS != 0 {
            destroy_value(source, from);
        }
        return true;
    }
    if flags & UNCONDITIONAL != 0 {
        let dynamic = if metadata::kind_of(from) == kind::EXISTENTIAL {
            open(source, from).1
        } else {
            from
        };
        report_failure(dynamic, to);
    }
    if flags & DESTROY_ON_FAILURE != 0 {
        destroy_value(source, from);
    }
    false
}

extern "C" fn dynamic_cast_class(object: Id, class: usize) -> Id {
    if is_instance(object, class) {
        object
    } else {
        core::ptr::null_mut()
    }
}

extern "C" fn dynamic_cast_unknown_class(object: Id, target: Metadata) -> Id {
    match class_of_metadata(target) {
        Some(class) => dynamic_cast_class(object, class),
        None => core::ptr::null_mut(),
    }
}

extern "C" fn dynamic_cast_class_unconditional(object: Id, class: usize) -> Id {
    if object.is_null() || is_instance(object, class) {
        return object;
    }
    report_failure(get_object_type(object), get_objc_class_metadata(class))
}

extern "C" fn dynamic_cast_unknown_class_unconditional(object: Id, target: Metadata) -> Id {
    let class = class_of_metadata(target).unwrap_or_else(|| report_failure(target, target));
    dynamic_cast_class_unconditional(object, class)
}

extern "C" fn get_foreign_type_metadata(_request: usize, candidate: Metadata) -> Response {
    Response {
        metadata: candidate,
        state: 0,
    }
}

/// `_bridgeAnyObjectToAny`: an `Any` holding the object as its dynamic type, or an optional
/// `AnyObject` holding `nil`.
pub(super) extern "C" fn bridge_any_object_to_any(result: *mut u8, object: Id) {
    if object.is_null() {
        let optional = generic::optional(address_of(&metadata::ANY_OBJECT))
            .unwrap_or_else(|| super::fatal_error(&[b"out of memory for Swift metadata"]));
        set_word(result as usize, 0);
        set_word(result as usize + 24, optional);
        return;
    }
    unknown_object_retain(object.cast());
    set_word(result as usize, object as usize);
    set_word(result as usize + 24, get_object_type(object));
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    use super::entry::{self, address};
    let found = match name {
        b"_swift_dynamicCast" => dynamic_cast as *const () as usize,
        b"_swift_dynamicCastClass" | b"_swift_dynamicCastObjCClass" => {
            dynamic_cast_class as *const () as usize
        }
        b"_swift_dynamicCastClassUnconditional" | b"_swift_dynamicCastObjCClassUnconditional" => {
            dynamic_cast_class_unconditional as *const () as usize
        }
        b"_swift_dynamicCastUnknownClass" => dynamic_cast_unknown_class as *const () as usize,
        b"_swift_dynamicCastUnknownClassUnconditional" => {
            dynamic_cast_unknown_class_unconditional as *const () as usize
        }
        b"_swift_getObjCClassMetadata" => get_objc_class_metadata as *const () as usize,
        b"_swift_getObjCClassFromMetadata" => get_objc_class_from_metadata as *const () as usize,
        b"_swift_getObjectType" => get_object_type as *const () as usize,
        b"_swift_getForeignTypeMetadata" => get_foreign_type_metadata as *const () as usize,
        b"_$ss018_bridgeAnyObjectToB0yypyXlSgF" => {
            return Some(address(entry::eggos_swift_bridge_any_object));
        }
        _ => return None,
    };
    Some(found as u64)
}

//! `Swift._SwiftObject` (`_TtCs12_SwiftObject`), the Objective-C root class every Swift class
//! without an Objective-C superclass descends from (D180): programs' class metadata name it and
//! its metaclass as their superclass and their metaclass's `isa`. Its methods count references
//! the Swift way, so an Objective-C message or ARC call on a Swift object reaches its count.

use super::heap_object::{self, header};
use super::refcount;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::class::{self, BuiltinClass};
use crate::objc::{RUNTIME, autorelease, methods, root};
use core::sync::atomic::Ordering;

pub static SWIFT_OBJECT: ClassObject = ClassObject::EMPTY;
pub static SWIFT_OBJECT_META: ClassObject = ClassObject::EMPTY;

/// The class object's address.
pub fn class() -> usize {
    &raw const SWIFT_OBJECT as usize
}

/// The metaclass's address.
pub fn metaclass() -> usize {
    &raw const SWIFT_OBJECT_META as usize
}

extern "C" fn retain(this: Id, _: Sel) -> Id {
    heap_object::swift_retain(this.cast()).cast()
}

extern "C" fn release(this: Id, _: Sel) {
    heap_object::swift_release(this.cast());
}

extern "C" fn autorelease(this: Id, _: Sel) -> Id {
    autorelease::add(this)
}

extern "C" fn retain_count(this: Id, _: Sel) -> usize {
    let bits = header(this.cast()).refcount.load(Ordering::Relaxed);
    refcount::strong_count(bits).min(usize::MAX as u64) as usize
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(this == other)
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    this as usize
}

extern "C" fn class_of(this: Id, _: Sel) -> *mut ClassObject {
    class::class_of(this)
}

extern "C" fn this(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn responds_to_selector(this: Id, _: Sel, sel: Sel) -> u8 {
    u8::from(root::responds(class::class_of(this), sel))
}

extern "C" fn is_kind_of_class(this: Id, _: Sel, ancestor: *mut ClassObject) -> u8 {
    u8::from(root::descends_from(class::class_of(this), ancestor))
}

extern "C" fn nothing(_: Id, _: Sel) {}

/// Defines the class; false if there is no memory.
pub fn define() -> bool {
    let instance = methods![
        "retain" => retain,
        "release" => release,
        "autorelease" => autorelease,
        "retainCount" => retain_count,
        "isEqual:" => is_equal,
        "hash" => hash,
        "class" => class_of,
        "self" => this,
        "respondsToSelector:" => responds_to_selector,
        "isKindOfClass:" => is_kind_of_class,
    ];
    let class_methods = methods![
        "initialize" => nothing,
        "load" => nothing,
        "class" => this,
        "self" => this,
    ];
    RUNTIME.with(|runtime| {
        class::define(
            runtime,
            &BuiltinClass {
                name: b"_TtCs12_SwiftObject\0",
                class: &SWIFT_OBJECT,
                meta: &SWIFT_OBJECT_META,
                superclass: None,
                instance_size: core::mem::size_of::<heap_object::HeapObject>(),
                flags: 0,
                instance_methods: &instance,
                class_methods: &class_methods,
            },
        )
    })
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_OBJC_CLASS_$__TtCs12_SwiftObject" => class(),
        b"_OBJC_METACLASS_$__TtCs12_SwiftObject" => metaclass(),
        _ => return None,
    };
    Some(address as u64)
}

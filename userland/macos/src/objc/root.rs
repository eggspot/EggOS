//! NSObject, the root class every class of a program and of Foundation descends from, and
//! NSAutoreleasePool. libobjc exports both NSObject objects; Foundation adds its methods that
//! need strings (`description`) on top.

use super::abi::{ClassObject, Id, Sel};
use super::class::{self, BuiltinClass, IMMORTAL};
use super::reference_count::{self, create_instance, direct_release, direct_retain};
use super::{RUNTIME, autorelease, send, send1, send2, terminate_uncaught};
use core::sync::atomic::Ordering;

pub static NSOBJECT: ClassObject = ClassObject::EMPTY;
pub static NSOBJECT_META: ClassObject = ClassObject::EMPTY;
static AUTORELEASE_POOL: ClassObject = ClassObject::EMPTY;
static AUTORELEASE_POOL_META: ClassObject = ClassObject::EMPTY;

/// NSObject's metaclass, every metaclass's class.
pub fn root_metaclass() -> *mut ClassObject {
    (&raw const NSOBJECT_META).cast_mut()
}

fn class_object(class: Id) -> *mut ClassObject {
    class.cast()
}

fn immortal(object: Id) -> bool {
    class::record_of(class::class_of(object)).is_none_or(|record| record.has(IMMORTAL))
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn retain(this: Id, _: Sel) -> Id {
    if immortal(this) {
        this
    } else {
        direct_retain(this)
    }
}

extern "C" fn release(this: Id, _: Sel) {
    if !immortal(this) {
        direct_release(this);
    }
}

extern "C" fn autorelease(this: Id, _: Sel) -> Id {
    if immortal(this) {
        this
    } else {
        autorelease::add(this)
    }
}

extern "C" fn retain_count(this: Id, _: Sel) -> usize {
    if immortal(this) {
        usize::MAX
    } else {
        reference_count::direct_retain_count(this)
    }
}

extern "C" fn dealloc(this: Id, _: Sel) {
    reference_count::dispose(this);
}

extern "C" fn class(this: Id, _: Sel) -> *mut ClassObject {
    class::class_of(this)
}

extern "C" fn this(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn superclass(this: Id, _: Sel) -> *mut ClassObject {
    // SAFETY: an object's class is a mapped class object.
    unsafe { &*class::class_of(this) }
        .superclass
        .load(Ordering::Relaxed)
}

/// Whether `class` is `ancestor` or descends from it.
pub fn descends_from(mut class: *mut ClassObject, ancestor: *mut ClassObject) -> bool {
    while !class.is_null() {
        if class == ancestor {
            return true;
        }
        // SAFETY: a chain of mapped class objects.
        class = unsafe { &*class }.superclass.load(Ordering::Relaxed);
    }
    false
}

/// Whether `object` is an instance of `class` or of one of its subclasses.
pub fn is_kind_of(object: Id, class: *const ClassObject) -> bool {
    !object.is_null() && descends_from(class::class_of(object), class.cast_mut())
}

extern "C" fn is_kind_of_class(this: Id, _: Sel, class: *mut ClassObject) -> u8 {
    u8::from(descends_from(class::class_of(this), class))
}

extern "C" fn is_member_of_class(this: Id, _: Sel, class: *mut ClassObject) -> u8 {
    u8::from(class::class_of(this) == class)
}

/// Whether instances of `class` (or the class itself, for a metaclass) respond to `sel`.
pub fn responds(class: *mut ClassObject, sel: Sel) -> bool {
    !sel.is_null() && class::lookup(class, sel).is_some()
}

extern "C" fn responds_to_selector(this: Id, _: Sel, sel: Sel) -> u8 {
    u8::from(responds(class::class_of(this), sel))
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(this == other)
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    this as usize
}

extern "C" fn copy(this: Id, _: Sel) -> Id {
    send1(this, sel!("copyWithZone:"), 0)
}

extern "C" fn mutable_copy(this: Id, _: Sel) -> Id {
    send1(this, sel!("mutableCopyWithZone:"), 0)
}

extern "C" fn perform_selector(this: Id, _: Sel, sel: Sel) -> Id {
    send(this, sel)
}

extern "C" fn perform_selector_with(this: Id, _: Sel, sel: Sel, object: usize) -> Id {
    send1(this, sel, object)
}

extern "C" fn perform_selector_with_with(
    this: Id,
    _: Sel,
    sel: Sel,
    first: usize,
    second: usize,
) -> Id {
    send2(this, sel, first, second)
}

extern "C" fn zone(_: Id, _: Sel) -> usize {
    0
}

/// The name of `object`'s class (for a class object, its own name).
pub fn class_name(object: Id) -> &'static [u8] {
    if object.is_null() {
        return b"nil";
    }
    class::record_of(class::class_of(object)).map_or(b"?", |record| record.name())
}

fn report_unrecognized(receiver: Id, sel: Sel) -> ! {
    let is_class =
        class::record_of(class::class_of(receiver)).is_some_and(|record| record.is_meta());
    let name = if sel.is_null() {
        &b"(null)"[..]
    } else {
        // SAFETY: a selector is a canonical, permanent name.
        unsafe { class::abi_name(sel) }
    };
    let mut digits = [0u8; 18];
    terminate_uncaught(
        b"NSInvalidArgumentException",
        &[
            if is_class { b"+[" } else { b"-[" },
            class_name(receiver),
            b" ",
            name,
            b"]: unrecognized selector sent to ",
            if is_class { b"class " } else { b"instance " },
            super::hexadecimal(receiver as usize, &mut digits),
        ],
    )
}

/// What a send of an unknown selector does: `-doesNotRecognizeSelector:`, which ends the
/// program.
pub fn does_not_recognize(receiver: Id, sel: Sel) -> usize {
    let does_not = sel!("doesNotRecognizeSelector:");
    if sel == does_not || class::record_of(class::class_of(receiver)).is_none() {
        report_unrecognized(receiver, sel);
    }
    send1(receiver, does_not, sel as usize);
    report_unrecognized(receiver, sel)
}

extern "C" fn does_not_recognize_selector(this: Id, _: Sel, sel: Sel) {
    report_unrecognized(this, sel)
}

extern "C" fn alloc(this: Id, _: Sel) -> Id {
    send1(this, sel!("allocWithZone:"), 0)
}

extern "C" fn alloc_with_zone(this: Id, _: Sel, _: usize) -> Id {
    create_instance(class_object(this), 0)
}

extern "C" fn new(this: Id, _: Sel) -> Id {
    let object = send(this, sel!("alloc"));
    if object.is_null() {
        return object;
    }
    send(object, sel!("init"))
}

extern "C" fn class_class(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn class_superclass(this: Id, _: Sel) -> *mut ClassObject {
    // SAFETY: a class object, mapped.
    unsafe { &*class_object(this) }
        .superclass
        .load(Ordering::Relaxed)
}

extern "C" fn nothing(_: Id, _: Sel) {}

extern "C" fn instances_respond_to_selector(this: Id, _: Sel, sel: Sel) -> u8 {
    u8::from(responds(class_object(this), sel))
}

extern "C" fn is_subclass_of_class(this: Id, _: Sel, class: *mut ClassObject) -> u8 {
    u8::from(descends_from(class_object(this), class))
}

extern "C" fn class_is_kind_of_class(this: Id, _: Sel, class: *mut ClassObject) -> u8 {
    u8::from(descends_from(class::class_of(this), class))
}

#[repr(C)]
struct AutoreleasePool {
    isa: usize,
    token: usize,
}

extern "C" fn pool_init(this: Id, _: Sel) -> Id {
    // SAFETY: an NSAutoreleasePool instance, which has the token after its class.
    unsafe { (*this.cast::<AutoreleasePool>()).token = autorelease::push() };
    this
}

extern "C" fn pool_drain(this: Id, _: Sel) {
    // SAFETY: as above.
    let token = unsafe { (*this.cast::<AutoreleasePool>()).token };
    autorelease::pop(token);
    reference_count::dispose(this);
}

/// Defines NSObject and NSAutoreleasePool; false if there is no memory.
pub fn define() -> bool {
    let object_instance = methods![
        "init" => init,
        "retain" => retain,
        "release" => release,
        "autorelease" => autorelease,
        "retainCount" => retain_count,
        "dealloc" => dealloc,
        "class" => class,
        "self" => this,
        "superclass" => superclass,
        "isKindOfClass:" => is_kind_of_class,
        "isMemberOfClass:" => is_member_of_class,
        "respondsToSelector:" => responds_to_selector,
        "isEqual:" => is_equal,
        "hash" => hash,
        "copy" => copy,
        "mutableCopy" => mutable_copy,
        "performSelector:" => perform_selector,
        "performSelector:withObject:" => perform_selector_with,
        "performSelector:withObject:withObject:" => perform_selector_with_with,
        "doesNotRecognizeSelector:" => does_not_recognize_selector,
        "zone" => zone,
    ];
    let object_class = methods![
        "alloc" => alloc,
        "allocWithZone:" => alloc_with_zone,
        "new" => new,
        "class" => class_class,
        "self" => this,
        "superclass" => class_superclass,
        "initialize" => nothing,
        "load" => nothing,
        "instancesRespondToSelector:" => instances_respond_to_selector,
        "isSubclassOfClass:" => is_subclass_of_class,
        "isKindOfClass:" => class_is_kind_of_class,
    ];
    let pool_instance = methods![
        "init" => pool_init,
        "drain" => pool_drain,
        "release" => pool_drain,
    ];
    RUNTIME.with(|runtime| {
        class::define(
            runtime,
            &BuiltinClass {
                name: b"NSObject\0",
                class: &NSOBJECT,
                meta: &NSOBJECT_META,
                superclass: None,
                instance_size: core::mem::size_of::<usize>(),
                flags: 0,
                instance_methods: &object_instance,
                class_methods: &object_class,
            },
        ) && class::define(
            runtime,
            &BuiltinClass {
                name: b"NSAutoreleasePool\0",
                class: &AUTORELEASE_POOL,
                meta: &AUTORELEASE_POOL_META,
                superclass: Some(&NSOBJECT),
                instance_size: core::mem::size_of::<AutoreleasePool>(),
                flags: 0,
                instance_methods: &pool_instance,
                class_methods: &[],
            },
        )
    })
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_OBJC_CLASS_$_NSObject" => &raw const NSOBJECT as usize,
        b"_OBJC_METACLASS_$_NSObject" => &raw const NSOBJECT_META as usize,
        b"_OBJC_CLASS_$_NSAutoreleasePool" => &raw const AUTORELEASE_POOL as usize,
        b"_OBJC_METACLASS_$_NSAutoreleasePool" => &raw const AUTORELEASE_POOL_META as usize,
        b"__objc_empty_cache" => &raw const super::cache::EMPTY as usize,
        b"__objc_empty_vtable" => 0,
        _ => return None,
    };
    Some(address as u64)
}

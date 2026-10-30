//! Objects and their reference counts. An object the runtime allocates sits 16 bytes into its
//! heap block; the word before it holds its extra retains (0 means one owner), in its top bit
//! that it is being deallocated, after which retains and releases do nothing, and in the next
//! bit that a weak variable names it (D175), so freeing it clears those variables. Objects of
//! immortal classes (metaclasses, constant strings and literals) are never counted; classes that
//! define their own `retain` or `release` get the messages. Native Swift objects count in their
//! own header, weak variables included (D180). ARC's functions build on these.

use super::abi::{ClassObject, Id, Sel};
use super::class::{self, CUSTOM_REFERENCE_COUNTING, HAS_CXX_DESTRUCT, IMMORTAL};
use super::{autorelease, send};
use crate::libsystem::{heap, sync};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const HEADER: usize = 16;
const DEALLOCATING: usize = 1 << (usize::BITS - 1);
const WEAKLY_REFERENCED: usize = 1 << (usize::BITS - 2);
const COUNT_MASK: usize = !(DEALLOCATING | WEAKLY_REFERENCED);

fn count_of(object: Id) -> &'static AtomicUsize {
    // SAFETY: only objects the runtime allocated are counted, and their count word is the
    // word before them, inside their block.
    unsafe { &*(object as *const AtomicUsize).wrapping_sub(1) }
}

enum Counting {
    Never,
    Messages,
    Direct,
    Swift,
}

fn counting(object: Id) -> Counting {
    if crate::swift::uses_swift_counting(object.cast()) {
        return Counting::Swift;
    }
    match class::record_of(class::class_of(object)) {
        None => Counting::Never,
        Some(record) if record.has(IMMORTAL) => Counting::Never,
        Some(record) if record.has(CUSTOM_REFERENCE_COUNTING) => Counting::Messages,
        Some(_) => Counting::Direct,
    }
}

/// A new zeroed instance of `class` with `extra` more bytes, counted once; null if there is no
/// memory.
pub fn create_instance(class: *mut ClassObject, extra: usize) -> Id {
    let Some(record) = class::prepare(class) else {
        return core::ptr::null_mut();
    };
    let size = record
        .instance_size
        .load(Ordering::Relaxed)
        .max(core::mem::size_of::<usize>())
        .saturating_add(extra);
    let block = heap::allocate(size.saturating_add(HEADER), true);
    if block.is_null() {
        return core::ptr::null_mut();
    }
    let object = block.wrapping_add(HEADER).cast::<super::abi::Object>();
    // SAFETY: the new block's object part, zeroed; its first word is its class.
    unsafe { &*object }.isa.store(class, Ordering::Relaxed);
    object
}

pub fn direct_retain(object: Id) -> Id {
    count_of(object).fetch_add(1, Ordering::Relaxed);
    object
}

/// Drops one reference; the last sends `dealloc`.
pub fn direct_release(object: Id) {
    let count = count_of(object);
    let mut value = count.load(Ordering::Relaxed);
    loop {
        if value & DEALLOCATING != 0 {
            return;
        }
        let next = match value & COUNT_MASK {
            0 => value | DEALLOCATING,
            _ => value - 1,
        };
        match count.compare_exchange_weak(value, next, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => break,
            Err(now) => value = now,
        }
    }
    if value & COUNT_MASK == 0 {
        core::sync::atomic::fence(Ordering::Acquire);
        send(object, sel!("dealloc"));
    }
}

pub fn direct_retain_count(object: Id) -> usize {
    (count_of(object).load(Ordering::Relaxed) & COUNT_MASK) + 1
}

/// Whether the runtime frees `object` once uncounted: one it allocated (with a count word) or a
/// mortal native Swift object.
pub fn is_counted(object: Id) -> bool {
    match counting(object) {
        Counting::Direct | Counting::Messages => true,
        Counting::Swift => !crate::swift::is_immortal(object.cast()),
        Counting::Never => false,
    }
}

/// Whether the counted `object` is being deallocated.
pub fn is_deallocating(object: Id) -> bool {
    if crate::swift::uses_swift_counting(object.cast()) {
        return crate::swift::is_deinitializing(object.cast());
    }
    count_of(object).load(Ordering::Acquire) & DEALLOCATING != 0
}

/// Marks the counted `object` as named by a weak variable.
pub fn mark_weakly_referenced(object: Id) {
    if crate::swift::uses_swift_counting(object.cast()) {
        crate::swift::mark_weakly_referenced(object.cast());
        return;
    }
    count_of(object).fetch_or(WEAKLY_REFERENCED, Ordering::Relaxed);
}

/// Retains `object` unless it is being deallocated (`None`); `Some(false)` when its class
/// counts by messages, which the caller sends once it holds no lock.
pub fn try_retain(object: Id) -> Option<bool> {
    match counting(object) {
        Counting::Never => Some(true),
        Counting::Swift => crate::swift::try_retain(object.cast()).then_some(true),
        Counting::Messages => match is_deallocating(object) {
            true => None,
            false => Some(false),
        },
        Counting::Direct => {
            let count = count_of(object);
            let mut value = count.load(Ordering::Relaxed);
            loop {
                if value & DEALLOCATING != 0 {
                    return None;
                }
                match count.compare_exchange_weak(
                    value,
                    value + 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Some(true),
                    Err(now) => value = now,
                }
            }
        }
    }
}

/// Runs every `.cxx_destruct` of the object's classes, the most derived first, then frees it.
pub fn dispose(object: Id) {
    if object.is_null() || !matches!(counting(object), Counting::Direct | Counting::Messages) {
        return;
    }
    let mut record = class::record_of(class::class_of(object));
    while let Some(current) = record {
        if current.has(HAS_CXX_DESTRUCT) {
            let destruct = super::RUNTIME.with(|_| current.own_method(sel!(".cxx_destruct")));
            if let Some(imp) = destruct.map(crate::callback::native_callable) {
                // SAFETY: `.cxx_destruct` takes the object and its selector.
                let destruct: extern "C" fn(Id, Sel) = unsafe { core::mem::transmute(imp) };
                destruct(object, sel!(".cxx_destruct"));
            }
        }
        record = current.superclass();
    }
    if count_of(object).load(Ordering::Acquire) & WEAKLY_REFERENCED != 0 {
        super::weak::clear(object);
    }
    heap::release(object.cast::<u8>().wrapping_sub(HEADER));
}

pub extern "C" fn objc_retain(object: Id) -> Id {
    if object.is_null() {
        return object;
    }
    match counting(object) {
        Counting::Never => object,
        Counting::Swift => crate::swift::retain(object.cast()).cast(),
        Counting::Messages => send(object, sel!("retain")),
        Counting::Direct => direct_retain(object),
    }
}

pub extern "C" fn objc_release(object: Id) {
    if object.is_null() {
        return;
    }
    match counting(object) {
        Counting::Never => {}
        Counting::Swift => crate::swift::release(object.cast()),
        Counting::Messages => {
            send(object, sel!("release"));
        }
        Counting::Direct => direct_release(object),
    }
}

pub extern "C" fn objc_autorelease(object: Id) -> Id {
    if object.is_null() {
        return object;
    }
    match counting(object) {
        Counting::Never => object,
        Counting::Messages => send(object, sel!("autorelease")),
        Counting::Direct | Counting::Swift => autorelease::add(object),
    }
}

extern "C" fn objc_retain_autorelease(object: Id) -> Id {
    objc_autorelease(objc_retain(object))
}

extern "C" fn objc_unsafe_claim_autoreleased_return_value(object: Id) -> Id {
    object
}

unsafe extern "C" fn objc_store_strong(location: *mut Id, object: Id) {
    // SAFETY: the caller's promise.
    let previous = unsafe { location.read() };
    if previous == object {
        return;
    }
    objc_retain(object);
    // SAFETY: as above.
    unsafe { location.write(object) };
    objc_release(previous);
}

extern "C" fn objc_alloc(class: Id) -> Id {
    if class.is_null() {
        return class;
    }
    send(class, sel!("alloc"))
}

extern "C" fn objc_alloc_init(class: Id) -> Id {
    let object = objc_alloc(class);
    if object.is_null() {
        return object;
    }
    send(object, sel!("init"))
}

extern "C" fn objc_opt_new(class: Id) -> Id {
    if class.is_null() {
        return class;
    }
    send(class, sel!("new"))
}

extern "C" fn objc_opt_class(object: Id) -> Id {
    if object.is_null() {
        return object;
    }
    send(object, sel!("class"))
}

extern "C" fn objc_opt_self(object: Id) -> Id {
    object
}

extern "C" fn objc_opt_is_kind_of_class(object: Id, class: usize) -> u8 {
    if object.is_null() {
        return 0;
    }
    u8::from(super::send_bool(object, sel!("isKindOfClass:"), class))
}

extern "C" fn objc_opt_responds_to_selector(object: Id, sel: usize) -> u8 {
    if object.is_null() {
        return 0;
    }
    u8::from(super::send_bool(object, sel!("respondsToSelector:"), sel))
}

extern "C" fn objc_enumeration_mutation(object: Id) {
    let mut digits = [0u8; 18];
    super::terminate_uncaught(
        b"NSGenericException",
        &[
            b"*** Collection <",
            super::root::class_name(object),
            b": ",
            super::hexadecimal(object as usize, &mut digits),
            b"> was mutated while being enumerated.",
        ],
    )
}

static PROPERTY_LOCK: AtomicU32 = AtomicU32::new(0);

fn slot(object: Id, offset: isize) -> *mut Id {
    object.cast::<u8>().wrapping_offset(offset).cast()
}

unsafe extern "C" fn objc_get_property(object: Id, _: Sel, offset: isize, atomic: u8) -> Id {
    let slot = slot(object, offset);
    if atomic == 0 {
        // SAFETY: the caller's promise.
        return unsafe { slot.read() };
    }
    sync::lock(&PROPERTY_LOCK);
    // SAFETY: as above.
    let value = objc_retain(unsafe { slot.read() });
    sync::unlock(&PROPERTY_LOCK);
    objc_autorelease(value)
}

fn store_property(object: Id, offset: isize, value: Id, atomic: bool, copy: u8) {
    let value = match copy {
        0 => objc_retain(value),
        2 => send(value, sel!("mutableCopy")),
        _ => send(value, sel!("copy")),
    };
    let slot = slot(object, offset);
    if atomic {
        sync::lock(&PROPERTY_LOCK);
    }
    // SAFETY: the property's ivar, which the compiler's setter names by its offset.
    let previous = unsafe { slot.read() };
    // SAFETY: as above.
    unsafe { slot.write(value) };
    if atomic {
        sync::unlock(&PROPERTY_LOCK);
    }
    objc_release(previous);
}

extern "C" fn objc_set_property(
    object: Id,
    _: Sel,
    offset: isize,
    value: Id,
    atomic: u8,
    copy: u8,
) {
    store_property(object, offset, value, atomic != 0, copy);
}

extern "C" fn objc_set_property_atomic(object: Id, _: Sel, value: Id, offset: isize) {
    store_property(object, offset, value, true, 0);
}

extern "C" fn objc_set_property_nonatomic(object: Id, _: Sel, value: Id, offset: isize) {
    store_property(object, offset, value, false, 0);
}

extern "C" fn objc_set_property_atomic_copy(object: Id, _: Sel, value: Id, offset: isize) {
    store_property(object, offset, value, true, 1);
}

extern "C" fn objc_set_property_nonatomic_copy(object: Id, _: Sel, value: Id, offset: isize) {
    store_property(object, offset, value, false, 1);
}

unsafe extern "C" fn objc_copy_struct(
    destination: *mut u8,
    source: *const u8,
    size: usize,
    atomic: u8,
    _: u8,
) {
    if atomic != 0 {
        sync::lock(&PROPERTY_LOCK);
    }
    // SAFETY: the caller's promise.
    unsafe { core::ptr::copy(source, destination, size) };
    if atomic != 0 {
        sync::unlock(&PROPERTY_LOCK);
    }
}

extern "C" fn class_create_instance(class: *mut ClassObject, extra: usize) -> Id {
    create_instance(class, extra)
}

extern "C" fn object_dispose(object: Id) -> Id {
    dispose(object);
    core::ptr::null_mut()
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_retain" => objc_retain as *const () as usize,
        b"_objc_release" => objc_release as *const () as usize,
        b"_objc_autorelease" => objc_autorelease as *const () as usize,
        b"_objc_retainAutorelease" => objc_retain_autorelease as *const () as usize,
        b"_objc_autoreleaseReturnValue" => objc_autorelease as *const () as usize,
        b"_objc_retainAutoreleaseReturnValue" => objc_retain_autorelease as *const () as usize,
        b"_objc_retainAutoreleasedReturnValue" => objc_retain as *const () as usize,
        b"_objc_claimAutoreleasedReturnValue" => objc_retain as *const () as usize,
        b"_objc_unsafeClaimAutoreleasedReturnValue" => {
            objc_unsafe_claim_autoreleased_return_value as *const () as usize
        }
        b"_objc_storeStrong" => objc_store_strong as *const () as usize,
        b"_objc_alloc" | b"_objc_allocWithZone" => objc_alloc as *const () as usize,
        b"_objc_alloc_init" => objc_alloc_init as *const () as usize,
        b"_objc_opt_new" => objc_opt_new as *const () as usize,
        b"_objc_opt_class" => objc_opt_class as *const () as usize,
        b"_objc_opt_self" => objc_opt_self as *const () as usize,
        b"_objc_opt_isKindOfClass" => objc_opt_is_kind_of_class as *const () as usize,
        b"_objc_opt_respondsToSelector" => objc_opt_responds_to_selector as *const () as usize,
        b"_objc_enumerationMutation" => objc_enumeration_mutation as *const () as usize,
        b"_objc_getProperty" => objc_get_property as *const () as usize,
        b"_objc_setProperty" => objc_set_property as *const () as usize,
        b"_objc_setProperty_atomic" => objc_set_property_atomic as *const () as usize,
        b"_objc_setProperty_nonatomic" => objc_set_property_nonatomic as *const () as usize,
        b"_objc_setProperty_atomic_copy" => objc_set_property_atomic_copy as *const () as usize,
        b"_objc_setProperty_nonatomic_copy" => {
            objc_set_property_nonatomic_copy as *const () as usize
        }
        b"_objc_copyStruct" => objc_copy_struct as *const () as usize,
        b"_class_createInstance" => class_create_instance as *const () as usize,
        b"_object_dispose" => object_dispose as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

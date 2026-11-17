//! The runtime's C functions a console program calls directly: classes by name, an object's
//! class, class and method queries, adding methods, selectors, and throwing.

use super::abi::{ClassObject, Id, Imp, Sel};
use super::class::{self, ClassRecord};
use super::selector::name_bytes;
use super::{RUNTIME, root, selector_named};
use core::ffi::c_char;
use core::sync::atomic::Ordering;

/// The class named `name`, realized and initialized, if the program or the layer defines it.
pub fn class_named(name: &[u8]) -> *mut ClassObject {
    let found = RUNTIME.with(|runtime| {
        runtime
            .classes
            .as_slice()
            .iter()
            .map(|&record| {
                // SAFETY: the class table holds records the runtime made.
                unsafe { &*record }
            })
            .find(|record| {
                !record.is_meta() && !record.has(class::MISSING) && record.name() == name
            })
            .map(|record| record.class)
    });
    match found {
        Some(class) => {
            class::prepare(class);
            class
        }
        None => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn objc_get_class(name: *const c_char) -> *mut ClassObject {
    if name.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's promise.
    class_named(unsafe { name_bytes(name.cast()) })
}

unsafe extern "C" fn objc_get_meta_class(name: *const c_char) -> *mut ClassObject {
    // SAFETY: the caller's promise.
    let class = unsafe { objc_get_class(name) };
    if class.is_null() {
        return class;
    }
    // SAFETY: a class object the runtime knows.
    unsafe { &*class }.isa.load(Ordering::Relaxed)
}

extern "C" fn object_get_class(object: Id) -> *mut ClassObject {
    if object.is_null() {
        return core::ptr::null_mut();
    }
    class::class_of(object)
}

extern "C" fn object_get_class_name(object: Id) -> *const u8 {
    if object.is_null() {
        return c"nil".as_ptr().cast::<u8>();
    }
    class::record_of(class::class_of(object))
        .map_or(c"?".as_ptr().cast::<u8>(), |record| record.name)
}

fn record(class: *mut ClassObject) -> Option<&'static ClassRecord> {
    class::record_of(class)
}

extern "C" fn class_get_name(class: *mut ClassObject) -> *const u8 {
    record(class).map_or(c"nil".as_ptr().cast::<u8>(), |record| record.name)
}

extern "C" fn class_get_superclass(class: *mut ClassObject) -> *mut ClassObject {
    if class.is_null() {
        return class;
    }
    // SAFETY: a class object.
    unsafe { &*class }.superclass.load(Ordering::Relaxed)
}

extern "C" fn class_is_meta_class(class: *mut ClassObject) -> u8 {
    u8::from(record(class).is_some_and(ClassRecord::is_meta))
}

extern "C" fn class_responds_to_selector(class: *mut ClassObject, sel: Sel) -> u8 {
    u8::from(!class.is_null() && root::responds(class, sel))
}

extern "C" fn class_get_instance_size(class: *mut ClassObject) -> usize {
    class::prepare(class).map_or(0, |record| record.instance_size.load(Ordering::Relaxed))
}

#[repr(C)]
#[derive(Debug)]
struct MethodHandle {
    sel: usize,
    imp: usize,
    owner: usize,
}

static HANDLES: crate::libsystem::Exclusive<super::buffer::Buffer<*mut MethodHandle>> =
    crate::libsystem::Exclusive::new(super::buffer::Buffer::new());

fn handle(sel: Sel, owner: &'static ClassRecord) -> *mut MethodHandle {
    let owner_address = owner as *const ClassRecord as usize;
    let imp = RUNTIME.with(|_| owner.own_method(sel)).unwrap_or(0);
    HANDLES.with(|handles| {
        for &existing in handles.as_slice() {
            // SAFETY: handles are permanent blocks made below.
            let found = unsafe { &mut *existing };
            if found.sel == sel as usize && found.owner == owner_address {
                found.imp = imp;
                return existing;
            }
        }
        let made = crate::libsystem::heap::allocate(core::mem::size_of::<MethodHandle>(), true)
            .cast::<MethodHandle>();
        if made.is_null() {
            return made;
        }
        // SAFETY: a new block of the handle's size.
        unsafe {
            made.write(MethodHandle {
                sel: sel as usize,
                imp,
                owner: owner_address,
            })
        };
        handles.push(made);
        made
    })
}

extern "C" fn class_get_instance_method(class: *mut ClassObject, sel: Sel) -> *mut MethodHandle {
    if class.is_null() || class::find(class, sel).is_none() {
        return core::ptr::null_mut();
    }
    match record(class).and_then(|record| class::owner_of(record, sel)) {
        Some(owner) => handle(sel, owner),
        None => core::ptr::null_mut(),
    }
}

fn set_implementation(method: *mut MethodHandle, imp: Imp) -> Imp {
    if method.is_null() {
        return 0;
    }
    // SAFETY: a handle from `handle`, permanent.
    let handle = unsafe { &mut *method };
    let owner = handle.owner as *const ClassRecord;
    if owner.is_null() {
        return 0;
    }
    // SAFETY: the owner is a record the runtime made, which lives as long as the program.
    let owner = unsafe { &*owner };
    let old = RUNTIME.with(|runtime| class::replace_own_method(runtime, owner, handle.sel as Sel, imp));
    handle.imp = imp;
    old.unwrap_or(0)
}

extern "C" fn method_set_implementation(method: *mut MethodHandle, imp: Imp) -> Imp {
    set_implementation(method, imp)
}

extern "C" fn method_exchange_implementations(first: *mut MethodHandle, second: *mut MethodHandle) {
    if first.is_null() || second.is_null() {
        return;
    }
    let first_imp = method_get_implementation(first);
    let second_imp = method_get_implementation(second);
    set_implementation(first, second_imp);
    set_implementation(second, first_imp);
}

extern "C" fn class_replace_method(class: *mut ClassObject, sel: Sel, imp: Imp, types: *const u8) -> Imp {
    let Some(record) = class::prepare(class) else {
        return 0;
    };
    let replaced = RUNTIME.with(|runtime| class::replace_own_method(runtime, record, sel, imp));
    match replaced {
        Some(old) => old,
        None => {
            class_add_method(class, sel, imp, types);
            0
        }
    }
}

extern "C" fn class_get_class_method(class: *mut ClassObject, sel: Sel) -> *mut MethodHandle {
    if class.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: a class object.
    let meta = unsafe { &*class }.isa.load(Ordering::Relaxed);
    class_get_instance_method(meta, sel)
}

extern "C" fn method_get_implementation(method: *const MethodHandle) -> Imp {
    if method.is_null() {
        return 0;
    }
    // SAFETY: a handle from `handle`.
    unsafe { &*method }.imp
}

extern "C" fn method_get_name(method: *const MethodHandle) -> Sel {
    if method.is_null() {
        return core::ptr::null();
    }
    // SAFETY: as above.
    unsafe { &*method }.sel as Sel
}

extern "C" fn class_add_method(class: *mut ClassObject, sel: Sel, imp: Imp, _: *const u8) -> u8 {
    let Some(record) = class::prepare(class) else {
        return 0;
    };
    RUNTIME.with(|runtime| {
        if record.own_method(sel).is_some() {
            return 0;
        }
        class::prepend_methods(runtime, record, &[(sel as usize, imp)]);
        1
    })
}

unsafe extern "C" fn sel_register_name(name: *const c_char) -> Sel {
    if name.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the caller's promise.
    selector_named(unsafe { name_bytes(name.cast()) }).unwrap_or(core::ptr::null())
}

extern "C" fn sel_get_name(sel: Sel) -> Sel {
    if sel.is_null() {
        c"<null selector>".as_ptr().cast::<u8>()
    } else {
        sel
    }
}

extern "C" fn sel_is_equal(left: Sel, right: Sel) -> u8 {
    u8::from(left == right)
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_objc_getClass" | b"_objc_lookUpClass" | b"_objc_getRequiredClass" => {
            objc_get_class as *const () as usize
        }
        b"_objc_getMetaClass" => objc_get_meta_class as *const () as usize,
        b"_object_getClass" => object_get_class as *const () as usize,
        b"_object_getClassName" => object_get_class_name as *const () as usize,
        b"_class_getName" => class_get_name as *const () as usize,
        b"_class_getSuperclass" => class_get_superclass as *const () as usize,
        b"_class_isMetaClass" => class_is_meta_class as *const () as usize,
        b"_class_respondsToSelector" => class_responds_to_selector as *const () as usize,
        b"_class_getInstanceSize" => class_get_instance_size as *const () as usize,
        b"_class_getInstanceMethod" => class_get_instance_method as *const () as usize,
        b"_class_getClassMethod" => class_get_class_method as *const () as usize,
        b"_method_getImplementation" => method_get_implementation as *const () as usize,
        b"_method_getName" => method_get_name as *const () as usize,
        b"_class_addMethod" => class_add_method as *const () as usize,
        b"_class_replaceMethod" => class_replace_method as *const () as usize,
        b"_method_setImplementation" => method_set_implementation as *const () as usize,
        b"_method_exchangeImplementations" => method_exchange_implementations as *const () as usize,
        b"_sel_registerName" | b"_sel_getUid" => sel_register_name as *const () as usize,
        b"_sel_getName" => sel_get_name as *const () as usize,
        b"_sel_isEqual" => sel_is_equal as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

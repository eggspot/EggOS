//! The runtime's C functions a console program calls directly: classes by name, an object's
//! class, class and method queries, adding methods, selectors, and throwing.

use super::abi::{ClassObject, Id, Imp, Sel};
use super::class::{self, ClassRecord};
use super::selector::name_bytes;
use super::{RUNTIME, root, selector_named, terminate_uncaught};
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
}

static HANDLES: crate::libsystem::Exclusive<super::buffer::Buffer<*mut MethodHandle>> =
    crate::libsystem::Exclusive::new(super::buffer::Buffer::new());

fn handle(sel: Sel, imp: Imp) -> *mut MethodHandle {
    HANDLES.with(|handles| {
        for &existing in handles.as_slice() {
            // SAFETY: handles are permanent blocks made below.
            let found = unsafe { &*existing };
            if found.sel == sel as usize && found.imp == imp {
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
            })
        };
        handles.push(made);
        made
    })
}

extern "C" fn class_get_instance_method(class: *mut ClassObject, sel: Sel) -> *mut MethodHandle {
    match class::lookup(class, sel) {
        Some(imp) if !class.is_null() => handle(sel, imp),
        _ => core::ptr::null_mut(),
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

extern "C" fn objc_exception_throw(exception: Id) -> ! {
    terminate_uncaught(
        root::class_name(exception),
        &[b"an Objective-C exception (no unwinder yet)"],
    )
}

extern "C" fn objc_terminate() -> ! {
    crate::libsystem::abort()
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
        b"_sel_registerName" | b"_sel_getUid" => sel_register_name as *const () as usize,
        b"_sel_getName" => sel_get_name as *const () as usize,
        b"_sel_isEqual" => sel_is_equal as *const () as usize,
        b"_objc_exception_throw" => objc_exception_throw as *const () as usize,
        b"_objc_terminate" => objc_terminate as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

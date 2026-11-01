//! Swift's weak references (D180): `weak var` of a native Swift class (`swift_weak*`) and of a
//! class-bound protocol or Objective-C class (`swift_unknownObjectWeak*`). A reference is one
//! word holding the address of a slot of its own, which the Objective-C runtime's weak table
//! (`objc/weak.rs`) keeps: the object's last release, native or Objective-C, makes the slot nil,
//! and a load retains the object only while it is not deinitializing. Because the reference
//! names its slot rather than the object, Swift may move it bitwise, as it moves native weak
//! references.

use super::heap_object::unknown_object_release;
use super::metadata::{set_word, word};
use crate::libsystem::heap;
use crate::objc::abi::Id;
use crate::objc::weak::{
    objc_destroy_weak, objc_init_weak, objc_load_weak_retained, objc_store_weak,
};

type Reference = *mut usize;

fn slot_of(reference: Reference) -> *mut Id {
    word(reference as usize) as *mut Id
}

fn new_slot(object: Id) -> usize {
    if object.is_null() {
        return 0;
    }
    let slot = heap::allocate(core::mem::size_of::<Id>(), true).cast::<Id>();
    if slot.is_null() {
        super::fatal_error(&[b"out of memory for a Swift weak reference"]);
    }
    // SAFETY: a new slot no weak variable has used.
    unsafe { objc_init_weak(slot, object) };
    slot as usize
}

fn free_slot(slot: *mut Id) {
    if slot.is_null() {
        return;
    }
    // SAFETY: a slot `new_slot` registered and nothing has forgotten.
    unsafe { objc_destroy_weak(slot) };
    heap::release(slot.cast());
}

extern "C" fn weak_init(reference: Reference, object: Id) -> Reference {
    set_word(reference as usize, new_slot(object));
    reference
}

extern "C" fn weak_assign(reference: Reference, object: Id) -> Reference {
    let slot = slot_of(reference);
    if slot.is_null() {
        set_word(reference as usize, new_slot(object));
    } else {
        // SAFETY: the reference's own registered slot.
        unsafe { objc_store_weak(slot, object) };
    }
    reference
}

extern "C" fn weak_load_strong(reference: Reference) -> Id {
    let slot = slot_of(reference);
    if slot.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: as in `weak_assign`.
    unsafe { objc_load_weak_retained(slot) }
}

extern "C" fn weak_destroy(reference: Reference) {
    free_slot(slot_of(reference));
    set_word(reference as usize, 0);
}

extern "C" fn weak_take_strong(reference: Reference) -> Id {
    let object = weak_load_strong(reference);
    weak_destroy(reference);
    object
}

extern "C" fn weak_copy_init(destination: Reference, source: Reference) -> Reference {
    let object = weak_load_strong(source);
    weak_init(destination, object);
    unknown_object_release(object.cast());
    destination
}

extern "C" fn weak_take_init(destination: Reference, source: Reference) -> Reference {
    set_word(destination as usize, word(source as usize));
    destination
}

extern "C" fn weak_copy_assign(destination: Reference, source: Reference) -> Reference {
    if destination != source {
        let object = weak_load_strong(source);
        weak_assign(destination, object);
        unknown_object_release(object.cast());
    }
    destination
}

extern "C" fn weak_take_assign(destination: Reference, source: Reference) -> Reference {
    if destination != source {
        weak_destroy(destination);
        set_word(destination as usize, word(source as usize));
    }
    destination
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let name = name
        .strip_prefix(b"_swift_unknownObjectWeak")
        .or_else(|| name.strip_prefix(b"_swift_weak"))?;
    let address = match name {
        b"Init" => weak_init as *const () as usize,
        b"Assign" => weak_assign as *const () as usize,
        b"LoadStrong" => weak_load_strong as *const () as usize,
        b"TakeStrong" => weak_take_strong as *const () as usize,
        b"Destroy" => weak_destroy as *const () as usize,
        b"CopyInit" => weak_copy_init as *const () as usize,
        b"TakeInit" => weak_take_init as *const () as usize,
        b"CopyAssign" => weak_copy_assign as *const () as usize,
        b"TakeAssign" => weak_take_assign as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

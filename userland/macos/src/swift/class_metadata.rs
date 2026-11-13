//! Swift classes whose layout is finished at run time (D387): `swift_initClassMetadata2` and
//! `swift_updateClassMetadata2` lay a class's stored properties out after its superclass's
//! instance size as the layer's Objective-C runtime knows it (an AppKit class of the layer's
//! is not Apple's size), write the field offsets, the metadata's instance size and alignment
//! and the class's `class_ro_t` and ivar offsets to match, and realize the class in the
//! Objective-C runtime; `swift_getInitializedObjCClass` realizes one.

use super::field_layout::lay_out;
use super::metadata::{word, word32};
use crate::objc::abi::{ClassObject, ClassReadOnly};
use crate::objc::class;
use core::sync::atomic::Ordering;

const SUPERCLASS: usize = 8;
const DATA: usize = 32;
const INSTANCE_SIZE: usize = 48;
const INSTANCE_ALIGN_MASK: usize = 52;
const LAYOUT_SIZE: usize = 0;
const LAYOUT_FLAGS: usize = 16;
const ALIGNMENT_MASK: usize = 0xff;
const HEADER_ALIGN_MASK: usize = 7;
const DATA_FLAG_BITS: usize = 3;
const MOST_FIELDS: usize = 256;

/// What the two calls answer: no dependency, the class is complete.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Dependency {
    metadata: usize,
    state: usize,
}

fn superclass_size(metadata: usize) -> usize {
    let superclass = word(metadata + SUPERCLASS);
    if superclass == 0 {
        return 2 * core::mem::size_of::<usize>();
    }
    match class::prepare(superclass as *mut ClassObject) {
        Some(record) => record.instance_size.load(Ordering::Relaxed),
        None => word32(superclass + INSTANCE_SIZE) as usize,
    }
}

fn lay_out_class(
    metadata: usize,
    count: usize,
    field_types: *const usize,
    field_offsets: *mut usize,
) {
    let count = count.min(MOST_FIELDS);
    let mut fields = [(0usize, 1usize); MOST_FIELDS];
    for (index, field) in fields.iter_mut().enumerate().take(count) {
        // SAFETY: the compiler's array of `count` type layouts for the class's fields.
        let layout = unsafe { field_types.add(index).read() };
        let size = word(layout + LAYOUT_SIZE);
        let alignment = (word32(layout + LAYOUT_FLAGS) as usize & ALIGNMENT_MASK) + 1;
        *field = (size, alignment);
    }
    let start = superclass_size(metadata);
    let mut offsets = [0usize; MOST_FIELDS];
    let (size, mask) = lay_out(
        start,
        &fields[..count],
        &mut offsets[..count],
        HEADER_ALIGN_MASK,
    );
    for (index, &offset) in offsets[..count].iter().enumerate() {
        // SAFETY: the class's field offset vector, `count` words the compiler reserved.
        unsafe { field_offsets.add(index).write(offset) };
    }
    let size_word = (metadata + INSTANCE_SIZE) as *mut u32;
    // SAFETY: the class metadata's instance size and alignment mask, in its writable data.
    unsafe {
        size_word.write(size as u32);
        ((metadata + INSTANCE_ALIGN_MASK) as *mut u16).write(mask as u16);
    }
    let read_only = word(metadata + DATA) & !DATA_FLAG_BITS;
    if read_only == 0 {
        return;
    }
    // SAFETY: the class's `class_ro_t`, in the image's writable data for a class whose layout
    // is finished at run time.
    let read_only = unsafe { &mut *(read_only as *mut ClassReadOnly) };
    read_only.instance_start = start as u32;
    read_only.instance_size = size as u32;
    // SAFETY: the class's ivar list, one ivar per stored property, in order.
    let ivars = unsafe { crate::objc::abi::ivars(read_only.ivars) };
    for (ivar, &offset) in ivars.iter().zip(&offsets[..count]) {
        if !ivar.offset.is_null() {
            // SAFETY: the ivar's offset variable, in the image's writable data.
            unsafe { &*ivar.offset }.store(offset as u32, Ordering::Relaxed);
        }
    }
}

extern "C" fn init_class_metadata(
    metadata: usize,
    _: usize,
    count: usize,
    field_types: *const usize,
    field_offsets: *mut usize,
) -> Dependency {
    lay_out_class(metadata, count, field_types, field_offsets);
    let _ = class::prepare(metadata as *mut ClassObject);
    Dependency {
        metadata: 0,
        state: 0,
    }
}

extern "C" fn get_initialized_objc_class(class: usize) -> usize {
    let _ = class::prepare(class as *mut ClassObject);
    class
}

/// The address of the runtime function `name`, if this module has it.
pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_swift_initClassMetadata2" | b"_swift_updateClassMetadata2" => {
            init_class_metadata as *const () as usize
        }
        b"_swift_getInitializedObjCClass" => get_initialized_objc_class as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

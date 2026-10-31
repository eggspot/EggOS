//! Native Swift objects (D180): `swift_allocObject` puts the metadata pointer and the reference
//! count (`refcount.rs`) at the start of a heap block, so `malloc_size` of an object is its
//! block's size as Swift's array buffers expect; `swift_retain` and `swift_release` count, and
//! the last release runs the class's destructor, which frees the object through
//! `swift_deallocClassInstance`. Bridge objects (a `String`'s or an `Array`'s storage word) and
//! unknown objects tell native Swift objects from Objective-C ones by their pointer bits and
//! their class's flags. A weakly referenced object's last release clears the weak references
//! naming it (`weak.rs`) before its destructor runs. Also static and stack objects,
//! `swift_once`, and the exclusivity checks of `swift_beginAccess`.

use super::metadata::{self, CLASS_ARGUMENTS, Metadata, SWIFT_CLASS, USES_SWIFT_REFCOUNTING, word};
use super::refcount;
use crate::libsystem::{heap, thread};
use crate::objc::reference_count;
use core::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering};

/// The header every native object starts with.
#[repr(C)]
#[derive(Debug)]
pub struct HeapObject {
    pub metadata: AtomicUsize,
    pub refcount: AtomicU64,
}

/// The header of the live object `object`.
pub fn header(object: *mut u8) -> &'static HeapObject {
    // SAFETY: the callers pass live native objects, whose first two words are the header.
    unsafe { &*object.cast::<HeapObject>() }
}

fn out_of_memory() -> ! {
    super::fatal_error(&[b"out of memory for a Swift object"])
}

/// A new object of `size` bytes aligned to `alignment_mask` + 1, counted once.
pub extern "C" fn swift_alloc_object(
    metadata: Metadata,
    size: usize,
    alignment_mask: usize,
) -> *mut u8 {
    let size = size.max(core::mem::size_of::<HeapObject>());
    let block = if alignment_mask < 16 {
        heap::allocate(size, false)
    } else {
        heap::allocate_aligned(alignment_mask + 1, size)
    };
    if block.is_null() {
        out_of_memory();
    }
    let object = HeapObject {
        metadata: AtomicUsize::new(metadata),
        refcount: AtomicU64::new(refcount::INITIAL),
    };
    // SAFETY: a new block of at least the header's size, 16-byte aligned.
    unsafe { block.cast::<HeapObject>().write(object) };
    block
}

pub extern "C" fn swift_retain(object: *mut u8) -> *mut u8 {
    if !object.is_null() {
        refcount::retain(&header(object).refcount, 1);
    }
    object
}

extern "C" fn swift_retain_n(object: *mut u8, count: u32) -> *mut u8 {
    if !object.is_null() {
        refcount::retain(&header(object).refcount, count);
    }
    object
}

pub extern "C" fn swift_release(object: *mut u8) {
    swift_release_n(object, 1);
}

extern "C" fn swift_release_n(object: *mut u8, count: u32) {
    if object.is_null() {
        return;
    }
    let object_header = header(object);
    if refcount::release(&object_header.refcount, count) {
        if refcount::is_weakly_referenced(object_header.refcount.load(Ordering::Acquire)) {
            crate::objc::weak::clear(object.cast());
        }
        let class = object_header.metadata.load(Ordering::Relaxed);
        super::entry::destroy(object as usize, class);
    }
}

/// Whether the native object `object` is never freed.
pub fn is_immortal(object: *mut u8) -> bool {
    refcount::is_immortal(header(object).refcount.load(Ordering::Relaxed))
}

/// Whether the native object `object` has started deinitializing.
pub fn is_deinitializing(object: *mut u8) -> bool {
    refcount::is_deinitializing(header(object).refcount.load(Ordering::Acquire))
}

/// Retains the native object `object` unless it has started deinitializing.
pub fn try_retain(object: *mut u8) -> bool {
    refcount::try_retain(&header(object).refcount)
}

/// Marks the native object `object` as named by a weak reference, so its last release clears
/// the reference.
pub fn mark_weakly_referenced(object: *mut u8) {
    refcount::mark_weakly_referenced(&header(object).refcount);
}

extern "C" fn swift_dealloc_object(object: *mut u8, _: usize, _: usize) {
    heap::release(object);
}

extern "C" fn swift_is_uniquely_referenced_non_null_native(object: *mut u8) -> bool {
    refcount::is_unique(header(object).refcount.load(Ordering::Acquire))
}

extern "C" fn swift_is_uniquely_referenced_native(object: *mut u8) -> bool {
    !object.is_null() && swift_is_uniquely_referenced_non_null_native(object)
}

extern "C" fn swift_init_stack_object(metadata: Metadata, object: *mut u8) -> *mut u8 {
    let stacked = HeapObject {
        metadata: AtomicUsize::new(metadata),
        refcount: AtomicU64::new(refcount::INITIAL),
    };
    // SAFETY: the compiler hands over stack room for the object, at least its header.
    unsafe { object.cast::<HeapObject>().write(stacked) };
    object
}

extern "C" fn swift_init_static_object(metadata: Metadata, object: *mut u8) -> *mut u8 {
    let token = object.wrapping_sub(8).cast::<AtomicIsize>();
    // SAFETY: the compiler puts the static object's once-token in the word before it.
    once(unsafe { &*token }, || {
        let fixed = HeapObject {
            metadata: AtomicUsize::new(metadata),
            refcount: AtomicU64::new(refcount::IMMORTAL),
        };
        // SAFETY: the static object's own header, in the program's writable data.
        unsafe { object.cast::<HeapObject>().write(fixed) };
    });
    object
}

extern "C" fn swift_set_deallocating(object: *mut u8) {
    refcount::set_deinitializing(&header(object).refcount);
}

const ONCE_RUNNING: isize = 1;
const ONCE_DONE: isize = -1;

/// Runs `work` the first time any thread reaches `token` (Darwin's `dispatch_once` states: 0,
/// then all ones when done); later callers wait until it has run.
pub fn once(token: &AtomicIsize, work: impl FnOnce()) {
    if token.load(Ordering::Acquire) == ONCE_DONE {
        return;
    }
    if token
        .compare_exchange(0, ONCE_RUNNING, Ordering::Acquire, Ordering::Acquire)
        .is_ok()
    {
        work();
        token.store(ONCE_DONE, Ordering::Release);
        return;
    }
    while token.load(Ordering::Acquire) != ONCE_DONE {
        thread::sched_yield();
    }
}

extern "C" fn swift_once(
    predicate: *mut isize,
    function: extern "C" fn(*mut u8),
    context: *mut u8,
) {
    // SAFETY: the program's once-token, a word it never touches otherwise.
    let token = unsafe { &*predicate.cast::<AtomicIsize>() };
    once(token, || function(context));
}

const TAGGED_POINTER: u64 = if cfg!(target_arch = "x86_64") {
    (1 << 63) | 1
} else {
    1 << 63
};
const BRIDGE_OBJC: u64 = 1 << 62;
const SPARE_BITS: u64 = if cfg!(target_arch = "x86_64") {
    0xf000_0000_0000_0006
} else {
    0xf000_0000_0000_0007
};

/// Whether `object` (not null, not tagged) counts references the Swift way: its class is Swift
/// class metadata whose flags say so.
pub fn uses_swift_counting(object: *mut u8) -> bool {
    let class = word(object as usize);
    if class < 0x1000 {
        return false;
    }
    let data = word(class + 32);
    data & SWIFT_CLASS != 0 && metadata::word32(class + 40) & USES_SWIFT_REFCOUNTING != 0
}

fn is_tagged(object: *mut u8) -> bool {
    object as u64 & TAGGED_POINTER != 0
}

pub extern "C" fn unknown_object_retain(object: *mut u8) -> *mut u8 {
    unknown_retain_n(object, 1)
}

extern "C" fn unknown_retain_n(object: *mut u8, count: u32) -> *mut u8 {
    if object.is_null() || is_tagged(object) {
        return object;
    }
    if uses_swift_counting(object) {
        return swift_retain_n(object, count);
    }
    for _ in 0..count {
        reference_count::objc_retain(object.cast());
    }
    object
}

pub extern "C" fn unknown_object_release(object: *mut u8) {
    unknown_release_n(object, 1);
}

extern "C" fn unknown_release_n(object: *mut u8, count: u32) {
    if object.is_null() || is_tagged(object) {
        return;
    }
    if uses_swift_counting(object) {
        swift_release_n(object, count);
        return;
    }
    for _ in 0..count {
        reference_count::objc_release(object.cast());
    }
}

fn bridge_target(value: usize) -> Option<(*mut u8, bool)> {
    let bits = value as u64;
    if bits == 0 || bits & TAGGED_POINTER != 0 {
        return None;
    }
    let object = (bits & !SPARE_BITS) as usize as *mut u8;
    (!object.is_null()).then_some((object, bits & BRIDGE_OBJC != 0))
}

pub extern "C" fn swift_bridge_object_retain(value: usize) -> usize {
    bridge_retain_n(value, 1)
}

extern "C" fn bridge_retain_n(value: usize, count: u32) -> usize {
    match bridge_target(value) {
        Some((object, true)) => {
            for _ in 0..count {
                reference_count::objc_retain(object.cast());
            }
        }
        Some((object, false)) => {
            swift_retain_n(object, count);
        }
        None => {}
    }
    value
}

pub extern "C" fn swift_bridge_object_release(value: usize) {
    bridge_release_n(value, 1);
}

extern "C" fn bridge_release_n(value: usize, count: u32) {
    match bridge_target(value) {
        Some((object, true)) => {
            for _ in 0..count {
                reference_count::objc_release(object.cast());
            }
        }
        Some((object, false)) => swift_release_n(object, count),
        None => {}
    }
}

#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)]
struct Access {
    pointer: usize,
    pc: usize,
    next_and_action: usize,
}

const MODIFY: usize = 1;
const TRACKING: usize = 0x20;

fn conflict(pointer: usize) -> ! {
    let mut digits = [0u8; 18];
    super::fatal_access(crate::objc::hexadecimal(pointer, &mut digits))
}

extern "C" fn swift_begin_access(pointer: *mut u8, buffer: *mut u8, flags: usize, pc: *mut u8) {
    let action = flags & MODIFY;
    let current = thread::current();
    // SAFETY: the running thread's own record.
    let head = unsafe { (*current).swift_accesses };
    let mut node = head;
    while node != 0 {
        let seen = word(node);
        let next_and_action = word(node + 16);
        if seen == pointer as usize && (action == MODIFY || next_and_action & MODIFY != 0) {
            conflict(pointer as usize);
        }
        node = next_and_action & !MODIFY;
    }
    if flags & TRACKING != 0 {
        let access = Access {
            pointer: pointer as usize,
            pc: pc as usize,
            next_and_action: head | action,
        };
        // SAFETY: the program's three-word buffer for this access, live until it ends it.
        unsafe { buffer.cast::<Access>().write(access) };
        // SAFETY: as above, the running thread's record.
        unsafe { (*current).swift_accesses = buffer as usize };
    }
}

extern "C" fn swift_end_access(buffer: *mut u8) {
    let current = thread::current();
    let ended = buffer as usize;
    // SAFETY: the running thread's own record.
    let head = unsafe { (*current).swift_accesses };
    let after = word(ended + 16) & !MODIFY;
    if head == ended {
        // SAFETY: as above.
        unsafe { (*current).swift_accesses = after };
        return;
    }
    let mut node = head;
    while node != 0 {
        let next_and_action = word(node + 16);
        let next = next_and_action & !MODIFY;
        if next == ended {
            metadata::set_word(node + 16, after | (next_and_action & MODIFY));
            return;
        }
        node = next;
    }
}

/// The generic argument `index` of the generic class instance `class`.
pub fn class_argument(class: Metadata, index: usize) -> Metadata {
    word(class + CLASS_ARGUMENTS + 8 * index)
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"_swift_allocObject" => swift_alloc_object as *const () as usize,
        b"_swift_retain" => swift_retain as *const () as usize,
        b"_swift_retain_n" => swift_retain_n as *const () as usize,
        b"_swift_release" => swift_release as *const () as usize,
        b"_swift_release_n" => swift_release_n as *const () as usize,
        b"_swift_deallocObject" | b"_swift_deallocClassInstance" => {
            swift_dealloc_object as *const () as usize
        }
        b"_swift_isUniquelyReferenced_nonNull_native" => {
            swift_is_uniquely_referenced_non_null_native as *const () as usize
        }
        b"_swift_isUniquelyReferenced_native" => {
            swift_is_uniquely_referenced_native as *const () as usize
        }
        b"_swift_initStackObject" => swift_init_stack_object as *const () as usize,
        b"_swift_initStaticObject" => swift_init_static_object as *const () as usize,
        b"_swift_setDeallocating" => swift_set_deallocating as *const () as usize,
        b"_swift_once" => swift_once as *const () as usize,
        b"_swift_unknownObjectRetain" => unknown_object_retain as *const () as usize,
        b"_swift_unknownObjectRetain_n" => unknown_retain_n as *const () as usize,
        b"_swift_unknownObjectRelease" => unknown_object_release as *const () as usize,
        b"_swift_unknownObjectRelease_n" => unknown_release_n as *const () as usize,
        b"_swift_bridgeObjectRetain" => swift_bridge_object_retain as *const () as usize,
        b"_swift_bridgeObjectRetain_n" => bridge_retain_n as *const () as usize,
        b"_swift_bridgeObjectRelease" => swift_bridge_object_release as *const () as usize,
        b"_swift_bridgeObjectRelease_n" => bridge_release_n as *const () as usize,
        b"_swift_beginAccess" => swift_begin_access as *const () as usize,
        b"_swift_endAccess" => swift_end_access as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

//! Blocks (D175), as clang lays them out and Apple's `libsystem_blocks` copies them: a block
//! literal starts on the stack (`_NSConcreteStackBlock`) or as a constant
//! (`_NSConcreteGlobalBlock`); `_Block_copy` moves a stack block to the heap
//! (`_NSConcreteMallocBlock`), with a reference count in its flags and its captured objects,
//! blocks and `__block` variables kept through the compiler's copy helper; `_Block_release`
//! drops one reference and disposes of the captures with the last. Blocks are objects of the
//! runtime: `copy`, `retain`, `release` and `autorelease` work on them, and ARC's
//! `objc_retainBlock` is `_Block_copy`.

use super::abi::{ClassObject, Id, Sel};
use super::class::{self, BuiltinClass, IMMORTAL};
use super::reference_count::{objc_release, objc_retain};
use super::root::NSOBJECT;
use super::{Runtime, autorelease};
use crate::libsystem::heap;
use core::sync::atomic::{AtomicI32, Ordering};

pub static NSBLOCK: ClassObject = ClassObject::EMPTY;
static NSBLOCK_META: ClassObject = ClassObject::EMPTY;
pub static STACK_BLOCK: ClassObject = ClassObject::EMPTY;
static STACK_BLOCK_META: ClassObject = ClassObject::EMPTY;
pub static MALLOC_BLOCK: ClassObject = ClassObject::EMPTY;
static MALLOC_BLOCK_META: ClassObject = ClassObject::EMPTY;
pub static GLOBAL_BLOCK: ClassObject = ClassObject::EMPTY;
static GLOBAL_BLOCK_META: ClassObject = ClassObject::EMPTY;

const DEALLOCATING: i32 = 0x0001;
const REFERENCE_COUNT_MASK: i32 = 0xfffe;
const NEEDS_FREE: i32 = 1 << 24;
const HAS_COPY_DISPOSE: i32 = 1 << 25;
const IS_GLOBAL: i32 = 1 << 28;

const BYREF_NEEDS_FREE: i32 = 1 << 24;
const BYREF_HAS_COPY_DISPOSE: i32 = 1 << 25;
const BYREF_LAYOUT_EXTENDED: i32 = 1 << 28;

const FIELD_IS_OBJECT: i32 = 3;
const FIELD_IS_BLOCK: i32 = 7;
const FIELD_IS_BYREF: i32 = 8;
const FIELD_IS_WEAK: i32 = 16;
const ALL_COPY_DISPOSE_FLAGS: i32 = 0x9f;

const FLAGS_OFFSET: usize = 8;
const INVOKE_OFFSET: usize = 16;
const DESCRIPTOR_OFFSET: usize = 24;
const DESCRIPTOR_SIZE_OFFSET: usize = 8;
const DESCRIPTOR_COPY_OFFSET: usize = 16;
const DESCRIPTOR_DISPOSE_OFFSET: usize = 24;

const BYREF_FORWARDING_OFFSET: usize = 8;
const BYREF_FLAGS_OFFSET: usize = 16;
const BYREF_SIZE_OFFSET: usize = 20;
const BYREF_HEADER: usize = 24;
const BYREF_KEEP_OFFSET: usize = 24;
const BYREF_DESTROY_OFFSET: usize = 32;
const BYREF_LAYOUT_OFFSET: usize = 40;

fn word_at(base: *const u8, offset: usize) -> usize {
    // SAFETY: the callers read the fields of a block, descriptor or `__block` variable the
    // compiler laid out, which hold a word at `offset`.
    unsafe { base.wrapping_add(offset).cast::<usize>().read_unaligned() }
}

fn function_at(base: *const u8, offset: usize) -> usize {
    crate::callback::native_callable(word_at(base, offset))
}

fn flags_at(base: *const u8, offset: usize) -> &'static AtomicI32 {
    // SAFETY: as above: the 32-bit flags word of a block or `__block` variable, which lives at
    // least as long as the reference the caller holds.
    unsafe { &*base.wrapping_add(offset).cast::<AtomicI32>() }
}

fn latching_increment(flags: &AtomicI32) {
    let mut old = flags.load(Ordering::Relaxed);
    loop {
        if old & REFERENCE_COUNT_MASK == REFERENCE_COUNT_MASK {
            return;
        }
        match flags.compare_exchange_weak(old, old + 2, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return,
            Err(now) => old = now,
        }
    }
}

fn latching_decrement_is_last(flags: &AtomicI32) -> bool {
    let mut old = flags.load(Ordering::Relaxed);
    loop {
        let count = old & REFERENCE_COUNT_MASK;
        if count == REFERENCE_COUNT_MASK || count == 0 {
            return false;
        }
        let last = old & (REFERENCE_COUNT_MASK | DEALLOCATING) == 2;
        let new = if last { old - 1 } else { old - 2 };
        match flags.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return last,
            Err(now) => old = now,
        }
    }
}

/// `_Block_copy`: a heap block of `block`, counted once more, or the block itself when it is
/// a constant or already on the heap.
pub extern "C" fn block_copy(block: *const u8) -> *mut u8 {
    if block.is_null() {
        return core::ptr::null_mut();
    }
    let flags = flags_at(block, FLAGS_OFFSET);
    let value = flags.load(Ordering::Relaxed);
    if value & NEEDS_FREE != 0 {
        latching_increment(flags);
        return block.cast_mut();
    }
    if value & IS_GLOBAL != 0 {
        return block.cast_mut();
    }
    let descriptor = word_at(block, DESCRIPTOR_OFFSET) as *const u8;
    let size = word_at(descriptor, DESCRIPTOR_SIZE_OFFSET);
    if size < DESCRIPTOR_OFFSET + 8 {
        return core::ptr::null_mut();
    }
    let copy = heap::allocate(size, false);
    if copy.is_null() {
        return copy;
    }
    // SAFETY: the stack block is `size` bytes (its descriptor says so) and the new block holds
    // as many.
    unsafe { copy.copy_from_nonoverlapping(block, size) };
    let copied = (value & !(REFERENCE_COUNT_MASK | DEALLOCATING)) | NEEDS_FREE | 2;
    flags_at(copy, FLAGS_OFFSET).store(copied, Ordering::Relaxed);
    // SAFETY: the copy's first word is its class.
    unsafe {
        copy.cast::<*const ClassObject>()
            .write(&raw const MALLOC_BLOCK)
    };
    if value & HAS_COPY_DISPOSE != 0 {
        // SAFETY: a block with the flag has the compiler's copy helper in its descriptor.
        let helper: extern "C" fn(*mut u8, *const u8) =
            unsafe { core::mem::transmute(function_at(descriptor, DESCRIPTOR_COPY_OFFSET)) };
        helper(copy, block);
    }
    copy
}

/// `_Block_release`: drops one reference to a heap block, disposing of it with the last.
pub extern "C" fn block_release(block: *const u8) {
    if block.is_null() {
        return;
    }
    let flags = flags_at(block, FLAGS_OFFSET);
    let value = flags.load(Ordering::Relaxed);
    if value & IS_GLOBAL != 0 || value & NEEDS_FREE == 0 {
        return;
    }
    if !latching_decrement_is_last(flags) {
        return;
    }
    if value & HAS_COPY_DISPOSE != 0 {
        let descriptor = word_at(block, DESCRIPTOR_OFFSET) as *const u8;
        // SAFETY: a block with the flag has the compiler's dispose helper in its descriptor.
        let helper: extern "C" fn(*const u8) =
            unsafe { core::mem::transmute(function_at(descriptor, DESCRIPTOR_DISPOSE_OFFSET)) };
        helper(block);
    }
    heap::release(block.cast_mut());
}

fn byref_copy(variable: *const u8) -> *const u8 {
    let source = word_at(variable, BYREF_FORWARDING_OFFSET) as *mut u8;
    let source_flags = flags_at(source, BYREF_FLAGS_OFFSET).load(Ordering::Relaxed);
    if source_flags & REFERENCE_COUNT_MASK == 0 {
        // SAFETY: the variable's 32-bit size field.
        let size = unsafe { source.wrapping_add(BYREF_SIZE_OFFSET).cast::<u32>().read() } as usize;
        if size < BYREF_HEADER {
            return variable;
        }
        let copy = heap::allocate(size, true);
        if copy.is_null() {
            return variable;
        }
        let copied = source_flags | BYREF_NEEDS_FREE | 4;
        // SAFETY: the new block holds `size` bytes; the header's words are written in place.
        unsafe { copy.cast::<usize>().write(0) };
        // SAFETY: as above.
        unsafe {
            copy.wrapping_add(BYREF_FORWARDING_OFFSET)
                .cast::<*mut u8>()
                .write(copy)
        };
        // SAFETY: as above.
        unsafe {
            copy.wrapping_add(BYREF_FLAGS_OFFSET)
                .cast::<i32>()
                .write(copied)
        };
        // SAFETY: as above.
        unsafe {
            copy.wrapping_add(BYREF_SIZE_OFFSET)
                .cast::<u32>()
                .write(size as u32)
        };
        // SAFETY: the variable's forwarding word, which now names the copy.
        unsafe {
            source
                .wrapping_add(BYREF_FORWARDING_OFFSET)
                .cast::<*mut u8>()
                .write(copy)
        };
        if source_flags & BYREF_HAS_COPY_DISPOSE != 0 {
            let keep = word_at(source, BYREF_KEEP_OFFSET);
            let destroy = word_at(source, BYREF_DESTROY_OFFSET);
            // SAFETY: a variable with the flag has its keep and destroy helpers after the
            // header; the copy has room for them.
            unsafe {
                copy.wrapping_add(BYREF_KEEP_OFFSET)
                    .cast::<usize>()
                    .write(keep)
            };
            // SAFETY: as above.
            unsafe {
                copy.wrapping_add(BYREF_DESTROY_OFFSET)
                    .cast::<usize>()
                    .write(destroy)
            };
            if source_flags & BYREF_LAYOUT_EXTENDED != 0 {
                // SAFETY: with the layout flag the variable's layout follows its helpers; the
                // copy has room for it.
                unsafe {
                    copy.wrapping_add(BYREF_LAYOUT_OFFSET)
                        .cast::<usize>()
                        .write(word_at(source, BYREF_LAYOUT_OFFSET))
                };
            }
            // SAFETY: the compiler's keep helper copies the variable's value.
            let keep: extern "C" fn(*mut u8, *mut u8) =
                unsafe { core::mem::transmute(crate::callback::native_callable(keep)) };
            keep(copy, source);
        } else {
            // SAFETY: both blocks hold `size` bytes; the value follows the header.
            unsafe {
                copy.wrapping_add(BYREF_HEADER).copy_from_nonoverlapping(
                    source.wrapping_add(BYREF_HEADER),
                    size - BYREF_HEADER,
                )
            };
        }
        return copy;
    }
    if source_flags & BYREF_NEEDS_FREE != 0 {
        latching_increment(flags_at(source, BYREF_FLAGS_OFFSET));
    }
    source
}

fn byref_release(variable: *const u8) {
    let variable = word_at(variable, BYREF_FORWARDING_OFFSET) as *mut u8;
    let flags = flags_at(variable, BYREF_FLAGS_OFFSET);
    let value = flags.load(Ordering::Relaxed);
    if value & BYREF_NEEDS_FREE == 0 || !latching_decrement_is_last(flags) {
        return;
    }
    if value & BYREF_HAS_COPY_DISPOSE != 0 {
        // SAFETY: a variable with the flag has its destroy helper after the header.
        let destroy: extern "C" fn(*mut u8) =
            unsafe { core::mem::transmute(function_at(variable, BYREF_DESTROY_OFFSET)) };
        destroy(variable);
    }
    heap::release(variable);
}

extern "C" fn object_assign(destination: *mut *const u8, object: *const u8, flags: i32) {
    let kind = flags & ALL_COPY_DISPOSE_FLAGS;
    let value = if kind == FIELD_IS_OBJECT {
        objc_retain(object.cast_mut().cast());
        object
    } else if kind == FIELD_IS_BLOCK {
        block_copy(object).cast_const()
    } else if kind == FIELD_IS_BYREF || kind == FIELD_IS_BYREF | FIELD_IS_WEAK {
        byref_copy(object)
    } else {
        object
    };
    // SAFETY: the compiler's copy helper passes the address of the field it copies into.
    unsafe { destination.write(value) };
}

extern "C" fn object_dispose(object: *const u8, flags: i32) {
    let kind = flags & ALL_COPY_DISPOSE_FLAGS;
    if kind == FIELD_IS_BYREF || kind == FIELD_IS_BYREF | FIELD_IS_WEAK {
        byref_release(object);
    } else if kind == FIELD_IS_BLOCK {
        block_release(object);
    } else if kind == FIELD_IS_OBJECT {
        objc_release(object.cast_mut().cast());
    }
}

/// The function a block runs, which takes the block first and then its own arguments: a
/// native function the layer can call, also when the block is a translated program's.
pub fn invoke_address(block: Id) -> usize {
    function_at(block.cast_const().cast(), INVOKE_OFFSET)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    block_copy(this.cast_const().cast()).cast()
}

extern "C" fn copy(this: Id, _: Sel) -> Id {
    block_copy(this.cast_const().cast()).cast()
}

extern "C" fn this_object(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn nothing(_: Id, _: Sel) {}

extern "C" fn heap_retain(this: Id, _: Sel) -> Id {
    block_copy(this.cast_const().cast()).cast()
}

extern "C" fn heap_release(this: Id, _: Sel) {
    block_release(this.cast_const().cast());
}

extern "C" fn heap_autorelease(this: Id, _: Sel) -> Id {
    autorelease::add(this)
}

extern "C" fn retain_count(this: Id, _: Sel) -> usize {
    let flags = flags_at(this.cast_const().cast(), FLAGS_OFFSET).load(Ordering::Relaxed);
    ((flags & REFERENCE_COUNT_MASK) / 2) as usize
}

extern "C" fn invoke(this: Id, _: Sel) {
    // SAFETY: a block's function takes the block; `invoke` is for blocks without arguments.
    let function: extern "C" fn(Id) = unsafe { core::mem::transmute(invoke_address(this)) };
    function(this);
}

fn define_one(
    runtime: &mut Runtime,
    name: &'static [u8],
    pair: (&'static ClassObject, &'static ClassObject),
    superclass: &'static ClassObject,
    flags: u32,
    instance_methods: &[(&'static [u8], usize)],
) -> bool {
    class::define(
        runtime,
        &BuiltinClass {
            name,
            class: pair.0,
            meta: pair.1,
            superclass: Some(superclass),
            instance_size: DESCRIPTOR_OFFSET + 8,
            flags,
            instance_methods,
            class_methods: &[],
        },
    )
}

/// Defines NSBlock and the three block classes; false if there is no memory.
pub fn define(runtime: &mut Runtime) -> bool {
    let block = methods![
        "copy" => copy,
        "copyWithZone:" => copy_with_zone,
        "invoke" => invoke,
    ];
    let constant = methods![
        "retain" => this_object,
        "release" => nothing,
        "autorelease" => this_object,
        "retainCount" => retain_count,
    ];
    let heap_block = methods![
        "retain" => heap_retain,
        "release" => heap_release,
        "autorelease" => heap_autorelease,
        "retainCount" => retain_count,
    ];
    define_one(
        runtime,
        b"NSBlock\0",
        (&NSBLOCK, &NSBLOCK_META),
        &NSOBJECT,
        0,
        &block,
    ) && define_one(
        runtime,
        b"__NSStackBlock__\0",
        (&STACK_BLOCK, &STACK_BLOCK_META),
        &NSBLOCK,
        0,
        &constant,
    ) && define_one(
        runtime,
        b"__NSMallocBlock__\0",
        (&MALLOC_BLOCK, &MALLOC_BLOCK_META),
        &NSBLOCK,
        0,
        &heap_block,
    ) && define_one(
        runtime,
        b"__NSGlobalBlock__\0",
        (&GLOBAL_BLOCK, &GLOBAL_BLOCK_META),
        &NSBLOCK,
        IMMORTAL,
        &constant,
    )
}

pub fn resolve(name: &[u8]) -> Option<u64> {
    let address = match name {
        b"__NSConcreteStackBlock" => &raw const STACK_BLOCK as usize,
        b"__NSConcreteMallocBlock" => &raw const MALLOC_BLOCK as usize,
        b"__NSConcreteGlobalBlock" => &raw const GLOBAL_BLOCK as usize,
        b"__Block_copy" | b"_objc_retainBlock" => block_copy as *const () as usize,
        b"__Block_release" => block_release as *const () as usize,
        b"__Block_object_assign" => object_assign as *const () as usize,
        b"__Block_object_dispose" => object_dispose as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

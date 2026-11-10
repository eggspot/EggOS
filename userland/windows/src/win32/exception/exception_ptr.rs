//! `std::exception_ptr` (D230): msvcp140's `__ExceptionPtr*` functions. A program's
//! `exception_ptr` is two words; the first names a reference-counted copy of an exception, its
//! record and, for a C++ exception, a copy of its object made with the most derived catchable
//! type's copy constructor (byte for byte without one), which the last reference destroys with
//! the type's destructor. `std::rethrow_exception` throws a fresh copy of the object, kept on
//! the thread's list until the catch block that takes it destroys it, so a handler never
//! changes the held one. A structured exception is held as its record alone and rethrown as
//! `RaiseException` raises it.

use super::super::heap;
use super::super::thread_environment::{self, teb};
use super::context::{Context, capturing_entry};
use super::dispatch::{self, Origin};
use super::frames::{self, Bridge};
use super::{ExceptionRecord, cxx, thrown};
use crate::cxx_data::{self, CatchableType, flags};
use crate::cxx_walk;
use crate::library::Export;
use crate::std_exceptions::StdException;
use core::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
struct ExceptionPtr {
    held: *mut Held,
    #[allow(dead_code, reason = "the program's object has the word")]
    unused: u64,
}

#[repr(C)]
struct Held {
    references: AtomicU64,
    record: ExceptionRecord,
    object_size: u64,
}

const OBJECT_OFFSET: usize = (core::mem::size_of::<Held>() + 15) & !15;
const COPY_OFFSET: usize = 16;

fn object_of(held: *mut Held) -> *mut u8 {
    held.cast::<u8>().wrapping_add(OBJECT_OFFSET)
}

fn most_derived(record: &ExceptionRecord) -> Option<CatchableType> {
    if !cxx_walk::is_cxx_exception(record.code, record.parameters()) {
        return None;
    }
    let image = thrown::image_of(record);
    let info = cxx_data::throw_info(&image, record.parameters[2])?;
    cxx_data::catchable_type(&image, &info, 0)
}

fn copy_object(catchable: &CatchableType, from: u64, to: *mut u8) {
    let size = catchable.size as usize;
    if catchable.copy_function == 0 || catchable.properties & flags::CATCHABLE_SIMPLE_TYPE != 0 {
        // SAFETY: `to` holds `size` bytes, and the thrown object is at least that big.
        unsafe { core::ptr::copy_nonoverlapping(from as *const u8, to, size) };
        return;
    }
    let virtual_base = catchable.properties & flags::CATCHABLE_VIRTUAL_BASE != 0;
    let mut bridge = Bridge::new(None, None);
    // SAFETY: the type's copy constructor, constructing the copy at `to` from the object.
    unsafe {
        frames::call_program(
            &mut bridge,
            catchable.copy_function,
            [to as u64, from, u64::from(virtual_base), 0],
        )
    };
}

fn destroy_object(record: &ExceptionRecord, object: *mut u8) {
    let image = thrown::image_of(record);
    let Some(info) = cxx_data::throw_info(&image, record.parameters[2]) else {
        return;
    };
    if info.destructor != 0 {
        let mut bridge = Bridge::new(None, None);
        // SAFETY: the thrown type's destructor on the held copy, which is never used again.
        unsafe { frames::call_program(&mut bridge, info.destructor, [object as u64, 0, 0, 0]) };
    }
}

fn hold(record: &ExceptionRecord) -> *mut Held {
    let catchable = most_derived(record);
    let size = catchable.map_or(0, |catchable| catchable.size as usize);
    let held = heap::allocate(OBJECT_OFFSET + size).cast::<Held>();
    if held.is_null() {
        return held;
    }
    let mut kept = *record;
    if let Some(catchable) = catchable {
        copy_object(&catchable, record.parameters[1], object_of(held));
        kept.parameters[1] = object_of(held) as u64;
    }
    // SAFETY: a fresh block big enough for the header.
    unsafe {
        held.write(Held {
            references: AtomicU64::new(1),
            record: kept,
            object_size: size as u64,
        })
    };
    held
}

fn add_reference(held: *mut Held) {
    // SAFETY: a held exception lives while a reference to it does, and the caller has one.
    if let Some(held) = unsafe { held.as_ref() } {
        held.references.fetch_add(1, Ordering::Relaxed);
    }
}

fn release(held: *mut Held) {
    // SAFETY: as in `add_reference`.
    let Some(kept) = (unsafe { held.as_ref() }) else {
        return;
    };
    if kept.references.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    if kept.object_size != 0 {
        destroy_object(&kept.record, object_of(held));
    }
    // SAFETY: the last reference is gone; the block came from this heap.
    unsafe { heap::free(held.cast()) };
}

fn pointer<'a>(pointer: *mut ExceptionPtr) -> &'a mut ExceptionPtr {
    // SAFETY: the functions' contract: the program's `exception_ptr`, two writable words.
    unsafe { &mut *pointer }
}

unsafe extern "efiapi" fn create(target: *mut ExceptionPtr) {
    *pointer(target) = ExceptionPtr {
        held: core::ptr::null_mut(),
        unused: 0,
    };
}

unsafe extern "efiapi" fn destroy(target: *mut ExceptionPtr) {
    let target = pointer(target);
    release(target.held);
    target.held = core::ptr::null_mut();
}

unsafe extern "efiapi" fn copy(target: *mut ExceptionPtr, source: *mut ExceptionPtr) {
    let held = pointer(source).held;
    add_reference(held);
    *pointer(target) = ExceptionPtr { held, unused: 0 };
}

unsafe extern "efiapi" fn assign(target: *mut ExceptionPtr, source: *mut ExceptionPtr) {
    let held = pointer(source).held;
    add_reference(held);
    let target = pointer(target);
    release(core::mem::replace(&mut target.held, held));
}

unsafe extern "efiapi" fn compare(left: *mut ExceptionPtr, right: *mut ExceptionPtr) -> bool {
    pointer(left).held == pointer(right).held
}

unsafe extern "efiapi" fn to_bool(source: *mut ExceptionPtr) -> bool {
    !pointer(source).held.is_null()
}

unsafe extern "efiapi" fn swap(left: *mut ExceptionPtr, right: *mut ExceptionPtr) {
    let left = pointer(left);
    let right = pointer(right);
    core::mem::swap(&mut left.held, &mut right.held);
}

unsafe extern "efiapi" fn current_exception(target: *mut ExceptionPtr) {
    let held = match cxx::innermost_catch() {
        Some(catch) => hold(&catch.record),
        None => core::ptr::null_mut(),
    };
    *pointer(target) = ExceptionPtr { held, unused: 0 };
}

unsafe extern "efiapi" fn copy_exception(target: *mut ExceptionPtr, object: u64, info: u64) {
    let image_base = thrown::image_base_of_throw_info(info);
    let record = ExceptionRecord::new(
        cxx_data::CXX_EXCEPTION,
        super::flag::NONCONTINUABLE,
        0,
        &[cxx_data::CXX_MAGIC, object, info, image_base],
    );
    let held = hold(&record);
    let target = pointer(target);
    release(core::mem::replace(&mut target.held, held));
}

fn copies_head() -> *mut u64 {
    (thread_environment::current() + teb::RETHROWN_COPIES) as *mut u64
}

/// Frees the copy of a held object that `std::rethrow_exception` threw, once the catch block
/// that took it has destroyed it; any other object is not on the list and is left alone.
pub fn forget_rethrown_copy(object: u64) {
    let mut link = copies_head();
    loop {
        // SAFETY: the list holds blocks this thread allocated, each starting with its next
        // link, and only this thread changes it.
        let next = unsafe { link.read() };
        if next == 0 {
            return;
        }
        let block = next as *mut u64;
        if block as u64 + COPY_OFFSET as u64 == object {
            // SAFETY: as above; `block` is on the list, so its first word is its next link.
            let after = unsafe { block.read() };
            // SAFETY: as above.
            unsafe { link.write(after) };
            // SAFETY: the block came from this heap and is now off the list.
            unsafe { heap::free(block.cast()) };
            return;
        }
        link = block;
    }
}

capturing_entry!(rethrow_entry => rethrow);

extern "C" fn rethrow(source: u64, _: u64, _: u64, _: u64, context: *mut Context) -> ! {
    let held = pointer(source as *mut ExceptionPtr).held;
    // SAFETY: as in `add_reference`.
    let Some(kept) = (unsafe { held.as_ref() }) else {
        thrown::throw(StdException::BadException, core::ptr::null(), context);
    };
    let mut record = kept.record;
    let Some(catchable) = most_derived(&record) else {
        // SAFETY: the capturing entry's `CONTEXT`.
        record.address = unsafe { (*context).pc() };
        dispatch::dispatch(&raw mut record, context, Origin::Raised);
    };
    let block = heap::allocate(COPY_OFFSET + kept.object_size as usize);
    if block.is_null() {
        thrown::throw(StdException::BadAlloc, core::ptr::null(), context);
    }
    let copy = block.wrapping_add(COPY_OFFSET);
    copy_object(&catchable, object_of(held) as u64, copy);
    let head = copies_head();
    // SAFETY: this thread's list head.
    let first = unsafe { head.read() };
    // SAFETY: a fresh block whose first word links it into this thread's list.
    unsafe { block.cast::<u64>().write(first) };
    // SAFETY: this thread's list head.
    unsafe { head.write(block as u64) };
    thrown::raise(
        copy as u64,
        record.parameters[2],
        record.parameters[3],
        context,
    )
}

/// msvcp140's `exception_ptr` functions, under their decorated names and their plain ones.
pub(super) const EXPORTS: &[Export] = exports![
    b"?__ExceptionPtrCreate@@YAXPEAX@Z" => create,
    b"?__ExceptionPtrDestroy@@YAXPEAX@Z" => destroy,
    b"?__ExceptionPtrCopy@@YAXPEAXPEBX@Z" => copy,
    b"?__ExceptionPtrAssign@@YAXPEAXPEBX@Z" => assign,
    b"?__ExceptionPtrCompare@@YA_NPEBX0@Z" => compare,
    b"?__ExceptionPtrToBool@@YA_NPEBX@Z" => to_bool,
    b"?__ExceptionPtrSwap@@YAXPEAX0@Z" => swap,
    b"?__ExceptionPtrCurrentException@@YAXPEAX@Z" => current_exception,
    b"?__ExceptionPtrRethrow@@YAXPEBX@Z" => rethrow_entry [direct],
    b"?__ExceptionPtrCopyException@@YAXPEAXPEBX1@Z" => copy_exception,
    b"__ExceptionPtrCreate" => create,
    b"__ExceptionPtrDestroy" => destroy,
    b"__ExceptionPtrCopy" => copy,
    b"__ExceptionPtrAssign" => assign,
    b"__ExceptionPtrCompare" => compare,
    b"__ExceptionPtrToBool" => to_bool,
    b"__ExceptionPtrSwap" => swap,
    b"__ExceptionPtrCurrentException" => current_exception,
    b"__ExceptionPtrRethrow" => rethrow_entry [direct],
    b"__ExceptionPtrCopyException" => copy_exception,
];

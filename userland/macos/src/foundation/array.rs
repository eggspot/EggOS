//! NSArray, NSMutableArray, the constant array clang's `@[...]` literals use and the empty
//! array (`___NSArray0__struct`). They share clang's `NSConstantArray` layout, `{isa, count,
//! objects}`; mutable arrays add a capacity and a mutation count for fast enumeration. An
//! array holds a reference to each object. Code that may run the program's methods (`release`,
//! `isEqual:`, `compare:`) reads the count and storage again after each call.

use super::{
    CONSTANT_ARRAY, CONSTANT_ARRAY_META, Definition, NSARRAY, NSARRAY_META, NSMUTABLEARRAY,
    NSMUTABLEARRAY_META, autoreleased, class_pointer, define as define_class, description,
    is_array, new_instance,
};
use crate::format::Arguments;
use crate::libsystem::heap;
use crate::libsystem::variadic::{RawVaList, VaList, variadic};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::class::IMMORTAL;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::sel;
use crate::objc::{Runtime, methods, send, send_bool, send1, terminate_uncaught};
use core::sync::atomic::{AtomicUsize, Ordering};

/// An array object's fields.
#[repr(C)]
#[derive(Debug)]
pub struct ArrayObject {
    isa: usize,
    count: usize,
    items: *mut Id,
}

/// A mutable array's fields.
#[repr(C)]
#[derive(Debug)]
pub struct MutableArrayObject {
    array: ArrayObject,
    capacity: usize,
    mutations: usize,
}

const NOT_FOUND: usize = isize::MAX as usize;

fn fields(object: Id) -> *mut ArrayObject {
    object.cast()
}

fn is_mutable(object: Id) -> bool {
    root::is_kind_of(object, &NSMUTABLEARRAY)
}

/// How many objects the array holds.
pub fn count_of(object: Id) -> usize {
    // SAFETY: the callers pass arrays, which share the layout.
    unsafe { (*fields(object)).count }
}

/// The object at `index`, below the count.
pub fn item(object: Id, index: usize) -> Id {
    // SAFETY: as above.
    let items = unsafe { (*fields(object)).items };
    // SAFETY: `index` is below the count, so inside the storage.
    unsafe { items.wrapping_add(index).read() }
}

/// A new array of `class` (counted once) holding `items`, each retained.
pub fn new_array(class: *mut ClassObject, items: &[Id]) -> Id {
    let object = new_instance(class);
    if object.is_null() {
        return object;
    }
    if !store(object, items) {
        objc_release(object);
        return core::ptr::null_mut();
    }
    object
}

fn store(object: Id, items: &[Id]) -> bool {
    let storage =
        heap::allocate(items.len().max(1) * core::mem::size_of::<Id>(), false).cast::<Id>();
    if storage.is_null() {
        return false;
    }
    for (index, &item) in items.iter().enumerate() {
        objc_retain(item);
        // SAFETY: the new block holds `items.len()` objects.
        unsafe { storage.wrapping_add(index).write(item) };
    }
    // SAFETY: a new array the caller owns.
    let array = unsafe { &mut *fields(object) };
    array.items = storage;
    array.count = items.len();
    if is_mutable(object) {
        // SAFETY: a mutable array has the mutable fields.
        unsafe { (*object.cast::<MutableArrayObject>()).capacity = items.len().max(1) };
    }
    true
}

fn snapshot(object: Id) -> Buffer<Id> {
    let mut items = Buffer::new();
    let count = count_of(object);
    items.reserve(count);
    for index in 0..count {
        items.push(item(object, index));
    }
    items
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

fn out_of_range(this: Id, method: &[u8], index: usize) -> ! {
    let count = count_of(this);
    let mut index_digits = [0u8; 24];
    let mut last_digits = [0u8; 24];
    let index_text = crate::format::decimal(index as u64, &mut index_digits);
    let last_text = crate::format::decimal(count.saturating_sub(1) as u64, &mut last_digits);
    let bounds: [&[u8]; 3] = if count == 0 {
        [b" beyond bounds for empty array", b"", b""]
    } else {
        [b" beyond bounds [0 .. ", last_text, b"]"]
    };
    terminate_uncaught(
        b"NSRangeException",
        &[
            b"*** -[",
            root::class_name(this),
            b" ",
            method,
            b"]: index ",
            index_text,
            bounds[0],
            bounds[1],
            bounds[2],
        ],
    )
}

extern "C" fn array(this: Id, _: Sel) -> Id {
    autoreleased(new_array(this_class(this), &[]))
}

extern "C" fn array_with_object(this: Id, _: Sel, object: Id) -> Id {
    autoreleased(new_array(this_class(this), &[object]))
}

unsafe extern "C" fn array_with_objects_count(
    this: Id,
    _: Sel,
    objects: *const Id,
    count: usize,
) -> Id {
    // SAFETY: the caller's promise.
    let items = unsafe { objects_slice(objects, count) };
    autoreleased(new_array(this_class(this), items))
}

/// # Safety
/// `objects` must be null with `count` 0, or hold `count` objects.
pub unsafe fn objects_slice<'a>(objects: *const Id, count: usize) -> &'a [Id] {
    if count == 0 || objects.is_null() {
        return &[];
    }
    // SAFETY: the caller's promise.
    unsafe { core::slice::from_raw_parts(objects, count) }
}

extern "C" fn array_with_objects_list(this: Id, _: Sel, first: Id, arguments: RawVaList) -> Id {
    let mut items = Buffer::new();
    if !first.is_null() {
        items.push(first);
        // SAFETY: the program's nil-terminated list of objects.
        let mut list = unsafe { VaList::new(arguments) };
        loop {
            let next = list.integer() as usize as Id;
            if next.is_null() {
                break;
            }
            items.push(next);
        }
    }
    let object = new_array(this_class(this), items.as_slice());
    items.free();
    autoreleased(object)
}

variadic!(eggos_array_with_objects => array_with_objects_list, 3, "x3", "rcx");

extern "C" fn array_with_array(this: Id, _: Sel, other: Id) -> Id {
    let mut items = if other.is_null() {
        Buffer::new()
    } else {
        snapshot(other)
    };
    let object = new_array(this_class(this), items.as_slice());
    items.free();
    autoreleased(object)
}

extern "C" fn array_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    autoreleased(new_array(this_class(this), &[]))
}

fn initialized(this: Id, items: &[Id]) -> Id {
    if store(this, items) {
        this
    } else {
        objc_release(this);
        core::ptr::null_mut()
    }
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    initialized(this, &[])
}

extern "C" fn init_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    initialized(this, &[])
}

unsafe extern "C" fn init_with_objects_count(
    this: Id,
    _: Sel,
    objects: *const Id,
    count: usize,
) -> Id {
    // SAFETY: the caller's promise.
    initialized(this, unsafe { objects_slice(objects, count) })
}

extern "C" fn init_with_array(this: Id, _: Sel, other: Id) -> Id {
    let mut items = if other.is_null() {
        Buffer::new()
    } else {
        snapshot(other)
    };
    let result = initialized(this, items.as_slice());
    items.free();
    result
}

fn release_items(this: Id) {
    loop {
        // SAFETY: an array being deallocated, which only this thread uses.
        let array = unsafe { &mut *fields(this) };
        let Some(last) = array.count.checked_sub(1) else {
            break;
        };
        array.count = last;
        objc_release(item(this, last));
    }
    // SAFETY: as above.
    let array = unsafe { &mut *fields(this) };
    heap::release(array.items.cast());
    array.items = core::ptr::null_mut();
}

extern "C" fn dealloc(this: Id, _: Sel) {
    release_items(this);
    crate::objc::reference_count::dispose(this);
}

extern "C" fn count(this: Id, _: Sel) -> usize {
    count_of(this)
}

extern "C" fn object_at_index(this: Id, sel: Sel, index: usize) -> Id {
    if index >= count_of(this) {
        // SAFETY: a canonical selector name.
        out_of_range(this, unsafe { crate::objc::class::abi_name(sel) }, index);
    }
    item(this, index)
}

extern "C" fn first_object(this: Id, _: Sel) -> Id {
    if count_of(this) == 0 {
        core::ptr::null_mut()
    } else {
        item(this, 0)
    }
}

extern "C" fn last_object(this: Id, _: Sel) -> Id {
    match count_of(this) {
        0 => core::ptr::null_mut(),
        count => item(this, count - 1),
    }
}

/// The first index whose object is `object` or equal to it.
pub fn index_of(this: Id, object: Id) -> usize {
    let mut index = 0;
    while index < count_of(this) {
        let candidate = item(this, index);
        if candidate == object
            || (!object.is_null() && send_bool(candidate, sel!("isEqual:"), object as usize))
        {
            return index;
        }
        index += 1;
    }
    NOT_FOUND
}

extern "C" fn index_of_object(this: Id, _: Sel, object: Id) -> usize {
    index_of(this, object)
}

extern "C" fn contains_object(this: Id, _: Sel, object: Id) -> u8 {
    u8::from(index_of(this, object) != NOT_FOUND)
}

fn equal_arrays(this: Id, other: Id) -> bool {
    if this == other {
        return true;
    }
    if other.is_null() || !is_array(other) || count_of(this) != count_of(other) {
        return false;
    }
    let mut index = 0;
    while index < count_of(this) && index < count_of(other) {
        let (left, right) = (item(this, index), item(other, index));
        if left != right && !send_bool(left, sel!("isEqual:"), right as usize) {
            return false;
        }
        index += 1;
    }
    count_of(this) == count_of(other)
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(equal_arrays(this, other))
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    count_of(this)
}

extern "C" fn components_joined_by_string(this: Id, _: Sel, separator: Id) -> Id {
    let mut text = Buffer::new();
    let mut index = 0;
    while index < count_of(this) {
        if index > 0 {
            super::with_utf8(separator, |bytes| {
                for &byte in bytes {
                    text.push(byte);
                }
            });
        }
        let piece = send(item(this, index), sel!("description"));
        super::with_utf8(piece, |bytes| {
            for &byte in bytes {
                text.push(byte);
            }
        });
        index += 1;
    }
    let joined = super::string_from_utf8(text.as_slice());
    text.free();
    joined
}

extern "C" fn array_by_adding_object(this: Id, _: Sel, object: Id) -> Id {
    let mut items = snapshot(this);
    items.push(object);
    let made = new_array(class_pointer(&NSARRAY), items.as_slice());
    items.free();
    autoreleased(made)
}

fn sorted(items: &mut Buffer<Id>, comparator: Sel) {
    let length = items.len();
    let mut scratch: Buffer<Id> = Buffer::new();
    scratch.reserve(length);
    for &item in items.as_slice() {
        scratch.push(item);
    }
    let mut width = 1;
    while width < length {
        let mut start = 0;
        while start < length {
            let middle = (start + width).min(length);
            let end = (start + 2 * width).min(length);
            let (mut left, mut right) = (start, middle);
            for slot in start..end {
                let take_left = right >= end
                    || (left < middle && {
                        let order = send1(
                            items.as_slice()[left],
                            comparator,
                            items.as_slice()[right] as usize,
                        ) as isize;
                        order <= 0
                    });
                scratch.as_mut_slice()[slot] = if take_left {
                    left += 1;
                    items.as_slice()[left - 1]
                } else {
                    right += 1;
                    items.as_slice()[right - 1]
                };
            }
            start = end;
        }
        items.as_mut_slice().copy_from_slice(scratch.as_slice());
        width *= 2;
    }
    scratch.free();
}

extern "C" fn sorted_array_using_selector(this: Id, _: Sel, comparator: Sel) -> Id {
    let mut items = snapshot(this);
    for &item in items.as_slice() {
        objc_retain(item);
    }
    sorted(&mut items, comparator);
    let made = new_array(class_pointer(&NSARRAY), items.as_slice());
    for &item in items.as_slice() {
        objc_release(item);
    }
    items.free();
    autoreleased(made)
}

/// `NSFastEnumerationState`.
#[repr(C)]
#[derive(Debug)]
pub struct FastEnumerationState {
    pub state: usize,
    pub items: *mut Id,
    pub mutations: *mut usize,
    pub extra: [usize; 5],
}

unsafe extern "C" fn count_by_enumerating(
    this: Id,
    _: Sel,
    state: *mut FastEnumerationState,
    _: *mut Id,
    _: usize,
) -> usize {
    // SAFETY: the caller's promise.
    let state = unsafe { &mut *state };
    if state.state != 0 {
        return 0;
    }
    state.state = 1;
    // SAFETY: an array; its storage stays while the enumeration is unmutated.
    state.items = unsafe { (*fields(this)).items };
    state.mutations = if is_mutable(this) {
        // SAFETY: a mutable array has the mutable fields.
        unsafe { &raw mut (*this.cast::<MutableArrayObject>()).mutations }
    } else {
        &raw mut state.extra[0]
    };
    count_of(this)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    if is_mutable(this) {
        let mut items = snapshot(this);
        let made = new_array(class_pointer(&NSARRAY), items.as_slice());
        items.free();
        return made;
    }
    objc_retain(this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let mut items = snapshot(this);
    let made = new_array(class_pointer(&NSMUTABLEARRAY), items.as_slice());
    items.free();
    made
}

extern "C" fn description_method(this: Id, _: Sel) -> Id {
    description::describe(this, 0)
}

extern "C" fn description_with_locale(this: Id, _: Sel, _: Id) -> Id {
    description::describe(this, 0)
}

extern "C" fn description_with_locale_indent(this: Id, _: Sel, _: Id, indent: usize) -> Id {
    description::describe(this, indent)
}

fn mutated(this: Id) -> &'static mut MutableArrayObject {
    if !is_mutable(this) {
        terminate_uncaught(
            b"NSInternalInconsistencyException",
            &[
                b"*** a mutating method sent to an immutable ",
                root::class_name(this),
            ],
        );
    }
    // SAFETY: a mutable array, which the program changes from one thread at a time.
    let mutable = unsafe { &mut *this.cast::<MutableArrayObject>() };
    mutable.mutations = mutable.mutations.wrapping_add(1);
    mutable
}

fn grow(mutable: &mut MutableArrayObject, needed: usize) -> bool {
    if needed <= mutable.capacity {
        return true;
    }
    let capacity = needed.max(mutable.capacity * 2).max(4);
    let grown = heap::reallocate(
        mutable.array.items.cast(),
        capacity * core::mem::size_of::<Id>(),
    );
    if grown.is_null() {
        return false;
    }
    mutable.array.items = grown.cast();
    mutable.capacity = capacity;
    true
}

fn refuse_nil(object: Id, method: &[u8]) {
    if object.is_null() {
        terminate_uncaught(
            b"NSInvalidArgumentException",
            &[b"*** -[NSMutableArray ", method, b"]: object cannot be nil"],
        );
    }
}

fn insert_at(this: Id, object: Id, index: usize) {
    refuse_nil(object, b"insertObject:atIndex:");
    if index > count_of(this) {
        out_of_range(this, b"insertObject:atIndex:", index);
    }
    let mutable = mutated(this);
    let needed = mutable.array.count + 1;
    if !grow(mutable, needed) {
        return;
    }
    let array = &mut mutable.array;
    objc_retain(object);
    // SAFETY: `grow` made room for one more; the objects from `index` move up one.
    unsafe {
        array
            .items
            .wrapping_add(index)
            .copy_to(array.items.wrapping_add(index + 1), array.count - index)
    };
    // SAFETY: the slot at `index` is free now.
    unsafe { array.items.wrapping_add(index).write(object) };
    array.count += 1;
}

fn remove_at(this: Id, index: usize) {
    if index >= count_of(this) {
        out_of_range(this, b"removeObjectAtIndex:", index);
    }
    let array = &mut mutated(this).array;
    let removed = item(this, index);
    // SAFETY: the objects after `index` move down one, inside the storage.
    unsafe {
        array
            .items
            .wrapping_add(index + 1)
            .copy_to(array.items.wrapping_add(index), array.count - index - 1)
    };
    array.count -= 1;
    objc_release(removed);
}

extern "C" fn add_object(this: Id, _: Sel, object: Id) {
    insert_at(this, object, count_of(this));
}

extern "C" fn add_objects_from_array(this: Id, _: Sel, other: Id) {
    let mut items = snapshot(other);
    for &object in items.as_slice() {
        insert_at(this, object, count_of(this));
    }
    items.free();
}

extern "C" fn insert_object(this: Id, _: Sel, object: Id, index: usize) {
    insert_at(this, object, index);
}

extern "C" fn remove_object_at_index(this: Id, _: Sel, index: usize) {
    remove_at(this, index);
}

extern "C" fn remove_last_object(this: Id, _: Sel) {
    if let Some(last) = count_of(this).checked_sub(1) {
        remove_at(this, last);
    }
}

extern "C" fn remove_object(this: Id, _: Sel, object: Id) {
    objc_retain(object);
    loop {
        let index = index_of(this, object);
        if index == NOT_FOUND {
            break;
        }
        remove_at(this, index);
    }
    objc_release(object);
}

extern "C" fn remove_all_objects(this: Id, _: Sel) {
    while let Some(last) = count_of(this).checked_sub(1) {
        remove_at(this, last);
    }
}

extern "C" fn replace_object_at_index(this: Id, _: Sel, index: usize, object: Id) {
    refuse_nil(object, b"replaceObjectAtIndex:withObject:");
    if index >= count_of(this) {
        out_of_range(this, b"replaceObjectAtIndex:withObject:", index);
    }
    objc_retain(object);
    let array = &mut mutated(this).array;
    let previous = item(this, index);
    // SAFETY: `index` is below the count.
    unsafe { array.items.wrapping_add(index).write(object) };
    objc_release(previous);
}

extern "C" fn set_object_at_indexed_subscript(this: Id, sel: Sel, object: Id, index: usize) {
    if index == count_of(this) {
        insert_at(this, object, index);
    } else {
        replace_object_at_index(this, sel, index, object);
    }
}

static EMPTY: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Makes the empty array a constant array.
pub fn set_up_empty() -> bool {
    EMPTY[0].store(&raw const CONSTANT_ARRAY as usize, Ordering::Relaxed);
    true
}

pub fn empty_address() -> u64 {
    &raw const EMPTY as u64
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithObjects:count:" => init_with_objects_count,
        "initWithArray:" => init_with_array,
        "dealloc" => dealloc,
        "count" => count,
        "objectAtIndex:" => object_at_index,
        "objectAtIndexedSubscript:" => object_at_index,
        "firstObject" => first_object,
        "lastObject" => last_object,
        "indexOfObject:" => index_of_object,
        "containsObject:" => contains_object,
        "isEqual:" => is_equal,
        "isEqualToArray:" => is_equal,
        "hash" => hash,
        "componentsJoinedByString:" => components_joined_by_string,
        "arrayByAddingObject:" => array_by_adding_object,
        "sortedArrayUsingSelector:" => sorted_array_using_selector,
        "countByEnumeratingWithState:objects:count:" => count_by_enumerating,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "description" => description_method,
        "descriptionWithLocale:" => description_with_locale,
        "descriptionWithLocale:indent:" => description_with_locale_indent,
    ];
    let class = methods![
        "array" => array,
        "arrayWithObject:" => array_with_object,
        "arrayWithObjects:count:" => array_with_objects_count,
        "arrayWithObjects:" => eggos_array_with_objects,
        "arrayWithArray:" => array_with_array,
    ];
    let mutable_instance = methods![
        "initWithCapacity:" => init_with_capacity,
        "addObject:" => add_object,
        "addObjectsFromArray:" => add_objects_from_array,
        "insertObject:atIndex:" => insert_object,
        "removeObjectAtIndex:" => remove_object_at_index,
        "removeLastObject" => remove_last_object,
        "removeObject:" => remove_object,
        "removeAllObjects" => remove_all_objects,
        "replaceObjectAtIndex:withObject:" => replace_object_at_index,
        "setObject:atIndexedSubscript:" => set_object_at_indexed_subscript,
    ];
    let mutable_class = methods![
        "arrayWithCapacity:" => array_with_capacity,
    ];
    let array_size = core::mem::size_of::<ArrayObject>();
    define_class(
        runtime,
        &Definition {
            name: b"NSArray\0",
            class: &NSARRAY,
            meta: &NSARRAY_META,
            superclass: &NSOBJECT,
            instance_size: array_size,
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableArray\0",
            class: &NSMUTABLEARRAY,
            meta: &NSMUTABLEARRAY_META,
            superclass: &NSARRAY,
            instance_size: core::mem::size_of::<MutableArrayObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &mutable_class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSConstantArray\0",
            class: &CONSTANT_ARRAY,
            meta: &CONSTANT_ARRAY_META,
            superclass: &NSARRAY,
            instance_size: array_size,
            flags: IMMORTAL,
            instance_methods: &[],
            class_methods: &[],
        },
    )
}

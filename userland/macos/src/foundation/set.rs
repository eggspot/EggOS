//! NSSet and NSMutableSet (D175): distinct objects, each retained, found through a hash table
//! of the objects' `hash`, compared with `isEqual:`, kept in the order they were added (which
//! is the order enumeration and `allObjects` give). The table is open-addressed with linear
//! probing, at most half full, and rebuilt when an object is removed.

use super::array::{FastEnumerationState, count_of, item};
use super::{
    Definition, NSARRAY, NSMUTABLESET, NSMUTABLESET_META, NSSET, NSSET_META, autoreleased,
    class_pointer, define as define_class, is_array, is_set, new_instance, string_from_utf8,
    with_utf8,
};
use crate::format::Arguments;
use crate::libsystem::variadic::{RawVaList, VaList, variadic};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::block::invoke_address;
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods, sel, send, send_bool, send2};

const MIN_SLOTS: usize = 8;

#[repr(C)]
#[derive(Debug)]
struct SetObject {
    isa: usize,
    items: Buffer<Id>,
    slots: Buffer<u32>,
    mutations: usize,
}

fn fields(object: Id) -> &'static mut SetObject {
    // SAFETY: the callers pass NSSet instances, whose memory starts with these fields, reached
    // from one thread at a time as Foundation's collections are.
    unsafe { &mut *object.cast::<SetObject>() }
}

fn hash_of(object: Id) -> usize {
    send(object, sel!("hash")) as usize
}

fn equal(left: Id, right: Id) -> bool {
    left == right || send_bool(left, sel!("isEqual:"), right as usize)
}

fn position(this: Id, object: Id) -> Option<usize> {
    if object.is_null() {
        return None;
    }
    let capacity = fields(this).slots.len();
    if capacity == 0 {
        return None;
    }
    let mut at = hash_of(object) & (capacity - 1);
    for _ in 0..capacity {
        let slot = *fields(this).slots.as_slice().get(at)? as usize;
        if slot == 0 {
            return None;
        }
        let candidate = *fields(this).items.as_slice().get(slot - 1)?;
        if equal(candidate, object) {
            return Some(slot - 1);
        }
        at = (at + 1) & (capacity - 1);
    }
    None
}

fn place(slots: &mut [u32], hash: usize, index: usize) {
    let mask = slots.len() - 1;
    let mut at = hash & mask;
    while slots[at] != 0 {
        at = (at + 1) & mask;
    }
    slots[at] = index as u32 + 1;
}

fn rebuild(this: Id) -> bool {
    let count = fields(this).items.len();
    let capacity = (count * 2).max(MIN_SLOTS).next_power_of_two();
    let mut slots = Buffer::new();
    if !slots.reserve(capacity) {
        return false;
    }
    for _ in 0..capacity {
        slots.push(0u32);
    }
    let mut index = 0;
    while let Some(&object) = fields(this).items.as_slice().get(index) {
        place(slots.as_mut_slice(), hash_of(object), index);
        index += 1;
    }
    let mut old = core::mem::replace(&mut fields(this).slots, slots);
    old.free();
    true
}

fn add(this: Id, object: Id) -> bool {
    if object.is_null() || position(this, object).is_some() {
        return !object.is_null();
    }
    objc_retain(object);
    if !fields(this).items.push(object) {
        objc_release(object);
        return false;
    }
    fields(this).mutations += 1;
    let count = fields(this).items.len();
    let capacity = fields(this).slots.len();
    if count * 2 > capacity {
        return rebuild(this);
    }
    let hash = hash_of(object);
    place(fields(this).slots.as_mut_slice(), hash, count - 1);
    true
}

fn remove(this: Id, object: Id) {
    let Some(index) = position(this, object) else {
        return;
    };
    let items = fields(this).items.as_mut_slice();
    let removed = items[index];
    items.copy_within(index + 1.., index);
    fields(this).items.pop();
    fields(this).mutations += 1;
    rebuild(this);
    objc_release(removed);
}

fn clear(this: Id) {
    let mut items = core::mem::replace(&mut fields(this).items, Buffer::new());
    let mut slots = core::mem::replace(&mut fields(this).slots, Buffer::new());
    fields(this).mutations += 1;
    for &object in items.as_slice() {
        objc_release(object);
    }
    items.free();
    slots.free();
}

fn snapshot(this: Id) -> Buffer<Id> {
    let mut copy = Buffer::new();
    copy.reserve(fields(this).items.len());
    for &object in fields(this).items.as_slice() {
        copy.push(object);
    }
    copy
}

fn new_set(class: *mut ClassObject, objects: &[Id]) -> Id {
    let set = new_instance(class);
    if set.is_null() {
        return set;
    }
    for &object in objects {
        add(set, object);
    }
    set
}

fn objects_of(other: Id) -> Buffer<Id> {
    if other.is_null() {
        return Buffer::new();
    }
    if is_set(other) {
        return snapshot(other);
    }
    let mut copy = Buffer::new();
    if is_array(other) {
        for index in 0..count_of(other) {
            copy.push(item(other, index));
        }
    }
    copy
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

fn made_from(class: *mut ClassObject, mut objects: Buffer<Id>) -> Id {
    let set = new_set(class, objects.as_slice());
    objects.free();
    set
}

fn objects_slice<'a>(objects: *const Id, count: usize) -> &'a [Id] {
    if objects.is_null() || count == 0 {
        return &[];
    }
    // SAFETY: the program passes `count` objects.
    unsafe { core::slice::from_raw_parts(objects, count) }
}

extern "C" fn set(this: Id, _: Sel) -> Id {
    autoreleased(new_set(this_class(this), &[]))
}

extern "C" fn set_with_object(this: Id, _: Sel, object: Id) -> Id {
    autoreleased(new_set(this_class(this), &[object]))
}

extern "C" fn set_with_objects_count(this: Id, _: Sel, objects: *const Id, count: usize) -> Id {
    autoreleased(new_set(this_class(this), objects_slice(objects, count)))
}

extern "C" fn set_with_collection(this: Id, _: Sel, other: Id) -> Id {
    autoreleased(made_from(this_class(this), objects_of(other)))
}

extern "C" fn set_with_objects_list(this: Id, _: Sel, first: Id, arguments: RawVaList) -> Id {
    let mut objects = Buffer::new();
    if !first.is_null() {
        objects.push(first);
        // SAFETY: the program's nil-terminated list of objects.
        let mut list = unsafe { VaList::new(arguments) };
        loop {
            let next = list.integer() as usize as Id;
            if next.is_null() {
                break;
            }
            objects.push(next);
        }
    }
    autoreleased(made_from(this_class(this), objects))
}

variadic!(eggos_set_with_objects => set_with_objects_list, 3, "x3", "rcx");

extern "C" fn init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn init_with_objects_count(this: Id, _: Sel, objects: *const Id, count: usize) -> Id {
    if !this.is_null() {
        for &object in objects_slice(objects, count) {
            add(this, object);
        }
    }
    this
}

extern "C" fn init_with_collection(this: Id, _: Sel, other: Id) -> Id {
    if !this.is_null() {
        let mut objects = objects_of(other);
        for &object in objects.as_slice() {
            add(this, object);
        }
        objects.free();
    }
    this
}

extern "C" fn init_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    clear(this);
    dispose(this);
}

extern "C" fn count(this: Id, _: Sel) -> usize {
    fields(this).items.len()
}

extern "C" fn member(this: Id, _: Sel, object: Id) -> Id {
    match position(this, object) {
        Some(index) => fields(this).items.as_slice()[index],
        None => core::ptr::null_mut(),
    }
}

extern "C" fn contains_object(this: Id, _: Sel, object: Id) -> u8 {
    u8::from(position(this, object).is_some())
}

extern "C" fn any_object(this: Id, _: Sel) -> Id {
    fields(this)
        .items
        .as_slice()
        .first()
        .copied()
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn all_objects(this: Id, _: Sel) -> Id {
    let mut objects = snapshot(this);
    let array = send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        objects.as_slice().as_ptr() as usize,
        objects.len(),
    );
    objects.free();
    array
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
    let fields = fields(this);
    state.items = fields.items.as_slice().as_ptr().cast_mut();
    state.mutations = &raw mut fields.mutations;
    fields.items.len()
}

fn every_in(this: Id, other: Id) -> bool {
    let mut objects = snapshot(this);
    let all = objects
        .as_slice()
        .iter()
        .all(|&object| position(other, object).is_some());
    objects.free();
    all
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    let same = other == this
        || (!other.is_null()
            && is_set(other)
            && fields(other).items.len() == fields(this).items.len()
            && every_in(this, other));
    u8::from(same)
}

extern "C" fn is_subset(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(!other.is_null() && is_set(other) && every_in(this, other))
}

extern "C" fn intersects(this: Id, _: Sel, other: Id) -> u8 {
    if other.is_null() || !is_set(other) {
        return 0;
    }
    let mut objects = snapshot(this);
    let any = objects
        .as_slice()
        .iter()
        .any(|&object| position(other, object).is_some());
    objects.free();
    u8::from(any)
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    fields(this).items.len()
}

extern "C" fn set_by_adding(this: Id, _: Sel, object: Id) -> Id {
    let mut objects = snapshot(this);
    objects.push(object);
    autoreleased(made_from(class_pointer(&NSSET), objects))
}

extern "C" fn enumerate_with_block(this: Id, _: Sel, block: Id) {
    if block.is_null() {
        return;
    }
    // SAFETY: the program's block takes the block, an object and a `BOOL *` to stop.
    let function: extern "C" fn(Id, Id, *mut u8) =
        unsafe { core::mem::transmute(invoke_address(block)) };
    let mut objects = snapshot(this);
    let mut stop = 0u8;
    for &object in objects.as_slice() {
        function(block, object, &mut stop);
        if stop != 0 {
            break;
        }
    }
    objects.free();
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    if root::is_kind_of(this, &NSMUTABLESET) {
        return made_from(class_pointer(&NSSET), snapshot(this));
    }
    objc_retain(this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    made_from(class_pointer(&NSMUTABLESET), snapshot(this))
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let mut out = Buffer::new();
    let mut push = |text: &[u8]| {
        if out.reserve(text.len()) {
            for &byte in text {
                out.push(byte);
            }
        }
    };
    push(b"{(");
    let mut objects = snapshot(this);
    for (index, &object) in objects.as_slice().iter().enumerate() {
        let separator: &[u8] = if index == 0 { b"\n    " } else { b",\n    " };
        push(separator);
        let text = send(object, sel!("description"));
        with_utf8(text, &mut push);
    }
    objects.free();
    push(b"\n)}");
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn add_object(this: Id, _: Sel, object: Id) {
    add(this, object);
}

extern "C" fn remove_object(this: Id, _: Sel, object: Id) {
    remove(this, object);
}

extern "C" fn remove_all_objects(this: Id, _: Sel) {
    clear(this);
}

extern "C" fn union_with(this: Id, _: Sel, other: Id) {
    let mut objects = objects_of(other);
    for &object in objects.as_slice() {
        add(this, object);
    }
    objects.free();
}

extern "C" fn minus_set(this: Id, _: Sel, other: Id) {
    let mut objects = objects_of(other);
    for &object in objects.as_slice() {
        remove(this, object);
    }
    objects.free();
}

extern "C" fn intersect_set(this: Id, _: Sel, other: Id) {
    let mut objects = snapshot(this);
    for &object in objects.as_slice() {
        if other.is_null() || position(other, object).is_none() {
            remove(this, object);
        }
    }
    objects.free();
}

extern "C" fn set_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    autoreleased(new_set(this_class(this), &[]))
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithObjects:count:" => init_with_objects_count,
        "initWithArray:" => init_with_collection,
        "initWithSet:" => init_with_collection,
        "dealloc" => dealloc,
        "count" => count,
        "member:" => member,
        "containsObject:" => contains_object,
        "anyObject" => any_object,
        "allObjects" => all_objects,
        "countByEnumeratingWithState:objects:count:" => count_by_enumerating,
        "isEqual:" => is_equal,
        "isEqualToSet:" => is_equal,
        "isSubsetOfSet:" => is_subset,
        "intersectsSet:" => intersects,
        "hash" => hash,
        "setByAddingObject:" => set_by_adding,
        "enumerateObjectsUsingBlock:" => enumerate_with_block,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "description" => description,
    ];
    let class = methods![
        "set" => set,
        "setWithObject:" => set_with_object,
        "setWithObjects:count:" => set_with_objects_count,
        "setWithObjects:" => eggos_set_with_objects,
        "setWithArray:" => set_with_collection,
        "setWithSet:" => set_with_collection,
    ];
    let mutable_instance = methods![
        "initWithCapacity:" => init_with_capacity,
        "addObject:" => add_object,
        "removeObject:" => remove_object,
        "removeAllObjects" => remove_all_objects,
        "addObjectsFromArray:" => union_with,
        "unionSet:" => union_with,
        "minusSet:" => minus_set,
        "intersectSet:" => intersect_set,
    ];
    let mutable_class = methods![
        "setWithCapacity:" => set_with_capacity,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSSet\0",
            class: &NSSET,
            meta: &NSSET_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<SetObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableSet\0",
            class: &NSMUTABLESET,
            meta: &NSMUTABLESET_META,
            superclass: &NSSET,
            instance_size: core::mem::size_of::<SetObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &mutable_class,
        },
    )
}

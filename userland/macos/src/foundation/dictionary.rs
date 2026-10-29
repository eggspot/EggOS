//! NSDictionary, NSMutableDictionary, the constant dictionary clang's `@{...}` literals use and
//! the empty dictionary (`___NSDictionary0__struct`). They share clang's
//! `NSConstantDictionary` layout, `{isa, options, count, keys, objects}`; mutable dictionaries
//! add a mutation count, and the dictionaries the layer builds have room for a capacity too. Keys are copied when stored and kept in insertion
//! order; a lookup compares by identity, then `isEqual:` (a hash index is not there yet).

use super::array::{FastEnumerationState, new_array, objects_slice};
use super::{
    CONSTANT_DICTIONARY, CONSTANT_DICTIONARY_META, Definition, NSARRAY, NSDICTIONARY,
    NSDICTIONARY_META, NSMUTABLEDICTIONARY, NSMUTABLEDICTIONARY_META, autoreleased, class_pointer,
    define as define_class, description, is_dictionary, new_instance,
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
use crate::objc::{Runtime, methods, send, send_bool, terminate_uncaught};
use core::sync::atomic::{AtomicUsize, Ordering};

/// A dictionary's fields.
#[repr(C)]
#[derive(Debug)]
pub struct DictionaryObject {
    isa: usize,
    options: usize,
    count: usize,
    keys: *mut Id,
    objects: *mut Id,
}

/// A mutable dictionary's fields.
#[repr(C)]
#[derive(Debug)]
pub struct MutableDictionaryObject {
    dictionary: DictionaryObject,
    capacity: usize,
    mutations: usize,
}

fn fields(object: Id) -> *mut DictionaryObject {
    object.cast()
}

fn is_mutable(object: Id) -> bool {
    root::is_kind_of(object, &NSMUTABLEDICTIONARY)
}

pub fn count_of(object: Id) -> usize {
    // SAFETY: the callers pass dictionaries, which share the layout.
    unsafe { (*fields(object)).count }
}

/// The key at `index`, below the count.
pub fn key(object: Id, index: usize) -> Id {
    // SAFETY: as above.
    let keys = unsafe { (*fields(object)).keys };
    // SAFETY: `index` is below the count.
    unsafe { keys.wrapping_add(index).read() }
}

/// The object at `index`, below the count.
pub fn value(object: Id, index: usize) -> Id {
    // SAFETY: as above.
    let objects = unsafe { (*fields(object)).objects };
    // SAFETY: `index` is below the count.
    unsafe { objects.wrapping_add(index).read() }
}

/// Where `wanted` is among the keys.
pub fn position(object: Id, wanted: Id) -> Option<usize> {
    if wanted.is_null() {
        return None;
    }
    let mut index = 0;
    while index < count_of(object) {
        let candidate = key(object, index);
        if candidate == wanted || send_bool(candidate, sel!("isEqual:"), wanted as usize) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn mutable_fields(this: Id) -> &'static mut MutableDictionaryObject {
    // SAFETY: a mutable dictionary, which the program changes from one thread at a time.
    unsafe { &mut *this.cast::<MutableDictionaryObject>() }
}

fn grow(this: Id, needed: usize) -> bool {
    let mutable = mutable_fields(this);
    if needed <= mutable.capacity {
        return true;
    }
    let capacity = needed.max(mutable.capacity * 2).max(4);
    let bytes = capacity * core::mem::size_of::<Id>();
    let keys = heap::reallocate(mutable.dictionary.keys.cast(), bytes);
    if keys.is_null() {
        return false;
    }
    mutable.dictionary.keys = keys.cast();
    let objects = heap::reallocate(mutable.dictionary.objects.cast(), bytes);
    if objects.is_null() {
        return false;
    }
    mutable.dictionary.objects = objects.cast();
    mutable.capacity = capacity;
    true
}

fn set(this: Id, object: Id, key_object: Id) {
    if object.is_null() || key_object.is_null() {
        terminate_uncaught(
            b"NSInvalidArgumentException",
            &[b"*** -[NSMutableDictionary setObject:forKey:]: object or key cannot be nil"],
        );
    }
    let copied = send(key_object, sel!("copy"));
    objc_retain(object);
    if let Some(index) = position(this, copied) {
        let previous = value(this, index);
        // SAFETY: a dictionary the caller is building or changing.
        let objects = unsafe { (*fields(this)).objects };
        // SAFETY: `index` is below the count.
        unsafe { objects.wrapping_add(index).write(object) };
        objc_release(previous);
        objc_release(copied);
        return;
    }
    let count = count_of(this);
    if !grow(this, count + 1) {
        objc_release(copied);
        objc_release(object);
        return;
    }
    let dictionary = &mut mutable_fields(this).dictionary;
    // SAFETY: `grow` made room for one more key and object.
    unsafe { dictionary.keys.wrapping_add(count).write(copied) };
    // SAFETY: as above.
    unsafe { dictionary.objects.wrapping_add(count).write(object) };
    dictionary.count = count + 1;
}

fn new_dictionary(class: *mut ClassObject, objects: &[Id], keys: &[Id]) -> Id {
    let object = new_instance(class);
    if object.is_null() {
        return object;
    }
    fill(object, objects, keys);
    object
}

fn fill(this: Id, objects: &[Id], keys: &[Id]) {
    for (&object, &key_object) in objects.iter().zip(keys) {
        set(this, object, key_object);
    }
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

extern "C" fn dictionary(this: Id, _: Sel) -> Id {
    autoreleased(new_dictionary(this_class(this), &[], &[]))
}

extern "C" fn dictionary_with_object(this: Id, _: Sel, object: Id, key_object: Id) -> Id {
    autoreleased(new_dictionary(this_class(this), &[object], &[key_object]))
}

unsafe extern "C" fn dictionary_with_objects_keys(
    this: Id,
    _: Sel,
    objects: *const Id,
    keys: *const Id,
    count: usize,
) -> Id {
    // SAFETY: the caller's promise.
    let objects = unsafe { objects_slice(objects, count) };
    // SAFETY: as above.
    let keys = unsafe { objects_slice(keys, count) };
    autoreleased(new_dictionary(this_class(this), objects, keys))
}

extern "C" fn dictionary_with_objects_and_keys_list(
    this: Id,
    _: Sel,
    first: Id,
    arguments: RawVaList,
) -> Id {
    let mut objects = Buffer::new();
    let mut keys = Buffer::new();
    if !first.is_null() {
        // SAFETY: the program's nil-terminated list of objects and keys.
        let mut list = unsafe { VaList::new(arguments) };
        let mut object = first;
        loop {
            let key_object = list.integer() as usize as Id;
            if key_object.is_null() {
                break;
            }
            objects.push(object);
            keys.push(key_object);
            object = list.integer() as usize as Id;
            if object.is_null() {
                break;
            }
        }
    }
    let made = new_dictionary(this_class(this), objects.as_slice(), keys.as_slice());
    objects.free();
    keys.free();
    autoreleased(made)
}

variadic!(
    eggos_dictionary_with_objects_and_keys => dictionary_with_objects_and_keys_list,
    3,
    "x3",
    "rcx"
);

fn entries_of(other: Id) -> (Buffer<Id>, Buffer<Id>) {
    let mut objects = Buffer::new();
    let mut keys = Buffer::new();
    if !other.is_null() {
        for index in 0..count_of(other) {
            objects.push(value(other, index));
            keys.push(key(other, index));
        }
    }
    (objects, keys)
}

extern "C" fn dictionary_with_dictionary(this: Id, _: Sel, other: Id) -> Id {
    let (mut objects, mut keys) = entries_of(other);
    let made = new_dictionary(this_class(this), objects.as_slice(), keys.as_slice());
    objects.free();
    keys.free();
    autoreleased(made)
}

extern "C" fn dictionary_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    autoreleased(new_dictionary(this_class(this), &[], &[]))
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn init_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    this
}

unsafe extern "C" fn init_with_objects_keys(
    this: Id,
    _: Sel,
    objects: *const Id,
    keys: *const Id,
    count: usize,
) -> Id {
    // SAFETY: the caller's promise.
    let objects = unsafe { objects_slice(objects, count) };
    // SAFETY: as above.
    let keys = unsafe { objects_slice(keys, count) };
    fill(this, objects, keys);
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    loop {
        // SAFETY: a dictionary being deallocated, which only this thread uses.
        let dictionary = unsafe { &mut *fields(this) };
        let Some(last) = dictionary.count.checked_sub(1) else {
            break;
        };
        dictionary.count = last;
        let (key_object, object) = (key(this, last), value(this, last));
        objc_release(key_object);
        objc_release(object);
    }
    // SAFETY: as above.
    let dictionary = unsafe { &mut *fields(this) };
    heap::release(dictionary.keys.cast());
    heap::release(dictionary.objects.cast());
    crate::objc::reference_count::dispose(this);
}

extern "C" fn count(this: Id, _: Sel) -> usize {
    count_of(this)
}

extern "C" fn object_for_key(this: Id, _: Sel, key_object: Id) -> Id {
    position(this, key_object).map_or(core::ptr::null_mut(), |index| value(this, index))
}

fn column(this: Id, keys: bool) -> Id {
    let mut items = Buffer::new();
    for index in 0..count_of(this) {
        items.push(if keys {
            key(this, index)
        } else {
            value(this, index)
        });
    }
    let made = new_array(class_pointer(&NSARRAY), items.as_slice());
    items.free();
    autoreleased(made)
}

extern "C" fn all_keys(this: Id, _: Sel) -> Id {
    column(this, true)
}

extern "C" fn all_values(this: Id, _: Sel) -> Id {
    column(this, false)
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
    // SAFETY: a dictionary; its keys stay while the enumeration is unmutated.
    state.items = unsafe { (*fields(this)).keys };
    state.mutations = if is_mutable(this) {
        &raw mut mutable_fields(this).mutations
    } else {
        &raw mut state.extra[0]
    };
    count_of(this)
}

fn equal_dictionaries(this: Id, other: Id) -> bool {
    if this == other {
        return true;
    }
    if other.is_null() || !is_dictionary(other) || count_of(this) != count_of(other) {
        return false;
    }
    let mut index = 0;
    while index < count_of(this) {
        let mine = value(this, index);
        let theirs = object_for_key(other, sel!("objectForKey:"), key(this, index));
        if theirs.is_null()
            || (mine != theirs && !send_bool(mine, sel!("isEqual:"), theirs as usize))
        {
            return false;
        }
        index += 1;
    }
    true
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(equal_dictionaries(this, other))
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    count_of(this)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    if is_mutable(this) {
        let (mut objects, mut keys) = entries_of(this);
        let made = new_dictionary(
            class_pointer(&NSDICTIONARY),
            objects.as_slice(),
            keys.as_slice(),
        );
        objects.free();
        keys.free();
        return made;
    }
    objc_retain(this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let (mut objects, mut keys) = entries_of(this);
    let made = new_dictionary(
        class_pointer(&NSMUTABLEDICTIONARY),
        objects.as_slice(),
        keys.as_slice(),
    );
    objects.free();
    keys.free();
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

fn note_mutation(this: Id) {
    let mutable = mutable_fields(this);
    mutable.mutations = mutable.mutations.wrapping_add(1);
}

extern "C" fn set_object_for_key(this: Id, _: Sel, object: Id, key_object: Id) {
    note_mutation(this);
    set(this, object, key_object);
}

extern "C" fn set_object_for_keyed_subscript(this: Id, sel: Sel, object: Id, key_object: Id) {
    if object.is_null() {
        remove_object_for_key(this, sel, key_object);
    } else {
        set_object_for_key(this, sel, object, key_object);
    }
}

fn remove_at(this: Id, index: usize) {
    note_mutation(this);
    let (key_object, object) = (key(this, index), value(this, index));
    let dictionary = &mut mutable_fields(this).dictionary;
    let after = dictionary.count - index - 1;
    // SAFETY: the keys and objects after `index` move down one, inside the storage.
    unsafe {
        dictionary
            .keys
            .wrapping_add(index + 1)
            .copy_to(dictionary.keys.wrapping_add(index), after)
    };
    // SAFETY: as above.
    unsafe {
        dictionary
            .objects
            .wrapping_add(index + 1)
            .copy_to(dictionary.objects.wrapping_add(index), after)
    };
    dictionary.count -= 1;
    objc_release(key_object);
    objc_release(object);
}

extern "C" fn remove_object_for_key(this: Id, _: Sel, key_object: Id) {
    if let Some(index) = position(this, key_object) {
        remove_at(this, index);
    }
}

extern "C" fn remove_all_objects(this: Id, _: Sel) {
    while let Some(last) = count_of(this).checked_sub(1) {
        remove_at(this, last);
    }
}

extern "C" fn add_entries_from_dictionary(this: Id, _: Sel, other: Id) {
    let (mut objects, mut keys) = entries_of(other);
    note_mutation(this);
    for (&object, &key_object) in objects.as_slice().iter().zip(keys.as_slice()) {
        set(this, object, key_object);
    }
    objects.free();
    keys.free();
}

static EMPTY: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

pub fn set_up_empty() -> bool {
    EMPTY[0].store(&raw const CONSTANT_DICTIONARY as usize, Ordering::Relaxed);
    true
}

pub fn empty_address() -> u64 {
    &raw const EMPTY as u64
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithObjects:forKeys:count:" => init_with_objects_keys,
        "dealloc" => dealloc,
        "count" => count,
        "objectForKey:" => object_for_key,
        "objectForKeyedSubscript:" => object_for_key,
        "allKeys" => all_keys,
        "allValues" => all_values,
        "countByEnumeratingWithState:objects:count:" => count_by_enumerating,
        "isEqual:" => is_equal,
        "isEqualToDictionary:" => is_equal,
        "hash" => hash,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "description" => description_method,
        "descriptionWithLocale:" => description_with_locale,
        "descriptionWithLocale:indent:" => description_with_locale_indent,
    ];
    let class = methods![
        "dictionary" => dictionary,
        "dictionaryWithObject:forKey:" => dictionary_with_object,
        "dictionaryWithObjects:forKeys:count:" => dictionary_with_objects_keys,
        "dictionaryWithObjectsAndKeys:" => eggos_dictionary_with_objects_and_keys,
        "dictionaryWithDictionary:" => dictionary_with_dictionary,
    ];
    let mutable_instance = methods![
        "initWithCapacity:" => init_with_capacity,
        "setObject:forKey:" => set_object_for_key,
        "setObject:forKeyedSubscript:" => set_object_for_keyed_subscript,
        "removeObjectForKey:" => remove_object_for_key,
        "removeAllObjects" => remove_all_objects,
        "addEntriesFromDictionary:" => add_entries_from_dictionary,
    ];
    let mutable_class = methods![
        "dictionaryWithCapacity:" => dictionary_with_capacity,
    ];
    let size = core::mem::size_of::<DictionaryObject>();
    define_class(
        runtime,
        &Definition {
            name: b"NSDictionary\0",
            class: &NSDICTIONARY,
            meta: &NSDICTIONARY_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<MutableDictionaryObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableDictionary\0",
            class: &NSMUTABLEDICTIONARY,
            meta: &NSMUTABLEDICTIONARY_META,
            superclass: &NSDICTIONARY,
            instance_size: core::mem::size_of::<MutableDictionaryObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &mutable_class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSConstantDictionary\0",
            class: &CONSTANT_DICTIONARY,
            meta: &CONSTANT_DICTIONARY_META,
            superclass: &NSDICTIONARY,
            instance_size: size,
            flags: IMMORTAL,
            instance_methods: &[],
            class_methods: &[],
        },
    )
}

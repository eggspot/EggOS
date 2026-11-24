//! NSSortDescriptor (D735): a key, an order and the selector or comparator that compares
//! values, kept for whoever sorts (a table view's data source, `sortedArrayUsingDescriptors:`);
//! `reversedSortDescriptor` gives the other order with the same key and selector.
//! `compareObject:toObject:` reads both objects' values at the key path through key-value
//! coding and compares them with the comparator, else the selector (`compare:`), nil first
//! (D741).

use super::{
    Definition, NSSORTDESCRIPTOR, NSSORTDESCRIPTOR_META, autoreleased, class_pointer,
    define as define_class, new_instance, string_from_utf8, with_utf8,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods, sel, send, send1, send_bool};

#[repr(C)]
#[derive(Debug)]
struct DescriptorFields {
    isa: usize,
    key: Id,
    ascending: bool,
    selector: usize,
    comparator: Id,
}

fn fields(object: Id) -> &'static mut DescriptorFields {
    // SAFETY: an NSSortDescriptor starts with these fields, used by one thread at a time.
    unsafe { &mut *object.cast::<DescriptorFields>() }
}

fn copied(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => send(object, sel!("copy")),
    }
}

fn set_up(this: Id, key: Id, ascending: u8, selector: usize, comparator: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = fields(this);
    fields.key = copied(key);
    fields.ascending = ascending != 0;
    fields.selector = selector;
    fields.comparator = copied(comparator);
    this
}

/// The key of descriptor `object`, nil for none.
pub fn key_of(object: Id) -> Id {
    match object.is_null() || !root::is_kind_of(object, &NSSORTDESCRIPTOR) {
        true => core::ptr::null_mut(),
        false => fields(object).key,
    }
}

/// Whether descriptor `object` sorts ascending.
pub fn is_ascending(object: Id) -> bool {
    !object.is_null() && root::is_kind_of(object, &NSSORTDESCRIPTOR) && fields(object).ascending
}

extern "C" fn init_with_key(this: Id, _: Sel, key: Id, ascending: u8) -> Id {
    set_up(this, key, ascending, sel!("compare:") as usize, core::ptr::null_mut())
}

extern "C" fn init_with_key_selector(
    this: Id,
    _: Sel,
    key: Id,
    ascending: u8,
    selector: usize,
) -> Id {
    set_up(this, key, ascending, selector, core::ptr::null_mut())
}

extern "C" fn init_with_key_comparator(
    this: Id,
    _: Sel,
    key: Id,
    ascending: u8,
    comparator: Id,
) -> Id {
    set_up(this, key, ascending, 0, comparator)
}

fn made(key: Id, ascending: u8, selector: usize, comparator: Id) -> Id {
    autoreleased(set_up(
        new_instance(class_pointer(&NSSORTDESCRIPTOR)),
        key,
        ascending,
        selector,
        comparator,
    ))
}

extern "C" fn with_key(_: Id, _: Sel, key: Id, ascending: u8) -> Id {
    made(key, ascending, sel!("compare:") as usize, core::ptr::null_mut())
}

extern "C" fn with_key_selector(_: Id, _: Sel, key: Id, ascending: u8, selector: usize) -> Id {
    made(key, ascending, selector, core::ptr::null_mut())
}

extern "C" fn with_key_comparator(_: Id, _: Sel, key: Id, ascending: u8, comparator: Id) -> Id {
    made(key, ascending, 0, comparator)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    for owned in [fields.key, fields.comparator] {
        if !owned.is_null() {
            objc_release(owned);
        }
    }
    dispose(this);
}

extern "C" fn key(this: Id, _: Sel) -> Id {
    fields(this).key
}

extern "C" fn ascending(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).ascending)
}

extern "C" fn selector(this: Id, _: Sel) -> usize {
    fields(this).selector
}

extern "C" fn comparator(this: Id, _: Sel) -> Id {
    fields(this).comparator
}

extern "C" fn reversed(this: Id, _: Sel) -> Id {
    let own = fields(this);
    made(own.key, u8::from(!own.ascending), own.selector, own.comparator)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

fn same_key(first: Id, second: Id) -> bool {
    first == second || (!first.is_null() && send_bool(first, sel!("isEqual:"), second as usize))
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    let same = other == this
        || (!other.is_null()
            && root::is_kind_of(other, &NSSORTDESCRIPTOR)
            && fields(other).ascending == fields(this).ascending
            && fields(other).selector == fields(this).selector
            && same_key(fields(this).key, fields(other).key));
    u8::from(same)
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    let key = fields(this).key;
    match key.is_null() {
        true => usize::from(fields(this).ascending),
        false => send(key, sel!("hash")) as usize ^ usize::from(fields(this).ascending),
    }
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let mut out: Buffer<u8> = Buffer::new();
    let mut put = |bytes: &[u8]| {
        for &byte in bytes {
            out.push(byte);
        }
    };
    put(b"(");
    let key = fields(this).key;
    if !key.is_null() {
        with_utf8(key, &mut put);
    }
    put(match fields(this).ascending {
        true => b", ascending, compare:)",
        false => b", descending, compare:)",
    });
    let text = string_from_utf8(out.as_slice());
    out.free();
    text
}

type CompareBlock = extern "C" fn(Id, Id, Id) -> isize;

fn value_at(object: Id, key: Id) -> Id {
    match object.is_null() || key.is_null() {
        true => object,
        false => send1(object, sel!("valueForKeyPath:"), key as usize),
    }
}

fn compare_with_block(block: Id, first: Id, second: Id) -> isize {
    // SAFETY: a block object: its invoke function is its third word.
    let invoke = unsafe { block.cast::<usize>().add(2).read() };
    let invoke = crate::callback::native_callable(invoke);
    // SAFETY: an NSComparator's invoke function: the block, then both objects.
    let invoke: CompareBlock = unsafe { core::mem::transmute(invoke) };
    invoke(block, first, second)
}

extern "C" fn compare_objects(this: Id, _: Sel, first: Id, second: Id) -> isize {
    let fields = fields(this);
    let (first, second) = (value_at(first, fields.key), value_at(second, fields.key));
    let order = match (first.is_null(), second.is_null()) {
        _ if !fields.comparator.is_null() => compare_with_block(fields.comparator, first, second),
        (true, true) => 0,
        (true, false) => -1,
        (false, true) => 1,
        (false, false) => {
            let selector = match fields.selector {
                0 => sel!("compare:"),
                chosen => chosen as Sel,
            };
            send1(first, selector, second as usize) as isize
        }
    };
    match fields.ascending {
        true => order,
        false => -order,
    }
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithKey:ascending:" => init_with_key,
        "initWithKey:ascending:selector:" => init_with_key_selector,
        "initWithKey:ascending:comparator:" => init_with_key_comparator,
        "dealloc" => dealloc,
        "key" => key,
        "ascending" => ascending,
        "selector" => selector,
        "comparator" => comparator,
        "reversedSortDescriptor" => reversed,
        "copyWithZone:" => copy_with_zone,
        "isEqual:" => is_equal,
        "hash" => hash,
        "description" => description,
        "compareObject:toObject:" => compare_objects,
    ];
    let class = methods![
        "sortDescriptorWithKey:ascending:" => with_key,
        "sortDescriptorWithKey:ascending:selector:" => with_key_selector,
        "sortDescriptorWithKey:ascending:comparator:" => with_key_comparator,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSSortDescriptor\0",
            class: &NSSORTDESCRIPTOR,
            meta: &NSSORTDESCRIPTOR_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<DescriptorFields>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

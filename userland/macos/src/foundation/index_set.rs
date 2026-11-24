//! NSIndexSet and NSMutableIndexSet (D725): a sorted set of unsigned indexes kept as disjoint,
//! non-adjacent ranges, so a selection of a million rows is one range. Lookups walk the ranges;
//! adding or removing splices them. Moved here from AppKit, where it held one index (D373), so
//! a program that links Foundation alone finds it; AppKit's tables keep their selection in one.

use super::string::Range;
use super::{
    Definition, NSINDEXSET, NSINDEXSET_META, NSMUTABLEINDEXSET, NSMUTABLEINDEXSET_META,
    autoreleased, class_pointer, define as define_class, new_instance, string_from_utf8,
};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::block::invoke_address;
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods};

const NOT_FOUND: usize = isize::MAX as usize;

#[repr(C)]
#[derive(Debug)]
struct IndexSetObject {
    isa: usize,
    ranges: Buffer<(usize, usize)>,
}

fn ranges(object: Id) -> &'static mut Buffer<(usize, usize)> {
    // SAFETY: an NSIndexSet starts with these fields, used by one thread at a time.
    unsafe { &mut (*object.cast::<IndexSetObject>()).ranges }
}

fn splice(list: &mut Buffer<(usize, usize)>, from: usize, to: usize, with: &[(usize, usize)]) {
    let old = list.len();
    let removed = to - from;
    let added = with.len();
    if added > removed {
        for _ in 0..added - removed {
            if !list.push((0, 0)) {
                return;
            }
        }
        list.as_mut_slice().copy_within(to..old, to + added - removed);
    } else if removed > added {
        list.as_mut_slice().copy_within(to..old, from + added);
        for _ in 0..removed - added {
            list.pop();
        }
    }
    list.as_mut_slice()[from..from + added].copy_from_slice(with);
}

fn insert_range(list: &mut Buffer<(usize, usize)>, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let slice = list.as_slice();
    let first = slice
        .iter()
        .position(|&(_, last_end)| last_end >= start)
        .unwrap_or(slice.len());
    let (mut merged_start, mut merged_end) = (start, end);
    let mut last = first;
    while last < slice.len() && slice[last].0 <= end {
        merged_start = merged_start.min(slice[last].0);
        merged_end = merged_end.max(slice[last].1);
        last += 1;
    }
    splice(list, first, last, &[(merged_start, merged_end)]);
}

fn remove_range(list: &mut Buffer<(usize, usize)>, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let slice = list.as_slice();
    let first = slice
        .iter()
        .position(|&(_, range_end)| range_end > start)
        .unwrap_or(slice.len());
    let mut last = first;
    while last < slice.len() && slice[last].0 < end {
        last += 1;
    }
    if first == last {
        return;
    }
    let mut kept = [(0, 0); 2];
    let mut count = 0;
    if slice[first].0 < start {
        kept[count] = (slice[first].0, start);
        count += 1;
    }
    if slice[last - 1].1 > end {
        kept[count] = (end, slice[last - 1].1);
        count += 1;
    }
    splice(list, first, last, &kept[..count]);
}

/// A new, empty NSMutableIndexSet, retained.
pub fn new_mutable() -> Id {
    new_instance(class_pointer(&NSMUTABLEINDEXSET))
}

/// Whether `set` holds `index`.
pub fn contains(set: Id, index: usize) -> bool {
    !set.is_null()
        && ranges(set)
            .as_slice()
            .iter()
            .any(|&(start, end)| start <= index && index < end)
}

/// Adds `start..end` to `set`.
pub fn insert(set: Id, start: usize, end: usize) {
    insert_range(ranges(set), start, end);
}

/// Takes `start..end` out of `set`.
pub fn remove(set: Id, start: usize, end: usize) {
    remove_range(ranges(set), start, end);
}

/// Empties `set`.
pub fn clear(set: Id) {
    let mut old = core::mem::replace(ranges(set), Buffer::new());
    old.free();
}

/// How many indexes `set` holds.
pub fn count(set: Id) -> usize {
    match set.is_null() {
        true => 0,
        false => ranges(set)
            .as_slice()
            .iter()
            .map(|&(start, end)| end - start)
            .sum(),
    }
}

/// `set`'s smallest index.
pub fn first(set: Id) -> Option<usize> {
    match set.is_null() {
        true => None,
        false => ranges(set).as_slice().first().map(|&(start, _)| start),
    }
}

/// `set`'s ranges, each as `start..end`.
pub fn each_range(set: Id) -> &'static [(usize, usize)] {
    match set.is_null() {
        true => &[],
        false => ranges(set).as_slice(),
    }
}

/// A new NSIndexSet holding what `set` holds, retained.
pub fn immutable_copy(set: Id) -> Id {
    let copy = new_instance(class_pointer(&NSINDEXSET));
    if !copy.is_null() {
        for &(start, end) in each_range(set) {
            insert(copy, start, end);
        }
    }
    copy
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

fn made(class: *mut ClassObject, start: usize, end: usize) -> Id {
    let set = new_instance(class);
    if !set.is_null() {
        insert(set, start, end);
    }
    autoreleased(set)
}

fn range_end(range: Range) -> usize {
    range.location.saturating_add(range.length)
}

extern "C" fn index_set(this: Id, _: Sel) -> Id {
    made(this_class(this), 0, 0)
}

extern "C" fn index_set_with_index(this: Id, _: Sel, index: usize) -> Id {
    made(this_class(this), index, index.saturating_add(1))
}

extern "C" fn index_set_with_range(this: Id, _: Sel, range: Range) -> Id {
    made(this_class(this), range.location, range_end(range))
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn init_with_index(this: Id, _: Sel, index: usize) -> Id {
    if !this.is_null() {
        insert(this, index, index.saturating_add(1));
    }
    this
}

extern "C" fn init_with_range(this: Id, _: Sel, range: Range) -> Id {
    if !this.is_null() {
        insert(this, range.location, range_end(range));
    }
    this
}

extern "C" fn init_with_index_set(this: Id, _: Sel, other: Id) -> Id {
    if !this.is_null() {
        for &(start, end) in each_range(other) {
            insert(this, start, end);
        }
    }
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    clear(this);
    dispose(this);
}

extern "C" fn count_method(this: Id, _: Sel) -> usize {
    count(this)
}

extern "C" fn first_index(this: Id, _: Sel) -> usize {
    first(this).unwrap_or(NOT_FOUND)
}

extern "C" fn last_index(this: Id, _: Sel) -> usize {
    ranges(this)
        .as_slice()
        .last()
        .map_or(NOT_FOUND, |&(_, end)| end - 1)
}

extern "C" fn contains_index(this: Id, _: Sel, index: usize) -> u8 {
    u8::from(contains(this, index))
}

fn holds_whole(this: Id, start: usize, end: usize) -> bool {
    start >= end
        || ranges(this)
            .as_slice()
            .iter()
            .any(|&(range_start, range_end)| range_start <= start && end <= range_end)
}

extern "C" fn contains_range(this: Id, _: Sel, range: Range) -> u8 {
    u8::from(holds_whole(this, range.location, range_end(range)))
}

extern "C" fn contains_indexes(this: Id, _: Sel, other: Id) -> u8 {
    let all = each_range(other)
        .iter()
        .all(|&(start, end)| holds_whole(this, start, end));
    u8::from(all)
}

fn overlap(this: Id, start: usize, end: usize) -> usize {
    ranges(this)
        .as_slice()
        .iter()
        .map(|&(range_start, range_end)| {
            range_end.min(end).saturating_sub(range_start.max(start))
        })
        .sum()
}

extern "C" fn intersects_range(this: Id, _: Sel, range: Range) -> u8 {
    u8::from(overlap(this, range.location, range_end(range)) > 0)
}

extern "C" fn count_in_range(this: Id, _: Sel, range: Range) -> usize {
    overlap(this, range.location, range_end(range))
}

fn at_or_after(this: Id, index: usize) -> usize {
    ranges(this)
        .as_slice()
        .iter()
        .find(|&&(_, end)| end > index)
        .map_or(NOT_FOUND, |&(start, _)| start.max(index))
}

fn at_or_before(this: Id, index: usize) -> usize {
    ranges(this)
        .as_slice()
        .iter()
        .rev()
        .find(|&&(start, _)| start <= index)
        .map_or(NOT_FOUND, |&(_, end)| (end - 1).min(index))
}

extern "C" fn index_greater_than(this: Id, _: Sel, index: usize) -> usize {
    match index.checked_add(1) {
        Some(next) => at_or_after(this, next),
        None => NOT_FOUND,
    }
}

extern "C" fn index_greater_than_or_equal(this: Id, _: Sel, index: usize) -> usize {
    at_or_after(this, index)
}

extern "C" fn index_less_than(this: Id, _: Sel, index: usize) -> usize {
    match index.checked_sub(1) {
        Some(previous) => at_or_before(this, previous),
        None => NOT_FOUND,
    }
}

extern "C" fn index_less_than_or_equal(this: Id, _: Sel, index: usize) -> usize {
    at_or_before(this, index)
}

fn same_indexes(this: Id, other: Id) -> bool {
    !other.is_null()
        && root::is_kind_of(other, &NSINDEXSET)
        && ranges(this).as_slice() == ranges(other).as_slice()
}

extern "C" fn is_equal_to_index_set(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(other == this || same_indexes(this, other))
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    count(this)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    match root::is_kind_of(this, &NSMUTABLEINDEXSET) {
        true => immutable_copy(this),
        false => objc_retain(this),
    }
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let copy = new_mutable();
    if !copy.is_null() {
        for &(start, end) in each_range(this) {
            insert(copy, start, end);
        }
    }
    copy
}

fn snapshot(this: Id) -> Buffer<(usize, usize)> {
    let mut copy = Buffer::new();
    for &range in each_range(this) {
        copy.push(range);
    }
    copy
}

extern "C" fn enumerate_indexes(this: Id, _: Sel, block: Id) {
    if block.is_null() {
        return;
    }
    // SAFETY: the program's block takes the block, an index and a `BOOL *` to stop.
    let function: extern "C" fn(Id, usize, *mut u8) =
        unsafe { core::mem::transmute(invoke_address(block)) };
    let mut list = snapshot(this);
    let mut stop = 0u8;
    'ranges: for &(start, end) in list.as_slice() {
        for index in start..end {
            function(block, index, &mut stop);
            if stop != 0 {
                break 'ranges;
            }
        }
    }
    list.free();
}

extern "C" fn enumerate_ranges(this: Id, _: Sel, block: Id) {
    if block.is_null() {
        return;
    }
    // SAFETY: the program's block takes the block, an `NSRange` and a `BOOL *` to stop.
    let function: extern "C" fn(Id, Range, *mut u8) =
        unsafe { core::mem::transmute(invoke_address(block)) };
    let mut list = snapshot(this);
    let mut stop = 0u8;
    for &(start, end) in list.as_slice() {
        let range = Range {
            location: start,
            length: end - start,
        };
        function(block, range, &mut stop);
        if stop != 0 {
            break;
        }
    }
    list.free();
}

fn push_decimal(out: &mut Buffer<u8>, value: usize) {
    let mut digits = [0u8; 20];
    let mut length = 0;
    let mut rest = value;
    loop {
        digits[length] = b'0' + (rest % 10) as u8;
        length += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for &digit in digits[..length].iter().rev() {
        out.push(digit);
    }
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let mut out = Buffer::new();
    let name = root::class_name(this);
    for &byte in b"<".iter().chain(name).chain(b">[number of indexes: ") {
        out.push(byte);
    }
    push_decimal(&mut out, count(this));
    for &byte in b" (in " {
        out.push(byte);
    }
    push_decimal(&mut out, ranges(this).len());
    for &byte in b" ranges), indexes: (" {
        out.push(byte);
    }
    for (at, &(start, end)) in ranges(this).as_slice().iter().enumerate() {
        if at > 0 {
            out.push(b' ');
        }
        push_decimal(&mut out, start);
        if end - start > 1 {
            out.push(b'-');
            push_decimal(&mut out, end - 1);
        }
    }
    out.push(b')');
    out.push(b']');
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn add_index(this: Id, _: Sel, index: usize) {
    insert(this, index, index.saturating_add(1));
}

extern "C" fn add_range(this: Id, _: Sel, range: Range) {
    insert(this, range.location, range_end(range));
}

extern "C" fn add_indexes(this: Id, _: Sel, other: Id) {
    let mut list = snapshot(other);
    for &(start, end) in list.as_slice() {
        insert(this, start, end);
    }
    list.free();
}

extern "C" fn remove_index(this: Id, _: Sel, index: usize) {
    remove(this, index, index.saturating_add(1));
}

extern "C" fn remove_range_method(this: Id, _: Sel, range: Range) {
    remove(this, range.location, range_end(range));
}

extern "C" fn remove_indexes(this: Id, _: Sel, other: Id) {
    let mut list = snapshot(other);
    for &(start, end) in list.as_slice() {
        remove(this, start, end);
    }
    list.free();
}

extern "C" fn remove_all(this: Id, _: Sel) {
    clear(this);
}

/// Moves every index from `start` up by `delta` (down when negative, the indexes it lands on
/// below `start` removed first), as `shiftIndexesStartingAtIndex:by:` does.
pub fn shift(set: Id, start: usize, delta: isize) {
    if delta < 0 {
        remove(set, start.saturating_sub(delta.unsigned_abs()), start);
    }
    let mut old = core::mem::replace(ranges(set), Buffer::new());
    let moved = |index: usize| index.saturating_add_signed(delta);
    for &(range_start, range_end) in old.as_slice() {
        if range_end <= start {
            insert(set, range_start, range_end);
        } else if range_start >= start {
            insert(set, moved(range_start), moved(range_end));
        } else {
            insert(set, range_start, start);
            insert(set, moved(start), moved(range_end));
        }
    }
    old.free();
}

extern "C" fn shift_indexes(this: Id, _: Sel, start: usize, delta: isize) {
    shift(this, start, delta);
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithIndex:" => init_with_index,
        "initWithIndexesInRange:" => init_with_range,
        "initWithIndexSet:" => init_with_index_set,
        "dealloc" => dealloc,
        "count" => count_method,
        "firstIndex" => first_index,
        "lastIndex" => last_index,
        "containsIndex:" => contains_index,
        "containsIndexesInRange:" => contains_range,
        "containsIndexes:" => contains_indexes,
        "intersectsIndexesInRange:" => intersects_range,
        "countOfIndexesInRange:" => count_in_range,
        "indexGreaterThanIndex:" => index_greater_than,
        "indexGreaterThanOrEqualToIndex:" => index_greater_than_or_equal,
        "indexLessThanIndex:" => index_less_than,
        "indexLessThanOrEqualToIndex:" => index_less_than_or_equal,
        "isEqualToIndexSet:" => is_equal_to_index_set,
        "isEqual:" => is_equal_to_index_set,
        "hash" => hash,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "enumerateIndexesUsingBlock:" => enumerate_indexes,
        "enumerateRangesUsingBlock:" => enumerate_ranges,
        "description" => description,
    ];
    let class = methods![
        "indexSet" => index_set,
        "indexSetWithIndex:" => index_set_with_index,
        "indexSetWithIndexesInRange:" => index_set_with_range,
    ];
    let mutable_instance = methods![
        "addIndex:" => add_index,
        "addIndexesInRange:" => add_range,
        "addIndexes:" => add_indexes,
        "removeIndex:" => remove_index,
        "removeIndexesInRange:" => remove_range_method,
        "removeIndexes:" => remove_indexes,
        "removeAllIndexes" => remove_all,
        "shiftIndexesStartingAtIndex:by:" => shift_indexes,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSIndexSet\0",
            class: &NSINDEXSET,
            meta: &NSINDEXSET_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<IndexSetObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableIndexSet\0",
            class: &NSMUTABLEINDEXSET,
            meta: &NSMUTABLEINDEXSET_META,
            superclass: &NSINDEXSET,
            instance_size: core::mem::size_of::<IndexSetObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &[],
        },
    )
}

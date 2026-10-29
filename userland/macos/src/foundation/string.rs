//! NSString, NSMutableString and the constant string class clang's `@"..."` literals use
//! (`___CFConstantStringClassReference`). All three share the constant CFString's layout,
//! `{isa, flags, characters, length}`, mutable strings adding a capacity: ASCII text is kept
//! as bytes (with a NUL after them) and anything else as UTF-16 units, so `length` and
//! `characterAtIndex:` take constant time on every string.

use super::string_text::{self, Text};
use super::{
    CONSTANT_STRING, CONSTANT_STRING_META, Definition, NSMUTABLESTRING, NSMUTABLESTRING_META,
    NSSTRING, NSSTRING_META, UTF8_BUFFER, UTF8_BUFFER_META, autoreleased, class_pointer,
    define as define_class, is_string, new_instance,
};
use crate::format::{self, Output};
use crate::libsystem::heap;
use crate::libsystem::variadic::{RawVaList, VaList, variadic};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::class::IMMORTAL;
use crate::objc::root::NSOBJECT;
use crate::objc::sel;
use crate::objc::{Runtime, methods, reference_count, send, terminate_uncaught};
use core::cmp::Ordering;

const WIDE: u32 = 0x10;
const OWNED: u32 = 0x1_0000;
const ASCII_FLAGS: u32 = 0x7c8;
const WIDE_FLAGS: u32 = 0x7d0;
const UTF8_ENCODING: usize = 4;
const ASCII_ENCODING: usize = 1;

/// A string object's fields, the constant CFString's.
#[repr(C)]
#[derive(Debug)]
pub struct StringObject {
    isa: usize,
    flags: u32,
    reserved: u32,
    characters: *mut u8,
    length: usize,
}

/// A mutable string's fields.
#[repr(C)]
#[derive(Debug)]
pub struct MutableStringObject {
    string: StringObject,
    capacity: usize,
}

fn fields(object: Id) -> *mut StringObject {
    object.cast()
}

/// The text of the string `object`; valid until the string changes or goes.
pub fn text_of(object: Id) -> Text<'static> {
    if object.is_null() {
        return Text::Ascii(b"");
    }
    // SAFETY: Foundation's strings share the layout; the callers pass strings.
    let string = unsafe { &*fields(object) };
    if string.characters.is_null() || string.length == 0 {
        return Text::Ascii(b"");
    }
    if string.flags & WIDE != 0 {
        // SAFETY: a wide string's characters are `length` UTF-16 units.
        Text::Wide(unsafe {
            core::slice::from_raw_parts(string.characters.cast::<u16>(), string.length)
        })
    } else {
        // SAFETY: an ASCII string's characters are `length` bytes.
        Text::Ascii(unsafe { core::slice::from_raw_parts(string.characters, string.length) })
    }
}

fn is_mutable(object: Id) -> bool {
    crate::objc::root::is_kind_of(object, &NSMUTABLESTRING)
}

fn release_storage(object: Id) {
    // SAFETY: a string the callers own.
    let string = unsafe { &mut *fields(object) };
    if string.flags & OWNED != 0 {
        heap::release(string.characters);
    }
    string.characters = core::ptr::null_mut();
    string.length = 0;
}

fn set_units(object: Id, units: &[u16]) -> bool {
    let ascii = units.iter().all(|&unit| unit < 0x80);
    let bytes = if ascii {
        units.len() + 1
    } else {
        units.len() * 2 + 2
    };
    let storage = heap::allocate(bytes, true);
    if storage.is_null() {
        return false;
    }
    if ascii {
        for (index, &unit) in units.iter().enumerate() {
            // SAFETY: the new block holds `units.len() + 1` bytes.
            unsafe { storage.wrapping_add(index).write(unit as u8) };
        }
    } else {
        // SAFETY: the new block holds the units and a terminating 0.
        unsafe {
            storage
                .cast::<u16>()
                .copy_from_nonoverlapping(units.as_ptr(), units.len())
        };
    }
    release_storage(object);
    // SAFETY: a string the callers own.
    let string = unsafe { &mut *fields(object) };
    string.flags = OWNED | if ascii { ASCII_FLAGS } else { WIDE_FLAGS };
    string.characters = storage;
    string.length = units.len();
    if is_mutable(object) {
        // SAFETY: a mutable string has the mutable fields.
        unsafe { (*object.cast::<MutableStringObject>()).capacity = units.len() };
    }
    true
}

fn units_of(text: &Text<'_>) -> Buffer<u16> {
    let mut units = Buffer::new();
    units.reserve(text.len());
    for unit in text.units() {
        units.push(unit);
    }
    units
}

fn set_text(object: Id, text: &Text<'_>) -> bool {
    let mut units = units_of(text);
    let made = set_units(object, units.as_slice());
    units.free();
    made
}

/// A new string of `class` (counted once) holding `units`.
pub fn new_string_from_units(class: *mut ClassObject, units: &[u16]) -> Id {
    let object = new_instance(class);
    if object.is_null() || !set_units(object, units) {
        return core::ptr::null_mut();
    }
    object
}

/// A new string of `class` (counted once) holding the UTF-8 `bytes`; null if they are not
/// UTF-8.
pub fn new_string_from_utf8(class: *mut ClassObject, bytes: &[u8]) -> Id {
    let mut units = Buffer::new();
    units.reserve(bytes.len());
    let valid = string_text::decode_utf8(bytes, &mut |unit| {
        units.push(unit);
    });
    let object = if valid {
        new_string_from_units(class, units.as_slice())
    } else {
        core::ptr::null_mut()
    };
    units.free();
    object
}

/// An autoreleased NSString of the UTF-8 `bytes` (invalid ones give an empty string).
pub fn string_from_utf8(bytes: &[u8]) -> Id {
    let object = new_string_from_utf8(class_pointer(&NSSTRING), bytes);
    if object.is_null() {
        return autoreleased(new_string_from_units(class_pointer(&NSSTRING), &[]));
    }
    autoreleased(object)
}

/// Runs `work` with the UTF-8 of the string `object`, converted into a temporary buffer only
/// when it is not ASCII.
pub fn with_utf8<R>(object: Id, work: impl FnOnce(&[u8]) -> R) -> R {
    match text_of(object) {
        Text::Ascii(bytes) => work(bytes),
        text => {
            let mut bytes = Buffer::new();
            text.write_utf8(&mut |piece| {
                for &byte in piece {
                    bytes.push(byte);
                }
            });
            let result = work(bytes.as_slice());
            bytes.free();
            result
        }
    }
}

/// The formatted text of a `printf`-style format with `%@`, as UTF-8.
pub struct Formatted(pub Buffer<u8>);

impl Output for Formatted {
    fn write(&mut self, bytes: &[u8]) {
        if self.0.reserve(bytes.len()) {
            for &byte in bytes {
                self.0.push(byte);
            }
        }
    }

    fn object(&mut self, object: u64) -> usize {
        let object = object as usize as Id;
        if object.is_null() {
            self.write(b"(null)");
            return 6;
        }
        let description = send(object, sel!("description"));
        let before = self.0.len();
        with_utf8(description, |bytes| self.write(bytes));
        self.0.len() - before
    }
}

/// Formats the NSString `format` with the arguments `arguments` points at.
pub fn format_with(format: Id, arguments: RawVaList) -> Formatted {
    let mut output = Formatted(Buffer::new());
    with_utf8(format, |bytes| {
        // SAFETY: the caller's `va_list` holds the arguments the format names.
        let mut arguments = unsafe { VaList::new(arguments) };
        // SAFETY: `%s` arguments are the C strings the format says.
        unsafe { format::format(bytes, &mut arguments, &mut output) };
    });
    output
}

fn new_formatted(class: *mut ClassObject, format: Id, arguments: RawVaList) -> Id {
    let mut formatted = format_with(format, arguments);
    let object = new_string_from_utf8(class, formatted.0.as_slice());
    formatted.0.free();
    object
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

fn new_like(text: &Text<'_>) -> Id {
    let mut units = units_of(text);
    let object = new_string_from_units(class_pointer(&NSSTRING), units.as_slice());
    units.free();
    autoreleased(object)
}

fn range_failure(this: Id, method: &[u8]) -> ! {
    terminate_uncaught(
        b"NSRangeException",
        &[
            b"-[",
            crate::objc::root::class_name(this),
            b" ",
            method,
            b"]: Range or index out of bounds",
        ],
    )
}

extern "C" fn string(this: Id, _: Sel) -> Id {
    autoreleased(new_string_from_units(this_class(this), &[]))
}

unsafe extern "C" fn string_with_utf8_string(this: Id, _: Sel, bytes: *const u8) -> Id {
    if bytes.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's promise.
    let bytes = unsafe { crate::objc::selector::name_bytes(bytes) };
    autoreleased(new_string_from_utf8(this_class(this), bytes))
}

unsafe extern "C" fn string_with_c_string(this: Id, sel: Sel, bytes: *const u8, _: usize) -> Id {
    // SAFETY: the caller's promise.
    unsafe { string_with_utf8_string(this, sel, bytes) }
}

extern "C" fn string_with_string(this: Id, _: Sel, other: Id) -> Id {
    let object = new_instance(this_class(this));
    if object.is_null() || !set_text(object, &text_of(other)) {
        return core::ptr::null_mut();
    }
    autoreleased(object)
}

unsafe extern "C" fn string_with_characters(
    this: Id,
    _: Sel,
    characters: *const u16,
    length: usize,
) -> Id {
    let units = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller's promise.
        unsafe { core::slice::from_raw_parts(characters, length) }
    };
    autoreleased(new_string_from_units(this_class(this), units))
}

extern "C" fn string_with_format_list(this: Id, _: Sel, format: Id, arguments: RawVaList) -> Id {
    autoreleased(new_formatted(this_class(this), format, arguments))
}

variadic!(eggos_string_with_format => string_with_format_list, 3, "x3", "rcx");

extern "C" fn string_with_capacity(this: Id, _: Sel, _: usize) -> Id {
    autoreleased(new_string_from_units(this_class(this), &[]))
}

fn initialized(this: Id, made: bool) -> Id {
    if made {
        this
    } else {
        reference_count::objc_release(this);
        core::ptr::null_mut()
    }
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    initialized(this, set_units(this, &[]))
}

unsafe extern "C" fn init_with_utf8_string(this: Id, _: Sel, bytes: *const u8) -> Id {
    if bytes.is_null() {
        return initialized(this, false);
    }
    // SAFETY: the caller's promise.
    let bytes = unsafe { crate::objc::selector::name_bytes(bytes) };
    init_with_utf8(this, bytes)
}

fn init_with_utf8(this: Id, bytes: &[u8]) -> Id {
    let mut units = Buffer::new();
    let valid = string_text::decode_utf8(bytes, &mut |unit| {
        units.push(unit);
    });
    let made = valid && set_units(this, units.as_slice());
    units.free();
    initialized(this, made)
}

unsafe extern "C" fn init_with_bytes(
    this: Id,
    _: Sel,
    bytes: *const u8,
    length: usize,
    encoding: usize,
) -> Id {
    if !matches!(encoding, UTF8_ENCODING | ASCII_ENCODING) || (bytes.is_null() && length > 0) {
        return initialized(this, false);
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller's promise.
        unsafe { core::slice::from_raw_parts(bytes, length) }
    };
    init_with_utf8(this, bytes)
}

extern "C" fn init_with_string(this: Id, _: Sel, other: Id) -> Id {
    let text = text_of(other);
    initialized(this, set_text(this, &text))
}

unsafe extern "C" fn init_with_characters(
    this: Id,
    _: Sel,
    characters: *const u16,
    length: usize,
) -> Id {
    let units = if length == 0 {
        &[][..]
    } else {
        // SAFETY: the caller's promise.
        unsafe { core::slice::from_raw_parts(characters, length) }
    };
    initialized(this, set_units(this, units))
}

extern "C" fn init_with_format_arguments(this: Id, _: Sel, format: Id, arguments: RawVaList) -> Id {
    let mut formatted = format_with(format, arguments);
    let result = init_with_utf8(this, formatted.0.as_slice());
    formatted.0.free();
    result
}

variadic!(eggos_string_init_with_format => init_with_format_arguments, 3, "x3", "rcx");

extern "C" fn dealloc(this: Id, _: Sel) {
    release_storage(this);
    reference_count::dispose(this);
}

extern "C" fn length(this: Id, _: Sel) -> usize {
    text_of(this).len()
}

extern "C" fn character_at_index(this: Id, _: Sel, index: usize) -> u16 {
    let text = text_of(this);
    if index >= text.len() {
        range_failure(this, b"characterAtIndex:");
    }
    text.unit(index)
}

unsafe extern "C" fn get_characters(
    this: Id,
    _: Sel,
    buffer: *mut u16,
    location: usize,
    length: usize,
) {
    let text = text_of(this);
    if location
        .checked_add(length)
        .is_none_or(|end| end > text.len())
    {
        range_failure(this, b"getCharacters:range:");
    }
    for index in 0..length {
        // SAFETY: the caller's promise.
        unsafe {
            buffer
                .wrapping_add(index)
                .write(text.unit(location + index))
        };
    }
}

#[repr(C)]
struct Utf8Buffer {
    isa: usize,
    bytes: *mut u8,
}

extern "C" fn utf8_buffer_dealloc(this: Id, _: Sel) {
    // SAFETY: a UTF-8 buffer object.
    heap::release(unsafe { (*this.cast::<Utf8Buffer>()).bytes });
    reference_count::dispose(this);
}

extern "C" fn utf8_string(this: Id, _: Sel) -> *const u8 {
    let text = text_of(this);
    if let Text::Ascii(bytes) = text
        && !bytes.is_empty()
    {
        return bytes.as_ptr();
    }
    if text.is_empty() {
        return c"".as_ptr().cast::<u8>();
    }
    let bytes = heap::allocate(text.utf8_len() + 1, true);
    let holder = new_instance(class_pointer(&UTF8_BUFFER));
    if bytes.is_null() || holder.is_null() {
        heap::release(bytes);
        crate::objc::reference_count::objc_release(holder);
        return c"".as_ptr().cast::<u8>();
    }
    let mut at = 0;
    text.write_utf8(&mut |piece| {
        // SAFETY: the block holds the whole UTF-8 text and its NUL.
        unsafe {
            bytes
                .wrapping_add(at)
                .copy_from_nonoverlapping(piece.as_ptr(), piece.len())
        };
        at += piece.len();
    });
    // SAFETY: a new UTF-8 buffer object.
    unsafe { (*holder.cast::<Utf8Buffer>()).bytes = bytes };
    autoreleased(holder);
    bytes
}

extern "C" fn c_string_using_encoding(this: Id, sel: Sel, encoding: usize) -> *const u8 {
    if encoding == ASCII_ENCODING && !matches!(text_of(this), Text::Ascii(_)) {
        return core::ptr::null();
    }
    utf8_string(this, sel)
}

extern "C" fn length_of_bytes(this: Id, _: Sel, _: usize) -> usize {
    text_of(this).utf8_len()
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(this == other || (is_string(other) && text_of(this).equals(&text_of(other))))
}

extern "C" fn is_equal_to_string(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(this == other || (!other.is_null() && text_of(this).equals(&text_of(other))))
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    text_of(this).hash() as usize
}

fn ordering(order: Ordering) -> isize {
    match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

extern "C" fn compare(this: Id, _: Sel, other: Id) -> isize {
    ordering(text_of(this).compare(&text_of(other)))
}

extern "C" fn has_prefix(this: Id, _: Sel, other: Id) -> u8 {
    let prefix = text_of(other);
    u8::from(!prefix.is_empty() && text_of(this).starts_with(&prefix))
}

extern "C" fn has_suffix(this: Id, _: Sel, other: Id) -> u8 {
    let suffix = text_of(other);
    u8::from(!suffix.is_empty() && text_of(this).ends_with(&suffix))
}

extern "C" fn contains_string(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(text_of(this).find(&text_of(other), 0).is_some())
}

fn concatenated(parts: &[Text<'_>]) -> Id {
    let mut units = Buffer::new();
    for part in parts {
        units.reserve(part.len());
        for unit in part.units() {
            units.push(unit);
        }
    }
    let object = new_string_from_units(class_pointer(&NSSTRING), units.as_slice());
    units.free();
    autoreleased(object)
}

extern "C" fn string_by_appending_string(this: Id, _: Sel, other: Id) -> Id {
    concatenated(&[text_of(this), text_of(other)])
}

extern "C" fn string_by_appending_format_list(
    this: Id,
    _: Sel,
    format: Id,
    arguments: RawVaList,
) -> Id {
    let tail = autoreleased(new_formatted(class_pointer(&NSSTRING), format, arguments));
    concatenated(&[text_of(this), text_of(tail)])
}

variadic!(eggos_string_by_appending_format => string_by_appending_format_list, 3, "x3", "rcx");

extern "C" fn substring_from_index(this: Id, _: Sel, index: usize) -> Id {
    let text = text_of(this);
    if index > text.len() {
        range_failure(this, b"substringFromIndex:");
    }
    new_like(&text.slice(index, text.len()))
}

extern "C" fn substring_to_index(this: Id, _: Sel, index: usize) -> Id {
    let text = text_of(this);
    if index > text.len() {
        range_failure(this, b"substringToIndex:");
    }
    new_like(&text.slice(0, index))
}

extern "C" fn substring_with_range(this: Id, _: Sel, location: usize, length: usize) -> Id {
    let text = text_of(this);
    match location.checked_add(length) {
        Some(end) if end <= text.len() => new_like(&text.slice(location, end)),
        _ => range_failure(this, b"substringWithRange:"),
    }
}

fn changed_case(this: Id, upper: bool) -> Id {
    let text = text_of(this);
    let mut units = Buffer::new();
    units.reserve(text.len());
    for unit in text.units() {
        units.push(string_text::ascii_case(unit, upper));
    }
    let object = new_string_from_units(class_pointer(&NSSTRING), units.as_slice());
    units.free();
    autoreleased(object)
}

extern "C" fn uppercase_string(this: Id, _: Sel) -> Id {
    changed_case(this, true)
}

extern "C" fn lowercase_string(this: Id, _: Sel) -> Id {
    changed_case(this, false)
}

extern "C" fn components_separated_by_string(this: Id, _: Sel, separator: Id) -> Id {
    let text = text_of(this);
    let separator = text_of(separator);
    let mut parts: Buffer<Id> = Buffer::new();
    let mut start = 0;
    if !separator.is_empty() {
        while let Some(found) = text.find(&separator, start) {
            parts.push(new_like(&text.slice(start, found)));
            start = found + separator.len();
        }
    }
    parts.push(new_like(&text.slice(start, text.len())));
    let array = super::array::new_array(class_pointer(&super::NSARRAY), parts.as_slice());
    parts.free();
    autoreleased(array)
}

extern "C" fn replacing_occurrences(this: Id, _: Sel, target: Id, replacement: Id) -> Id {
    let text = text_of(this);
    let target = text_of(target);
    let replacement = text_of(replacement);
    let mut units = Buffer::new();
    let mut start = 0;
    if !target.is_empty() {
        while let Some(found) = text.find(&target, start) {
            for unit in text.slice(start, found).units().chain(replacement.units()) {
                units.push(unit);
            }
            start = found + target.len();
        }
    }
    for unit in text.slice(start, text.len()).units() {
        units.push(unit);
    }
    let object = new_string_from_units(class_pointer(&NSSTRING), units.as_slice());
    units.free();
    autoreleased(object)
}

extern "C" fn int_value(this: Id, _: Sel) -> i32 {
    text_of(this)
        .integer_value()
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

extern "C" fn integer_value(this: Id, _: Sel) -> i64 {
    text_of(this).integer_value()
}

extern "C" fn double_value(this: Id, _: Sel) -> f64 {
    text_of(this).double_value()
}

extern "C" fn float_value(this: Id, _: Sel) -> f32 {
    text_of(this).double_value() as f32
}

extern "C" fn bool_value(this: Id, _: Sel) -> u8 {
    u8::from(text_of(this).bool_value())
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    if is_mutable(this) {
        let text = text_of(this);
        let object = new_instance(class_pointer(&NSSTRING));
        if object.is_null() || !set_text(object, &text) {
            return core::ptr::null_mut();
        }
        return object;
    }
    reference_count::objc_retain(this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let text = text_of(this);
    let object = new_instance(class_pointer(&NSMUTABLESTRING));
    if object.is_null() || !set_text(object, &text) {
        return core::ptr::null_mut();
    }
    object
}

fn replace(this: Id, start: usize, end: usize, insert: &Text<'_>) {
    let text = text_of(this);
    if start > end || end > text.len() {
        range_failure(this, b"replaceCharactersInRange:withString:");
    }
    if let (Text::Ascii(_), Text::Ascii(added)) = (text, *insert)
        && start == text.len()
        && append_ascii(this, added)
    {
        return;
    }
    let mut units = Buffer::new();
    units.reserve(text.len() + insert.len());
    for unit in text
        .slice(0, start)
        .units()
        .chain(insert.units())
        .chain(text.slice(end, text.len()).units())
    {
        units.push(unit);
    }
    set_units(this, units.as_slice());
    units.free();
}

fn append_ascii(this: Id, added: &[u8]) -> bool {
    if !is_mutable(this) {
        return false;
    }
    // SAFETY: a mutable string the caller owns.
    let mutable = unsafe { &mut *this.cast::<MutableStringObject>() };
    if mutable.string.flags & OWNED == 0 {
        return false;
    }
    let length = mutable.string.length;
    let needed = length + added.len();
    if needed > mutable.capacity {
        let capacity = needed.max(mutable.capacity * 2).max(16);
        let grown = heap::reallocate(mutable.string.characters, capacity + 1);
        if grown.is_null() {
            return false;
        }
        mutable.string.characters = grown;
        mutable.capacity = capacity;
    }
    let characters = mutable.string.characters;
    // SAFETY: the block holds `capacity + 1` bytes, room for the added bytes and the NUL.
    unsafe {
        characters
            .wrapping_add(length)
            .copy_from_nonoverlapping(added.as_ptr(), added.len())
    };
    // SAFETY: as above.
    unsafe { characters.wrapping_add(needed).write(0) };
    mutable.string.length = needed;
    true
}

extern "C" fn append_string(this: Id, _: Sel, other: Id) {
    let added = text_of(other);
    let length = text_of(this).len();
    if other == this {
        let copy = autoreleased(copy_with_zone(this, sel!("copy"), 0));
        replace(this, length, length, &text_of(copy));
        return;
    }
    replace(this, length, length, &added);
}

extern "C" fn append_format_list(this: Id, _: Sel, format: Id, arguments: RawVaList) {
    let tail = autoreleased(new_formatted(class_pointer(&NSSTRING), format, arguments));
    let length = text_of(this).len();
    replace(this, length, length, &text_of(tail));
}

variadic!(eggos_string_append_format => append_format_list, 3, "x3", "rcx");

extern "C" fn set_string(this: Id, _: Sel, other: Id) {
    let mut copy = units_of(&text_of(other));
    set_units(this, copy.as_slice());
    copy.free();
}

extern "C" fn insert_string(this: Id, _: Sel, other: Id, index: usize) {
    let inserted = autoreleased(copy_with_zone(other, sel!("copy"), 0));
    replace(this, index, index, &text_of(inserted));
}

extern "C" fn delete_characters(this: Id, _: Sel, location: usize, length: usize) {
    match location.checked_add(length) {
        Some(end) => replace(this, location, end, &Text::Ascii(b"")),
        None => range_failure(this, b"deleteCharactersInRange:"),
    }
}

extern "C" fn mutable_init(this: Id, _: Sel) -> Id {
    initialized(this, set_units(this, &[]))
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithUTF8String:" => init_with_utf8_string,
        "initWithString:" => init_with_string,
        "initWithCharacters:length:" => init_with_characters,
        "initWithBytes:length:encoding:" => init_with_bytes,
        "initWithFormat:" => eggos_string_init_with_format,
        "initWithFormat:arguments:" => init_with_format_arguments,
        "dealloc" => dealloc,
        "length" => length,
        "characterAtIndex:" => character_at_index,
        "getCharacters:range:" => get_characters,
        "UTF8String" => utf8_string,
        "cStringUsingEncoding:" => c_string_using_encoding,
        "lengthOfBytesUsingEncoding:" => length_of_bytes,
        "isEqual:" => is_equal,
        "isEqualToString:" => is_equal_to_string,
        "hash" => hash,
        "compare:" => compare,
        "hasPrefix:" => has_prefix,
        "hasSuffix:" => has_suffix,
        "containsString:" => contains_string,
        "stringByAppendingString:" => string_by_appending_string,
        "stringByAppendingFormat:" => eggos_string_by_appending_format,
        "substringFromIndex:" => substring_from_index,
        "substringToIndex:" => substring_to_index,
        "substringWithRange:" => substring_with_range,
        "uppercaseString" => uppercase_string,
        "lowercaseString" => lowercase_string,
        "componentsSeparatedByString:" => components_separated_by_string,
        "stringByReplacingOccurrencesOfString:withString:" => replacing_occurrences,
        "intValue" => int_value,
        "integerValue" => integer_value,
        "longLongValue" => integer_value,
        "doubleValue" => double_value,
        "floatValue" => float_value,
        "boolValue" => bool_value,
        "description" => description,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
    ];
    let class = methods![
        "string" => string,
        "stringWithUTF8String:" => string_with_utf8_string,
        "stringWithCString:encoding:" => string_with_c_string,
        "stringWithString:" => string_with_string,
        "stringWithCharacters:length:" => string_with_characters,
        "stringWithFormat:" => eggos_string_with_format,
    ];
    let mutable_instance = methods![
        "init" => mutable_init,
        "appendString:" => append_string,
        "appendFormat:" => eggos_string_append_format,
        "setString:" => set_string,
        "insertString:atIndex:" => insert_string,
        "deleteCharactersInRange:" => delete_characters,
    ];
    let mutable_class = methods![
        "stringWithCapacity:" => string_with_capacity,
    ];
    let buffer_instance = methods![
        "dealloc" => utf8_buffer_dealloc,
    ];
    let string_size = core::mem::size_of::<StringObject>();
    define_class(
        runtime,
        &Definition {
            name: b"NSString\0",
            class: &NSSTRING,
            meta: &NSSTRING_META,
            superclass: &NSOBJECT,
            instance_size: string_size,
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableString\0",
            class: &NSMUTABLESTRING,
            meta: &NSMUTABLESTRING_META,
            superclass: &NSSTRING,
            instance_size: core::mem::size_of::<MutableStringObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &mutable_class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"__NSCFConstantString\0",
            class: &CONSTANT_STRING,
            meta: &CONSTANT_STRING_META,
            superclass: &NSSTRING,
            instance_size: string_size,
            flags: IMMORTAL,
            instance_methods: &[],
            class_methods: &[],
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"EggUTF8Buffer\0",
            class: &UTF8_BUFFER,
            meta: &UTF8_BUFFER_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<Utf8Buffer>(),
            flags: 0,
            instance_methods: &buffer_instance,
            class_methods: &[],
        },
    )
}

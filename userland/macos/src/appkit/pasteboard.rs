//! NSPasteboard (`docs/ux/macos-gui-v0.3.md` section 14.1, D233): the general pasteboard is the
//! window server's one shared text (D197), read and written on every call so what another app
//! copied a moment ago is what the next read gives; its change count is the server's serial.
//! The other named pasteboards (`Find`, `Font`, `Ruler`, `Drag`, unique names) are private to
//! the process: one text and a count of their own.

use super::{NSPASTEBOARD, NSPASTEBOARD_META, server, store, with_text};
use crate::foundation::{
    self, NSARRAY, NSDATA, NSSTRING, PASTEBOARD_NAME_GENERAL, PASTEBOARD_TYPE_STRING, class_pointer,
};
use crate::libsystem::{Exclusive, heap};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release};
use crate::objc::{Runtime, methods, root, sel, send, send_bool, send1, send2};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::pasteboard::{MAX_TEXT_BYTES, is_text_type, unique_name};

const NAMED_PASTEBOARDS: usize = 8;
const READ_ATTEMPTS: usize = 4;

#[repr(C)]
#[derive(Debug)]
struct PasteboardFields {
    isa: usize,
    private: bool,
    count: isize,
    name: Id,
    text: Buffer<u8>,
}

static GENERAL: AtomicUsize = AtomicUsize::new(0);
static NAMED: Exclusive<[usize; NAMED_PASTEBOARDS]> = Exclusive::new([0; NAMED_PASTEBOARDS]);
static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn fields(object: Id) -> &'static mut PasteboardFields {
    // SAFETY: the callers pass NSPasteboard instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<PasteboardFields>() }
}

fn new_pasteboard(name: Id, private: bool) -> Id {
    let object = foundation::new_instance(class_pointer(&NSPASTEBOARD));
    if object.is_null() {
        return object;
    }
    let fields = fields(object);
    fields.private = private;
    fields.text = Buffer::new();
    store(&mut fields.name, name);
    object
}

fn is_text_type_object(kind: Id) -> bool {
    !kind.is_null() && foundation::is_string(kind) && with_text(kind, is_text_type)
}

/// The shared clipboard's whole text as a new string, nil when it is empty, unreadable or
/// longer than one copy holds ([`MAX_TEXT_BYTES`]).
pub fn shared_text() -> Id {
    let Ok((mut length, _)) = server::clipboard(&mut []) else {
        return core::ptr::null_mut();
    };
    for _ in 0..READ_ATTEMPTS {
        if length == 0 || length > MAX_TEXT_BYTES {
            return core::ptr::null_mut();
        }
        let block = heap::allocate(length, false);
        if block.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: `block` holds `length` bytes, used only here and released below.
        let bytes = unsafe { core::slice::from_raw_parts_mut(block, length) };
        let read = server::clipboard(bytes);
        let string = match read {
            Ok((whole, _)) if whole == length => foundation::string_from_utf8(bytes),
            _ => core::ptr::null_mut(),
        };
        heap::release(block);
        match read {
            Ok((whole, _)) if whole != length => length = whole,
            _ => return string,
        }
    }
    core::ptr::null_mut()
}

/// Makes `text` the shared clipboard's text; false when it is too long or refused.
pub fn set_shared_text(text: &[u8]) -> bool {
    text.len() <= MAX_TEXT_BYTES && server::set_clipboard(text).is_ok()
}

fn serial() -> isize {
    server::clipboard(&mut []).map_or(0, |(_, serial)| serial as isize)
}

fn text_of(this: Id) -> Id {
    let fields = fields(this);
    match fields.private {
        true if fields.text.len() == 0 => core::ptr::null_mut(),
        true => foundation::string_from_utf8(fields.text.as_slice()),
        false => shared_text(),
    }
}

fn has_text(this: Id) -> bool {
    let fields = fields(this);
    match fields.private {
        true => fields.text.len() != 0,
        false => server::clipboard(&mut []).is_ok_and(|(length, _)| length != 0),
    }
}

fn write_text(this: Id, text: &[u8]) -> bool {
    if text.len() > MAX_TEXT_BYTES || core::str::from_utf8(text).is_err() {
        return false;
    }
    let fields = fields(this);
    if !fields.private {
        return set_shared_text(text);
    }
    fields.text.free();
    if !fields.text.reserve(text.len()) {
        return false;
    }
    for &byte in text {
        fields.text.push(byte);
    }
    fields.count += 1;
    true
}

fn clear(this: Id) -> isize {
    let fields = fields(this);
    if !fields.private {
        let _ = server::set_clipboard(&[]);
        return serial();
    }
    fields.text.free();
    fields.count += 1;
    fields.count
}

fn array_of(item: Option<Id>) -> Id {
    let class: Id = class_pointer(&NSARRAY).cast();
    match item {
        None => send(class, sel!("array")),
        Some(only) => send1(class, sel!("arrayWithObject:"), only as usize),
    }
}

fn each_item(array: Id, mut work: impl FnMut(Id) -> bool) {
    if array.is_null() {
        return;
    }
    let count = send(array, sel!("count")) as usize;
    for index in 0..count {
        if work(send1(array, sel!("objectAtIndex:"), index)) {
            return;
        }
    }
}

fn reads_strings(classes: Id) -> bool {
    let mut found = false;
    each_item(classes, |class| {
        found = !class.is_null() && root::descends_from(class.cast(), class_pointer(&NSSTRING));
        found
    });
    found
}

extern "C" fn general_pasteboard(_: Id, _: Sel) -> Id {
    let existing = GENERAL.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = new_pasteboard(foundation::constant(PASTEBOARD_NAME_GENERAL), false);
    if made.is_null() {
        return made;
    }
    match GENERAL.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => {
            dispose(made);
            winner as Id
        }
    }
}

extern "C" fn pasteboard_with_name(this: Id, sel: Sel, name: Id) -> Id {
    let general = foundation::constant(PASTEBOARD_NAME_GENERAL);
    if name.is_null() || send_bool(name, sel!("isEqualToString:"), general as usize) {
        return general_pasteboard(this, sel);
    }
    NAMED.with(|named| {
        let same = |slot: usize| {
            let equal = sel!("isEqualToString:");
            slot != 0 && send_bool(fields(slot as Id).name, equal, name as usize)
        };
        if let Some(&slot) = named.iter().find(|&&slot| same(slot)) {
            return slot as Id;
        }
        let copy = send(name, sel!("copy"));
        let made = new_pasteboard(copy, true);
        objc_release(copy);
        match named.iter_mut().find(|slot| **slot == 0) {
            Some(free) if !made.is_null() => {
                *free = made as usize;
                made
            }
            _ => foundation::autoreleased(made),
        }
    })
}

extern "C" fn pasteboard_with_unique_name(_: Id, _: Sel) -> Id {
    let name = unique_name(UNIQUE.fetch_add(1, Ordering::Relaxed) as u64);
    let text = foundation::string_from_utf8(&name);
    foundation::autoreleased(new_pasteboard(text, true))
}

extern "C" fn name(this: Id, _: Sel) -> Id {
    fields(this).name
}

extern "C" fn change_count(this: Id, _: Sel) -> isize {
    match fields(this).private {
        true => fields(this).count,
        false => serial(),
    }
}

extern "C" fn clear_contents(this: Id, _: Sel) -> isize {
    clear(this)
}

extern "C" fn declare_types(this: Id, _: Sel, _: Id, _: Id) -> isize {
    clear(this)
}

extern "C" fn set_string(this: Id, _: Sel, string: Id, kind: Id) -> u8 {
    if string.is_null() || !foundation::is_string(string) || !is_text_type_object(kind) {
        return 0;
    }
    u8::from(foundation::with_utf8(string, |bytes| {
        write_text(this, bytes)
    }))
}

extern "C" fn set_data(this: Id, _: Sel, data: Id, kind: Id) -> u8 {
    if data.is_null() || !foundation::is_data(data) || !is_text_type_object(kind) {
        return 0;
    }
    let length = send(data, sel!("length")) as usize;
    let bytes = send(data, sel!("bytes")) as *const u8;
    if length == 0 || bytes.is_null() {
        return u8::from(write_text(this, &[]));
    }
    // SAFETY: an NSData's `bytes` hold `length` bytes while the data lives, as it does here.
    let bytes = unsafe { core::slice::from_raw_parts(bytes, length) };
    u8::from(write_text(this, bytes))
}

extern "C" fn write_objects(this: Id, _: Sel, objects: Id) -> u8 {
    let mut written = false;
    each_item(objects, |object| {
        if foundation::is_string(object) {
            written = foundation::with_utf8(object, |bytes| write_text(this, bytes));
            return true;
        }
        false
    });
    u8::from(written)
}

extern "C" fn string_for_type(this: Id, _: Sel, kind: Id) -> Id {
    if !is_text_type_object(kind) {
        return core::ptr::null_mut();
    }
    text_of(this)
}

extern "C" fn data_for_type(this: Id, _: Sel, kind: Id) -> Id {
    if !is_text_type_object(kind) {
        return core::ptr::null_mut();
    }
    let text = text_of(this);
    if text.is_null() {
        return text;
    }
    foundation::with_utf8(text, |bytes| {
        send2(
            class_pointer(&NSDATA).cast(),
            sel!("dataWithBytes:length:"),
            bytes.as_ptr() as usize,
            bytes.len(),
        )
    })
}

extern "C" fn types(this: Id, _: Sel) -> Id {
    match has_text(this) {
        true => array_of(Some(foundation::constant(PASTEBOARD_TYPE_STRING))),
        false => array_of(None),
    }
}

extern "C" fn available_type(this: Id, _: Sel, kinds: Id) -> Id {
    if !has_text(this) {
        return core::ptr::null_mut();
    }
    let mut found = core::ptr::null_mut();
    each_item(kinds, |kind| {
        if is_text_type_object(kind) {
            found = kind;
            return true;
        }
        false
    });
    found
}

extern "C" fn read_objects(this: Id, _: Sel, classes: Id, _: Id) -> Id {
    if !reads_strings(classes) {
        return array_of(None);
    }
    let text = text_of(this);
    array_of((!text.is_null()).then_some(text))
}

extern "C" fn can_read_objects(this: Id, _: Sel, classes: Id, _: Id) -> u8 {
    u8::from(reads_strings(classes) && has_text(this))
}

extern "C" fn can_read_types(this: Id, _: Sel, kinds: Id) -> u8 {
    let mut found = false;
    each_item(kinds, |kind| {
        found = is_text_type_object(kind);
        found
    });
    u8::from(found && has_text(this))
}

extern "C" fn release_globally(_: Id, _: Sel) {}

extern "C" fn pasteboard_dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    fields.text.free();
    store(&mut fields.name, core::ptr::null_mut());
    dispose(this);
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "name" => name,
        "changeCount" => change_count,
        "clearContents" => clear_contents,
        "declareTypes:owner:" => declare_types,
        "setString:forType:" => set_string,
        "setData:forType:" => set_data,
        "writeObjects:" => write_objects,
        "stringForType:" => string_for_type,
        "dataForType:" => data_for_type,
        "types" => types,
        "availableTypeFromArray:" => available_type,
        "readObjectsForClasses:options:" => read_objects,
        "canReadObjectForClasses:options:" => can_read_objects,
        "canReadItemWithDataConformingToTypes:" => can_read_types,
        "releaseGlobally" => release_globally,
        "dealloc" => pasteboard_dealloc,
    ];
    let class = methods![
        "generalPasteboard" => general_pasteboard,
        "pasteboardWithName:" => pasteboard_with_name,
        "pasteboardWithUniqueName" => pasteboard_with_unique_name,
    ];
    super::define(
        runtime,
        b"NSPasteboard\0",
        (&NSPASTEBOARD, &NSPASTEBOARD_META),
        &root::NSOBJECT,
        core::mem::size_of::<PasteboardFields>(),
        &instance,
        &class,
    )
}

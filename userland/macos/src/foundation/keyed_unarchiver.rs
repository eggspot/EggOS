//! NSCoder and NSKeyedUnarchiver (D360): a keyed archive is a property list (binary or XML)
//! whose `$archiver` is `NSKeyedArchiver`, whose `$top` names the root object (`root`) and whose
//! `$objects` holds every object, `$null` first; objects refer to one another by UID. A plain
//! string, number, boolean or data is itself; a dictionary names its class by `$class` (a UID of
//! `{$classname, $classes}`). The Foundation classes decode natively (strings from `NS.string`,
//! arrays and sets from `NS.objects`, dictionaries from `NS.keys` and `NS.objects`, data from
//! `NS.data`, dates from `NS.time`, `NSNull`); any other class is allocated and sent
//! `initWithCoder:` with the unarchiver, whose `decode...ForKey:` read the keys of the object
//! being decoded. Each object is decoded once and shared, so the graph keeps its sharing. An
//! archive is untrusted input: every UID is checked, and a reference cycle through natively
//! decoded objects ends as nil rather than recursing.

use super::array::new_array;
use super::{
    Definition, NSARRAY, NSCODER, NSCODER_META, NSDATA, NSDATE, NSDICTIONARY, NSKEYEDUNARCHIVER,
    NSKEYEDUNARCHIVER_META, NSMUTABLEARRAY, NSMUTABLEDICTIONARY, NSMUTABLESET, NSMUTABLESTRING,
    NSSET, NSSTRING, autoreleased, class_pointer, define as define_class, new_instance,
};
use crate::libsystem::heap;
use crate::objc::sel;
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, send, send1};
use eggos_plist::{Document, Kind, Object};

const TOP: usize = usize::MAX;
const DECODING: Id = 1 as Id;
const ROOT_KEY: &[u8] = b"root";
const KEY_BYTES: usize = 256;
const MOST_OBJECTS: usize = 1 << 20;

#[repr(C)]
struct Unarchiver {
    isa: usize,
    data: Id,
    cache: *mut Id,
    objects: usize,
    current: usize,
    secure: bool,
}

fn fields(this: Id) -> &'static mut Unarchiver {
    // SAFETY: an unarchiver, which one thread decodes with at a time.
    unsafe { &mut *this.cast::<Unarchiver>() }
}

fn document(this: Id) -> Option<Document<'static>> {
    let data = fields(this).data;
    if data.is_null() {
        return None;
    }
    let document = Document::parse(super::data::bytes_of(data)).ok()?;
    let archiver = document.top().get(b"$archiver")?.text()?;
    archiver.equals(b"NSKeyedArchiver").then_some(document)
}

fn objects_of<'b>(document: &Document<'b>) -> Option<Object<'b>> {
    document.top().get(b"$objects")
}

fn current_object<'b>(this: Id, document: &Document<'b>) -> Option<Object<'b>> {
    match fields(this).current {
        TOP => document.top().get(b"$top"),
        uid => objects_of(document)?.item(uid),
    }
}

fn key_bytes(key: Id, out: &mut [u8; KEY_BYTES]) -> Option<usize> {
    if key.is_null() {
        return None;
    }
    let mut length = 0;
    let mut fits = true;
    super::string::text_of(key).write_utf8(&mut |bytes: &[u8]| {
        let end = length + bytes.len();
        match out.get_mut(length..end) {
            Some(slot) => {
                slot.copy_from_slice(bytes);
                length = end;
            }
            None => fits = false,
        }
    });
    fits.then_some(length)
}

fn value_for<'b>(this: Id, document: &Document<'b>, key: Id) -> Option<Object<'b>> {
    let mut bytes = [0u8; KEY_BYTES];
    let length = key_bytes(key, &mut bytes)?;
    current_object(this, document)?.get(&bytes[..length])
}

pub(super) fn string_of(object: &Object<'_>) -> Id {
    let Some(text) = object.text() else {
        return core::ptr::null_mut();
    };
    match text.as_utf8() {
        Some(bytes) => super::string::new_string_from_utf8(class_pointer(&NSSTRING), bytes),
        None => {
            let mut units: Buffer<u16> = Buffer::new();
            for character in text.chars() {
                let mut pair = [0u16; 2];
                for &unit in character.encode_utf16(&mut pair).iter() {
                    units.push(unit);
                }
            }
            let class = class_pointer(&NSSTRING);
            let string = super::string::new_string_from_units(class, units.as_slice());
            units.free();
            string
        }
    }
}

pub(super) fn data_of(object: &Object<'_>) -> Id {
    let Some(data) = object.data() else {
        return core::ptr::null_mut();
    };
    let mut bytes: Buffer<u8> = Buffer::new();
    for byte in data.bytes() {
        bytes.push(byte);
    }
    let made = super::data::new_data(class_pointer(&NSDATA), bytes.as_slice());
    bytes.free();
    made
}

fn class_name_of<'b>(document: &Document<'b>, object: &Object<'b>) -> Option<Object<'b>> {
    let class = object.get(b"$class")?.uid()?;
    let class = objects_of(document)?.item(usize::try_from(class).ok()?)?;
    class.get(b"$classname")
}

fn uids_of(this: Id, list: Option<Object<'_>>, out: &mut Buffer<Id>) {
    let Some(items) = list.and_then(|list| list.items()) else {
        return;
    };
    for item in items {
        let decoded = item
            .uid()
            .map_or(core::ptr::null_mut(), |uid| decode_uid(this, uid));
        if !decoded.is_null() {
            out.push(decoded);
        }
    }
}

fn native(this: Id, object: &Object<'_>, name: &[u8]) -> Option<Id> {
    let many = |list: &[u8], into: &mut Buffer<Id>| uids_of(this, object.get(list), into);
    Some(match name {
        b"NSString" | b"NSMutableString" => {
            let text = object.get(b"NS.string")?;
            let string = string_of(&text);
            match name {
                b"NSMutableString" => {
                    let mutable = send(string, sel!("mutableCopy"));
                    objc_release(string);
                    mutable
                }
                _ => string,
            }
        }
        b"NSArray" | b"NSMutableArray" | b"NSSet" | b"NSMutableSet" => {
            let mut items = Buffer::new();
            many(b"NS.objects", &mut items);
            let class = match name {
                b"NSArray" => class_pointer(&NSARRAY),
                b"NSMutableArray" => class_pointer(&NSMUTABLEARRAY),
                b"NSSet" => class_pointer(&NSSET),
                _ => class_pointer(&NSMUTABLESET),
            };
            let made = match name {
                b"NSArray" | b"NSMutableArray" => new_array(class, items.as_slice()),
                _ => {
                    let array = new_array(class_pointer(&NSARRAY), items.as_slice());
                    let set = send1(class.cast(), sel!("setWithArray:"), array as usize);
                    objc_release(array);
                    objc_retain(set)
                }
            };
            items.free();
            made
        }
        b"NSDictionary" | b"NSMutableDictionary" => {
            let (mut keys, mut values) = (Buffer::new(), Buffer::new());
            many(b"NS.keys", &mut keys);
            many(b"NS.objects", &mut values);
            let count = keys.len().min(values.len());
            let class = match name {
                b"NSDictionary" => class_pointer(&NSDICTIONARY),
                _ => class_pointer(&NSMUTABLEDICTIONARY),
            };
            let made = super::dictionary::new_dictionary(
                class,
                &values.as_slice()[..count],
                &keys.as_slice()[..count],
            );
            keys.free();
            values.free();
            made
        }
        b"NSData" | b"NSMutableData" => data_of(&object.get(b"NS.data")?),
        b"NSDate" => {
            let seconds = object.get(b"NS.time")?;
            let seconds = seconds
                .real()
                .or_else(|| seconds.integer().map(|value| value as f64))?;
            super::date::new_date(class_pointer(&NSDATE), seconds)
        }
        b"NSNull" => {
            let class = crate::objc::class_named(b"NSNull");
            objc_retain(send(class.cast(), sel!("null")))
        }
        _ => return None,
    })
}

fn decode_uid(this: Id, uid: u64) -> Id {
    let unarchiver = fields(this);
    let Some(index) = usize::try_from(uid)
        .ok()
        .filter(|&index| index < unarchiver.objects)
    else {
        return core::ptr::null_mut();
    };
    // SAFETY: the cache holds `objects` slots.
    let cached = unsafe { unarchiver.cache.add(index).read() };
    match cached {
        DECODING => return core::ptr::null_mut(),
        cached if !cached.is_null() => return cached,
        _ => {}
    }
    let Some(document) = document(this) else {
        return core::ptr::null_mut();
    };
    let Some(object) = objects_of(&document).and_then(|objects| objects.item(index)) else {
        return core::ptr::null_mut();
    };
    let decoded = match object.kind() {
        Some(Kind::String) if index == 0 => return core::ptr::null_mut(),
        Some(Kind::String) => string_of(&object),
        Some(Kind::Integer) => {
            let value = object.integer().unwrap_or(0);
            super::number::new_integer(value as i64)
        }
        Some(Kind::Real) => super::number::new_double(object.real().unwrap_or(0.0)),
        Some(Kind::Boolean) => {
            let truth = object.boolean() == Some(true);
            objc_retain(super::number::boolean_object(truth) as Id)
        }
        Some(Kind::Data) => data_of(&object),
        Some(Kind::Dictionary) => {
            // SAFETY: as above.
            unsafe { fields(this).cache.add(index).write(DECODING) };
            let made = decode_class(this, &document, &object, index);
            // SAFETY: as above.
            unsafe { fields(this).cache.add(index).write(core::ptr::null_mut()) };
            made
        }
        _ => core::ptr::null_mut(),
    };
    // SAFETY: as above.
    unsafe { fields(this).cache.add(index).write(decoded) };
    decoded
}

fn decode_class(this: Id, document: &Document<'_>, object: &Object<'_>, index: usize) -> Id {
    let Some(name) = class_name_of(document, object).and_then(|name| name.text()) else {
        return core::ptr::null_mut();
    };
    let mut bytes = [0u8; KEY_BYTES];
    let mut length = 0;
    for character in name.chars() {
        let mut encoded = [0u8; 4];
        let encoded = character.encode_utf8(&mut encoded).as_bytes();
        let end = length + encoded.len();
        let Some(slot) = bytes.get_mut(length..end) else {
            return core::ptr::null_mut();
        };
        slot.copy_from_slice(encoded);
        length = end;
    }
    let name = &bytes[..length];
    if let Some(made) = native(this, object, name) {
        return made;
    }
    let class: *mut ClassObject = crate::objc::class_named(name);
    if class.is_null() {
        return core::ptr::null_mut();
    }
    let allocated = send(class.cast(), sel!("alloc"));
    if allocated.is_null() {
        return allocated;
    }
    let unarchiver = fields(this);
    let outer = unarchiver.current;
    unarchiver.current = index;
    let made = send1(allocated, sel!("initWithCoder:"), this as usize);
    fields(this).current = outer;
    made
}

fn release_cache(this: Id) {
    let unarchiver = fields(this);
    for index in 0..unarchiver.objects {
        // SAFETY: the cache holds `objects` slots.
        let object = unsafe { unarchiver.cache.add(index).read() };
        if !object.is_null() && object != DECODING {
            objc_release(object);
        }
    }
    heap::release(unarchiver.cache.cast());
    unarchiver.cache = core::ptr::null_mut();
    unarchiver.objects = 0;
}

extern "C" fn init_for_reading_with_data(this: Id, _: Sel, data: Id) -> Id {
    if this.is_null() {
        return this;
    }
    objc_retain(data);
    let unarchiver = fields(this);
    unarchiver.data = data;
    unarchiver.current = TOP;
    let count = document(this)
        .and_then(|document| objects_of(&document)?.count())
        .filter(|&count| count <= MOST_OBJECTS);
    let Some(count) = count else {
        objc_release(this);
        return core::ptr::null_mut();
    };
    let cache = heap::allocate(count.max(1) * core::mem::size_of::<Id>(), true);
    if cache.is_null() {
        objc_release(this);
        return core::ptr::null_mut();
    }
    let unarchiver = fields(this);
    unarchiver.cache = cache.cast();
    unarchiver.objects = count;
    this
}

extern "C" fn init_for_reading_from_data(this: Id, sel: Sel, data: Id, error: *mut Id) -> Id {
    if !error.is_null() {
        // SAFETY: the caller's `NSError **`.
        unsafe { error.write(core::ptr::null_mut()) };
    }
    let made = init_for_reading_with_data(this, sel, data);
    if !made.is_null() {
        fields(made).secure = true;
    }
    made
}

extern "C" fn dealloc(this: Id, _: Sel) {
    release_cache(this);
    objc_release(fields(this).data);
    crate::objc::reference_count::dispose(this);
}

extern "C" fn decode_object_for_key(this: Id, _: Sel, key: Id) -> Id {
    let Some(document) = document(this) else {
        return core::ptr::null_mut();
    };
    let found = value_for(this, &document, key).and_then(|value| value.uid());
    match found {
        Some(uid) => autoreleased(objc_retain(decode_uid(this, uid))),
        None => core::ptr::null_mut(),
    }
}

extern "C" fn decode_object_of_class_for_key(this: Id, sel: Sel, _: Id, key: Id) -> Id {
    decode_object_for_key(this, sel, key)
}

extern "C" fn decode_integer_for_key(this: Id, _: Sel, key: Id) -> i64 {
    let Some(document) = document(this) else {
        return 0;
    };
    match value_for(this, &document, key) {
        Some(value) => value
            .integer()
            .map(|value| value as i64)
            .or_else(|| value.real().map(|value| value as i64))
            .or_else(|| value.boolean().map(i64::from))
            .unwrap_or(0),
        None => 0,
    }
}

extern "C" fn decode_int32_for_key(this: Id, sel: Sel, key: Id) -> i32 {
    decode_integer_for_key(this, sel, key) as i32
}

extern "C" fn decode_bool_for_key(this: Id, _: Sel, key: Id) -> u8 {
    let Some(document) = document(this) else {
        return 0;
    };
    let value = value_for(this, &document, key);
    let truth = value.as_ref().and_then(Object::boolean).or_else(|| {
        value
            .as_ref()
            .and_then(Object::integer)
            .map(|value| value != 0)
    });
    u8::from(truth == Some(true))
}

extern "C" fn decode_double_for_key(this: Id, _: Sel, key: Id) -> f64 {
    let Some(document) = document(this) else {
        return 0.0;
    };
    match value_for(this, &document, key) {
        Some(value) => value
            .real()
            .or_else(|| value.integer().map(|value| value as f64))
            .unwrap_or(0.0),
        None => 0.0,
    }
}

extern "C" fn decode_float_for_key(this: Id, sel: Sel, key: Id) -> f32 {
    decode_double_for_key(this, sel, key) as f32
}

extern "C" fn contains_value_for_key(this: Id, _: Sel, key: Id) -> u8 {
    let found = document(this).and_then(|document| value_for(this, &document, key).map(|_| ()));
    u8::from(found.is_some())
}

extern "C" fn allows_keyed_coding(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn requires_secure_coding(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).secure)
}

extern "C" fn set_requires_secure_coding(this: Id, _: Sel, value: u8) {
    fields(this).secure = value != 0;
}

extern "C" fn finish_decoding(_: Id, _: Sel) {}

fn root_of(this: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let Some(document) = document(this) else {
        return core::ptr::null_mut();
    };
    let root = document
        .top()
        .get(b"$top")
        .and_then(|top| top.get(ROOT_KEY))
        .and_then(|root| root.uid());
    root.map_or(core::ptr::null_mut(), |uid| decode_uid(this, uid))
}

fn unarchive(data: Id) -> Id {
    let class = class_pointer(&NSKEYEDUNARCHIVER);
    let unarchiver = init_for_reading_with_data(new_instance(class), sel!("init"), data);
    if unarchiver.is_null() {
        return unarchiver;
    }
    let root = objc_retain(root_of(unarchiver));
    objc_release(unarchiver);
    autoreleased(root)
}

extern "C" fn unarchive_object_with_data(_: Id, _: Sel, data: Id) -> Id {
    unarchive(data)
}

extern "C" fn unarchive_object_with_file(_: Id, _: Sel, path: Id) -> Id {
    let data = send1(
        class_pointer(&NSDATA).cast(),
        sel!("dataWithContentsOfFile:"),
        path as usize,
    );
    unarchive(data)
}

extern "C" fn unarchived_object_of_class(_: Id, _: Sel, _: Id, data: Id, error: *mut Id) -> Id {
    if !error.is_null() {
        // SAFETY: the caller's `NSError **`.
        unsafe { error.write(core::ptr::null_mut()) };
    }
    unarchive(data)
}

extern "C" fn unarchive_top_level(_: Id, _: Sel, data: Id, error: *mut Id) -> Id {
    if !error.is_null() {
        // SAFETY: the caller's `NSError **`.
        unsafe { error.write(core::ptr::null_mut()) };
    }
    unarchive(data)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let coder = methods![
        "allowsKeyedCoding" => allows_keyed_coding,
    ];
    let instance = methods![
        "initForReadingWithData:" => init_for_reading_with_data,
        "initForReadingFromData:error:" => init_for_reading_from_data,
        "dealloc" => dealloc,
        "decodeObjectForKey:" => decode_object_for_key,
        "decodeObjectOfClass:forKey:" => decode_object_of_class_for_key,
        "decodeObjectOfClasses:forKey:" => decode_object_of_class_for_key,
        "decodeIntegerForKey:" => decode_integer_for_key,
        "decodeInt64ForKey:" => decode_integer_for_key,
        "decodeIntForKey:" => decode_int32_for_key,
        "decodeInt32ForKey:" => decode_int32_for_key,
        "decodeBoolForKey:" => decode_bool_for_key,
        "decodeDoubleForKey:" => decode_double_for_key,
        "decodeFloatForKey:" => decode_float_for_key,
        "containsValueForKey:" => contains_value_for_key,
        "allowsKeyedCoding" => allows_keyed_coding,
        "requiresSecureCoding" => requires_secure_coding,
        "setRequiresSecureCoding:" => set_requires_secure_coding,
        "finishDecoding" => finish_decoding,
    ];
    let class = methods![
        "unarchiveObjectWithData:" => unarchive_object_with_data,
        "unarchiveObjectWithFile:" => unarchive_object_with_file,
        "unarchivedObjectOfClass:fromData:error:" => unarchived_object_of_class,
        "unarchivedObjectOfClasses:fromData:error:" => unarchived_object_of_class,
        "unarchiveTopLevelObjectWithData:error:" => unarchive_top_level,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSCoder\0",
            class: &NSCODER,
            meta: &NSCODER_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<usize>(),
            flags: 0,
            instance_methods: &coder,
            class_methods: &[],
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSKeyedUnarchiver\0",
            class: &NSKEYEDUNARCHIVER,
            meta: &NSKEYEDUNARCHIVER_META,
            superclass: &NSCODER,
            instance_size: core::mem::size_of::<Unarchiver>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

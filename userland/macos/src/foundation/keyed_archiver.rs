//! NSKeyedArchiver (D365): an object graph written as a keyed archive in an XML property list,
//! the layout NSKeyedUnarchiver reads (D360): `$archiver`, `$version` 100000, `$top` with the
//! root's UID and `$objects`, `$null` first. Each object is written once, by identity, and
//! referred to by its UID: strings, numbers, booleans and data plainly; arrays, sets,
//! dictionaries, dates and NSNull as dictionaries naming their class; any other object as the
//! keys its `encodeWithCoder:` gives. A class is written once, `{$classname, $classes}`.

use super::{
    Definition, NSCODER, NSDATA, NSKEYEDARCHIVER, NSKEYEDARCHIVER_META, NSMUTABLEARRAY,
    NSMUTABLEDICTIONARY, NSMUTABLESET, autoreleased, class_pointer, define as define_class,
    is_array, is_data, is_date, is_dictionary, is_number, is_set, is_string, new_instance,
};
use crate::objc::sel;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::objc_release;
use crate::objc::root;
use crate::objc::{Runtime, methods, send1, send2};

const NO_ENTRY: u32 = u32::MAX;
const ARCHIVE_VERSION: i64 = 100_000;
const HEADER: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \
\"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n";

#[derive(Debug, Clone, Copy)]
enum Value {
    Uid(u32),
    Integer(i64),
    Real(f64),
    Boolean(bool),
    Text(u32, u32),
    Data(u32, u32),
    Uids(u32, u32),
}

#[derive(Debug, Clone, Copy)]
struct Field {
    entry: u32,
    key: (u32, u32),
    value: Value,
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Null,
    Plain(Value),
    Object(u32),
    Class(u32, u32),
}

#[repr(C)]
struct Archiver {
    isa: usize,
    entries: Buffer<Entry>,
    fields: Buffer<Field>,
    bytes: Buffer<u8>,
    uids: Buffer<u32>,
    seen: Buffer<(Id, u32)>,
    current: u32,
    root: u32,
    secure: bool,
    finished: Id,
}

fn fields_of(this: Id) -> &'static mut Archiver {
    // SAFETY: an archiver, which one thread encodes with at a time.
    unsafe { &mut *this.cast::<Archiver>() }
}

fn set_up(this: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let archiver = fields_of(this);
    archiver.entries = Buffer::new();
    archiver.fields = Buffer::new();
    archiver.bytes = Buffer::new();
    archiver.uids = Buffer::new();
    archiver.seen = Buffer::new();
    archiver.entries.push(Entry::Null);
    archiver.current = NO_ENTRY;
    archiver.root = NO_ENTRY;
    archiver.finished = core::ptr::null_mut();
    this
}

fn stored(this: Id, bytes: &[u8]) -> (u32, u32) {
    let archiver = fields_of(this);
    let start = archiver.bytes.len() as u32;
    for &byte in bytes {
        archiver.bytes.push(byte);
    }
    (start, bytes.len() as u32)
}

fn stored_string(this: Id, string: Id) -> (u32, u32) {
    let archiver = fields_of(this);
    let start = archiver.bytes.len() as u32;
    super::string::text_of(string).write_utf8(&mut |bytes: &[u8]| {
        for &byte in bytes {
            archiver.bytes.push(byte);
        }
    });
    (start, archiver.bytes.len() as u32 - start)
}

fn text(this: Id, (start, length): (u32, u32)) -> &'static [u8] {
    let bytes = fields_of(this).bytes.as_slice();
    &bytes[start as usize..(start + length) as usize]
}

fn class_entry(this: Id, name: &[u8]) -> u32 {
    let archiver = fields_of(this);
    let found = archiver
        .entries
        .as_slice()
        .iter()
        .position(|entry| match *entry {
            Entry::Class(start, length) => text(this, (start, length)) == name,
            _ => false,
        });
    if let Some(at) = found {
        return at as u32;
    }
    let (start, length) = stored(this, name);
    let archiver = fields_of(this);
    archiver.entries.push(Entry::Class(start, length));
    archiver.entries.len() as u32 - 1
}

fn add_field(this: Id, entry: u32, key: &[u8], value: Value) {
    let key = stored(this, key);
    fields_of(this).fields.push(Field { entry, key, value });
}

fn add_uids(this: Id, objects: &[Id]) -> Value {
    let mut uids: Buffer<u32> = Buffer::new();
    for &object in objects {
        uids.push(encode(this, object));
    }
    let archiver = fields_of(this);
    let start = archiver.uids.len() as u32;
    for &uid in uids.as_slice() {
        archiver.uids.push(uid);
    }
    let count = uids.len() as u32;
    uids.free();
    Value::Uids(start, count)
}

fn class_name_for(object: Id) -> &'static [u8] {
    match () {
        () if is_array(object) && root::is_kind_of(object, &NSMUTABLEARRAY) => b"NSMutableArray",
        () if is_array(object) => b"NSArray",
        () if is_set(object) && root::is_kind_of(object, &NSMUTABLESET) => b"NSMutableSet",
        () if is_set(object) => b"NSSet",
        () if is_dictionary(object) && root::is_kind_of(object, &NSMUTABLEDICTIONARY) => {
            b"NSMutableDictionary"
        }
        () if is_dictionary(object) => b"NSDictionary",
        () if is_date(object) => b"NSDate",
        () => root::class_name(object),
    }
}

fn plain(object: Id, this: Id) -> Option<Value> {
    Some(match () {
        () if is_string(object) => {
            let (start, length) = stored_string(this, object);
            Value::Text(start, length)
        }
        () if object as u64 == super::number::boolean_object(true) => Value::Boolean(true),
        () if object as u64 == super::number::boolean_object(false) => Value::Boolean(false),
        () if is_number(object) => match super::number::exact_value(object) {
            super::number::Exact::Integer(value) => Value::Integer(value as i64),
            super::number::Exact::Real(value) => Value::Real(value),
        },
        () if is_data(object) => {
            let (start, length) = stored(this, super::data::bytes_of(object));
            Value::Data(start, length)
        }
        () => return None,
    })
}

fn encode(this: Id, object: Id) -> u32 {
    if object.is_null() {
        return 0;
    }
    let seen = fields_of(this)
        .seen
        .as_slice()
        .iter()
        .find(|(known, _)| *known == object);
    if let Some(&(_, uid)) = seen {
        return uid;
    }
    let uid = fields_of(this).entries.len() as u32;
    fields_of(this).entries.push(Entry::Null);
    fields_of(this).seen.push((object, uid));
    if let Some(value) = plain(object, this) {
        fields_of(this).entries.as_mut_slice()[uid as usize] = Entry::Plain(value);
        return uid;
    }
    let class = class_entry(this, class_name_for(object));
    fields_of(this).entries.as_mut_slice()[uid as usize] = Entry::Object(class);
    let outer = fields_of(this).current;
    fields_of(this).current = uid;
    match () {
        () if is_array(object) => {
            let mut items: Buffer<Id> = Buffer::new();
            for index in 0..super::array::count_of(object) {
                items.push(super::array::item(object, index));
            }
            let value = add_uids(this, items.as_slice());
            add_field(this, uid, b"NS.objects", value);
            items.free();
        }
        () if is_set(object) => {
            let mut items = super::set::objects_of(object);
            let value = add_uids(this, items.as_slice());
            add_field(this, uid, b"NS.objects", value);
            items.free();
        }
        () if is_dictionary(object) => {
            let (mut keys, mut values): (Buffer<Id>, Buffer<Id>) = (Buffer::new(), Buffer::new());
            for index in 0..super::dictionary::count_of(object) {
                keys.push(super::dictionary::key(object, index));
                values.push(super::dictionary::value(object, index));
            }
            let keys_value = add_uids(this, keys.as_slice());
            let values_value = add_uids(this, values.as_slice());
            add_field(this, uid, b"NS.keys", keys_value);
            add_field(this, uid, b"NS.objects", values_value);
            keys.free();
            values.free();
        }
        () if is_date(object) => {
            let seconds = super::date::seconds_of(object);
            add_field(this, uid, b"NS.time", Value::Real(seconds));
        }
        () if root::class_name(object) == b"NSNull" => {}
        () => {
            send1(object, sel!("encodeWithCoder:"), this as usize);
        }
    }
    fields_of(this).current = outer;
    uid
}

fn key_of(key: Id) -> Buffer<u8> {
    let mut bytes = Buffer::new();
    if !key.is_null() {
        super::string::text_of(key).write_utf8(&mut |part: &[u8]| {
            for &byte in part {
                bytes.push(byte);
            }
        });
    }
    bytes
}

fn encode_value(this: Id, key: Id, value: Value) {
    let current = fields_of(this).current;
    let entry = match current {
        NO_ENTRY => return,
        entry => entry,
    };
    let mut bytes = key_of(key);
    add_field(this, entry, bytes.as_slice(), value);
    bytes.free();
}

extern "C" fn encode_object_for_key(this: Id, _: Sel, object: Id, key: Id) {
    if fields_of(this).current == NO_ENTRY {
        let mut bytes = key_of(key);
        if bytes.as_slice() == b"root" {
            fields_of(this).root = encode(this, object);
        }
        bytes.free();
        return;
    }
    let uid = encode(this, object);
    encode_value(this, key, Value::Uid(uid));
}

extern "C" fn encode_integer_for_key(this: Id, _: Sel, value: i64, key: Id) {
    encode_value(this, key, Value::Integer(value));
}

extern "C" fn encode_int_for_key(this: Id, _: Sel, value: i32, key: Id) {
    encode_value(this, key, Value::Integer(i64::from(value)));
}

extern "C" fn encode_bool_for_key(this: Id, _: Sel, value: u8, key: Id) {
    encode_value(this, key, Value::Boolean(value != 0));
}

extern "C" fn encode_double_for_key(this: Id, _: Sel, value: f64, key: Id) {
    encode_value(this, key, Value::Real(value));
}

extern "C" fn encode_float_for_key(this: Id, _: Sel, value: f32, key: Id) {
    encode_value(this, key, Value::Real(f64::from(value)));
}

struct Writer {
    out: Buffer<u8>,
}

impl Writer {
    fn put(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.out.push(byte);
        }
    }

    fn escaped(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'&' => self.put(b"&amp;"),
                b'<' => self.put(b"&lt;"),
                b'>' => self.put(b"&gt;"),
                byte => self.put(&[byte]),
            }
        }
    }

    fn integer(&mut self, value: i64) {
        let mut digits = [0u8; 20];
        let mut length = 0;
        let mut rest = value.unsigned_abs();
        loop {
            digits[length] = b'0' + (rest % 10) as u8;
            length += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.put(b"<integer>");
        if value < 0 {
            self.put(b"-");
        }
        for &digit in digits[..length].iter().rev() {
            self.put(&[digit]);
        }
        self.put(b"</integer>");
    }

    fn uid(&mut self, uid: u32) {
        self.put(b"<dict><key>CF$UID</key>");
        self.integer(i64::from(uid));
        self.put(b"</dict>");
    }

    fn base64(&mut self, bytes: &[u8]) {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in bytes.chunks(3) {
            let word = chunk.iter().enumerate().fold(0u32, |word, (at, &byte)| {
                word | u32::from(byte) << (16 - 8 * at)
            });
            for at in 0..4 {
                match at <= chunk.len() {
                    true => self.put(&[ALPHABET[(word >> (18 - 6 * at) & 0x3f) as usize]]),
                    false => self.put(b"="),
                }
            }
        }
    }

    fn value(&mut self, this: Id, value: Value) {
        match value {
            Value::Uid(uid) => self.uid(uid),
            Value::Integer(value) => self.integer(value),
            Value::Real(value) => {
                self.put(b"<real>");
                super::number::exact_decimal(value, &mut |bytes| self.put(bytes));
                self.put(b"</real>");
            }
            Value::Boolean(true) => self.put(b"<true/>"),
            Value::Boolean(false) => self.put(b"<false/>"),
            Value::Text(start, length) => {
                self.put(b"<string>");
                self.escaped(text(this, (start, length)));
                self.put(b"</string>");
            }
            Value::Data(start, length) => {
                self.put(b"<data>");
                self.base64(text(this, (start, length)));
                self.put(b"</data>");
            }
            Value::Uids(start, count) => {
                self.put(b"<array>");
                let uids = fields_of(this).uids.as_slice();
                for &uid in &uids[start as usize..(start + count) as usize] {
                    self.uid(uid);
                }
                self.put(b"</array>");
            }
        }
    }

    fn entry(&mut self, this: Id, index: u32, entry: Entry) {
        match entry {
            Entry::Null => self.put(b"<string>$null</string>"),
            Entry::Plain(value) => self.value(this, value),
            Entry::Class(start, length) => {
                self.put(b"<dict><key>$classname</key><string>");
                self.escaped(text(this, (start, length)));
                self.put(b"</string><key>$classes</key><array><string>");
                self.escaped(text(this, (start, length)));
                self.put(b"</string><string>NSObject</string></array></dict>");
            }
            Entry::Object(class) => {
                self.put(b"<dict><key>$class</key>");
                self.uid(class);
                for field in fields_of(this).fields.as_slice() {
                    if field.entry == index {
                        self.put(b"<key>");
                        self.escaped(text(this, field.key));
                        self.put(b"</key>");
                        self.value(this, field.value);
                    }
                }
                self.put(b"</dict>");
            }
        }
        self.put(b"\n");
    }
}

fn finish(this: Id) -> Id {
    let archiver = fields_of(this);
    if !archiver.finished.is_null() {
        return archiver.finished;
    }
    let mut writer = Writer { out: Buffer::new() };
    writer.put(HEADER);
    writer.put(b"<key>$archiver</key><string>NSKeyedArchiver</string>\n");
    writer.put(b"<key>$objects</key><array>\n");
    let count = fields_of(this).entries.len();
    for index in 0..count {
        let entry = fields_of(this).entries.as_slice()[index];
        writer.entry(this, index as u32, entry);
    }
    writer.put(b"</array>\n<key>$top</key><dict><key>root</key>");
    let root = match fields_of(this).root {
        NO_ENTRY => 0,
        root => root,
    };
    writer.uid(root);
    writer.put(b"</dict>\n<key>$version</key>");
    writer.integer(ARCHIVE_VERSION);
    writer.put(b"\n</dict>\n</plist>\n");
    let data = super::data::new_data(class_pointer(&NSDATA), writer.out.as_slice());
    writer.out.free();
    fields_of(this).finished = data;
    data
}

extern "C" fn init_requiring_secure_coding(this: Id, _: Sel, secure: u8) -> Id {
    let this = set_up(this);
    if !this.is_null() {
        fields_of(this).secure = secure != 0;
    }
    this
}

extern "C" fn init(this: Id, sel: Sel) -> Id {
    init_requiring_secure_coding(this, sel, 0)
}

extern "C" fn encoded_data(this: Id, _: Sel) -> Id {
    finish(this)
}

extern "C" fn finish_encoding(this: Id, _: Sel) {
    finish(this);
}

extern "C" fn set_output_format(_: Id, _: Sel, _: usize) {}

extern "C" fn requires_secure_coding(this: Id, _: Sel) -> u8 {
    u8::from(fields_of(this).secure)
}

extern "C" fn allows_keyed_coding(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let archiver = fields_of(this);
    archiver.entries.free();
    archiver.fields.free();
    archiver.bytes.free();
    archiver.uids.free();
    archiver.seen.free();
    objc_release(archiver.finished);
    crate::objc::reference_count::dispose(this);
}

fn archive(root: Id) -> Id {
    let archiver = set_up(new_instance(class_pointer(&NSKEYEDARCHIVER)));
    if archiver.is_null() {
        return archiver;
    }
    fields_of(archiver).root = encode(archiver, root);
    let data = finish(archiver);
    fields_of(archiver).finished = core::ptr::null_mut();
    objc_release(archiver);
    autoreleased(data)
}

extern "C" fn archived_data_with_root_object(_: Id, _: Sel, root: Id) -> Id {
    archive(root)
}

extern "C" fn archived_data_secure(_: Id, _: Sel, root: Id, _: u8, error: *mut Id) -> Id {
    if !error.is_null() {
        // SAFETY: the caller's `NSError **`.
        unsafe { error.write(core::ptr::null_mut()) };
    }
    archive(root)
}

extern "C" fn archive_root_object_to_file(_: Id, _: Sel, root: Id, path: Id) -> u8 {
    let data = archive(root);
    u8::from(!send2(data, sel!("writeToFile:atomically:"), path as usize, 1).is_null())
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initRequiringSecureCoding:" => init_requiring_secure_coding,
        "encodedData" => encoded_data,
        "finishEncoding" => finish_encoding,
        "setOutputFormat:" => set_output_format,
        "requiresSecureCoding" => requires_secure_coding,
        "allowsKeyedCoding" => allows_keyed_coding,
        "encodeObject:forKey:" => encode_object_for_key,
        "encodeConditionalObject:forKey:" => encode_object_for_key,
        "encodeInteger:forKey:" => encode_integer_for_key,
        "encodeInt64:forKey:" => encode_integer_for_key,
        "encodeInt:forKey:" => encode_int_for_key,
        "encodeInt32:forKey:" => encode_int_for_key,
        "encodeBool:forKey:" => encode_bool_for_key,
        "encodeDouble:forKey:" => encode_double_for_key,
        "encodeFloat:forKey:" => encode_float_for_key,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "archivedDataWithRootObject:" => archived_data_with_root_object,
        "archivedDataWithRootObject:requiringSecureCoding:error:" => archived_data_secure,
        "archiveRootObject:toFile:" => archive_root_object_to_file,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSKeyedArchiver\0",
            class: &NSKEYEDARCHIVER,
            meta: &NSKEYEDARCHIVER_META,
            superclass: &NSCODER,
            instance_size: core::mem::size_of::<Archiver>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

//! NSUserDefaults (D381): the app's own settings, kept as an XML property list at
//! `~/Library/Preferences/<CFBundleIdentifier>.plist` (the program's name without a bundle),
//! read once when `standardUserDefaults` is first asked for and written back after every
//! change, with the values `registerDefaults:` gives answering for keys never set.

use super::bundle::{app_name, info_string};
use super::{
    NSDATA, NSFILEMANAGER, NSMUTABLEDICTIONARY, NSUSERDEFAULTS, NSUSERDEFAULTS_META, Definition,
    class_pointer, define as define_class, is_array, is_dictionary, is_number, is_string,
    new_instance, string_from_utf8,
};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::objc_retain;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, send, send1, send2, sel};
use eggos_plist::Document;

const PREFERENCES: &[u8] = b"/Library/Preferences/";
const SUFFIX: &[u8] = b".plist";
const LONGEST_PATH: usize = 1024;

#[repr(C)]
#[derive(Debug)]
struct DefaultsFields {
    isa: usize,
    values: Id,
    registered: Id,
}

static STANDARD: Exclusive<usize> = Exclusive::new(0);

fn fields(object: Id) -> &'static mut DefaultsFields {
    // SAFETY: the callers pass NSUserDefaults instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<DefaultsFields>() }
}

fn home() -> &'static [u8] {
    // SAFETY: `getenv` reads the NUL-terminated name.
    let value = unsafe { crate::libsystem::process::getenv(c"HOME".as_ptr()) };
    if value.is_null() {
        return b"/";
    }
    // SAFETY: an environment value is a NUL-terminated string that lives as long as the
    // program.
    let bytes = unsafe { crate::objc::selector::name_bytes(value.cast_const().cast()) };
    match bytes.is_empty() {
        true => b"/",
        false => bytes,
    }
}

fn with_path<R>(work: impl FnOnce(&[u8]) -> R) -> Option<R> {
    let name = info_string(b"CFBundleIdentifier").unwrap_or_else(app_name);
    let home = home();
    let mut path = [0u8; LONGEST_PATH];
    let parts = [home, PREFERENCES, name, SUFFIX];
    let length: usize = parts.iter().map(|part| part.len()).sum();
    if length > LONGEST_PATH || name.is_empty() {
        return None;
    }
    let mut at = 0;
    for part in parts {
        path[at..at + part.len()].copy_from_slice(part);
        at += part.len();
    }
    Some(work(&path[..length]))
}

fn new_mutable_dictionary() -> Id {
    send(send(class_pointer(&NSMUTABLEDICTIONARY).cast(), sel!("alloc")), sel!("init"))
}

fn load() -> Id {
    let values = new_mutable_dictionary();
    if values.is_null() {
        return values;
    }
    let data = with_path(|path| {
        send1(
            class_pointer(&NSDATA).cast(),
            sel!("dataWithContentsOfFile:"),
            string_from_utf8(path) as usize,
        )
    })
    .unwrap_or(core::ptr::null_mut());
    if data.is_null() {
        return values;
    }
    let bytes = super::data::bytes_of(data);
    let Ok(document) = Document::parse(bytes) else {
        return values;
    };
    let read = super::property_list::object_of(&document.top(), 0);
    if !read.is_null() && is_dictionary(read) {
        send1(values, sel!("addEntriesFromDictionary:"), read as usize);
    }
    if !read.is_null() {
        crate::objc::reference_count::objc_release(read);
    }
    values
}

fn save(this: Id) {
    let mut bytes: Buffer<u8> = Buffer::new();
    if super::property_list::write_xml(fields(this).values, &mut bytes) {
        let data = super::data::new_data(class_pointer(&NSDATA), bytes.as_slice());
        let _ = with_path(|path| {
            let folder_end = path.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
            let manager = send(class_pointer(&NSFILEMANAGER).cast(), sel!("defaultManager"));
            let create: extern "C" fn(Id, Sel, Id, u8, Id, *mut Id) -> u8 =
                crate::objc::message::msg_send_as();
            create(
                manager,
                sel!("createDirectoryAtPath:withIntermediateDirectories:attributes:error:"),
                string_from_utf8(&path[..folder_end]),
                1,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            send2(data, sel!("writeToFile:atomically:"), string_from_utf8(path) as usize, 1);
        });
        if !data.is_null() {
            crate::objc::reference_count::objc_release(data);
        }
    }
    bytes.free();
}

extern "C" fn standard_user_defaults(_: Id, _: Sel) -> Id {
    STANDARD.with(|standard| {
        if *standard == 0 {
            let made = new_instance(class_pointer(&NSUSERDEFAULTS));
            if !made.is_null() {
                fields(made).values = load();
                fields(made).registered = new_mutable_dictionary();
            }
            *standard = made as usize;
        }
        *standard as Id
    })
}

extern "C" fn reset_standard_user_defaults(_: Id, _: Sel) {}

fn value_for(this: Id, key: Id) -> Id {
    if key.is_null() {
        return key;
    }
    let set = send1(fields(this).values, sel!("objectForKey:"), key as usize);
    match set.is_null() {
        true => send1(fields(this).registered, sel!("objectForKey:"), key as usize),
        false => set,
    }
}

extern "C" fn object_for_key(this: Id, _: Sel, key: Id) -> Id {
    value_for(this, key)
}

extern "C" fn set_object_for_key(this: Id, _: Sel, value: Id, key: Id) {
    if key.is_null() {
        return;
    }
    match value.is_null() {
        true => send1(fields(this).values, sel!("removeObjectForKey:"), key as usize),
        false => send2(fields(this).values, sel!("setObject:forKey:"), value as usize, key as usize),
    };
    save(this);
}

extern "C" fn remove_object_for_key(this: Id, sel: Sel, key: Id) {
    set_object_for_key(this, sel, core::ptr::null_mut(), key);
}

fn number_value(this: Id, key: Id) -> Id {
    let value = value_for(this, key);
    match value.is_null() || !(is_number(value) || is_string(value)) {
        true => core::ptr::null_mut(),
        false => value,
    }
}

extern "C" fn integer_for_key(this: Id, _: Sel, key: Id) -> isize {
    let value = number_value(this, key);
    match value.is_null() {
        true => 0,
        false => send(value, sel!("integerValue")) as isize,
    }
}

extern "C" fn bool_for_key(this: Id, _: Sel, key: Id) -> u8 {
    let value = number_value(this, key);
    match value.is_null() {
        true => 0,
        false => u8::from(send(value, sel!("boolValue")) as usize & 0xff != 0),
    }
}

extern "C" fn double_for_key(this: Id, _: Sel, key: Id) -> f64 {
    let value = number_value(this, key);
    if value.is_null() {
        return 0.0;
    }
    let read: extern "C" fn(Id, Sel) -> f64 = crate::objc::message::msg_send_as();
    read(value, sel!("doubleValue"))
}

extern "C" fn float_for_key(this: Id, sel: Sel, key: Id) -> f32 {
    double_for_key(this, sel, key) as f32
}

fn set_number(this: Id, key: Id, number: Id) {
    set_object_for_key(this, sel!("setObject:forKey:"), number, key);
    if !number.is_null() {
        crate::objc::reference_count::objc_release(number);
    }
}

extern "C" fn set_integer_for_key(this: Id, _: Sel, value: isize, key: Id) {
    set_number(this, key, super::number::new_integer(value as i64));
}

extern "C" fn set_bool_for_key(this: Id, _: Sel, value: u8, key: Id) {
    let object = super::number::boolean_object(value != 0) as Id;
    set_object_for_key(this, sel!("setObject:forKey:"), object, key);
}

extern "C" fn set_double_for_key(this: Id, _: Sel, value: f64, key: Id) {
    set_number(this, key, super::number::new_double(value));
}

extern "C" fn set_float_for_key(this: Id, _: Sel, value: f32, key: Id) {
    set_number(this, key, super::number::new_double(f64::from(value)));
}

fn typed(this: Id, key: Id, check: fn(Id) -> bool) -> Id {
    let value = value_for(this, key);
    match !value.is_null() && check(value) {
        true => value,
        false => core::ptr::null_mut(),
    }
}

extern "C" fn string_for_key(this: Id, _: Sel, key: Id) -> Id {
    typed(this, key, is_string)
}

extern "C" fn array_for_key(this: Id, _: Sel, key: Id) -> Id {
    typed(this, key, is_array)
}

extern "C" fn dictionary_for_key(this: Id, _: Sel, key: Id) -> Id {
    typed(this, key, is_dictionary)
}

extern "C" fn data_for_key(this: Id, _: Sel, key: Id) -> Id {
    typed(this, key, |value| crate::objc::root::is_kind_of(value, &NSDATA))
}

extern "C" fn register_defaults(this: Id, _: Sel, registered: Id) {
    if !registered.is_null() && is_dictionary(registered) {
        send1(fields(this).registered, sel!("addEntriesFromDictionary:"), registered as usize);
    }
}

extern "C" fn synchronize(this: Id, _: Sel) -> u8 {
    save(this);
    1
}

extern "C" fn dictionary_representation(this: Id, _: Sel) -> Id {
    let all = new_mutable_dictionary();
    if all.is_null() {
        return all;
    }
    send1(all, sel!("addEntriesFromDictionary:"), fields(this).registered as usize);
    send1(all, sel!("addEntriesFromDictionary:"), fields(this).values as usize);
    super::autoreleased(all)
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    let standard = standard_user_defaults(core::ptr::null_mut(), sel!("standardUserDefaults"));
    if this != standard && !this.is_null() {
        crate::objc::reference_count::objc_release(this);
    }
    objc_retain(standard)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "objectForKey:" => object_for_key,
        "setObject:forKey:" => set_object_for_key,
        "removeObjectForKey:" => remove_object_for_key,
        "integerForKey:" => integer_for_key,
        "boolForKey:" => bool_for_key,
        "doubleForKey:" => double_for_key,
        "floatForKey:" => float_for_key,
        "stringForKey:" => string_for_key,
        "arrayForKey:" => array_for_key,
        "dictionaryForKey:" => dictionary_for_key,
        "dataForKey:" => data_for_key,
        "setInteger:forKey:" => set_integer_for_key,
        "setBool:forKey:" => set_bool_for_key,
        "setDouble:forKey:" => set_double_for_key,
        "setFloat:forKey:" => set_float_for_key,
        "setValue:forKey:" => set_object_for_key,
        "valueForKey:" => object_for_key,
        "registerDefaults:" => register_defaults,
        "synchronize" => synchronize,
        "dictionaryRepresentation" => dictionary_representation,
    ];
    let class = methods![
        "standardUserDefaults" => standard_user_defaults,
        "resetStandardUserDefaults" => reset_standard_user_defaults,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSUserDefaults\0",
            class: &NSUSERDEFAULTS,
            meta: &NSUSERDEFAULTS_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<DefaultsFields>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

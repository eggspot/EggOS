//! NSUserDefaults (D381, D728): the app's own settings, kept as an XML property list at
//! `~/Library/Preferences/<CFBundleIdentifier>.plist` (the program's name without a bundle),
//! read once when `standardUserDefaults` is first asked for and written back after every
//! change. A key is looked up through macOS's domains in order: the arguments (`-key value`
//! on the command line), the app's own values, the suites added with `addSuiteNamed:`, the
//! global domain (`~/Library/Preferences/.GlobalPreferences.plist`) and the values
//! `registerDefaults:` gives. `initWithSuiteName:` makes defaults whose own values live in the
//! suite's file. Every change posts `NSUserDefaultsDidChangeNotification`.

use super::bundle::{app_name, info_string};
use super::{
    Definition, NSDATA, NSFILEMANAGER, NSMUTABLEDICTIONARY, NSPROCESSINFO, NSURL, NSUSERDEFAULTS,
    NSUSERDEFAULTS_META, class_pointer, define as define_class, is_array, is_dictionary,
    is_number, is_string, new_instance, notification_with_name, post, string_from_utf8,
    with_utf8,
};
use crate::libsystem::Exclusive;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send, send1, send2};
use eggos_plist::Document;

const PREFERENCES: &[u8] = b"/Library/Preferences/";
const SUFFIX: &[u8] = b".plist";
const LONGEST_PATH: usize = 1024;
const GLOBAL_FILE: &[u8] = b".GlobalPreferences";
const GLOBAL_DOMAIN: &[u8] = b"NSGlobalDomain";
const ARGUMENT_DOMAIN: &[u8] = b"NSArgumentDomain";
const REGISTRATION_DOMAIN: &[u8] = b"NSRegistrationDomain";
const LONGEST_NAME: usize = 256;

#[repr(C)]
#[derive(Debug)]
struct DefaultsFields {
    isa: usize,
    values: Id,
    registered: Id,
    arguments: Id,
    global: Id,
    suites: Id,
    suite: Id,
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

fn app_domain() -> &'static [u8] {
    info_string(b"CFBundleIdentifier").unwrap_or_else(app_name)
}

fn name_of(this: Id, out: &mut [u8; LONGEST_NAME]) -> Option<usize> {
    let suite = fields(this).suite;
    if suite.is_null() {
        let name = app_domain();
        out.get_mut(..name.len())?.copy_from_slice(name);
        return Some(name.len());
    }
    text_into(suite, out)
}

fn text_into(text: Id, out: &mut [u8; LONGEST_NAME]) -> Option<usize> {
    let mut length = 0;
    let mut fits = true;
    with_utf8(text, |bytes| match out.get_mut(..bytes.len()) {
        Some(slot) => {
            slot.copy_from_slice(bytes);
            length = bytes.len();
        }
        None => fits = false,
    });
    (fits && length > 0).then_some(length)
}

fn with_path<R>(name: &[u8], work: impl FnOnce(&[u8]) -> R) -> Option<R> {
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
    send(
        send(class_pointer(&NSMUTABLEDICTIONARY).cast(), sel!("alloc")),
        sel!("init"),
    )
}

fn load(name: &[u8]) -> Id {
    let values = new_mutable_dictionary();
    if values.is_null() {
        return values;
    }
    let data = with_path(name, |path| {
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
    let mut name = [0u8; LONGEST_NAME];
    let Some(length) = name_of(this, &mut name) else {
        return;
    };
    write_domain(&name[..length], fields(this).values);
}

fn write_domain(name: &[u8], values: Id) {
    let mut bytes: Buffer<u8> = Buffer::new();
    if super::property_list::write_xml(values, &mut bytes) {
        let data = super::data::new_data(class_pointer(&NSDATA), bytes.as_slice());
        let _ = with_path(name, |path| {
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
            send2(
                data,
                sel!("writeToFile:atomically:"),
                string_from_utf8(path) as usize,
                1,
            );
        });
        if !data.is_null() {
            crate::objc::reference_count::objc_release(data);
        }
    }
    bytes.free();
}

fn argument_values() -> Id {
    let values = new_mutable_dictionary();
    if values.is_null() {
        return values;
    }
    let process = send(class_pointer(&NSPROCESSINFO).cast(), sel!("processInfo"));
    let arguments = send(process, sel!("arguments"));
    let count = match arguments.is_null() {
        true => 0,
        false => send(arguments, sel!("count")) as usize,
    };
    let mut at = 1;
    while at + 1 < count {
        let flag = send1(arguments, sel!("objectAtIndex:"), at);
        let mut bytes = [0u8; LONGEST_NAME];
        let key = text_into(flag, &mut bytes)
            .filter(|&length| length > 1 && bytes[0] == b'-' && bytes[1] != b'-');
        match key {
            Some(length) => {
                let value = send1(arguments, sel!("objectAtIndex:"), at + 1);
                let name = string_from_utf8(&bytes[1..length]);
                send2(values, sel!("setObject:forKey:"), value as usize, name as usize);
                at += 2;
            }
            None => at += 1,
        }
    }
    values
}

fn set_up(this: Id, suite: Id, values: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let defaults = fields(this);
    defaults.values = values;
    defaults.registered = new_mutable_dictionary();
    defaults.arguments = argument_values();
    defaults.global = load(GLOBAL_FILE);
    defaults.suites = new_mutable_dictionary();
    defaults.suite = match suite.is_null() {
        true => suite,
        false => send(suite, sel!("copy")),
    };
    this
}

extern "C" fn standard_user_defaults(_: Id, _: Sel) -> Id {
    STANDARD.with(|standard| {
        if *standard == 0 {
            let made = new_instance(class_pointer(&NSUSERDEFAULTS));
            *standard = set_up(made, core::ptr::null_mut(), load(app_domain())) as usize;
        }
        *standard as Id
    })
}

extern "C" fn reset_standard_user_defaults(_: Id, _: Sel) {}

fn lookup(dictionary: Id, key: Id) -> Id {
    match dictionary.is_null() {
        true => dictionary,
        false => send1(dictionary, sel!("objectForKey:"), key as usize),
    }
}

fn value_for(this: Id, key: Id) -> Id {
    if key.is_null() {
        return key;
    }
    let defaults = fields(this);
    for domain in [defaults.arguments, defaults.values] {
        let found = lookup(domain, key);
        if !found.is_null() {
            return found;
        }
    }
    let suites = defaults.suites;
    let count = match suites.is_null() {
        true => 0,
        false => super::dictionary::count_of(suites),
    };
    for index in 0..count {
        let found = lookup(super::dictionary::value(suites, index), key);
        if !found.is_null() {
            return found;
        }
    }
    for domain in [defaults.global, defaults.registered] {
        let found = lookup(domain, key);
        if !found.is_null() {
            return found;
        }
    }
    core::ptr::null_mut()
}

fn changed(this: Id) {
    let name = string_from_utf8(b"NSUserDefaultsDidChangeNotification");
    post(notification_with_name(name, this));
}

extern "C" fn object_for_key(this: Id, _: Sel, key: Id) -> Id {
    value_for(this, key)
}

extern "C" fn set_object_for_key(this: Id, _: Sel, value: Id, key: Id) {
    if key.is_null() {
        return;
    }
    match value.is_null() {
        true => send1(
            fields(this).values,
            sel!("removeObjectForKey:"),
            key as usize,
        ),
        false => send2(
            fields(this).values,
            sel!("setObject:forKey:"),
            value as usize,
            key as usize,
        ),
    };
    save(this);
    changed(this);
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
    typed(this, key, |value| {
        crate::objc::root::is_kind_of(value, &NSDATA)
    })
}

extern "C" fn register_defaults(this: Id, _: Sel, registered: Id) {
    if !registered.is_null() && is_dictionary(registered) {
        send1(
            fields(this).registered,
            sel!("addEntriesFromDictionary:"),
            registered as usize,
        );
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
    send1(
        all,
        sel!("addEntriesFromDictionary:"),
        fields(this).registered as usize,
    );
    send1(
        all,
        sel!("addEntriesFromDictionary:"),
        fields(this).values as usize,
    );
    super::autoreleased(all)
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    let standard = standard_user_defaults(core::ptr::null_mut(), sel!("standardUserDefaults"));
    if this != standard && !this.is_null() {
        crate::objc::reference_count::objc_release(this);
    }
    objc_retain(standard)
}

fn names_app_or_global(name: Id) -> bool {
    let mut bytes = [0u8; LONGEST_NAME];
    let Some(length) = text_into(name, &mut bytes) else {
        return true;
    };
    let name = &bytes[..length];
    name == app_domain() || name == GLOBAL_DOMAIN
}

extern "C" fn init_with_suite_name(this: Id, sel: Sel, suite: Id) -> Id {
    if suite.is_null() {
        return init(this, sel);
    }
    let mut bytes = [0u8; LONGEST_NAME];
    let Some(length) = text_into(suite, &mut bytes) else {
        objc_release(this);
        return core::ptr::null_mut();
    };
    if names_app_or_global(suite) {
        objc_release(this);
        return core::ptr::null_mut();
    }
    set_up(this, suite, load(&bytes[..length]))
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let defaults = fields(this);
    for each in [
        defaults.values,
        defaults.registered,
        defaults.arguments,
        defaults.global,
        defaults.suites,
        defaults.suite,
    ] {
        if !each.is_null() {
            objc_release(each);
        }
    }
    dispose(this);
}

extern "C" fn add_suite_named(this: Id, _: Sel, suite: Id) {
    let mut bytes = [0u8; LONGEST_NAME];
    let Some(length) = text_into(suite, &mut bytes) else {
        return;
    };
    if names_app_or_global(suite) {
        return;
    }
    let values = load(&bytes[..length]);
    if values.is_null() {
        return;
    }
    send2(
        fields(this).suites,
        sel!("setObject:forKey:"),
        values as usize,
        suite as usize,
    );
    objc_release(values);
}

extern "C" fn remove_suite_named(this: Id, _: Sel, suite: Id) {
    if !suite.is_null() {
        send1(fields(this).suites, sel!("removeObjectForKey:"), suite as usize);
    }
}

extern "C" fn persistent_domain_for_name(this: Id, _: Sel, name: Id) -> Id {
    let mut bytes = [0u8; LONGEST_NAME];
    let Some(length) = text_into(name, &mut bytes) else {
        return core::ptr::null_mut();
    };
    let name = &bytes[..length];
    let mut own = [0u8; LONGEST_NAME];
    let own_length = name_of(this, &mut own).unwrap_or(0);
    let domain = match name {
        GLOBAL_DOMAIN => objc_retain(fields(this).global),
        _ if name == &own[..own_length] => objc_retain(fields(this).values),
        _ => load(name),
    };
    let copy = send(domain, sel!("copy"));
    objc_release(domain);
    super::autoreleased(copy)
}

extern "C" fn set_persistent_domain(this: Id, _: Sel, domain: Id, name: Id) {
    let mut bytes = [0u8; LONGEST_NAME];
    let Some(length) = text_into(name, &mut bytes) else {
        return;
    };
    let file: &[u8] = match &bytes[..length] {
        GLOBAL_DOMAIN => GLOBAL_FILE,
        name => name,
    };
    let values = new_mutable_dictionary();
    if values.is_null() {
        return;
    }
    if !domain.is_null() && is_dictionary(domain) {
        send1(values, sel!("addEntriesFromDictionary:"), domain as usize);
    }
    write_domain(file, values);
    let mut own = [0u8; LONGEST_NAME];
    let own_length = name_of(this, &mut own).unwrap_or(0);
    let target = match &bytes[..length] {
        GLOBAL_DOMAIN => Some(&mut fields(this).global),
        name if name == &own[..own_length] => Some(&mut fields(this).values),
        _ => None,
    };
    match target {
        Some(slot) => {
            let old = core::mem::replace(slot, values);
            if !old.is_null() {
                objc_release(old);
            }
        }
        None => objc_release(values),
    }
    changed(this);
}

extern "C" fn remove_persistent_domain(this: Id, sel: Sel, name: Id) {
    set_persistent_domain(this, sel, core::ptr::null_mut(), name);
}

extern "C" fn volatile_domain_for_name(this: Id, _: Sel, name: Id) -> Id {
    let mut bytes = [0u8; LONGEST_NAME];
    let length = text_into(name, &mut bytes).unwrap_or(0);
    let domain = match &bytes[..length] {
        ARGUMENT_DOMAIN => fields(this).arguments,
        REGISTRATION_DOMAIN => fields(this).registered,
        _ => core::ptr::null_mut(),
    };
    match domain.is_null() {
        true => domain,
        false => super::autoreleased(send(domain, sel!("copy"))),
    }
}

extern "C" fn object_is_forced(_: Id, _: Sel, _: Id) -> u8 {
    0
}

fn expanded_path(text: Id) -> Id {
    let mut bytes = [0u8; LONGEST_PATH];
    let mut length = 0;
    let mut fits = true;
    with_utf8(text, |given| {
        let (prefix, rest): (&[u8], &[u8]) = match given.first() {
            Some(b'~') if given.len() == 1 || given[1] == b'/' => (home(), &given[1..]),
            _ => (&[], given),
        };
        let total = prefix.len() + rest.len();
        match total <= LONGEST_PATH {
            true => {
                bytes[..prefix.len()].copy_from_slice(prefix);
                bytes[prefix.len()..total].copy_from_slice(rest);
                length = total;
            }
            false => fits = false,
        }
    });
    match fits {
        true => string_from_utf8(&bytes[..length]),
        false => core::ptr::null_mut(),
    }
}

extern "C" fn url_for_key(this: Id, _: Sel, key: Id) -> Id {
    let value = value_for(this, key);
    if value.is_null() || !is_string(value) {
        return core::ptr::null_mut();
    }
    let remote = with_utf8(value, |bytes| bytes.windows(3).any(|part| part == b"://"));
    let url_class: Id = class_pointer(&NSURL).cast();
    if remote {
        return send1(url_class, sel!("URLWithString:"), value as usize);
    }
    let path = expanded_path(value);
    if path.is_null() {
        return path;
    }
    send1(url_class, sel!("fileURLWithPath:"), path as usize)
}

extern "C" fn set_url_for_key(this: Id, sel: Sel, url: Id, key: Id) {
    let text = match url.is_null() {
        true => url,
        false if send(url, sel!("isFileURL")) as usize & 0xff != 0 => send(url, sel!("path")),
        false => send(url, sel!("absoluteString")),
    };
    set_object_for_key(this, sel, text, key);
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
        "initWithSuiteName:" => init_with_suite_name,
        "dealloc" => dealloc,
        "addSuiteNamed:" => add_suite_named,
        "removeSuiteNamed:" => remove_suite_named,
        "persistentDomainForName:" => persistent_domain_for_name,
        "setPersistentDomain:forName:" => set_persistent_domain,
        "removePersistentDomainForName:" => remove_persistent_domain,
        "volatileDomainForName:" => volatile_domain_for_name,
        "objectIsForcedForKey:" => object_is_forced,
        "URLForKey:" => url_for_key,
        "setURL:forKey:" => set_url_for_key,
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

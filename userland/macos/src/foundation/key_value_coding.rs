//! Key-value coding (D741): NSObject's `valueForKey:` and `setValue:forKey:` find a key's
//! accessor (`get<Key>`, `<key>`, `is<Key>`, `_<key>`; `set<Key>:`, `_set<Key>:`) or, when the
//! class allows it, its instance variable (`_<key>`, `_is<Key>`, `<key>`, `is<Key>`), boxing
//! and unboxing numbers and booleans by the method's or ivar's type encoding, so Swift's
//! `@objc` properties and Objective-C properties answer by name; key paths, undefined keys
//! (`NSUndefinedKeyException`), nil for scalars, `dictionaryWithValuesForKeys:` and
//! `setValuesForKeysWithDictionary:`. Dictionaries answer keys with their objects (an `@` key
//! goes to NSObject's own), mutable dictionaries set and remove them, and arrays answer with
//! the array of their elements' values or `@count`.

use super::exception::raise_named;
use super::number::{boolean_object, new_double, new_integer};
use super::{
    NSARRAY, NSDICTIONARY, NSMUTABLEARRAY, NSMUTABLEDICTIONARY, autoreleased, class_pointer,
    string_from_utf8, with_utf8,
};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::type_encoding::{self, Kind};
use crate::objc::{
    Runtime, class, class_named, existing_selector, message, methods, sel, send, send1, send2,
};

const LONGEST_NAME: usize = 256;
const GETTERS: [(&[u8], bool); 4] = [(b"get", true), (b"", false), (b"is", true), (b"_", false)];
const SETTERS: [&[u8]; 2] = [b"set", b"_set"];
const IVARS: [(&[u8], bool); 4] = [(b"_", false), (b"_is", true), (b"", false), (b"is", true)];

struct Name {
    bytes: [u8; LONGEST_NAME],
    length: usize,
}

impl Name {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn compose(prefix: &[u8], key: &[u8], capitalized: bool, suffix: &[u8]) -> Option<Name> {
    let length = prefix.len() + key.len() + suffix.len();
    if key.is_empty() || length > LONGEST_NAME {
        return None;
    }
    let mut name = Name {
        bytes: [0; LONGEST_NAME],
        length,
    };
    name.bytes[..prefix.len()].copy_from_slice(prefix);
    name.bytes[prefix.len()..prefix.len() + key.len()].copy_from_slice(key);
    if capitalized {
        name.bytes[prefix.len()] = key[0].to_ascii_uppercase();
    }
    name.bytes[prefix.len() + key.len()..length].copy_from_slice(suffix);
    Some(name)
}

fn accessor(class: *mut ClassObject, name: &Name) -> Option<Sel> {
    let sel = existing_selector(name.as_bytes())?;
    class::find(class, sel).map(|_| sel)
}

fn encoding_of(class: *mut ClassObject, sel: Sel, index: usize) -> &'static [u8] {
    class::method_types(class, sel)
        .and_then(|types| type_encoding::argument(types, index))
        .unwrap_or(b"@")
}

fn letter(encoding: &[u8]) -> u8 {
    encoding
        .iter()
        .copied()
        .find(|byte| !b"rnNoORVA".contains(byte))
        .unwrap_or(b'@')
}

fn number_of_integer(value: i64) -> Id {
    autoreleased(new_integer(value))
}

fn number_of_double(value: f64) -> Id {
    autoreleased(new_double(value))
}

fn get_scalar<T: Copy>(this: Id, sel: Sel) -> T {
    let function: extern "C" fn(Id, Sel) -> T = message::msg_send_as();
    function(this, sel)
}

fn call_getter(this: Id, sel: Sel, encoding: &[u8]) -> Option<Id> {
    Some(match letter(encoding) {
        b'@' | b'#' => send(this, sel),
        b'B' => boolean_object(get_scalar::<u8>(this, sel) != 0) as Id,
        b'c' => number_of_integer(i64::from(get_scalar::<i8>(this, sel))),
        b's' => number_of_integer(i64::from(get_scalar::<i16>(this, sel))),
        b'i' => number_of_integer(i64::from(get_scalar::<i32>(this, sel))),
        b'l' | b'q' => number_of_integer(get_scalar::<i64>(this, sel)),
        b'C' => number_of_integer(i64::from(get_scalar::<u8>(this, sel))),
        b'S' => number_of_integer(i64::from(get_scalar::<u16>(this, sel))),
        b'I' => number_of_integer(i64::from(get_scalar::<u32>(this, sel))),
        b'L' | b'Q' => number_of_integer(get_scalar::<u64>(this, sel) as i64),
        b'f' => number_of_double(f64::from(get_scalar::<f32>(this, sel))),
        b'd' => number_of_double(get_scalar::<f64>(this, sel)),
        _ => return None,
    })
}

fn read_ivar(this: Id, offset: usize, encoding: &[u8]) -> Option<Id> {
    let at = (this as usize + offset) as *const u8;
    // SAFETY: the ivar at `offset` of the object, of the type its encoding names.
    unsafe {
        Some(match letter(encoding) {
            b'@' | b'#' => at.cast::<Id>().read_unaligned(),
            b'B' => boolean_object(at.read() != 0) as Id,
            b'c' => number_of_integer(i64::from(at.cast::<i8>().read())),
            b's' => number_of_integer(i64::from(at.cast::<i16>().read_unaligned())),
            b'i' => number_of_integer(i64::from(at.cast::<i32>().read_unaligned())),
            b'l' | b'q' => number_of_integer(at.cast::<i64>().read_unaligned()),
            b'C' => number_of_integer(i64::from(at.read())),
            b'S' => number_of_integer(i64::from(at.cast::<u16>().read_unaligned())),
            b'I' => number_of_integer(i64::from(at.cast::<u32>().read_unaligned())),
            b'L' | b'Q' => number_of_integer(at.cast::<u64>().read_unaligned() as i64),
            b'f' => number_of_double(f64::from(at.cast::<f32>().read_unaligned())),
            b'd' => number_of_double(at.cast::<f64>().read_unaligned()),
            _ => return None,
        })
    }
}

fn reads_ivars(class: *mut ClassObject) -> bool {
    let function: extern "C" fn(Id, Sel) -> u8 = message::msg_send_as();
    function(class.cast(), sel!("accessInstanceVariablesDirectly")) != 0
}

fn find_ivar(class: *mut ClassObject, key: &[u8]) -> Option<(usize, &'static [u8])> {
    if !reads_ivars(class) {
        return None;
    }
    IVARS.iter().find_map(|&(prefix, capitalized)| {
        let name = compose(prefix, key, capitalized, b"")?;
        class::ivar_named(class, name.as_bytes())
    })
}

fn value_for(this: Id, key: Id, bytes: &[u8]) -> Id {
    let class = class::class_of(this);
    for (prefix, capitalized) in GETTERS {
        let Some(name) = compose(prefix, bytes, capitalized, b"") else {
            continue;
        };
        if let Some(sel) = accessor(class, &name) {
            let encoding = encoding_of(class, sel, 0);
            if let Some(value) = call_getter(this, sel, encoding) {
                return value;
            }
        }
    }
    if let Some((offset, encoding)) = find_ivar(class, bytes)
        && let Some(value) = read_ivar(this, offset, encoding)
    {
        return value;
    }
    send1(this, sel!("valueForUndefinedKey:"), key as usize)
}

extern "C" fn value_for_key(this: Id, _: Sel, key: Id) -> Id {
    if key.is_null() {
        return send1(this, sel!("valueForUndefinedKey:"), 0);
    }
    with_utf8(key, |bytes| value_for(this, key, bytes))
}

fn signed_of(value: Id) -> i64 {
    get_scalar::<i64>(value, sel!("longLongValue"))
}

fn call_setter(this: Id, sel: Sel, value: Id, encoding: &[u8]) -> bool {
    let kind = type_encoding::kind_of(encoding);
    if kind == Kind::Object {
        send1(this, sel, value as usize);
        return true;
    }
    match letter(encoding) {
        b'B' => {
            let flag = get_scalar::<u8>(value, sel!("boolValue"));
            let function: extern "C" fn(Id, Sel, u8) = message::msg_send_as();
            function(this, sel, u8::from(flag != 0));
        }
        b'f' => {
            let number = get_scalar::<f32>(value, sel!("floatValue"));
            let function: extern "C" fn(Id, Sel, f32) = message::msg_send_as();
            function(this, sel, number);
        }
        b'd' => {
            let number = get_scalar::<f64>(value, sel!("doubleValue"));
            let function: extern "C" fn(Id, Sel, f64) = message::msg_send_as();
            function(this, sel, number);
        }
        code @ (b'c' | b's' | b'i' | b'l' | b'q' | b'C' | b'S' | b'I' | b'L' | b'Q') => {
            let number = narrowed(signed_of(value), code);
            let function: extern "C" fn(Id, Sel, i64) = message::msg_send_as();
            function(this, sel, number);
        }
        _ => return false,
    }
    true
}

fn narrowed(value: i64, code: u8) -> i64 {
    match code {
        b'c' => i64::from(value as i8),
        b's' => i64::from(value as i16),
        b'i' => i64::from(value as i32),
        b'C' => i64::from(value as u8),
        b'S' => i64::from(value as u16),
        b'I' => i64::from(value as u32),
        _ => value,
    }
}

fn write_ivar(this: Id, offset: usize, encoding: &[u8], value: Id) -> bool {
    let at = (this as usize + offset) as *mut u8;
    let code = letter(encoding);
    // SAFETY: the object's ivar at `offset`, of its encoding's type; object ivars are strong.
    unsafe {
        match code {
            b'@' | b'#' => {
                let old = at.cast::<Id>().read_unaligned();
                at.cast::<Id>().write_unaligned(objc_retain(value));
                objc_release(old);
            }
            b'B' => at.write(u8::from(get_scalar::<u8>(value, sel!("boolValue")) != 0)),
            b'c' | b'C' => at.write(signed_of(value) as u8),
            b's' | b'S' => at.cast::<u16>().write_unaligned(signed_of(value) as u16),
            b'i' | b'I' => at.cast::<u32>().write_unaligned(signed_of(value) as u32),
            b'l' | b'q' | b'L' | b'Q' => at.cast::<i64>().write_unaligned(signed_of(value)),
            b'f' => at
                .cast::<f32>()
                .write_unaligned(get_scalar::<f32>(value, sel!("floatValue"))),
            b'd' => at
                .cast::<f64>()
                .write_unaligned(get_scalar::<f64>(value, sel!("doubleValue"))),
            _ => return false,
        }
    }
    true
}

fn set_value(this: Id, value: Id, key: Id, bytes: &[u8]) {
    let class = class::class_of(this);
    for prefix in SETTERS {
        let Some(name) = compose(prefix, bytes, true, b":") else {
            continue;
        };
        if let Some(sel) = accessor(class, &name) {
            let encoding = encoding_of(class, sel, 3);
            if value.is_null() && type_encoding::kind_of(encoding) != Kind::Object {
                send1(this, sel!("setNilValueForKey:"), key as usize);
                return;
            }
            if call_setter(this, sel, value, encoding) {
                return;
            }
        }
    }
    if let Some((offset, encoding)) = find_ivar(class, bytes) {
        if value.is_null() && type_encoding::kind_of(encoding) != Kind::Object {
            send1(this, sel!("setNilValueForKey:"), key as usize);
            return;
        }
        if write_ivar(this, offset, encoding, value) {
            return;
        }
    }
    send2(this, sel!("setValue:forUndefinedKey:"), value as usize, key as usize);
}

extern "C" fn set_value_for_key(this: Id, _: Sel, value: Id, key: Id) {
    if key.is_null() {
        send2(this, sel!("setValue:forUndefinedKey:"), value as usize, 0);
        return;
    }
    with_utf8(key, |bytes| set_value(this, value, key, bytes));
}

fn split_path(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let dot = path.iter().position(|&byte| byte == b'.')?;
    Some((&path[..dot], &path[dot + 1..]))
}

extern "C" fn value_for_key_path(this: Id, _: Sel, path: Id) -> Id {
    if path.is_null() {
        return send1(this, sel!("valueForKey:"), 0);
    }
    with_utf8(path, |bytes| match split_path(bytes) {
        None => send1(this, sel!("valueForKey:"), path as usize),
        Some((first, rest)) => {
            let next = send1(this, sel!("valueForKey:"), string_from_utf8(first) as usize);
            match next.is_null() {
                true => core::ptr::null_mut(),
                false => send1(next, sel!("valueForKeyPath:"), string_from_utf8(rest) as usize),
            }
        }
    })
}

extern "C" fn set_value_for_key_path(this: Id, _: Sel, value: Id, path: Id) {
    if path.is_null() {
        send2(this, sel!("setValue:forKey:"), value as usize, 0);
        return;
    }
    with_utf8(path, |bytes| match split_path(bytes) {
        None => {
            send2(this, sel!("setValue:forKey:"), value as usize, path as usize);
        }
        Some((first, rest)) => {
            let next = send1(this, sel!("valueForKey:"), string_from_utf8(first) as usize);
            if !next.is_null() {
                let rest = string_from_utf8(rest) as usize;
                send2(next, sel!("setValue:forKeyPath:"), value as usize, rest);
            }
        }
    });
}

fn class_name(this: Id) -> &'static [u8] {
    class::record_of(class::class_of(this)).map_or(&b"?"[..], |record| record.name())
}

fn key_copy(key: Id) -> Name {
    let mut name = Name {
        bytes: [0; LONGEST_NAME],
        length: 0,
    };
    let mut keep = |bytes: &[u8]| {
        name.length = bytes.len().min(LONGEST_NAME);
        name.bytes[..name.length].copy_from_slice(&bytes[..name.length]);
    };
    match key.is_null() {
        true => keep(b"(null)"),
        false => with_utf8(key, keep),
    }
    name
}

extern "C" fn value_for_undefined_key(this: Id, _: Sel, key: Id) -> Id {
    let key = key_copy(key);
    raise_named(
        b"NSUndefinedKeyException",
        &[
            b"[<",
            class_name(this),
            b"> valueForUndefinedKey:]: this class is not key value coding-compliant for ",
            b"the key ",
            key.as_bytes(),
            b".",
        ],
    )
}

extern "C" fn set_value_for_undefined_key(this: Id, _: Sel, _: Id, key: Id) {
    let key = key_copy(key);
    raise_named(
        b"NSUndefinedKeyException",
        &[
            b"[<",
            class_name(this),
            b"> setValue:forUndefinedKey:]: this class is not key value coding-compliant ",
            b"for the key ",
            key.as_bytes(),
            b".",
        ],
    )
}

extern "C" fn set_nil_value_for_key(this: Id, _: Sel, key: Id) {
    let key = key_copy(key);
    raise_named(
        b"NSInvalidArgumentException",
        &[
            b"[<",
            class_name(this),
            b"> setNilValueForKey]: could not set nil as the value for the key ",
            key.as_bytes(),
            b".",
        ],
    )
}

fn null_object() -> Id {
    let class = class_named(b"NSNull");
    match class.is_null() {
        true => core::ptr::null_mut(),
        false => send(class.cast(), sel!("null")),
    }
}

fn is_null_object(value: Id) -> bool {
    !value.is_null() && value == null_object()
}

extern "C" fn dictionary_with_values_for_keys(this: Id, _: Sel, keys: Id) -> Id {
    let made = send(class_pointer(&NSMUTABLEDICTIONARY).cast(), sel!("dictionary"));
    let count = match keys.is_null() {
        true => 0,
        false => send(keys, sel!("count")) as usize,
    };
    for index in 0..count {
        let key = send1(keys, sel!("objectAtIndex:"), index);
        let mut value = send1(this, sel!("valueForKey:"), key as usize);
        if value.is_null() {
            value = null_object();
        }
        if !value.is_null() {
            send2(made, sel!("setObject:forKey:"), value as usize, key as usize);
        }
    }
    made
}

extern "C" fn set_values_for_keys(this: Id, _: Sel, values: Id) {
    if values.is_null() {
        return;
    }
    let keys = send(values, sel!("allKeys"));
    for index in 0..send(keys, sel!("count")) as usize {
        let key = send1(keys, sel!("objectAtIndex:"), index);
        let mut value = send1(values, sel!("objectForKey:"), key as usize);
        if is_null_object(value) {
            value = core::ptr::null_mut();
        }
        send2(this, sel!("setValue:forKey:"), value as usize, key as usize);
    }
}

extern "C" fn access_ivars_directly(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn dictionary_value_for_key(this: Id, _: Sel, key: Id) -> Id {
    if key.is_null() {
        return core::ptr::null_mut();
    }
    with_utf8(key, |bytes| match bytes.strip_prefix(b"@") {
        Some(own) => value_for(this, key, own),
        None => send1(this, sel!("objectForKey:"), key as usize),
    })
}

extern "C" fn dictionary_set_value_for_key(this: Id, _: Sel, value: Id, key: Id) {
    match value.is_null() {
        true => {
            send1(this, sel!("removeObjectForKey:"), key as usize);
        }
        false => {
            send2(this, sel!("setObject:forKey:"), value as usize, key as usize);
        }
    }
}

extern "C" fn array_value_for_key(this: Id, _: Sel, key: Id) -> Id {
    let count = send(this, sel!("count")) as usize;
    if !key.is_null() && with_utf8(key, |bytes| bytes == b"@count") {
        return number_of_integer(count as i64);
    }
    let made = send1(
        class_pointer(&NSMUTABLEARRAY).cast(),
        sel!("arrayWithCapacity:"),
        count,
    );
    for index in 0..count {
        let item = send1(this, sel!("objectAtIndex:"), index);
        let mut value = send1(item, sel!("valueForKey:"), key as usize);
        if value.is_null() {
            value = null_object();
        }
        if !value.is_null() {
            send1(made, sel!("addObject:"), value as usize);
        }
    }
    made
}

extern "C" fn array_set_value_for_key(this: Id, _: Sel, value: Id, key: Id) {
    for index in 0..send(this, sel!("count")) as usize {
        let item = send1(this, sel!("objectAtIndex:"), index);
        send2(item, sel!("setValue:forKey:"), value as usize, key as usize);
    }
}

/// Adds key-value coding to NSObject, NSDictionary, NSMutableDictionary and NSArray.
pub fn define(runtime: &mut Runtime) -> bool {
    let object = methods![
        "valueForKey:" => value_for_key,
        "setValue:forKey:" => set_value_for_key,
        "valueForKeyPath:" => value_for_key_path,
        "setValue:forKeyPath:" => set_value_for_key_path,
        "valueForUndefinedKey:" => value_for_undefined_key,
        "setValue:forUndefinedKey:" => set_value_for_undefined_key,
        "setNilValueForKey:" => set_nil_value_for_key,
        "dictionaryWithValuesForKeys:" => dictionary_with_values_for_keys,
        "setValuesForKeysWithDictionary:" => set_values_for_keys,
    ];
    let object_class = methods![
        "accessInstanceVariablesDirectly" => access_ivars_directly,
    ];
    let dictionary = methods![
        "valueForKey:" => dictionary_value_for_key,
    ];
    let mutable_dictionary = methods![
        "setValue:forKey:" => dictionary_set_value_for_key,
    ];
    let array = methods![
        "valueForKey:" => array_value_for_key,
        "setValue:forKey:" => array_set_value_for_key,
    ];
    class::extend(runtime, &NSOBJECT, &object, &object_class)
        && class::extend(runtime, &NSDICTIONARY, &dictionary, &[])
        && class::extend(runtime, &NSMUTABLEDICTIONARY, &mutable_dictionary, &[])
        && class::extend(runtime, &NSARRAY, &array, &[])
}

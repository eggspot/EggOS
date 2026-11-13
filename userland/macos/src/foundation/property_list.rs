//! Property-list objects to and from property lists (D381): a parsed list (XML or binary,
//! `eggos-plist`) made into Foundation's immutable strings, numbers, booleans, dates, data,
//! arrays and dictionaries, and those objects written back as an XML property list, as
//! `NSUserDefaults` keeps them.

use super::array::new_array;
use super::dictionary::new_dictionary;
use super::number::{Exact, exact_value, new_double, new_integer};
use super::{
    BOOLEAN, NSARRAY, NSDATA, NSDATE, NSDICTIONARY, class_pointer, is_array, is_date,
    is_dictionary, is_number, is_string, with_utf8,
};
use crate::objc::abi::Id;
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::objc_release;
use crate::objc::{root, sel, send};
use eggos_plist::{Kind, Object};

const MAX_DEPTH: usize = 64;
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const HEADER: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";
const FOOTER: &[u8] = b"\n</plist>\n";

fn released_after<R>(objects: &Buffer<Id>, work: impl FnOnce(&[Id]) -> R) -> R {
    let made = work(objects.as_slice());
    for &object in objects.as_slice() {
        if !object.is_null() {
            objc_release(object);
        }
    }
    made
}

/// The property list in the file at `path` (an `NSString`), XML or binary, as Foundation's
/// objects, counted once; nil when it cannot be read (a compiled storyboard's `Info.plist`,
/// D406).
pub fn object_of_file(path: Id) -> Id {
    if path.is_null() {
        return core::ptr::null_mut();
    }
    let data = crate::objc::send1(
        class_pointer(&NSDATA).cast(),
        sel!("dataWithContentsOfFile:"),
        path as usize,
    );
    if data.is_null() {
        return data;
    }
    let Ok(document) = eggos_plist::Document::parse(super::data::bytes_of(data)) else {
        return core::ptr::null_mut();
    };
    object_of(&document.top(), 0)
}

/// The object `object` stands for, counted once; nil for what a property list cannot hold.
pub fn object_of(object: &Object<'_>, depth: usize) -> Id {
    if depth > MAX_DEPTH {
        return core::ptr::null_mut();
    }
    match object.kind() {
        Some(Kind::Boolean) => {
            let value = object.boolean().unwrap_or(false);
            super::number::boolean_object(value) as Id
        }
        Some(Kind::Integer) => {
            let value = object.integer().unwrap_or(0);
            new_integer(value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
        }
        Some(Kind::Real) => new_double(object.real().unwrap_or(0.0)),
        Some(Kind::Date) => {
            super::date::new_date(class_pointer(&NSDATE), object.date().unwrap_or(0.0))
        }
        Some(Kind::String) => super::keyed_unarchiver::string_of(object),
        Some(Kind::Data) => super::keyed_unarchiver::data_of(object),
        Some(Kind::Array | Kind::Set) => {
            let mut items: Buffer<Id> = Buffer::new();
            for item in object.items().into_iter().flatten() {
                let made = object_of(&item, depth + 1);
                if !made.is_null() {
                    items.push(made);
                }
            }
            let array = released_after(&items, |items| new_array(class_pointer(&NSARRAY), items));
            items.free();
            array
        }
        Some(Kind::Dictionary) => {
            let mut keys: Buffer<Id> = Buffer::new();
            let mut values: Buffer<Id> = Buffer::new();
            for (key, value) in object.entries().into_iter().flatten() {
                let key = super::keyed_unarchiver::string_of(&key);
                let value = object_of(&value, depth + 1);
                if key.is_null() || value.is_null() {
                    for made in [key, value] {
                        if !made.is_null() {
                            objc_release(made);
                        }
                    }
                    continue;
                }
                keys.push(key);
                values.push(value);
            }
            let made = released_after(&keys, |keys| {
                released_after(&values, |values| {
                    new_dictionary(class_pointer(&NSDICTIONARY), values, keys)
                })
            });
            keys.free();
            values.free();
            made
        }
        _ => core::ptr::null_mut(),
    }
}

const SECONDS_TO_2001: i64 = 978_307_200;

fn civil(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn iso8601(seconds: f64, out: &mut [u8; 20]) -> &[u8] {
    let unix = seconds.floor() as i64 + SECONDS_TO_2001;
    let (days, rest) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    let fields = [
        (year.clamp(0, 9999) as u32, 4, b'-'),
        (month, 2, b'-'),
        (day, 2, b'T'),
        ((rest / 3600) as u32, 2, b':'),
        ((rest / 60 % 60) as u32, 2, b':'),
        ((rest % 60) as u32, 2, b'Z'),
    ];
    let mut at = 0;
    for (value, width, after) in fields {
        for place in (0..width).rev() {
            out[at] = b'0' + (value / 10u32.pow(place) % 10) as u8;
            at += 1;
        }
        out[at] = after;
        at += 1;
    }
    &out[..at]
}

struct Writer<'o> {
    out: &'o mut Buffer<u8>,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> bool {
        bytes.iter().all(|&byte| self.out.push(byte))
    }

    fn escaped(&mut self, bytes: &[u8]) -> bool {
        bytes.iter().all(|&byte| match byte {
            b'&' => self.put(b"&amp;"),
            b'<' => self.put(b"&lt;"),
            b'>' => self.put(b"&gt;"),
            byte => self.put(&[byte]),
        })
    }

    fn text(&mut self, tag: &[u8], string: Id) -> bool {
        let opened = self.put(b"<") && self.put(tag) && self.put(b">");
        let body = with_utf8(string, |bytes| self.escaped(bytes));
        opened && body && self.put(b"</") && self.put(tag) && self.put(b">")
    }

    fn number(&mut self, value: i128) -> bool {
        let mut digits = [0u8; 40];
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
        (value >= 0 || self.put(b"-"))
            && digits[..length]
                .iter()
                .rev()
                .all(|&digit| self.put(&[digit]))
    }

    fn base64(&mut self, bytes: &[u8]) -> bool {
        bytes.chunks(3).all(|chunk| {
            let word = chunk.iter().enumerate().fold(0u32, |word, (at, &byte)| {
                word | u32::from(byte) << (16 - 8 * at)
            });
            (0..4).all(|at| match at <= chunk.len() {
                true => self.put(&[BASE64[(word >> (18 - 6 * at) & 0x3f) as usize]]),
                false => self.put(b"="),
            })
        })
    }

    fn object(&mut self, object: Id, depth: usize) -> bool {
        if object.is_null() || depth > MAX_DEPTH {
            return false;
        }
        if root::is_kind_of(object, &BOOLEAN) {
            let value = matches!(exact_value(object), Exact::Integer(value) if value != 0);
            return self.put(if value { b"<true/>" } else { b"<false/>" });
        }
        if is_number(object) {
            return match exact_value(object) {
                Exact::Integer(value) => {
                    self.put(b"<integer>") && self.number(value) && self.put(b"</integer>")
                }
                Exact::Real(value) => {
                    let mut written = self.put(b"<real>");
                    super::number::exact_decimal(value, &mut |bytes| written &= self.put(bytes));
                    written && self.put(b"</real>")
                }
            };
        }
        if is_string(object) {
            return self.text(b"string", object);
        }
        if is_date(object) {
            let seconds = super::date::seconds_of(object);
            let mut written = self.put(b"<date>");
            let mut text = [0u8; 20];
            written &= self.put(iso8601(seconds, &mut text));
            return written && self.put(b"</date>");
        }
        if root::is_kind_of(object, &NSDATA) {
            let bytes = super::data::bytes_of(object);
            return self.put(b"<data>") && self.base64(bytes) && self.put(b"</data>");
        }
        if is_array(object) {
            let count = send(object, sel!("count")) as usize;
            if !self.put(b"<array>") {
                return false;
            }
            for index in 0..count {
                let item = crate::objc::send1(object, sel!("objectAtIndex:"), index);
                if !self.object(item, depth + 1) {
                    return false;
                }
            }
            return self.put(b"</array>");
        }
        if is_dictionary(object) {
            let keys = send(object, sel!("allKeys"));
            let count = send(keys, sel!("count")) as usize;
            if !self.put(b"<dict>") {
                return false;
            }
            for index in 0..count {
                let key = crate::objc::send1(keys, sel!("objectAtIndex:"), index);
                if !is_string(key) {
                    continue;
                }
                let value = crate::objc::send1(object, sel!("objectForKey:"), key as usize);
                if !self.text(b"key", key) || !self.object(value, depth + 1) {
                    return false;
                }
            }
            return self.put(b"</dict>");
        }
        false
    }
}

/// Writes `object` as an XML property list into `out`; false when it holds something a
/// property list cannot, or there is no memory.
pub fn write_xml(object: Id, out: &mut Buffer<u8>) -> bool {
    let mut writer = Writer { out };
    writer.put(HEADER) && writer.object(object, 0) && writer.put(FOOTER)
}

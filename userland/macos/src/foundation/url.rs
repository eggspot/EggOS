//! NSURL for files (D392): a URL made from a path (`fileURLWithPath:`, `URLWithString:` of a
//! `file://` URL), which answers its path, its last component and extension, its string form
//! and URLs beside or under it. Other schemes keep their string and answer no path.

use super::{
    Definition, NSURL, NSURL_META, autoreleased, class_pointer, define as define_class,
    new_instance, string_from_utf8, with_utf8,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods};

const FILE_SCHEME: &[u8] = b"file://";

#[repr(C)]
#[derive(Debug)]
struct UrlFields {
    isa: usize,
    text: Buffer<u8>,
    is_file: bool,
}

fn fields(object: Id) -> &'static mut UrlFields {
    // SAFETY: the callers pass NSURL instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<UrlFields>() }
}

fn new_url(text: &[u8], is_file: bool) -> Id {
    let object = new_instance(class_pointer(&NSURL));
    if object.is_null() {
        return object;
    }
    let url = fields(object);
    for &byte in text {
        url.text.push(byte);
    }
    url.is_file = is_file;
    autoreleased(object)
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn unescaped(text: &[u8], out: &mut Buffer<u8>) {
    let mut at = 0;
    while at < text.len() {
        let byte = match (text[at], text.get(at + 1).and_then(|&b| hex(b)), text.get(at + 2).and_then(|&b| hex(b))) {
            (b'%', Some(high), Some(low)) => {
                at += 2;
                high << 4 | low
            }
            (byte, _, _) => byte,
        };
        out.push(byte);
        at += 1;
    }
}

fn escaped(path: &[u8], out: &mut Buffer<u8>) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in path {
        let plain = byte.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&byte);
        match plain {
            true => {
                out.push(byte);
            }
            false => {
                out.push(b'%');
                out.push(DIGITS[usize::from(byte >> 4)]);
                out.push(DIGITS[usize::from(byte & 0xf)]);
            }
        }
    }
}

extern "C" fn file_url_with_path(_: Id, _: Sel, path: Id) -> Id {
    if path.is_null() {
        return path;
    }
    with_utf8(path, |bytes| new_url(bytes, true))
}

extern "C" fn file_url_with_path_directory(class: Id, sel: Sel, path: Id, _: u8) -> Id {
    file_url_with_path(class, sel, path)
}

extern "C" fn url_with_string(_: Id, _: Sel, text: Id) -> Id {
    if text.is_null() {
        return text;
    }
    with_utf8(text, |bytes| match bytes.strip_prefix(FILE_SCHEME) {
        Some(rest) => {
            let mut path = Buffer::new();
            unescaped(rest, &mut path);
            let made = new_url(path.as_slice(), true);
            path.free();
            made
        }
        None => new_url(bytes, false),
    })
}

extern "C" fn dealloc(this: Id, _: Sel) {
    fields(this).text.free();
    dispose(this);
}

extern "C" fn path(this: Id, _: Sel) -> Id {
    let url = fields(this);
    match url.is_file {
        true => string_from_utf8(url.text.as_slice()),
        false => core::ptr::null_mut(),
    }
}

extern "C" fn absolute_string(this: Id, _: Sel) -> Id {
    let url = fields(this);
    if !url.is_file {
        return string_from_utf8(url.text.as_slice());
    }
    let mut out = Buffer::new();
    for &byte in FILE_SCHEME {
        out.push(byte);
    }
    escaped(url.text.as_slice(), &mut out);
    let made = string_from_utf8(out.as_slice());
    out.free();
    made
}

extern "C" fn is_file_url(this: Id, _: Sel) -> u8 {
    u8::from(fields(this).is_file)
}

fn last_component(path: &[u8]) -> &[u8] {
    let trimmed = path.strip_suffix(b"/").unwrap_or(path);
    trimmed.rsplit(|&byte| byte == b'/').next().unwrap_or(trimmed)
}

extern "C" fn last_path_component(this: Id, _: Sel) -> Id {
    string_from_utf8(last_component(fields(this).text.as_slice()))
}

extern "C" fn path_extension(this: Id, _: Sel) -> Id {
    let name = last_component(fields(this).text.as_slice());
    let extension = match name.iter().rposition(|&byte| byte == b'.') {
        Some(0) | None => &b""[..],
        Some(at) => &name[at + 1..],
    };
    string_from_utf8(extension)
}

extern "C" fn appending_path_component(this: Id, _: Sel, component: Id) -> Id {
    let url = fields(this);
    let mut joined = Buffer::new();
    for &byte in url.text.as_slice() {
        joined.push(byte);
    }
    if !joined.as_slice().ends_with(b"/") {
        joined.push(b'/');
    }
    with_utf8(component, |bytes| {
        for &byte in bytes {
            joined.push(byte);
        }
    });
    let made = new_url(joined.as_slice(), url.is_file);
    joined.free();
    made
}

extern "C" fn deleting_last_path_component(this: Id, _: Sel) -> Id {
    let url = fields(this);
    let text = url.text.as_slice();
    let trimmed = text.strip_suffix(b"/").unwrap_or(text);
    let parent = match trimmed.iter().rposition(|&byte| byte == b'/') {
        Some(0) => &b"/"[..],
        Some(at) => &trimmed[..at],
        None => &b"."[..],
    };
    new_url(parent, url.is_file)
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    if this == other {
        return 1;
    }
    if other.is_null() || !crate::objc::root::is_kind_of(other, &NSURL) {
        return 0;
    }
    u8::from(fields(this).text.as_slice() == fields(other).text.as_slice() && fields(this).is_file == fields(other).is_file)
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "dealloc" => dealloc,
        "path" => path,
        "absoluteString" => absolute_string,
        "description" => absolute_string,
        "isFileURL" => is_file_url,
        "lastPathComponent" => last_path_component,
        "pathExtension" => path_extension,
        "URLByAppendingPathComponent:" => appending_path_component,
        "URLByDeletingLastPathComponent" => deleting_last_path_component,
        "isEqual:" => is_equal,
        "copyWithZone:" => copy_with_zone,
    ];
    let class = methods![
        "fileURLWithPath:" => file_url_with_path,
        "fileURLWithPath:isDirectory:" => file_url_with_path_directory,
        "URLWithString:" => url_with_string,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSURL\0",
            class: &NSURL,
            meta: &NSURL_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<UrlFields>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

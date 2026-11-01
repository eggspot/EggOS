//! NSError (D175): a domain, a code and a user info dictionary, with the description macOS
//! gives one: the user info's `NSLocalizedDescription` when there is one, else
//! `The operation couldn’t be completed. (<domain> error <code>.)`.

use super::constants::{self, LOCALIZED_DESCRIPTION_KEY, LOCALIZED_FAILURE_REASON_KEY};
use super::{
    Definition, NSERROR, NSERROR_META, autoreleased, define as define_class, is_string,
    new_instance, string_from_utf8, with_utf8,
};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send, send1};

#[repr(C)]
#[derive(Debug)]
struct ErrorObject {
    isa: usize,
    domain: Id,
    code: isize,
    user_info: Id,
}

fn fields(object: Id) -> &'static mut ErrorObject {
    // SAFETY: the callers pass NSError instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ErrorObject>() }
}

fn retained(object: Id) -> Id {
    match object.is_null() {
        true => object,
        false => objc_retain(object),
    }
}

extern "C" fn init_with_domain(this: Id, _: Sel, domain: Id, code: isize, user_info: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = fields(this);
    fields.domain = match domain.is_null() {
        true => domain,
        false => send(domain, sel!("copy")),
    };
    fields.code = code;
    fields.user_info = retained(user_info);
    this
}

extern "C" fn error_with_domain(this: Id, _: Sel, domain: Id, code: isize, user_info: Id) -> Id {
    let object = new_instance(this.cast::<ClassObject>());
    autoreleased(init_with_domain(
        object,
        core::ptr::null(),
        domain,
        code,
        user_info,
    ))
}

extern "C" fn domain(this: Id, _: Sel) -> Id {
    fields(this).domain
}

extern "C" fn code(this: Id, _: Sel) -> isize {
    fields(this).code
}

extern "C" fn user_info(this: Id, _: Sel) -> Id {
    fields(this).user_info
}

fn user_info_string(this: Id, key: usize) -> Option<Id> {
    let user_info = fields(this).user_info;
    if user_info.is_null() {
        return None;
    }
    let value = send1(
        user_info,
        sel!("objectForKey:"),
        constants::string(key) as usize,
    );
    (!value.is_null() && is_string(value)).then_some(value)
}

fn push(out: &mut Buffer<u8>, bytes: &[u8]) {
    if out.reserve(bytes.len()) {
        for &byte in bytes {
            out.push(byte);
        }
    }
}

fn push_number(out: &mut Buffer<u8>, value: isize) {
    let mut digits = [0u8; 24];
    let mut at = digits.len();
    let mut rest = value.unsigned_abs();
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if value < 0 {
        push(out, b"-");
    }
    push(out, &digits[at..]);
}

fn built(work: impl FnOnce(&mut Buffer<u8>)) -> Id {
    let mut out = Buffer::new();
    work(&mut out);
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn localized_description(this: Id, _: Sel) -> Id {
    if let Some(description) = user_info_string(this, LOCALIZED_DESCRIPTION_KEY) {
        return description;
    }
    let (domain, code) = (fields(this).domain, fields(this).code);
    built(|out| {
        push(
            out,
            "The operation couldn\u{2019}t be completed. (".as_bytes(),
        );
        with_utf8(domain, |domain| push(out, domain));
        push(out, b" error ");
        push_number(out, code);
        push(out, b".)");
    })
}

extern "C" fn localized_failure_reason(this: Id, _: Sel) -> Id {
    user_info_string(this, LOCALIZED_FAILURE_REASON_KEY).unwrap_or(core::ptr::null_mut())
}

extern "C" fn description(this: Id, sel: Sel) -> Id {
    let (domain, code) = (fields(this).domain, fields(this).code);
    let text = localized_description(this, sel);
    built(|out| {
        push(out, b"Error Domain=");
        with_utf8(domain, |domain| push(out, domain));
        push(out, b" Code=");
        push_number(out, code);
        push(out, b" \"");
        with_utf8(text, |text| push(out, text));
        push(out, b"\"");
    })
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    for object in [fields.domain, fields.user_info] {
        if !object.is_null() {
            objc_release(object);
        }
    }
    dispose(this);
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithDomain:code:userInfo:" => init_with_domain,
        "domain" => domain,
        "code" => code,
        "userInfo" => user_info,
        "localizedDescription" => localized_description,
        "localizedFailureReason" => localized_failure_reason,
        "description" => description,
        "copyWithZone:" => copy_with_zone,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "errorWithDomain:code:userInfo:" => error_with_domain,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSError\0",
            class: &NSERROR,
            meta: &NSERROR_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<ErrorObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

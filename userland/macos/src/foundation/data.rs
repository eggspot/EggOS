//! NSData and NSMutableData (D175): bytes the object owns in the layer's heap, made from a
//! buffer, a string's UTF-8 or a file, compared byte for byte, described as macOS describes
//! them (`{length = 3, bytes = 0x616263}`), and grown by appending.

use super::{
    Definition, NSDATA, NSDATA_META, NSMUTABLEDATA, NSMUTABLEDATA_META, autoreleased,
    class_pointer, define as define_class, is_data, new_instance, string_from_utf8, with_utf8,
};
use crate::libsystem::heap;
use crate::linux::{self, O_CLOEXEC, O_RDONLY};
use crate::objc::abi::{ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods, terminate_uncaught};

const SHOWN_BYTES: usize = 24;
const READ_CHUNK: usize = 4096;
const HEXADECIMAL: &[u8; 16] = b"0123456789abcdef";

#[repr(C)]
#[derive(Debug)]
struct DataObject {
    isa: usize,
    bytes: *mut u8,
    length: usize,
    capacity: usize,
}

fn fields(object: Id) -> &'static mut DataObject {
    // SAFETY: the callers pass NSData instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<DataObject>() }
}

/// The bytes of the data object `object`; valid until it changes or goes.
pub fn bytes_of(object: Id) -> &'static [u8] {
    let fields = fields(object);
    if fields.bytes.is_null() || fields.length == 0 {
        return &[];
    }
    // SAFETY: the object owns `length` bytes at `bytes`.
    unsafe { core::slice::from_raw_parts(fields.bytes, fields.length) }
}

fn reserve(this: Id, needed: usize) -> bool {
    let fields = fields(this);
    if needed <= fields.capacity {
        return true;
    }
    let capacity = needed.max(fields.capacity.saturating_mul(2)).max(16);
    let grown = match fields.bytes.is_null() {
        true => heap::allocate(capacity, true),
        false => heap::reallocate(fields.bytes, capacity),
    };
    if grown.is_null() {
        return false;
    }
    fields.bytes = grown;
    fields.capacity = capacity;
    true
}

fn append(this: Id, bytes: &[u8]) -> bool {
    let length = fields(this).length;
    let Some(needed) = length.checked_add(bytes.len()) else {
        return false;
    };
    if bytes.is_empty() {
        return true;
    }
    if !reserve(this, needed) {
        return false;
    }
    let fields = fields(this);
    // SAFETY: the block holds `capacity >= needed` bytes; `bytes` is another object's or the
    // caller's, never this block, except when appending itself, which copies first.
    unsafe {
        fields
            .bytes
            .wrapping_add(length)
            .copy_from(bytes.as_ptr(), bytes.len())
    };
    fields.length = needed;
    true
}

fn new_data(class: *mut ClassObject, bytes: &[u8]) -> Id {
    let object = new_instance(class);
    if object.is_null() {
        return object;
    }
    if !append(object, bytes) {
        objc_release(object);
        return core::ptr::null_mut();
    }
    object
}

fn this_class(this: Id) -> *mut ClassObject {
    this.cast()
}

fn borrowed<'a>(bytes: *const u8, length: usize) -> &'a [u8] {
    if bytes.is_null() || length == 0 {
        return &[];
    }
    // SAFETY: the program passes `length` readable bytes.
    unsafe { core::slice::from_raw_parts(bytes, length) }
}

extern "C" fn data(this: Id, _: Sel) -> Id {
    autoreleased(new_data(this_class(this), &[]))
}

extern "C" fn data_with_bytes(this: Id, _: Sel, bytes: *const u8, length: usize) -> Id {
    autoreleased(new_data(this_class(this), borrowed(bytes, length)))
}

extern "C" fn data_with_data(this: Id, _: Sel, other: Id) -> Id {
    let mut copy = match other.is_null() {
        true => Buffer::new(),
        false => copied(bytes_of(other)),
    };
    let made = new_data(this_class(this), copy.as_slice());
    copy.free();
    autoreleased(made)
}

fn copied(bytes: &[u8]) -> Buffer<u8> {
    let mut copy = Buffer::new();
    if copy.reserve(bytes.len()) {
        for &byte in bytes {
            copy.push(byte);
        }
    }
    copy
}

fn read_file(path: &[u8], out: &mut Buffer<u8>) -> bool {
    let mut terminated = Buffer::new();
    for &byte in path.iter().chain(&[0]) {
        terminated.push(byte);
    }
    let opened = linux::open(terminated.as_slice(), O_RDONLY | O_CLOEXEC, 0);
    terminated.free();
    let Ok(fd) = opened else {
        return false;
    };
    let mut chunk = [0u8; READ_CHUNK];
    let read_all = loop {
        match linux::read(fd, &mut chunk) {
            Ok(0) => break true,
            Ok(read) => {
                if !out.reserve(read) {
                    break false;
                }
                for &byte in &chunk[..read] {
                    out.push(byte);
                }
            }
            Err(_) => break false,
        }
    };
    let _ = linux::close(fd);
    read_all
}

extern "C" fn data_with_contents_of_file(this: Id, _: Sel, path: Id) -> Id {
    if path.is_null() {
        return core::ptr::null_mut();
    }
    let mut contents = Buffer::new();
    let read = with_utf8(path, |path| read_file(path, &mut contents));
    let made = match read {
        true => autoreleased(new_data(this_class(this), contents.as_slice())),
        false => core::ptr::null_mut(),
    };
    contents.free();
    made
}

extern "C" fn init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn init_with_bytes(this: Id, _: Sel, bytes: *const u8, length: usize) -> Id {
    if this.is_null() || append(this, borrowed(bytes, length)) {
        return this;
    }
    objc_release(this);
    core::ptr::null_mut()
}

extern "C" fn length(this: Id, _: Sel) -> usize {
    fields(this).length
}

extern "C" fn bytes(this: Id, _: Sel) -> *const u8 {
    fields(this).bytes
}

extern "C" fn get_bytes(this: Id, _: Sel, out: *mut u8, length: usize) {
    let source = bytes_of(this);
    let count = length.min(source.len());
    if count > 0 && !out.is_null() {
        // SAFETY: the program passes room for `length` bytes.
        unsafe { out.copy_from_nonoverlapping(source.as_ptr(), count) };
    }
}

extern "C" fn is_equal(this: Id, _: Sel, other: Id) -> u8 {
    u8::from(!other.is_null() && is_data(other) && bytes_of(this) == bytes_of(other))
}

extern "C" fn hash(this: Id, _: Sel) -> usize {
    bytes_of(this)
        .iter()
        .take(80)
        .fold(fields(this).length, |hash, &byte| {
            hash.wrapping_mul(31).wrapping_add(usize::from(byte))
        })
}

extern "C" fn description(this: Id, _: Sel) -> Id {
    let bytes = bytes_of(this);
    let mut out = Buffer::new();
    let mut push = |text: &[u8]| {
        if out.reserve(text.len()) {
            for &byte in text {
                out.push(byte);
            }
        }
    };
    push(b"{length = ");
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut rest = bytes.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    push(&digits[at..]);
    push(b", bytes = 0x");
    let shown = match bytes.len() > SHOWN_BYTES {
        true => &bytes[..SHOWN_BYTES / 2],
        false => bytes,
    };
    for &byte in shown {
        push(&[
            HEXADECIMAL[usize::from(byte >> 4)],
            HEXADECIMAL[usize::from(byte & 15)],
        ]);
    }
    if bytes.len() > SHOWN_BYTES {
        push(b" ... ");
        for &byte in &bytes[bytes.len() - 4..] {
            push(&[
                HEXADECIMAL[usize::from(byte >> 4)],
                HEXADECIMAL[usize::from(byte & 15)],
            ]);
        }
    }
    push(b"}");
    let string = string_from_utf8(out.as_slice());
    out.free();
    string
}

extern "C" fn copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    if root::is_kind_of(this, &NSMUTABLEDATA) {
        let mut copy = copied(bytes_of(this));
        let made = new_data(class_pointer(&NSDATA), copy.as_slice());
        copy.free();
        return made;
    }
    objc_retain(this)
}

extern "C" fn mutable_copy_with_zone(this: Id, _: Sel, _: usize) -> Id {
    let mut copy = copied(bytes_of(this));
    let made = new_data(class_pointer(&NSMUTABLEDATA), copy.as_slice());
    copy.free();
    made
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = fields(this);
    if !fields.bytes.is_null() {
        heap::release(fields.bytes);
    }
    dispose(this);
}

fn grown_or_fail(this: Id, done: bool) {
    if !done {
        terminate_uncaught(
            b"NSMallocException",
            &[
                b"*** -[",
                root::class_name(this),
                b" appendBytes:length:]: out of memory",
            ],
        );
    }
}

extern "C" fn append_bytes(this: Id, _: Sel, bytes: *const u8, length: usize) {
    let mut copy = copied(borrowed(bytes, length));
    grown_or_fail(this, append(this, copy.as_slice()));
    copy.free();
}

extern "C" fn append_data(this: Id, _: Sel, other: Id) {
    if other.is_null() {
        return;
    }
    let mut copy = copied(bytes_of(other));
    grown_or_fail(this, append(this, copy.as_slice()));
    copy.free();
}

extern "C" fn set_length(this: Id, _: Sel, length: usize) {
    if length > fields(this).length {
        let more = length - fields(this).length;
        if reserve(this, length) {
            let fields = fields(this);
            // SAFETY: the block holds at least `length` bytes; the new tail is zeroed.
            unsafe {
                fields
                    .bytes
                    .wrapping_add(fields.length)
                    .write_bytes(0, more)
            };
            fields.length = length;
        }
        return;
    }
    fields(this).length = length;
}

extern "C" fn mutable_bytes(this: Id, _: Sel) -> *mut u8 {
    fields(this).bytes
}

extern "C" fn data_with_capacity(this: Id, _: Sel, capacity: usize) -> Id {
    let made = new_data(this_class(this), &[]);
    if !made.is_null() {
        reserve(made, capacity.min(1 << 20));
    }
    autoreleased(made)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "init" => init,
        "initWithBytes:length:" => init_with_bytes,
        "length" => length,
        "bytes" => bytes,
        "getBytes:length:" => get_bytes,
        "isEqual:" => is_equal,
        "isEqualToData:" => is_equal,
        "hash" => hash,
        "description" => description,
        "copyWithZone:" => copy_with_zone,
        "mutableCopyWithZone:" => mutable_copy_with_zone,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "data" => data,
        "dataWithBytes:length:" => data_with_bytes,
        "dataWithData:" => data_with_data,
        "dataWithContentsOfFile:" => data_with_contents_of_file,
    ];
    let mutable_instance = methods![
        "appendBytes:length:" => append_bytes,
        "appendData:" => append_data,
        "setLength:" => set_length,
        "mutableBytes" => mutable_bytes,
    ];
    let mutable_class = methods![
        "dataWithCapacity:" => data_with_capacity,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSData\0",
            class: &NSDATA,
            meta: &NSDATA_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<DataObject>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    ) && define_class(
        runtime,
        &Definition {
            name: b"NSMutableData\0",
            class: &NSMUTABLEDATA,
            meta: &NSMUTABLEDATA_META,
            superclass: &NSDATA,
            instance_size: core::mem::size_of::<DataObject>(),
            flags: 0,
            instance_methods: &mutable_instance,
            class_methods: &mutable_class,
        },
    )
}

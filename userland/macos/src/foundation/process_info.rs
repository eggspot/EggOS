//! NSProcessInfo (D175): one shared object answering for the running program: its name (the
//! last part of the path it was started by, as `main`'s `argv[0]` gives it), process id,
//! arguments and environment, the machine's host name, processors, memory and uptime, and the
//! operating system's version string.

use super::{
    Definition, NSARRAY, NSDICTIONARY, NSPROCESSINFO, NSPROCESSINFO_META, class_pointer,
    define as define_class, new_instance, string_from_utf8,
};
use crate::libsystem::process;
use crate::linux::{self, O_CLOEXEC, O_RDONLY};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send2};
use core::ffi::c_char;
use core::sync::atomic::{AtomicUsize, Ordering};

const HOST_NAME_FILE: &[u8] = b"/etc/hostname\0";
const MEMORY_FILE: &[u8] = b"/proc/meminfo\0";
const MEMORY_TOTAL: &[u8] = b"MemTotal:";
const FILE_BYTES: usize = 256;
const MONOTONIC_CLOCK: usize = 1;
const VERSION: &str = concat!("Version ", env!("CARGO_PKG_VERSION"), " (EggOS)");

static SHARED: AtomicUsize = AtomicUsize::new(0);

fn c_bytes<'a>(text: *const c_char) -> &'a [u8] {
    if text.is_null() {
        return b"";
    }
    // SAFETY: the callers pass NUL-terminated strings that live as long as the process.
    unsafe { core::ffi::CStr::from_ptr(text) }.to_bytes()
}

fn read_small_file<'a>(path: &[u8], buffer: &'a mut [u8; FILE_BYTES]) -> &'a [u8] {
    let Ok(fd) = linux::open(path, O_RDONLY | O_CLOEXEC, 0) else {
        return &[];
    };
    let mut filled = 0;
    while filled < buffer.len() {
        match linux::read(fd, &mut buffer[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let _ = linux::close(fd);
    &buffer[..filled]
}

extern "C" fn process_info(_: Id, _: Sel) -> Id {
    let existing = SHARED.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let made = new_instance(class_pointer(&NSPROCESSINFO));
    match SHARED.compare_exchange(0, made as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => made,
        Err(winner) => {
            crate::objc::reference_count::objc_release(made);
            winner as Id
        }
    }
}

extern "C" fn process_name(_: Id, _: Sel) -> Id {
    string_from_utf8(c_bytes(process::program_name()))
}

extern "C" fn process_identifier(_: Id, _: Sel) -> i32 {
    process::getpid()
}

fn strings_of(list: *const *mut c_char, count: Option<usize>) -> Buffer<Id> {
    let mut strings = Buffer::new();
    if list.is_null() {
        return strings;
    }
    let mut index = 0;
    while count.is_none_or(|count| index < count) {
        // SAFETY: the process's argument or environment vector, `count` long or NULL-ended.
        let entry = unsafe { list.wrapping_add(index).read() };
        if entry.is_null() {
            break;
        }
        strings.push(string_from_utf8(c_bytes(entry)));
        index += 1;
    }
    strings
}

extern "C" fn arguments(_: Id, _: Sel) -> Id {
    let (count, vector) = process::arguments();
    let mut strings = strings_of(vector, Some(count));
    let array = send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        strings.as_slice().as_ptr() as usize,
        strings.len(),
    );
    strings.free();
    array
}

extern "C" fn environment(_: Id, _: Sel) -> Id {
    // SAFETY: the environment is set before the program runs; reading the pointer is a copy.
    let vector = unsafe { process::ENVIRON };
    let mut keys = Buffer::new();
    let mut values = Buffer::new();
    let mut index = 0;
    while !vector.is_null() {
        // SAFETY: a NULL-ended environment vector.
        let entry = unsafe { vector.wrapping_add(index).read() };
        if entry.is_null() {
            break;
        }
        let bytes = c_bytes(entry);
        if let Some(equals) = bytes.iter().position(|&byte| byte == b'=') {
            keys.push(string_from_utf8(&bytes[..equals]));
            values.push(string_from_utf8(&bytes[equals + 1..]));
        }
        index += 1;
    }
    let function: extern "C" fn(Id, Sel, *const Id, *const Id, usize) -> Id =
        crate::objc::message::msg_send_as();
    let dictionary = function(
        class_pointer(&NSDICTIONARY).cast(),
        sel!("dictionaryWithObjects:forKeys:count:"),
        values.as_slice().as_ptr(),
        keys.as_slice().as_ptr(),
        keys.len(),
    );
    keys.free();
    values.free();
    dictionary
}

extern "C" fn host_name(_: Id, _: Sel) -> Id {
    let mut buffer = [0u8; FILE_BYTES];
    let text = read_small_file(HOST_NAME_FILE, &mut buffer);
    let line = text.split(|&byte| byte == b'\n').next().unwrap_or(b"");
    match line.is_empty() {
        true => string_from_utf8(b"localhost"),
        false => string_from_utf8(line),
    }
}

extern "C" fn processor_count(_: Id, _: Sel) -> usize {
    linux::cpu_count().max(1) as usize
}

fn total_memory_kibibytes(text: &[u8]) -> u64 {
    let Some(line) = text
        .split(|&byte| byte == b'\n')
        .find(|line| line.starts_with(MEMORY_TOTAL))
    else {
        return 0;
    };
    line[MEMORY_TOTAL.len()..]
        .iter()
        .skip_while(|byte| byte.is_ascii_whitespace())
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0u64, |total, &digit| {
            total
                .saturating_mul(10)
                .saturating_add(u64::from(digit - b'0'))
        })
}

extern "C" fn physical_memory(_: Id, _: Sel) -> u64 {
    let mut buffer = [0u8; FILE_BYTES];
    total_memory_kibibytes(read_small_file(MEMORY_FILE, &mut buffer)).saturating_mul(1024)
}

extern "C" fn system_uptime(_: Id, _: Sel) -> f64 {
    let [seconds, nanoseconds] = linux::clock_time(MONOTONIC_CLOCK).unwrap_or([0, 0]);
    seconds as f64 + nanoseconds as f64 / 1e9
}

extern "C" fn version_string(_: Id, _: Sel) -> Id {
    string_from_utf8(VERSION.as_bytes())
}

extern "C" fn retained(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn nothing(_: Id, _: Sel) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "processName" => process_name,
        "processIdentifier" => process_identifier,
        "arguments" => arguments,
        "environment" => environment,
        "hostName" => host_name,
        "processorCount" => processor_count,
        "activeProcessorCount" => processor_count,
        "physicalMemory" => physical_memory,
        "systemUptime" => system_uptime,
        "operatingSystemVersionString" => version_string,
        "retain" => retained,
        "release" => nothing,
        "autorelease" => retained,
    ];
    let class = methods![
        "processInfo" => process_info,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSProcessInfo\0",
            class: &NSPROCESSINFO,
            meta: &NSPROCESSINFO_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<usize>(),
            flags: 0,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

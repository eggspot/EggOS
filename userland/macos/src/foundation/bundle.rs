//! NSBundle's main bundle. A program inside an `.app` (`X.app/Contents/MacOS/<name>`) belongs to
//! that bundle: its path is the `.app` folder, its resources are in `Contents/Resources` and its
//! information dictionary holds `Contents/Info.plist`'s string values. A console program is not
//! inside one, so, as on macOS, its main bundle is the directory that holds the executable, with
//! no identifier and an empty information dictionary.

use super::{
    Definition, NSBUNDLE, NSBUNDLE_META, NSMUTABLEDICTIONARY, class_pointer,
    define as define_class, new_instance, string_from_utf8,
};
use crate::info_plist;
use crate::libsystem::heap;
use crate::linux;
use crate::objc::abi::{Id, Sel};
use crate::objc::class::IMMORTAL;
use crate::objc::reference_count::objc_retain;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send, send2};
use core::sync::atomic::{AtomicUsize, Ordering};

const INFO_PLIST: &[u8] = b"/Contents/Info.plist";
const RESOURCES: &[u8] = b"/Contents/Resources";
const LARGEST_INFO: usize = 64 * 1024;
const LONGEST_VALUE: usize = 1024;
const LONGEST_PATH: usize = 4096;

#[repr(C)]
struct BundleObject {
    isa: usize,
    path: Id,
    executable: Id,
    resources: Id,
    info: Id,
}

static MAIN: AtomicUsize = AtomicUsize::new(0);
static INFO_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static INFO_LENGTH: AtomicUsize = AtomicUsize::new(0);
static NAME_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static NAME_LENGTH: AtomicUsize = AtomicUsize::new(0);

fn executable_path() -> &'static [u8] {
    let path = crate::libsystem::dynamic::executable_path();
    if path.is_null() {
        return b"";
    }
    // SAFETY: the executable's path is a NUL-terminated string kept while the program runs.
    unsafe { crate::objc::selector::name_bytes(path.cast()) }
}

fn directory_of(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) => b"/",
        Some(slash) => &path[..slash],
        None => b".",
    }
}

fn file_name(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(slash) => &path[slash + 1..],
        None => path,
    }
}

/// A copy of `bytes` that lives as long as the program; empty if there is no memory.
pub fn permanent(bytes: &[u8]) -> &'static [u8] {
    let copy = heap::allocate(bytes.len().max(1), false);
    if copy.is_null() {
        return b"";
    }
    // SAFETY: the new block holds `bytes.len()` bytes; it is never freed.
    unsafe { copy.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
    // SAFETY: as above, initialized and never freed.
    unsafe { core::slice::from_raw_parts(copy, bytes.len()) }
}

fn joined(first: &[u8], second: &[u8], out: &mut [u8; LONGEST_PATH]) -> Option<usize> {
    let length = first.len() + second.len();
    if length >= LONGEST_PATH {
        return None;
    }
    out[..first.len()].copy_from_slice(first);
    out[first.len()..length].copy_from_slice(second);
    out[length] = 0;
    Some(length)
}

fn read_info() -> &'static [u8] {
    let Some(bundle) = info_plist::bundle_path(executable_path()) else {
        return b"";
    };
    let mut path = [0u8; LONGEST_PATH];
    let Some(length) = joined(bundle, INFO_PLIST, &mut path) else {
        return b"";
    };
    let Ok(fd) = linux::open(&path[..=length], linux::O_RDONLY | linux::O_CLOEXEC, 0) else {
        return b"";
    };
    let buffer = heap::allocate(LARGEST_INFO, false);
    if buffer.is_null() {
        let _ = linux::close(fd);
        return b"";
    }
    // SAFETY: the new block holds `LARGEST_INFO` bytes, used only through this slice; it is
    // never freed.
    let bytes = unsafe { core::slice::from_raw_parts_mut(buffer, LARGEST_INFO) };
    let mut filled = 0;
    while filled < LARGEST_INFO {
        match linux::read(fd, &mut bytes[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(read) => filled += read,
        }
    }
    let _ = linux::close(fd);
    &bytes[..filled]
}

fn info_plist_bytes() -> &'static [u8] {
    let address = INFO_ADDRESS.load(Ordering::Acquire);
    if address != 0 {
        // SAFETY: a permanent block `read_info` filled, published below.
        return unsafe {
            core::slice::from_raw_parts(address as *const u8, INFO_LENGTH.load(Ordering::Acquire))
        };
    }
    let bytes = read_info();
    let address = if bytes.is_empty() {
        b"".as_ptr() as usize
    } else {
        bytes.as_ptr() as usize
    };
    INFO_LENGTH.store(bytes.len(), Ordering::Release);
    INFO_ADDRESS.store(address, Ordering::Release);
    bytes
}

/// The string value of `key` in the bundle's `Info.plist`, if it has one.
pub fn info_string(key: &[u8]) -> Option<&'static [u8]> {
    let mut scratch = [0u8; LONGEST_VALUE];
    let mut found = None;
    info_plist::for_each_string(info_plist_bytes(), &mut scratch, |name, value| {
        if found.is_none() && name == key {
            found = Some(permanent(value));
        }
    });
    found
}

/// The app's name: its bundle's `CFBundleName`, else the executable's file name.
pub fn app_name() -> &'static [u8] {
    let address = NAME_ADDRESS.load(Ordering::Acquire);
    if address != 0 {
        // SAFETY: a permanent copy published below.
        return unsafe {
            core::slice::from_raw_parts(address as *const u8, NAME_LENGTH.load(Ordering::Acquire))
        };
    }
    let name = info_string(b"CFBundleName")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| permanent(file_name(executable_path())));
    NAME_LENGTH.store(name.len(), Ordering::Release);
    NAME_ADDRESS.store(name.as_ptr() as usize, Ordering::Release);
    name
}

fn info_dictionary_of_plist() -> Id {
    let dictionary = send(
        class_pointer(&NSMUTABLEDICTIONARY).cast(),
        sel!("dictionary"),
    );
    if dictionary.is_null() {
        return dictionary;
    }
    let mut scratch = [0u8; LONGEST_VALUE];
    info_plist::for_each_string(info_plist_bytes(), &mut scratch, |key, value| {
        send2(
            dictionary,
            sel!("setObject:forKey:"),
            string_from_utf8(value) as usize,
            string_from_utf8(key) as usize,
        );
    });
    objc_retain(dictionary)
}

extern "C" fn main_bundle(_: Id, _: Sel) -> Id {
    let existing = MAIN.load(Ordering::Acquire);
    if existing != 0 {
        return existing as Id;
    }
    let bundle = new_instance((&raw const NSBUNDLE).cast_mut());
    if bundle.is_null() {
        return bundle;
    }
    let path = executable_path();
    let folder = info_plist::bundle_path(path).unwrap_or_else(|| directory_of(path));
    let mut resources_path = [0u8; LONGEST_PATH];
    let resources = match info_plist::bundle_path(path) {
        Some(bundle) => {
            joined(bundle, RESOURCES, &mut resources_path).map(|length| &resources_path[..length])
        }
        None => None,
    }
    .unwrap_or(folder);
    // SAFETY: a new bundle object.
    let fields = unsafe { &mut *bundle.cast::<BundleObject>() };
    fields.path = objc_retain(string_from_utf8(folder));
    fields.executable = objc_retain(string_from_utf8(path));
    fields.resources = objc_retain(string_from_utf8(resources));
    fields.info = info_dictionary_of_plist();
    match MAIN.compare_exchange(0, bundle as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => bundle,
        Err(winner) => winner as Id,
    }
}

fn fields(this: Id) -> &'static BundleObject {
    // SAFETY: a bundle object, which lives as long as the program.
    unsafe { &*this.cast::<BundleObject>() }
}

extern "C" fn bundle_path(this: Id, _: Sel) -> Id {
    fields(this).path
}

extern "C" fn resource_path(this: Id, _: Sel) -> Id {
    fields(this).resources
}

extern "C" fn executable(this: Id, _: Sel) -> Id {
    fields(this).executable
}

extern "C" fn bundle_identifier(this: Id, sel: Sel) -> Id {
    object_for_info_key(this, sel, string_from_utf8(b"CFBundleIdentifier"))
}

extern "C" fn info_dictionary(this: Id, _: Sel) -> Id {
    match fields(this).info {
        info if info.is_null() => super::dictionary::empty_address() as usize as Id,
        info => info,
    }
}

extern "C" fn object_for_info_key(this: Id, _: Sel, key: Id) -> Id {
    let info = fields(this).info;
    if info.is_null() || key.is_null() {
        return core::ptr::null_mut();
    }
    crate::objc::send1(info, sel!("objectForKey:"), key as usize)
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "bundlePath" => bundle_path,
        "resourcePath" => resource_path,
        "executablePath" => executable,
        "bundleIdentifier" => bundle_identifier,
        "infoDictionary" => info_dictionary,
        "objectForInfoDictionaryKey:" => object_for_info_key,
    ];
    let class = methods![
        "mainBundle" => main_bundle,
    ];
    define_class(
        runtime,
        &Definition {
            name: b"NSBundle\0",
            class: &NSBUNDLE,
            meta: &NSBUNDLE_META,
            superclass: &NSOBJECT,
            instance_size: core::mem::size_of::<BundleObject>(),
            flags: IMMORTAL,
            instance_methods: &instance,
            class_methods: &class,
        },
    )
}

//! NSBundle's main bundle. A console program is not inside a `.app`, so, as on macOS, its main
//! bundle is the directory that holds the executable: its path and resource path are that
//! directory, it has no identifier and an empty information dictionary.

use super::{
    Definition, NSBUNDLE, NSBUNDLE_META, define as define_class, new_instance, string_from_utf8,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::class::IMMORTAL;
use crate::objc::reference_count::objc_retain;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods};
use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
struct BundleObject {
    isa: usize,
    path: Id,
    executable: Id,
}

static MAIN: AtomicUsize = AtomicUsize::new(0);

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
    // SAFETY: a new bundle object.
    let fields = unsafe { &mut *bundle.cast::<BundleObject>() };
    fields.path = objc_retain(string_from_utf8(directory_of(path)));
    fields.executable = objc_retain(string_from_utf8(path));
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

extern "C" fn executable(this: Id, _: Sel) -> Id {
    fields(this).executable
}

extern "C" fn nothing(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn nothing_for(_: Id, _: Sel, _: Id) -> Id {
    core::ptr::null_mut()
}

extern "C" fn info_dictionary(_: Id, _: Sel) -> Id {
    super::dictionary::empty_address() as usize as Id
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "bundlePath" => bundle_path,
        "resourcePath" => bundle_path,
        "executablePath" => executable,
        "bundleIdentifier" => nothing,
        "infoDictionary" => info_dictionary,
        "objectForInfoDictionaryKey:" => nothing_for,
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

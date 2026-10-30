//! The Swift runtime, `/usr/lib/swift/libswiftCore.dylib` (D180): what a Swift program built
//! for macOS 13 or later imports from the operating system's copy of the standard library and
//! its runtime, written from Swift's public ABI documents and observed behaviour. Native
//! objects and their reference counts, type metadata and value witnesses, `String`, `Hasher`,
//! `Array` and `Dictionary` storage, mangled type names, the standard library's protocol
//! conformances and `print`; witness tables of a program's own conformances, dynamic casts,
//! weak references, `Character`s and the runtime parts of the overlays. It is set up only for a
//! program that links it, on top of the Objective-C runtime, since every Swift class is also an
//! Objective-C class.

pub mod case_mapping;
pub mod conformance_layout;
pub mod enum_tag;
pub mod grapheme;
pub mod hash_table;
pub mod hasher;
pub mod mangling;
pub mod number_text;
pub mod refcount;
pub mod string_index;
pub mod string_object;

#[cfg(target_env = "musl")]
mod cast;
#[cfg(target_env = "musl")]
mod characters;
#[cfg(target_env = "musl")]
mod conformance;
#[cfg(target_env = "musl")]
mod entry;
#[cfg(target_env = "musl")]
mod failures;
#[cfg(target_env = "musl")]
mod generic;
#[cfg(target_env = "musl")]
mod heap_object;
#[cfg(target_env = "musl")]
mod metadata;
#[cfg(target_env = "musl")]
mod overlays;
#[cfg(target_env = "musl")]
mod print;
#[cfg(target_env = "musl")]
mod protocols;
#[cfg(target_env = "musl")]
mod standard_protocols;
#[cfg(target_env = "musl")]
mod storage;
#[cfg(target_env = "musl")]
mod string;
#[cfg(target_env = "musl")]
mod swift_object;
#[cfg(target_env = "musl")]
mod type_lookup;
#[cfg(target_env = "musl")]
mod type_name;
#[cfg(target_env = "musl")]
mod weak;

#[cfg(target_env = "musl")]
pub use runtime::*;

#[cfg(target_env = "musl")]
mod runtime {
    use super::{
        cast, characters, conformance, failures, heap_object, metadata, overlays, print, protocols,
        standard_protocols, storage, string, swift_object, type_lookup, type_name, weak,
    };
    use crate::libsystem::{abort, stdio, write_error};

    pub use super::heap_object::{
        is_deinitializing, is_immortal, mark_weakly_referenced, swift_release as release,
        swift_retain as retain, try_retain, uses_swift_counting,
    };

    /// Sets the runtime up for a program that links it, after the Objective-C runtime: the
    /// `_SwiftObject` class, the descriptors and the empty singletons. False if there is no
    /// memory.
    pub fn set_up() -> bool {
        if !swift_object::define() {
            return false;
        }
        metadata::set_up();
        protocols::set_up();
        standard_protocols::set_up();
        string::set_up();
        storage::set_up();
        true
    }

    /// The address of the libswiftCore function or variable `name`, if the runtime has it.
    pub fn resolve(name: &[u8]) -> Option<u64> {
        if !(name.starts_with(b"_$s")
            || name.starts_with(b"_swift_")
            || name.starts_with(b"__swift")
            || name.starts_with(b"_OBJC_"))
        {
            return None;
        }
        heap_object::resolve(name)
            .or_else(|| metadata::resolve(name))
            .or_else(|| string::resolve(name))
            .or_else(|| storage::resolve(name))
            .or_else(|| protocols::resolve(name))
            .or_else(|| type_lookup::resolve(name))
            .or_else(|| print::resolve(name))
            .or_else(|| swift_object::resolve(name))
            .or_else(|| standard_protocols::resolve(name))
            .or_else(|| conformance::resolve(name))
            .or_else(|| cast::resolve(name))
            .or_else(|| weak::resolve(name))
            .or_else(|| characters::resolve(name))
            .or_else(|| failures::resolve(name))
            .or_else(|| type_name::resolve(name))
            .or_else(|| overlays::resolve(name))
    }

    fn report(prefix: &[u8], parts: &[&[u8]]) {
        stdio::flush_all();
        write_error(prefix);
        for part in parts {
            write_error(part);
        }
        write_error(b"\n");
    }

    /// Ends the program on a Swift runtime check, as Swift's `fatalError` does: the message on
    /// standard error, then `abort`.
    pub fn fatal_error(parts: &[&[u8]]) -> ! {
        report(b"Fatal error: ", parts);
        abort()
    }

    /// Ends the program on an access conflict at `address` that exclusivity checking found.
    pub fn fatal_access(address: &[u8]) -> ! {
        report(
            b"Simultaneous accesses to ",
            &[
                address,
                b", but modification requires exclusive access.\nFatal access conflict \
                  detected.",
            ],
        );
        abort()
    }

    /// Ends the program because it used a part of the Swift runtime EggOS lacks.
    pub fn unimplemented(parts: &[&[u8]]) -> ! {
        report(b"eggos-macos: unimplemented Swift runtime feature: ", parts);
        crate::linux::exit(127)
    }
}

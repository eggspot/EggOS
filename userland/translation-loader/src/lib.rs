//! What the Windows and macOS layers share to run an x86_64 program on an Arm computer
//! through ahead-of-time translation (D191): the translation cache over a directory
//! ([`cache_files`]: checked loading, atomic stores and evictions, with the disk directory in
//! `disk` on aarch64), what a trap in translated code means ([`trap`]), the code generation
//! mode from the CPU's capabilities, and where a layer's own code lies
//! ([`executable_range`]). On aarch64, `installed` maps a translation with its bridges,
//! makes each thread's guest state and calls into translated code through the entry
//! trampoline, and [`callback_thunks`] lays out the native thunks through which a layer's own
//! code calls a translated function it was given.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod cache_files;
pub mod callback_thunks;
#[cfg(all(target_env = "musl", target_arch = "aarch64"))]
pub mod disk;
#[cfg(all(target_env = "musl", target_arch = "aarch64"))]
pub mod installed;
mod program_headers;
pub mod trap;

pub use program_headers::executable_range;

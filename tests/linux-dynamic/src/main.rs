//! A dynamically linked Linux program from our own source (D167), which `cargo xtask
//! dynamic-linking` builds twice per architecture, against glibc and against musl, and boots
//! with their dynamic loaders and C libraries in the image. It uses the C library alone, no
//! Rust standard library: the C library's start file calls its `main`, which prints one line
//! with `printf` (the loader must have mapped and relocated the library and set up its thread
//! pointer for that) and returns 0, which the C library's `exit` makes the program's status.
//! On other hosts, and as a test target, it is an empty program, so the workspace builds
//! everywhere.

#![cfg_attr(all(target_os = "linux", not(test)), no_std, no_main)]

#[cfg(all(target_os = "linux", not(test)))]
mod program;

#[cfg(any(not(target_os = "linux"), test))]
fn main() {}

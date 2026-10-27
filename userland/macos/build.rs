//! Links the program as a static PIE with no C library on Linux targets.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("musl") {
        return;
    }
    for argument in ["-nostdlib", "-pie", "--no-dynamic-linker", "-z", "text"] {
        println!("cargo:rustc-link-arg-bins={argument}");
    }
}

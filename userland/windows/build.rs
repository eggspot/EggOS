//! Links the program as a static PIE with no C library on Linux targets, and sets
//! `runs_windows_programs` where the layer runs Windows programs: x86_64 and aarch64.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(runs_windows_programs)");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("musl") {
        return;
    }
    if matches!(
        env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86_64" | "aarch64")
    ) {
        println!("cargo::rustc-cfg=runs_windows_programs");
    }
    for argument in ["-nostdlib", "-pie", "--no-dynamic-linker", "-z", "text"] {
        println!("cargo:rustc-link-arg-bins={argument}");
    }
}

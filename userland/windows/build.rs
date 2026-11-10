//! Links the program as a static PIE with no C library in EggOS user builds (`eggos_user`),
//! and sets `runs_windows_programs` where the layer runs Windows programs: x86_64 and aarch64.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(runs_windows_programs)");
    if env::var_os("CARGO_CFG_EGGOS_USER").is_none() {
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

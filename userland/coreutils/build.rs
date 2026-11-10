//! Links the program as a static PIE with no C library in EggOS user builds (`eggos_user`).

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("CARGO_CFG_EGGOS_USER").is_none() {
        return;
    }
    for argument in ["-nostdlib", "-pie", "--no-dynamic-linker", "-z", "text"] {
        println!("cargo:rustc-link-arg-bins={argument}");
    }
}

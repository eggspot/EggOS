//! Links the kernel image with the linker script of the architecture being
//! built for. The scripts live with the rest of the architecture code, in the
//! HAL's `arch/<architecture>/link.ld`.

use std::env;
use std::path::PathBuf;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("cargo sets CARGO_CFG_TARGET_ARCH");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let script: PathBuf = [&manifest_dir, "..", "hal", "src", "arch", &arch, "link.ld"]
        .iter()
        .collect();
    println!("cargo::rerun-if-changed={}", script.display());
    println!("cargo::rustc-link-arg-bins=-T{}", script.display());
}

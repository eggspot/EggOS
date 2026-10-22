//! Links the kernel binary with its architecture's linker script from the HAL.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "none" {
        return; // Host build: the binary is a stub and links normally.
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("cargo sets CARGO_CFG_TARGET_ARCH");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let script = manifest_dir
        .join("../hal/src/arch")
        .join(&arch)
        .join("linker.ld");
    assert!(
        script.exists(),
        "no linker script for architecture `{arch}` at {}",
        script.display()
    );
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg-bins=-T{}", script.display());
    if arch == "x86_64" {
        // The 32-bit entry code uses absolute addresses, so the kernel must not be PIE. The
        // `relocation-model=static` flag in `.cargo/config.toml` does this too, but a RUSTFLAGS
        // environment variable silently replaces it; this keeps the link working either way.
        println!("cargo:rustc-link-arg-bins=--no-pie");
    }
}

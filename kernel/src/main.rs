//! The EggOS kernel image.
//!
//! On bare-metal targets this is the kernel binary; the architecture entry point lives in the
//! HAL. On the development host it is only a stub, so `cargo build` works everywhere.

#![cfg_attr(target_os = "none", no_std, no_main)]

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("eggos-kernel is a bare-metal binary: build it for a kernel target, not the host.");
    std::process::exit(1);
}

//! The workspace build's timer (D173): a Linux program built from our own source with Rust's
//! standard library and linked statically with musl, which runs the same build of
//! `tests/multi-crate` with the official toolchain inside EggOS (from `eggsh`) and inside Linux
//! (as its `/init`, in the same QEMU machine), so both are timed alike. It runs `rustc
//! --version`, `cargo build`, `cargo build` again with nothing to do and `cargo run`, with as
//! many jobs as it has CPUs, and writes a `BUILD-TIMER` line before and after each, the
//! program's output, and `BUILD-TIMER done`. As Linux's first process it mounts `/proc`, `/dev`
//! and `/tmp` first and powers the machine off at the end. On other hosts it is an empty
//! program, so the workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the Linux programs boot test adds to the image and runs from
//! `eggsh` (D155): it uses memory up four ways, through shared memory files, its own pages,
//! socket buffers and window buffers, and checks that EggOS refuses each with Linux's answer
//! while the kernel keeps its reserve, then that the memory comes back. It writes one
//! `LINUX-MEMORY` line for each check. On other hosts it is an empty program, so the workspace
//! builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

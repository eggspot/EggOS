//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the Linux programs boot test adds to the image and runs from
//! `eggsh` (D138): it acts as a server on a socket file and starts a copy of itself as its
//! client, and checks `AF_UNIX` stream sockets end to end, writing one `LINUX-SOCKETS` line
//! for each check. Only the server writes, so the lines come in a fixed order. On other hosts
//! it is an empty program, so the workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

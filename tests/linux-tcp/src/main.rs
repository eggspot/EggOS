//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the network boot test adds to the image and runs from `eggsh`
//! (D158): it checks `AF_INET` stream sockets through `std::net`, a client and a server over
//! the loopback network and, given a host and a port, with a TCP echo server on the host,
//! writing one `LINUX-TCP` line for each check. On other hosts it is an empty program, so the
//! workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

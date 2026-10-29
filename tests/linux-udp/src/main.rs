//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the network boot test adds to the image and runs from `eggsh`
//! (D157): it checks `AF_INET` datagram sockets through `std::net`, over the loopback network
//! and, given a host and a port, with a UDP echo server on the host, writing one `LINUX-UDP`
//! line for each check. On other hosts it is an empty program, so the workspace builds
//! everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the Linux programs boot test adds to the image and runs from
//! `eggsh`: it runs an event loop as GLib, Qt, SDL3 and winit do, with `epoll`, a `timerfd`, an
//! `eventfd`, a `signalfd` and a pipe a thread writes to; waits on a pipe with `select` and
//! `pselect6`; and checks what Linux promises of memory across `fork`: private pages copied on
//! write, `MAP_SHARED` anonymous memory and POSIX shared memory in `/dev/shm` shared, a writable
//! `MAP_SHARED` mapping of a `/tmp` file written back by `msync` and seeing the file's writes,
//! and `MADV_DONTFORK` and `MADV_WIPEONFORK`. It writes one `LINUX-EVENTS` line for each check.
//! On other hosts it is an empty program, so the workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

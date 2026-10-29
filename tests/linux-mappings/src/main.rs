//! A Linux program built from our own source with Rust's standard library and linked
//! statically with musl, which the Linux programs boot test adds to the image and runs from
//! `eggsh` (D167): it maps files with `mmap` and checks what Linux promises of them. A private
//! mapping reads the file, zeros after its end in the last page and `SIGBUS` past it; writes to
//! it stay the program's, and `MADV_DONTNEED` reads the file again; a shared read-only mapping
//! cannot be made writable; the program can map its own file; a child made by `fork` reads the
//! file's pages; `mremap` grows a mapping into the file; `msync` succeeds on a mapping and is
//! `ENOMEM` where nothing is mapped. It writes one `LINUX-MAPPINGS` line for each check. On
//! other hosts it is an empty program, so the workspace builds everywhere.

#[cfg(target_os = "linux")]
mod program;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    program::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

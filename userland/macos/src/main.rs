//! The macOS layer (D120), at `/usr/lib/eggos/macos`: the kernel starts it for a Mach-O or fat
//! file, and it runs that macOS console program natively, arm64 programs on aarch64 and x86_64
//! programs on x86_64. The Mach-O format, the fat slice, chained fixups and Darwin's numbers and
//! layouts are pure logic with host tests, and `printf`'s formatting is the shared
//! `eggos-c-format` crate's (D145); `start.rs` loads the
//! program, standing in for dyld, `libsystem/` implements the libSystem functions it imports,
//! and `objc/` and `foundation/` the Objective-C runtime and Foundation's core (D142).

#![cfg_attr(target_env = "musl", no_std, no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod calendar;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod darwin;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod fixups;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
use eggos_c_format as format;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod foundation;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod getopt;
#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod libsystem;
#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod linux;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod macho;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod objc;
#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod start;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod stream_buffer;
#[cfg(test)]
mod tests;

#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
eggos_runtime::main!(start::run);

#[cfg(all(
    target_env = "musl",
    not(any(target_arch = "aarch64", target_arch = "x86_64"))
))]
eggos_runtime::main!(unsupported);

#[cfg(all(
    target_env = "musl",
    not(any(target_arch = "aarch64", target_arch = "x86_64"))
))]
fn unsupported(_: &eggos_runtime::startup::Startup) -> i32 {
    eggos_runtime::syscall::write_all(2, b"macos: macOS programs run on aarch64 and x86_64 only\n");
    126
}

#[cfg(not(target_env = "musl"))]
fn main() {}

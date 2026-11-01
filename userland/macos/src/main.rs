//! The macOS layer (D120), at `/usr/lib/eggos/macos`: the kernel starts it for a Mach-O or fat
//! file, and it runs that macOS program natively, arm64 programs on aarch64 and x86_64
//! programs on x86_64, and x86_64 programs on aarch64 through ahead-of-time translation
//! (`translated.rs` and `translation.rs`, D191). The Mach-O format, the fat slice, chained
//! fixups, `LC_DYLD_INFO` opcodes, option parsing and Darwin's numbers and layouts are pure
//! logic with host tests, and `printf`'s formatting and `scanf`'s scanning
//! are the shared `eggos-c-format` crate's (D146, D190); `start.rs` loads the
//! program, standing in for dyld, `libsystem/` implements the libSystem functions it imports,
//! `objc/` and `foundation/` the Objective-C runtime and Foundation's core (D142),
//! `appkit/` AppKit for GUI apps on the window server (D147), drawing through `eggos-macos-gui`,
//! and `swift/` the Swift runtime of `libswiftCore` (D180).

#![cfg_attr(target_env = "musl", no_std, no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(any(test, all(target_env = "musl", target_arch = "aarch64")))]
extern crate alloc;

#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod appkit;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod calendar;
#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
))]
mod callback;
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
mod darwin_locale;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod darwin_signals;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod dyld_info;
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod fixups;
#[cfg(all(
    target_env = "musl",
    any(target_arch = "aarch64", target_arch = "x86_64")
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
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod info_plist;
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
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod spawn_actions;
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
#[cfg(any(
    test,
    all(
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )
))]
mod swift;
#[cfg(test)]
mod tests;
#[cfg(any(test, all(target_env = "musl", target_arch = "aarch64")))]
mod translated;
#[cfg(all(target_env = "musl", target_arch = "aarch64"))]
mod translation;

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

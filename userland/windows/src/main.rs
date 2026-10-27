//! The Windows layer (D118), at `/usr/lib/eggos/windows`: the kernel starts it for a file that
//! starts with `MZ`, and it runs that Windows console program natively on x86_64. The PE
//! format, the image's layout, relocations and imports, UTF-16 text and the command line are
//! pure logic with host tests; `start.rs` loads the program and `win32/` implements the Win32
//! functions it imports.

#![cfg_attr(target_env = "musl", no_std, no_main)]

#[cfg(any(test, all(target_env = "musl", target_arch = "x86_64")))]
mod command_line;
#[cfg(any(test, all(target_env = "musl", target_arch = "x86_64")))]
mod image;
#[cfg(any(test, all(target_env = "musl", target_arch = "x86_64")))]
mod library;
#[cfg(all(target_env = "musl", target_arch = "x86_64"))]
mod linux;
#[cfg(any(test, all(target_env = "musl", target_arch = "x86_64")))]
mod pe;
#[cfg(all(target_env = "musl", target_arch = "x86_64"))]
mod start;
#[cfg(any(test, all(target_env = "musl", target_arch = "x86_64")))]
mod text;
#[cfg(all(target_env = "musl", target_arch = "x86_64"))]
mod win32;

#[cfg(all(target_env = "musl", target_arch = "x86_64"))]
eggos_runtime::main!(start::run);

#[cfg(all(target_env = "musl", not(target_arch = "x86_64")))]
eggos_runtime::main!(unsupported);

#[cfg(all(target_env = "musl", not(target_arch = "x86_64")))]
fn unsupported(_: &eggos_runtime::startup::Startup) -> i32 {
    eggos_runtime::syscall::write_all(2, b"windows: Windows programs run on x86_64 only\n");
    126
}

#[cfg(not(target_env = "musl"))]
fn main() {}

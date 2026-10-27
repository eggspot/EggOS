//! The Windows layer (D118), at `/usr/lib/eggos/windows`: the kernel starts it for a file that
//! starts with `MZ`, and it runs that Windows console or GUI program (D129) natively on x86_64
//! (x64 programs) and aarch64 (ARM64 programs, D124). The PE format, the image's layout,
//! relocations and imports, UTF-16 text, the command line and the GUI's tables (`gui/`) are pure
//! logic with host tests; `start.rs` loads the program and `win32/` implements the Win32
//! functions it imports.

#![cfg_attr(target_env = "musl", no_std, no_main)]
#![cfg_attr(test, allow(dead_code))]

#[cfg(any(test, runs_windows_programs))]
mod calendar;
#[cfg(any(test, runs_windows_programs))]
mod command_line;
#[cfg(any(test, runs_windows_programs))]
mod errors;
#[cfg(any(test, runs_windows_programs))]
mod find;
#[cfg(any(test, runs_windows_programs))]
mod gui;
#[cfg(any(test, runs_windows_programs))]
mod image;
#[cfg(any(test, runs_windows_programs))]
mod library;
#[cfg(runs_windows_programs)]
mod linux;
#[cfg(any(test, runs_windows_programs))]
mod pe;
#[cfg(runs_windows_programs)]
mod start;
#[cfg(any(test, runs_windows_programs))]
mod text;
#[cfg(runs_windows_programs)]
mod win32;

#[cfg(runs_windows_programs)]
eggos_runtime::main!(start::run);

#[cfg(all(target_env = "musl", not(runs_windows_programs)))]
eggos_runtime::main!(unsupported);

#[cfg(all(target_env = "musl", not(runs_windows_programs)))]
fn unsupported(_: &eggos_runtime::startup::Startup) -> i32 {
    eggos_runtime::syscall::write_all(
        2,
        b"windows: Windows programs run on x86_64 and aarch64 only\n",
    );
    126
}

#[cfg(not(target_env = "musl"))]
fn main() {}

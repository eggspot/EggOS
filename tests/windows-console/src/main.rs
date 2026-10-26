//! An ordinary Windows console program, built from our own source for
//! `x86_64-pc-windows-msvc` and linked by the standard toolchain's `rust-lld` with its imports
//! from kernel32 and ntdll (no C runtime), that the Windows programs boot test adds to the
//! image with `cargo xtask image --add` and runs from `eggsh` (D118). It writes one
//! `WINDOWS-CONSOLE` line for each thing console programs use: its arguments (from
//! `GetCommandLineW`), the environment, its TEB and PEB through GS, a file read from the image,
//! a missing file's error, the heap and virtual memory, the clocks and `Sleep`, a large stack
//! frame (`__chkstk`), `LoadLibrary` and `GetProcAddress`, and standard error through
//! `WriteConsoleW`, then returns 0. `exit <code>` ends with `ExitProcess(code)` instead.

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
mod program;

#[cfg(any(not(windows), test))]
fn main() {}

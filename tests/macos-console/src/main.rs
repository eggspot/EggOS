//! An ordinary macOS console program, built from our own source for the `*-apple-darwin`
//! targets and linked by Apple's linker against libSystem, that the macOS programs boot test
//! adds to the image with `cargo xtask image --add` and runs from `eggsh` (D120). It writes one
//! `MACOS-CONSOLE` line for each thing console programs use through libSystem: its arguments
//! and `apple[]`, the environment, `printf`'s formats, strings and memory, numbers, a file, a
//! missing file's `errno`, `stat`, a directory listing, the working directory, the heap, the
//! clocks, standard error and `atexit`, then returns 0. `exit <status>` exits with that status
//! instead, `abort` aborts and `unimplemented` calls a function the layer lacks.

#![cfg_attr(all(target_os = "macos", not(test)), no_std, no_main)]

#[cfg(all(target_os = "macos", not(test)))]
mod program;

#[cfg(any(not(target_os = "macos"), test))]
fn main() {}

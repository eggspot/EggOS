//! An ordinary Android console program, built from our own source for the
//! `*-linux-android` targets and linked statically by the Android NDK's clang with bionic's own
//! start-up code and C library (`crtbegin_static.o`, `libc.a`), that the Android programs boot
//! test adds to the image with `cargo xtask image --add` and runs from `eggsh` (D119). It
//! writes one `ANDROID-CONSOLE` line for each thing console programs use through bionic: its
//! arguments, the environment, a file, a directory listing, `malloc`, the clocks, threads, two
//! system properties (D188), a log entry sent to logd's socket as liblog sends it (D188), the
//! auxiliary vector, `arc4random`, `system` (which runs
//! `/system/bin/sh`) and standard error, then returns 0. `exit <status>` exits with that
//! status instead, and `abort` aborts.

#![cfg_attr(all(target_os = "android", not(test)), no_std)]

#[cfg(all(target_os = "android", not(test)))]
mod program;

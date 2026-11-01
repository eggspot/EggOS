//! An ordinary Android console program, built from our own source for the
//! `*-linux-android` targets and linked by the Android NDK's clang with bionic's own start-up
//! code and C library: statically (`crtbegin_static.o`, `libc.a`) for the Android programs boot
//! test (D119), and dynamically, run by Android's `linker64` with the runtime module's
//! `libc.so`, for the dynamic Android programs boot test (D188). Each test adds it to the image
//! with `cargo xtask image --add` and runs it from `eggsh`. It writes one `ANDROID-CONSOLE` line
//! for each thing console programs use through bionic: its arguments, the environment, a
//! file, a directory listing, `malloc`, the clocks, threads, two system properties (D188), a
//! log entry sent to logd's socket as liblog sends it (D188), the auxiliary vector (its page
//! size, and whether `linker64` loaded it), `arc4random`, `system` (which runs
//! `/system/bin/sh`) and standard error, then returns 0. `exit <status>` exits with that
//! status instead, and `abort` aborts.

#![cfg_attr(all(target_os = "android", not(test)), no_std)]

#[cfg(all(target_os = "android", not(test)))]
mod program;

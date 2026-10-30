# Running Android programs

EggOS runs static Android console programs, natively: a program built with the Android NDK and
linked statically against bionic (Android's C library) runs through the same Linux system-call
layer as [Linux programs](linux-programs.md), its machine code directly on the CPU, with no
emulator and no virtual machine (D17, D119). Bionic's own start-up code (`crtbegin_static.o`)
and C library (`libc.a`) run unchanged; EggOS gives them what they read from a Linux kernel.

This is the first part of Android support: console programs, on aarch64 and x86_64. Android
apps (`.apk`), dynamically linked programs and anything that needs Android's system services
do not run yet (see below).

## Add a program to the image

```
cargo xtask image --arch aarch64 --add path/to/hello
cargo xtask run --arch x86_64 --add path/to/hello      # build that image and boot it
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello`. When an
added file is an Android program (it carries the `.note.android.ident` note the NDK puts in
every Android program), the image also gets `/system/bin/sh`, another name for `eggsh`, which
bionic's `system()` and `popen()` run (unless you add your own file there with `--as`). Images
without an Android program get no `/system` and no Android files (D18); only the kernel grew,
by 176 to 344 bytes, for the calls bionic needs (D119's "Cost"). At the prompt, type `hello` and its arguments; the shell
reports the exit status in `$?`, and an abort as `Aborted` and `134`.

## Build a program that runs

Use the NDK's clang for the image's architecture, linked statically (`-static`), with an API
level of 29 or later:

| Image | NDK target |
|-------|------------|
| aarch64 | `--target=aarch64-linux-android29` |
| x86_64 | `--target=x86_64-linux-android29` |

```
$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/<host>/bin/clang \
    --target=aarch64-linux-android29 -static -O2 -o hello hello.c
```

From Rust, the boot test's program (`tests/android-console`) shows one way: a `no_std` static
library for the official `aarch64-linux-android` or `x86_64-linux-android` target that defines
`main` and calls bionic's functions, linked by the NDK's clang as above
(`cargo xtask qemu` does it; `rustup target add aarch64-linux-android x86_64-linux-android`).
Programs on Rust's standard library for Android are not tested yet.

## What CI checks

On aarch64 and x86_64, CI links `tests/android-console` with the GitHub runner's NDK and runs
it from `eggsh` (`cargo xtask qemu`): its arguments, the environment (`getenv`), standard output
through bionic's stdio (`fputs`, `printf`) and standard error, reading a file (`fopen`,
`fgets`), listing a directory (`opendir`, `readdir`), `malloc`, `calloc` and `realloc` of small
and 1 MiB blocks, the monotonic and realtime clocks and `usleep`, four `pthread_create` threads
joined, `__system_property_get` (see below), `getauxval(AT_PAGESZ)`, `arc4random_buf`,
`system()` running a command through `/system/bin/sh`, `exit(42)` and `abort()`.

A machine without the NDK skips this test with a message (`cargo xtask qemu` says how to point
it at one: `ANDROID_NDK_HOME`, or `ANDROID_HOME` with the NDK in its `ndk/` folder); CI never
skips it. There is no Rust Android target with a prebuilt library for riscv64, so riscv64 has
no Android test.

On x86_64 the NDK's ABI requires SSSE3, SSE4.1, SSE4.2 and POPCNT (x86-64-v2: Intel Nehalem
(2008) and newer, AMD Bulldozer (2011) and newer); `cargo xtask run` and the release's run scripts give QEMU's CPU them. On
AArch64, programs may keep a tag in a pointer's top byte, as Linux's arm64 ABI allows (bionic's
allocator does).

## System properties

The kernel keeps Android's system properties in one property area in bionic's own format,
which `/dev/__properties__` shows as a read-only file of root's (D188): bionic maps it at
start-up, so `__system_property_get`, `__system_property_find`, `__system_property_foreach`
and `android_get_device_api_level()` work as on Android. At boot the kernel sets the properties
Android's `/system/build.prop` lists; `cargo xtask image --add` writes one into an image with an
Android program: Android 10 (`ro.build.version.sdk=29`), EggOS as brand, model and hardware, and
the CPU's ABI (`ro.product.cpu.abi`). Then:

```
# getprop ro.build.version.sdk
29
# setprop debug.level 1
# getprop
[debug.level]: [1]
[ro.hardware]: [eggos]
...
```

`getprop name [default]` prints a value (or the default, or an empty line), `getprop` alone
every property; `setprop name value` sets one (root only; a name is parts of letters, digits,
`_ - : @` between dots, a value at most 91 bytes), and a read-only property (`ro.`) keeps its
first value. Agents use the MCP tools `getprop` and `setprop` (the second needs the `write`
grant). A running program sees each page of the area as it was when it first read that page:
set a property before starting the program that reads it. The area holds up to 64 KiB.

The boot test sets a property with `setprop`, reads it with `getprop`, is refused a change to
`ro.build.version.sdk`, and the program prints both properties as bionic reads them.

## What does not work yet

- **Programs' own property changes.** `__system_property_set` (Android's socket to `init`'s
  property service) fails: set properties with `setprop`. Values longer than 91 bytes, property
  contexts (SELinux) and `persist.` properties kept across boots are not there.
- **Android apps and dynamically linked programs.** No `.apk`, no ART, no `linker64`: link
  statically.
- **Android's system services and logging.** No `logd` (`__android_log_print` is in
  `liblog.so`, which the NDK ships only as a shared library, so it comes with dynamically linked
  programs), no binder, no `/dev`, `/proc` or `/sys` (`sysconf(_SC_NPROCESSORS_ONLN)` and anything
  else that reads them falls back or fails).
- **Pipes.** `popen()` and anything else that needs a pipe fails; `system()` works.
- **What [Linux programs](linux-programs.md) lack** applies too: a read-only file system, no
  `mmap` of files, no sockets, a wall clock in UTC with no time zones (D150).
- **Tagged pointers in system calls (AArch64).** `prctl(PR_SET_TAGGED_ADDR_CTRL)` fails, so a
  program must not pass a pointer with a tag in its top byte to the kernel, as on a Linux
  without the tagged-address ABI; bionic then passes none.
- **Processes run as root**, not as an app's own user id.

# Running Linux programs

EggOS runs unmodified static Linux programs: its own kernel implements the Linux system calls
and process start-up those programs expect (D84, D103, D117). A program you built for Linux,
linked statically, runs from the `eggsh` prompt by its name once you add it to the image.
Programs linked with musl are what CI tests; static glibc programs use the same start-up but
are not tested yet.

## Add a program to the image

The image's only file system is the read-only initramfs built into it (D102), so programs go in
when the image is built:

```
cargo xtask image --arch aarch64 --add path/to/hello
cargo xtask image --arch x86_64 --add build/tool --as /usr/bin/tool --add notes.txt --as /etc/notes
cargo xtask run --arch riscv64 --add path/to/hello      # build that image and boot it
```

- `--add <file>` puts the file at `/bin/<file's name>`, which is on the shell's `PATH`;
  `--as <path>` right after it puts it at that path instead. Repeat both for more files.
- Added files are executable (`0755`), owned by root. A path already in the image (`/init`,
  `/bin/eggsh`, `/bin/ls`, ...) is refused, as are a path under
  a file, a path with an empty, `.` or `..` part, a name over 255 bytes and a path of 4096
  bytes or more.
- `image` writes `target/image/eggos-<arch>.elf` (`--arch all` writes all three), a release
  image with your files, to boot with QEMU's `-kernel` and the arguments `cargo xtask run`
  uses; `run` takes the same `--add` options, builds that image and boots it.

Then, at the prompt, type its name: `hello`, or `tool --help` with `/usr/bin` on `PATH`
(`export PATH=$PATH:/usr/bin`). The shell reports the exit status in `$?` as on Linux, and a
program killed by a signal as `128` plus the signal (`134` and `Aborted` for an abort).

## Build a program that runs

Build for the image's architecture with the standard Linux toolchains, statically linked:

| Image | Rust target | Static C toolchain (musl) |
|-------|-------------|---------------------------|
| aarch64 | `aarch64-unknown-linux-musl` | `aarch64-linux-musl-gcc -static` |
| riscv64 | `riscv64gc-unknown-linux-musl` | `riscv64-linux-musl-gcc -static` |
| x86_64 | `x86_64-unknown-linux-musl` | `x86_64-linux-musl-gcc -static` |

For Rust, `rustup target add <target>`, then
`RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target <target>` (on a
host without a Linux linker, add `-C linker=rust-lld -C link-self-contained=yes`, as EggOS's
own build does). Static executables and static PIEs both run: Rust's x86_64 musl target makes
a static PIE, its aarch64 and riscv64 targets a static executable at a fixed address.

What CI checks on all three architectures (`tests/linux-static`, a std-only Rust program,
built with `-C relocation-model=static` and with `-C relocation-model=pie`): its arguments and
environment, standard output and standard error, reading a file, listing a directory, the
monotonic and realtime clocks, a `HashMap` with random keys, threads spawned and joined, the
number of CPUs it may use (`std::thread::available_parallelism`), its exit status, and a panic
that aborts it.

## What does not work yet

- **Dynamically linked programs.** One that names an interpreter (`/lib/ld-linux-*.so`,
  `/lib/ld-musl-*.so`) fails with `No such file or directory`, as on Linux when the loader is
  missing. Link statically.
- **Starting other programs from a program.** Rust's `std::process::Command` and C's
  `posix_spawn`, `system` and `popen` create a pipe first (to report a child's start-up
  failure, or to read its output), and EggOS has no pipes yet, so they fail;
  `fork`, `vfork`, `execve` and `wait4` themselves work.
- **Files you write.** The file system is read-only: there is no `/tmp`, and writing or
  creating a file fails with `EROFS`. There is no `/proc` (so `std::env::current_exe` fails)
  and no `/dev`.
- **The wall clock** reads 1970 without an RTC driver.
- **Networking, sockets and `mmap` of files** are not implemented (`mmap` takes anonymous
  memory only).
- **Choosing CPUs.** `sched_getaffinity` reports every online CPU; `sched_setaffinity` is not
  there yet.

The roadmap lists these as next capabilities; [limitations](limitations.md) has the rest.

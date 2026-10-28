# Running Linux programs

EggOS runs unmodified static Linux programs: its own kernel implements the Linux system calls
and process start-up those programs expect (D84, D103, D117). A program you built for Linux,
linked statically, runs from the `eggsh` prompt by its name once you add it to the image.
Programs linked with musl are what CI tests; static glibc programs use the same start-up but
are not tested yet.

## Add a program to the image

The image's root file system is the read-only initramfs built into it (D102), so programs go
in when the image is built (or come from a host folder, below):

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

## Or run it from a folder on your computer

Without building an image: put the program in a folder and boot with that folder attached
(`sh run-eggos.sh --apps <folder>`, `.\run-eggos.ps1 -Apps <folder>` or `cargo xtask run
--arch <arch> --apps <folder>`). EggOS shows it read-only at `/media/host` (D130), so the
program runs by its path: `/media/host/hello`, or `cd /media/host` and `./hello`. Files on the
host folder are all executable. A program run this way is read into memory first and must be
smaller than 4 MiB; the folder's own limits are in [limitations](limitations.md).

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

## GUI programs (Wayland)

A Linux program that opens its windows through Wayland shows them on the EggOS desktop
(D140). `eggway`, EggOS's own Wayland server, starts with the system when there is a display
and listens on `/run/user/0/wayland-0`; every program started from the terminal or the
desktop has `XDG_RUNTIME_DIR=/run/user/0` and `WAYLAND_DISPLAY=wayland-0`, so a client finds
it as it would on Linux. Each top-level window (`xdg_toplevel`) becomes a desktop window with
EggOS's own title bar and controls: move it, resize it from its edges, maximise, minimise or
close it as any window, and its clicks, wheel and keys reach the program. `eggway --status`
(at `/usr/libexec/eggway`) says whether the server runs; `wm list` lists the windows.

What eggway serves: `wl_compositor` (version 4), `wl_shm` (1; `ARGB8888` and `XRGB8888`
buffers in memory files), `wl_surface` (attach, damage, frame callbacks at up to 60 Hz,
commit), `wl_seat` (5; pointer and keyboard, with a US keyboard map), `wl_output` (3) and
`xdg_wm_base` (3; `xdg_surface`, `xdg_toplevel` with title, minimum and maximum size,
maximise, fullscreen as maximise, minimise, close, and move and resize from a press in the
window). Surfaces are shown opaque, at scale 1.

`tests/linux-gui` is a complete example: `linux-paint` is a static Rust program that speaks the
wire protocol itself; the Linux GUI boot test opens its window, clicks, types, drags, maximises
and closes it on every architecture.

### What real toolkits need next

GTK, Qt, SDL, browsers and Electron apps do not run yet. What they need, roughly in the order
it unlocks them:

| Need | Who needs it |
|------|--------------|
| Dynamic linking (`ld.so`, `libwayland-client.so`, `libxkbcommon.so`, glibc) | every distribution's binaries |
| `epoll`, `eventfd`, `timerfd`, `signalfd` | GLib and GTK, Qt, SDL3, winit/calloop, Chromium and Electron, EFL |
| `fork` of a process with shared memory | foot, weston-terminal |
| `MAP_SHARED` of tmpfs files, `shm_open` and `/dev/shm` | older clients |
| `wl_subcompositor` | GTK, Qt, libdecor (SDL) |
| `xdg-decoration` (server-side decorations) | apps that would draw their own title bars |
| `xdg_popup` placement | menus and tooltips |
| `wl_data_device_manager` and the primary selection | copy and paste, drag and drop |
| cursor surfaces and `cursor-shape-v1` | the program's own pointer shapes |
| `wp_viewporter`, fractional scale, buffer scales above 1 | scaling |
| `linux-dmabuf` and a GPU driver stack | EGL and Vulkan through Mesa |
| `text-input-v3` | input methods |
| `xdg-activation` | apps that raise their own windows |
| relative pointer and pointer constraints | games |
| `/etc/fonts` and D-Bus with the desktop portals | most toolkit apps |
| seccomp, namespaces and `clone3` | browser sandboxes |
| alpha compositing | translucent windows |

## What does not work yet

- **Dynamically linked programs.** One that names an interpreter (`/lib/ld-linux-*.so`,
  `/lib/ld-musl-*.so`) fails with `No such file or directory`, as on Linux when the loader is
  missing. Link statically.
- **`fork` after `MAP_SHARED`.** Rust's `std::process::Command`, `posix_spawn`, `fork`,
  `vfork`, `execve` and `wait4` work (the sockets boot test starts its client with `Command`),
  but a process with shared memory cannot `fork` (D104).
- **`/proc/self`.** `/proc` has only `cmdline`, `meminfo`, `uptime`, `ps` and `net/unix`, so
  `std::env::current_exe` fails: start a copy of the program by its path. Files can be written
  in `/tmp`, `/run`, `/home` and `/var` (D125, D131).
- **The wall clock** reads 1970 without an RTC driver.
- **Networking and `mmap` of files** are not implemented (`mmap` takes anonymous memory
  only, and memfds, D139). Local sockets work: `AF_UNIX` stream sockets, `socketpair` and
  descriptors passed with `SCM_RIGHTS` (D138), and so does memory shared between programs
  with `memfd_create` and `mmap(MAP_SHARED)` (D139); datagram sockets and `epoll` do not
  yet. Wayland GUI programs work through `eggway` (D140, above), the toolkits not yet.
- **Choosing CPUs.** `sched_getaffinity` reports every online CPU; `sched_setaffinity` is not
  there yet.

The roadmap lists these as next capabilities; [limitations](limitations.md) has the rest.

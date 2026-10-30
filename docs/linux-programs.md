# Running Linux programs

EggOS runs unmodified Linux programs: its own kernel implements the Linux system calls and
process start-up those programs expect (D84, D103, D117). A program you built for Linux runs
from the `eggsh` prompt by its name once you add it to the image: linked statically, or
dynamically with its dynamic loader and libraries beside it (D167, below). Static programs
linked with musl and dynamically linked ones against glibc and musl are what CI tests.

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

## Dynamically linked programs

A dynamically linked program names its dynamic loader (`PT_INTERP`: glibc's
`/lib64/ld-linux-x86-64.so.2`, `/lib/ld-linux-aarch64.so.1` or
`/lib/ld-linux-riscv64-lp64d.so.1`, musl's `/lib/ld-musl-<arch>.so.1`). EggOS loads that
loader beside the program, at a base of its own below the stack, and starts it with
`AT_BASE`, `AT_ENTRY` and `AT_PHDR` set as Linux sets them (D167); the loader then maps the
libraries itself with `mmap` of their files. Put the loader at the path the program names and
the libraries where the loader looks (glibc: the program's run path, `/lib/<triplet>`, `/lib`
or `/usr/lib`; musl: `/lib`, `/usr/local/lib` or `/usr/lib`), for example:

```
cargo xtask run --arch aarch64 --add hello --add /usr/aarch64-linux-gnu/lib/ld-linux-aarch64.so.1 \
    --as /lib/ld-linux-aarch64.so.1 --add /usr/aarch64-linux-gnu/lib/libc.so.6 --as /lib/libc.so.6
```

There is no vDSO (`AT_SYSINFO_EHDR`), so the C libraries make their clock calls as system
calls. A missing loader is `No such file or directory`, as on Linux; one EggOS cannot run (not
an ELF program for this machine) is `ELIBBAD`.

What CI checks on all three architectures (`cargo xtask dynamic-linking`, its own job):
`tests/linux-dynamic`, our own program that uses the C library alone, built against glibc
(with the Ubuntu cross compilers' libraries) and against musl (with Alpine's), runs with its
loader and `libc` in the image, prints through `printf` and exits with 0.

## Files mapped into memory

`mmap` of a regular file works (D167). A private mapping (`MAP_PRIVATE`) reads each page from
the file when the program first touches it, into memory of its own: writes stay the
program's, `MADV_DONTNEED` reads the file again, and a page wholly past the file's end raises
`SIGBUS` (`BUS_ADRERR`), as on Linux; code can be mapped executable, which is how a loader maps
a library. A shared mapping (`MAP_SHARED`) of a regular file is read-only here: its pages are
the file as it was read, it can never be made writable (`mprotect` is `EACCES`), and a
writable one is `ENODEV`. `msync` succeeds on mapped ranges (nothing waits to be written back).
`tests/linux-mappings` checks all of this in CI.

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
window). Since D172 also: `wl_subcompositor` (1; subsurfaces drawn over their parent at their
position, synchronized or not, stacked with `place_above` and `place_below`),
`xdg_popup` (menus and tooltips placed by their `xdg_positioner`, flipped, slid or shrunk to
stay inside their toplevel's window, drawn into it, with `grab`, `reposition` and
`popup_done` on a click outside), `zxdg_decoration_manager_v1` (1; always server-side: EggOS
draws the title bar), cursor surfaces (`wl_pointer.set_cursor`; a null one hides the pointer
over the window) and `wp_cursor_shape_manager_v1` (1; the text, resize and default shapes
EggOS draws), and `wp_viewporter` (1; a source rectangle and a destination size, scaled to
the nearest pixel). A toplevel's subsurfaces and popups are drawn from the buffers they hold,
blended by their alpha over the toplevel, which is shown opaque, at scale 1. Since D176 also:
`wl_data_device_manager` (3; copy and paste: the app with the keyboard focus is offered the
selection with every MIME type, and reading it passes its pipe to the app that copied; drag
and drop is refused), `zwp_primary_selection_device_manager_v1` (1; the same for what is
selected), `xdg_activation_v1` (1; a token from the app with the keyboard focus raises and
focuses a window once, within ten seconds), `zwp_text_input_manager_v3` (1; the focused text
field joins the window's element tree with the text around its cursor, a password's left out;
no input method sends text yet) and KDE's `org_kde_kwin_server_decoration_manager` (1;
always the server's, so GTK 3 draws no title bar of its own). An app's own environment comes
from its manifest (`environment=`, D176), and `/etc/fonts/fonts.conf` points fontconfig at
`/usr/share/fonts`, `~/.local/share/fonts` and the fonts installed apps carry
(`/Applications/.fonts`).

A dynamically linked program installs with its loader and libraries beside it: put them in a
`.zip` with the program (`app install`, [apps](apps.md)), and it opens through them. The
first real apps that run this way are Brogue CE, an SDL game, and Mousepad, a GTK 3 editor
([compatibility](compatibility.md)).

`tests/linux-gui` is a complete example: `linux-paint` is a static Rust program that speaks the
wire protocol itself; the Linux GUI boot test opens its window, clicks, types, drags, maximises
and closes it on every architecture.

### What real toolkits need next

SDL apps run (D172) and GTK 3 apps run (D176, [compatibility](compatibility.md)); Qt,
GTK 4, browsers and Electron apps do not yet. What they need, roughly in the order it unlocks them:

| Need | Who needs it |
|------|--------------|
| The toolkits' libraries in the image (`libwayland-client.so`, `libxkbcommon.so`) beside glibc or musl; dynamic linking itself works (D167) | every distribution's binaries |
| (`epoll`, `eventfd`, `timerfd`, `signalfd`: written, D169) | GLib and GTK, Qt, SDL3, winit/calloop, Chromium and Electron, EFL |
| (`fork` of a process with shared memory: written, D169) | foot, weston-terminal |
| (writable `MAP_SHARED` of tmpfs files, `shm_open` and `/dev/shm`: written, D169) | older clients |
| (`wl_subcompositor`: done, D172) | GTK, Qt, libdecor (SDL) |
| (`xdg-decoration`, server-side decorations: done, D172) | apps that would draw their own title bars |
| (`xdg_popup` placement: done inside the toplevel's window, D172) | menus and tooltips |
| (`wl_data_device_manager` and the primary selection: copy and paste done, D176); drag and drop | copy and paste, drag and drop |
| (cursor surfaces and `cursor-shape-v1`: done with EggOS's shapes, D172); a cursor surface's own image, the hand and busy shapes | the program's own pointer shapes |
| (`wp_viewporter`: done, D172); fractional scale, buffer scales above 1 | scaling |
| `linux-dmabuf` and a GPU driver stack | EGL and Vulkan through Mesa |
| (`text-input-v3`: served, D176); an input method to send text through it | input methods |
| (`xdg-activation`: done, D176) | apps that raise their own windows |
| relative pointer and pointer constraints | games |
| (`/etc/fonts`: done, D176); D-Bus with the desktop portals | most toolkit apps |
| seccomp, namespaces and `clone3` | browser sandboxes |
| alpha compositing | translucent windows |

## What does not work yet

- **Libraries for dynamically linked programs** come only with the program: EggOS ships no
  C library, so add the loader and the libraries a program needs to the image at the paths
  the loader looks in (D167), or install the program from a `.zip` holding them, which it
  then opens through (D172).
- **`fork` with odd shared memory.** Rust's `std::process::Command`, `posix_spawn`, `fork`
  (copy on write, D169), `vfork`, `execve` and `wait4` work, and shared memory stays shared
  across `fork`; only a process that mapped `MAP_SHARED` anonymous memory with no access or
  executable cannot `fork` (`ENOSYS`, D104).
- **Parts of `/proc` and `/dev`.** `/proc/self` and `/proc/<pid>` have `cmdline`, `exe`,
  `fd`, `maps`, `stat` and `status`, so `std::env::current_exe`, glibc's `$ORIGIN` and
  `pthread_getattr_np` work, and `/dev` has `null`, `zero`, `full`, `random`, `urandom`, `tty`,
  `console`, `fd` and the standard streams (D168); there is no `/proc/<pid>/task`, `environ`
  or `cwd`, and no `/sys`. Files can be written in `/tmp`, `/run`, `/home` and `/var`
  (D125, D131).
- **Links, scripts and removed files** (D170). `symlink`, `symlinkat`, `link`, `linkat`,
  `readlink`, `chmod`, `fchmod`, `fchmodat` and `fchmodat2` work on `/tmp`, `/home`, the work
  disk and (links only, to read) the initramfs; `openat2` with `RESOLVE_NO_SYMLINKS`; a file
  that starts with `#!` runs its interpreter with one optional argument, as Linux; a file
  removed while it is open, mapped or running stays until its last close.
- **The wall clock** is UTC, read from the real-time clock at boot; there are no time zones (D150).
- **Shared file mappings.** A writable `MAP_SHARED` of a file shares the file's pages with
  every other such mapping and with `read` and `write` (D169), but its pages are all read when
  it is made and read as zeros past the file's end instead of raising `SIGBUS`; a read-only
  one of a file nobody maps writable does not see later writes (D167). Event loops work:
  `epoll`, `eventfd`, `timerfd`, `signalfd`, `select` and `pselect6` (D169). Local sockets
  work: `AF_UNIX` stream sockets, `socketpair` and descriptors passed with `SCM_RIGHTS`
  (D138), and so does memory shared between programs with `memfd_create`, `shm_open` on
  `/dev/shm` and `mmap(MAP_SHARED)` (D139, D169). The network works (D157, D158,
  [network.md](network.md)): `AF_INET` UDP sockets (`std::net::UdpSocket`), ping sockets and
  TCP (`std::net::TcpStream` and `TcpListener`: `connect`, `listen`, `accept`, `poll`,
  `shutdown`, timeouts), and names through musl's resolver; IPv6 and `AF_UNIX` datagrams do
  not yet. Wayland GUI programs work through `eggway` (D140, above), and SDL apps (D172) and GTK 3
  apps (D176) on it; Qt not yet. `sysinfo` and `fadvise64` work (D176); `inotify`, `chown`,
  `copy_file_range`, `splice` and extended attributes do not yet.
- **Choosing CPUs.** `sched_getaffinity` reports every online CPU; `sched_setaffinity` is not
  there yet.

The roadmap lists these as next capabilities; [limitations](limitations.md) has the rest.

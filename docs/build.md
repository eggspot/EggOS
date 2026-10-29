# Build, test and CI configuration

What the configuration files do and why. They carry no comments themselves (D66); this page
holds what used to be written in them.

## Toolchain (`rust-toolchain.toml`)
- **Pinned** so every host (macOS, Windows, CI) builds the kernel with the same compiler and
  lints.
- **Kernel targets**: `aarch64-unknown-none-softfloat`, `riscv64imac-unknown-none-elf` and
  `x86_64-unknown-none`, none of which lets the compiler use FP/SIMD registers in the kernel
  (D54).
- **Test-program targets** (D84): `aarch64-unknown-linux-musl`, `riscv64gc-unknown-linux-musl`
  and `x86_64-unknown-linux-musl` build Linux programs from our own Rust source, with Rust's std,
  for the QEMU tests: static executables linked by `rust-lld` with the targets' own
  self-contained C library and start files (`-C linker=rust-lld -C link-self-contained=yes -C
  target-feature=+crt-static`), so they link the same way on macOS, Windows and Linux with no C
  toolchain. They are test fixtures, never part of EggOS.

## Cargo workspace (`Cargo.toml`)
- **Lints.** `clippy::undocumented_unsafe_blocks` is denied, so every `unsafe` block carries a
  `// SAFETY:` comment (repo rule). `clippy::multiple_unsafe_ops_per_block` is denied, so each
  `// SAFETY:` comment justifies exactly one operation.
- **Release profile** (`lto = true`, `codegen-units = 1`): what we measure and ship
  (`CLAUDE.md`, "The target: the fastest OS"). Both profiles use `panic = "abort"`. Release
  builds keep their symbols (for `llvm-objdump` and `llvm-nm`) but no debug information
  (`strip = "debuginfo"`): DWARF was 2.5 MB of each 3-4 MB image and carried the builder's
  checkout path, which made `release_image_bytes` depend on where the repository lives (D116).

## Cargo configuration (`.cargo/config.toml`)
- **`cargo xtask`** is an alias that runs the `xtask` package: host-side developer commands
  written in Rust, so they work the same on macOS, Windows and Linux.
- **x86_64 kernel: `relocation-model=static`.** The x86_64 entry code runs 32-bit code that uses
  absolute addresses, so the kernel is linked as a static, non-PIE executable at its load
  address (the target defaults to static PIE). A `RUSTFLAGS` environment variable replaces
  these flags, so `kernel/build.rs` also passes `--no-pie` to the linker: the kernel still links,
  as non-PIE, when `RUSTFLAGS` is set.
- **Linux test-program targets: `rust-lld`, `link-self-contained=no`, `relocation-model=pie`**
  (D85), for programs on our own runtime, which links no C library or start files. `cargo
  xtask` passes each test program's flags in `CARGO_ENCODED_RUSTFLAGS` (D86, D88), which
  `RUSTFLAGS` cannot override, so their flags do not depend on these `rustflags` lines or on
  the environment (the `linker` lines still apply).

## Kernel features (`kernel/Cargo.toml`)
- **`qemu-test`**: power the machine off after boot, so the QEMU test runner sees the verdict.
- **`qemu-test-fatal-exception`**: after the self-tests pass, write to the kernel's read-only
  data, to test that it faults and that the fatal exception is reported with the fail marker
  (the QEMU runner then expects that failure, at that address).
- **`qemu-bench`**: after the self-tests pass, run the boot-time benchmarks and print their
  results, which `cargo xtask bench` reads (it boots with `-icount shift=0`).
- **`qemu-boot-clock`**: the release kernel, plus one line: the clock ticks from the start of
  `boot` to the first time a program waits for console input (the shell at its first prompt),
  written once as `EGGOS-BENCH: boot_to_prompt ...`, which `cargo xtask bench` reads (D116).

## Developer commands (`xtask/`)
- **`cargo xtask ci`** runs CI's checks in CI's order, cheapest first. **`lint`** fails on a
  comment the repository's rules disallow in a tracked `.rs`, `.toml` or `.yml` file, beyond the
  per-file counts in `xtask/lint-legacy-files.txt` (the files cleanup PRs have not reached yet:
  the list only shrinks, `lint --shrink` lowers it after a cleanup). **`clippy-targets`**
  lints the user crates as compiled for each architecture's Linux target and the kernel crates
  for each bare-metal target (where the architecture modules a host build cannot see are
  compiled), without features and with each test feature, since they change code paths; the
  user programs the test kernels embed are built first.
- **Our Linux programs' flags.** A program on our own runtime (init, the shell, the core
  utilities, the runtime's self-test program) links no C library or start files, as a static
  PIE (D85); the ones the release initramfs holds also drop debug information and symbols. A
  program on Rust's standard library (the std test program) links the toolchain's own musl and
  start files statically, as a static PIE where the target has one, without debug information
  or symbols (D84, D86). The kernel build finds each test program through an environment
  variable naming its path.
- **`cargo xtask qemu`** boots, for each architecture: the self-test kernel, which must pass
  every self-test with the std program reading the console as the runner types (D100); the
  fatal-exception kernel, which must report a write to read-only data after its self-tests
  (D29), both with the `.bss` probe poisoned so the `.bss` self-test can fail; the release
  kernel with a test init whose login program, the std program, ends and init powers off, then
  resets the machine, which ends QEMU under `-no-reboot` (D106); init starting the shell, which
  the runner drives on the serial console (D107); and last the release kernel itself, booted to
  the shell's prompt, where the runner runs the core utilities, reports the time from QEMU's
  start to the first prompt, and powers off with `poweroff` (D108). Before that last boot, the Linux
  programs test boots the release image with `tests/linux-static` added as `/bin/linux-static`
  (built with `-C relocation-model=static`) and `/bin/linux-static-pie` (built with `-C
  relocation-model=pie`: a static PIE on x86_64), each with `-C link-self-contained=yes -C
  target-feature=+crt-static` in a target directory of its own, and types commands at the prompt
  that run them, by name or by path; each command's `LINUX-STATIC` lines, the panic's message,
  `Aborted` and the exit statuses must appear in order before the next prompt (D117). Then the
  program windows test boots the release image with `userland/hello-window` added as
  `/usr/libexec/hello-window`, with its display, types `ls /dev` and a pipeline of two of them,
  and through QMP clicks the first window to raise it, drags it by its title bar, hovers and
  drags its corner to resize it by (100, 60), maximises and restores it by its control and by
  double-clicks on its title bar, cycles the windows with Alt+Tab, closes the first with Alt+F4
  and the second with its close control, comparing the display, the pointer included, with the
  host's render of the same desktop after each step, and runs `wm list` (D127, D128). Before
  it, the host folder test writes `target/host-folder-<arch>/` (`readme.txt` and
  `tests/linux-static`'s static build as `Programs and Tools/linux-static`), boots the release
  image with it attached as `--apps` attaches one (`qemu::host_folder_args`: QEMU's `vvfat` on
  the virtio block device) and types commands that list it at `/media/host`, read the file
  (also through its short name), `cd` into the subfolder, run the program by its relative path
  and try `>`, `mkdir` and `rm`; the mount's boot line, each command's lines, the program's
  `LINUX-STATIC` checks and `Read-only file system` for each change must appear in order (D130).
  A boot that has not powered off within 30 seconds (90 for the benchmarks) has hung.
- **The Windows programs boot test** adds `tests/windows-console`'s two programs, built for
  `x86_64-pc-windows-msvc` with `rust-lld`, as `/bin/windows-console.exe` and
  `/bin/windows-unsupported.exe` (with the Windows layer on x86_64) and types three commands:
  on x86_64 each `WINDOWS-CONSOLE` line, the exit codes 0 and 44 (`ExitProcess(300)`) and the
  refusal of the unsupported import with status 126 must appear in order; on aarch64 and
  riscv64, `Exec format error` and 126 (D118).
- **The Windows GUI boot test** adds `tests/windows-gui`'s `win-paint.exe`, built for this
  CPU's Windows target (x64's on riscv64) with `rust-lld`, `/SUBSYSTEM:WINDOWS` and
  `/ENTRY:WinMainCRTStartup` in a target directory of its own, as `/bin/win-paint.exe` with the
  Windows layer, boots with the display and runs it with its output redirected to
  `/tmp/win-paint.log`. On x86_64 and aarch64 it compares the display with the host's render
  (the desktop, and the program's scene and the message box painted by `eggos-windows-gui`)
  as the window appears, after a click on its ellipse, a key, Space (the message box), Enter,
  F2 (a second thread's message box), Enter and a click on the close control, then checks `WIN-PAINT status 0` and the program's lines
  in the file; on riscv64, `Exec format error` and 126 (D129).
- **The Windows C runtime boot test** (x86_64 and aarch64) adds `tests/windows-crt`'s
  `eggpad.exe`, our own C notepad built by MSVC's `cl.exe` as a `/MD` program (x64's on x86_64,
  ARM64's on aarch64), as `/bin/eggpad.exe` with the Windows layer, boots with the display,
  writes `/tmp/note.txt` from the shell and runs `eggpad.exe 'C:\tmp\note.txt'` with its output
  redirected to `/tmp/eggpad.log`. It compares the display with the host's render (eggpad's
  lines and status painted by `eggos-windows-gui`'s text) as the file appears, after typing
  `abc` and Enter, after Ctrl+S (`Saved 27 bytes`) and after a click on the close control, then
  checks `EGGPAD status 0`, eggpad's lines in the log and the saved file's two lines. The
  programs come from `EGGOS_WINDOWS_PROGRAMS` (a folder of `<program>-<x64|arm64>.exe` files)
  or, on a Windows host with Visual Studio's C++ tools, `cargo xtask windows-programs` builds
  them (`vswhere`, `vcvarsall.bat`, `cl /MD /O2 /W3 /WX`); otherwise the test is skipped with a
  message, but fails when `CI` is set. CI builds them in its `windows-programs` job; in the
  `qemu` jobs xtask waits for that artifact (up to 25 minutes, `xtask/src/ci_artifact.rs`) just
  before this boot test, so the MSVC build overlaps the kernels' build and the earlier boot tests
  (D146).
- **The Android programs boot test** (aarch64 and x86_64) builds `tests/android-console`, a
  `no_std` static library defining `main`, for `aarch64-linux-android` or
  `x86_64-linux-android`, links it with the Android NDK's clang (`--target=<triple>29 -static`,
  bionic's `crtbegin_static.o` and `libc.a`), adds it as `/bin/android-console` with
  `/system/bin/sh`, and types three commands: each `ANDROID-CONSOLE` line (including the line
  `system()` runs through `/system/bin/sh`), the statuses 0 and 42, `Aborted` and 134 must
  appear in order. The NDK comes from `ANDROID_NDK_HOME`, `ANDROID_NDK_LATEST_HOME`,
  `ANDROID_NDK_ROOT` or the newest version under `$ANDROID_HOME/ndk` (or `ANDROID_SDK_ROOT`);
  without one the test is skipped with a message, but fails when `CI` is set. CI mounts the
  runner's NDKs into the `qemu` jobs' container and sets `ANDROID_HOME` (D119).
- **The macOS programs boot test** adds `tests/macos-console`'s programs, linked by Apple's
  linker for `aarch64-apple-darwin` and `x86_64-apple-darwin` (`MACOSX_DEPLOYMENT_TARGET=13.0`,
  chained fixups), as `/bin/macos-console` (this CPU's), `/bin/macos-unsupported` (links
  `libz`), `/bin/macos-other` (the other CPU's) and `/bin/macos-fat` (a fat file of both,
  written by xtask), with the macOS layer on aarch64 and x86_64, and types seven commands:
  each `MACOS-CONSOLE` line, the statuses 0, 42 and 3, `Aborted` and 134, the unimplemented
  function's message and 127, and the two refusals with 126 must appear in order; on riscv64,
  `Exec format error` and 126. The programs come from `EGGOS_MACOS_PROGRAMS` (a folder of
  `<program>-<cpu>` files) or, on a macOS host, `cargo xtask macos-programs` builds them;
  otherwise the test is skipped with a message, but fails when `CI` is set. CI builds them in
  its `macos-programs` job; in the `qemu` jobs xtask waits for that artifact just before the
  first boot test that needs it (D120, D146).
- **The macOS GUI boot test** (`xtask/src/qemu/macos_gui.rs`, aarch64 and x86_64) takes
  `mac-paint`, our AppKit test app built from `tests/macos-gui/main.m` with the other macOS
  programs (`-framework AppKit -framework Foundation`), writes `mac-paint.app` (its
  `Info.plist` and this CPU's slice as `Contents/MacOS/mac-paint`) in a host folder, and boots
  the release image itself, which carries the macOS layer (D152), on a blank disk with that
  folder attached. It installs
  the bundle with `app install /media/host/mac-paint.app`, opens it from the launcher, then
  compares the display with the host's render (the desktop, and the window and alert painted by
  `eggos-macos-gui-tests`) after each step: the window with its menu in the top bar, `Hello` and
  Enter, clicks in the paint view, `c`, `Clear`, `Info` and Esc, the `File` menu and Esc, and
  Cmd+Q, which must end the app with status 0. It is skipped like the macOS programs boot test
  (D147).
- **The Text Editor boot test** (`xtask/src/qemu/text_editor.rs`, every architecture) boots
  the release kernel with no disk, writes `/tmp/notes.txt` at the prompt and opens it with
  `edit`, then compares the display with the host's render (the desktop, and the editor's
  window drawn by `eggos-editor` over a file system in memory holding the same file, with the
  caret shown or hidden) after typing, Ctrl+F and `egg`, Esc, Ctrl+S and the close control;
  `cat` must then print the saved text (D149).
- **The real-time clock boot test** (`xtask/src/qemu/real_time.rs`, every architecture) boots
  the release kernel, whose real-time clock follows the host's (QEMU's `-rtc base=utc`), and
  checks that the boot log names the expected driver (`rtc-cmos`, `rtc-pl031`, `rtc-goldfish`),
  that `date +%s` is within 5 minutes of the host's clock, that a file made in `/tmp` has that
  time (`date -r`), that `hwclock -r` agrees, and that `date -s` reaches both `date` and the
  hardware clock (D150). Every desktop frame check accepts the bar's clock at the uptime or at
  the host's time give or take a few minutes.
- **`cargo xtask image`** builds the release image with the `--add <file> [--as <path>]` files in
  its initramfs (`/bin/<name>` by default, mode `0755`; a path already in the image, under a file,
  with an empty, `.` or `..` part, a name over 255 bytes or a path of 4096 bytes or more is
  refused) and writes `target/image/eggos-<arch>.elf`; `cargo xtask run` takes the same options
  ([Linux programs](linux-programs.md), D117). Every release image, with or without added
  files, carries the Windows layer at `/usr/lib/eggos/windows` on x86_64 and aarch64
  ([Windows programs](windows-programs.md), D118) and the macOS layer at `/usr/lib/eggos/macos`
  on aarch64 and x86_64 ([macOS programs](macos-programs.md), D120), as a v0.3.0 exception to
  D18 until the component manager (D152); so does every test image built on the release
  initramfs. Adding an Android program (an ELF file with the `.note.android.ident` note) adds
  `/system/bin/sh`, another name for `eggsh` ([Android programs](android-programs.md), D119).
- **The reboot boot test** (`xtask/src/qemu/reboot.rs`, every architecture) boots the release
  image with `qemu::interactive_args`' serial arguments, as `run-eggos.sh --serial` does,
  types `reboot` at the prompt and must see the restart, a second boot and its prompt in the
  same QEMU, then powers off with `poweroff` (D153).
- **The install, Files and MCP boot tests boot the release image itself**, the kernel `cargo
  xtask release` copies to `target/release-dist/`, with no `--add`: they install
  `win-paint.exe` (x86_64 and aarch64) or the static Linux program (riscv64) from the host
  folder and open them, and the MCP test, through `run-eggos.sh --mcp --no-window --apps`,
  installs and captures `win-paint` (x86_64 and aarch64) and captures `linux-paint`; the macOS
  GUI boot test does the same with `mac-paint.zip` (D152).
- **`cargo xtask sample-apps --arch <arch>`** builds our own apps for people to try installing
  into `target/sample-apps/<arch>/` and prints that folder, for the run scripts' `--apps`:
  `win-paint.exe` and `win-controls.exe` (x86_64 and aarch64, `tests/windows-gui`),
  `linux-paint` (`tests/linux-gui`, a static Wayland program) and, where Apple's linker or
  `EGGOS_MACOS_PROGRAMS` provides the Mach-O programs, `mac-paint.zip` and `mac-paint.app`
  (aarch64 and x86_64). They are test programs built from our sources, never release assets
  (D152).
- **What the runner matches.** The lines it looks for are the ones the kernel and the test
  programs write, and must be changed with them: the self-tests' `EGGOS-SELFTEST: PASS` and
  `FAIL` markers and `EGGOS-CONSOLE: input ready` (`eggos_kernel::selftest`,
  `eggos_kernel::console`), the user-mode lines `EGGOS-USER: ...` (`eggos_hal::user`,
  `eggos_kernel::selftest`, `userland/selftest`, `userland/std-selftest`), the std program's
  `EGGOS-CONSOLE: type ...` requests and what is typed for each (`console::request` in
  `userland/std-selftest`), `kernel exception: ` and the write-protect probe's line
  (`eggos_kernel::exception`, `eggos_kernel::selftest`), the boot banner `EggOS <version>
  (<arch>)`, `boot: input keyboard pointer` and the input self-test's `EGGOS-INPUT: send
  pointer and key events` (`eggos_kernel::boot`, `eggos_kernel::devices::selftest`, D113), and
  init's, the shell's and the utilities' lines in the D106 to D108 sessions.
  What it types answers a request each time the request appears as a whole line: after
  `EGGOS-CONSOLE: input ready`, `eggos_kernel::selftest::CONSOLE_INPUT_SCRIPT`, which a
  self-test checks arrives through the receive interrupt; then the std program's requests, among
  them lines edited with backspace, delete, `^W`, `^U` and `^D`, whose echo (each erased
  character rubbed out with backspace, space, backspace) the runner checks. The shell sessions
  type a line at each prompt instead, as a person would, so the echo never mixes with what a
  command writes.
- **QEMU's arguments.** Every boot has 128 MiB and 4 CPUs, so the secondary CPUs start (D40;
  `eggos_kernel::selftest::QEMU_CPUS` must match), and never `-no-reboot` unless a test wants a
  reset to end QEMU: with it an x86 triple fault (in the power-off path, say) ends QEMU with
  status 0, which looks like a clean power-off; without it the machine resets and prints the
  banner again, which fails the test. The `.bss` probe (`eggos_selftest_bss_probe`) is filled
  with `0xa5` bytes by one QEMU `loader` device per 8-byte word, which QEMU applies at reset
  after placing the image and before any firmware runs (observed on QEMU 10 on all three
  architectures, not a documented guarantee: a QEMU upgrade that changes it shows up as `.bss`
  failing). QEMU hands over zeroed memory, so without this the `.bss` self-test would pass even
  if the entry point never cleared `.bss`.
- **Devices** (D110, D111). Every run (tests, benchmarks, `cargo xtask run`) gets a virtio
  entropy device, `-device virtio-rng-device` on the Arm and RISC-V `virt` machines and
  `-device virtio-rng-pci,disable-legacy=on` (modern only) on q35, which the self-tests read.
  The `virt` machines also get `-global virtio-mmio.force-legacy=false`: QEMU (10.2 included)
  still makes virtio-mmio devices legacy (version 1) by default, which EggOS refuses. A release
  kernel has no entropy driver yet, so that device only shows in its device count.
- **Display** (D112). Every run but the two init boots gets a virtio GPU asking for 1280x800:
  `-device virtio-gpu-device,xres=1280,yres=800` on the `virt` machines, and `-vga none
  -device virtio-gpu-pci,disable-legacy=on,xres=1280,yres=800` on q35 (without `-vga none`,
  QEMU's window would show q35's standard VGA first). The tests stay headless with `-display
  none`, which hides the window but keeps the device. The two init boots run without it, to
  check that a kernel without a display behaves as before (`boot: no display`). The
  boot-to-prompt test also opens QEMU's machine protocol (QMP) on a free local TCP port and, at
  the first prompt and again after the line typed on the virtio keyboard, saves the display with
  `screendump` (`target/screendump-<arch>.ppm`) and checks it is the graphical terminal (D114)
  exactly as the host renders it: the console's bytes after `boot: terminal 158x49` fed to the
  same terminal engine (`eggos_drivers::terminal`), compared pixel for pixel, retried for up to
  5 s while the kernel's drawing thread catches up; on a mismatch the expected picture is
  saved next to it (`target/screendump-<arch>-expected.ppm`). The self-test kernels check the
  release kernel's first frame (pixel hash `eggos_kernel::display::FIRST_FRAME_HASH_1280X800`)
  and a terminal screen with SGR colours (`eggos_kernel::terminal::TEST_SCREEN_HASH_1280X800`).
  `cargo xtask run` shows the display in a
  window and the serial console on the terminal (`-serial mon:stdio`, so Ctrl-A then X still
  quits); `--nographic`, anywhere among the options, keeps both on the terminal for a machine
  without a screen; `--apps <folder>` attaches a host folder, which EggOS mounts read-only at
  `/media/host` (D130).
- **Persistent disk** (D131). `cargo xtask run` attaches `target/image/eggos-<arch>-disk.img`,
  created empty and 1 GiB sparse if missing (`File::set_len`: sparse on APFS and ext4; on NTFS
  the space is reserved), which EggOS formats with EggFS at its first boot and mounts at
  `/home` and `/Applications`. `--disk <image>` uses another image, `--no-disk` none, and
  `--format-disk` empties the image first, erasing everything on it: the only way to reuse a
  disk that holds something else, which EggOS never formats on its own. The run scripts take
  the same options (`-Disk`, `-NoDisk`, `-FormatDisk` in PowerShell) and keep the image next
  to themselves; `run-eggos.sh` creates it with `dd ... seek=`, `run-eggos.ps1` with
  `qemu-img create -f raw` from QEMU's folder (sparse), else `SetLength`. The persistent files
  boot test (`xtask/src/qemu/persistent.rs`) boots on images in `target/qemu-disks` and checks
  them afterwards with the `eggos-eggfs` crate.
- **Input** (D113). Every run with the display also gets a virtio keyboard, tablet and mouse:
  `-device virtio-keyboard-device -device virtio-tablet-device -device virtio-mouse-device` on
  the `virt` machines, the `-pci` devices with `disable-legacy=on` on q35; the kernel prints
  `boot: input keyboard pointer`, which the boot-to-prompt test requires. Every boot with them
  but the benchmarks opens QMP: the self-test kernels write `EGGOS-INPUT: send pointer and key
  events` (`eggos_kernel::devices::selftest::INPUT_EVENTS_LINE`) and the runner answers with
  `input-send-event` reports 100 ms apart (the tablet at `TABLET_POSITION`, a left click, the
  mouse moved by `MOUSE_MOTION`, a wheel notch up, Shift pressed and released; the constants
  must match the self-test's), and the boot-to-prompt test types `uname -a` and `echo
  EGGOS-KEYBOARD Typed_On+Virtio:OK` on the virtio keyboard instead of the serial console, a
  report per key 10 ms apart and at most 8 keys ahead of their echo, so the device's 64-event
  queue never fills between the kernel's polls, however slow the machine. In `cargo xtask
  run`, QEMU's window types on the virtio keyboard and points with the tablet.
- **Releases** (D116). **`cargo xtask release`** builds the release kernel of every
  architecture, runs the boot-to-prompt test on it (the time it reports goes into the notes),
  and writes to `target/release-dist/` (under `CARGO_TARGET_DIR` when set): the image as
  `eggos-<version>-<arch>.elf` (the version is `[workspace.package]`'s), `SHA256SUMS` in
  `sha256sum`'s format (xtask's own SHA-256), `run-eggos.sh` and `run-eggos.ps1`, and
  `RELEASE-NOTES.md`: what's new, written from the subjects of the pull requests merged on
  `main` since the last `v*` tag that say what users can now do (`git log --first-parent`, so
  the release needs the history and its tags), each image's size and boot-to-prompt time, what
  runs, installing an app, the keys, the MCP grants and what they do not protect, and the known
  limits (D153). The scripts' QEMU arguments are `qemu::interactive_args`, the ones `cargo
  xtask run` uses, with no `-no-reboot`, so `reboot` restarts EggOS in the same QEMU (D153), and
  with `-nographic` for `--serial` (`-Serial`), `qemu::host_folder_args` for `--apps <folder>` (`-Apps`, or `--apps`, in
  PowerShell), with the folder made absolute and its commas doubled, and `qemu::disk_args`
  for the disk image; with `--mcp` (`-Mcp`) `qemu::mcp_args`: the agent channel on the
  script's standard input and output (`qemu::agent_channel_args`), the console in
  `eggos-<arch>-serial.log`, no monitor, and with `--no-window` (`-NoWindow`) no display or
  input device (`Arch::offscreen_args`, `-display none`) and the kernel command line word
  `qemu::OFFSCREEN_WORD`, `eggos.display=offscreen`, so EggOS draws its desktop in memory
  (D144), every script message on standard error, and the disk image checked with `qemu-io`
  for another EggOS's lock, and `-no-reboot`, so a reboot ends the MCP session (D136, D153,
  docs/mcp.md); with `--mcp-allow <list>` (`-McpAllow`,
  `--mcp` only) the word `qemu::grant_word`, `eggos.mcp_allow=<list>`, once each name is
  checked as `view`, `write`, `run` or `install` (D137, D143). The words go in one `-append`
  (`qemu::command_line_args`), since QEMU keeps only the last; host tests check the scripts'
  text. `run-eggos.ps1` reads its arguments itself (`$args`: PowerShell's `-Name` and the shell
  script's `--name` spellings alike, no `param` block), so an unknown or misplaced one prints
  one usage line on standard error and exits with 2, as `run-eggos.sh` does (D153). Each script picks the host's architecture
  unless one is named, checks the image against `SHA256SUMS` with `sha256sum`, `shasum` or
  `Get-FileHash` (refusing a mismatch, warning when no tool or no `SHA256SUMS` is found), and looks for
  `qemu-system-<arch>` on `PATH` (and under `Program Files\qemu` on Windows). They are the one
  kind of shell and PowerShell file in the repository, generated, never committed, and hold no
  comments. **`cargo xtask screenshot --arch <arch>`** boots the release image headless with
  its display, types `uname -a` and `ls /bin` at the prompts, checks at the last prompt that
  the display shows the desktop exactly as the host renders it, then saves QMP's
  `screendump` as `docs/images/desktop.png` (8-bit RGB, each row with the PNG filter whose
  bytes sum smallest, DEFLATE with LZ77 and the fixed Huffman codes: about 27 KiB at
  1280x800).
- **JSON.** xtask reads the path of each executable cargo built from cargo's JSON messages with
  just enough JSON for them (string escapes such as Windows paths' `\\`).

## CI (`.github/workflows/ci.yml`, D8, D36, D122)
- **Parallel jobs, a final `check`.** `changes` detects a docs-only change; then, at the same time:
  `fast` (format, `cargo xtask lint`, clippy on the host and the kernel targets, docs, host unit
  tests), `qemu` (the boot tests, one job per architecture), `bench` (the benchmarks, one job per
  architecture), `macos-programs` (the Mach-O test programs), `windows-programs` (the MSVC test
  programs, D146) and, on PRs, `hosts` (macOS and Windows). Runners are free for this public repository, so the goal is wall-clock time: the run
  takes as long as its slowest job.
- **`check` is the only required status check on `main`.** It is the last job (`needs` every other
  job, `if: always()`) and fails if any of them failed or was cancelled; a skipped job passes. The
  workflow always runs: a workflow skipped by a `paths` filter never reports, and would leave docs
  PRs pending for ever. `changes` reads the PR's changed files (Markdown, `docs/`, `LICENSE-MIT`,
  `LICENSE-APACHE`, `.github/FUNDING.yml` do not count as code, hence `pull-requests: read`); on a
  docs-only change every other job is skipped and `check` reports success in seconds.
- **Jobs run in the CI image** (below), with QEMU, OpenSBI and the pinned toolchain preinstalled
  (`packages: read` to pull it). CI always runs in `main`'s image, so a change that needs something
  new in the image lands in its own PR first. The `qemu` jobs mount the runner's Android NDKs for
  the Android programs boot test (D119) and, on PRs, fetch the `macos-programs` and
  `windows-programs` artifacts through xtask just before the boot tests that run them, waiting
  for them when their jobs are still building (D120, D146); their 40-minute timeout covers the
  boot tests plus a Windows job that takes its whole 15 minutes.
- **Benchmarks are report-only (D122).** Each `bench` job runs `cargo xtask bench --arch <arch>
  --report-only`: every result is compared with `benchmarks/baseline.txt` and written to the job
  summary and to `target/bench/report.json` and `report.md`; the job fails only for a result more
  than 2x slower than its baseline. The two files are uploaded as the artifact
  `bench-pr-<number>-<head sha>-<arch>` (`bench-main-<sha>-<arch>` on pushes to `main`) and kept
  for 90 days, so a regression found by the weekly performance pass can be traced to its PR. The
  exact gate stays the default of `cargo xtask bench` and `cargo xtask ci`. Image sizes measured
  on Linux differ from a Mac's by a few hundred bytes: they are reported, and only the 2x rule
  blocks.
- **Cache.** Each job restores the newest `target/` cache of its own kind (`fast`, `qemu-<arch>`,
  `bench-<arch>`), falling back to the older shared cache; only pushes to `main` save one, so PR
  commits do not each upload their own. There are no dependencies, so only `target/` is worth
  caching.
- **Hosts.** Development hosts are macOS and Windows, so on PRs into `main`, when code changed, a
  small job on each builds, lints and tests the workspace and builds the kernel for every
  architecture.
- **Billing**: the global rules in `~/.claude/CLAUDE.md` (CI section) and D8: concurrency
  cancels superseded PR runs only, every job has a timeout, checkouts are shallow.

## CI image (`.github/ci-image/Dockerfile`, `.github/workflows/ci-image.yml`, D36)
- **What it is.** Ubuntu 24.04 with QEMU, OpenSBI and the pinned Rust toolchain installed, so a
  run neither downloads QEMU from the Ubuntu mirror (normally 15 s, sometimes over 20 minutes)
  nor installs the toolchain (about 12 s). The QEMU packages are Ubuntu 24.04's, as
  `benchmarks/baseline.txt` requires. Besides QEMU it holds `git` (for `actions/checkout`),
  `zstd` (`actions/cache`'s compression, as on GitHub's runners), `gcc` and `libc6-dev` (the
  linker for host builds) and `gh` (the docs-only check). The toolchain, targets and components
  are the ones `rust-toolchain.toml` pins.
- **How it is built.** `ci-image.yml` builds it, with the repository root as the build context,
  and pushes it to `ghcr.io/eggspot/eggos-ci:main`, where `ci.yml` pulls it. It runs only when the
  image's inputs change (or by hand), so it bills a few minutes per toolchain or package change,
  not per PR. On a PR, or a manual run on another branch, it builds and checks the image; on
  `main` it also pushes. Only `main`'s image is ever pushed.
- **Layers** are reused from the current image (`--cache-from`, which fetches only the layers
  it reuses), so a toolchain bump does not reinstall QEMU. Without registry access (a PR) the cache import logs a 403 "failed to
  configure registry cache importer" error, which is expected, and the image builds from
  scratch.
- **Timeout 30 minutes**: a rebuild installs QEMU from the Ubuntu mirror, which has stalled for
  over 20 minutes.
- **The check step** mounts the checkout into the image, so the checkout keeps no credentials.
  It runs in the checkout so that rustup picks the toolchain `rust-toolchain.toml` pins, as CI
  does, and fails if that toolchain is not the image's own (rustup must not install it there).
- **Concurrency**: a superseded PR build is cancelled; a push to the registry is never cut off
  halfway.

## Other repository files
- **`.github/dependabot.yml`** keeps the GitHub Actions used by CI current; EggOS has no
  third-party crates to update.
- **`.github/FUNDING.yml`** shows the "Sponsor" button on the repository page
  (<https://github.com/sponsors/eggspot>). Other platforms (Open Collective, Liberapay, a custom
  URL) are added there once their accounts exist; each takes the account name, not a URL.

# EggOS roadmap

Status keys: **done**, **in progress**, **next**, planned (no marker).
Updated by every build-loop run; the order is refined as we learn (reasons in `docs/decisions.md`).

## v0.3.0 plan (next, owner 2026-10-26, D99)

The first interactive release, 2026-11-01. Until it ships this list comes before the milestones'
own order. Feature freeze: 2026-10-30 07:00.

Must-have, in order:
1. **done** — device interrupt routing (GIC SPIs, PLIC, I/O APIC) and UART receive (D98, M3).
2. **done** — console `read` with a line discipline (echo, erase, ^C/^D, `TCSETS*`, `POLLIN`),
   and an xtask QEMU test that feeds scripted serial input when the test program asks (D100).
3. **done** — a descriptor table per process (`close`, `dup`, `dup2`, `dup3`, `fcntl`,
   `close_range`), the console an ordinary open file description, `readv` and `O_NONBLOCK`
   reads (D101).
4. **done** — VFS core and a read-only initramfs (cpio, embedded by xtask): `openat`, `read`,
   `pread64`, `lseek`, `getdents64`, `fstat`/`newfstatat`/`statx` on files, `getcwd`, `chdir`,
   `fchdir` (D102). Later slices of issue #94: tmpfs, devfs, symbolic links, the dentry cache.
5. **done** — `execve` and `execveat` from a file: a new process with the caller's id, handlers
   reset, close-on-exec descriptors closed, other threads ended (D103).
6. **done** — process creation: `clone` without `CLONE_THREAD`, `fork` and `vfork`, the
   memory copied at once or shared with `CLONE_VM`, the parent sleeping until a `vfork` child
   execs or ends, children's exit records kept for `wait4` (D104). Follow-up: copy-on-write,
   and with it shared anonymous memory across `fork` (refused with `ENOSYS` until then).
7. **done** — `wait4` and `waitid`, zombies kept in a process table until reaped, orphans
   reparented (nearest subreaper, then init, then the kernel), `SIGCHLD`, `setpgid`,
   `getpgid`, `setsid`, `getsid`, `TIOCSCTTY`/`TIOCGPGRP`/`TIOCSPGRP`/`TIOCGSID`, `^C` to the
   console's foreground group (D105). Follow-ups: stop and continue (`WUNTRACED`, `SIGTTIN`,
   `SIGTTOU`), `kill` to other processes and groups (init is named as pid 1 by item 8).
8. **done** — init as pid 1: the release kernel starts every CPU and runs `/init` from the
   initramfs as process 1, the process orphans go to, instead of halting; `userland/init`
   starts the login program from `/etc/eggos/init.conf` (default `/bin/eggsh`, a banner and
   reaping only while it is missing) in its own session with the console, reaps every child,
   respawns it with a back-off or takes the machine down as configured; `reboot(2)` powers
   off, halts and resets through the HAL on all three, parking the other CPUs first; a QEMU
   test per architecture boots the release kernel into a test init that powers off, and
   another that resets (D106). Follow-ups: init's shutdown signals once `kill` reaches other
   processes, a protected init (`SIGKILL` and default actions not delivered to pid 1).
9. **done** — the `eggsh` shell (`userland/eggsh`, at `/bin/eggsh` in the release kernel's
   initramfs, which init starts): a prompt, quoting, `$` expansion, assignments and an
   environment prefix, `;` and comments, `PATH` search, the builtins `cd`, `pwd`, `exit`,
   `help`, `export`, `unset`, `set`, `echo`, `type`, `true`, `false`, `:` and `.`, `eggsh file`
   and `eggsh -c`, POSIX exit statuses (127, 126, 128 plus the signal); each program in a
   process group of its own that has the console, so `^C` reaches only it, and `^C` at the
   prompt a fresh prompt; pipes, redirections, `&&`, `||`, `&` refused as not supported yet
   (item 16); a QEMU test per architecture drives a session on the serial console (D107,
   docs/shell.md). Follow-ups: globbing, `$(...)`, functions and control flow, line editing
   and history.
10. **done** — the core utilities as one multi-call program (`userland/coreutils`, at
    `/bin/coreutils` with `cat`, `echo`, `false`, `halt`, `ls`, `poweroff`, `reboot`, `sleep`,
    `true` and `uname` as hard links to it, which the initramfs now supports: one copy of the
    data), and a boot-to-prompt QEMU test on all three architectures in CI: the release kernel
    boots to the shell's prompt, the test runs each utility and powers off with `poweroff`, and
    reports the time to the first prompt (D108, docs/coreutils.md). Follow-ups: `ls` in
    columns on a terminal and with user names, more utilities (`mkdir`, `rm`, `cp` once there
    is a writable file system), `poweroff` through init.
11. **done** — design done (D109): UI design (Claude Fable, D16) for the boot screen,
    the terminal and a minimal desktop is in `docs/ux/desktop-v0.3.md` (with `terminal-v0.3.md`
    and the font `font-8x16.txt`), including the rendering plan and the acceptance criteria for
    items 11-14. Part 1 (D110, D111, docs/drivers.md): the driver model, device discovery
    (device tree on aarch64 and riscv64, MCFG and PCI ECAM on x86_64), device windows, DMA
    regions and the polled virtio core (virtio-mmio version 2, modern virtio-pci, the split
    virtqueue), proven by virtio-rng on all three. Part 2 (D112): a polled virtio-gpu 2D driver
    on all three, in the release kernel: the display's mode, a `B8G8R8X8` framebuffer on scanout
    0, damage-tracked fenced transfers and flushes, the kernel's display API, the first frame
    (`wallpaper.top`) and `boot: display 1280x800`; `cargo xtask run` shows it in a window, and
    the boot-to-prompt test checks it through QMP's `screendump`. Moved to item 13, which needs
    them first (D112): the drawing crate, the boot screen's logo, name and progress with its
    `fb: boot <hash>` line, and the cursor queue (item 14's pointer).
12. **done** — virtio-input (D113, docs/drivers.md): a polled driver for QEMU's virtio
    keyboard, tablet and mouse on all three, in the release kernel; the terminal design's US
    keymap (xterm's encoding, Caps Lock, Ctrl, Alt, key repeat at 400 ms then every 40 ms)
    turns keys into bytes that enter the console's line discipline exactly as UART bytes do;
    keys, pointer motion (tablet positions scaled to the display, mouse motion clamped to it),
    buttons and the wheel go into a lock-free input event queue of 256 for the desktop;
    `boot: input keyboard pointer`. The boot-to-prompt test types two of its lines on the
    virtio keyboard through QMP `input-send-event`, and a self-test checks the pointer,
    button, wheel and key events QMP sends. Polled from a thread that exists only with an
    input device (8 ms while active, 32 ms idle) until device interrupts (issue #97 slice 3).
    Follow-ups: device interrupts, the keyboard layout setting, LEDs (the status queue).
13. **done** — graphical terminal (D114, docs/drivers.md): the Egg Mono 8x16 font (the design
    file's 95 ASCII glyphs, checked by a host test, and the box drawing, block, shape and arrow
    extras) and the terminal emulator of `terminal-v0.3.md` in `eggos-drivers` (cell grid,
    256-line scrollback with Shift+Page Up/Down, Home, End and the wheel, the ANSI subset with
    SGR's 16 colours, bold, underline and inverse, the alternate screen, DSR and DA replies,
    UTF-8 to glyph ids); the release kernel shows the console's session on the whole display
    (158x49 cells at 1280x800, 126x47 at 1024x768), mirrored from the serial console, rendering
    only damaged cells and moving pixels on scrolls, flushed at most every 16 ms by a drawing
    thread; `boot: terminal 158x49`. The boot-to-prompt test compares QEMU's `screendump` pixel
    for pixel with the host's render of the serial bytes at the first prompt and after the line
    typed on the virtio keyboard; a self-test checks an SGR screen's hash on all three.
    Follow-ups: the boot screen (logo, name, progress, `fb: boot` hash) moved to item 14, which
    replaces it with the desktop at the first prompt; the cursor's blink (item 14's frame tick);
    `TERM=xterm` in init's environment; `SIGWINCH`.
14. **done** — users can now see EggOS's own desktop in the QEMU window (D115): the
    procedural wallpaper (gradient, egg and yolk), the top bar with the egg mark and `EggOS`,
    an uptime clock and a power button whose menu powers off or restarts, and the shell in an
    80x24 terminal window with a rounded border, shadow, title bar and controls. The mouse
    focuses the window, drags it by its title bar and minimises it to a chip in the bar that
    restores it; keys reach the shell while the window is focused; the serial console works
    alongside, and `TIOCGWINSZ` is 80x24 like the serial side; `boot: terminal 80x24`. The
    boot-to-prompt test compares QEMU's `screendump` pixel for pixel with the host's render of
    the desktop and the serial bytes at the first prompt, then minimises, restores and drags
    the window through QMP and checks each frame, on all three architectures. Next, as user
    capabilities: users can double-click the title bar to maximise the terminal; users see a
    pointer drawn by the GPU's cursor queue and hover highlights on the bar and controls;
    users see a boot screen with the logo and progress before the desktop; users can drag a
    window partly off screen; users see the wall clock once an RTC driver sets it (done, D150); users with a display smaller than
    658x458 get a full-screen terminal; console output never waits for the desktop's repaint
    (D115's review follow-ups).
15. **done** — users can now download one image per architecture, `SHA256SUMS` and a run
    script, run it, and boot EggOS in a QEMU window with its desktop and terminal, or on the
    serial console alone with `--serial`, at an `eggsh` prompt (D116): `cargo xtask release`
    builds and boots the release image of each architecture and writes
    `eggos-<version>-<arch>.elf`, `SHA256SUMS` (our own SHA-256), `run-eggos.sh`,
    `run-eggos.ps1` (QEMU's arguments generated from `cargo xtask run`'s, the image checked
    before it boots) and a `RELEASE-NOTES.md` skeleton with each image's size and boot-to-prompt
    time; `cargo xtask bench` gates `boot_to_prompt` (guest time from kernel entry to the
    shell's first prompt, deterministic under `-icount`) and `release_image_bytes` (exactly);
    `cargo xtask screenshot` saves the desktop as a PNG from our own encoder; README "Try
    EggOS" and `docs/limitations.md`. The shipped images carry symbols but no debug
    information (`strip = "debuginfo"`), about 5x smaller. Next, as user capabilities: users on
    Windows get a run script tested on
    Windows in CI; users see the release's what's-new written from the merged PRs.
16. **done** — users can now run unmodified static Linux programs from `eggsh` (D117,
    docs/linux-programs.md): `cargo xtask image --arch <arch> --add <file> [--as <path>]` builds
    the release image with the files in its initramfs (`/bin/<name>` by default) and writes
    `target/image/eggos-<arch>.elf`, and `cargo xtask run` takes the same options;
    `sched_getaffinity` reports the online CPUs, so `std::thread::available_parallelism` no longer
    says 1. A QEMU test per architecture adds an ordinary std-only Rust program, built by the
    standard musl toolchain as a static executable and with `relocation-model=pie` (a static PIE on
    x86_64), and runs it at the prompt, by name and by path: arguments, environment, standard
    output and error, a file, a directory listing, the clocks, a `HashMap`, threads, the CPU count,
    exit statuses and an aborting panic. Next, as user capabilities: users' programs can start
    other programs (`std::process::Command`, `posix_spawn`, `system`, `popen`) once pipes exist;
    users can run static glibc programs, tested in CI; users can choose a program's CPUs
    (`sched_setaffinity`); users can add programs to a running system once there is a writable file
    system; static PIEs built for aarch64 and riscv64 are tested too.
17. **done** — Android: users can now run Android console programs from `eggsh` on aarch64 and
    x86_64 (D119, docs/android-programs.md): a static program built with the Android NDK, with
    bionic's own start-up code and C library, runs natively through the Linux layer, which
    gained `sched_getscheduler` and `sched_getparam`, AArch64's top-byte ignore for user
    addresses and `eggsh -c --`; QEMU's x86_64 CPU gains the SSE4.2 and POPCNT the NDK's ABI
    requires; `cargo xtask image --add` gives an image with an Android program
    `/system/bin/sh` (`eggsh`, the shell `system()` runs) and leaves other images unchanged.
    System properties all read as unset (no property area). A QEMU test on aarch64 and x86_64,
    run in CI with the runner's NDK, links a program from our own source against bionic and
    checks stdio, files, directories, the heap, clocks, threads, properties, `getauxval`,
    `arc4random`, `system()`, exit statuses and `abort`. Next, as user capabilities: users'
    programs can read system properties (a property service); users can run Android programs
    on riscv64 and dynamically linked ones; then apps (`.apk`).
18. **done** — Windows, part 1: users can now run Windows console programs from `eggsh` on
    x86_64 (D118, docs/windows-programs.md): an unmodified PE32+ console program runs
    natively through the Windows layer, an optional component at `/usr/lib/eggos/windows` that
    `cargo xtask image --add` installs with the first Windows program; the kernel's `execve`
    starts it for a file that starts with `MZ` (elsewhere, and without the layer, `ENOEXEC`).
    The layer checks and maps the image (preferred base or relocated), binds its kernel32 and
    ntdll imports to our own `extern "win64"` functions (console, files, heap, virtual memory,
    command line, environment, time, last error, TLS, modules, `__chkstk`), builds the TEB and
    PEB through GS (`arch_prctl(ARCH_SET_GS)`, behind the SWAPGS fence D49 asked for) and runs
    the entry point; a missing import is refused by name with status 126. A QEMU test on all
    three boots two programs built from our own source for `x86_64-pc-windows-msvc`. Next, as
    user capabilities: item 19; then structured exception handling, other DLLs, ARM64 programs
    and x86_64 programs on Arm through ahead-of-time translation.
19. **done** — Windows, part 2a: users can now run small Windows console tools from
    `eggsh` on x86_64 (D121, docs/windows-programs.md): threads (`CreateThread`, each with its
    own TEB through GS, last error, TLS and FLS slots and static TLS copy), waits on threads,
    events, mutexes and semaphores (any or all, with timeouts, on one wake-sequence futex),
    critical sections, SRW locks, condition variables, one-time initialisation and
    `WaitOnAddress` on futexes, Windows' error codes (`ERROR_PATH_NOT_FOUND` for a missing
    directory, `FormatMessageW`), UTF-8/UTF-16 conversions and ordinal comparisons,
    `FindFirstFileW` with `*` and `?`, attributes, the working directory and full paths in
    Windows' form, `CommandLineToArgvW`, the system and version. A QEMU test on all three runs
    `windows-tools.exe` (`dir`, `cat`, `checks`) from our own source; part 2b below brings it
    to aarch64, and riscv64 refuses it with status 126.
19b. **done** — Windows, part 2b: users can now run ARM64 Windows console programs from
    `eggsh` on aarch64 (D124, docs/windows-programs.md): the PE32+ loader accepts machine
    `0xAA64` on aarch64 (and `0x8664` on x86_64; the other CPU's program is refused with the
    reason and status 126), and the whole Win32 console subset of parts 1 and 2a works on Arm:
    Windows' ARM64 ABI with the TEB in `x18`, reloaded by a small entry on every return from
    the layer and before every call into the program, per-thread TEBs in `tpidr_el0`, an
    aarch64 thread start and `__chkstk`. A QEMU test on all three runs part 1's program and
    `windows-tools.exe` built for `aarch64-pc-windows-msvc` on aarch64 with the same lines and
    exit codes as on x86_64. Next, as user capabilities: other processes (`CreateProcessW`,
    pipes), programs built on Rust's std or a C runtime (MSVC's, MinGW's), x64 programs on Arm
    through ahead-of-time translation (D121's and D124's "Not yet").
20. **done** — macOS, part 1: users can now run macOS console programs from `eggsh` on aarch64
    (arm64 programs) and x86_64 (x86_64 programs), thin or fat (D120,
    docs/macos-programs.md): an optional macOS layer at `/usr/lib/eggos/macos`, which
    `cargo xtask image --add` installs with the first Mach-O file, stands in for dyld: it picks
    the slice for the CPU, maps the segments with the slide, binds the imports to our own
    libSystem subset (stdio and `printf`, strings and memory, the heap, numbers, files and
    directories with Darwin's layouts, the environment, clocks and `mach_absolute_time`,
    `exit`, `atexit`, `abort`) through the chained fixups and calls `main`; an unimplemented
    import stops the program only when called (status 127), another library refuses it by
    name (126). The kernel's `execve` starts the layer for Mach-O and fat files (not Java
    class files) on aarch64 and x86_64; riscv64 refuses them with `ENOEXEC`. A QEMU test on all
    three boots programs built from our own source by Apple's linker (on CI's macOS runner).
21. **done** — macOS, part 2: users can now run small real macOS console tools from `eggsh`:
    `getopt`/`getopt_long`, directory listing, calendar time (`gmtime_r`, `localtime_r`,
    `timegm`, `strftime`, in UTC), `errno` per thread with `strerror`, C-locale `setlocale`,
    `nl_langinfo` and `localeconv`, and Rust std programs (arguments, environment, files,
    directories, `std::thread`, `Mutex`, `thread_local!`) on aarch64 and x86_64 (D123,
    docs/macos-programs.md): threads with their own control blocks behind the thread pointer,
    Mach-O thread-local variables, pthread keys, mutexes, condition variables and once,
    libdispatch semaphores, `mmap`, `fcntl`, `writev` and `dlsym`. Signal handlers are recorded
    but not delivered, and the unwinder, time zones, other locales, programs without chained
    fixups, other dylibs and frameworks, and x86_64 programs on Arm come later (D123's "Not
    yet").
22. **done** — users can now pipe commands and redirect input and output from `eggsh` (D125,
    docs/shell.md): `a | b | c` runs its programs at the same time, piped, in one process
    group that `^C` reaches, with the last one's status; `<`, `>` and `>>` on builtins and
    programs; the kernel's pipes (`pipe`, `pipe2`, blocking and non-blocking reads and writes,
    end of file, `EPIPE` and `SIGPIPE`, `poll`); a history of the last lines on the up and down
    arrows, on the serial and graphical terminals; `grep`, `wc`, `head` and `tail`; and `ps`,
    `free` and `uptime` from a minimal `/proc` (`meminfo`, `uptime`, `ps`). Next, as user
    capabilities: `2>` and `2>&1`; moving the cursor within a line; `/proc/self` and
    `/proc/<pid>`; the utilities' GUI and MCP front-ends (M14).
23. **done** — users can now create and change files from `eggsh` (D125, docs/coreutils.md):
    `/tmp` and `/home` are writable file systems in memory (files and directories, at most
    half of the memory together, `ENOSPC` past it), with `cp`, `mv`, `rm` (`-r`, `-f`), `mkdir`
    (`-p`) and `touch`. Nothing survives a reboot. Next, as user capabilities: a file removed
    while open stays readable until its last close; `cp -r`; files that survive a reboot (a disk driver and a disk file system).

The GUI block (owner, 2026-10-27, D126): v0.3.0 must install and open Windows, macOS and Linux
apps in windows on the desktop; console programs alone are not enough. In this order, after
items 22 and 23; the design of 30-31 and 34-37 is Claude Fable's (docs/ux/desktop-v0.3-gui.md,
D16):

30. **done** — several windows from programs: the window server, `/dev/window` (D127):
    programs open their own windows (shared pixel buffers, commits, input events), the
    terminal is one window among them; click to raise and focus, move by the title bar,
    minimise to a chip, the close control asks the program to close.
31. **done** — users can now resize, maximise, restore, close and switch between windows with
    the mouse and keyboard (D128): edge and corner resizing, maximise and restore (the green
    control, a double-click on the title bar, Super+Up/Down), snap to halves, the drawn pointer
    and its shapes, Alt+Tab and the switcher, Alt+F4 and Cmd+W, the window menu (Alt+Space),
    show desktop, the 2 s rule for a program that does not answer a close request, and the
    `wm` command. Next: windows partly off screen, a profile setting (item 34), `wm`'s MCP
    tools (with the MCP server).
32. **done** — users can now run Windows GUI programs from `eggsh` and see their windows on
    the desktop (D129, docs/windows-programs.md): x64 on x86_64 and ARM64 on aarch64, the
    layer's own user32 and gdi32 over `/dev/window`: window classes, windows with EggOS's
    chrome, the message loop with the mouse and keyboard as Win32 messages, painting with GDI's
    brushes, pens, shapes and Egg Mono text, and `MessageBoxW` as a modal dialog in Claude
    Fable's design (docs/ux/windows-gui-v0.3.md); a QEMU test drives `win-paint.exe` and
    compares every frame with the host's render. Next: controls, menus and dialogs (item 42),
    the C runtime (item 43), fonts and text beyond ASCII, timers.
33. **done** — users can now pass a folder from their computer to EggOS and use its files
    (D130): `run-eggos.sh --apps <folder>`, `run-eggos.ps1 -Apps <folder>` and `cargo xtask run
    --apps <folder>` attach it with QEMU's `vvfat` on a virtio block device, and EggOS mounts
    it read-only at `/media/host`: a polled virtio-blk driver on all three architectures, disks
    in the driver model (`block`), a FAT12/16/32 reader with long names, and programs run from
    any file system (`/tmp`, `/home` and `/media/host` too). Next: the writable disk (item
    25), several requests in flight, interrupts.
25. **done** — users can now keep files across reboots (D131, docs/eggfs.md): the run scripts
    and `cargo xtask run` create and attach a sparse 1 GiB `eggos-<arch>-disk.img` (`--disk`,
    `--no-disk`, `--format-disk`); EggOS formats it with EggFS, its own journaled file system
    (metadata journaled, data copy on write, CRC-checked commits), and mounts `/home` and
    `/Applications` on it; disks are told apart by what they hold; virtio-blk writes and
    flushes; `sync`, `syncfs`, `fsync`, `fdatasync`, the `sync` utility and a sync before
    power-off; a committer every 5 s; host crash tests at every write and a QEMU power cut.
    Next: a repair tool, links, a format command in EggOS with its three front-ends, several
    requests in flight.
34. **done** — users can now open Settings, pick the Windows, macOS or Linux profile and keep
    it across reboots (D132, docs/settings.md): `userland/toolkit`, a native toolkit with no
    heap (a retained widget tree with layout, labels, buttons, checkboxes, radio groups, cards,
    lists, text fields and scroll views, focus and keyboard navigation, hover and pressed
    states, the design's tokens, damage and 2x), Egg Sans 16, Claude Fable's proportional UI
    font (docs/ux/font-ui-16.txt), and `/bin/settings`, the window and the `settings get`,
    `set` and `list` command; `GET_PROFILE` and `SET_PROFILE` switch the controls' side and
    the shortcuts live, and the kernel reads the saved profile from
    `/home/root/.config/eggos/settings` before the desktop's first frame. Next: the shell's
    panels per profile and a launcher entry (item 35), the MCP tools, 2x, the Mouse and
    Components panes.
35. **done** — users can now use the desktop like the system they came from (D133,
    docs/apps.md): the taskbar with Start, the windows and the clock (Windows profile), the
    top bar and the dock (macOS), the top bar with `Activities` (Linux), all laid out by one
    function of the screen and the profile and switched live; the launcher (Super, Cmd+Space,
    Start, `Activities`) with search by name and synonym; desktop icons; Claude Fable's icon
    set; the app catalogue in `/Applications/<name>/app.manifest`; Settings from the dock, the
    launcher and the desktop. Next: install and uninstall (item 36), Files (item 37), the
    phone layout, 2x.
36. **done** — users can now install, open and uninstall Windows, macOS and Linux apps (D134,
    docs/apps.md): `app install <path>` recognises a PE32+ `.exe`, a Mach-O tool or `.app`
    bundle, or a static ELF by its bytes, refuses what this machine cannot run with the
    reason, and copies it into `/Applications/<name>/` with its manifest written last;
    `app uninstall` and `app list`; the installer's window (prompt, progress, result,
    uninstall confirmation, a chooser for the host share) from `--window`, the launcher's
    `Install from host share...` and the icon menus' `Uninstall...`; the launcher and icons
    follow at once (`APPS_CHANGED`). Next: Files (item 37), the MCP tools with the MCP
    server, installing a missing layer with the component manager.
37. **done** — users can now browse their folders and the host share in a graphical file
    manager (D135, docs/ux/desktop-v0.3-gui.md section 8.1): `/bin/files`, from the dock, the
    launcher, the desktop's Files icon and its `Host share` icon, with places (Home and its
    folders, tmp, the share, Apps, System, Trash), a list of names, sizes and times, back, up
    and a path field; a double-click runs a Linux program in the Terminal window, offers a
    Windows or macOS program to the installer (item 36), opens an installed app and shows a
    text file with `cat`; copy, cut and paste (Ctrl or Cmd with C, X, V, and the context menu)
    within and across file systems, out of the read-only share too, in steps with a progress
    dialog; new folders, rename in place, Properties; a real Trash in the freedesktop.org layout
    (`Move to Trash`, Restore, `Empty Trash`, permanent delete after a question); and the same
    on the command line: `cp -r`, `mv` of folders across file systems, and `trash` with
    `-l`, `-r` and `-e`; `TYPE_IN_TERMINAL`. Next: the MCP tools with the MCP server, free
    space (`statfs`), multiple selection, a shared clipboard, the Text Editor (item 46).
28. **Done** — users can now connect Claude Code or any MCP client to EggOS and let it work
    with files, programs and apps under the grants they set (D136, D137, docs/mcp.md):
    `run-eggos.sh --mcp` (`run-eggos.ps1 -Mcp`, `--no-window` with no screen, D144) wires a named
    virtio console port to the script's stdio; init starts `eggmcpd` when `/dev/agent` exists;
    the tools `ls`, `cat`, `ps`, `wm_list` and `app_list` read; with `--mcp-allow
    write,run,install` (kernel command line, `/proc/cmdline`) `tee` and `trash` write below
    `/home/root`, `run` runs a program (full control), `app_install`, `app_open` and
    `app_uninstall` manage apps; destructive calls ask the user through elicitation or need
    `confirm: true`; every call goes to `/var/log/agent-audit.log` (seven generations, a budget
    per run, content never logged), which `agentlog` shows; `kill` reaches other processes and
    groups, init excepted. Next: per-agent grants, network MCP, the log in Settings.
38. **done** — users can now run Linux programs that talk over local stream sockets, pass
    descriptors and share memory (D138, D139): `AF_UNIX` stream sockets, `socketpair`,
    `SCM_RIGHTS` and `poll` readiness (38a); `memfd_create`, `ftruncate`, `fallocate`, seals
    and `MAP_SHARED` of memfds (38b). The item's "`MAP_SHARED` of files" is narrowed to
    memfds, which is what Wayland's shared buffers use; tmpfs and EggFS files come later. Next:
    the Wayland server (item 39, done), then `epoll`, `eventfd`, `timerfd` and `signalfd`.
39. **done** — users can now run Linux programs that open their windows through Wayland
    (D140, docs/linux-programs.md): `eggway`, our own Wayland server from the public protocol
    specifications, started by init when a display exists, on `/run/user/0/wayland-0`;
    `wl_compositor`, `wl_shm`, `wl_surface`, `wl_seat` with a US keymap, `wl_output`,
    `xdg_wm_base`, `xdg_surface` and `xdg_toplevel`; each toplevel a desktop window with
    EggOS's chrome, its input as Wayland events, frame callbacks at 60 Hz; `xdg_toplevel.move`
    and `resize` through the new `/dev/window` request `START_MOVE_OR_RESIZE`. Next: `epoll`,
    `eventfd`, `timerfd` and `signalfd`, dynamic linking, then the toolkits' protocols
    (subcompositor, decorations, popups, the clipboard).
40. **done** — users can now run macOS Objective-C console programs from `eggsh` on aarch64
    and x86_64 (D142, docs/macos-programs.md): our own Objective-C runtime in the macOS layer
    (`objc_msgSend` and `objc_msgSendSuper2` in arm64 and x86_64 assembly with lock-free method
    caches, classes and categories from the image's metadata, selectors, non-fragile ivars,
    reference counting, ARC's functions, autorelease pools, `+load` and `+initialize`) and the
    core of Foundation (NSObject, constant and dynamic NSString, NSArray, NSDictionary and their
    mutable forms, NSNumber, the compiler's literals, NSBundle's main bundle, `NSLog` and `%@`).
    A QEMU test runs an Objective-C program from our own source, built by clang on CI's macOS
    runner, with the same lines as on a Mac. Next: blocks, `__weak`, exceptions, more of
    Foundation (D142's "Not yet").
47a. **done** — users can now let an AI agent see and operate any app's window. Part 1, the
    command line (D141, docs/windows.md): `CAPTURE_WINDOW` copies a window's content from its
    own buffer (covered or minimised too), `SEND_INPUT` sends clicks, keys, characters and the
    wheel straight to its queue, a person always wins (`EBUSY`), `wm capture` writes a PNG with
    our own encoder (`png/`), `wm click`, `type`, `press` and `scroll` operate it, and `app
    open` opens the built-in apps by name. Part 2, the agents (D143, docs/mcp.md): the `view`
    grant and `wm_capture`, whose answer is the PNG as an MCP image after a text that frames
    it as data; `wm_click`, `wm_type`, `wm_press` and `wm_scroll` under `run`; each call in the
    audit log (typed text included); the MCP boot check captures Settings, `win-paint` and
    `linux-paint` (covered and minimised) exactly and clicks and types through the tools.
47b. **done** — users can now run EggOS with no screen at all and still open Windows, macOS and
    Linux GUI apps for an AI agent (D144, docs/mcp.md): `run-eggos.sh --mcp --no-window`
    (`-Mcp -NoWindow`) gives QEMU no display, keyboard or mouse and passes
    `eggos.display=offscreen[:<width>x<height>]` in the one `-append` it builds with the grants;
    with no display device the kernel draws the desktop into a framebuffer in memory, at
    1280x800 by default, and ticks its clock without an input poller. The MCP check's boots run
    that way on all three architectures.
42. **done** — users can now run Windows programs with buttons, text boxes, labels, lists,
    menus, timers and Open and Save dialogs (D145, docs/windows-programs.md): child windows in
    the layer, the system's `BUTTON`, `EDIT`, `STATIC` and `LISTBOX` with `WM_COMMAND`, the
    keyboard focus and `IsDialogMessageW`, the menu bar and its drop-downs, `SetTimer`, and
    comdlg32's `GetOpenFileNameW` and `GetSaveFileNameW` on EggOS's own picker, all in Claude
    Fable's design (docs/ux/windows-controls-v0.3.md). The Windows GUI boot test drives
    `win-controls.exe` through typing, clicks, the menu and both pickers on x86_64 and aarch64
    and compares every frame with the host's render. Next: multi-line edits and the clipboard,
    radio buttons, combo boxes, scroll bars, menu keys (D145's "Not yet").
43. **done** — users can now run Windows programs built by Microsoft's compiler on its C
    runtime (D146, docs/windows-programs.md): C programs built with `/MD`, console or GUI, x64
    on x86_64 and ARM64 on aarch64, import `vcruntime140.dll` and the Universal C Runtime
    through the `api-ms-win-crt-*` API sets, which the Windows layer now provides: the
    runtime's startup and exit, `malloc`, strings, `FILE` streams in text or binary mode and
    `printf`'s family (one engine with the macOS layer's, `eggos-c-format`). A QEMU test on
    x86_64 and aarch64 runs `eggpad.exe`, our own C notepad built by MSVC in CI, opens a file,
    types, saves it with Ctrl+S and reads it back. Next: `scanf`, locales, the math library,
    C++ exceptions (D146's "Not yet").
41. **done** — users can now run macOS GUI apps (D147, docs/macos-programs.md): programmatic
    Objective-C AppKit apps in an `.app` bundle install and open from the launcher, arm64 on
    aarch64 and x86_64 on x86_64. The macOS layer's own AppKit runs NSApplication's event loop
    and its delegate, NSWindow on the window server, NSView's `drawRect:` with NSColor,
    `NSRectFill`, NSBezierPath and NSString's `drawAtPoint:`, and `mouseDown:` and `keyDown:`;
    NSBundle reads the bundle's `Info.plist`. A QEMU test installs `mac-paint.app`, our own
    source built by clang in CI, opens it from the launcher and compares every frame.
44. **done** — users can now use macOS apps' buttons, text fields, alerts and menus (D147):
    NSButton's target and action, editable NSTextField and labels drawn as the toolkit's
    controls, NSAlert's `runModal` in a dialog, and the main menu in the macOS profile's top bar
    (the window server's new `SET_MENU` and `MenuChosen`), with key equivalents and Cmd+Q. The
    same test types into a field, presses buttons, answers an alert, opens the `File` menu and
    quits with Cmd+Q. Next: nibs, Swift, scroll views, images, timers (D147's "Not yet").
45. **done** — users can now install an app from a `.zip` and see its own icon (D148,
    docs/apps.md, Claude Fable's docs/ux/app-icons-v0.3.md): `app install <file>.zip` (and
    Files and the installer's window, and the MCP tool `app_install`) extracts the archive's
    one program with everything beside it, `--entry` choosing among several, through our own
    inflate (every DEFLATE block type, shared by a new row-by-row PNG decoder in `eggos-png`);
    the installer then makes the app's own icon from a Windows program's icon resources, a
    bundle's `.icns` or a PNG beside it, scaled to 48 and 32 pixels and written as `app.icon`,
    which the dock, desktop, launcher, taskbar and switcher draw instead of the letter tile;
    `app list` and `app_list` name it. A QEMU test installs `mac-paint.zip` and compares the
    desktop's and the launcher's frames, its own icon in them. Next: `app icon` and `app_icon`,
    icons in the installer's dialogs and title bars, 2x images, binary property lists.
46. **done** — users can now open, edit, find in and save text files in a native Text Editor
    (D149, docs/apps.md, Claude Fable's docs/ux/text-editor-v0.3.md), replacing old item 24's
    terminal `edit`: Files opens text files in it, the launcher lists it (Notepad, TextEdit,
    gedit), `edit <path>` opens a file in a window of its own and returns, `app open "Text
    Editor"` and the MCP tool `edit_open` open it too. New, Open, Save and Save As (a path bar,
    no picker yet), find with next, previous and `Match case`, undo and redo, cut, copy and
    paste, line numbers, `Ln, Col` and `Modified` in the status bar, CRLF files and byte-order
    marks kept, up to 4 MiB of UTF-8, both Ctrl and Cmd in every profile; the editing logic is
    the host-tested `eggos-editor` crate. A QEMU test opens a file with `edit`, types, finds,
    saves and closes it, every frame compared, and `cat` reads the saved text. Next: word wrap,
    replace, a file picker, a shared clipboard, text beyond Egg Mono's glyphs.
29. **done** — users can now see the real date and time, and files carry real times (D150,
    docs/coreutils.md, Claude Fable's docs/ux/date-time-v0.3.md): the kernel reads the
    machine's real-time clock once at boot (x86_64's CMOS clock, AArch64's PL031 and RISC-V's
    Goldfish clock, behind the driver model), so `CLOCK_REALTIME`, `gettimeofday`, `time`, file
    times, the Windows and macOS layers' clocks and the desktop's clock (`HH:MM`) are real.
    `date` shows and, for root, sets the time (`date -s`), `hwclock` reads and writes the
    hardware clock, Settings has a `Date & time` pane, and the MCP tools `clock_get` and
    `clock_set` read and set it; setting the clock writes the hardware clock too. All UTC. A
    QEMU test checks on all three architectures that `date`, a new file's time and `hwclock`
    are within minutes of the host's clock and that `date -s` reaches the hardware clock.
    Next: time zones, network time, `/dev/rtc0`, alarms.

The release path (the release-path review's issues #141-#145, after the items above):
- #141 **done** — users who download the release can install and open Windows and macOS apps
  (D152, docs/apps.md): the x86_64 and aarch64 release images carry the Windows and macOS
  layers, a v0.3.0 exception to D18 until the component manager; the install, Files, MCP and
  macOS GUI boot tests boot the release artifact itself, with no `--add`; `cargo xtask
  sample-apps` builds our own apps to try. Next: the layers as optional components (M14).
- #142 **partly done** — switchers feel at home in their first minute and see when an agent
  drives (D156, docs/ux/desktop-v0.3-gui.md section 14): the `Agent` chip while an MCP client
  is connected (yellow while it acts, a click runs `agentlog`), ` (agent)` on the driven
  window's title and a yellow arrow where its pointer input went; tooltips `Apps  Super`,
  `Activities  Super` and `Start  Super`, the launcher's footer of keys, the Mac host line in
  `run-eggos.sh` and the release notes; `/etc/motd` and eggsh starting in `/home/root`; window
  controls that darken while pressed and act on release. Next: first boot asks "Where are you
  coming from?", init's line on the serial console only, the screendump checks.
- #143 **done** — a program that uses up memory, or a damaged disk, can't crash EggOS (D155):
  programs' pages, memory files, window buffers, pipes, `/tmp` files, threads and socket
  queues stop at a kernel reserve (a thirty-second of memory, 2 to 16 MiB, shown by `free` and
  `/proc/meminfo`), window buffers at a quarter of memory and socket queues at an eighth; the
  Linux programs boot test's `linux-memory` uses memory up four ways on every architecture and
  the shell answers after; EggFS is tested with hostile inodes whose checksums are valid, and
  a looping FAT cluster chain is an I/O error. Next: per-user limits once users exist (M10).
- #144 **done** — users can follow the README, docs and release notes as written (D153):
  `sh run-eggos.sh --mcp` everywhere, `reboot` restarts EggOS in the run scripts (a QEMU check
  on every architecture), QEMU 8.2 or later and no acceleration stated, one measured boot time,
  one-line PowerShell usage errors with status 2, the docs reconciled with the MCP server, and
  release notes whose "What's new" comes from the merged pull requests.
- #145 **done** — Windows users' `run-eggos.ps1` is booted by CI (D154): `cargo xtask
  run-scripts` drives the release's own script, serial with a host folder, the disk lock and
  `-Mcp` over piped standard input and output, through `run-eggos.ps1` in a new
  `run-scripts-windows` job and through `run-eggos.sh` in the `qemu` jobs.
- Old item 26, networking part 1 **partly done** — EggOS is on the network (D157,
  docs/network.md): a polled virtio-net driver, `eggos-net` (Ethernet, ARP, IPv4, ICMP echo,
  UDP, a DHCP client, DNS messages), `lo`, `/etc/resolv.conf` from the lease, `AF_INET` UDP and
  ping sockets for Linux programs, `ip`, `ping` and `nslookup`, the MCP tool `net_status`; the
  run scripts attach QEMU's user networking (`--no-network`: none); a QEMU check on all three
  architectures gets the lease, pings the host, looks a name up and talks UDP through
  `std::net`. Next: the GUI front-end Fable designed (Settings' `Network` pane, the bar's
  indicator, docs/ux/network-v0.3.md).
- Old item 27, networking part 2 **partly done** — TCP and downloads over HTTP (D158,
  docs/network.md): TCP of our own in `eggos-net` (handshake, segment size, windows,
  retransmission on a measured timeout, fast retransmission, window probes, FIN, `TIME-WAIT`,
  resets) and HTTP/1.1 downloads (redirects, chunked bodies); `AF_INET` stream sockets for
  Linux programs (`connect`, `listen`, `accept`, `poll`, `shutdown`); `fetch` in the core
  utilities; the MCP tools `net_ping`, `net_lookup` and `fetch`; the QEMU check downloads a file
  from the host and runs `std::net` TCP over `lo` and with the host. Next: the GUI front-ends
  (the Network pane and indicator above, a downloads window by a Fable design). HTTPS and the
  network clock (part 3, D159): our own `crypto/` (SHA-2, HMAC, HKDF, AES-GCM,
  ChaCha20-Poly1305, X25519, ECDSA and RSA checks) and `tls/` (a TLS 1.3 client with X.509
  path validation), `fetch https://` against the roots the user gives in `/etc/ssl/roots.pem`,
  `date --sync` and MCP `clock_sync` by SNTP. Next: the Date & time pane's "Set
  automatically" (a Fable addition to date-time-v0.3.md first), syncing at boot, a QEMU check
  of `fetch https://` against a TLS test server.

Extras, in order, after those and before the freeze: 42 Windows controls, menus and dialogs; 43
Windows apps on the C runtime; 44 macOS GUI apps, part 2; 45 real icons and archives; 46 a Text
Editor (done, D149, replacing the terminal's `edit`); 29 an RTC driver for `date` (done, D150). Moved after
the release: networking (part 1 done after it: D157, below); the MCP server is back in the release (owner, 2026-10-28; item 28, D136). The graphical terminal stays the owner's priority: nothing above may break it.

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo skeleton: workspace, pinned toolchain, HAL layout, CI, docs | **done** |
| M1 | Kernel boots in QEMU and prints, on the first architectures, with an automated QEMU test runner | **done** — aarch64, riscv64, x86_64 |
| M2 | Memory management: physical frames, paging, kernel heap | **done** (0.2.0) — memory map from the device tree / PVH and a frame allocator on aarch64, riscv64, x86_64 (D28); kernel page tables: a direct map in the high half, W^X kernel sections and a boot-stack guard page on aarch64, riscv64, x86_64 (D29), with the image's direct-map alias keeping its protections (D32); kernel heap: `alloc` works, size classes up to 2 KiB (D30); buddy allocator: contiguous, aligned blocks of up to 4 MiB, so heap allocations up to 4 MiB work (D31); the kernel runs in the top 2 GiB with boot page tables built before any Rust code, and the low half is empty (D33) |
| M3 | Interrupts and timers | **done** (released with v0.3.0, D99) — exception vectors (D25); monotonic clock and one-shot timer interrupts through GICv2, SBI and the local APIC on aarch64, riscv64, x86_64 (D26); device interrupt routing: GICv2 SPIs, a PLIC driver with the boot hart's context from the device tree, an I/O APIC driver with the MADT's overrides, and the console UART's receive interrupt on all three, its bytes queued in the kernel (D98); GICv3, AIA and MSI-X come with the driver model (M5) |
| M4 | Scheduler, processes and threads (SMP-ready) | **in progress** — kernel threads with a context switch on aarch64, riscv64, x86_64 and a cooperative round-robin scheduler on one CPU, 16 KiB stacks with an overflow canary (D35); voluntary preemption: a 10 ms timer tick ends time slices and threads switch at preemption points, the scheduler's lock handed over across the switch (D37); sleeping and wait queues: an idle thread, `sleep`, `WaitQueue`, a blocking `join`, threads linked through themselves, and one timer shared by the tick and the sleepers with no tick while one thread is runnable (D39); secondary CPUs: discovered from the device tree / MADT, started (PSCI, SBI HSM, INIT-SIPI with a real-mode trampoline) on the kernel's page tables, each with a per-CPU block behind its per-CPU register, reporting in and parking (D40); every CPU takes its own interrupts: per-CPU timer and interrupt controller setup, wake-up interrupts between CPUs (GICv2 SGI, SBI IPI, local APIC fixed IPI), per-CPU interrupt counts, and a one-load per-CPU read for hot paths (D41); every CPU runs threads: per-CPU run queues, sleepers and idle threads, new threads placed on idle CPUs first, threads woken from any CPU onto their own with the wake-up interrupt or a reschedule request, one scheduler lock for now (D43); load balancing: an idle CPU takes ready threads from busy ones, and a slice that ends with threads ready wakes an idle CPU (D44); user address spaces: a low half of their own with the kernel half shared, user mappings on aarch64, riscv64, x86_64, switching with the CPU's user translations dropped, and kernel access to user pages only in a window (D45); user mode: the kernel runs a program in EL0, U-mode or ring 3 until a system call (`svc`, `ecall`, `syscall`), an interrupt or an exception brings the CPU back, with the program's registers in a `UserContext` and Linux's system-call registers; a per-CPU GDT and TSS on x86_64; a system-call round trip in about 106 (aarch64), 148 (riscv64) and 107 (x86_64) instructions (D47); processes: threads that run their program in a loop and hand each exit to a handler, address spaces loaded lazily per CPU at the point of use and kept until another is needed, preemption on return to user mode, processes on every CPU (D48); review of D47–D48: user programs get no FP/SIMD, vector unit or timer access whatever firmware left, `int3` after x86 returns, speculation mitigations per CPU with recorded triggers (D49); next: the Linux system calls (M6); per-CPU and per-wait-queue locks wait for a workload that shows contention (D45) |
| M5 | Common driver model (bus enumeration, device tree / ACPI discovery) | **in progress** — the driver model: static driver descriptors from cargo features with match tables ranked by specificity, probe passes (interrupt controllers, buses, devices), a device table on the heap and the `devices` capability (front-ends with M10); discovery from the device tree (aarch64, riscv64) and from the MCFG with PCI ECAM enumeration and BAR sizing (x86_64); device windows mapped into the live kernel tables below a root entry made in advance, ECAM kept out of the direct map; DMA regions (D110); next: device interrupts for drivers (GICv2 SPIs, the PLIC, MSI-X; issue #97 slice 3), the GIC and UART addresses from discovery instead of D12's, `ranges` translation, PCI on aarch64 and riscv64, AML (M13) |
| M6 | Linux syscall ABI: what a static hello-world needs (`write`, `exit_group`, `brk`, `mmap`, ...) | **in progress** — the Linux system-call layer: dispatch by Linux number per architecture, `write` to the console, `exit` and `exit_group`, `ENOSYS` for the rest, exceptions end the process with Linux's signal, user memory copied after a table walk (D50); programs start from ELF images: a checked ELF parser, executables and static PIEs mapped page by page with W^X and a 0x10000 floor, Linux's initial stack with `argv`, `envp` and the auxiliary vector, Linux's `execve` errors (D51); demand paging: processes have regions (Linux's mappings), a page is mapped with a zeroed frame when the program first touches it, the stack is one 8 MiB region mapped as it grows, kernel copies map untouched pages, and live address spaces gain mappings (D52); the memory calls: `brk`, anonymous `mmap`, `munmap` and `mprotect` on the regions with Linux's checks and errors, top-down placement, `PROT_NONE` pages keeping their frame in an invalid entry, and other CPUs that keep a changed address space loaded marked stale so they switch again before using it (D53); FP/SIMD registers per thread: given on first use, saved on every switch away and loaded again only when the CPU no longer holds them, cleared before a program that has none runs (LazyFP), the RISC-V kernel soft-float (D54); the first startup calls: `writev`, `getpid`, `getppid`, `gettid`, `uname` (Linux 6.1.0-eggos), `set_tid_address` and `set_robust_list`, and process ids (D55); the kernel's random number generator (BLAKE2s input, ChaCha20 with fast key erasure and per-CPU keys, seeded from the device tree's `rng-seed` and `RDSEED`/`RDRAND`), `AT_RANDOM` from it and `getrandom`, with frames and page tables zeroed a line at a time (`linux_exec_load` −27%, −18%, −1%) (D56); the thread pointer in each thread's user context, loaded on every way into user mode (`TPIDR_EL0` saved and loaded, `tp`, the FS base written only when it changed), x86_64's `arch_prctl` and `sched_yield`, with no cost on the thread switch and the system-call round trips slightly faster (D57); review of D55–D57: empty ranges past user space `EFAULT` as on Linux, `writev`'s count an `unsigned int`, BLAKE2s wiped with volatile stores (D58); resource limits: `getrlimit`, `setrlimit` and `prlimit64` on per-process limits with a current distribution's defaults, copied on a process's first change, no hard limit raised until processes have credentials, not yet enforced (D59); signal state: `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` with Linux's rules, actions per process copied on first change, the blocked mask and alternate stack per thread, handlers accepted but nothing delivered yet (D60); the standard descriptors: 0, 1 and 2 are the console, open for reading and writing as Linux starts `init`; `poll` (x86_64) and `ppoll` with Linux's checks, the console always writable and never readable, waits that sleep out their timeout and write back the remaining time; the console's `ioctl`s: `TCGETS` and `TCGETS2` with a Linux serial console's settings, `TIOCGWINSZ` and `TIOCSWINSZ`, `FIONREAD`, `ENOTTY` for the rest (D61); review of D59–D61: `linux_exec_load` loads only the code up to the end of `linux_start`, so new HAL test programs no longer grow it, `writev` checks its entries in order as Linux does, and `ppoll` writes the time not slept back for every timeout that is not zero (D62); signals sent and delivered, part 1: `kill`, `tkill` and `tgkill` to the caller (its own group for 0 and minus its id), `rt_sigpending`, a pending set per thread, and delivery by the default action or `SIG_IGN` before the call that made a signal due returns (Term and Core end the process with the signal, so `abort()` ends with `SIGABRT`; the terminal's stop signals are discarded in an orphaned process group; `SIGSTOP` stops for good; `SIGCONT` and the stop signals cancel each other), with nothing added to the system-call path (D63); signal handlers: Linux's signal frames on each architecture (`siginfo_t`, `ucontext_t`, FP/SIMD parts in Linux's shape holding the initial state), `rt_sigreturn` restoring every register, the mask and the alternate stack, `SA_SIGINFO`, `SA_ONSTACK` with `SS_AUTODISARM`, `SA_NODEFER`, `SA_RESETHAND`, one frame per due signal stacked as on Linux, `SIGSEGV` for a frame that cannot be written or read, and a return trampoline mapped on first use for AArch64 and RISC-V handlers without a restorer, with nothing added to the hot paths (D64); faults delivered to signal handlers: the signal, `si_code` and `si_addr` Linux gives for each exception on each architecture (`SEGV_MAPERR` or `SEGV_ACCERR` by the process's regions), x86_64's `err`, `trapno` and `cr2` and AArch64's `fault_address` and `esr_context` in the frame, sanitized for addresses outside user space, forced as on Linux (a blocked or ignored fault signal ends the process), new HAL kinds for `#DE`, `#DB`, `#NP` and `#SS` (D65); review of D63–D65: synchronous signals delivered before others, those the kernel raised first, so a failed frame's `SIGSEGV` is not overtaken, and AArch64 frames of `brk` and undefined instructions with no fault address or `esr_context`, as on Linux (D67); real-time signals queued with their `siginfo_t` (each instance in send order, a standard signal keeping its first info), `RLIMIT_SIGPENDING` enforced with Linux's exceptions, `rt_sigqueueinfo`, `rt_tgsigqueueinfo` and `rt_sigtimedwait`, nothing added to the hot paths (D68); waiting for a signal: `rt_sigsuspend` with its mask in place until a handler starts (the frame saving the mask from before), then `EINTR`, and x86_64's `pause` (D70); `ppoll`'s signal mask applied during the call, a signal it unblocks interrupting a wait that finds nothing ready (`EINTR` once a handler starts, its frame saving the thread's mask) (D71); review of D68–D71: `ppoll` interrupted by an ignored signal fails with `EINTR` when its timeout cannot be written back, `rt_sigtimedwait` takes synchronous signals by number, kernel-raised ones delivered in the order raised, a drained signal queue shrunk (D72); clocks and sleeping: `clock_gettime`, `clock_getres`, `gettimeofday`, x86_64's `time`, `nanosleep` and `clock_nanosleep` (relative and absolute, never early) on the realtime, monotonic, boot-time and TAI clocks, the wall clock starting at the epoch until a real-time clock driver sets it, the CPU-time clocks `EINVAL` until there is CPU time, `clock_settime` and `settimeofday` `EPERM` until processes have credentials (D73); the console's status: `fstat`, `newfstatat` and `statx` on descriptors 0 to 2 report `/dev/console`'s character device 5:1 (root, `0600`, epoch times, `statx` with the basic fields), with Linux's path lookup and errors (a null path with `AT_EMPTY_PATH` accepted, other paths `ENOENT`, `ENOTDIR` or `EBADF` until there are files), and `lseek` refused with `ESPIPE` (D74); credentials: real, effective, saved and file system user and group ids and supplementary groups, starting as root, with `getuid`, `geteuid`, `getgid`, `getegid`, `getresuid`, `getresgid`, `getgroups`, `setuid`, `setgid`, `setreuid`, `setregid`, `setresuid`, `setresgid`, `setfsuid`, `setfsgid` and `setgroups` on Linux's rules, privilege as effective uid 0 (exact until capabilities exist), root raising hard limits, the auxiliary vector's ids and `AT_SECURE` from them, `si_uid` the sender's (D75); review of D73–D75: `setgroups` refuses group id -1 in list order, a `setresuid` or `setresgid` that changes nothing keeps the file system id, `newfstatat` takes `statx`'s synchronisation flags (D76); the memory calls allocators make at run time: `madvise` (`DONTNEED`, `DONTNEED_LOCKED` and `FREE` free the pages' frames and leave zeros, `POPULATE_WRITE` maps frames up front, `POPULATE_READ` checks, the advice about paging, merging, huge pages, dumps and `fork` accepted, `REMOVE` and `COLLAPSE` `EINVAL`, `HWPOISON` and `SOFT_OFFLINE` `EPERM` unprivileged) and `mremap` (shrinking, growing in place below the stack's guard gap, moving with `MREMAP_MAYMOVE` by relinking each page's entry so frames and contents go along and the pages go back if tables run out, `MREMAP_FIXED`, `MREMAP_DONTUNMAP`), Linux's checks in Linux's order (D77); signal frames hold the thread's FP/SIMD registers and `rt_sigreturn` loads them with Linux's checks (x86_64's handlers start with the initial state, a null `fpstate` is the initial state, a misaligned one or reserved `MXCSR` bits `SIGSEGV`; a thread without registers gets them when its frame holds other values) (D82); review of D54, D77 and D82: x86_64 clears `CR4.OSXSAVE`, `PKE`, `CET` and `UINTR` whatever firmware left, so no program sees AVX, protection-key, shadow-stack or user-interrupt state that is never switched, `mremap` with `MREMAP_FIXED` checks the old range's end before it moves anything, `MADV_POPULATE_WRITE` leaves mapped pages alone (D83); the official `*-unknown-linux-musl` targets pinned for test programs built from our own Rust source with std (D84); a first program built from our own Rust source runs in the QEMU self-tests on aarch64, riscv64 and x86_64, on a runtime of our own with no C library (`userland/runtime`: `_start`, a static PIE relocating itself, `PT_GNU_RELRO` protected, raw system calls), checking 17 start-up calls a C library and std make (D85); a program on Rust's standard library, statically linked with musl, runs in the QEMU self-tests on all three: musl's and std's start-up, a `HashMap` with random keys, the heap, clocks, sleeping and standard output, loaded as a static PIE on x86_64 and a fixed-address executable on AArch64 and RISC-V (D86); `AT_MINSIGSTKSZ` in the auxiliary vector, our `MINSIGSTKSZ`, checked at compile time to hold the largest signal frame at any alignment (D87); review of D84–D87: user programs build whatever `RUSTFLAGS` holds, relocation tables checked whole, exit statuses read per program (D88); `futex`: `FUTEX_WAIT`, `FUTEX_WAKE` and their bitset forms, private and shared kept apart, Linux's checks in Linux's order, FIFO waiters on the waiting threads' stacks in 256 buckets, on a new timed park in the scheduler (`thread::Parker`) (D89); `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` with Linux's counts, order and errors, for musl's condition variables (D90); threads: `clone` with `pthread_create`'s flags (`CLONE_SETTLS`, the id words, `CLONE_CHILD_CLEARTID`'s clear and shared wake), each thread's own id, blocked mask, pending signals and alternate stack, `set_tid_address` remembered, `exit` ending one thread and `exit_group`, fatal signals and the out-of-memory kill ending every thread (other CPUs asked to look, futex waits woken), the first thread's status when all end alone, Linux's `EINVAL`s and `ENOSYS` for new processes; Rust's `std::thread`, `Mutex` and `Condvar` run in the QEMU self-tests; a process's threads stay on one CPU until changed mappings are shot down on others (D91); review of D89–D91: every Linux sleep (`nanosleep`, `ppoll`, `rt_sigsuspend`, `pause`, `rt_sigtimedwait`, a stop) waits in the futex table, so a process's end wakes it and its thread ends, and `Parker::unpark` takes a pointer, since the parked thread may drop its `Parker` before the call returns (D92); robust futex lists: `set_robust_list` remembered, `get_robust_list` for the caller, and a thread's exit releasing the locks on its list that it holds (owner-died bit, waiters bit kept, one shared wake, 2048 entries at most, the pending entry last) before its tid word is cleared (D93); a process's threads run in parallel on every CPU: a TLB shootdown (each CPU holding the address space asked to drop its user translations, the wake-up interrupt, a wait for each, served on wake-ups, in the wait itself and while waiting for any lock but a leaf one), pages unmapped in two steps so no CPU writes a frame after it is reused, futex words read and robust locks released with atomics on user memory, `getcpu`, and no thread pinned any more (D94); signals between a process's threads: `tkill`, `tgkill` and `rt_tgsigqueueinfo` reach every thread of the caller's process (Linux's `ESRCH`, an exited first thread taking signals it never delivers, `SIGKILL` included), `SIGKILL` ends the process at once, any other signal waits in the process's signal state until its thread takes it in, as if sent then: woken from its sleep (a futex word per thread), interrupted on another CPU, interrupting its own CPU when switched back in, or on its way back from an interrupt, `sched_yield`, `futex`, a sleep or before its own signal calls; `SIG_IGN` discards every thread's pending instances in order with what was sent; `rt_sigtimedwait`, `rt_sigsuspend`, `pause`, `poll` and `ppoll` (with or without a mask) are woken and interrupted by them, `nanosleep` sleeps out its time and delivers after; Rust's std program handles a signal sent to a thread waiting in `sigsuspend` and to one that keeps running; nothing added to the system-call round trip (D95); signals sent to the process (`kill`, `rt_sigqueueinfo`) taken by the caller or, if it blocks them, by the first thread that does not, woken or interrupted to look, a thread that unblocks one later or waits for it in `rt_sigtimedwait` taking it, `rt_sigpending` reporting them; Rust's std program handles a `kill` to itself in a thread that does not block it (D96); signals interrupt `futex` waits and sleeps as on Linux: `nanosleep` and `clock_nanosleep` write the time left and fail with `EINTR`, an untimed `futex` wait restarts after an `SA_RESTART` handler and fails with `EINTR` otherwise, a timed one fails with `EINTR`, calls restarted by the HAL's `restart_syscall`, the sender reaching the waiter through its thread's wait slot in one futex bucket, a stopped thread leaving the process's signals to the others (D97); the console's line discipline: `read` from descriptors 0 to 2 in canonical mode (echo, erase with delete and backspace, word erase, kill, end of file, `^C` and `^\` discarding the input and signalling the console's foreground, the last thread that read it or polled it) and non-canonical mode (`VMIN`, `VTIME`), restarted after an `SA_RESTART` handler, `TCSETS`, `TCSETSW`, `TCSETSF` and their `termios2` forms, `TCFLSH`, `FIONREAD`, and `poll`/`ppoll` reporting `POLLIN` and woken by input (D100); a descriptor table per process shared by its threads (`close`, `dup`, `dup2`, `dup3`, `fcntl`, `close_range`, `FIONBIO`, `FIOCLEX`, `FIONCLEX`), the console an open file description on descriptors 0 to 2, `readv` and `O_NONBLOCK` reads (D101); children's ends: a process table (parent, exit signal, group, session, zombie), `wait4` and `waitid` (interruptible, `WNOHANG`, `WNOWAIT`, `__WCLONE`, `__WALL`), `SIGCHLD` with `CLD_EXITED`/`CLD_KILLED`, ignored `SIGCHLD` and `SA_NOCLDWAIT` reaping at once, orphans to the nearest `PR_SET_CHILD_SUBREAPER` ancestor, init or the kernel, `setpgid`, `getpgid`, `getpgrp`, `setsid`, `getsid`, the console a session's controlling terminal with a foreground group (`TIOCSCTTY`, `TIOCGPGRP`, `TIOCSPGRP`, `TIOCGSID`) that `^C` and `^\` signal, its leader's end hanging it up (D105); next: interrupting only the target's CPU; a process-wide completion for launchers, with `wait4`; C programs linked with musl; the SWAPGS fence before programs can set GS or map low pages (D49) |
| M7 | VFS and a first file system | **in progress** — the VFS core (a `FileSystem` trait, nodes, Linux's path walk), a read-only initramfs built by xtask from `initramfs/` as a `newc` cpio archive, embedded in the kernel and mounted as `/`, served in place; `openat`, `read`, `pread64`, `lseek`, `getdents64`, `fstat`/`newfstatat`/`statx` on files, `getcwd`, `chdir`, `fchdir`, a working directory per process (D102); `execve` and `execveat` from a file of the initramfs, loaded into a new process with the caller's id and exit record, with Linux's checks and errors before the point of no return, signal actions reset, close-on-exec descriptors closed, other threads ended, and the test kernels' programs in `usr/libexec/` (D103); next: tmpfs, devfs, `#!` scripts, setuid on exec (issue #94) |
| M8 | virtio drivers (console, block, net, rng, input) | **in progress** — the polled virtio core: virtio-mmio version 2 and modern virtio-pci behind one `Transport`, the reset, feature and status handshake, the split virtqueue with the spec's barriers, DMA regions freed only after the device's reset reads back 0; virtio-rng returns bytes on aarch64, riscv64 and x86_64 (D111); the doorbell ordered after the ring's writes for devices, virtio-gpu 2D on all three (D112), virtio-input keyboard, tablet and mouse on all three (D113), virtio-blk (polled, read and write) with disks in the driver model on all three (D130); virtio-console's agent channel (D136); virtio-net with an IPv4 stack of our own, DHCP, UDP and ping sockets, `ip`, `ping`, `nslookup` and `net_status` on all three (D157, networking part 1); TCP stream sockets, `fetch` over HTTP and the MCP tools `net_ping`, `net_lookup` and `fetch` (D158, networking part 2); HTTPS with a TLS 1.3 client and cryptography of our own, and the network clock by SNTP (D159, networking part 3); next: interrupts instead of polling, the kernel's generator reseeded from virtio-rng |
| M9 | More architectures through the HAL | |
| M10 | Userland: init, shell, core utilities, and the built-in MCP server with its permission model (default deny, local only): every command is also an MCP tool (D21, D22); first size/memory/boot budgets (D24); the first installer: a compressed base image that downloads and installs in seconds, with download size, install time and first-boot time measured (D69) | **in progress** — init as pid 1, started by the release kernel, with `reboot(2)` (D106); next: the `eggsh` shell and the core utilities |
| M11 | Graphics stack (virtio-gpu first) and compositor | **in progress** — the polled virtio-gpu 2D driver and the kernel's display API: a framebuffer on scanout 0, damage, fenced transfers and flushes (D112); the Egg Mono font and the graphical terminal, the console's session on the display (D114); the minimal desktop with the terminal's window (D115); next: the cursor queue, display events, interrupts instead of polling, 3D (virgl, Venus), a userland compositor |
| M12 | Adaptive shell, designed by Claude Fable from `docs/ux/` (D16, D23): desktop first, then phone/tablet and TV | |
| M13 | Real hardware and vendor GPU drivers | |
| M14 | Component manager (D18) and app capability registry: installed apps published as CLI commands and MCP tools; agent permissions and audit log (D21); components downloaded on demand in the background, delta updates applied atomically (D69) | |
| M15 | Compatibility layers, native not emulated (D17), each an optional component exposed to AI through M14: Linux userland, Windows and macOS side by side, then Android and iOS (order: D34) | **in progress** — Windows console programs on x86_64 and aarch64, parts 1, 2a and 2b: the PE32+ loader and a kernel32/ntdll subset as an optional user-mode layer (D118), then threads, waits, locks, error codes, UTF-16 and directory listing (D121), then ARM64 programs on aarch64 (D124); static Android console programs (NDK, bionic) through the Linux layer on aarch64 and x86_64 (D119); macOS console programs on aarch64 and x86_64, part 1: a Mach-O loader (thin and fat, chained fixups) and a libSystem subset as an optional user-mode layer (D120), and part 2: threads, thread-local variables, locks, `getopt`, calendar time and the C locale, so small tools and Rust std programs run (D123) |
| M16 | Editions as component presets (D18): personal, professional, enterprise, server | |

Timer follow-ups (D26): on x86_64, read the TSC/APIC frequencies from CPUID 0x15 and hypervisor
leaves instead of measuring, TSC-deadline mode, and the ACPI PM timer or HPET as fallback
reference clocks when there is no 8254; `stimecmp` directly when RISC-V Sstc is present; GICv3; device-tree timebase and GIC
addresses (M5).

Paging follow-ups (D29, D33): a separate exception stack (x86 IST, AArch64/RISC-V stack switch) so a guard-page hit is reported; TLB maintenance for changing live tables (M4; user address spaces need none yet, D45); honour device-tree `no-map` reservations (M5); AArch64 data-cache maintenance for memory written with the MMU off, the boot tables included, and a boot direct map built from the device tree instead of QEMU `virt`'s fixed device/RAM split (real hardware, M13); secondary CPUs start on the kernel's tables, global kernel pages and PCID (M4); a device window behind a mapping call instead of the direct map's (done for drivers, D110: the early devices still use the direct map's); unmapping device windows, with cross-CPU TLB shootdown for kernel mappings (M5); RAM above 4 GiB on x86_64; on x86_64, map ACPI tables on demand and reserved PVH ranges uncached instead of direct-mapping them cached (M5; early device pages and the MCFG's ECAM are already left out, D38, D110); keep the PVH start info and memory map in `physical_memory` even when a loader map has more than 62 disjoint entries; hardware probes in the boot self-tests that the low half, the image's direct-map alias and the stack guard page really fault (they check the tables in software today); 1 GiB pages on x86_64 (CPUID); Sv48/Sv57 chosen at boot on RISC-V when Sv39's 256 GiB is not enough; Svpbmt.

Address-space follow-ups (D45): (threads that carry an address space, switched only when it
differs: done, lazily at the point of use, D48; the first trip to user mode and back: done, D47);
ASIDs and PCID; (per-page TLB maintenance on this CPU: done, D52, `drop_user_translations`;
mappings added while active: done, D52); (cross-CPU TLB maintenance for the memory calls: done, D53, CPUs that keep the address space
loaded are marked stale and switch again before using it); (a shootdown once `clone` runs a
process on several CPUs at once: done, D94, the wake-up interrupt and a wait per CPU); the
shootdown by broadcast invalidation where the CPU has it (AArch64 `tlbi ...is`, RISC-V's SBI
remote fence or Svinval), ranged instead of every user translation, and only to CPUs running
the process rather than every CPU that keeps it loaded (D94); `with_active` and
`with_user_memory` of a spread address space from another CPU; fault-around
and large pages for demand-zero regions after measuring; a region tree that tracks gap sizes past
about a thousand regions (D52); from PR #44's review: table entries written with an atomic
release store, kernel copies that drop the regions lock between pages once `clone` exists, a
shared zero page for kernel reads of untouched memory, a region for the HAL test program's code; PAN, SMAP and SMEP; (user faults that end the process: done, D50).

User-mode follow-ups (D47): (processes whose threads run their program in a loop over `run_user`,
preempted on return to user mode: done, D48); on x86_64, IST stacks for NMI, double fault, machine check and debug exceptions before any
untrusted program runs (`syscall` and `sysretq` leave a moment in ring 0 on the program's stack),
the paranoid GS check in those handlers, and reloading SS for AMD's `sysretq`; FP/SIMD state per
thread, saved lazily, once programs may use it (it traps on every architecture, whatever firmware left: D49); (TLS registers (`TPIDR_EL0`,
`FS` base) in the context: done, D57), and `KERNEL_GS_BASE` per program with `ARCH_SET_GS` (D57); the Linux system calls themselves
(M6); a measured system-call cost outside QEMU `-icount` (M13). From PR #39's independent review:
a self-test that interrupts a program mid-pattern and checks every register (the `iretq` and
interrupt-exit paths); `SCTLR_EL1.UMA` and `E0E` written explicitly, not left to firmware (M13);
`IA32_CSTAR` at a safe stub before any 32-bit user segment exists; on AArch64, `SCTLR_EL1.UCT`,
`UCI`, `DZE` and `nTWE`/`nTWI` set as Linux allows (EL0 `mrs ctr_el0`, `dc cvau`, `ic ivau` for
`__clear_cache` and JITs, `dc zva`, `wfe`), found by the D52 research; DPL-3 gates for `int3` and
`into` once user debuggers matter; UMIP where the CPU has it; `TSS.RSP0` cleared when `run_user`
returns. From the D47–D48 review (D49): an NMI from ring 3 handled by the kernel, not as the
program's exception, and NMIs unblocked with an `iretq` (with the IST stacks); AArch64
`PSTATE.DIT`, `SSBS` and `TCO` for programs; `-Zharden-sls` for rustc's own `ret`s; RISC-V
`scounteren` written explicitly (harmless today: counter reads cannot touch the timer).

Speculation mitigations (D49), each chosen per CPU at boot: the SWAPGS `lfence` (a second copy
of the x86 entry stubs) **before a program can set its GS base or map pages near address 0**
(M6; `mmap`'s floor, 64 KiB as Linux's `mmap_min_addr`, is in place, D53); KPTI with PCID, MDS buffer clearing and Retbleed **before x86_64
runs anything but our own test programs on a real Intel CPU** (M13 at the latest); KPTI for
affected Arm cores (Cortex-A75).

Fault follow-ups (D65): `AT_MINSIGSTKSZ` in the auxiliary vector (Rust's std sizes its
alternate stacks from it); `#MF` and `#XM` with their `FPE_*` codes and `trapno`, and AArch64's
trapped FP exceptions, with D54; AArch64's top byte cleared from `si_addr` unless
`SA_EXPOSE_TAGBITS`, if TBI is turned on for programs; RISC-V misaligned accesses emulated
as Linux does (QEMU's harts do them in hardware); `SIGBUS` `BUS_ADRERR` past the end of a
file mapping (M7); `TRAP_TRACE` and `TRAP_HWBKPT` with single-step and debug registers
(`ptrace`); `/proc/self/maps` and `RLIMIT_STACK` for glibc's `pthread_getattr_np`, so Rust's
std finds the main thread's guard page and reports an overflow by name. From PR #58's
independent review: RISC-V's `si_addr` (`sepc` or `stval`) and `si_code` for access faults
and misaligned accesses checked against a real Linux under QEMU; `TRAP_TRACE` for a single
step a program set with `popf`; a non-canonical `rip` (only through `rt_sigreturn`) reported
as `#GP` (`SI_KERNEL`, `trapno` 13) rather than a fetch page fault; NMIs and machine checks
from user mode handled by the kernel instead of becoming the program's `SIGSEGV`;
(AArch64's `fault_address` 0 for `SIGILL` and `SIGTRAP`: done, D67); checked against a real
Linux: whether 6.4 and later pass the syndrome for undefined instructions, and what alignment
faults and external aborts report in `fault_address` and `esr_context`; whether AArch64's
`rt_sigreturn` sends `SIGSEGV` for a frame whose `pstate` is not EL0t (we mask it and
continue); a QEMU test of `SIGILL` and `SIGTRAP` handlers (`ud2`, `brk`, `ebreak`) with their
`si_addr` and resume point.

Process follow-ups (D48): preemption on whenever user programs run (with it off, as for kernel
threads, a program that never traps keeps its CPU); waking CPUs that keep an ended process's
address space loaded while they stay busy with kernel threads, or idle after a look that raced with
another CPU switching away from it (an idle CPU lets it go otherwise); (TLB shootdown, to every CPU that has the
address space loaded, once a shared process's mappings may change: done, D94); (pages owned by the address
space: done, freed with it, D52, and by `munmap`, D53); (user faults that end the process with Linux's default signal, and the
exit status `exit_group` records: done, D50); signal delivery to handlers (their state: done, D60; default actions and `SIG_IGN`: done, D63; handlers: done, D64; queued `siginfo_t`: done, D68; between a process's threads: done, D95, process-directed ones taken by any thread: done, D96; `futex` waits and sleeps interrupted, `EINTR`, `rem`, `SA_RESTART`: done, D97; `rt_sigsuspend` and `pause`: done, D70); (a process's threads, `clone`: done, D91, in parallel on every CPU, D94)
and `wait4`; (user-memory copies racing another thread's writes: decided by research, D94: the
copy stays a plain one through the direct map, each byte read once into a kernel buffer, the
bytes unspecified as on Linux, and words used as atomics go through `Process::with_user_word`);
user-memory copies at user addresses with fault fixups when something needs them; (a cap per `write` call, Linux's `MAX_RW_COUNT`, and a preemption
point per chunk, so one large write cannot hold its CPU: done, PR #46).

Exec follow-ups (D51): (a kernel random number generator for `AT_RANDOM`, and `getrandom`:
done, D56); layout randomization
of the PIE base, stack and `mmap` area; (demand paging, so the stack grows into its 8 MiB
reserve and its pages cost frames only when touched: done, D52); `execve` itself, with the image read from
a file (M7) and the process's old address space replaced; dynamically linked programs, with
the interpreter loaded at its own base and `AT_BASE` set (M7); `AT_SYSINFO_EHDR` with a vDSO
(fast clock reads), `AT_MINSIGSTKSZ` with signal frames; `AT_HWCAP` recomputed when FP/SIMD
state is saved per thread; RWX segments or a JIT path only if a real binary needs one; a
(faster zeroing of frames and page tables: done, D56); pre-zeroed frames; before `execve`
takes outside input, a cap on a program's eagerly mapped pages and the initial stack written
straight into its frames (no 2 MiB heap buffer); `CPUID` for `AT_HWCAP` read once at boot;
two segments on one page if a real binary needs it.

FP/SIMD follow-ups (D54): AVX and AVX-512 through `XSAVE`/`XSAVEOPT`, SVE/SME on AArch64 and V
on RISC-V, each with a first-use trap of its own; FP/SIMD state in signal frames (`MXCSR`
checked on `sigreturn`); `fork` and `clone` copying or starting it; #XM and #MF with their own `FPE_*` codes (they are `SIGFPE` with `FPE_INTDIV` now);
the AMD x87 pointer reset skipped where CPUID 0x80000008:EBX[2] says it is not needed. From
the D83 review: the CPU's own `MXCSR_MASK` in the frames of threads without registers;
`SIGFPE` for AArch64's trapped floating-point exceptions (EC 0x2C); a build check that the
kernel image has FP/SIMD instructions only in the HAL's `fp` code and the test programs; a
QEMU test of `rt_sigreturn` with reserved `MXCSR` bits; `CR4.PKS` cleared too, whatever
firmware left (it affects only the kernel).

Memory-call follow-ups (D53, D77): a "shared" mark on `MAP_SHARED` anonymous regions before
`fork`, so it shares their pages instead of copying them (then `MADV_REMOVE`, `MADV_FREE`'s
`EINVAL` and `mremap` with an old size of 0 on shared memory, as on Linux); `fork` keeping
`MADV_DONTFORK` and `MADV_WIPEONFORK` per region (D77 accepts them with nothing to apply them
to yet); a shared zero frame for `MADV_POPULATE_READ` and read faults; (`madvise` and
`mremap`: done, D77); a `mremap` benchmark once `realloc` of large blocks shows in a real
workload; `MAP_POPULATE` and `MAP_GROWSDOWN` when a program needs them;
`PROT_BTI` enforced (the `GP` bit) once BTI is reported in `AT_HWCAP2`; file mappings (M7);
`RLIMIT_DATA` and `RLIMIT_AS` for `brk` and `mmap`; randomized `mmap` base and break with the
random number generator; from PR #45's independent review: (`munmap` and a shrinking `brk`
free their frames after the translations are dropped, taking the frame allocator's lock once:
done, PR #46), and anonymous `MAP_SHARED_VALIDATE` and `MAP_SHARED | MAP_GROWSDOWN`
checked against Linux's observed behaviour (`EINVAL` there, possibly, where we accept or say
`EOPNOTSUPP`). From the D83 review: regions that know what backs them (M7), so
`MADV_DONTNEED` on a program's ELF segments reads the file's contents again, `MADV_FREE` there
is `EINVAL`, and moving the signal trampoline's page moves its recorded address too.

Clock follow-ups (D73): real-time clock drivers (PL031, Goldfish, CMOS) setting the wall
clock at boot, after M5; a vDSO for clock reads without a system call, with the counter
readable from user mode again (D49 turned it off); per-thread CPU time for the CPU-time
clocks, `getrusage` and `times`, with the owner's OK on its switch-path cost; the
invariant-TSC check (SMP follow-ups) so `CLOCK_MONOTONIC` never goes backwards across x86_64
CPUs; absolute realtime sleepers re-evaluated once the wall clock can be set; `EINTR` with the
time left in `rem` once signals can arrive during a sleep.

Signal-queue follow-ups (D72): a queue that grows by fixed-size chunks instead of one
contiguous buffer, so a long queue of real-time signals never needs a large contiguous block
or copies itself under the signals lock before it reaches `RLIMIT_SIGPENDING`.

Startup-call follow-ups (D55): process ids reused after `wait4` reaps them (the process table
exists, D105);
`set_tid_address`'s word cleared and woken and the robust list released at thread exit, with
`clone` and futexes; (parents for `getppid` with `fork`: done, D104, reparenting D105); `sethostname` and `setdomainname`; the
same release in `/proc/sys/kernel/osrelease` and `/proc/version` (procfs); a `writev`
benchmark once a sink faster than the console's UART exists (M7), and `getpid`'s round trip
as the cheapest real call; (`writev`'s two edge cases from PR #48's review checked against
Linux: done, D58); (`writev` with a partly unreadable array whose readable entries hold a
negative length `EINVAL`, as Linux reads the entries in order: done, D62); `init` as pid 1 (M10).

Descriptor follow-ups (D61): (a descriptor table per process with `close`, `dup`, `dup2`,
`dup3`, `fcntl` (`F_DUPFD`, `F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`),
`close_range`, `FIONBIO`, `FIOCLEX` and `FIONCLEX`: done, D101); the descriptor table's
follow-ups (D101): `F_SETFL`'s `O_ASYNC`, `O_DIRECT` and `O_NOATIME`, record locks
(`F_GETLK`, `F_SETLK`, `F_OFD_*`), `F_SETOWN` and `SIGIO`, `O_LARGEFILE` in `F_GETFL` once
`openat` sets it, a descriptor table above 4 MiB of slots; (console input with `read`,
`POLLIN` and a line discipline behind `TCSETS`, `TCSETSW` and
`TCSETSF`, pollers woken when input arrives: done, D100); the line discipline's follow-ups
(D100): (`readv`, `O_NONBLOCK` reads: done, D101), `VREPRINT`, `VDISCARD`, `VSUSP` with job control, `IXON`
flow control, output flags that the UART driver obeys, tab erase from the column the output
left (output column tracking), virtio-console input (M8); `SIGWINCH` on
`TIOCSWINSZ`; sessions
and job control (`TIOCGPGRP`, `TIOCSPGRP`, `TIOCSCTTY`) with `setsid` (M10); `select`,
`pselect6` and `epoll` (M7); `poll` arrays copied in pieces with preemption points
between them, once programs poll many thousands of descriptors; checked by observation on a
real Linux: the order of `ppoll`'s checks, whether `sigsetsize` is checked with no mask,
`CLOCAL` on a serial console, whether current glibc reads settings with `TCGETS2`, and whether `poll`
of no descriptors at an address past user space is `EFAULT`; (the time not slept written back
by every `ppoll` with a timeout that is not zero, also when something is ready at once: done,
D62); only the `revents` fields written back, as Linux does, once `clone` lets another thread
change the array during a wait (today the whole array is copied back).

Thread-pointer follow-ups (D57): `CLONE_SETTLS` with `clone`; FS and GS selectors saved and
reset per program; `ARCH_SET_GS` with the SWAPGS fence (D49); FSGSBASE for programs (the FS
base then saved on the way out); a per-CPU check instead of AArch64's unconditional `msr
tpidr_el0` if real hardware shows it costs; `set_thread_area` and 32-bit programs; with
FSGSBASE, a new `fs_base_id` whenever the saved FS base changes, and `ARCH_GET_FS` reporting 0
after a program loads an FS selector (D58).

Random-generator follow-ups (D56): RNDR on AArch64 and Zkr on RISC-V; timer jitter, UEFI
and TPM seeds (M13); virtio-rng and periodic reseeding (M8); `/dev/random` and `/dev/urandom`
(M7); `random.trust_cpu` and `random.trust_bootloader`; vmgenid reseeding; a vDSO `getrandom`;
`EINTR` with signals; `dc zva` and Zicboz for zeroing; the seeded state as an MCP and CLI
capability (D22); ChaCha20's working state wiped within the benchmark gate (BLAKE2s's: done,
D58).

Thread follow-ups (D35): guard pages below thread stacks in a kernel virtual area, with a per-CPU
cache of free stacks, once exception stacks exist; a self-test measuring stack high-water to
confirm 16 KiB on every architecture (interrupt frames now land on thread stacks at
preemption points); revisit the stack size when FP/SIMD state or nested interrupts add frames.

Preemption follow-ups (D37): (involuntary preemption on return to user mode: done, D48);
(preemption on a secondary CPU exercised by a boot self-test: done, D44; per-CPU tick state and
run queues: done, D43); a measured worst-case gap between
preemption points as a latency benchmark; a debug-build check that no lock is held at a
preemption point, nor an address space or user-access window open (D46: an address space made
active over another on one CPU is already caught; RISC-V's `sstatus.SUM` is per CPU, not per thread).

Sleep and wait-queue follow-ups (D39): waits with a timeout (a timed park: done, D89, `thread::Parker`; wait queues themselves still wait without one); a condition-variable wait for data
behind a `SpinLock` and a futex-style wait (M6); wait-queue locks of their own (per-CPU idle
threads: done, D43); waking threads from device interrupt handlers (M8); RISC-V Sstc, arming the timer
without an SBI call; per-thread CPU time from clock reads at each switch; a pairing heap or
timer wheel for the sleepers if a CPU ever has many.

SMP follow-ups (D40, D41, D43, D44): per-CPU run-queue locks and wait-queue locks instead of the
one scheduler lock, once a multi-CPU workload or a benchmark outside `-icount` can show the
contention (D45: a lock-free local run queue with per-CPU wake lists is the intended design); an idle-CPU mask for placement and balancing; wake-time placement onto idle
CPUs, balancing with preemption off, CPU affinity, and balancing by load, cache and NUMA distance
(D44); canaried idle stacks on the secondaries (load balancing: done, D44; per-CPU run queues, idle
threads, waking an idle CPU with `cpu::wake`, and a secondary waking the boot CPU: done, D43;
per-CPU interrupt setup and timer interrupt counts: done, D41); GICv3 SGIs and more than 8 CPUs on
aarch64; TLB shootdown when live tables change; x2APIC for APIC ids of 255 and up;
an invariant-TSC check, since threads now move between CPUs (D44) and may read two CPUs' clocks; booting from a CPU whose affinity is
not 0 on aarch64, the PSCI conduit from the device tree and a full known `SCTLR_EL1` (M13); a
console lock for messages from any CPU; starting the CPUs in `boot`, not in a self-test.

Heap and buddy follow-ups (D30, D31): returning empty slabs to the frame allocator (a live-block
count per frame); per-CPU caches instead of one heap lock and a high watermark on the free stack
(M4); a physical address limit for 32-bit DMA (M8); one buddy bitmap per range; anti-fragmentation
grouping.

Comment cleanup (D66): code keeps only `// SAFETY:`, lint directives and rustdoc on public
items, which `cargo xtask docs` checks in CI (done, PR #59). Done: drivers; xtask's `bench`,
`elf` and `qemu`; the Cargo, CI, Dependabot and funding files, whose notes are now in
`docs/build.md`; the kernel's `linux.rs`, whose test comments only repeated their assertions
or what D50–D75 record (PR #71, after PR #70 touched it); the kernel's `linux/memory.rs`
(PR #78); `rust-toolchain.toml` and the CI image's `Dockerfile` and `ci-image.yml`, whose notes
are in `docs/build.md` (PR #79). Next: the HAL, then the kernel's other modules, a few hundred
lines per PR. Last: the other files PR #47 changed (`xtask/src/main.rs` among them). `cargo xtask lint`
(D122) fails on any new disallowed comment and lists the files still to clean in
`xtask/lint-legacy-files.txt`; each cleanup PR lowers it with `cargo xtask lint --shrink`.

Setup follow-ups from the 2026-10-22 review (all done: exception vectors in D25, the benchmark harness
in D27):
- **Benchmarks: done** (D27). `cargo xtask bench` boots each architecture under QEMU `-icount`
  and fails on a regression against `benchmarks/baseline.txt`; CI runs it report-only, failing only a
  2x slowdown, and the weekly performance pass enforces the exact gate (D122). Every new hot path
  (frame allocator, page faults, scheduler, syscalls) adds its benchmark there. Cycle-accurate
  numbers on real hardware come with M13.
- **A BSS self-test that can fail: done.** The QEMU boot tests look the kernel's `.bss` probe
  up in its ELF symbol table and fill it with a non-zero pattern through QEMU's `loader`
  device before the kernel starts, so a missing zeroing loop fails `bss_is_zeroed` (checked on
  aarch64, riscv64 and x86_64 by removing the loop).

UX track (runs alongside, from the start): `docs/ux/` — **in progress** (switcher-habit research).

## Support matrix

### CPU architectures (QEMU boot test in CI is the bar for "done")
| Architecture | Status | Notes |
|--------------|--------|-------|
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (PSCI `CPU_ON`) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt`, EL1, PSCI power-off and reset (D9, D10, D106) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary harts start (SBI HSM) and take their own timer and wake-up interrupts, threads on every hart, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off and reset, legacy SBI shutdown fallback (D9, D106) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (INIT-SIPI) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, per-CPU GDT and TSS, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off, reset through the reset control register or the 8042 (D15, D106) |
| loongarch64, ppc64, s390x, mips64, arm (32-bit), riscv32, sparc64 | planned | M9 |

### GPUs
| GPU | Status |
|-----|--------|
| virtio-gpu | **done**: 2D, polled, one scanout (D112; cursor, 3D: planned, M11) |
| Intel, AMD, NVIDIA, Apple, Arm Mali, Qualcomm Adreno | planned (M13) |

### Device classes
| Class | Status |
|-------|--------|
| Server, desktop, laptop (QEMU first) | planned |
| Phone, tablet | planned |
| Embedded | planned |

### Drivers
| Driver | Status |
|--------|--------|
| PL011 UART (Arm), transmit and interrupt-driven receive | **done** |
| ACPI tables: RSDP/RSDT/XSDT/FADT, `\_S5` from the DSDT (power-off), MADT local APICs (CPU discovery), I/O APICs and interrupt source overrides | **done** (full AML interpreter: planned, M5) |
| Flattened device tree: memory nodes and reservations, `/cpus`, `/chosen` seeds (taken and removed, D56), a hart's PLIC context, every node for device discovery (`compatible`, `reg`, interrupts through `interrupt-parent` and `interrupts-extended`, status, DMA coherence, D110) | **done** (`ranges` translation: planned, M5) |
| PCI Express: the MCFG, ECAM configuration space, enumeration through bridges, BAR sizing, the capability list (D110) | **done** on x86_64 (PCI on aarch64 and riscv64, a BAR allocator, MSI-X: planned, M5) |
| NS16550 UART (MMIO, any `reg-shift`, and x86 port I/O), transmit and interrupt-driven receive | **done** |
| Arm GICv2 (distributor + CPU interface, per-CPU setup, SGIs between CPUs, SPIs routed with their trigger mode) | **done** (GICv3: planned, M5) |
| RISC-V PLIC (priorities, per-context enables, claim/complete; the hart's context from the device tree) | **done** (AIA: planned, M5) |
| Arm generic timer (EL1 virtual timer) | **done** |
| RISC-V SBI timer and IPI | **done** |
| x86 local APIC (xAPIC), on every CPU: its timer, INIT/start-up IPIs and fixed IPIs; 8259 PICs (remapped and masked); 8254 PIT (frequency measurement) | **done** (x2APIC, HPET: planned) |
| x86 I/O APIC (lines routed by the MADT's overrides) | **done** (MSI-X: planned, M5) |
| virtio core: virtio-mmio (version 2), modern virtio-pci, the split virtqueue, polled (D111) | **done** (interrupts: planned, issue #97 slice 3) |
| virtio-rng (polled) | **done** (D111) |
| virtio-gpu 2D (polled): display mode, framebuffer on scanout 0, transfers and flushes of damaged rectangles (D112) | **done** (cursor queue, interrupts: planned) |
| virtio-input (polled): keyboard, tablet and mouse, capabilities from the device's configuration, the US keymap and key repeat, the input event queue (D113) | **done** (interrupts, LEDs, keyboard layouts: planned) |
| Graphical terminal on the virtio-gpu display: Egg Mono 8x16, the ANSI subset, scrollback, damage-only rendering (D114) | **done** (cursor blink, 2x scale: planned) |
| virtio-blk (polled): capacity, read-only disks, reads, writes and flushes of 512-byte sectors, one request at a time; a FAT12/16/32 reader mounts the host folder at `/media/host` (D130) | **done** (several requests in flight, interrupts: planned) |
| EggFS, our own journaled disk file system: `/home` and `/Applications` on a disk image, metadata journal, copy-on-write data, CRC-checked commits, crash recovery at mount, `sync`/`fsync` (D131, docs/eggfs.md) | **done** (repair tool, links, several requests in flight: planned) |
| virtio-console (the agent channel's named port, D136) | **done** |
| virtio-net (polled): hardware address, link state and MTU from the configuration, 16 receive and 16 transmit buffers (D157) | **done** (interrupts, offloads, several adapters: planned) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~3%**. The kernel boots on three architectures (x86_64, aarch64, riscv64) to
a desktop with windows; static Linux programs, Windows console and GUI programs (part 1), and
Android and macOS console programs run natively; files in `/home` survive reboots on an EggFS
disk (D131); no network or real hardware yet.

| Area | Missing |
|------|---------|
| Kernel | per-CPU scheduler locks, the Linux syscalls (processes run their programs in user mode on every CPU, preempted on return to user mode, and make system calls; user address spaces exist; CPU exceptions are caught and reported; timer interrupts work; physical frames are allocated; the kernel runs on its own page tables in the top 2 GiB with W^X, a direct map and an empty low half; it has a heap and contiguous frame allocation; kernel threads switch on yield and, with time slices, at preemption points; they sleep and wait in wait queues, and an idle CPU takes no tick; every CPU starts, has its per-CPU block, takes its own timer and wake-up interrupts and runs threads, and idle CPUs take threads from busy ones) |
| Hardware | the other architectures; every driver beyond the early UARTs and the polled virtio core with virtio-rng (D111), virtio-gpu 2D (D112), virtio-input (D113) and virtio-blk (D130); device interrupts for drivers |
| Compatibility | Linux ABI (static ELF programs load and start with Linux's stack; `write`, `writev`, `exit`, `brk`, `mmap`, `munmap`, `mprotect`, `getpid`, `gettid`, `uname`, `set_tid_address`, `set_robust_list`, `getrandom`, `sched_yield`, `arch_prctl`, the resource limits and the signal state's calls, `poll`, `ppoll` and the console's `ioctl`s work, with `AT_RANDOM` from the kernel's generator; pages are mapped on first touch and the stack grows on demand; each thread keeps its own thread pointer; signals are sent and delivered to handlers on Linux's frames, faults included, real-time signals are queued with their `siginfo_t`, and `rt_sigsuspend` and `pause` wait for a handler; the clocks read and programs sleep; the console's status is a character device and it cannot seek; `futex` waits and wakes; `clone` starts threads, Rust's `std::thread` runs, each on its process's one CPU until TLB shootdown; FP state per thread; signals between a process's threads and to the process sent and delivered, interrupted futex waits and sleeps still missing; new processes and files still missing), Android, Windows (console programs, and GUI programs' windows, painting, input and message
boxes, part 1, D129; controls, menus, timers and file pickers, part 2, D145; C programs on MSVC's C runtime, D146), macOS and iOS layers |
| Desktop / UX | compositor, apps, maximise, pointer, boot screen (the minimal desktop works: wallpaper, top bar with clock and power menu, the shell's terminal in a window moved, focused and minimised with the mouse, D115); design in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

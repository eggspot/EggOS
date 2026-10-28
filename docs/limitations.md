# Limitations of EggOS 0.3.0

EggOS 0.3.0 is pre-alpha: the first release you can boot and type in, to try, not to use. This
page lists what it does not do yet, so nobody has to find out by trying. The plan for each is in
the [roadmap](roadmap.md); the reasons behind the current state are in the
[decisions](decisions.md) cited.

## Where it runs
- **QEMU only.** The images boot on QEMU's `virt` machines (aarch64 with a Cortex-A72 and GICv2,
  riscv64 through OpenSBI) and `q35` (x86_64), with 128 MiB of memory and 4 CPUs, as the run
  scripts start them. No real hardware, no other hypervisor, no UEFI or bootable disk image yet.
- **No acceleration.** QEMU emulates the CPU (TCG), so the system runs slower than it will on
  hardware; boot to the prompt still takes about a tenth of a second.
- **Devices:** the serial console, virtio-gpu (2D, polled), virtio keyboard, tablet and mouse,
  virtio-blk (polled, for the host folder) and on test kernels virtio-rng. Nothing else is
  driven: no network, no sound, no USB, no real-time clock (D110-D113, D130).

## What the system can do
- **Agents, under grants for the whole boot.** The built-in MCP server (D136, D137,
  docs/mcp.md) lets an MCP client read files in `/home` and `/media/host` and list processes,
  windows and installed apps; with grants given when starting EggOS (`--mcp-allow
  write,run,install`) it also writes and trashes files below `/home/root`, runs programs and
  installs, opens and uninstalls apps. Grants hold for the whole machine and boot: no per-agent
  grants, no revoking them while EggOS runs; `run` is full control, the audit log included.
  Commands run as root (file permissions are not enforced yet). Reached only through the run
  scripts' `--mcp` (no network MCP). The audit log keeps at least 3.5 MiB and shows times since
  the boot until the RTC (item 29).
- **One disk, and no network.** The root file system is the read-only initramfs inside the
  image: `/init`, `/bin` (the shell and the core utilities), `/etc/motd`, `/etc/hostname` and
  `/usr/share/eggos/about`. `/home`, `/Applications` and `/var` live on the disk image the run
  scripts attach, formatted with EggFS, and survive reboots (D131); `/tmp` is in memory, as `/home` is
  with `--no-disk` (together at most half of the memory), and `/proc` has `cmdline`, `meminfo`,
  `uptime` and `ps` (D125, D137). Changes reach the disk within 5 s, at `sync` and at power-off; closing QEMU's
  window or a power cut loses the last seconds' changes, never the file system. EggFS has no
  hard or symbolic links, access times or repair tool yet, searches directories linearly,
  and keeps one request to the disk in flight; moving a file between `/home` and
  `/Applications` fails (`EXDEV`) as between two disks. A disk holding anything but EggFS or
  FAT is left alone: `--format-disk` erases the image. On Windows the image takes its full 1
  GiB when QEMU's `qemu-img` is missing, and nothing stops two QEMUs sharing one image (macOS
  and Linux refuse the second). A file removed while a program has it open is gone at once. A Linux program run from `/tmp`, `/home` or `/media/host` is read into
  memory whole first, so it must be smaller than 4 MiB (D130).
- **The host folder is read-only and limited by QEMU.** The run scripts' `--apps <folder>`
  shows a folder of your computer at `/media/host` through QEMU's `vvfat` driver (D130): it
  cannot be changed from EggOS (`Read-only file system`), it must fit about 500 MB, its top
  level holds fewer than about 250 names (QEMU refuses to start with "Too many entries in root
  directory" beyond), files of 2 GB or more are refused, names are compared without case for
  ASCII letters only, file times are your computer's local time shown as UTC, and changes made
  on your computer while EggOS runs are not seen. Only one folder can be attached. A Windows or
  macOS program in it runs only on an image that has its layer, which the release images do not
  carry yet (`cargo xtask run --add` adds it with a program; D118, D120).
- **The clock starts at the epoch.** Without an RTC driver the wall clock reads 1970 and the
  desktop's clock shows the uptime (D73, D115).
- **Linux programs: static ones only.** The kernel runs static Linux programs through its own
  Linux system-call layer; `cargo xtask image --add` builds them into an image, and nothing
  can be added to a running system yet ([Linux programs](linux-programs.md), D117). No
  dynamic linking, only a minimal `/proc` (`cmdline`, `meminfo`, `uptime` and EggOS's own `ps`, no
  `/proc/self` or `/proc/<pid>`), `/dev` holds only `window` and `agent`, no `mmap` of files but memfds, no copy-on-write `fork`
  (memory is copied at once, D104). Pipes work (D125), but `poll` on many descriptors wakes
  with every pipe, socket and console change. Local sockets work (D138): `AF_UNIX` stream
  sockets only (no datagrams or sequenced packets, no networking), socket files only in the
  memory file systems (`/tmp`, `/run`, `/var` and `/home` without a disk; `bind` on the disk
  is refused), descriptors passed with `SCM_RIGHTS` but never a socket that carries sockets
  itself, and at most 4096 descriptors in flight in all; no `epoll`, `eventfd`, `timerfd` or
  `signalfd` yet, which most graphical Linux toolkits need. Shared memory works through
  `memfd_create` (D139): `MAP_SHARED`, or `MAP_PRIVATE` read-only, of a memfd; not of other
  files (`ENODEV`), no `shm_open` or `/dev/shm`, no writable private mapping of a memfd, no
  executable or `PROT_NONE` mapping of one (`EPERM`), no `F_SEAL_WRITE` or
  `F_SEAL_FUTURE_WRITE`, no `SIGBUS` past the end (those pages read as zeros), `fallocate`
  with mode 0 only, a write, `fallocate` or mapping at a gigabyte-scale offset fails
  (`ENOSPC`, `ENOMEM`), and a process that maps one cannot `fork` (`posix_spawn` and `vfork`
  work).
- **Windows: console programs and simple GUI programs on x86_64 and aarch64 only.** Windows
  programs that use the kernel32, ntdll, shell32, user32 and gdi32 functions the Windows layer
  provides run natively, x64 programs on x86_64 and ARM64 programs on aarch64, added with
  `cargo xtask image --add` ([Windows programs](windows-programs.md), D118, D121, D124, D129):
  threads, waits, locks, directory listing and UTF-16 strings work, and GUI programs open
  windows, paint them with GDI, get the mouse and keyboard and show message boxes. Programs on
  a C/C++ runtime or Rust's std, other processes, other DLLs, structured exception handling,
  the system's controls (buttons, edits), menus, dialogs from resources, timers, fonts other
  than Egg Mono and text beyond ASCII, x64 programs on Arm (and ARM64 programs on x86_64) and
  riscv64 do not run yet; names are case-sensitive and local time is UTC.
- **Android: static console programs only, on aarch64 and x86_64.** Programs built with the
  Android NDK and linked statically against bionic run natively through the Linux layer
  ([Android programs](android-programs.md), D119). No apps (`.apk`), no dynamic linking, no
  system properties (all read as unset), no `logd` or binder, no riscv64.
- **macOS: console programs on aarch64 and x86_64 only, part 1.** Mach-O console programs
  (thin or fat, built for macOS 13 or later) that link only libSystem and use the functions
  the macOS layer provides run natively, arm64 programs on aarch64 and x86_64 programs on
  x86_64, added with `cargo xtask image --add` ([macOS programs](macos-programs.md), D120).
  Programs on Rust's std, Objective-C or Swift, threads, signal handlers, `mmap`, other
  libraries and frameworks, `.app` bundles and riscv64 do not run yet.
- **iOS apps are not in yet** (D17).
- **No MCP server, no components.** The built-in MCP server (D21), the CLI and MCP front-ends
  of every capability (D22) and the component manager (D18) come later.
- **One user, root.** Processes start as root; there are no logins or permissions to manage
  (D75).

## The shell and the core utilities
- `eggsh` runs commands with arguments, quoting, variables, `cd`, `exit`, `.` and scripts,
  pipelines and the `<`, `>` and `>>` redirections, with a history of recent lines on the
  arrows, but refuses `&&`, `||`, background jobs, subshells and command substitution, and has
  no wildcards, `if`, `while`, `for`, `case`, functions or `2>` ([shell](shell.md)).
- The core utilities are `cat`, `cp`, `echo`, `false`, `free`, `grep`, `halt`, `head`, `ls`,
  `mkdir`, `mv`, `poweroff`, `ps`, `reboot`, `rm`, `sleep`, `sync`, `tail`, `touch`, `true`,
  `uname`, `uptime`, `wc` and `wm`, each with the common options only ([core utilities](coreutils.md));
  they are command-line only, with no GUI or MCP front-end yet (`wm` is the window
  capability's CLI; its GUI is the desktop's own chrome and keys).
- The keyboard layout is US only (D113).

## The desktop
The desktop is minimal (D115, D127, D128): wallpaper, a top bar with the uptime and a power
menu, the shell in an 80x24 terminal window, and the windows programs open through
`/dev/window` ([windows from programs](windows.md)). You can focus any window by clicking it or
with Alt+Tab, drag it by its title bar, resize it from its edges and corners, maximise and
restore it (its green control, a double-click on the title bar, Super+Up and Super+Down), snap
it to a half (Super+Left and Super+Right), minimise it to a chip in the bar, open its window
menu (Alt+Space) and ask it to close (its close control, Alt+F4, Cmd+W); the pointer is drawn
on screen, in the shape of a resize zone or of what a program asks for. Settings
(`settings`, [Settings](settings.md), D132) picks the layout profile, Windows, macOS or Linux
habits (the controls' side and the shortcuts), kept on the disk across reboots. Not yet:
- a boot screen with the logo and progress (the plain wallpaper shows until init starts);
- the pressed look of the window controls, and the `Hand` and `Busy` pointer shapes (the
  panels' items have hover and pressed looks since D133);
- dragging a window partly off screen;
- resizing or maximising the terminal: its grid stays 80x24 until terminals are user programs;
- F11; the phone layout of the shell, 2x, dragging or arranging desktop icons, the caret's
  blink in the launcher's search field and keyboard focus in the Linux launcher's dash (D133);
- installing a missing Windows or macOS layer, `app install --force` and the MCP tools
  `app_install`, `app_uninstall` and `app_list` (with the MCP server): `app install` refuses a program whose layer is not in the image (D134, docs/apps.md);
- `.app` bundles that hold symbolic links (a framework's `Versions/Current`, for example):
  `app install` refuses them, as EggFS stores no symbolic links (D131, D134); the installer's
  chooser lists at most four programs and leaves out names longer than 64 bytes (`app install
  <path>` installs any);
- closing the terminal (its close control is disabled) and a second terminal: launching
  Terminal focuses the one console terminal, and installed console apps are typed into it, so
  they reach whatever reads the console at that moment (docs/apps.md);
- Settings, the installer's window (`app --window`, D134) and Files (`/bin/files`, D135),
  all served by the same program, are the programs with a window in the image (the window tests' `hello-window`
  and `win-paint.exe` are added only to their own images); Settings' Mouse and Components panes,
  key repeat and 2x come later; Windows GUI programs open windows
  (part 1, D129), macOS and Linux GUI apps do not yet (items 39 to 41);
- 3D graphics;
- a terminal on a display smaller than 658x530 (the macOS profile's top bar and dock take 100
  rows): the window does not fit every profile's work area, and the display then shows no
  terminal (the serial console still works);
- Files (D135, docs/ux/desktop-v0.3-gui.md section 8.1) selects one item at a time (no
  multiple selection, dragging or drop), has one list view (no icons, thumbnails or columns
  to sort by), hides names starting with `.`, shows at most 512 entries (32 KiB of names) of
  a folder, and shows no free space in its status bar (no `statfs` yet); its clipboard holds
  one item inside the Files process, shared with no other program and lost when Files
  closes; times follow the wall clock, which counts from the boot until an RTC driver lands
  (item 29); a text file opens with `cat` in the Terminal window until the Text Editor (item
  46); each click on the desktop's `Host share` opens another Files window; folders nest at
  most 32 levels deep in a copy, a move across file systems or a delete; the MCP tools for
  files come with the MCP server (item 28, after the release);
- the Trash holds only what is on the home folder's file system (`/home`): an item in `/tmp`
  or `/Applications` is deleted for good after a question, and the read-only host share's are
  neither trashed nor changed;
- copy and paste of text, and scaling for high-density displays.

# Limitations of EggOS 0.3.0

EggOS 0.3.0 is pre-alpha: the first release you can boot and type in, to try, not to use. This
page lists what it does not do yet, so nobody has to find out by trying. The plan for each is in
the [roadmap](roadmap.md); the reasons behind the current state are in the
[decisions](decisions.md) cited.

## Where it runs
- **QEMU only.** The images boot on QEMU's `virt` machines (aarch64 with a Cortex-A72 and GICv2,
  riscv64 through OpenSBI) and `q35` (x86_64), with 128 MiB of memory and 4 CPUs, as the run
  scripts start them; they need QEMU 8.2 or later (CI tests with Ubuntu 24.04's 8.2.2). No real
  hardware, no other hypervisor, no UEFI or bootable disk image yet.
- **No acceleration.** QEMU emulates the CPU (TCG) on every host, so the system runs slower than
  it will on hardware; boot to the prompt still takes about a quarter of a second (120 ms on
  aarch64, 161 ms on riscv64 and 261 ms on x86_64 from QEMU's start, on CI's machines on
  2026-10-29; the release notes give each image's own figure).
- **Devices:** the serial console, virtio-gpu (2D, polled), virtio keyboard, tablet and mouse,
  virtio-blk (polled, for the host folder), the real-time clock (x86_64 CMOS, AArch64 PL031,
  RISC-V Goldfish, D150), virtio-net (polled, the first adapter only, D157) and on test
  kernels virtio-rng. Nothing else is driven: no Wi-Fi, no sound, no USB (D110-D113, D130).
- **Network, parts 1 and 2** (D157, D158, [network.md](network.md)). IPv4 over one wired
  adapter with an address from DHCP, UDP, ping and TCP for programs, DNS through
  `/etc/resolv.conf`, `lo`; `ip`, `ping`, `nslookup`, `fetch` (downloads over `http://` and
  `https://`, TLS 1.3 with certificates checked against the roots you give in
  `/etc/ssl/roots.pem`: EggOS ships none, D159) and the MCP tools `net_status`, `net_ping`,
  `net_lookup` and `fetch`; TLS offers only X25519, has no resumption, client certificates or
  revocation checks; TCP has no window scaling (64 KiB windows), no selective
  acknowledgments, keep-alive or urgent data, drops segments that arrive out of order (they
  are sent again), and on a machine without an adapter its timers run only when a program
  makes a socket call. No downloads window in the GUI yet. No IPv6, no fragment reassembly, no static addresses or second adapter, no
  multicast, no reverse lookups, no netlink (iproute2 and `getifaddrs` fail; `ip` reads
  `/proc/net`). Settings has no Network pane and the bar no network indicator yet (designed in
  docs/ux/network-v0.3.md). The adapter is polled (50 wakeups a second when idle) until drivers
  run on interrupts. On riscv64, `--mcp --apps` with a disk and the network takes more than
  the machine's eight virtio slots: add `--no-network` there.

## What the system can do
- **Agents, under grants for the whole boot.** The built-in MCP server (D136, D137,
  docs/mcp.md) lets an MCP client read files in `/home`, `/media/host` and `/work` and list processes,
  windows and installed apps; with grants given when starting EggOS (`--mcp-allow
  view,write,run,install`) it also sees what app windows show (`view`), writes and trashes
  files below `/home/root`, runs programs, clicks and types in windows, and installs, opens and
  uninstalls apps. Grants hold for the whole machine and boot: no per-agent grants, no revoking
  them while EggOS runs; `run` is full control, the audit log included. Agents read toolkit and Wayland
  windows as element trees and act on their controls by id (D161), Windows programs too, with
  the text they draw (D162); macOS programs are pixels only (D143) until their layer describes
  its NSView tree, and a Windows list box's items and a check box's state are not in the tree
  yet.
  Commands run as root (file permissions are not enforced yet). Reached only through the run
  scripts' `--mcp` (no network MCP). The audit log keeps at least 3.5 MiB. The desktop shows
  an `Agent` chip while a client is connected and marks what it drives for a second (D156);
  the first boot does not ask "Where are you coming from?" yet.
- **One disk.** The root file system is the read-only initramfs inside the
  image: `/init`, `/bin` (the shell and the core utilities), `/etc/motd`, `/etc/hostname` and
  `/usr/share/eggos/about`, `/etc/hosts` and `/etc/resolv.conf` (the DHCP lease's DNS servers,
  D157). `/home`, `/Applications` and `/var` live on the disk image the run
  scripts attach, formatted with EggFS, and survive reboots (D131); `/tmp` is in memory, as `/home` is
  with `--no-disk` (together at most half of the memory), and `/proc` has `cmdline`, `meminfo`,
  `uptime`, `ps`, `version`, `sys/kernel`, `net/interfaces`, `net/route`, `net/arp`, `net/unix`,
  `driver/rtc`, `self` and a folder per process (D125, D137, D150, D157, D168). Changes reach the disk within 5 s, at `sync` and at power-off; closing QEMU's
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
  on your computer while EggOS runs are not seen. Only one folder can be attached. The x86_64
  and aarch64 release images carry the Windows and macOS layers, so Windows and macOS programs
  in it run and install there; the riscv64 image carries neither, as neither layer runs on
  riscv64 (D152).
- **A work disk is a copy, packed before the boot and unpacked after it** (D166). `cargo
  xtask run --work <folder>` (and `cargo xtask work pack`, for the run scripts' `--work
  <image>`) copies a folder into an EggFS image that EggOS mounts writable at `/work`; what
  EggOS writes there comes back only when QEMU has ended, into a new folder (`<folder>.out`
  or `--work-out`'s), never into the folder itself, and files removed in EggOS are simply
  absent from it. Changes made on your computer while EggOS runs are not seen. The folder's
  top-level `.git` and `target` are left out, and so are symbolic links (EggFS has none yet),
  with a warning for each; names must be valid Unicode and at most 255 bytes. The run scripts
  cannot pack or unpack a folder themselves: that needs `cargo xtask` in an EggOS checkout.
  Only one work disk can be attached. The image is sized once, when it is packed (the folder
  plus a thirty-second plus 1 GiB): a build that writes more fills it (`No space left on
  device`). Unpacking reads the whole image's metadata and checks it first, so a damaged image
  gives nothing back. Agents read `/work` without a grant, as `/media/host`; the write tools
  still reach only `/home/root`.
- **The clock is UTC, with no time zones** (D150). The kernel reads the real-time clock once
  at boot (QEMU's follows the host's clock) and keeps time from it; the network time is
  set by hand (`date --sync`, MCP `clock_sync`; D159), not at boot or by Settings, so a clock
  that drifts stays off until one of those, `date -s`, Settings or `clock_set` sets it, which
  also sets the real-time clock. Local time is UTC everywhere: the desktop's clock,
  `date`, file times and the Windows and macOS layers' local-time calls; there is no time zone
  setting or database. Only a privileged process (root) may set the clock. An absolute sleep or
  wait on `CLOCK_REALTIME` ends at the time the clock pointed to when it started, even if the
  clock is set meanwhile (Linux wakes it). No `/dev/rtc0` (`hwclock` reads
  `/proc/driver/rtc`), no alarms or wake-ups (`CLOCK_REALTIME_ALARM`), no UEFI or ACPI time
  services. Without a real-time clock the wall clock starts at 1970 at boot and the desktop's
  clock shows the uptime until the clock is set (D73). `/proc/driver/rtc` reads the hardware
  clock at most once a second and may be up to a second behind it in between.
- **Memory has firm limits, and no swap** (D155). The kernel keeps a reserve for itself, a
  thirty-second of memory (4 MiB at the run scripts' 128 MiB, at least 2 and at most 16 MiB),
  that programs never get: `free` shows it on its `Reserve:` line and `/proc/meminfo` as
  `KernelReserve`. A program that touches more memory than is left is ended with `SIGKILL`
  (whichever program faulted, not the largest, as Linux's out-of-memory killer would choose);
  a memory file, a window buffer, a pipe, a file in `/tmp`, a new thread or a socket's queue
  that would take the reserve fails with `ENOMEM` (`ENOSPC` for writes to memory files and
  `/tmp`). Window buffers hold at most 32 MiB per program and a quarter of memory for all
  programs; local socket queues at most an eighth of memory together (a full queue makes its
  writer wait for the reader; an empty one that cannot grow is `ENOMEM`). Disks are read
  defensively: a damaged EggFS disk or a FAT volume whose cluster chain loops gives I/O errors
  instead of hanging or crashing EggOS.
- **Linux programs: static ones, and dynamic ones with their own libraries.** The kernel runs
  Linux programs through its own Linux system-call layer, from the host folder (`--apps`),
  installed as apps (`app install`), or built into an image by `cargo xtask image --add`
  ([Linux programs](linux-programs.md), D117, D130, D134). Dynamically linked programs run
  with glibc's or musl's loader (D167), but EggOS ships no C library: the loader and libraries
  go into the image with the program; no vDSO (`AT_SYSINFO_EHDR`), and `execve` still reads a
  program and its loader whole into memory and copies their segments at once (pages mapped
  from files are read only when first touched). `/proc/self` and `/proc/<pid>` hold `cmdline`,
  `exe`, `fd`, `maps`, `stat` and `status` only (no `task`, `environ`, `cwd`, `mountinfo` or
  `/proc/thread-self`; `stat`'s times and fault counts are 0, `status` has no signal or memory
  lines but `VmSize`, links in `fd` to pipes and sockets cannot be opened, and every file is
  root's), and there is no `/sys` (D168); `/dev` holds the memory devices, `tty`, `console`, the
  standard streams' links, `window` and `agent`, but no `/dev/shm`, `/dev/pts` or block devices;
  the only symbolic links are those in `/proc` and `/dev` (D168); `mmap` of files is private, or shared and read-only
  (a shared mapping does not see later writes to the file, and a writable one is `ENODEV`,
  D167), and a file page is read under the process's memory lock, so its other threads wait
  for a disk read; no copy-on-write `fork`
  (memory is copied at once, D104). Pipes work (D125), but `poll` on many descriptors wakes
  with every pipe, socket and console change. Local sockets work (D138): `AF_UNIX` stream
  sockets only (no datagrams or sequenced packets); `AF_INET` datagram sockets work for UDP
  and ping (D157), stream sockets for TCP (D158); socket files only in the
  memory file systems (`/tmp`, `/run`, `/var` and `/home` without a disk; `bind` on the disk
  is refused), descriptors passed with `SCM_RIGHTS` but never a socket that carries sockets
  itself, and at most 4096 descriptors in flight in all; no `epoll`, `eventfd`, `timerfd` or
  `signalfd` yet, which most graphical Linux toolkits need. Shared memory works through
  `memfd_create` (D139): `MAP_SHARED`, or `MAP_PRIVATE` read-only, of a memfd; no
  writable `MAP_SHARED` of other files (`ENODEV`), no `shm_open` or `/dev/shm`, no writable private mapping of a memfd, no
  executable or `PROT_NONE` mapping of one (`EPERM`), no `F_SEAL_WRITE` or
  `F_SEAL_FUTURE_WRITE`, no `SIGBUS` past the end (those pages read as zeros), `fallocate`
  with mode 0 only, a write, `fallocate` or mapping at a gigabyte-scale offset fails
  (`ENOSPC`, `ENOMEM`), and a process that maps one cannot `fork` (`posix_spawn` and `vfork`
  work).
- **Linux GUI programs: static Wayland clients only.** `eggway` (D140) shows a static program's
  `xdg_toplevel`s as desktop windows; GTK, Qt, SDL and browsers need their libraries, `epoll`,
  `eventfd` and `timerfd` and protocols eggway does not serve yet ([Linux
  programs](linux-programs.md) lists them). It serves one seat and one output (whose mode is the
  work area's size, at scale 1), shows surfaces opaque and ignores buffer scale and transform,
  window geometry and the window menu request; popups close at once, and there is no clipboard,
  subsurfaces, touch or cursor surfaces; at most 16 clients, 16 surfaces and 8 pools each, pools
  of 32 MiB at most and 48 MiB for all clients, and the terminal waits while a Wayland program
  opened from it runs.
- **Windows: console programs and simple GUI programs on x86_64 and aarch64 only.** Windows
  programs that use the kernel32, ntdll, shell32, user32 and gdi32 functions the Windows layer
  provides run natively, x64 programs on x86_64 and ARM64 programs on aarch64, from the host
  folder or installed as apps; the layer is in the base of those images until the component
  manager makes it optional (D152) ([Windows programs](windows-programs.md), D118, D121, D124, D129):
  threads, waits, locks, directory listing and UTF-16 strings work, GUI programs open
  windows, paint them with GDI, get the mouse and keyboard and show message boxes, and C
  programs built by MSVC on its C runtime (`/MD`) run (D146). Programs on a C++ runtime,
  MinGW's or Rust's std, `scanf`, locales, the math library, other processes, other DLLs, structured exception handling,
  controls beyond buttons, edits, labels and lists (D145), dialogs from resources, fonts other
  than Egg Mono and text beyond ASCII, x64 programs on Arm (and ARM64 programs on x86_64) and
  riscv64 do not run yet; names are case-sensitive and local time is UTC.
- **Android: static console programs only, on aarch64 and x86_64.** Programs built with the
  Android NDK and linked statically against bionic run natively through the Linux layer
  ([Android programs](android-programs.md), D119). No apps (`.apk`), no dynamic linking, no
  system properties (all read as unset), no `logd` or binder, no riscv64.
- **macOS: console programs and programmatic AppKit apps, on aarch64 and x86_64 only.**
  Mach-O console programs (thin or fat, built for macOS 13 or later) that link libSystem,
  Objective-C programs that use the core of Foundation, and Objective-C GUI apps built in code
  (no nib or storyboard) on AppKit's core run natively, arm64 programs on aarch64 and x86_64
  programs on x86_64, from the host folder or installed as apps; the layer is in the base of
  those images until the component manager makes it optional (D152)
  ([macOS programs](macos-programs.md), D120, D123, D142, D147). AppKit
  covers windows, views drawn with `drawRect:` (rects, lines and ovals 1 px wide, one font, no
  alpha), push buttons, text fields, labels, alerts and the main menu in the macOS profile's
  top bar (at most 6 menus in 248 bytes of labels; `wm menu` and the MCP tools list and choose
  its items, except while the app shows an alert); scroll views, tables, images, timers, panels, Auto Layout, Swift, Objective-C blocks,
  `__weak` and exceptions, most of Foundation, signal handlers, other frameworks and riscv64
  do not run yet.
- **Seeing and operating windows from the command line, partly** (D141). `wm capture` reads any
  app window's content as a PNG and `wm click`, `drag`, `type`, `press` and `scroll` operate it, but
  the terminal's window can't be captured, takes no pointer input or sideways scrolling, and
  non-ASCII typing is not in yet; clicks, drags and scrolling aimed at a window with a dialog
  open are refused (keys go to the dialog: D162); Windows programs get no `WM_LBUTTONDBLCLK`
  (a double-click is two `WM_LBUTTONDOWN`s), and no mouse sends a sideways wheel yet (only
  `wm scroll` and agents do); a minimised Wayland app does not repaint after input (eggway
  sends no frame callbacks to minimised surfaces) until it is restored; every process is root
  until users come (M10), so any program can capture and send input; agents get it only
  through the MCP server's grants (`view` to see, `run` to operate, D143). Input to a window
  whose program stopped reading its events is retried for 3 s, then fails.
- **iOS apps are not in yet** (D17).
- **No components yet.** The component manager (D18) comes later (M14): for v0.3.0 the
  desktop and the Windows and macOS layers are in the base and cannot be left out (D109,
  D152). The built-in MCP server is in (D136, docs/mcp.md), but not every capability has its
  three front-ends yet (D22): Settings and the device list have no MCP tools, and Files'
  copy, move and new folder have none either (an agent with `run` uses `cp`, `mv` and
  `mkdir`). Window focus, move, size, minimise, maximise, restore and close have theirs
  (D160).
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
- **The command model, slices 1 to 3 (D160, D163, D164, docs/ux/command-model.md).** Every command
  is in the registry, but `eggsh`'s built-ins are only named there (the shell does not link it).
  `--json` exists where a command's output is records or an object (`ls`, `ps`, `wm list`, `wm
  show`, `app list`, `agentlog`, `help`, `wc`, `free`, `uptime`, `uname`, `settings list` and
  `get`, `ip -j`); `grep`, `head`, `date`, `ping`, `fetch` and the other text or content
  commands have none, so their errors are coded only in the text's wording. `date`, `hwclock`,
  `ip`, `nslookup`, `fetch`, `trash`, `agentlog` and `wm`'s verbs but `list` keep their own
  option parsers (D165); `app install|open|uninstall` write text errors only. Examples exist
  for the commands a fake can show, `date` and `wm list` included (not for the network,
  `hwclock` or apps). Pages
  (`--limit`, `--after`) exist for `ls`, `ps`, `wm list` and `app list` and their tools, not
  for `agentlog`, `settings list`, `help` or `trash -l`; `--dry-run` for `trash`, `app
  uninstall` and `settings set` (and MCP `tee`), not for `fetch`, `date -s`, `app install`,
  `cp`, `mv` or `rm`, whose previews would not be exact. Names inside text error lines are
  written as they are, control characters included.
  An MCP tool whose command fails (rather than being refused) still returns that command's
  text error without a code: the twins run in text mode.
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
- removing or installing the Windows or macOS layer on its own (the component manager, D18,
  D152), and `app install --force`: `app install` refuses a program whose layer is not in the
  image, which on the release images happens only on riscv64 (D134, docs/apps.md);
- `.app` bundles that hold symbolic links (a framework's `Versions/Current`, for example):
  `app install` refuses them, as EggFS stores no symbolic links (D131, D134); the installer's
  chooser lists at most four programs and leaves out names longer than 64 bytes (`app install
  <path>` installs any);
- archives other than `.zip` (`.dmg`, `.pkg`, `.tar`, `.7z`, `.msi`), ZIP64 (archives or
  entries past 4 GiB or 65 535 entries), encrypted entries, methods other than stored and
  deflate, symbolic links in a `.zip`, and names longer than 255 bytes (D148); a `.zip` whose
  files would take more than 4 GiB once extracted, or holding a file named `app.manifest`,
  `app.manifest.tmp` or `app.icon` at its top, is refused (as is a program of one of those
  names); the window's chooser for a `.zip` with several programs lists four (`app install
  --entry <path>` installs any) and shows a path longer than a button's 32 bytes as `...` and
  its end; any file starting with a zip's signature (a `.docx`, `.jar` or `.apk`) is offered
  `Install...` in Files and refused as holding no program; every entry of a `.zip` is
  extracted in one step of the installer, so `Cancel` waits for the entry being extracted;
- app icons (D148): binary property lists, asset catalogues (`Assets.car`), JPEG 2000 and
  `it32` entries of an `.icns`, interlaced PNGs, 16-bit and compressed bitmap entries of a
  Windows icon, and images larger than 1024 pixels (256 for a bitmap entry) are not read (the
  next image is taken, else the app wears the letter tile); icons are made at 48 and 32 pixels
  only (2x doubles them); the title bar, Files and the installer's dialogs still show the
  generic tile; a native EggOS app cannot name its own icon in its manifest yet beyond `icon=`
  pointing at an `app.icon` it ships; `app icon` (writing an app's icon as a PNG) and the MCP
  tool `app_icon` from `docs/ux/app-icons-v0.3.md` section 7 are not built yet;
- closing the terminal (its close control is disabled) and a second terminal: launching
  Terminal focuses the one console terminal, and installed console apps are typed into it, so
  they reach whatever reads the console at that moment (docs/apps.md);
- Settings, the installer's window (`app --window`, D134) and Files (`/bin/files`, D135),
  all served by the same program, are the programs with a window in the image (the window tests' `hello-window`
  and `win-paint.exe` are added only to their own images); Settings' Mouse and Components panes,
  key repeat and 2x come later; Windows GUI programs (D129), Linux Wayland apps (D140) and
  macOS AppKit apps (D147) open windows;
- 3D graphics;
- a terminal on a display smaller than 658x530 (the macOS profile's top bar and dock take 100
  rows): the window does not fit every profile's work area, and the display then shows no
  terminal (the serial console still works);
- Files (D135, docs/ux/desktop-v0.3-gui.md section 8.1) selects one item at a time (no
  multiple selection, dragging or drop), has one list view (no icons, thumbnails or columns
  to sort by), hides names starting with `.`, shows at most 512 entries (32 KiB of names) of
  a folder, and shows no free space in its status bar (no `statfs` yet); its clipboard holds
  one item inside the Files process, shared with no other program and lost when Files
  closes; a text file opens in the Text Editor (D149), or with `cat` in the Terminal window
  in an image without it; each click on the desktop's `Host share` opens another Files window; folders nest at
  most 32 levels deep in a copy, a move across file systems or a delete; agents read, write
  and trash files through the MCP tools `ls`, `cat`, `tee` and `trash` (docs/mcp.md), and copy,
  move or make folders only through `run`;
- the Trash holds only what is on the home folder's file system (`/home`): an item in `/tmp`
  or `/Applications` is deleted for good after a question, and the read-only host share's are
  neither trashed nor changed;
- the Text Editor (D149) edits UTF-8 text of up to 4 MiB in Egg Mono, so a character beyond
  ASCII and the font's box and arrow glyphs shows as `?`; it has no word wrap, no replace, no
  go-to-line, no syntax colouring and no file picker (Open and Save As take a typed path); its
  clipboard is its own, shared with no other program; a file that is not text is refused, not
  shown read-only; opening a file twice gives two windows; with no desktop `edit` cannot open
  it and there is no terminal editor; a file with mixed line endings is saved all CRLF once it
  holds one `\r\n`; saving replaces a symbolic link by a regular file and breaks hard links;
  a save cut short (power lost, the editor killed) leaves its hidden `.<name>.saving` beside the
  file, which later saves leave alone (they write `.<name>.<n>.saving` instead);
  Alt and Option with a letter type nothing (the keymap has no Option or AltGr layer yet);
- copy and paste of text, and scaling for high-density displays.

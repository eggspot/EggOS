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
  and on test kernels virtio-rng. Nothing else is driven: no disk, no network, no sound, no USB,
  no real-time clock (D110-D113).

## What the system can do
- **No disk and no network.** The only file system is the read-only initramfs inside the
  image: `/init`, `/bin` (the shell and the core utilities), `/etc/motd`, `/etc/hostname` and
  `/usr/share/eggos/about`.
  Nothing can be written, and nothing you do survives a reboot (D102).
- **The clock starts at the epoch.** Without an RTC driver the wall clock reads 1970 and the
  desktop's clock shows the uptime (D73, D115).
- **Linux programs: static ones only.** The kernel runs static Linux programs through its own
  Linux system-call layer, but the image ships only our own programs and there is no way to
  add others to a running system yet. No dynamic linking, no `/proc`, `/dev` or `/tmp`, no
  pipes, no copy-on-write `fork` (memory is copied at once, D104).
- **Foreign apps are not in yet.** Windows, macOS, Android and iOS apps (D17) and their
  console programs do not run in this release.
- **No MCP server, no components.** The built-in MCP server (D21), the CLI and MCP front-ends
  of every capability (D22) and the component manager (D18) come later.
- **One user, root.** Processes start as root; there are no logins or permissions to manage
  (D75).

## The shell and the core utilities
- `eggsh` runs commands with arguments, quoting, variables, `cd`, `exit`, `.` and scripts, but
  refuses pipes, redirections, `&&`, `||`, background jobs, subshells and command substitution,
  and has no wildcards, `if`, `while`, `for`, `case`, functions, line editing or history
  ([shell](shell.md)).
- The core utilities are `cat`, `echo`, `false`, `halt`, `ls`, `poweroff`, `reboot`, `sleep`,
  `true` and `uname` ([core utilities](coreutils.md)).
- The keyboard layout is US only (D113).

## The desktop
The desktop is minimal (D115): wallpaper, a top bar with the uptime and a power menu, and the
shell in an 80x24 terminal window you can focus, drag by its title bar and minimise to a chip
in the bar. Not yet:
- a boot screen with the logo and progress (the plain wallpaper shows until init starts);
- a pointer drawn on screen (the host's cursor is the only one in the QEMU window);
- maximising the window (double-click on the title bar) and closing it (the close control is
  disabled), hover highlights on the bar and the controls;
- dragging the window partly off screen;
- more windows or apps, a compositor, 3D graphics;
- a terminal on a display smaller than 658x458: the window does not fit, and the display then
  shows no terminal (the serial console still works);
- copy and paste, and scaling for high-density displays.

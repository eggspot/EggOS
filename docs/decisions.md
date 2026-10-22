# Architecture decisions

Numbered, append-only. A later decision may supersede an earlier one; say so in both entries.
Status is **Decided** (owner) or **Provisional, owner to confirm**.

## D1 Kernel: our own, in Rust, Linux-compatible — Decided
We write our own kernel in Rust; no Linux kernel C code. "Linux-compatible" means the kernel
implements the Linux interfaces (system calls, ABI, `/proc`/`/sys` conventions where needed) so
unmodified Linux software runs on it.

## D2 Compatibility layers: Linux userland first — Provisional; iOS added and native execution required by D17; order superseded by D19
Windows, macOS and Android apps run through compatibility layers we write, with the same features
on each. The owner has not chosen which comes first. Provisional order:
1. **Linux userland** — the kernel's native ABI (D1), so it is not a separate layer so much as the
   foundation every other layer needs (process model, files, signals, mmap, threads).
2. **Android** — Android apps run on a Linux ABI plus Binder, ashmem and the ART runtime, so it
   reuses almost all of step 1.
3. **Windows** (PE loader, NT syscalls/Win32 on top of our kernel) and **macOS** (Mach-O loader,
   XNU/Mach traps, frameworks), order between them to be decided once the Linux layer is solid.
Reasoning: nothing else can run before the Linux ABI exists, and each step reuses the previous one.

## D3 AI-native interface — Decided; shape decided by D21 (MCP) and D22
The OS exposes its capabilities (files, processes, settings, devices, UI) to AI agents as a
first-class, permissioned interface, not as screen scraping. Design is a later roadmap milestone.

## D4 Hardware scope: everything, behind a HAL and one driver model — Decided; device classes spelled out by D23
Every CPU architecture, GPU, device type and driver, all written by us. Therefore:
- the kernel core is architecture-independent; architecture code lives only in the HAL, one
  module per architecture (`hal/src/arch/<arch>/`);
- drivers sit behind one common driver model (`drivers/`), reaching hardware only through
  register/bus access traits, so the same driver serves every architecture;
- every supported architecture boots in QEMU in automated tests.

## D5 License: MIT, clean-room — Decided, permanent
All code is MIT, and the license will never change (owner, 2026-10-22): no relicensing, no dual
licensing, no extra terms. Never copy or translate GPL/LGPL/copyleft code (including Linux, GPL-2.0);
implement from public specifications, datasheets, man pages and observed behaviour.

## D6 Repository layout: a Cargo workspace of four crates — Decided by the build loop (engineering)
- `kernel/` (`eggos-kernel`): architecture-independent core (library, host-testable) plus the
  kernel binary (a stub on the host so `cargo build` works on every development machine).
- `hal/` (`eggos-hal`): the `Platform` trait the core uses, and one module per architecture.
- `drivers/` (`eggos-drivers`): device drivers, generic over register-access traits.
- `xtask/`: developer commands in Rust (`cargo xtask ...`), so macOS, Windows, Linux and CI run
  the same steps without shell scripts.
The core is generic over `P: Platform` (zero-sized types, associated functions), so the
abstraction costs nothing at run time: every call is monomorphised and inlined.

## D7 Toolchain: pinned stable Rust, no nightly — Decided by the build loop (engineering)
`rust-toolchain.toml` pins an exact stable release so every host and CI use the same compiler
and lints. Bare-metal targets are official tier-2 targets with a prebuilt `core`, so no nightly
and no `build-std` are needed yet. Revisit only if the kernel genuinely needs a nightly feature.

## D8 CI: one cheap Linux job, plus a macOS/Windows host check on PRs — Decided by the build loop (engineering, per billing rules); updated by the setup review (2026-10-22)
One `ubuntu-24.04` job (`check`) runs fmt → clippy → host tests → QEMU boot tests on every
architecture, cheapest first, with `timeout-minutes`, PR-only cancellation of superseded runs,
`fetch-depth: 1` and a `target/` cache that is restored on every run but saved only from `main`.
`check` is a required status check on `main`, so the workflow always runs: a first step detects
docs-only PRs and skips the rest, reporting success in seconds (a `paths-ignore` filter would
leave such PRs pending forever). On PRs with code changes, a `hosts` job (macOS and Windows,
after `check` passes) runs clippy, host tests and the kernel builds, because those are the
development hosts. The repo is public, so standard runners are not billed today. If it ever
becomes private, that job is the first thing to reconsider (10x and 2x rates).

## D9 Bring-up order: aarch64 and riscv64 first, x86_64 next — Decided by the build loop (engineering); x86_64 boot protocol superseded by D15
QEMU's `virt` machines for aarch64 and riscv64 load an ELF kernel directly and start it in the
kernel's privilege level (EL1 / S-mode under OpenSBI) with a UART at a fixed address, so the
first boot needs only a few lines of entry assembly. Together they cover phones, tablets,
Apple-silicon-class laptops, Arm servers and the growing RISC-V embedded/server space.
x86_64 (most desktops, laptops and servers) comes right after: QEMU's `-kernel` enters x86
kernels through Multiboot in 32-bit protected mode, so it needs a 32→64-bit trampoline with
early page tables — a self-contained next step through the same HAL interface.

## D10 AArch64 kernel target is `aarch64-unknown-none-softfloat` — Decided by the build loop (engineering)
The kernel does not use FP/SIMD registers, so traps and context switches never save them for
kernel code (user FP state is saved lazily, later), and early boot works before FP is enabled.
The target also implies `strict-align`, which is required while the MMU is off (all memory is
Device memory). Hot paths that benefit from SIMD (e.g. memcpy, checksums, crypto) will opt in
explicitly in a dedicated, benchmarked module.

## D11 Boot tests: console verdict marker plus real power-off — Decided by the build loop (engineering); a fatal-exception boot added by D25
The kernel prints `EGGOS-SELFTEST: PASS` (or `FAIL`, also on panic) and, when built with the
`qemu-test` feature, powers off through each architecture's standard firmware interface (PSCI
`SYSTEM_OFF` on Arm, SBI System Reset on RISC-V) rather than a QEMU-only debug device. The
runner (`cargo xtask qemu`) passes only on a clean QEMU exit within 30 s **and** the pass marker
on its own line **and** no fail marker. This keeps one verdict mechanism for every architecture
and exercises the same power-off path real hardware will use. Without `qemu-test` the kernel
stays up (halts) after boot.

## D12 Board addresses hard-coded until device-tree discovery — Provisional (engineering, until M5)
The early consoles use QEMU `virt`'s UART addresses (PL011 at `0x0900_0000`, NS16550 at
`0x1000_0000`). The driver model milestone (M5) replaces them with discovery from the device
tree the firmware passes (`a1` on RISC-V, `x0` with a Linux-style boot on Arm) and ACPI on x86.

## D13 Performance target: the fastest OS — Decided
EggOS must beat macOS, Windows and Linux on boot time, latency, throughput and memory footprint on
the same hardware, and run foreign apps at least as fast as their native OS. Every hot path gets a
benchmark when it is written, PRs touching one report before/after numbers, and a performance
regression blocks merge unless the owner explicitly accepts it. Release builds (LTO, one codegen
unit) are what we measure and ship. See CLAUDE.md, "The target: the fastest OS".

## D14 Everything written by us, in Rust — Decided
All of EggOS (kernel, drivers, compatibility layers, userland, libraries, tools, build automation)
is Rust we write ourselves. Assembly only where the CPU requires it, inside the HAL's architecture
modules. No third-party crates, ever, with no exceptions (owner, 2026-10-22): whatever a crate
would provide, we write ourselves. Machine-provided firmware (UEFI, OpenSBI, QEMU loaders) is outside EggOS.

## D15 x86_64 boots through PVH and powers off through ACPI S5 — Decided by engineering (owner approved the port); supersedes D9's Multiboot plan
QEMU's `-kernel` does not load ELF64 images through Multiboot, but it does through PVH (the
boot ABI of Xen's public `start_info.h`, also used by Linux and Firecracker): an ELF note
(`XEN_ELFNOTE_PHYS32_ENTRY`) names a 32-bit entry point, entered in protected mode with paging
off and `ebx` pointing at `hvm_start_info`. The x86_64 HAL entry code identity-maps the first
4 GiB (2 MiB pages: RAM, ACPI tables and MMIO below 4 GiB) and switches to long mode. The kernel
is linked static, non-PIE at 1 MiB, because that 32-bit code uses absolute addresses. UEFI
boot for real hardware is a later decision.
Power-off (D11) uses ACPI sleep state S5, the standard firmware interface on PCs, not QEMU's
`isa-debug-exit`: RSDP from `hvm_start_info.rsdp_paddr`, or the spec's BIOS-area search when
that is 0 (as QEMU 10 passes it) or invalid →
RSDT/XSDT → FADT (PM1 control ports, ACPI-enable handshake) → `\_S5` package in the DSDT.
`\_S5` is read as the static `Name`/`Package` firmware defines it, without an AML interpreter;
the interpreter comes with ACPI device discovery in M5. The ACPI table code (`hal::acpi`) is
architecture-independent and unit-tested on the host, since Arm servers use ACPI too.

## D16 GUI design is done by Claude Fable — Decided
Every user-visible design (desktop shell, window chrome, system apps, settings, installer, icons,
design system, mockups, layouts, motion) comes from Claude Fable: Claude sessions delegate the
design to an agent on the `fable` model (currently `claude-fable-5-1`; the alias tracks newer
releases), record it in `docs/ux/`, and then implement it in Rust. Non-visual work does not need
Fable. Implementation follows the design; a change that departs from it updates the design first.

## D17 Foreign apps run natively at 100% of native speed or better, never emulated — Decided; extends D2 with iOS
Windows, macOS, Android and iOS apps run from their own unmodified files. No virtual machine, no
emulator, no simulator, no interpreter:
- **Loaders** for each format (PE/COFF, Mach-O including universal binaries, APK/DEX, iOS `.app`)
  map the app's machine code into a normal EggOS process, where it runs directly on the CPU.
- **System calls** (NT, XNU/Mach traps, Linux/Bionic for Android) are implemented by our kernel as
  additional native syscall personalities, not trapped and forwarded to another OS.
- **System libraries** (Win32/NT user-mode DLLs, Darwin frameworks such as Foundation, AppKit and
  UIKit, the Android framework and ART) are reimplemented by us in Rust on top of EggOS, with the
  same exported symbols and behaviour, clean-room (D5): we never ship Microsoft's or Apple's
  binaries and never copy Wine (LGPL), Darling (GPL) or similar projects.
- **Graphics and media APIs** (DirectX, Metal, OpenGL ES, Vulkan, Core Audio, WASAPI, ...) are
  implemented straight on our drivers, without an intermediate API hop.
Target: each app runs at 100% of its speed on its own OS or better, on the same hardware. Reaching
"better" comes from the rest of EggOS (faster syscalls, scheduler, I/O, memory management, no
background services the user did not install). Every compatibility-layer hot path gets a benchmark
that compares against the native OS, and a regression blocks merge like any other performance
regression (CLAUDE.md, Priorities).
Different CPU architecture: an app built for another architecture than the machine (an x86_64
Windows app on an Arm laptop, an arm64 iOS app on an x86_64 PC) cannot run directly. Its code is
translated once, ahead of time, into native code and cached on disk (the approach of Apple's
Rosetta 2), never interpreted. That is the fastest known method, but translation has a cost, so
those cases are measured and reported separately; on matching hardware (arm64 apps on Arm,
x86_64 apps on x86_64) there is no translation at all.
Our code: settled. Everything is our own clean-room implementation of public interfaces (D5); no
Apple, Microsoft or Google code or binaries are copied or shipped.
Where the apps come from — Decided (owner): no App Store access. macOS apps come from the
vendor's own download when the vendor offers one (`.dmg` disk images, `.pkg` installers, `.zip`
archives), so EggOS must open and install those formats itself; iOS apps likewise only when they
are distributed outside the App Store. App Store apps (FairPlay-encrypted, licensed for Apple
devices only) are out of scope.

## D18 Modular install: only the base is installed, every feature is the user's choice — Decided
EggOS installs a small base by default: the kernel, drivers for the hardware present, the base
userland, the built-in MCP server (D21, so any installed agent can use the OS) and the
installer/component manager. Everything else is an optional component the user picks at setup or later and can remove
cleanly: each compatibility layer, the desktop and its apps, AI services and agents, server roles,
developer tools, and so on. The editions (personal, professional, enterprise, server) become preset
selections of components plus policy, not separate builds.
Rules for every component:
- it declares its dependencies on other components, and nothing outside it assumes it is installed;
- when it is not installed it costs nothing: no disk, no memory, no running service, no boot time;
- kernel parts that belong to one component (e.g. the NT syscall personality, Android Binder) stay
  out of the running kernel unless that component is installed; the mechanism (loadable kernel
  modules or per-component kernel builds) is a later decision, made with benchmarks;
- removing it leaves no files, services or settings behind.

## D19 Compatibility-layer order: Linux, Windows, Android, macOS, iOS — Decided by research (owner can overrule); supersedes D2's order
Order: (1) Linux userland, the kernel's native ABI and the base of every layer; (2) Windows;
(3) Android; (4) macOS; (5) iOS. Windows comes before macOS because it is where most switchers
are (StatCounter, June 2026: Windows ~57% of desktop use, macOS ~16%, Linux ~4%) and nearly all
games (Steam survey, July 2026: Windows 93.7%); its apps are mostly direct downloads, so the
no-store rule (D17) barely shrinks the catalogue; the same apps cover much of a macOS
switcher's daily use; and its ABI is stable and publicly documented. Android follows: it reuses
almost all of the Linux ABI, but its value is mainly on phones and tablets (hardware bring-up
M13), Microsoft dropped Android apps on Windows (WSA) in 2025, and many apps need Google Play
Services, which we cannot ship. macOS then iOS, so iOS reuses the Mach-O loader, dyld, the
Objective-C/Swift runtimes and the Darwin frameworks; iOS is last because most of its apps are
App Store-only and therefore out of reach (D17). Each layer stays an optional component (D18).

## D20 Releases: every finished milestone ships, without asking — Decided (owner)
Semantic versioning, tags `vMAJOR.MINOR.PATCH`. Until 1.0, each finished roadmap milestone bumps
MINOR and fixes in between bump PATCH; `1.0.0` is the first release someone can install and use as
their everyday OS (decided by research: the usual meaning of 1.0, and it keeps 0.x honest about
maturity). Claude cuts the release as soon as a milestone is merged and `main` is green: version
bump PR, full `cargo xtask ci` on the merged commit (locally, so no extra CI minutes), kernel images
for every architecture that boots, tag, and a GitHub Release with notes and the images attached
(procedure: CLAUDE.md, "Releases"). Only new accounts, signing keys, credentials or money wait for
the owner.

## D21 AI-native through a built-in MCP server; every installed app is AI-usable — Decided (owner)
The AI interface of D3 is the Model Context Protocol (MCP), the open standard AI agents already use
to call tools. The EggOS MCP server is part of the base (D18) and implemented by us in Rust from
the public specification, with no SDK crate (D14). Local transport first (stdio and a local socket);
network access is an opt-in component.
- **The OS:** every system capability (D22) is an MCP tool or resource.
- **Native apps:** an app's manifest declares its actions (name, typed parameters, description,
  whether it is destructive). On install the component manager publishes them as MCP tools and as
  CLI commands, and removes them on uninstall. An app with no declared actions still gets the
  generic tools below.
- **Foreign apps:** we implement their UI and system libraries (D17), so we can expose their
  windows and controls (Win32/WinUI, AppKit, UIKit, Android views) as structured data with actions
  (read, click, type, select), not screenshots. Their own automation interfaces are bridged too:
  Windows COM and UI Automation, macOS AppleScript and App Intents, Android intents, and iOS App
  Intents and URL schemes.
- **Security:** a capability-based permission model (correctness and security is priority 2). Each
  agent gets only what the user grants, per capability and revocable. The default for an unknown
  agent is deny. Destructive actions ask the user. Text read from apps, files and the web is data,
  never instructions to the OS. Every agent action goes into an audit log the user can browse.

## D22 One capability, three front-ends: GUI, CLI and MCP at 100% parity — Decided (owner)
Each OS feature is implemented once, as a typed capability in a system service (operations, typed
inputs and outputs, errors, permissions). The GUI, the CLI command and the MCP tool are front-ends
over that same capability. Where possible, the CLI command and the MCP tool are generated from the
capability's definition, so they cannot drift apart. A feature is done only when all three exist
and are tested. A test walks the capability registry and fails if any capability lacks a front-end.
The CLI is scriptable (stable output with a machine-readable mode) and familiar to switchers: the
UX track maps common macOS, Windows and Linux commands.

## D23 Every device class, one OS and one adaptive shell — Decided (owner); extends D4
The device classes are desktop, laptop, phone, tablet, TV, wearable, car, server and embedded/IoT,
and new classes are added as they appear. They share one kernel, one driver model, one capability
layer and one app model. The shell adapts to screen size, distance (a TV is used from across the
room) and input (keyboard and mouse, touch, TV remote/D-pad, voice, game controller). Server and
embedded builds can run headless, using only the CLI and MCP. Each class's design comes from Claude
Fable (D16). Form factor is a runtime property; code never assumes one.

## D24 Lightweight: size, memory and boot-time budgets that only shrink — Decided (owner)
EggOS must be the lightest OS to install and run as well as the fastest (D13). Every release
measures and publishes:
- the base install size;
- idle memory after boot;
- boot time to a usable CLI, and to a usable GUI when installed;
- the per-component cost of each optional component (D18).

The numbers may only go down. Any growth needs a stated reason in the PR and is handled like a
performance regression. Concrete budgets are set once the base first installs (M10), from
measurements against minimal Linux, Windows and macOS installs on the same hardware.

## D25 CPU exceptions: the HAL decodes, the kernel decides — Decided by the build loop (engineering)
Each architecture module installs its exception vectors in the entry code, before any kernel
code runs (`VBAR_EL1` on AArch64, `stvec` in direct mode on RISC-V, a 32-gate IDT on x86_64).
Every entry saves all general-purpose registers on the current stack and decodes the exception
into one architecture-neutral `eggos_hal::Exception` (a coarse `ExceptionKind` for policy, the
architecture's own name, raw cause value, pc and fault address). The HAL hands it to the kernel
binary's `kernel_exception`, the same way the entry code calls `kernel_main`, and resumes the
interrupted code if that returns (after the instruction, for a breakpoint). So the policy
(today: breakpoints resume, everything else is fatal and printed with the fail marker) lives in
the architecture-independent core, and later page-fault handling, signals and debuggers plug in
there without touching the HAL.
- FP/SIMD registers are not saved: the AArch64 and x86_64 kernel targets are soft-float, and the
  RISC-V boot code turns the FPU off (`sstatus.FS` = Off), so a stray FP instruction in kernel
  code traps instead of silently corrupting state. User FP state is saved lazily later (M4).
- x86_64 has no IST stacks yet, so a double fault from a stack overflow resets the machine
  (which the QEMU runner detects); IST stacks for #DF/NMI come with the kernel page tables (M2).
- Testing: a boot self-test takes a breakpoint and checks that it reached the kernel and came
  back with state intact; a second QEMU boot per architecture (kernel feature
  `qemu-test-fatal-exception`) executes an illegal instruction after the self-tests and the
  runner requires the exception report followed by the fail marker and a clean power-off.
- Not benchmarked yet: exception entry becomes a hot path with syscalls and page faults (M3, M6);
  its benchmark comes with the benchmark harness (roadmap follow-up) before that.

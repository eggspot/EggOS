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
The memory map already comes from the device tree (D28).

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

## D19 Compatibility-layer order: Linux, Windows, Android, macOS, iOS — Decided by research; superseded by D34 (owner, 2026-10-22); supersedes D2's order
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
  its benchmark comes with the benchmark harness (roadmap follow-up) before that. (Done: D27.)

## D26 Clock and timer interrupts: one monotonic clock and one one-shot timer per CPU — Decided by the build loop (engineering)
The HAL gives the kernel a per-CPU monotonic clock (`clock_ticks`, `clock_frequency`) and a
one-shot timer (`set_timer_deadline`, in clock ticks), and delivers its expiry as
`eggos_hal::Interrupt::Timer` to the kernel binary's `kernel_interrupt`, the same way exceptions
reach `kernel_exception` (D25). The contract is "never early, once per arming": the kernel builds
periodic ticks, tickless idle and preemption (M4) on top, so the HAL stays policy-free. A
deadline (not a period or a relative delay) is what every architecture's hardware compares
against, so arming costs one register write on Arm and RISC-V and no drift accumulates.
`wait_for_interrupt` sleeps with interrupts masked and takes the pending interrupt on wake-up,
which is race-free on every architecture (the condition check and the sleep cannot miss the
interrupt that changes it). The HAL acknowledges the interrupt and quiets the source before
the kernel runs, so the kernel may later switch context from the handler without returning.

Per architecture:
- **AArch64:** `CNTVCT_EL0` (with an `isb` so a read is not performed early) and the EL1
  virtual timer, PPI 27, through a GICv2 (QEMU `virt` with `gic-version=2`, set explicitly by
  the runner). The handler delivers only if `CNTV_CTL_EL0.ISTATUS` says the deadline is met,
  dropping an interrupt the GIC latched for a deadline since moved later. GICv3 and
  device-tree discovery of the GIC come with M5.
- **RISC-V:** `time` and the SBI Timer extension (works with and without Sstc; using
  `stimecmp` directly when Sstc is present is a later optimisation). The 10 MHz timebase is QEMU
  `virt`'s, read from the device tree in M5.
- **x86_64:** the TSC (read after `lfence`) and the local APIC timer in one-shot mode. Both
  frequencies are measured at boot against the 8254 PIT over 2 ms (boot-time cost justified:
  without it there is no timer at all); the APIC timer is armed rounded up and re-armed if it
  still fires before the TSC deadline, so measurement error never makes a deadline early. The
  measurement gives up after about 2 s of TSC time and the kernel panics with a clear message,
  because some machines and hypervisors have no working 8254 (decided by research: keep the
  2 ms PIT measurement now, bounded; read the frequencies from CPUID 0x15, hypervisor leaves
  0x40000010 / Hyper-V MSRs, and use TSC-deadline mode where available, with the ACPI PM timer
  or HPET as fallback reference clocks, as follow-ups). The legacy 8259s are remapped to
  0x20-0x2f and masked, because the firmware leaves the PC timer on vector 8 (the double-fault
  vector); their spurious interrupts are dropped. Vectors: 0x30
  APIC timer, 0xff APIC spurious; all other unused vectors have not-present gates, so a stray
  interrupt is reported as a fault instead of running random code.
- Unexpected interrupts (any source the kernel did not ask for) are reported as an
  `Exception` of kind `Interrupt` and are fatal, like other unhandled exceptions; spurious
  ones are dropped in the HAL.
- Per-CPU timer state is in statics while only the boot CPU runs; it moves to per-CPU data
  with SMP (M4).
- Testing: a boot self-test arms a 1 ms deadline and sleeps in `wait_for_interrupt` until the
  kernel counts the timer interrupt, checks it did not come early, and gives up after 1 s;
  drivers (GICv2, local APIC, 8259, 8254) are unit-tested on the host against fake registers.
- Not benchmarked yet (as D25): interrupt entry and timer arming join the benchmark harness
  (roadmap follow-up) before M4 uses them for preemption. (Done: D27.)

## D27 Benchmarks: deterministic instruction counts in QEMU, gated against a committed baseline — Decided by the build loop (engineering)
"Never merge a performance regression" (D13) needs a number CI can check. Wall-clock timings on
shared CI runners vary by tens of percent between runs, so they cannot gate a PR. Instead, the
kernel feature `qemu-bench` runs boot-time benchmarks after the self-tests (`kernel::bench`:
each body runs 100 untimed then 10 000 timed iterations between two reads of the platform
clock) and `cargo xtask bench` boots it under QEMU `-icount shift=0,sleep=off`, where guest
time advances exactly 1 ns per instruction executed. Each result is therefore an instruction
count per iteration, identical on every run and every host (checked: repeated runs give the
same numbers to the last digit), and is compared with `benchmarks/baseline.txt`:
- slower than the baseline by more than 1% + 0.5 instructions fails (CI and `cargo xtask ci`
  run it after the boot tests); faster is reported so the baseline can be tightened;
- a benchmark without a baseline line, or a baseline line no longer measured, fails, so
  benchmarks cannot be added or dropped silently;
- `cargo xtask bench --arch <arch|all> --update-baseline` rewrites the lines of the
  architectures it ran; a PR that changes the baseline says why (faster, or a regression the
  owner accepted).

First benchmarks: the empty loop (the per-iteration overhead in every other result), a clock
read, a breakpoint round trip (exception entry and return, the future syscall and page-fault
path, D25), arming the timer, and a timer-interrupt round trip (D26).

Limits, accepted for now: an instruction count is a proxy, not a cycle count; it does not see
cache misses, pipeline stalls or slow device registers. On RISC-V, `timer_arm` and the
interrupt round trip include the SBI firmware's instructions, so they depend on the OpenSBI
build QEMU loads (measured: v1.3 in CI's Ubuntu 24.04 packages gives 266 and 640, v1.7 in
Homebrew's QEMU 10 gives 286 and 680; every other result is identical on both). The baseline
records the firmware version (`riscv64 firmware OpenSBI-v1.3`) and follows CI; on other
firmware those two results are printed but not gated, everything else still is, and
`--update-baseline` keeps their baseline values, so updating on a developer machine cannot
move CI's numbers. Host
microbenchmarks were considered and left out: host timings are as noisy as CI wall-clock, and
architecture-independent code (allocators, data structures) runs in this harness on the real
targets instead. Cycle-accurate benchmarks on real machines, and comparisons against macOS,
Windows and Linux on the same hardware, come with real-hardware bring-up (M13).

## D28 Physical memory: the HAL reports free RAM, the core allocates frames (bump + free stack) — Decided by the build loop (engineering; allocator choice decided by research)
First step of M2. Each architecture module builds a `MemoryMap` (`Platform::memory_map`) from
what its firmware describes, adds the RAM, then reserves everything in use; the
architecture-independent core turns it into a `FrameAllocator` of 4 KiB frames.
- **Where the memory map comes from.** RISC-V: the device tree whose address OpenSBI passes in
  `a1` (its `/memory` nodes, memory reservation block and `/reserved-memory`, where OpenSBI
  reserves itself). AArch64: the device tree at the start of RAM, where QEMU `virt` puts it for a
  non-Linux ELF image (it passes the address in `x0` only to Linux images), and only if it fits
  below the kernel; taking it from `x0` comes with booting through other loaders (M5, M13). QEMU's tree is 1 MiB, so the AArch64 kernel now
  links at `0x4020_0000` instead of `0x4008_0000` (2 MiB aligned, which later suits a 2 MiB
  kernel mapping). x86_64: the PVH start info's memory map (version 1, E820 types; RAM is type
  1, and every other entry is reserved, so where entries overlap the more restrictive wins). The device-tree reader (`eggos_hal::fdt`) is architecture-independent and reads only what
  memory needs; device discovery from it comes with M5 (D12).
- **Reserved:** the kernel image with `.bss` and boot stack (`__kernel_start..__kernel_end`),
  the device tree (kept for M5), every firmware reservation, and on x86_64 all memory below
  1 MiB (BIOS data, EBDA, the BIOS area where the RSDP is found, and the future AP start-up
  trampoline), the PVH start info and its memory map. On x86_64 only memory inside the boot
  identity map (below 4 GiB) is offered until the kernel has its own page tables (next M2 step).
- **The map** has a fixed capacity (64 ranges), needs no allocator, keeps whole frames only and
  rounds reservations out to whole frames. When it is full it drops RAM (the smallest range),
  never a reservation, so running out costs memory, not correctness.
- **The allocator** (decided by research, options: bump + free stack, bitmap, buddy): a bump
  allocator over the free ranges, top down so low memory that some devices' DMA is limited to
  goes last, plus a stack of freed frames linked through their first word (reached through
  `Platform::physical_memory_offset`, 0 while the kernel runs identity-mapped). Building it
  costs O(ranges), not O(frames), so boot time does not grow with RAM, and it uses no metadata
  memory (D24); a frame never handed out is never touched. Allocating a never-used frame costs
  10-13 instructions and a free + allocate pair 15-16, beyond the benchmark loop's own 4
  (benchmarks `frame_allocate_untouched`, `frame_free_allocate`). A bitmap
  was rejected for its boot-time initialisation and O(n) search; a buddy allocator (contiguous
  power-of-two blocks for DMA and huge pages) replaces this one when paging and the direct map
  exist and SMP needs per-CPU caches (M2/M4, as Linux's memblock hands over to its buddy
  allocator); the free stack becomes the per-CPU cache.
- Not yet: a global allocator instance and its lock (the paging step adds them, with the first
  user), double-free detection (the buddy allocator's metadata gives it for free).
- Testing: host unit tests for the map (merging, splitting, rounding, a full map, randomised
  invariants), the device-tree reader (QEMU-shaped trees, cell sizes, disabled nodes, every
  truncation and single-byte corruption of the structure block) and the allocator (top-down
  order, LIFO reuse, exhaustion, randomised allocate/free); a boot self-test on every
  architecture writes and reads back 64 frames, checks LIFO reuse, and counts every frame to
  the end of memory against the map.

## D29 Kernel page tables: one architecture-independent mapper, a direct map in the high half, W^X kernel sections and a stack guard page — Decided by the build loop (engineering; Sv39 and kernel placement decided by research)
Second step of M2. The kernel now runs on page tables it builds itself on every architecture.
- **One mapper for every architecture.** All supported architectures translate through a
  radix tree of 4 KiB tables of 512 entries, 9 address bits per level. The HAL describes only
  what differs, as a `paging::Format` (entry encoding, levels, roots, the largest leaf, valid
  addresses): x86_64 4-level paging (2 MiB leaves; 1 GiB pages wait for a CPUID check),
  AArch64 4 KiB granule with 48-bit halves (TTBR0 and TTBR1, 2 MiB and 1 GiB blocks), RISC-V
  Sv39 (2 MiB and 1 GiB leaves). The formats are plain bit encodings compiled on the host too,
  so each is unit-tested against the manuals' bit patterns. The kernel's `PageTables<F>` walks,
  allocates tables from the frame allocator and maps every range with the largest leaves the
  alignment allows, so the direct map of 128 MiB costs a few tables and few TLB entries.
- **Layout.** The kernel image stays at its physical address (identity-mapped) for this step,
  with each part getting only the access it needs: code read+execute, `.rodata` (and the PVH
  note) read-only, data, `.bss` and the boot stack read+write and never executable. The page
  below the boot stack is left unmapped (a new guard page in each linker script), so an overflow
  faults instead of overwriting `.bss`. All physical memory is mapped read+write, not
  executable, at a fixed high-half offset, the direct map: x86_64 `0xffff_8000_0000_0000`,
  AArch64 `0xffff_0000_0000_0000` (TTBR1), RISC-V `0xffff_ffc0_0000_0000` (Sv39's upper half,
  256 GiB). `Platform::physical_memory_offset` becomes that offset once the tables are active,
  and the frame allocator, the tables and the HAL's readers of firmware data (device tree,
  PVH start info, ACPI) go through it. The direct map covers `Platform::physical_memory`: every
  RAM range of the device tree, or every entry of the PVH memory map (whatever its type, since
  the ACPI tables live in reserved entries) plus the low 1 MiB; boot fails if free memory lies
  outside it. The device registers the HAL already drives (UART, GIC, local APIC) are mapped as
  device memory at their physical addresses until the driver model maps devices (M5).
- **Switching.** AArch64 programs `MAIR_EL1` (index 0 Device-nGnRnE, 1 Normal write-back),
  `TCR_EL1` (48-bit halves, 4 KiB granule, cached inner-shareable walks, IPS from
  `ID_AA64MMFR0_EL1`), both TTBRs, invalidates the TLB and turns on the MMU and both caches.
  RISC-V writes `satp` (Sv39) between two `sfence.vma`. x86_64 sets `EFER.NXE` and `CR0.WP` (so
  read-only means read-only for the kernel too) and loads CR3; the boot GDT's accessed bits are
  preset so the CPU never writes the now read-only GDT.
- **Sv39 rather than Sv48 (decided by research).** Every RVA profile (RVA20/22/23) requires
  Sv39 and Sv48 is optional, shipping cores (SiFive U74, T-Head C910) are Sv39-only, and a TLB
  miss walks three levels instead of four. The mapper takes the level count from the format, so
  Sv48 or Sv57 can be chosen at boot when memory or address space needs it (roadmap follow-up).
- **The kernel stays at its physical address in this step (decided by research).** Relinking
  to the high half needs position-independent pre-MMU code and a jump to the high alias on
  three architectures, a separate risk from building tables. It is the next M2 step and must
  land before user space (M4/M6): user space owns the low half (TTBR0 on AArch64), and a
  low-half kernel would get in the way of SMEP/SMAP/PAN and address-space randomisation.
- **Testing.** Host unit tests for each format (bit patterns from the manuals, round trips,
  canonical addresses) and the mapper on all three formats (largest leaves where aligned,
  translation, double mapping, invalid and unaligned ranges, running out of frames). Boot
  self-tests on every architecture check every part of the image has exactly its access, the
  guard page is unmapped, the devices are device memory, and a frame written through the
  direct map is where the tables say; the frame self-test now writes every free frame through
  the direct map and gives them all back. The fatal-exception boot now writes the kernel's
  `.rodata` instead of executing an illegal instruction, and the runner requires the fault
  report to name that address, so hardware write protection is tested on every architecture
  in CI without an extra boot.
- **Benchmarks.** No change in any result (the switch adds no instructions to measured paths);
  mapping is boot-time only for now, and its benchmarks come with the first runtime user (page
  faults, `mmap`).
- **No global frame allocator yet** (D28 planned it, with its lock, for this step): nothing
  allocates concurrently, so boot owns the kernel's `Memory` (frames and tables) and hands it
  to the self-tests; the global instance and its lock come with the first concurrent user (M4).
- **Not yet, recorded on the roadmap:** the high-half kernel; changing live tables (TLB
  shootdown, M4); a separate exception stack (x86 IST, AArch64/RISC-V stack switch) so a guard
  page hit is reported instead of faulting again on the same stack; honouring device-tree
  `no-map` reservations (they are in the direct map, never touched); on AArch64 real hardware,
  cleaning and invalidating the data cache over memory written with the MMU off before the
  caches go on (QEMU models no caches, M13); RAM above 4 GiB on x86_64 (tables are still built
  through the boot identity map); on x86_64 the direct map covers reserved PVH entries as
  cached memory because QEMU's firmware puts the ACPI tables in one (checked: power-off faults
  without it), which on real hardware may alias device registers with the wrong memory type,
  so the ACPI work (M5) maps firmware tables on demand and reserved ranges uncached; 1 GiB pages on x86_64; Svpbmt on RISC-V.

## D30 Kernel heap: power-of-two size classes in the direct map, one frame at most until the buddy allocator — Decided by the build loop (engineering; ordering, large allocations and locking decided by research); the panic on a held lock superseded by D43
Third step of M2. `alloc` (`Box`, `Vec`, `BTreeMap`, ...) works in the kernel on every
architecture once `paging::enable` has run; before that, allocating fails.
- **Before the high-half relink (decided by research).** D29 planned the relink first. The heap
  does not depend on where the image is linked: it lives in the direct map, which the relink
  does not move, and the relink's pre-MMU code cannot allocate anyway. Splitting them keeps each
  change reviewable and benchmarked on its own. The relink is still required before M4.
- **Design.** Size classes of 16, 32, ..., 2048 bytes. Each class carves 4 KiB frames from the
  frame allocator into blocks of its size, reached through the direct map, so every block is
  aligned to its size (any alignment up to 2048 is served by picking the class from
  `max(size, align)`). A class keeps three words: the most recently freed block (freed blocks
  are linked through their first word) and the never-used rest of the frame being carved, so a
  block never handed out is never touched and the heap needs no other metadata. Layouts of 2049
  bytes to one frame (alignment up to 4096) take a whole frame, given back on free. `realloc`
  returns the same pointer when the new size stays in the same class.
- **Nothing larger than a frame yet (decided by research).** Options: fail above one frame;
  carve contiguous frames from the frame allocator's never-used ranges; the buddy allocator
  now. The second was implemented first and failed on every boot: the frame self-test cycles
  through all memory once, after which no never-used range is left, so contiguous allocation
  would depend on history. The buddy allocator in the same change would mix two decisions and
  risk a benchmarked regression on single-frame allocation. So larger layouts fail (null, and
  `alloc`'s handler panics) until the buddy allocator, the next M2 step, which keeps today's
  free stack as its order-0 cache so single-frame allocation stays as fast, and which virtio
  DMA (M8) needs as well.
- **Empty slabs are not returned yet**: knowing a frame's blocks are all free needs a count per
  frame; it comes with the buddy allocator's per-frame metadata. Memory held by a class is
  bounded by that class's peak use.
- **Locking (decided by research).** One spin lock for the heap and one for the frame allocator,
  which moves from boot's `Memory` into a global (`memory::FRAMES`, D28's planned global
  instance). Lock order: heap, then frames. A block allocation or free never takes the frame
  lock. The lock does not mask interrupts (saving and restoring the interrupt state on every
  allocation would cost every allocation, and no handler needs to allocate), so interrupt and
  exception handlers must not allocate. On the bare-metal targets, with one CPU so far, the
  heap takes its lock with a try-lock and panics if it is held, which can only mean a handler
  re-entered the heap: a clear report instead of a hang, at no cost to the handlers (counting
  handler entries was measured at +2 to +11 instructions on the interrupt and breakpoint
  round trips, a regression, and dropped). Per-CPU caches replace this with SMP (M4).
- **Benchmarks.** `heap_allocate_free_small` (a 32-byte block allocated and freed through the
  global allocator: lock, free-list pop and push, unlock): 52 instructions per pair on
  aarch64, 95 on riscv64, 52 on x86_64, beyond the loop's own 4. Every other result unchanged.
- **Testing.** Host unit tests for the spin lock (exclusion across threads), the size-class
  choice, alignment, distinct blocks, LIFO reuse, frame use per class, frames given back, and
  exhaustion and oversized layouts failing. A boot self-test on every architecture allocates
  from every size class and as whole frames through the global heap, checks each block is
  aligned, writable and in the direct map and that none overlap (each filled with its own
  number), frees them and checks that exactly the whole frames came back, then grows a `Vec` to
  a frame through `realloc`. The spin lock's guard is `Sync` only for `T: Sync` (found by the
  independent review: without that, two threads could reach a `Cell` through one guard).

## D31 Contiguous frames: a buddy allocator behind the single-frame fast paths — Decided by the build loop (engineering; design checked by research)
Fourth step of M2, planned in D28 and D30. `FrameAllocator::allocate_contiguous(order)` hands out
2^order contiguous frames aligned to their size (order up to `MAX_ORDER` = 10, 4 MiB, as
Linux), and `free_contiguous` takes them back. The kernel heap uses it for allocations larger
than a frame (D30's one-frame limit is gone; up to 4 MiB now), and virtio DMA (M8) will.
- **Single frames stay on their fast paths.** `allocate` still pops the free stack, then bumps
  the untouched ranges; `free` still pushes the stack. Only when both are empty does `allocate`
  take a frame from the buddy allocator, through the same cold refill that moves to the next
  untouched range, so the hot path's code is unchanged (`frame_allocate_untouched` and
  `frame_free_allocate` within the benchmark gate on every architecture).
- **Where contiguous blocks come from:** the buddy allocator's free lists (splitting a larger
  block); else an aligned block carved from the top of the highest untouched range that has
  one, the frames above it going to the buddy allocator as aligned power-of-two pieces; else
  frames moved from the free stack into the buddy allocator, merging with free neighbours,
  until a block of the order forms (not the whole stack: draining millions of frames under the
  lock would stall, and the stack's frames are the cache-warm ones; research). Freed blocks go
  to the buddy allocator, never the stack.
- **Metadata (decided by research): one bit per frame**, set when the frame starts a free buddy
  block; the block's own first three words hold its list links and its order. Freeing finds out
  in one load whether the buddy is free (it is only read when the bit says so). A bit per frame
  is 8 times less than a byte: 4 KiB for 128 MiB, 2 MiB for 64 GiB. The bitmap covers the span
  from the lowest to the highest free frame and is taken from the top of the highest range that
  keeps a frame besides it (no such range: no buddy allocator, single frames only). It is not
  written until the buddy allocator is first used, so `FrameAllocator::new` stays O(ranges) and
  writes nothing (D28), and a boot that never needs contiguous memory never pays for it.
- **Correctness of the bit:** it is zero for everything that is not a free buddy block: frames
  in use, on the free stack, still untouched, the bitmap itself and the holes between ranges. A
  frame handed out before the bitmap is zeroed keeps a zero bit, which is right. Buddies are
  computed from physical addresses (blocks are aligned to their size physically), and a buddy
  outside the span is never read.
- **Heap size classes by table.** Profiling the heap benchmark on riscv64 showed most of its
  95 instructions were `next_power_of_two().trailing_zeros()`, which without the Zbb extension
  is a long software sequence. A 128-byte table indexed by `(size - 1) / 16` gives the class:
  `heap_allocate_free_small` 50 instructions on aarch64 (was 52), 57 on riscv64 (was 95), 48 on
  x86_64 (was 52). The slab refill moved out of line.
- **Benchmarks.** New `frame_contiguous_allocate_free` (an order-2 block allocated and freed
  through the global allocator; with the rest of memory free, each round splits down from and
  merges back up to a 4 MiB block): 159 instructions on aarch64, 200 on riscv64, 152 on x86_64.
  `frame_allocate_untouched` now maps twice the frames it allocates, so the bitmap (taken from
  the top, never written there) leaves it the same untouched frames to time.
- **Testing.** Host tests: blocks of every order aligned and disjoint and merging back into a
  4 MiB block from a base aligned to only 1 MiB; a block and its buddy merging; blocks forming
  again after every frame went through the free stack; the bitmap placed below a too-small top
  range, and no buddy allocator when nothing fits; 20,000 random single and contiguous
  allocations and frees with exact free-frame accounting, no overlap and the bitmap never handed
  out; the heap's multi-frame layouts. Boot self-tests on every architecture: blocks of every
  order (up to what a quarter of free memory allows) after the frame self-test sent every frame
  through the free stack, written, checked, freed and merged back into the largest block; the
  heap self-test adds 2-, 4- and 16-frame allocations and grows a `Vec` to 40 KiB.
- **Not yet:** finding a free run that straddles the untouched part of a range and frames on
  the free stack (the carve looks at untouched memory only and the drain stops at the first
  block of the order; found by the independent review, it costs at most about one block per
  range boundary, near exhaustion); double-free detection for single frames (a check in `free` would cost every
  free; a debug-only check comes with the per-CPU caches); one bitmap per range instead of one
  over the span (a sparse machine pays for its holes, 1 bit per 4 KiB); a physical address limit
  for 32-bit DMA devices (M8); anti-fragmentation grouping (Linux's migrate types); a high
  watermark on the free stack, giving frames back to the buddy allocator (with per-CPU caches,
  M4).

## D32 The direct map keeps the kernel image's protections — Decided by the build loop (review fix)
Found by the build loop's review of M2 (D28-D31). D29 mapped all of physical memory read+write
in the direct map, the kernel image included, so the image's code and constants were writable
through their direct-map alias (`.text` at its physical address + the direct map offset): one
stray or malicious kernel write could patch code, and the W^X that D29 tests on the image's own
mapping did not hold for the frame.
- **The fix.** The direct map is built around the kernel image: RAM below and above it stays
  read+write and never executable; each part of the image gets its own access minus execute
  (code and constants read-only, data, `.bss` and the boot stack read+write), and the stack
  guard page is unmapped there too. Every mapping of an image frame is now writable or
  executable, never both, and nothing that is read-only at the image's address is writable
  anywhere. Nothing in the kernel wrote the image through the direct map.
- **Cost.** The 2 MiB block or two holding the image are mapped with 4 KiB pages instead of a
  large leaf: one or two more 4 KiB page tables and more TLB entries for direct-map accesses
  there. On aarch64 and riscv64, which have 1 GiB leaves, a machine with a whole GiB of RAM
  around the image also maps that GiB with 2 MiB leaves instead of one 1 GiB leaf (one more
  table, more TLB entries for direct-map accesses anywhere in that GiB); x86_64 has no 1 GiB
  leaves yet. QEMU's 128 MiB never used a 1 GiB leaf, so no benchmark changed; the cost on
  real hardware is measured with M13.
  The high-half relink (roadmap) maps the image at its own virtual address and keeps this rule.
- **Testing.** Host tests map the direct map around a kernel image and check the access at
  both ends of every part, the guard page and the surrounding RAM, that large leaves are still
  used away from the image (2 MiB on x86_64, 1 GiB on Sv39), and an image across two RAM ranges
  with a hole in it. The boot self-test now checks every part's
  direct-map alias and the guard page's on aarch64, riscv64 and x86_64; with the old mapping it
  fails on all three. The host test platform's direct map now covers its kernel image, as on
  real machines.

## D33 The kernel runs in the top 2 GiB; the low half is empty — Decided by the build loop (engineering; design checked by research)
Last step of M2, required before user space (M4/M6). Supersedes D29's "the kernel image stays
at its physical address" and its identity-mapped devices.
- **Layout.** The kernel is linked, and runs, in the top 2 GiB of the address space on every
  architecture: x86_64 at `0xffff_ffff_8010_0000` (the kernel code model requires the top
  2 GiB), aarch64 and riscv64 at `0xffff_ffff_8020_0000`. It is still loaded at its physical
  address: every ELF segment's `p_paddr` is the physical address (`AT()` in each linker
  script) and the ELF entry, like the PVH note on x86_64, names the physical `_start`.
  The image's virtual minus physical address, `KERNEL_VIRTUAL_OFFSET`, is a constant in each
  HAL module, which the linker script repeats and checks at link time against a symbol the HAL
  exports. `KernelImage` reports the physical parts plus that offset.
- **Boot page tables, built in assembly before any Rust code runs.** The entry code maps, with
  the largest leaves: the gigabyte holding the kernel at its physical address (only for the
  instructions that turn translation on and jump high) and at its virtual address, read, write
  and execute; and a boot direct map at the same offset the kernel's own tables use (x86_64
  0..4 GiB with 2 MiB pages, as before; aarch64 256 GiB of 1 GiB blocks, device memory below
  the gigabyte RAM starts in, normal memory from there on; riscv64 254 GiB of gigapages). It
  then jumps to the kernel's virtual address and only then sets the stack, the exception
  vectors and the GDT (x86_64 reloads it with its virtual address), and calls Rust. So Rust
  only ever runs at the kernel's virtual address and reaches physical memory through one
  constant offset, the direct map: `Platform::physical_memory_offset` no longer changes when the
  kernel's tables go live, and the frame allocator and the tables never switch offsets. Free
  memory handed out is limited to what the boot direct map covers (the page tables are built
  through it), as x86_64 already did.
- **The kernel's own tables** map the image at its virtual address with W^X as before (D29,
  D32), the direct map, and the early devices as device memory in the direct map's window
  (physical address + direct map offset), and nothing in the low half. The self-test checks
  that the image and the devices are no longer reachable at their physical addresses. Devices
  in the direct map's window (decided by research): the final direct map covers only RAM and
  firmware tables, so a device's physical address never gets two memory types (Arm ARM B2.8,
  Intel SDM's PAT rules); the HAL computes the addresses as constants, so no hot path pays for
  it. The driver model (M5) moves to a device window behind a mapping call (PCI BARs above the
  direct map, devices sharing a large leaf with RAM).
- **Sv39 (decided by research).** The direct map and the kernel share Sv39's 256 GiB high half,
  so the direct map holds 254 GiB; RAM beyond it is left unused (and the kernel refuses to build
  a direct map that would reach its own addresses).
  Putting the kernel inside the direct map instead would lose its separate W^X mapping. Sv48 or
  Sv57 chosen at boot (D29 follow-up) lifts the limit, as will vmalloc, stacks and the device
  window once they need room.
- **Switching (decided by research).** riscv64 writes `satp` between two `sfence.vma`; x86_64
  loads CR3 (the boot tables set no global pages, so the TLB is flushed). On aarch64 the boot
  tables' 1 GiB kernel block and the new 4 KiB pages must never meet in the TLB (break-before-
  make, TLB conflict aborts): the code branches to the kernel's identity alias, points TTBR1 at
  the empty low root, invalidates the TLB, points TTBR1 at the new tables, branches back high,
  then replaces TTBR0 with the empty root and invalidates again. MAIR and TCR are set once, by
  the boot code.
- **Benchmarks.** No result is slower. Breakpoints got faster (aarch64 106 -> 92, riscv64
  159 -> 132 instructions) because the fatal-exception report moved out of line: the console
  code was costing the resumable path a saved register. The baseline records the new numbers.
- **Not yet, on the roadmap:** aarch64 real hardware needs the boot tables' cache lines
  invalidated (written with the MMU off, walked cached) and RAM mapped from the device tree
  instead of a fixed Device/Normal split (M13); secondary CPUs start on the kernel's tables
  (M4); global kernel mappings and PCID (M4).

## D34 Compatibility-layer order: Linux, Windows and macOS together, then Android and iOS — Decided (owner, 2026-10-22); supersedes D19
The three desktop layers are built side by side, not one after another:
- **Linux userland**: the kernel's native ABI, and the base the other layers build on;
- **Windows**: PE loader, NT system calls and Win32, DirectX going straight to our GPU drivers;
- **macOS**: Mach-O loader, dyld, the Objective-C and Swift runtimes, the Darwin frameworks, Metal.

**Android and iOS follow**, after the desktop layers and phone/tablet hardware bring-up (M13):
Android reuses the Linux ABI, iOS reuses the macOS loader, runtimes and frameworks.

Why: the owner wants people switching from all three desktop systems served at the same time
(D13, D23), rather than Windows users first and macOS users last. D19's research (desktop market
share, which catalogues are reachable without an app store) stays valid as background.

Consequences: M15 work for Linux, Windows and macOS is interleaved and tracked per layer in the
roadmap's support matrix, and D17's rule (the same features on each) is checked across all three
as they grow. Within that, the pieces the other two layers depend on (process model, files,
sockets, threads, the Linux userland base) still land first. Each layer stays an optional
component (D18).

## D35 Kernel threads: a one-word context saved on the thread's stack, a cooperative round-robin scheduler first — Decided by the build loop (engineering; stack size and overflow check decided by research)
First step of M4.
- **The switch (HAL).** `Platform::prepare_thread` and `Platform::switch_thread`. A thread that
  is not running is one word, its saved context: the stack pointer the switch left it at. The
  switch pushes the registers the calling convention says a call preserves (x86_64 rbx, rbp,
  r12-r15; aarch64 x19-x30; riscv64 ra, s0-s11), stores the stack pointer, loads the other
  thread's, pops and returns into it. No FP/SIMD registers: the kernel is soft-float on
  x86_64 and aarch64 and runs with the FPU off on riscv64; when user threads bring FP state,
  it is saved lazily on the user/kernel boundary, not here (should the kernel itself ever turn
  the riscv64 FPU on, the switch must save fs0-fs11 too). A new thread's initial frame is
  what a switch would have left, returning into a per-architecture trampoline that calls
  `entry(argument)` with the stack 16-byte aligned. The shared layout code is
  `eggos_hal::thread`, unit-tested on the host.
- **The scheduler (kernel, `thread`).** One run queue, round-robin, for the one CPU; a thread
  runs until it calls `yield_now` or its function returns. `spawn` takes a closure; the boot
  code becomes thread 0 the first time it uses the scheduler. An exited thread's stack is freed
  by the next thread to run (a thread cannot free the stack it is on), and `JoinHandle::join`
  returns only once that has happened. The run queue's capacity grows in `spawn`, so a switch
  never allocates. The scheduler's lock is released before the switch: correct only on one
  CPU with interrupts masked and no handler using the scheduler, which is how the kernel runs
  today; the SMP step hands the lock over across the switch instead.
- **Stacks (decided by research).** 16 KiB (an order-2 buddy block) through the direct map, the
  size Linux uses on x86_64 since 3.15 and on arm64/riscv64, FreeBSD on amd64/arm64; frames
  are small (soft-float, LTO, interrupts never nest). Under the direct map's large pages a
  stack costs no TLB entries. Overflow detection: a canary in the stack's lowest word, checked
  on every switch away from the thread (one load and compare); a clobbered canary stops the
  kernel naming the thread. It catches overflow only after the fact and a large frame can jump
  it, so it is a backstop: guard pages come with a kernel virtual area and exception stacks
  (a guard-page fault on the same stack cannot be reported), both on the roadmap.
- **Testing.** Host unit tests run the scheduler on the test platform, where each kernel thread
  is a host thread and a switch hands a baton between them, so exactly one runs at a time:
  turn order, stack freeing, yielding alone, the canary. The boot self-test runs two threads
  and the boot thread through three rounds each, checking turn order, values live across every
  switch (registers and stack) and that joining frees both stacks, on aarch64, riscv64 and
  x86_64; with one register pair not restored on aarch64 the boot fails.
- **Benchmark.** `thread_yield_round_trip`: two switches through `yield_now` per iteration,
  with the queue rotation and canary checks: aarch64 162, riscv64 201, x86_64 149 instructions.
  No existing benchmark changed.

## D36 CI's Linux job runs in our own image with QEMU and the toolchain preinstalled — Decided by the build loop (owner request, 2026-10-22); extends D8
The `check` job installed QEMU from the Ubuntu mirror and the pinned toolchain on every run.
Over 11 green runs on 2026-10-22 the toolchain took 10-14 s and QEMU 13-23 s, except four runs
where the mirror stalled `apt-get` for 194, 198, 850 and 1373 s (unpacking took about 4 s).
- **The image.** `.github/ci-image/Dockerfile`: Ubuntu 24.04 with QEMU and OpenSBI (the same
  packages the benchmark baseline is recorded with), git, gcc and gh, and the toolchain
  `rust-toolchain.toml` pins. It only installs tools, as CLAUDE.md allows outside `cargo xtask`.
  `.github/workflows/ci-image.yml` builds it, checks it holds every tool CI uses, and pushes it
  to `ghcr.io/eggspot/eggos-ci:main` (and the commit's SHA) only from `main`, when the image's
  inputs change or by hand, reusing the previous image's layers; PRs that change the inputs
  build and check it without pushing.
- **CI** pulls `:main` with the workflow's own token (`packages: read`) and drops its QEMU step.
  `rustup toolchain install` stays: a no-op with the image's toolchain, and an install only
  when a PR moves the pin before the image is rebuilt from `main`. The `target/` cache stays.
- **Rollout:** the image workflow lands first and builds the image, then CI switches to it,
  so `main` never points at an image that does not exist.
- **Not decided here:** making the package public, which fork PRs may need to pull it, is an
  organisation setting for the owner. Update (2026-10-22): the first push created it public and
  linked to this repository, so fork PRs can pull it; nothing was changed by hand.
- **Image changes land first:** CI runs in `main`'s image, so a PR that needs something new in
  the image adds it in its own PR, which `ci-image.yml` builds and checks; CI uses it once
  that PR is on `main`. Pinning `ci.yml` to an image digest (so a rebuild cannot bring a newer
  QEMU into the benchmarks unnoticed) is a possible follow-up.

## D37 Preemption is voluntary inside the kernel: a timer tick ends time slices, threads switch at preemption points — Decided by the build loop (engineering; model decided by research); supersedes D35's lock release before the switch
Second step of M4.
- **The model (decided by research).** Three options: full kernel preemption (a per-CPU
  preempt-disable count in every `SpinLock`, as Linux `CONFIG_PREEMPT` and FreeBSD do), spin
  locks that mask interrupts, or a non-preemptible kernel with voluntary preemption (Linux
  `PREEMPT_NONE`/`PREEMPT_VOLUNTARY`, the server default; seL4's preemption points). The
  first two put instructions on every lock and unlock: about +40% on
  `heap_allocate_free_small`. We chose the third: kernel code keeps running with interrupts
  masked, so nothing is ever preempted while it holds a lock and an interrupt handler never
  finds one held, and locks cost nothing extra. Kernel latency is the longest stretch between
  preemption points, which long-running kernel code bounds by calling one; user code will be
  preempted involuntarily on its return to user mode (processes, M4). All locks go through
  `sync`, so full preemption can still be added there later (for example as a low-latency
  option for desktop and phone) if measured latency calls for it.
- **The tick.** `thread::enable_preemption` arms the timer for a 10 ms time slice
  (`TIME_SLICE_MS`, as macOS's default quantum; Linux and Windows use a few to tens of ms);
  `disable_preemption` replaces the deadline with one never reached. While it runs, the tick
  owns the CPU's one-shot timer; sharing it with sleeping threads comes with wait queues.
- **Preemption points.** `thread::preemption_point` opens an interrupt window (new
  `Platform::interrupt_window`: unmask, let a pending interrupt in, mask again; `sti; nop;
  cli`, `msr daifclr; isb; msr daifset`, `csrsi; csrci` on SIE) and compares the timer
  interrupt count with the count when the slice started. If a timer interrupt has come since
  and preemption is on, it starts the next slice and yields. The interrupt handler does
  nothing for preemption, so the interrupt path costs exactly what it did. A preemption point
  costs 9 instructions on aarch64 and x86_64 and 7 on riscv64 when the slice is not over
  (`preemption_point_idle`).
- **The scheduler's lock is handed over across the switch** (a D35 follow-up, done here):
  `yield_now` and `exit` keep it held through `switch_thread` (`SpinLockGuard::leak`) and the
  resumed thread releases it (`SpinLock::assume_locked` in `finish_switch`). Nothing can take
  the scheduler between choosing the next thread and saving the previous one's context, on any
  CPU, which SMP needs; and a switch takes the lock once instead of twice:
  `thread_yield_round_trip` went from 162 to 144 instructions on aarch64, 201 to 183 on
  riscv64 and 149 to 135 on x86_64.
- **Testing.** Host unit tests: a thread that only passes preemption points lets another run
  once its slice is over, and with preemption off a preemption point never switches. Boot
  self-test on aarch64, riscv64 and x86_64: with preemption on, a thread that never yields
  spins at preemption points until a second thread has run; with the interrupt window
  removed on aarch64 the boot fails ("not preempted within 2 s").
- **Follow-ups (roadmap).** Involuntary preemption on return to user mode; sleeping and wait
  queues sharing the timer (tickless: no tick while one thread is runnable); per-CPU tick
  state and run queues (SMP); a measured worst-case gap between preemption points as a
  latency benchmark; a debug-build check that no lock is held at a preemption point.

## D38 The direct map leaves early device pages out; x86 firmware tables are read only inside the direct map — Decided by the build loop (review fix)
A review of D33 found two gaps on x86_64, neither visible on QEMU:
- **A device page inside a firmware range.** x86_64's `physical_memory` covers every entry of
  the loader's memory map (the ACPI tables sit in reserved entries), and loaders such as Xen
  PVH dom0 list the local APIC's page as reserved. The kernel then mapped that page as normal,
  cached memory in the direct map, and mapping it as a device afterwards failed ("a page is
  mapped twice"), so boot stopped. Now `kernel_page_tables` leaves the pages of every early
  device region out of the normal direct map and maps them as device memory only, on every
  architecture: one page never gets two memory types. Should leaving them out split a range of
  a memory map that is already full (64 ranges), which would drop other memory from the
  direct map, building the tables fails (`TooManyRanges`) instead; and free memory that
  overlaps an early device's page stops the boot, so the kernel never hands out (and writes)
  a device's registers as RAM.
- **ACPI tables outside the direct map.** `mapped` only checked that a table lay below 4 GiB,
  while the kernel's direct map covers only `physical_memory`; a table in a hole of the
  loader's map would have faulted at power-off. `mapped` now requires the table to lie inside
  one range of `physical_memory`. The loader's own data (the start info and its memory map,
  which `physical_memory` always includes) is checked against the boot map only
  (`in_boot_direct_map`), since `physical_memory` itself reads it.
- **Testing.** Host tests: a device page inside reported memory is mapped as a device, its
  neighbours as normal memory, and a full map fails instead of losing memory; both fail
  without the fix. QEMU boot and power-off on aarch64, riscv64 and x86_64.
- **Still open (roadmap):** reserved ranges other than device pages are still direct-mapped
  cached; mapping ACPI tables on demand and leaving reserved ranges out comes with M5.

## D39 Sleeping and wait queues: an idle thread, threads linked through themselves, one timer shared by the tick and the sleepers — Decided by the build loop (engineering; design decided by research); supersedes D37's tick owning the timer and D35's yielding `join`
Third step of M4.
- **Blocking.** A thread blocks in `thread::sleep_until` / `sleep` (until a clock tick, never
  waking early) or in `WaitQueue::wait_until(condition)`, which returns once `condition`
  holds; `wake_one` / `wake_all` make waiting threads ready again, first come first served.
  The condition is checked with the scheduler's lock held and the thread queued before the
  lock is released, so a wake that follows a change to the condition is never lost (on SMP
  too). `JoinHandle::join` now blocks on a wait queue the exiting thread's joiners share,
  instead of yielding in a loop. Not here yet: waits with a timeout, a condition-variable
  form for data behind a `SpinLock`, and a futex-style "wait while the word holds this value"
  (with the Linux ABI, M6).
- **Idle (decided by research).** Each CPU has an idle thread, as Linux, FreeBSD, Windows,
  macOS and seL4 do, rather than letting the blocking thread wait for interrupts on its own
  stack: an exiting thread cannot free its own stack, and on SMP another CPU could resume a
  thread whose context is not saved yet. It runs only when no other thread is ready: it arms
  the timer for the earliest sleeper alone and waits for an interrupt. It costs one 16 KiB
  stack per CPU, taken when the scheduler starts (the first `spawn`, or the first blocking
  call, which stops the kernel if even that memory is missing).
- **Lists (decided by research).** The ready threads, every wait queue and the sleepers are
  lists linked through a field of the thread itself (a thread is in at most one), so a switch,
  a block and a wake never allocate, and `spawn` no longer reserves run-queue capacity. The
  sleepers are kept sorted by wake time: O(n) to add one, O(1) to find and wake the earliest;
  enough for a few threads per CPU, behind three operations, so it can become a pairing heap or
  a timer wheel when measurements call for it. Wait queues are guarded by the scheduler's lock
  for now; their own locks come with per-CPU run queues.
- **One timer, tickless (decided by research).** The CPU's one-shot timer is armed for the
  earlier of the running thread's slice end, only while another thread is ready to run, and
  the earliest sleeper's wake time: one runnable thread takes no tick at all, and a woken or
  new thread brings the tick back (a slice already over then ends at the next preemption
  point). The scheduler remembers what it armed and arms again only for an earlier deadline;
  a later one waits for the earlier interrupt, which costs a spurious interrupt instead of an
  SBI call per switch on RISC-V. Slices are measured on the clock, as before: a thread that
  runs after a voluntary yield, block or exit has the rest of the slice under way, and one
  the idle thread lets run gets a fresh slice. Whenever the scheduler sees the timer has
  fired, it arms it again before going on, wherever it noticed (a preemption point, a yield,
  a block or the idle thread).
- **Expiry at preemption points, not in the interrupt handler (decided by the build loop,
  against the research's suggestion).** The handler still only counts timer interrupts; the
  scheduler compares the count with the one it last saw at every preemption point, yield and
  block, and in the idle thread, and then wakes the sleepers that are due and ends the slice
  if it is over. The interrupt path and `timer_interrupt_round_trip` cost what they did, and
  a sleeper wakes as late as the next preemption point, the same bound D37 gives preemption.
  Device interrupts that wake threads will need the handler to reach the scheduler (it may:
  handlers only run where no lock is held, D37).
- **Testing.** Host unit tests: sleepers wake in deadline order and never early while the
  boot thread blocks in `join` and the idle thread waits for the timer; a deadline already
  passed and a condition that already holds never switch; a wait queue wakes one thread or all
  in the order they waited, and a thread woken while its condition does not hold waits again;
  the tick is armed only while another thread is ready, stays armed when a slice ends in a
  yield, and a thread woken from idle gets a whole slice (both found in review, and both
  fail without their fix). Boot self-test on aarch64, riscv64 and
  x86_64 (`threads_sleep_wait_and_run_tickless`): three sleepers wake in order and on time, a
  waiter runs once its condition holds and it is woken, and 30 ms with preemption on and one
  runnable thread take no timer interrupt.
- **Benchmarks.** `thread_yield_round_trip` became faster with the intrusive run queue and the
  out-of-line slow paths: aarch64 144 → 138, riscv64 183 → 157, x86_64 135 → 113
  instructions. New: `wait_queue_wake_empty` (aarch64 16, riscv64 14, x86_64 16) and
  `thread_wait_wake_round_trip`, two blocks, two wakes and two switches (aarch64 228, riscv64
  228, x86_64 205). `frame_contiguous_allocate_free` on riscv64 went from 200 to 203 with the
  allocator's machine code unchanged (checked instruction by instruction): the idle thread's
  stack now holds one block, which changes how far the measured allocation splits and merges
  (aarch64 moved the other way, 159 → 158).
- **Follow-ups (roadmap).** Wait timeouts, the condition-variable and futex-style waits; per-CPU
  idle threads, run queues and wait-queue locks (SMP); RISC-V Sstc (`stimecmp` without an SBI
  call); waking from device interrupt handlers; per-thread CPU time from clock reads at each
  switch (ticks no longer sample it).

## D40 Secondary CPUs start on the boot page tables and report in through a per-CPU block — Decided by the build loop (engineering; design decided by research)
Fourth step of M4, the first part of SMP: every CPU starts; running threads on them (per-CPU
run queues) is the next step.
- **Discovery.** The HAL lists the CPUs by hardware id (`Platform::cpus`): the enabled children
  of the device tree's `/cpus` on aarch64 (MPIDR affinity) and riscv64 (hart id), the enabled
  Processor Local APIC structures of the ACPI MADT on x86_64 (local APIC id). x86 entries that
  are only online capable (hot-plug) and x2APIC structures (ids of 255 and up, beyond xAPIC
  mode) are skipped. `Platform::boot_cpu` names the CPU that booted: affinity 0 on aarch64
  (`_start` lets only it boot), the hart id OpenSBI passed in `a0` on riscv64 (any hart may
  win its boot lottery), the local APIC id read at boot on x86_64.
- **Numbering (decided by research).** The kernel numbers the CPUs itself: 0 for the boot CPU,
  then the others in the firmware's order. There is no compile-time CPU limit (D24): the
  per-CPU blocks are allocated once from the firmware's count, published as one table
  (`cpu::CPUS`), and never freed; a sanity cap of 4096 guards against a corrupt table.
- **Per-CPU block (decided by research).** `Platform::set_cpu_local` / `cpu_local` give each CPU
  a pointer to its `cpu::Cpu` block in a register only the kernel uses: `TPIDR_EL1` on aarch64,
  `tp` on riscv64 (the kernel uses no thread-local storage; `sscratch` stays free for the
  user-mode trap entry, which swaps it), the GS base (`IA32_GS_BASE`) on x86_64 (`swapgs` and
  `KERNEL_GS_BASE` come with user mode). The block's first word holds its own address, so x86
  reads it with one `mov gs:[0]`. The boot CPU's block is a static, set as `boot` starts.
  Hot paths will reach per-CPU state through it, never through the table.
- **Starting (decided by research).** PSCI `CPU_ON` over HVC on aarch64, SBI HSM `hart_start`
  on riscv64 (after probing for HSM), and on x86_64 INIT to every other CPU once, the SDM's
  10 ms wait once, then a start-up interrupt per CPU (a second one only if the first is not
  taken within 200 µs, SDM vol. 3A 9.4.4.1). The x86 CPU starts in real mode in a trampoline
  copied to physical 0x8000 (inside the low 1 MiB the allocator never hands out; checked to
  be RAM free of loader data), which enters protected mode with its own GDT, then long mode
  with the boot page tables, and jumps to the kernel's virtual address.
- **Page tables (decided by research).** A secondary CPU turns translation on with the boot
  page tables, still in `.bss` and never written after boot, which map the kernel at its
  physical and virtual addresses and the boot direct map; it jumps to the kernel's virtual
  address, reads its `CpuStart` (stack, entry, argument and the kernel's page-table roots,
  passed explicitly) and only then switches to the kernel's tables with the same
  `activate_page_tables` the boot CPU used (on aarch64 its break-before-make sequence, D33).
  It reads nothing the boot CPU wrote before translation and the caches are on: with the MMU
  off, AArch64 reads are not coherent with the boot CPU's caches.
- **Parking.** A secondary CPU sets its per-CPU register, marks its block online, counts
  itself in `CPUS.online()` and parks with interrupts masked. It prints nothing (the early
  console has no lock) and must not touch the scheduler, which is still one CPU's. Its 16 KiB
  stack, from the buddy allocator, is kept: it becomes the CPU's idle thread's. A CPU the
  firmware refuses keeps its stack too (an x86 CPU that missed its deadline may start late),
  and the start stops there.
- **Testing.** Host unit tests: the `/cpus` walker (enabled CPUs only, one- and two-cell ids,
  `cpu-map` and nested nodes skipped, malformed trees rejected), MADT parsing (enabled local
  APICs only, malformed structures stop the walk), the local APIC's INIT and start-up commands,
  and the kernel's numbering, publishing and refusal handling. The QEMU boot tests now run
  with `-smp 4`; the boot self-test `secondary_cpus_start_and_report_in` checks the boot CPU
  is CPU 0, every listed CPU reports in through its own block within a second, and the
  firmware lists all four CPUs. It fails if a CPU reports in through another CPU's block, or
  if discovery finds only the boot CPU (both checked by breaking them). Benchmarks stay on one
  CPU: with `-icount`, QEMU runs every CPU in one host thread.
- **Cost (D24).** Per secondary CPU: a 16 KiB stack and a few dozen bytes of block and start
  data. Boot time: one 10 ms INIT wait on x86_64 when there are other CPUs; none on aarch64
  and riscv64. No benchmark changed.
- **Follow-ups (roadmap).** Per-CPU run queues, idle threads and tick state, and wait-queue
  locks, so secondaries run threads; per-CPU interrupt controller and timer setup (the GIC CPU
  interface and timer PPI, `sie.STIE`, the local APIC and its LVT), with x86's IDT already
  split into fill-once and load-per-CPU; TLB shootdown for tables changed while other CPUs run
  (`tlbi ...is`, SBI RFENCE, IPIs); x2APIC mode for ids of 255 and up; checking for an
  invariant TSC before threads move between CPUs; booting from a CPU whose affinity is not 0
  on aarch64 (M13); a console lock for messages from any CPU (a parked x86 CPU that takes an
  NMI would report it through the shared handler); the PSCI conduit (HVC or SMC) and
  `enable-method` read from the device tree, and a full known `SCTLR_EL1` value instead of
  ORing bits into the reset value (real hardware, M13); starting the CPUs in `boot` itself,
  with the self-test only checking them, once secondaries run threads.

## D41 Every CPU takes its own interrupts: per-CPU timer and interrupt controller setup, wake-up interrupts between CPUs, per-CPU interrupt counts — Decided by the build loop (engineering); supersedes D40's parking with interrupts masked
- **Why now.** Per-CPU run queues (M4) need two things first: each CPU's own timer (for its
  slices and sleepers) and a way to wake an idle CPU when a thread becomes ready on it. D40
  left the secondaries parked with interrupts masked; this step makes them take interrupts,
  as its own reviewable change, before the scheduler goes per-CPU.
- **Setup.** Each CPU sets its own interrupt controller and timer up before it runs the kernel's
  code: the boot CPU's entry code as before, a secondary CPU's entry code right after it
  switches to the kernel's page tables, before `CpuStart::entry`. On aarch64: the timer PPI and
  the wake-up SGI enabled in the GICv2 distributor's banked registers, and the CPU interface
  on; on riscv64: `sie.STIE` and `sie.SSIE`; on x86_64: the local APIC enabled and its timer in
  one-shot mode. The x86 TSC and APIC timer frequencies are measured once, on the boot CPU, and
  shared (one machine's CPUs run them at one rate; the invariant-TSC check stays a follow-up).
- **Wake-up interrupt.** `Platform::wake_cpu(hardware_id)` raises `Interrupt::Wakeup` on another
  CPU, which carries nothing: the kernel looks at its own shared state to see why. GICv2 SGI 0
  on aarch64 (the target named by the CPU interface number each CPU records for its MPIDR
  affinity as it sets up, read from its banked `GICD_ITARGETSR0`; `dsb ishst` first so the
  target sees earlier writes), the SBI IPI extension on riscv64 (`fence rw, rw` first; the
  handler clears `sip.SSIP` before the kernel looks), a fixed IPI on vector 0x31 through the
  local APIC's ICR on x86_64 (an uncached store, ordered after earlier stores).
- **Per-CPU state.** The per-CPU block now starts with its own address, then a part the HAL
  owns (`hal::cpu::HalCpuLocal`, at offset 8), then the kernel's. x86 keeps its timer's
  deadline there, read and written GS-relative in one instruction (it was a global). The
  kernel counts interrupts in the CPU's block instead of a global: wake-ups, and timer
  interrupts split into those the scheduler has not seen yet and those it has.
- **Hot paths stay as cheap as globals.** `Platform::cpu_local_u64::<OFFSET>()` reads a word of
  this CPU's block in one load where the architecture allows: `gs:[OFFSET]` on x86_64,
  `OFFSET(tp)` on riscv64, `mrs` plus a load on aarch64. The preemption point's fast path is
  now one load and a branch on "timer interrupts not yet seen" (it compared two global
  counters), which makes it one instruction cheaper on every architecture; the scheduler
  moves them to "seen" when it looks. The register is read anew at every use (its asm is not
  `pure`), so the compiler never carries one CPU's block across a context switch that may
  later move the thread to another CPU. Trap dispatch checks the timer first, then skips the
  rarer interrupts' checks for exceptions, so adding an interrupt made no path slower.
- **Testing.** A secondary CPU reports in only after it takes its own timer interrupt (armed for
  now), so a CPU whose timer or interrupt controller is not set up never reports in. The boot
  self-test `cpus_take_their_own_interrupts` checks that every secondary took its timer
  interrupt, and wakes each one in turn: it must take exactly one wake-up within a second, and
  no other CPU (the boot CPU included) may count one. It failed as it should when a secondary
  skipped its setup (aarch64) and when wake-ups went to the wrong CPU (x86_64). Host unit tests
  cover the GIC SGI and target registers, the local APIC's fixed IPI, and per-CPU counting.
- **Benchmarks.** Faster: the preemption point (9→8 on aarch64 and x86_64, 7→6 on riscv64),
  yield and wait/wake round trips (one preemption-point-style check each), breakpoint dispatch
  on riscv64 and x86_64. Unchanged within the gate: the timer interrupt round trip (its
  benchmark now finds its CPU's block once, as it found the global's address once before);
  riscv64's SBI-bound round trip measured 670 on `main` and 673 here with this machine's
  OpenSBI 1.7.
- **Cost (D24).** 24 bytes more per CPU block (three counters), 32 on x86_64 (the timer
  deadline, which was a global); 64 bytes of GIC target
  table on aarch64; no boot time beyond one timer interrupt per secondary CPU.
- **Review fixes.** x86's `wake` puts a compiler fence before the ICR write (no instruction):
  the hardware keeps the uncached store after earlier stores, but the compiler may move
  ordinary accesses across volatile ones. aarch64 and riscv64 already had barriers in `asm!`
  without `nomem`, which the compiler does not move accesses across. The HAL's per-CPU part is
  empty except on x86_64. Another CPU reads a CPU's timer count seen-then-unseen, so it can
  lag but never runs ahead.
- **Follow-ups (roadmap).** Per-CPU run queues and idle threads using these wake-ups; a
  benchmark of the wake-up round trip once the benchmarks run on more than one CPU (they run
  on one: with `-icount`, QEMU runs every CPU in one host thread); the boot CPU woken by a
  secondary (same setup path, not yet exercised); probing for the SBI IPI extension instead of
  assuming SBI v0.2 or later (a debug check today); a visible error for a CPU beyond GICv2's
  8 interfaces; x2APIC ids above 254; GICv3
  (`ICC_SGI1R_EL1`), with more than 8 CPUs; the invariant-TSC check.

## D42 A disarmed timer stays quiet; x86 converts deadlines with a fixed-point factor — Decided by the build loop (review fix)
A review of D39-D41 (sleeping, secondary CPUs, per-CPU interrupts) found one defect, on x86_64.
- **Disarming armed the timer for its longest count.** The scheduler disarms the timer with the
  deadline `u64::MAX` (tickless idle, preemption off). On x86_64 that started the local APIC
  timer at its largest count (2^32 - 1) instead of stopping it, so an idle CPU woke for
  nothing once that count ran out: after 4.3 s on QEMU (measured; a 1 GHz APIC clock), after
  tens of seconds to minutes on real machines. The HAL then delivered nothing, so no test saw
  it, but it broke D39's promise of no tick on an idle CPU, which costs power. Now
  `set_timer_deadline(u64::MAX)` stops the APIC timer (initial count 0), and the `Platform`
  contract says what `u64::MAX` means on every architecture: no interrupt and no wake-up from
  then on (aarch64 and riscv64 already behaved so). Checked by a throwaway boot test that
  disarms and waits for an interrupt: before the fix it woke after 4315 ms; with it, the CPU
  slept until the QEMU runner's 30 s timeout. No permanent test: the kernel cannot see a wake
  that delivers no interrupt, short of waiting seconds in every boot test.
- **The fix had to cost nothing.** The extra compare made `timer_arm` 3 instructions slower,
  which the benchmark gate rejects. Arming divided a 128-bit product on every call
  (`__udivti3`); now `init` turns the measured TSC and APIC timer frequencies into one 32.32
  fixed-point factor, rounded up (`time::ratio_ceil`), and arming multiplies by it
  (`time::scale_ceil`): never below the exact conversion, so a deadline is still never early,
  and above it by at most 1 count plus 1 per 2^32 TSC ticks (about 4 µs late at the APIC
  timer's 172 s reach with a 25 MHz APIC clock and a 2.4 GHz TSC; longer deadlines re-arm
  anyway). Host tests check both bounds on measured and awkward clock pairs.
- **Benchmarks (x86_64).** `timer_arm` 65 → 33 instructions, `timer_interrupt_round_trip`
  95 → 81, and `breakpoint_round_trip` 81 → 73: without the division call, the trap handler,
  which inlines the timer interrupt's re-arm, became a leaf function and no longer saves and
  restores callee-saved registers on every exception. Nothing else moved beyond the gate.
- **Also fixed.** `thread::enable_preemption`'s comment said a slice starts with every switch;
  D39 has a thread that runs after a yield, block or exit finish the slice under way.

## D43 Per-CPU run queues: every CPU runs threads; one scheduler lock for now — Decided by the build loop (engineering); supersedes D30's report of a held heap lock
- **Why now.** D40 and D41 started the other CPUs and gave each its own timer and wake-up
  interrupt, but threads still ran on the boot CPU alone. This step makes every CPU schedule
  threads. Splitting the scheduler's one lock into per-CPU and per-wait-queue locks is the next,
  separate step: it changes how a thread blocks and is woken (two locks, taken in a fixed order),
  and is easier to review and measure on its own.
- **Per-CPU state.** Each CPU's scheduler state (running thread, ready threads, sleepers, idle
  thread, exited-thread slot, slice end, armed deadline) lives in its per-CPU block
  (`Cpu::scheduler`), reached only with the scheduler's lock held (`Schedulers::of`, which
  borrows the lock's one `Schedulers` mutably, as `WaitQueue` did with the old `Scheduler`).
  Thread ids come from a global counter, 0 reserved for the boot thread. A secondary CPU's start
  code becomes its idle thread once it has taken its timer interrupt, just before it reports in,
  so a CPU that is online can run threads.
- **Placement.** A thread stays on the CPU `spawn` places it on: woken there, sleeping on that
  CPU's timer, running there until it exits. `spawn` picks the lowest-numbered idle CPU (started,
  idle thread running, nothing ready), otherwise the next started CPU in turn, otherwise the
  spawning CPU (one CPU, or before the CPU table exists: the host tests and the benchmarks).
  O(CPUs) per spawn, which allocates a stack anyway. Moving threads between CPUs to balance them
  (stealing from busy CPUs when idle) is a follow-up.
- **Telling another CPU.** A thread made ready on another CPU is announced to it: an idle CPU
  gets the wake-up interrupt (D41), sent after the lock is released (`wake_all` sends them while
  holding it, having nowhere to keep the threads without allocating); a flag in its scheduler
  state (`wakeup_sent`) skips further interrupts until its idle thread looks again. A CPU whose
  scheduler has started but which has not reported in yet gets no interrupt: its idle thread
  looks after reporting in, before it first waits, under the same lock. A busy CPU whose running
  thread was alone with preemption on gets a reschedule request instead: the top
  bit of the per-CPU word its preemption point already loads (now `unseen_by_scheduler`: timer
  interrupts in the low 63 bits), set with one atomic OR, no interrupt. Its next preemption
  point, yield or block arms the tick, which is where an armed tick would have ended the slice
  anyway. The same rule applies on the CPU itself: making a thread ready no longer arms the timer
  directly, which made wait/wake cheaper.
- **Preemption.** `enable_preemption` and `disable_preemption` set the slice on every CPU; turning
  it on asks the other CPUs to look, so those with ready threads arm their ticks.
- **The heap's lock waits now.** On bare metal the heap panicked when its lock was held, since on
  one CPU only an interrupt handler allocating could find it so (D30). With threads on four CPUs
  two of them allocate at once, which the new self-test hit intermittently on x86_64; the heap's
  lock now waits like any other.
- **Keeping the hot paths as cheap.** Finding the per-CPU block costs an instruction a static
  did not on RISC-V (`mv` from `tp`) and forced a saved register on x86_64 where the block's
  address lived across a call. Recovered by: the timer slow path and `park` closures finding or
  receiving their CPU, so no caller keeps it across a call; `finish_switch` reading the
  exited-thread slot with one per-CPU load (the HAL's `cpu_local_u64` contract now also allows a
  word in an `UnsafeCell` that nothing writes meanwhile); `Stack::base` as a `NonZeroUsize`, so
  `Option<Stack>` is one word and the canary check one load shorter; `ThreadList::push_back`
  writing the thread's link after reading the list; and `core::hint::cold_path()` in the
  spin lock's contended loop, so the free case falls straight through (this removed a jump on
  RISC-V from every lock). Tried and dropped: a word-sized lock with `swap` or `fetch_or`
  (cheaper on RISC-V, dearer on AArch64 without LSE atomics) and a canary pointer per thread
  (slower on two architectures).
- **Testing.** Host unit tests: notices for an idle CPU (one wake-up per look), a busy CPU (a
  reschedule request only while preemption is on and its thread was alone), `make_ready` across
  CPUs, placement (idle first, then in turn over started CPUs, the spawner when alone), and a
  reschedule request seen once without counting a timer interrupt. The boot self-test
  `threads_run_on_every_cpu` places one thread on each secondary CPU and checks they all run at
  once, sleep on their own CPU's timer, are woken from afar out of a wait queue (each idle CPU
  takes the wake-up interrupt) and stay on their CPU, and that the boot CPU, blocked in `join`,
  is woken from afar when they exit. It fails when placement is broken (aarch64: the threads
  never run at once) and hangs, which the runner reports, when remote wake-ups are dropped
  (x86_64).
- **Benchmarks** (instructions per iteration, one CPU, this machine's QEMU, before → after):
  `thread_wait_wake_round_trip` aarch64 222 → 204, riscv64 226 → 207, x86_64 203 → 193;
  `thread_yield_round_trip` aarch64 132 → 121, riscv64 151 → 150, x86_64 111 → 110;
  `wait_queue_wake_empty` aarch64 16 → 15, riscv64 14 → 13, x86_64 16 → 14. Nothing slower.
- **Cost (D24).** About 100 bytes more per CPU block (the scheduler state, which was one static);
  no boot time.
- **Follow-ups (roadmap).** Per-CPU locks for the run queues and per-queue locks for wait queues;
  load balancing (an idle CPU taking ready threads from a busy one); an idle-CPU mask instead of
  scanning the CPU table in `spawn`; per-CPU idle stacks with canaries for the secondaries (their
  start stacks have none); preemption on a secondary CPU exercised by a boot self-test. (D41's
  follow-up of a secondary CPU waking the boot CPU is done: the self-test checks it.)

## D44 Load balancing: idle CPUs take ready threads from busy ones; a slice that ends with threads ready wakes an idle CPU — Decided by the build loop (engineering); refines D43's placement
- **Why now, and before the lock split.** D43 kept every thread on the CPU `spawn` placed it on,
  so a thread woken onto a busy CPU waited there for its turn while other CPUs sat idle. D43
  listed per-CPU locks first and balancing second; this step takes balancing first because, under
  the one scheduler lock, it is a contained change to the idle and slice-end paths that leaves the
  wait, wake and switch paths (and their benchmarks) alone. The lock split changes those paths
  and their costs, and needs measuring on its own; the balancing policy carries over to it, with
  the victim's lock taken instead of the one lock.
- **Pull.** When a CPU's idle thread looks and nothing is ready on its CPU, it takes the first
  ready thread of the next busy CPU after it (in index order, wrapping, so idle CPUs spread over
  the victims) and moves it: the thread's CPU becomes the idle CPU, where it runs, sleeps and is
  made ready from then on. A *busy* CPU runs a thread other than its idle thread; a CPU whose idle
  thread runs is left alone even with threads ready, since it was told of them (D43) and is about
  to run them itself, so balancing never fights a wake-up. The front thread is taken, the one that
  has waited longest. Moving a ready thread is safe under the one lock: a thread put in a ready
  list as its CPU switches away from it is still being saved, but the lock stays held until the
  save is done (it is handed over across the switch, D37), so whoever takes the lock next finds it
  saved; and a thread in a list is reached by nothing but that list.
- **Push.** A CPU whose slice ends at a timer event with threads ready (the only time its tick
  is armed, D39) sends the wake-up interrupt to the lowest-numbered idle CPU not already sent one
  since it last looked (`wakeup_sent`, D43), with the lock held as `wake_all` does; that CPU then
  pulls. So with preemption on, a thread waits a slice (10 ms) or a few while a CPU is idle: the
  woken CPU takes from the next busy CPU after it, not necessarily the one whose slice ended, so
  that one may need another slice end. This also happens when a slice ends at a yield or block.
  With preemption off, only the pull at an idle CPU's next look applies.
- **Pinned: the boot thread.** It is never moved: it runs the boot CPU's setup and the boot
  self-tests, which expect the boot CPU. A CPU whose first ready thread is the boot thread is
  passed over (the lists are singly linked, so taking from the middle would need a walk). So
  while the boot thread is the only thread waiting on a busy boot CPU, each slice end there
  wakes an idle CPU that finds nothing to take: one wasted wake-up interrupt per slice, rare and
  cheap. A general CPU-affinity setting is a follow-up, when processes need one.
- **Hot paths untouched.** Both halves live off the hot paths: the pull in the idle loop after
  its own ready list came up empty, the push in the timer's cold slow path, only when a slice
  ended, out of line (`#[cold]`, `#[inline(never)]`). Both are O(CPUs) scans of the CPU table;
  an idle-CPU mask can replace them when CPU counts grow. Every benchmark is unchanged on all
  three architectures.
- **Testing.** Host unit tests: the pull takes from the next busy CPU, leaves a CPU whose idle
  thread runs alone, passes over a boot thread at the front, moves the thread, and never takes
  from itself; the push picks the lowest-numbered idle CPU, once per look, never itself, never a
  CPU that has not started. Boot self-test `threads_move_to_idle_cpus` (four CPUs): a thread
  sleeps on CPU 1, a second thread is placed on that idle CPU and runs there at preemption
  points with preemption on, then the sleeper wakes onto the busy CPU; the two must end up
  running side by side on two CPUs within 1 s. It fails with stealing disabled (x86_64) and with
  the slice-end wake-up disabled (riscv64). It also exercises preemption on a secondary CPU, a
  D37 follow-up.
- **Cost (D24).** No memory, no boot time; code only on the idle and slice-end paths.
- **Follow-ups (roadmap).** Wake-time placement (waking a thread onto an idle CPU directly, when
  its own is busy and its cache is cold anyway); balancing with preemption off (a busy CPU's
  yield or block could wake an idle CPU too); CPU affinity; an idle-CPU mask; balancing by load
  and cache and NUMA distance, not just idle versus busy, once there are real workloads to
  measure.

## D45 User address spaces: a low half of their own, the kernel half shared; the scheduler's lock split waits for a workload that shows contention — Decided by the build loop (engineering; order decided by research); reorders D43's and D44's follow-ups
- **Why this step, before the lock split.** D43 and D44 listed per-CPU run-queue locks and
  per-queue wait locks next. A research subagent weighed that against starting processes and
  recommended processes first: the benchmarks run on one CPU under QEMU `-icount`, which runs
  every virtual CPU on one host thread, so they can show only the split's cost (a wait/wake round
  trip taking two locks per side instead of one, against a gate of 1% + 0.5 instructions on
  about 200) and never its benefit; no workload contends for the lock yet; and user mode is on
  the critical path to M6 (a static Linux hello world). seL4 still runs SMP under one kernel
  lock, and Zircon and Linux split theirs once workloads showed contention. The split waits for
  a measurable multi-CPU workload or a benchmark outside `-icount`; the intended design is a
  lock-free local run queue (kernel code runs with interrupts masked) with a per-CPU list for
  wake-ups from other CPUs, which could make a one-CPU wait/wake cheaper than today.
- **The address space.** `AddressSpace` (kernel) owns the tables of the low half, user space:
  addresses below the format's new `Format::USER_END` (AArch64 `TTBR0_EL1`'s whole 48-bit half,
  x86_64 the lower 128 TiB, RISC-V Sv39 the lower 256 GiB). The kernel half is shared: on AArch64
  the kernel's `TTBR1_EL1` root itself, with one root (RISC-V, x86_64) copies of the kernel root's
  upper entries, which point to the same tables. That is sound because the kernel's tables never
  change once active (`PageTables::map` refuses), so every kernel-half table exists from boot;
  sharing refuses tables not marked active, and `AddressSpace::new` is `unsafe`: its caller
  vouches that the tables are the kernel's own, active on every CPU, since switching loads roots
  whose kernel half is copied from them. Its tables (`PageTables::new_sharing_kernel_half`) map and unmap only user mappings, only in
  the low half (`MapError::NotUser` otherwise), so nothing reaches the shared kernel half through
  them; they unmap whole leaves (`NotMapped`, `PartOfLargePage`), and the address space frees its
  low half's tables when dropped (`free_user_half`, `unsafe`, refusing the kernel's tables); the
  frames user pages map belong to whoever mapped them.
- **The user bit.** `Attributes` gains `user`, with `USER_TEXT`, `USER_READ_ONLY` and
  `USER_READ_WRITE`. AArch64: `AP[1]` (EL0 access), not global (`nG`), and `PXN` always, so the
  kernel never executes a user page; `executable` means `UXN` clear for a user page and `PXN`
  clear for a kernel page. RISC-V: `U`, in leaves only (it is reserved in a pointer to a table).
  x86_64: `U/S` in the leaf and in every table entry on the walk to it (`Format::table` now takes
  whether the walk leads to user mappings), so kernel tables keep it clear at every level.
- **Switching.** `Platform::switch_address_space` replaces this CPU's low half and drops its
  cached user translations: AArch64 writes `TTBR0_EL1` and invalidates ASID 0's non-global
  entries (`TLBI ASIDE1`), keeping the kernel's global ones; RISC-V writes `satp` between two
  `sfence.vma`; x86_64 loads CR3, which drops everything (no page is global yet).
  `AddressSpace::with_active` runs code with the address space active on its CPU and switches
  back to the kernel's empty low half; it stops the kernel if the thread left its CPU meanwhile.
  Changing mappings takes `&mut`, so it never happens while the address space is active, and the
  switch both ways drops what was cached: no per-page TLB maintenance and no cross-CPU shootdown
  are needed yet.
- **Kernel access to user memory.** Only inside `Platform::with_user_access`: on RISC-V it sets
  `sstatus.SUM` for that closure alone (restoring what it was, so windows nest), so a stray kernel pointer into user space faults. PAN
  (AArch64) and SMAP/SMEP (x86_64) are not turned on yet (QEMU's `cortex-a72` and default x86 CPU
  lack them); when they are, they are lifted in the same place.
- **Testing.** Host unit tests: every format's user encodings and round trips; on every format,
  user tables reach the kernel's mappings, refuse kernel attributes and kernel-half addresses,
  and free exactly the frames they took, and the kernel's tables are never freed; a new address
  space shares the kernel half and maps nothing below; user-only and low-half-only mapping;
  unmapping (not mapped, part of a large page); dropping frees exactly the tables it allocated and
  none of the kernel's; `with_active` switches to the space's root and back. Boot self-test
  `address_spaces_keep_user_pages_apart` (aarch64, riscv64, x86_64): two address spaces map a
  frame each at one user address; with either active the kernel reads that space's frame there,
  a write lands in it, the kernel's low half stays empty, a remapped address shows the new frame,
  and dropping them frees every table. It fails on aarch64 without the `TLBI` (a switch keeps
  the first space's translation) and faults on riscv64 without `SUM`.
- **Cost (D24).** No hot path changes: the scheduler's machine code is unchanged (checked by
  disassembly on aarch64; only a string's address differs) and every benchmark matches `main` on
  this machine. Code only; no memory or boot time until an address space is made.
- **Follow-ups (roadmap).** Threads that carry an address space, switched on a context switch
  only when it differs (lazily for kernel threads); the first trip to user mode and back (EL0,
  U-mode, ring 3; `svc`, `ecall`, `syscall` with a GDT and TSS on x86_64) with a system-call
  round-trip benchmark; ASIDs and PCID; global kernel pages on x86_64; per-page TLB maintenance
  and cross-CPU shootdown once mappings change while active; PAN, SMAP and SMEP; user faults
  that end the process instead of stopping the kernel.

## D46 Review fixes to D45: x86_64 user space ends a page early; one address space active per CPU, checked — Decided by the build loop (review item)
- **Why.** A review of D43–D45 (a review subagent, then checked by hand) found no race or lock-order
  bug in the per-CPU scheduler or the address spaces, and two things worth fixing before user
  mode lands.
- **x86_64 user space ends one page below 2^47.** `Format::USER_END` was `1 << 47`, so a program
  could map the last canonical page and run a two-byte `syscall` at its end; `sysretq` would then
  return to `0x8000_0000_0000`, which is not canonical, and on Intel CPUs `sysretq` raises the #GP
  in ring 0 with the user's stack pointer already loaded: a known privilege-escalation pattern.
  With `USER_END = (1 << 47) - 4096` that page is never user space, so no user instruction runs
  there and no `sysretq` can return to the hole. AArch64 and RISC-V return with `eret`/`sret`
  and fault in user mode instead, so they keep their whole low half. `USER_END` is now
  documented as page-aligned and possibly short of the low half's end; the root entry that holds
  it is still the user's.
- **One address space active per CPU, checked.** `AddressSpace::with_active` must not block or
  yield; it already stopped the kernel if its thread came back on another CPU, but not if another
  thread made its own address space active on the same CPU meanwhile (the first would then switch
  that one away under it). Each CPU's block now records the first root of the address space active
  on it (`u64::MAX` for none); `with_active` refuses to start over another one and checks on the
  way out that its own is still recorded. Not on any hot path: the scheduler is unchanged.
- **Left for later (roadmap).** The general check that no preemption point is reached with a
  lock held or a window open (address space, or RISC-V's per-CPU `sstatus.SUM` for user access)
  stays a debug-build follow-up of D37.
- **Testing.** Host unit tests: the last page before the hole is refused (`NotUser`) and the one
  below it maps; a nested `with_active` panics before the second space becomes active. Boot and
  fatal-exception tests on aarch64, riscv64 and x86_64 and every benchmark unchanged.

## D47 User mode: the kernel runs a program until it traps ("run until trap"), Linux's system-call registers — Decided by the build loop (design decided by research)
- **The interface.** `Platform::run_user(&mut UserContext) -> UserExit` enters user mode (EL0,
  U-mode, ring 3) with the program's registers and returns once a system call, an interrupt or an
  exception takes the CPU back, with the registers saved in the same `UserContext` and the reason
  (`Syscall`, `Interrupt` already handed to the kernel's handler, or `Exception`). The kernel's
  side is plain Rust: run, handle the exit, run again. A research subagent compared this with the
  classic design (traps build a frame on the thread's kernel stack, handlers return into it) and
  recommended it: all architecture state stays in two short HAL routines, the user registers are a
  typed struct that signals, `ptrace`, NT's `GetThreadContext` and Darwin's `thread_get_state`
  will read and write directly, the ABI (Linux, NT, Darwin) is chosen by a Rust `match` per
  process rather than three entry paths, and preemption on return to user mode is just "switch
  threads before running it again" on an interrupt exit, the D37 model unchanged. Its cost is the
  kernel's callee-saved registers, saved and restored once per round trip (6 on x86_64, 12 on
  AArch64, 13 on RISC-V): measured, not guessed, below.
- **System-call registers.** Linux's on each architecture, ready for M6: the number in
  `x8`/`a7`/`rax`, six arguments in `x0`–`x5`/`a0`–`a5`/`rdi, rsi, rdx, r10, r8, r9`, the result
  in `x0`/`a0`/`rax`; every other register comes back unchanged (`syscall` takes `rcx` and `r11`).
- **AArch64.** The lower-EL vector entries (8–15) store straight into the context: while the
  program runs, `SP_EL1` points just past the context's saved part, and the kernel's stack
  pointer sits right there. `SPSR_EL1` is sanitized to the program's condition flags (EL0t,
  nothing masked), and a `dsb nsh; isb` follows the `eret` against straight-line speculation.
  `decode` and `irq` are now inlined into both callers, so the kernel's trap path keeps its length.
- **RISC-V.** While a program runs, `stvec` points at a separate user trap entry and `sscratch` at
  the context, so traps from the kernel pay nothing for user mode (the usual "`sscratch` is 0 in
  the kernel" check would have added two instructions to every kernel trap). The entry saves
  every register, the program's `tp` included, puts `stvec` back and returns with the kernel's
  `tp` (the per-CPU block) restored from the kernel stack. `sret` goes to U-mode with `SPIE`
  set; `sstatus.SUM` stays clear.
- **x86_64.** Each CPU now has its own GDT (kernel code and data at the boot GDT's selectors,
  user data 0x1b and user code 0x23 in the order `sysretq` needs, and a TSS) in the HAL part of
  its per-CPU block, loaded in `set_cpu_local` with `ltr`, and `syscall` set up (`IA32_STAR`,
  `IA32_LSTAR`, `IA32_FMASK` masking interrupts, trap, direction, alignment check and nested
  task). `TSS.RSP0` points into the running program's context, so an interrupt from ring 3 pushes
  its frame there and the entry pushes the rest after it; `syscall` builds the same frame. The
  common entry tells the two apart by the pushed CS (`testb $3`) and runs `swapgs` only from ring
  3. To pay for that test, a trap from the kernel now saves only the nine registers a call may
  change: breakpoint and timer-interrupt round trips got faster (73 → 59 and 81 → 71
  instructions). After a system call the program is entered with `sysretq`, otherwise with
  `iretq`; `run_user` refuses a `rip` outside user space before entering, as both would fault in
  ring 0, and sanitizes `rflags` (IOPL 0, interrupts on) and the selectors.
- **Test program.** Each architecture's HAL carries a tiny position-independent program
  (`Platform::user_test_program`) for the self-tests and benchmarks; the kernel copies it into a
  zeroed frame, makes it visible to instruction fetch (`Platform::make_instructions_visible`: data
  cache clean and instruction cache invalidate on AArch64, `fence.i` on RISC-V) and maps it
  `USER_TEXT` (`kernel::user::LoadedTestProgram`).
- **Testing.** Boot self-test `user_programs_make_system_calls_and_fault` on aarch64, riscv64 and
  x86_64: the program's six arguments arrive, the result comes back, every register the ABI keeps
  is unchanged across the call (it fails with one register restore removed on x86_64 and on
  riscv64), a read of kernel memory is a data fault at that address that the kernel survives, a
  timer interrupt takes the CPU back from a spinning program, and every frame is freed. Host
  tests: the program's mapping and freeing, and the benchmark loop on a test platform.
- **Cost (D24).** `syscall_round_trip`: about 106 (aarch64), 148 (riscv64) and 107 (x86_64)
  instructions per null system call, with the program's setup amortized in. No other benchmark
  moved except the x86_64 speedups above. 168 more bytes per CPU block on x86_64 (GDT, TSS, one
  scratch word); a test program of 396 (aarch64), 334 (riscv64) and 199 (x86_64) bytes in the
  kernel image.
- **Follow-ups (roadmap).** Processes that run their program in a loop and switch threads on an
  interrupt exit (preemption on return to user mode); on x86_64, IST stacks for NMI, double fault,
  machine check and debug exceptions, with the paranoid GS check, before any untrusted program
  runs, since `syscall` and `sysretq` leave a moment in ring 0 on the program's stack (nothing
  raises an NMI today); FP/SIMD state per thread; TLS registers; the Linux system calls (M6).

## D48 Processes: threads that run their program in a loop, address spaces loaded lazily per CPU, preempted on return to user mode — Decided by the build loop (design decided by research)
- **Processes.** `kernel::process::Process` owns a user address space and the frames its pages map
  (`map_new_page`: a zeroed frame of its own, made visible to instruction fetch when executable),
  frees them when dropped, and is shared as an `Arc` by its threads, which freezes its mappings.
  It replaces D47's `kernel::user::LoadedTestProgram` (`Process::with_test_program`). A process's
  thread (`process::spawn`) runs D47's loop: a preemption point, load the address space, `run_user`,
  hand the exit to the thread's handler (`ControlFlow`), which stands in for the system calls (M6)
  and signals, and may block, sleep or yield. A user thread ends when its handler says so.
- **Address spaces are loaded lazily, per CPU, at the point of use.** A research subagent compared
  (a) switching eagerly in the scheduler's `switch_to` whenever the next thread's space differs,
  (b) Linux's lazy TLB (kernel threads keep the last user space, the scheduler switches when a
  thread with another one runs) and (c) loading at the point of use with no scheduler change, and
  recommended (c): `AddressSpace::load_lazily` compares the CPU's recorded root (D46) with the
  space's (a load and a compare) and switches, out of line, only when they differ; the CPU then
  keeps the space loaded, and a type-erased reference to its owner (`Cpu::lazily_loaded`) so its
  tables stay allocated, until it needs another one. The thread loop loads before every `run_user`;
  `Process::with_user_memory` loads before every kernel access to the process's pages, so a
  handler that blocked while another process ran on its CPU still reaches its own. Reasons: the
  scheduler's switch, and so every kernel-thread switch, pays nothing (option (b) cost about 4
  instructions per `thread_yield_round_trip`, beyond the gate); on x86_64 without global pages or
  PCID, (a) would flush the TLB twice per hand-off to a kernel thread and back; it works unchanged
  when balancing (D44) moves a thread, since each CPU compares its own record; and freeing is
  simple: the owner's last reference is dropped off the scheduler's lock. Kernel threads running
  with a user space loaded is no new exposure: every system call runs the kernel so, and the kernel
  reaches user pages only through `with_user_access` (PAN, SMAP and SMEP stay follow-ups).
- **D46's check.** The per-CPU root record stays the single source of truth: it always equals
  what the CPU has loaded. `with_active` (self-tests, `syscall_round_trip`) unloads a lazily loaded
  space first and keeps its nesting and same-CPU checks.
- **Freeing.** A user thread that ends drops its reference and, if its CPU then holds the last one,
  unloads the space there, which frees the process. An idle CPU does the same at each look
  (`address_space::release_unused`: a load and a compare when nothing is loaded), for a process
  whose thread ended elsewhere.
- **Preemption on return to user mode.** Before each run the thread passes
  `thread::preemption_point_before_user_mode`, which repeats until the scheduler has seen every
  timer interrupt and reschedule request on the CPU. The plain preemption point was not enough: a
  thread resumed by a switch (after the previous one exited, or from a yield) may find a timer
  interrupt nobody has looked at, whose one-shot timer has fired, so nothing would re-arm it or
  wake the sleepers while the program spins (found by the new self-test, which hung on aarch64).
  A reschedule request to a CPU whose running thread is a user thread (`Thread::user`, set by
  `thread::spawn_user`) also sends it the wake-up interrupt: in user mode it would see the
  request only at its next system call or interrupt, if ever. That branch is reached only with
  preemption on, and is out of line.
- **Instruction caches across CPUs.** A program loaded on one CPU now runs on any: AArch64's
  `make_instructions_visible` invalidates every CPU's instruction cache (`ic ialluis`; the data
  cache clean by address reaches them already), and RISC-V's `switch_address_space` runs `fence.i`,
  which is the hart's own, so a hart that switches to a process's address space fetches its code
  as written. x86 keeps the caches coherent itself.
- **`Platform: 'static`.** Kernel objects that live on their own and are generic over the platform
  (a process its threads share, type-erased per CPU) need it; every platform is a type with no data.
- **Testing.** Boot self-tests on aarch64, riscv64 and x86_64: `processes_keep_their_own_memory_across_switches`
  (two processes map different data at the same address; each yields at every system call and then
  reads its page through `with_user_memory`; it fails with the load there removed),
  `processes_are_preempted_on_return_to_user_mode` (two programs that never make a system call take
  turns on one CPU, and end at an interrupt), `processes_run_on_every_cpu` (two processes per CPU,
  up to 16, on every CPU, with balancing; then every CPU lets the address spaces go), every frame
  freed after each. Host tests: lazy loading switches only when the root differs, the CPU keeps a
  dropped process until another loads or it idles, `with_active` over a lazily loaded space, a user
  thread's loop and handler, and the reschedule request to a CPU running a user thread.
- **Cost (D24).** New benchmarks, instructions per iteration: `process_syscall_round_trip` (the
  thread loop's null system call: preemption point, load check, handler) 120.30 (aarch64), 158.44
  (riscv64), 120.54 (x86_64), against `syscall_round_trip`'s 106.17, 148.27 and 107.44;
  `process_switch_round_trip` (two processes' threads yielding at each system call: two calls, two
  thread switches and two address-space switches) 482.55, 590.84 and 443.06. No existing benchmark
  moved: adding the branch to `make_ready` first stopped `WaitQueue::wake_one` from being inlined
  into its callers (+8 instructions on an empty wake), and `wake_one` is now `#[inline]`, which
  restores exactly the old numbers. The instruction counts do not show what an address-space switch
  costs afterwards in TLB refills (M13 measures that on hardware). About 48 more bytes per CPU
  block (the slot and its lock) and a flag per thread.
- **Follow-ups (roadmap).** Waking CPUs that keep an ended process's address space while they stay
  busy with kernel threads; TLB shootdown once a shared process's mappings may change (they are
  frozen today); preemption on whenever user programs run (with it off, as for kernel threads, a
  program that never traps keeps its CPU); pages owned by the address space for `mmap` (M6); ASIDs
  and PCID; PAN, SMAP and
  SMEP; user faults that end the process with a signal (M6); IST stacks and the other D47 follow-ups.

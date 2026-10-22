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

## D30 Kernel heap: power-of-two size classes in the direct map, one frame at most until the buddy allocator — Decided by the build loop (engineering; ordering, large allocations and locking decided by research)
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

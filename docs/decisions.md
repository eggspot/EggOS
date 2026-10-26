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

## D5 License: MIT, clean-room — Decided, permanent; the license superseded by D79 (MIT OR Apache-2.0)
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

## D13 Performance target: the fastest OS — Decided; the owner's acceptance of regressions superseded by D78
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
- **Freeing.** A CPU lets a process's space go once only CPUs still refer to it: its owner's
  reference count is no larger than the number of CPUs that record its root. No thread or other
  holder is then left, so none can load it again, and every CPU may drop its reference. A count
  that changes while the test reads it can make it wrong either way, harmlessly: a CPU only ever
  drops its own reference, which never frees memory another holder uses (the space is loaded
  again where it is needed), and a wrong "no" waits for the next look. A user thread that ends
  drops its reference, lets the space go on its CPU if the test holds, and sends the wake-up
  interrupt to every other CPU that has it loaded; an idle CPU makes the same test at each look
  (`address_space::release_unused`: a load and a compare when nothing is loaded), and a CPU that
  lets a space go when idle or for `with_active` wakes the others that keep it when only CPUs held
  it. A CPU that drops one by switching to another process's space wakes nobody: the look at every
  CPU cost about 25 instructions per switch (`process_switch_round_trip` +10%), so a CPU whose last
  look raced with such a switch keeps the ended space until something wakes it (follow-up). The first version freed only on the last
  reference, which leaked a process whose threads had run on two CPUs that both went idle with
  it loaded (found by the independent review; `processes_run_on_every_cpu` now covers it and fails
  with the old rule).
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
  turns on one CPU, and end at an interrupt), `processes_run_on_every_cpu` (one process with a thread per CPU,
  up to 16, so several CPUs have it loaded when it ends; then two threads each of a process per
  CPU, up to 8, with balancing; after each, every CPU lets the address spaces go), every frame
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
- **Follow-ups (roadmap).** Letting an ended process's address space go on CPUs that stay busy with
  kernel threads (woken, they look only when they idle), and on an idle CPU whose last look raced
  with another CPU switching away from it (a per-process thread count would make that check one
  load); TLB shootdown once a shared process's mappings may change (they are
  frozen today); preemption on whenever user programs run (with it off, as for kernel threads, a
  program that never traps keeps its CPU); pages owned by the address space for `mmap` (M6); ASIDs
  and PCID; PAN, SMAP and
  SMEP; user faults that end the process with a signal (M6); IST stacks and the other D47 follow-ups.

## D49 Review fixes to D47–D48: user programs get no FP/SIMD, vector unit or timer access, whatever firmware left; speculation mitigations chosen per CPU, with triggers — Decided by the build loop (review item; mitigation timing decided by research)
- **Why.** A review of D47–D48 (a review subagent, each finding then checked by hand) found no
  register leak, privilege bug or lifetime race in the user-mode entry and exit paths or the
  lazily loaded address spaces, and one isolation leak that was real: on x86_64, ring 3 could use
  the x87 and MMX registers, which no one saves per thread, so one process could read what
  another left there. The roadmap's "FP/SIMD traps today" was true on AArch64 and RISC-V under
  QEMU only.
- **x86_64.** Each CPU sets `CR0.TS` and `CR0.MP` and clears `CR0.EM` (in `set_up_user_mode`),
  so every x87, MMX and SSE instruction raises #NM (SSE raises #UD or #NM while `CR4.OSFXSR` is
  clear), and `CR0.NE` for native x87 error reporting later. TS is also how lazy per-thread FP
  state will work (`clts` on the first use). #NM is now decoded as an illegal instruction, as
  the other architectures report it.
- **AArch64.** Each CPU writes `CPACR_EL1 = 0` (FP/SIMD and SVE trap at EL0 and EL1; the kernel
  uses none) and `CNTKCTL_EL1 = 0` (EL0 may not read the counters or touch the timers, so a
  program cannot reprogram or stop the kernel's timer) at entry, instead of trusting reset
  values: QEMU resets both to 0, but UEFI or TF-A may leave FP open and `EL0VTEN` is UNKNOWN at
  reset. User reads of the virtual counter (`EL0VCTEN`) come with a fast clock for programs,
  deliberately. The SIMD/FP access trap (EC 0x07) is decoded as an illegal instruction.
- **RISC-V.** The boot code clears `sstatus.VS` along with `FS`, so vector instructions trap
  too where the hart has V (OpenSBI enables VS; QEMU's default `rv64` has no V, so CI cannot see
  the difference).
- **Straight-line speculation.** An `int3` after each x86_64 `iretq`, `sysretq` and the user
  path's `retq`, and `dsb nsh; isb` after AArch64's kernel `eret`, as after its user-mode one
  (D47): never executed, so they cost nothing at run time. The `ret`s rustc generates need `-Zharden-sls` (nightly only): a
  follow-up.
- **SWAPGS fence and KPTI: per CPU, deferred until their triggers (decided by research).** The
  kernel-entry `testb $3` branch has no `lfence` (CVE-2019-1125, SWAPGS), and the kernel half
  with its direct map is mapped in every user address space (Meltdown). A research subagent
  compared adding them now unconditionally, per CPU at boot, or later with a recorded trigger,
  and recommended per CPU, later: Linux, Windows and the BSDs enable both only on affected CPUs
  (Intel without `RDCL_NO`; AMD is not affected); unconditional KPTI would cost every machine a
  CR3 write each way (5–30% on system-call-heavy loads), against the performance target; and
  neither is exploitable today (the user GS base is always 0, and QEMU TCG does not leak). The
  SWAPGS fence must land, as a second copy of the entry stubs chosen at boot (a per-CPU flag
  would be one more branch to mispredict), **before a program can set its GS base or map pages
  near address 0** (`arch_prctl(ARCH_SET_GS)`, FSGSBASE, `mmap` at low addresses: M6, with an
  `mmap_min_addr`-style floor). KPTI, with PCID, must land **before x86_64 EggOS runs anything
  but our own test programs on a real Intel CPU** (KVM, HVF, WHPX or bare metal; M13 at the
  latest), together with the other transient-execution mitigations chosen per CPU (MDS buffer
  clearing, Retbleed) and KPTI for affected Arm cores (Cortex-A75).
- **Smaller fixes.** A program may set `rflags.ID` (old-style CPUID detection toggles it);
  alignment check stays clear, as interrupt gates keep it and with SMAP it would open user
  memory to the kernel. An executable page is made visible to instruction fetch whole, not
  only up to its contents, so fetch past them cannot see the frame's old data on AArch64.
- **Left for later (roadmap).** An NMI from ring 3 is reported as the program's exception and,
  since the user path returns with `retq`, leaves NMIs blocked until the next `iretq` on that
  CPU (nothing raises NMIs yet; it belongs with the IST stacks); AArch64 `PSTATE.DIT`, `SSBS`
  and `TCO` are not the program's to set yet.
- **Testing.** The boot self-test `user_programs_make_system_calls_and_fault` now also runs one
  floating-point instruction in user mode (`TestProgram::floating_point`: `fld1`, `fmov d0, xzr`,
  `fmv.d.x`) and requires an illegal-instruction exception at it, on aarch64, riscv64 and
  x86_64; without the CR0 change x86_64 runs on into its next system call and fails. No
  benchmark moved beyond noise (the user-mode ones by 0.02 instructions: a whole page cleaned
  once per program setup on AArch64); the `int3`s are never executed.

## D50 The Linux system-call layer starts: `write`, `exit`, `exit_group`, `ENOSYS`; user memory copied through the direct map after a table walk — Decided by the build loop (copy design decided by research)
- **The layer.** `kernel::linux` is the handler a Linux program's threads hand their exits to
  (D48's `ControlFlow` handler): `linux::spawn(process, context)` starts one. A system call is
  dispatched by Linux's number on the architecture (`linux::number`: x86_64's own table,
  AArch64 and RISC-V the generic one) and returns its result or `-errno` (`linux::Errno`) in
  the result register; an interrupt resumes the program; an exception ends the process with the
  signal Linux would send by default (`SIGILL`, `SIGTRAP`, `SIGBUS`, otherwise `SIGSEGV`; finer
  ones, such as `SIGFPE` for a divide error, come with signals). First calls: `write` to
  standard output and standard error (the console), `exit` and `exit_group` (one thread per
  process until `clone`, so both end the process; Linux keeps the status's low 8 bits).
  Every other number returns `ENOSYS`, as Linux does for a call it lacks, so a program that
  probes carries on.
- **How a process ends.** `Process::exit(ProcessExit)` records it, first one wins
  (`Exited(status)` or `Killed { signal }`), and `Process::exit_status` reads it: what `wait4`
  will return to the parent.
- **User memory (decided by research).** `Process::read_user_memory` and `write_user_memory`
  take an untrusted user address and length, check that it ends within user space, then walk
  the process's tables a page at a time, require a user page of normal memory (writable, to
  write), and copy through the kernel's direct map at the frame's address. A research subagent
  compared this with (b) the same walk then a copy at the user address inside the user-access
  window, and (c) Linux's way: copy at the user address with an exception-fixup table so a
  fault returns `EFAULT`. It recommended this one for now: it is correct today because a
  process's mappings are frozen while its threads run (D48) and it owns every frame they map,
  so nothing can change between the check and the copy and nothing can fault; (b) does the same
  walk and adds the window; (c) is the end state but needs per-architecture copy routines and
  fixups in every trap handler, and today saves only the walk. The copy also needs no address
  space loaded on this CPU and never opens the user-access window. **Trigger to switch to (c):**
  the first change that lets a mapping change or appear while threads run (`munmap`,
  `mprotect`, `mremap`, a shrinking `brk`, demand paging, copy-on-write); also if PAN/SMAP
  turn on (the window is then paid anyway) or a benchmark shows the walk dominating large
  copies. Only the inside of the two `Process` methods changes. Callers copy into kernel
  buffers and read each user value once.
- **`write` returns exactly what Linux returns.** It copies at most 256 bytes at a time
  through a stack buffer and never across a page, so when part of the buffer is not the
  program's memory it returns the bytes before that part, or `EFAULT` if there are none. A
  zero-length write returns 0 unchecked, a range that does not end within user space is
  `EFAULT` before anything is written (Linux's `access_ok`), and only the descriptor's low 32
  bits count (it is an `unsigned int`), as on Linux.
- **Cost (D24).** The dispatcher is inlined into the thread's loop and everything with a frame
  of its own (`write`, ending the process) is out of line: the first version kept `write`'s
  buffer and six saved registers in the shared frame and cost 35–50 instructions per call.
  New benchmark `linux_syscall_round_trip` (an unknown call, `ENOSYS`, the cheapest dispatch,
  from the HAL test program's new `linux_unknown_syscalls` loop): 134.34 (aarch64), 171.47
  (riscv64), 137.56 (x86_64) instructions per call, against `process_syscall_round_trip`'s
  120.30, 158.44 and 120.54. No other benchmark moved. The table is three compares today; a
  jump table when it grows. Two more test-program entries in the kernel image (about 100 bytes
  per architecture).
- **Testing.** Boot self-test `linux_programs_write_and_exit` on aarch64, riscv64 and x86_64:
  the HAL test program's `linux_hello` writes `EGGOS-USER: hello from user mode` with `write`,
  checks `EFAULT` for a buffer at address 0 and `ENOSYS` for call 1000, and ends with
  `exit_group(0)` only if all three are right; a program that runs a floating-point
  instruction is ended with `SIGILL`; every frame is freed. The QEMU runner now requires the
  line on the console, so the bytes are checked end to end. Host tests: copies across pages
  both ways and refusals (read-only, unmapped, kernel, past user space, wrapping), `write`'s
  chunks across pages and its partial count, bad descriptors, `ENOSYS`, both exits, and the
  signal for each exception kind.
- **Next (roadmap).** The rest of what a static hello-world binary needs (`brk`, `mmap`,
  `uname`, `writev`, `arch_prctl`/`set_tid_address`, ...), an ELF loader and a stack with
  `argv`, `envp` and the auxiliary vector, and a first program built from our own Rust source;
  the SWAPGS fence before a program can set its GS base or map low pages (D49).

## D51 Linux programs start from ELF images: a checked parser, segments mapped page by page, Linux's initial stack and auxiliary vector — Decided by the build loop (loader policy decided by research)
- **What.** `kernel::elf` parses an untrusted ELF-64 little-endian image without copying: the
  file header checked against the kernel's machine (`e_machine` 62, 183, 243), executables and
  position-independent files only, 56-byte program headers, at most a page of them (as Linux;
  this also refuses `PN_XNUM`), all within the image. `kernel::linux::exec::load(process,
  image, path, arguments, environment)` is what `execve` will do once it has the file's bytes:
  it checks everything first and maps nothing until every check passed, then maps each `LOAD`
  segment page by page (a frame of its own per page, holding the file's bytes for that page
  from the page's start, as a file mapping has them, and zero after the segment's file bytes,
  so `.bss` needs nothing more), maps the stack and lays it out, and returns the program's
  registers: its entry and stack pointer, every other register zero (C libraries take a
  non-zero `rdx` / `x0` / `a0` as an exit hook). `ExecError::errno` gives Linux's error.
- **Loader policy (decided by research).** A research subagent was asked about each choice;
  its recommendations, followed:
  - **W^X for programs.** A segment both writable and executable is refused (`ENOEXEC`), as a
    `PT_GNU_STACK` asking for an executable stack is: user pages are never both, as kernel
    pages are not. Modern linkers never emit either unless forced; what breaks is `ld -N`
    output, some very old or hand-made binaries and GCC nested-function trampolines. JITs need
    writable-then-executable memory through `mmap`/`mprotect` (dual mapping or a `MAP_JIT`-style
    flip), a separate item. A segment with no access is refused too (no `PROT_NONE` mappings
    yet). **Trigger to revisit:** a real binary we must run needs RWX.
  - **A floor.** No segment may start below `exec::MINIMUM_ADDRESS` = 0x10000 (`EPERM`, Linux's
    default `mmap_min_addr` and RISC-V linkers' lowest base): a null pointer always faults, and
    no program can map pages near 0 before D49's SWAPGS fence exists.
  - **Static PIE is supported**, the default of Rust's musl targets: a position-independent
    program is moved by a bias to `exec::pie_base(USER_END, align)`, two thirds of the way up
    user space, rounded down to 2 MiB or the largest segment alignment (a power of two up to
    1 GiB; anything else is refused): 0x5555_5540_0000 on x86_64, 0xaaaa_aaa0_0000 on
    AArch64, 0x2a_aaa0_0000 on RISC-V Sv39. The program relocates itself; `AT_BASE` is 0.
    Fixed until layout randomization comes with a random number generator. A program that
    names an interpreter (dynamically linked) is refused with `ENOENT`, what Linux returns when
    the interpreter is missing, until a file system can supply one (M7). Note: glibc's static
    PIE startup calls `mprotect` on its RELRO region and dies if that fails; musl's does not.
  - **Segments** must be in ascending order without sharing a page (eager per-page mapping
    cannot honour two attribute sets on one page), with file offset and address equal within a
    page and file bytes within the image (`ENOEXEC`; a segment without file bytes is zero
    pages alone and its offset is not checked, as on Linux), no more file bytes than memory,
    and within user space below the stack's reserve, before the PIE move as well as after, so
    the move cannot wrap around (`EINVAL`, as Linux); so must the entry point. **Trigger to
    revisit** the shared-page rule (Linux allows two segments on one page): a real binary
    built with a small maximum page size or a hand-written linker script needs it.
  - **`AT_PHDR`** is where the `LOAD` segment whose file bytes hold the whole program header
    table maps it, as Linux computes it; stricter than Linux, a table in no segment is refused
    (musl reads `AT_PHDR` unchecked), and so is a `PT_PHDR` that names another address (static
    PIEs find their base from it).
  - **The stack.** `exec::STACK_RESERVE` (8 MiB, Linux's default stack limit) below the end of
    user space is kept for the stack, and `STACK_GUARD_GAP` (1 MiB, Linux's) below it stays
    unmapped; no segment may reach either. Mapped up front: the arguments' pages and
    `STACK_SIZE` (128 KiB, what Linux guarantees at `execve`) below them; there is no demand
    paging yet to grow it. Linux's limits: argument and environment strings, pointers and the
    auxiliary vector together at most a quarter of the reserve (2 MiB), each string at most
    128 KiB with its NUL (`MAX_ARG_STRLEN`), else `E2BIG`. The layout is Linux's: from the 16-byte
    aligned stack pointer, `argc`, `argv` and a null, `envp` and a null, the auxiliary vector;
    above them the 16 `AT_RANDOM` bytes, the argument strings, the environment strings, the
    path (`AT_EXECFN`) and 8 zero bytes at the very top.
  - **The auxiliary vector**, in Linux's order: `AT_HWCAP`, `AT_PAGESZ`, `AT_CLKTCK` (100),
    `AT_PHDR`, `AT_PHENT`, `AT_PHNUM`, `AT_BASE` (0), `AT_FLAGS` (0), `AT_ENTRY`, `AT_UID`,
    `AT_EUID`, `AT_GID`, `AT_EGID` (all 0, equal, with `AT_SECURE` 0, or C libraries treat the
    program as privileged), `AT_SECURE`, `AT_RANDOM`, `AT_HWCAP2`, `AT_EXECFN`. Left out until
    they mean something: `AT_SYSINFO_EHDR` (no vDSO; C libraries fall back to system calls),
    `AT_MINSIGSTKSZ` (comes with signal frames), `AT_PLATFORM` (only the dynamic loader uses
    it). `AT_HWCAP`/`AT_HWCAP2` come from the new HAL call
    `Platform::linux_hardware_capabilities`: only what user mode really runs while FP/SIMD
    traps (D49): 0 on AArch64; I, M, A and C on RISC-V; `CPUID.1:EDX` without the FPU, MMX,
    FXSR, SSE and SSE2 bits on x86_64, and `AT_HWCAP2` 0 there (its `FSGSBASE` bit would invite
    programs to set their GS base, D49's trigger). Recompute all three when FP state is saved
    per thread.
- **`AT_RANDOM` is not random yet (decided by research): a recorded placeholder.** There is no
  random number generator, and RDRAND/RDSEED, RNDR, Zkr and virtio-rng each need feature
  detection, retry and quirk handling (some AMD parts return all-ones from RDRAND; Zkr needs
  firmware to open `mseccfg.SSEED`) and should feed a mixing generator rather than reach
  programs raw: a design of its own. Until then `exec::insecure_random_bytes` mixes the clock
  and a count of programs started (SplitMix64's output step), so the bytes at least differ
  between programs; its documentation says plainly that it is not secure. **Trigger:** before
  any program not built by us runs, it draws from the kernel's random number generator (our
  own ChaCha20 or BLAKE2s, seeded from the hardware sources above), which also serves
  `getrandom` (Rust's std calls it on first `HashMap` use) and layout randomization.
- **Cost (D24).** New benchmark `linux_exec_load`: a new process, the self-test's image (code,
  data and `.bss` across six pages, two arguments and an environment string) loaded, then
  freed: 112,859 (aarch64), 96,571 (riscv64), 60,529 (x86_64) instructions. Zeroing frames
  dominates: 33 for the stack, 6 for the program and the page tables. `map_new_page` now zeroes
  only the part of a frame its contents do not fill. Follow-ups that cut it: demand paging (the
  stack's pages only when touched), a faster `memset` on the kernel's targets, and frames
  zeroed ahead of time. Every other benchmark is unchanged (the frame benchmarks' +0.2–0.7%
  against the baseline, within the gate, is on `main` too). The HAL test program grows by
  about 100 bytes per architecture; the parser and loader add a few KiB to the kernel image.
- **Testing.** Boot self-test `linux_programs_start_from_elf_images` on aarch64, riscv64 and
  x86_64: an ELF executable built around the HAL test program (`selftest::linux_start_image`:
  headers and code in one read-only, executable segment with `PT_PHDR`, data at an unaligned
  address with `.bss` across three pages, a non-executable `PT_GNU_STACK`) is loaded with two
  arguments and one environment string; the kernel checks the data and that `.bss` is zero,
  then runs it as a Linux program. The test program's new `linux_start` entry reads its stack
  as Linux lays it out and writes every argument and environment string, which together are
  `EGGOS-USER: started from an ELF image with arguments and an environment`, then
  `exit_group(argc)` (255 for a misaligned stack); the self-test requires status 2 and the
  QEMU runner requires the line on the console. Host tests: the parser's checks and refusals;
  the loaded program's mappings and attributes, contents and `.bss`; the stack's layout,
  alignment, end marker, strings and every auxiliary vector entry, read back as a program reads
  it; a static PIE moved to the PIE base; the PIE base on all three formats; each refusal with
  its Linux error and nothing mapped; `E2BIG` for a long string and for too many; every frame
  freed.
- **From the independent review (non-blocking, on the roadmap's exec follow-ups).** Before
  `execve` takes outside input: a cap on a program's eagerly mapped pages (a hostile image's
  huge `p_memsz` would zero frames until memory runs out), and the initial stack written
  straight into its frames instead of through an infallible 2 MiB heap buffer; `CPUID` for
  `AT_HWCAP` read once at boot rather than per exec (a VM exit under virtualization).
- **Next (roadmap).** `brk`, `mmap`, `munmap`, `mprotect` (and with them D50's trigger to copy
  user memory with fault fixups), `uname`, `writev`, `set_tid_address`, `arch_prctl`, then what
  Rust's std calls at startup (`poll`/`ppoll` on descriptors 0–2, `rt_sigaction` for
  `SIGPIPE`, `sigaltstack`, `getrandom`); FP/SIMD state per thread, without which no compiled
  Rust or C program runs on AArch64 or x86_64 (their ABIs use FP registers); then a first
  program built from our own Rust source, embedded until there is a file system.

## D52 Demand paging: processes have regions, their pages are mapped when first touched, and address spaces gain mappings while their program runs — Decided by the build loop (design decided by research)
- **Why now.** The Linux memory calls (`brk`, `mmap`, `munmap`, `mprotect`, M6) need a
  process's memory to change while its program runs, and real programs reserve far more than
  they touch (Rust thread stacks, allocator arenas, `PROT_NONE` reservations): mapping every
  page up front would zero all of it and fail where Linux succeeds. This entry is the first
  part: regions, demand-zero pages, mappings added to a live address space. Removing and
  changing mappings (with the TLB work that needs) come with the calls themselves.
- **Regions** (`kernel::regions`). A process keeps sorted, disjoint `Region`s (start, end,
  attributes): Linux's mappings (VMAs). A region says what may be mapped, not what is. They
  live in one `Vec` searched by binary search, joined with a touching neighbour of the same
  attributes; research recommended this over a tree for the handful a static program has
  (revisit past about a thousand regions, as JVMs and glibc arenas reach, with a tree that
  tracks gap sizes for `mmap`'s search). `Process::add_region` adds one while the process is
  built.
- **Demand paging.** A page fault in a program (`ExceptionKind::DataFault` or
  `InstructionFault` with an address) goes to `Process::resolve_page_fault(address, access)`:
  if the address is in a region that allows the access, the page is mapped with a zeroed frame
  of its own (made visible to instruction fetch if executable) and the program runs the access
  again; anywhere else the Linux layer ends the process with `SIGSEGV`, and with `SIGKILL` when
  there was no frame (as Linux's out-of-memory killer would). Whether an exception is a page
  fault at all, and which access made it, comes from the new HAL call
  `Platform::page_fault(kind, code)`, which decodes the raw cause only when the kernel asks, so
  the trap path decodes nothing more (a field filled at every trap cost the kernel's breakpoint
  round trip an instruction): on AArch64 only translation, access-flag and permission faults
  (fault status `0b0001LL`–`0b0011LL`), with `WnR` without `CM` for a write; on RISC-V only
  `scause` 12, 13, 15 (access faults are PMP's, not page faults); on x86_64 `#PF` without the
  reserved-bit flag. Alignment faults, external aborts, TLB conflicts, unsupported atomics and
  access faults are not page faults and end the process as before (`SIGSEGV`; `SIGBUS` for
  some comes with signals).
- **Spurious faults.** A fault that may be stale (AArch64 translation and access-flag faults,
  every RISC-V page fault, x86 faults with no present entry) on a page already mapped for the
  access is a stale translation: a RISC-V hart without Svvptc may cache an invalid entry.
  Dropping this CPU's translation of the page (`Platform::drop_user_translations`, new) and
  running the access again is the whole fix. A permission fault on such a page is never
  retried: the entry the CPU used raised it, and retrying would fault for ever (from the
  independent review, which blocked an earlier version that retried every data abort,
  alignment faults included). The memory calls will make other CPUs drop an address space whose
  mappings changed in any way, so a permission increase never leaves a stale entry behind.
- **The stack** is now one region: `exec::STACK_RESERVE` (8 MiB) below the end of user space,
  with only the pages holding the arguments mapped at `execve` (written through the kernel's
  copy, which maps them); the rest is mapped as the program grows into it, and the guard gap
  below stays outside every region, so running off the stack is `SIGSEGV`. `exec::STACK_SIZE`
  (128 KiB mapped up front) is gone. Program segments are regions too, already mapped.
- **Kernel copies populate.** `Process::read_user_memory` / `write_user_memory` map an untouched
  page of a region that allows the access before copying, as the program's own access would:
  otherwise `write()` from a fresh stack buffer would be `EFAULT`. They hold the process's
  regions lock throughout, so a copy never meets a page being mapped (or, later, unmapped), and
  D50's direct-map copy stays sound; its trigger moves to `clone` (another thread of the
  process writing the same page during a copy).
- **Live address spaces gain mappings** (`AddressSpace::map` takes `&self`; its tables sit
  behind a lock, their roots copied out so loading the address space takes no lock). Sound
  without telling other CPUs: an added mapping replaces an invalid entry, which AArch64 and x86
  never cache, and a RISC-V hart that did takes one spurious fault. The frame's contents and a
  new table's zeros are ordered before the entry that makes them reachable by a release fence
  in `PageTables::map_leaf` (`dmb ish` on AArch64), since another CPU's walker is an observer
  like any other. Removing or changing a mapping still takes `&mut` (never while the address
  space is shared) until the memory calls bring TLB maintenance for other CPUs. Page-table
  frames are never freed before the address space is (research: another CPU may still walk a
  cached pointer to one, and an x86 walker may set Accessed bits in a reused frame).
- **Frames belong to the address space.** `AddressSpace::new_owning_frames` (a process's) frees
  the 4 KiB frames its user pages map in the same walk that frees its tables
  (`PageTables::free_user_half` hands each leaf to a callback); the process's `Vec` of frames
  is gone, so `munmap` can free a page's frame from the tables alone.
- **HAL.** `Platform::drop_user_translations(start, length)`: this CPU's cached translations of
  user pages, one page at a time up to `paging::USER_PAGES_DROPPED_ONE_BY_ONE` (32; Linux draws
  the line at 33 on x86) and all user translations beyond: `dsb ishst; tlbi vae1; dsb nsh; isb`
  or `tlbi aside1` on AArch64, `sfence.vma va, zero` or `sfence.vma` on RISC-V, `invlpg` or a
  CR3 reload on x86_64. `Platform::page_fault(kind, code)` as above, with `paging::Access` and
  `paging::PageFault` in the HAL. The test program gains
  `stack_growth`: from its stack pointer's page down, it stores each page's address in the
  page, until it runs off the stack.
- **Cost (D24).** New benchmark `linux_page_fault`: a store to a page touched for the first
  time, trap to return, amortized over 512 pages per process: 2,162 (aarch64), 2,202
  (riscv64), 1,159 (x86_64) instructions, dominated by zeroing the frame (the faster `memset`
  and pre-zeroed frames on the roadmap cut it). `linux_exec_load` drops by 32–44% (the stack's
  32 untouched pages are no longer zeroed): 62,884 / 57,413 / 41,297. Faster too:
  `process_syscall_round_trip` (−2.4% on aarch64 and x86_64: the address space is no longer an
  `Option`), `linux_syscall_round_trip` (−2.9% aarch64, −1.4% x86_64, −0.8% riscv64) and
  `process_switch_round_trip` (−1.8% to −2.7%: the CPU's slot stays locked across the switch,
  so the compiler sees it empty and drops nothing). Two regressions found on the way and fixed
  before the PR: a runtime-length slice of the roots (bounds checks and spills, +14–24% on the
  process switch) and passing the whole exception to the Linux layer's fault path (+4
  instructions per system call on riscv64; the parts are passed alone, out of line and cold).
  Every other benchmark is unchanged. The host test platform's RAM grows from 512 KiB to
  16 MiB, enough for the stack self-test.
- **Testing.** Boot self-test `linux_stacks_grow_on_demand` on aarch64, riscv64 and x86_64: the
  self-test's ELF image started at `stack_growth` stores to all 2,048 pages of the stack's
  reserve, each a page fault resolved with a zeroed frame, and is killed with `SIGSEGV` at the
  guard gap; every page must hold its own address, nothing below the reserve may be mapped,
  and every frame must be freed. Host tests: regions (sorting, joining, refusals), faults that
  map a zeroed page once and drop the stale translation, faults the regions refuse (outside,
  write to read-only, fetch from data, write to code, no address, not a fault) with nothing
  mapped, kernel copies that populate untouched pages, the exec stack as one region, and the
  benchmark's run and frees.
- **Next.** `brk`, `mmap`, `munmap`, `mprotect` on these regions (research's answers recorded
  for that PR: `PROT_WRITE|PROT_EXEC` refused with `EACCES` as Linux's `PR_SET_MDWE` and
  SELinux `execmem` do, allowing the flip from writable to executable; `PROT_NONE` pages that
  hold data keep their frame in an invalid entry; top-down `mmap` placement below the stack;
  the first break at the page-aligned end of the highest segment), with them TLB maintenance
  for other CPUs when a mapping is removed or reduced (CPUs that keep the address space loaded
  told to drop it before they run it again) and fault-around or large pages only after
  measuring. From the independent review, non-blocking: `AddressSpace::unmap` now refuses an
  address space that owns its frames (the frame would leak; the owner unmaps, M6); table entries
  written with an atomic release store instead of a fence and a volatile write (same code, says
  what is meant); kernel copies over untouched memory hold the regions lock for the whole copy
  (chunk it, or drop the lock between pages, once `clone` gives a process threads that fault
  meanwhile); kernel reads of untouched pages allocate zeroed frames where Linux maps its shared
  zero page; the HAL test program's code page has no region yet (a stale fault there on RISC-V
  would end the test).

## D53 The Linux memory calls: `brk`, `mmap`, `munmap`, `mprotect` on the regions; changed mappings make CPUs that keep the address space loaded switch again — Decided by the build loop (design decided by research)
- **Why now.** D52's "next": every C library and Rust's std allocate with `brk` or `mmap` at
  startup, and loaders and JITs re-protect their pages with `mprotect`. These are the first
  calls that remove and reduce mappings of a live address space, so they bring the TLB work
  D45 and D52 left for them.
- **The calls** (`kernel::linux::memory`), anonymous mappings only until there are files
  (M7). Arguments are checked as Linux's man pages and observed behaviour say, with Linux's
  error numbers (an unaligned
  offset `EINVAL` first; a map type other than shared, private or shared-validate `EINVAL`;
  unknown flags `EOPNOTSUPP` only with `MAP_SHARED_VALIDATE`, ignored otherwise; length 0
  `EINVAL`; a length that wraps when rounded up `ENOMEM`; a file mapping `EBADF`;
  `MAP_HUGETLB` `ENOMEM`, as on a Linux with no huge-page pool; writable and executable at
  once `EACCES`, D52's research; `MAP_FIXED` unaligned `EINVAL`, past user space `ENOMEM`,
  below `MINIMUM_ADDRESS` `EPERM`; `MAP_FIXED_NOREPLACE` over a mapping `EEXIST`). `mmap`
  places top-down from below the stack's reserve and its 1 MiB guard gap, uses a free hint
  (rounded up to a page), and `MAP_32BIT` (x86_64 only) in the second gigabyte. `MAP_SHARED`
  anonymous memory behaves as private while no process can share it (`fork`: the region
  needs a "shared" mark then, so `fork` shares instead of copying, from research). `munmap`
  of a range with nothing mapped succeeds; unaligned, empty or past user space is `EINVAL`.
  `mprotect` of a range with a hole changes what lies before the hole, then `ENOMEM`, as
  Linux does (research); length 0 succeeds. `brk` returns the new break, or the current one
  when the move is refused (below the start, past the `mmap` area, into a mapping); the break
  starts at the page-aligned end of the last segment (`exec::load`). AArch64's `PROT_BTI` is
  accepted and not enforced, as on a CPU without BTI, so a BTI-marked static glibc starts.
- **Regions** gain `remove` (splitting the regions a range cuts), `protect` and `free_below`
  (the top-down gap search, a linear walk over the sorted `Vec`; D52's gap-tracking tree past
  about a thousand regions still stands). A region's attributes are an `Option`: `None` is a
  `PROT_NONE` region the program may not touch.
- **`PROT_NONE` pages that hold data keep their frame in an invalid entry** (research,
  confirming D52's answer): `Format::kept(phys)` writes the frame with the valid bit clear and a
  software mark (x86 bit 9, AArch64 bit 55, RISC-V RSW bit 8), which the hardware ignores and
  never caches, and `decode` returns `Entry::Kept(phys)`. Translations, kernel copies and
  faults see nothing there; the frame is freed with the tables or by `munmap`, and `mprotect`
  back rewrites it as a leaf. Research rejected the alternative, a kernel-only leaf: on
  AArch64 a leaf without the user bit is global (no `nG`), so it would survive the
  address-space switch's `tlbi aside1` into the next process, and PAN would not catch stray
  kernel accesses to it. The same encoding is the groundwork for swap and file-backed entries.
- **Changing a live address space** (`AddressSpace::change_pages`, over
  `PageTables::change_user_pages`, which skips unmapped stretches a table at a time): the
  entries are rewritten under the tables' lock, frames unmapped are handed back after their
  entry is cleared, this CPU's translations of the range are dropped if it has the address
  space loaded, then (after a `SeqCst` fence) every other CPU that keeps it loaded is marked
  stale: a compare-exchange of its `active_address_space` record from the root to the root
  with bit 0 (`cpu::STALE`) set. Its next `load_lazily` finds the record different and
  switches, dropping every user translation, before the program runs. Zero cost on the
  system-call path (the check was already a load and a compare) and no interrupt. Sound
  because a process has one thread, the one making the call, so no other CPU uses the address
  space meanwhile, and the thread reaches another CPU only through the scheduler's locks, after
  the mark (research, checked for AArch64, RISC-V including harts that cache invalid entries,
  and x86_64; A/D and access-flag updates write table entries, never data frames, and table
  frames are never freed before the address space). The invariant: a CPU stores a root without
  the mark only along with a full switch. Code that counts or wakes the CPUs holding an address
  space masks the bit (`Cpu::holds`). Increases are marked too: a stale entry with less access
  raises a permission fault that is never retried (D52). A frame freed while a lazily-holding
  CPU may still cache it is reachable there only speculatively from kernel code, the same
  exposure other kernels accept for lazy TLB; PAN and SMAP will narrow it (roadmap). **Before
  `clone`**, or anything else that uses an address space on two CPUs at once (`with_active` or
  `with_user_memory` from another CPU), these changes need a shootdown: an interrupt to every
  CPU running the address space, waiting for each to drop the range.
- **Cost (D24).** New benchmark `linux_mmap_munmap`, 16 untouched pages mapped and unmapped
  with the address space active: 378 (aarch64), 410 (riscv64), 407 (x86_64) instructions. On
  the way, three regressions found and fixed before the PR: the bigger dispatch made LLVM stop
  inlining the Linux handler into the thread's loop (+11–15% on `linux_syscall_round_trip`;
  now `#[inline(always)]`, with the memory calls behind one out-of-line function and one
  compare against the highest call number first, which makes it 3.4–4.1% faster than before);
  the kept-entry test in every decode of an empty entry (+10% on x86_64 `linux_exec_load`:
  freeing tables now tests raw entries for zero, four a step, and placing a leaf tests the raw
  entry); and the fault path's extra work. Net, against `main`: `linux_exec_load` −20.4%
  (aarch64), −31.9% (riscv64), −30.5% (x86_64), freeing mostly empty tables being most of it;
  `linux_page_fault` −1.1%, −0.2%, −3.3%, because a fault that may be a first touch maps
  first and walks for an existing entry only if the map finds one (one walk, not two); every
  other benchmark unchanged.
- **Testing.** Boot self-test `linux_memory_calls_take_effect_at_once` on aarch64, riscv64 and
  x86_64: a program walks down an `mmap`ed range storing to every page, then the kernel (in the
  program's thread, between runs) unmaps the upper half, maps it again (zeroed) and makes the
  range read-only, then writable again; each walk must stop at the top page while it is
  unmapped or read-only, cover the range when writable, and every frame must be freed. It fails
  on aarch64 and x86_64 with this CPU's translation drop removed. Host tests: the regions'
  remove, protect and gap search; `mmap` placement, hints, fixed and no-replace mappings and
  every argument check; `munmap` splitting and freeing; `mprotect` with kept pages, the
  writable-to-executable flip, `EACCES` and the hole; `brk` growing, shrinking and refusing;
  kept entries round-tripping in every format; changed pages and stale marks.
- **Next.** The rest of a static program's startup calls (`uname`, `writev`,
  `set_tid_address`, `arch_prctl`, `rt_sigprocmask`, ...), FP/SIMD state per thread, and a
  first program built from our own Rust source; the shootdown with `clone`; `madvise`
  (`MADV_DONTNEED` zeroes private pages), `mremap` and `MAP_POPULATE` when a program needs them.

## D54 FP/SIMD registers per thread: given on first use, saved on every switch away, loaded again only when the CPU no longer holds them; the RISC-V kernel builds soft-float — Decided by the build loop (save/restore design decided by research; first-use allocation and the switch-path check chosen by the build loop; their cost accepted by research under D78)
- **Why now.** M6's next step: every compiled program uses FP/SIMD within its first
  instructions (SSE2 is part of the x86_64 ABI, NEON of AArch64's, F and D of RV64GC's, and
  libc's and Rust's `memcpy` use them), and since D49 they trapped and ended the program.
- **When a thread gets them: on first use.** Each CPU leaves FP/SIMD off for user mode
  (x86_64 `CR0.TS`, AArch64 `CPACR_EL1.FPEN = 0b01`, RISC-V `sstatus.FS = Off`), so a
  program's first FP/SIMD instruction traps as an illegal instruction. The thread's handler
  calls `process::first_fp_use` (the Linux layer does, in its cold exception path), which
  allocates an `FpState` (64-byte aligned: 512 bytes of registers on x86_64, 528 on
  AArch64 and 264 on RISC-V, in blocks of 512, 576 and 320) holding what Linux starts a
  program with (all zero; x87 control word `0x037F`, `MXCSR` `0x1F80`; `FPCR` and `fcsr` 0),
  loads it, turns FP/SIMD on and resumes the program
  at the same instruction. A second illegal instruction is a real one (`SIGILL`). Kernel
  threads, and programs that never use FP/SIMD, have no state and cost nothing. The research
  recommended giving every user thread its state at creation (every real program traps at
  once anyway); the build loop chose first use so that a thread that never uses them pays no
  memory and the existing process-switch path stays as it was for them, at the cost of one
  trap per thread lifetime.
- **Switching (research): save eagerly, load lazily.** A switch away from a thread that has
  FP/SIMD registers saves them (`FXSAVE64`; `stp q` pairs and `FPCR`/`FPSR`; `fsd` and
  `fcsr`, skipped on RISC-V unless `sstatus.FS` is Dirty), so balancing can move it to any
  CPU (D44). A switch to one loads them unless this CPU still holds them: its last owner was
  this thread (thread ids are never reused) and the thread has not loaded them on another CPU
  since. A switch to a program that has none, while FP/SIMD is on, first loads the initial
  values over the registers if they hold a program's, then turns FP/SIMD off: off alone would
  leave another program's values where some CPUs read them speculatively. That clearing runs
  only when a program that never used FP/SIMD follows one that did, out of line, from one
  read-only initial state (512, 576 and 320 bytes of kernel data). A program
  never runs with registers another left, not even with FP/SIMD off (no LazyFP,
  CVE-2018-3665: Linux, FreeBSD and macOS all switched to eager saving for it). On AMD CPUs that leave the x87 error pointers alone in
  `FXRSTOR` (CVE-2006-1056), an x87 load before every load resets them to the kernel's.
- **The switch-path cost: accepted by research (D78, 2026-10-23).** Every switch now looks at
  one per-CPU flag (does the running thread have FP/SIMD registers?) and the next thread's
  `user` flag, both on cache lines the switch reads anyway, and goes to an out-of-line copy of
  the switch only when FP/SIMD work may be needed. In QEMU's instruction counts that is about
  five instructions per switch: `thread_yield_round_trip` +8.0 to +10.9%,
  `thread_wait_wake_round_trip` +6.7 to +7.2%, `process_switch_round_trip` +3.2 to +4.4% (two
  more loads for a user thread). The system-call paths are unchanged; x86_64's
  `linux_page_fault` measures 1137.57 against `main`'s 1135.70 on the same machine (+0.16%,
  about two instructions, with no change on the fault path: the first-use check sits in
  `signal_fault`, which page faults reach only when they are not resolved). Alternatives
  rejected: trapping on every switch back (lazy restore) leaves the previous owner's values in
  the registers, the LazyFP leak; folding the flag into `finish_switch`'s existing exited-slot
  check would leave the benchmarks flat but only because they run with no FP/SIMD owner on the
  CPU, while every real workload would still pay; one combined flag byte saves no instruction
  on AArch64 and RISC-V (still two loads, an or and a branch) and one on x86_64. The research
  found one further cut: test only the outgoing thread's flag on the switch, and load or clear
  the incoming program's registers on its way back to user mode, behind the address space's
  existing compare (Linux's shape, with a pending bit in the same word as the active address
  space so system calls keep one compare), on the condition that the system-call paths stay
  flat. Built and measured, it cut `thread_yield_round_trip` to +3.3 to +5.5% and
  `thread_wait_wake_round_trip` to +2.4 to +3.6%, but made `process_switch_round_trip` +11.6
  to +15.1%, `fp_process_switch_round_trip` +6 to +7% and x86_64's `linux_syscall_round_trip`
  +1.6%, so by the research's own condition it was dropped. What remains is the price of
  eager saving, which LazyFP requires: Linux tests every outgoing task's FP state on every
  switch, kernel threads included, and FreeBSD (FreeBSD-SA-18:07), Windows (ADV180016) and
  macOS moved to eager handling for the same CVE. The baseline lines of these nine
  benchmarks and x86_64's `linux_page_fault` were updated to the numbers above, no others.
- **The kernel stays FP-free on every architecture.** AArch64 and x86_64 were already
  soft-float targets. RISC-V moves from `riscv64gc-unknown-none-elf` (hard-float ABI, which
  may use FP registers in kernel code, which would now silently corrupt a program's) to
  `riscv64imac-unknown-none-elf`; the save and restore routines and the test program enable D
  locally (`.option arch, +d`). AArch64 keeps EL1 untrapped (`FPEN 0b01` traps EL0 only), so
  the kernel saves and loads without turning anything on; x86_64 and RISC-V turn the unit on
  first.
- **What programs are told.** `AT_HWCAP` now reports FP and ASIMD on AArch64 (from
  `ID_AA64PFR0_EL1`), F and D on RISC-V (RV64GC, which every RISC-V Linux distribution
  requires), and the full `CPUID.1:EDX` on x86_64. x86_64 sets `CR4.OSFXSR` and
  `OSXMMEXCPT`; `OSXSAVE` stays clear, so `CPUID` tells programs not to use AVX.
- **Tests.** QEMU self-test `processes_keep_their_fp_registers_across_switches`: two programs
  on one CPU (and later two per CPU on every CPU, moving between them) each check that their
  registers start at zero though another program's are loaded, fill every FP/SIMD register
  (both halves of the 128-bit ones) and the rounding mode, and check them after each of 20
  yields. Without the switch's save and load it fails. The Linux self-test's `SIGILL` case now
  runs a really undefined instruction (`udf`, `unimp`, `ud2`), which first gets FP/SIMD
  registers and is then killed. Host tests cover the first-use path's calls and the initial
  state's layout. New benchmark `fp_process_switch_round_trip` (two FP programs switching,
  with their checks).
- **Follow-ups.** AVX and AVX-512 through `XSAVE`/`XSAVEOPT` (sized from CPUID leaf 0xD,
  `XCR0` set), then SVE/SME on AArch64 and V on RISC-V, each with a first-use trap of its own
  as Linux does for the large states; FP/SIMD state in signal frames (with `MXCSR` checked
  against `MXCSR_MASK` on `sigreturn`); `fork` and `clone` copying or starting it; #XM and #MF
  (x87 and SSE exceptions a program unmasks), already `SIGFPE`, given their own `FPE_*`
  codes rather than `FPE_INTDIV`;
  skipping the AMD x87 pointer reset on CPUs with CPUID 0x80000008:EBX[2]; `execve` in a
  running thread dropping or resetting the state; `SIGKILL`, not `SIGILL`, when there is no
  memory for the state; skipping the save on a thread's last switch, when it exits; a
  per-architecture "has FP" check read at boot before `start_fp` and `AT_HWCAP` rely on it (a
  RISC-V hart without F and D, or an AArch64 core without FP, would otherwise trap in the
  kernel's load; QEMU's machines all have them); 64-byte alignment for x86_64 only (it pads
  the other states); host tests of every `CpuFp::switch` transition, and `MXCSR` and x87
  checks in the x86_64 test program. Programs run
  directly from kernel code (`run_user` in self-tests and benchmarks) get whatever FP/SIMD
  state the CPU has: only our own test programs do that.

## D55 Linux startup calls, first part: `writev`, `getpid`, `getppid`, `gettid`, `uname`, `set_tid_address`, `set_robust_list`; processes get ids — Decided by the build loop (`uname`'s values decided by research)
(D54, FP/SIMD state per thread, is taken by PR #47, which waits for the owner's OK on its
switch-path cost; this entry follows it.)
- **Why now.** M6's next step: what a static C or Rust program calls at startup before
  `main`. musl and glibc call `set_tid_address` and `set_robust_list` first thing, stdio
  flushes with `writev` (its buffer and the new data in one call), and libraries and build
  scripts ask `uname` what they run on. These need no FP/SIMD state, so they do not wait for
  PR #47. The rest of the startup calls (`arch_prctl` and the thread pointer, which belong in
  the switch path with FP/SIMD; signals; `getrandom` with the random number generator;
  `prlimit64`, `poll`, `ioctl`) come next.
- **Processes get ids** (`process::ProcessId`): from 1 up, from one atomic counter, never
  reused, at most `i32::MAX` (a Linux `pid_t` is positive); `Process::new` fails with "out of
  process ids" past that. Reuse waits for a process table with `wait4`. A process has one
  thread until `clone`, and its first thread's id is the process's, as on Linux, so `gettid`
  returns the process id. `getppid` returns 0, as for a process the kernel started itself
  (Linux's `init`), until `fork` and `clone` give processes parents.
- **`set_tid_address` and `set_robust_list` remember nothing yet.** Linux keeps both
  addresses to act at thread exit (clear the word and wake a futex there; release the robust
  futexes) for the process's other threads and processes sharing its memory. A process has
  neither yet, and its memory ends with it, so nothing could observe those writes.
  `set_tid_address` returns the thread id; `set_robust_list` returns `EINVAL` unless the
  length is 24 (`struct robust_list_head` on 64-bit), as Linux does. Both start storing their
  addresses with `clone`.
- **`writev`** (to standard output and standard error, the console, as `write`): `EBADF`
  first; more than 1024 buffers (`IOV_MAX`) `EINVAL`; the `struct iovec` array copied into
  the kernel once (on the stack for up to 8 entries, C libraries pass 2, on the heap above;
  `ENOMEM` if that fails), so the program changing it meanwhile changes nothing; an
  unreadable array `EFAULT`; then every length checked (negative as a `ssize_t`: `EINVAL`)
  and every buffer checked to end within user space (`EFAULT`), before anything is written,
  as Linux's `access_ok`; then the buffers in order, at most `MAX_RW_COUNT` bytes in all,
  stopping at the first byte that is not the program's memory (the bytes before, or
  `EFAULT` for none). `write` and `writev` share one copy loop (page-bounded chunks, a
  preemption point after each).
- **`uname`** (research: gVisor, WSL1 and FreeBSD's Linux layer all do the same): `sysname`
  "Linux", so build scripts and libraries that look at `uname -s` take their Linux path;
  `release` "6.1.0-eggos", a long-term release above every C library's minimum kernel on
  every architecture (loongarch64 needs 5.19), low enough to limit what programs assume, with
  EggOS's suffix; `version` "#1 SMP EggOS <workspace version>" (no build date, for
  reproducible builds; "PREEMPT" only once the kernel itself is preemptible); `machine`
  Linux's name for the architecture; `nodename` "localhost" until `sethostname` (Linux's
  "(none)" breaks local host lookups); `domainname` "(none)". The structure is built at
  compile time and copied out in one call. Consequence: some programs pick features by kernel
  version instead of trying them (Go, the JDK, systemd, Docker), so every 6.1-era call we
  have not written must keep returning `ENOSYS`, never a wrong result; the release is raised
  only by a recorded decision; `/proc/sys/kernel/osrelease` and `/proc/version` will report
  the same when procfs exists.
- **Dispatch cost.** Every new call goes behind one out-of-line function (`out_of_line_call`), so
  the thread loop's inlined dispatch keeps three arms and its `number > LAST` compare.
  Growing it still tipped LLVM into calling `linux::syscall` instead of inlining it (+8 to
  +10% on `linux_syscall_round_trip`, found by diffing the loop's disassembly); `syscall` is
  now `#[inline(always)]`, as `handle` was, and every benchmark is back within the gate.
- **Tests.** Host tests: `writev` in order, empty buffers, more than the stack holds, every
  error before anything is written, partial writes; the identity calls through the
  dispatcher, two processes with different ids, `uname` into writable memory and `EFAULT`
  into read-only memory or across into unmapped memory; the `utsname` layout. QEMU, on aarch64, riscv64
  and x86_64: the ELF-started test program now checks `gettid == getpid` and writes its
  arguments and environment with `writev` from an iovec on its stack, so both run with each
  architecture's real numbers and the runner checks the line reached the console.
- **Follow-ups:** a `writev` benchmark once a sink faster than the console's UART exists
  (pipes, files: M7); `getpid`'s round trip as the cheapest real call, with the next batch.
  From PR #48's independent review, to confirm by observation on Linux: a buffer count with
  bits above the low 32 (Linux may truncate it, returning 0 for `1 << 32` where we return
  `EINVAL`), and a single oversized iovec running past user space (Linux may cap it before
  its range check). Self-test processes take ids, so the first real program is not pid 1:
  `init` gets pid 1 when it exists (M10).

## D56 The kernel's random number generator: BLAKE2s input, ChaCha20 output with fast key erasure and per-CPU keys; `AT_RANDOM` and `getrandom` draw from it; frames zeroed a line at a time — Decided by the build loop (design and the benchmark offset decided by research)
- **Why now.** D51's `AT_RANDOM` was a placeholder that had to go "before any program not
  built by us runs": C libraries take their stack-protector canary and pointer guard from it.
  `getrandom` is one of the startup calls left after D55 (Rust's std calls it with
  `GRND_INSECURE` on first `HashMap` use). It is chosen ahead of `arch_prctl` and the thread
  pointer, which D55 places in the thread switch next to FP/SIMD (PR #47, waiting for the
  owner); the generator touches no switch path.
- **Construction** (research: Linux since 5.17/5.18, Windows 10's per-processor generators,
  Bernstein's fast-key-erasure note), all our own code from the RFCs, checked against their
  test vectors (RFC 8439 sections 2.1.1, 2.3.2 and appendix A.1; RFC 7693 appendix B and
  the reference keyed vector):
  - **Input:** one BLAKE2s-256 state; each piece of seed material is hashed in after its
    source's tag and its length, with a count of credited bits. Hashing, not XOR, so a weak
    or hostile source cannot cancel the others.
  - **Seeding** (`random::reseed`): `extracted = BLAKE2s(input)`, fed back into the next
    input; `base = BLAKE2s(key: extracted, message: base)`; the base key's generation moves
    on. Seeded at 256 credited bits, as Linux; seeded stays seeded.
  - **Per-CPU keys** in each CPU's block, derived from the base key when its generation
    moved on (one ChaCha20 block: next base key, CPU key), so the base key's lock is taken
    once per CPU per generation, and only the CPU itself takes its key's lock.
  - **Each draw:** one ChaCha20 block of the CPU key (nonce 0, counter 0); bytes 0–31
    replace the key, 32–63 are the output. Up to 32 bytes (`AT_RANDOM`'s 16) come from it
    directly; more stream from a key made of those 32, outside any lock, in 64-byte chunks.
    Keys, blocks and output not handed out yet are 8-byte aligned and wiped on drop
    (volatile word stores).
- **Seed sources** (`Platform::random_seeds`, credit is the kernel's policy):

  | Source | Where | Credit |
  |---|---|---|
  | Device tree `/chosen/rng-seed` (QEMU `virt` gives 32 bytes) | AArch64, RISC-V | full, as Linux's default `random.trust_bootloader` |
  | Device tree `/chosen/kaslr-seed` (where the loader gives one: QEMU on AArch64) | AArch64, RISC-V | none |
  | `RDSEED` (100 tries per word), else `RDRAND` (10 tries, Intel's guide); samples of all ones or equal to the one before are thrown away | x86_64 | full, as Linux's default `random.trust_cpu` |
  | The clock, at every reseed | all | none |

  A device-tree seed is removed from the tree once taken: the property is overwritten with
  `FDT_NOP` tokens (Devicetree Specification 5.4.1), so nothing that reads the tree later, a
  kernel started after this one included, sees it or an empty seed it might trust; a boot
  self-test checks that asking again finds none. The x86_64 QEMU tests and benchmarks run
  `-cpu qemu64,+rdrand,+rdseed` (not `max`, which would change other code paths), and the
  benchmarks add `-seed 1` so QEMU's random values repeat.
- **Unseeded** (no source: a plain `qemu64`, pre-2012 x86, a board without a seed): the
  kernel boots and says so (`random: NOT SEEDED ...`). `AT_RANDOM` still comes from the
  generator, keyed from whatever was hashed in (Linux does not wait there either);
  `getrandom` without `GRND_INSECURE` waits on a wait queue that the reseed which seeds the
  generator wakes, or with `GRND_NONBLOCK` returns `EAGAIN`. No timer-jitter source: under
  `-icount` it would credit predictable data; jitter, UEFI's `EFI_RNG_PROTOCOL` and TPMs
  come with real hardware (M13). No periodic reseeding yet: every source today exists only
  at boot, and fast key erasure already protects earlier output.
- **`getrandom(buffer, length, flags)`** (318 on x86_64, 278 on AArch64 and RISC-V), as
  Linux: `EINVAL` for an unknown flag or `GRND_INSECURE | GRND_RANDOM`; the seeding check
  next (so a call for no bytes with `GRND_NONBLOCK` tells whether it is seeded);
  `GRND_RANDOM` means nothing more since Linux 5.6; at most `MAX_RW_COUNT` bytes, then
  `EFAULT` if the range does not end within user space; the bytes written before the first
  that is not the program's writable memory, or `EFAULT` for none; a preemption point every
  4096 bytes. `flags` is an `unsigned int`.
- **Performance.** One ChaCha20 block costs 1237 (x86_64), 1323 (AArch64) and 1998
  (RISC-V) instructions under `-icount` (`random_fill_16`, new; `random_fill_4096`, the
  long path, new): drawing `AT_RANDOM` alone made `linux_exec_load` 3.0%, 5.8% and 4.7%
  slower (AArch64, RISC-V, x86_64), and no batching brings 16 bytes of ChaCha20 under the
  gate's 1%. **Decided by research:** fix the exec path in the same PR rather than leave a
  regression for the owner, reporting each part apart (D53 did the same). Frames and page
  tables are now zeroed through `eggos_hal::memory::zero`: on AArch64 and RISC-V 64-byte
  lines of eight word stores (the compiler's `memset` there is a loop of single word
  stores, and page tables were zeroed one volatile entry at a time), on x86_64 `rep stosq`
  (the compiler's `memset`, which is the CPU's own fast fill; a store loop there made
  `linux_page_fault` 27% slower). Against `main`, with the generator: `linux_exec_load`
  −26.7% (AArch64), −18.3% (RISC-V), −0.6% (x86_64, where the tables' zeroing pays for the
  block); `linux_page_fault` −36.0% and −36.3% (AArch64, RISC-V), x86_64 unchanged. The
  owner can overrule this and ask for the two to be split.
- **Follow-ups:** RNDR/RNDRRS on AArch64 (FEAT_RNG, through `ID_AA64ISAR0_EL1`) and Zkr's
  `seed` CSR on RISC-V (needs `mseccfg.SSEED` from M-mode firmware and a trap-safe probe);
  timer jitter, UEFI and TPM seeds (M13); virtio-rng and reseeding at growing intervals,
  then every minute, once a source keeps producing (M8); `/dev/random` and `/dev/urandom`
  on the same generator (M7); `random.trust_cpu` and `random.trust_bootloader` boot
  options; a reseed when a virtual machine is cloned (vmgenid); a vDSO `getrandom` (Linux
  6.11); layout randomization (PIE base, stack, `mmap` area) from this generator (D51);
  `getrandom` interrupted by a signal once signals exist (`EINTR` before any byte, the
  partial count after; never for 256 bytes or fewer); `dc zva` on AArch64 and `cbo.zero`
  (Zicboz) on RISC-V for zeroing, measured; confirm by observation on Linux whether
  `getrandom` caps the length before or after its range check; an MCP and CLI capability
  that reports whether the generator is seeded (D22). From PR #49's independent review:
  `getrandom` copies 64 bytes at a time (a regions lock and a table walk each), far above
  ChaCha20's own cost for large buffers: fill a page at a time and measure the copy; a
  console line when a thread first waits for an unseeded generator (it waits for good
  until a later source exists); wiping ChaCha20's and BLAKE2s's working state.

## D57 The thread pointer: in each thread's user context, loaded on every way into user mode; x86_64's `arch_prctl`; `sched_yield` — Decided by the build loop (`arch_prctl`'s answers decided by research)
- **Why now.** Every C library and Rust's std set the thread pointer before `main`
  (thread-local storage, the stack-protector canary on x86_64 at `%fs:0x28`), so no compiled
  program runs past its startup without it; it is the next startup call after D55 and D56.
- **Where it lives** (supersedes D55's plan to save it in the thread switch next to FP/SIMD):
  in the thread's `UserContext`, as `UserContext::thread_pointer` and `set_thread_pointer`,
  so it moves with the thread between CPUs and nothing is added to the thread switch, which
  kernel threads use too (PR #47 showed that two compares there already fail the gate).
  - **AArch64:** `TPIDR_EL0`, which programs write themselves (`msr`), so the way out of EL0
    saves it and the way in loads it, with the other registers. The three extra instructions
    are paid for in the same code: `stp`/`ldp` with pre- and post-index for the kernel's
    callee-saved registers and `add sp, x0, #...` instead of two moves. `TPIDRRO_EL0` is
    zeroed at boot on every CPU (programs can read it; it held firmware's value).
  - **RISC-V:** `tp` is an ordinary register for the trap entry, already saved and loaded
    with the rest (the kernel's own `tp`, its per-CPU block, is kept on the kernel stack).
  - **x86_64:** the FS base. Programs cannot write it (`CR4.FSGSBASE` is now cleared at
    boot, whatever firmware left), so it is never saved. Each context carries an
    `fs_base_id`: 0 while its FS base was never set (0), and a new number from a global
    counter each time it is set. The way into ring 3 compares it with the one this CPU's FS
    base was loaded for (a per-CPU word) and runs `wrmsr` only when they differ, so a
    program that keeps its CPU pays one load and compare per entry and no MSR write. The id,
    not the value, is compared: a program that loads FS with a selector (base 0) cannot make
    another program with the same value keep that zero (until `fork`/`clone` copy a context
    with its id: then a fresh id for the copy, a follow-up). `set_thread_pointer` refuses a
    value at or above the end of user space on x86_64 (`InvalidThreadPointer`), whoever
    calls it (`clone`'s `CLONE_SETTLS` later): loading a non-canonical FS base would fault
    in the kernel. The `sysretq` path now loads the
    registers from the context with moves instead of pops and skips the two that `sysretq`
    overwrites, which pays for the check. The FS base and the user GS base
    (`IA32_KERNEL_GS_BASE`) are zeroed at CPU start.
- **Cost.** Gated benchmarks within the gate on every architecture, the system-call round
  trips slightly faster: `syscall_round_trip` 106.17 → 105.24 (AArch64), 148.27 → 147.95
  (RISC-V), 107.44 → 106.98 (x86_64). The thread switch is untouched.
- **`arch_prctl(code, address)`** (x86_64, 158), as Linux on a CPU without CPUID faulting
  (decided by research, from the man page and observed behaviour):

  | Code | Result |
  |---|---|
  | `ARCH_SET_FS` | 0; `EPERM` at or above the end of user space (2^47 − 4096); not checked for a mapping |
  | `ARCH_GET_FS`, `ARCH_GET_GS` | the base written as an `unsigned long`, `EFAULT` if it cannot be; the GS base is always 0 |
  | `ARCH_SET_GS` | `EINVAL`, as for a code the kernel lacks, until the SWAPGS fence (D49) lets a program choose its GS base; no C library or Rust's std sets it (Wine and DynamoRIO do) |
  | `ARCH_GET_CPUID` | 1 |
  | `ARCH_SET_CPUID` | `ENODEV` |
  | anything else (shadow stacks, AMX permissions, vDSO mapping, LAM) | `EINVAL`, which their callers take as the feature being absent |

  `code` is a C `int`: only its low 32 bits count.
- **`sched_yield()`** (24 on x86_64, 124 on AArch64 and RISC-V): lets the next ready thread
  on the CPU run, and returns 0. The self-test needs it, and C libraries and Rust's std call
  it in spin loops.
- **Tests.** A host test of every `arch_prctl` answer. A QEMU self-test,
  `linux_programs_keep_their_thread_pointers`: Linux test programs check that their thread
  pointer starts at zero, set it to `getpid() << 12` as their C library would (`msr
  tpidr_el0`, `mv tp`, `arch_prctl(ARCH_SET_FS)`; on x86_64 they read a marker through
  `%fs`, since a program cannot read its FS base), and check it after each of 20
  `sched_yield`s: three programs on one CPU before the secondary CPUs start, then up to 17
  on every CPU, twice, with every frame of the second round freed. It fails with the
  AArch64 load or the x86_64 `wrmsr` removed (checked).
- **Follow-ups:** a per-CPU "last context" check on AArch64 instead of the unconditional
  `msr` once a real CPU shows `TPIDR_EL0` writes cost more than QEMU's count; with `clone`,
  `CLONE_SETTLS` through `set_thread_pointer`, and a fresh `fs_base_id` for any context
  `fork` or `clone` copies (from PR #50's independent review); the DS, ES, FS and GS
  selectors a program loads stay for the next program on its CPU (16-bit channels between
  programs; the FS and GS bases do not leak), and a program that loaded FS sees base 0 only
  on that CPU: save and reset them as Linux does; a `process_switch_round_trip` variant with
  the FS base set (every real program has one, so real switches pay the `wrmsr`), and the
  `wrmsr` and `msr tpidr_el0` measured on real hardware (M13); the GS base
  per thread with `ARCH_SET_GS` and the SWAPGS fence (D49); FSGSBASE for programs (Linux
  5.9+, `AT_HWCAP2`), which needs the FS base saved on the way out; `set_thread_area` and
  32-bit programs.

## D58 Review of D55–D57: empty ranges past user space are `EFAULT`; `writev`'s count is an `unsigned int`; BLAKE2s wiped with volatile stores — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** The review step after three feature PRs (#48–#50): an independent read of the
  merged code for bugs those PRs' reviews missed. It found no serious bug; the RFC 8439 and
  RFC 7693 test vectors, the per-CPU key derivation and the thread-pointer paths check out.
- **An empty range must still start within user space** (supersedes D50's "a zero-length
  write returns 0 unchecked"). `ends_in_user_space` accepted any address for 0 bytes; Linux's
  user-range check also runs for 0 bytes and then asks only that the start be at most the end
  of user space. So `write(1, kernel_address, 0)`, `getrandom(kernel_address, 0, flags)` and a
  `writev` with an empty buffer at a kernel address are now `EFAULT` (the whole `writev`,
  before anything is written), while 0 bytes anywhere up to and including `USER_END` (mapped
  or not, `NULL` too) still return 0. Research: the generic `access_ok` series merged for
  Linux 5.18 (a single comparison against the end of user space, on every architecture),
  LTP's `EFAULT` cases; high confidence on x86_64 and AArch64, medium-high on RISC-V. Nothing
  is read or written either way: ABI, not security.
- **`writev`'s count is an `unsigned int`**, as the descriptor already was: only the
  register's low 32 bits count, so `1 << 32` writes nothing and returns 0, and
  `(1 << 32) | 1` writes the first buffer (research: writev(2)'s `int iovcnt`; gVisor reads
  32 bits too). Before, both were `EINVAL`.
- **`writev`'s other edge case from PR #48's review**, one huge buffer past user space: Linux
  6.1 (the release `uname` reports) checks every buffer's whole range before capping at
  `MAX_RW_COUNT`, so `EFAULT`, which is what we do. Linux 6.4 and later cap a single-buffer
  `writev` first and return a partial count (research). We keep 6.1's answer while `uname`
  reports 6.1.
- **BLAKE2s wipes its state and buffer on drop**, with volatile stores
  (`eggos_hal::random::wipe`). `finalize` cleared the buffer with a plain store to a value
  about to go away, which the compiler may remove, leaving seed material (`rng-seed`,
  `RDSEED`) on the stack. Off every hot path (reseeding only). ChaCha20's per-block working
  state is still not wiped: doing it in `chacha20::block` adds about 32 stores per 64-byte
  block, about 3% on `random_fill_4096`, above the benchmark gate; it waits for a design that
  keeps the state in registers or wipes once per draw. Best effort, as `Secret` (D56): copies
  the compiler makes (`compress`'s working vector, the state copied out in `finalize`, moves)
  are not wiped.
- **Follow-ups added.** When programs get FSGSBASE (D57), the way out of user mode saves a
  program-written FS base: give the context a new `fs_base_id` whenever the saved value
  differs, or two copies sharing an id (after `fork` or `clone`) could hold different bases
  and one would run on the other's. Today copies share an id only with the same value, which
  is correct. `ARCH_GET_FS` after a program loads an FS selector reports the old base where
  Linux reports 0 (the program only affects itself); fixed with D57's selector follow-up.

## D59 Resource limits: `getrlimit`, `setrlimit`, `prlimit64`; per-process limits copied on first change; no hard limit raised until credentials — Decided by the build loop (privilege, `pid` and defaults decided by research)
- **Why now.** M6's next startup call after D55–D58: glibc reads `RLIMIT_STACK` at startup
  (its default thread stack size), Rust's std reads it for the main thread's guard, and
  shells, build tools and servers read the open-file limit and raise their soft limit to the
  hard one. musl tries `prlimit64` first and falls back to `getrlimit`/`setrlimit`, so all
  three exist, with each architecture's numbers (x86_64: 97, 160, 302; the generic table
  AArch64 and RISC-V use: 163, 164, 261).
- **Per process, copied on first change** (`linux::limits::ResourceLimits`, in
  `Process::resource_limits` behind its own `SpinLock`, never held while user memory is
  copied): `None` means the shared `DEFAULTS`; the first change that changes anything makes
  the process's own copy on the heap (fallible: `ENOMEM`). A first version kept all 16
  limits inline: `Process` grew from 168 to 432 bytes and every `Process::new` copied
  256 bytes, +1.3% on riscv64's `linux_exec_load` (above the gate), and the larger `Process`
  moved to the heap's 512-byte class, which a boot self-test's frame count noticed. Now a
  process costs a word and a lock for its limits and `linux_exec_load` is within noise.
- **No process may raise a hard limit yet** (research: fail closed). Linux needs
  `CAP_SYS_RESOURCE`; processes have no credentials, so nothing can tell a privileged caller
  from another, and allowing raises would make every lowered hard limit reversible, which
  sandboxes and agent confinement (D21) rely on. Startup code only raises soft limits up to
  the hard one, which is allowed. One predicate (`may_raise_hard_limits`, false) becomes the
  capability check with credentials, and `init` (M10) gets the privilege. An open-file limit
  above 1048576 (`fs.nr_open`'s default, `MAX_OPEN_FILES`) is `EPERM` whatever the privilege,
  so `RLIM_INFINITY` there is refused, as on Linux.
- **Only the caller's own limits** (research): `prlimit64` with a `pid` other than 0 or the
  caller's (its thread id is the same, D55) is `ESRCH` until there is a process table; then
  other processes are `EPERM` without the same user or the capability.
- **Defaults** (research: what current systemd distributions show in `ulimit -a`; man pages):
  everything unlimited except `RLIMIT_STACK` 8 MiB soft (D52's stack region) and unlimited
  hard; `RLIMIT_CORE` 0 soft, unlimited hard; `RLIMIT_NOFILE` 1024 soft and 524288 hard
  (systemd's, not the kernel's 4096: with no way to raise a hard limit, 4096 would strand
  servers); `RLIMIT_MEMLOCK` 8 MiB (Linux 5.16 and later; `uname` says 6.1);
  `RLIMIT_NPROC` and `RLIMIT_SIGPENDING` a fixed 65536 (Linux derives them from memory: fixed
  until enforced, so every machine reports the same); `RLIMIT_MSGQUEUE` 819200;
  `RLIMIT_NICE` and `RLIMIT_RTPRIO` 0. The resource numbers are the generic ones (MIPS,
  SPARC and Alpha differ); another architecture is a compile error until it is checked.
- **Linux's order of errors**: `EFAULT` for an unreadable new limit; `ESRCH`; `EINVAL` for a
  number with no resource (the register's low 32 bits, as `pid`'s) or a soft limit above the
  hard one; `EPERM`; then, with the new limits set, `EFAULT` for an unwritable old one.
  `getrlimit` and `setrlimit` with a null pointer are `EFAULT`; `prlimit64` takes null for
  either and with both only checks. Research could not confirm the order between `ESRCH` and
  `EINVAL` on a running Linux: to check by observation.
- **Nothing is enforced yet.** Each limit is enforced when its resource exists: file
  descriptors (M7), `RLIMIT_DATA` and `RLIMIT_AS` on the memory calls, `RLIMIT_STACK` when
  the stack region's size follows it at `execve` (today it is the fixed 8 MiB the default
  reports; a larger soft limit is accepted and does not grow it), `fork` copying the limits.
- **Tests.** Host tests: the defaults, lowering and raising rules, no copy for a change that
  changes nothing, the structure's layout; the three calls through the dispatcher with user
  memory (upper halves ignored, every error, nothing changed by a failed call, a new limit
  kept when the old one cannot be copied out, a second process starting from the defaults).
  QEMU, on aarch64, riscv64 and x86_64: the ELF-started
  test program calls `prlimit64(0, RLIMIT_STACK, NULL, old)` with the architecture's number
  and checks 8 MiB soft and unlimited hard.

## D60 Signal state: `rt_sigaction`, `rt_sigprocmask`, `sigaltstack`; actions per process copied on first change, mask and alternate stack per thread; nothing delivered yet — Decided by the build loop (design and Linux's rules decided by research)
- **Why now.** The signal calls are the next startup calls on M6: Rust's std sets `SIGPIPE` to
  `SIG_IGN` (a failure is fatal to it), reads `SIGSEGV`'s and `SIGBUS`'s actions and, when
  they are `SIG_DFL`, installs its stack-overflow handlers on an alternate stack; glibc blocks
  and unblocks signals around thread creation, `abort` and more. With `ENOSYS` every Rust
  program would stop at startup. Numbers: x86_64 13, 14, 131; the generic table AArch64 and
  RISC-V use, 134, 135, 132.
- **Where the state lives** (research; `linux::signals::Signals`, in `Process::signals`
  behind its own `SpinLock`, never held while user memory is copied): the actions are the
  process's, the blocked mask and the alternate stack its thread's (`ThreadSignals`, a type
  of its own now so it moves to each thread as a unit when `clone` exists, with D55's
  `set_tid_address` and robust-list state). The 64 actions are `None` (all `SIG_DFL`) until
  the first change that changes anything makes the process's own 2 KiB table, allocated
  fallibly (`ENOMEM`), as D59 did for the limits: a process costs a word for its actions.
- **The kernel's `struct sigaction`** per architecture: handler, flags, restorer, mask (32
  bytes) on x86_64 and AArch64, which define `SA_RESTORER`; handler, flags, mask (24 bytes)
  on RISC-V, which has no `sa_restorer` (its handlers return through the vDSO). `stack_t` is
  the base, an `int` of flags, 4 bytes of padding (written as zero) and the size.
- **Linux's rules** (research, from the man pages; error order to check by observation):
  - `rt_sigaction`: `EINVAL` for a `sigsetsize` other than 8 (all 64 bits compared); `EFAULT`
    for an unreadable new action; `EINVAL` for a number outside 1 to 64 (the register's low
    32 bits) or a new action for `SIGKILL` or `SIGSTOP` (theirs may still be read); then the
    old action copied out, `EFAULT` keeping the new one, as D59. Unknown flags are cleared,
    as since Linux 5.11, so `SA_UNSUPPORTED` reads back clear and a program learns which
    flags the kernel lacks; `SIGKILL` and `SIGSTOP` are left out of the handler's mask. The
    handler's address is not checked.
  - `rt_sigprocmask`: `EINVAL` for a bad `sigsetsize`; `EFAULT` for an unreadable set;
    `EINVAL` for a `how` other than block, unblock or set, looked at only with a set;
    `SIGKILL` and `SIGSTOP` silently never blocked; then the old mask copied out.
  - `sigaltstack`: `EFAULT`; `EPERM` while the thread's stack pointer is on its alternate
    stack (above its base and at most at its top, as the stack grows down; never with
    `SS_AUTODISARM`, which disarms it under a handler); `EINVAL` for flags
    other than 0, `SS_ONSTACK` (taken as 0) or `SS_DISABLE`, each with or without
    `SS_AUTODISARM`; `ENOMEM` below `MINSIGSTKSZ` (2048, 5120 on AArch64); a disabled stack
    keeps no base or size. The old stack reports `SS_DISABLE`, `SS_ONSTACK` or 0, with
    `SS_AUTODISARM` if it was set.
- **Handlers are accepted before delivery exists** (research): refusing them would break
  every Rust program. A fault still ends the process with its signal whatever the action
  and mask say. That is what Linux does for a fault whose signal is blocked or ignored, and
  what std's handler ends in (it restores the default and the fault repeats). A runtime that
  recovers from faults (a JVM, Go, a garbage collector's write barrier) dies until delivery
  exists. Delivery is the next step: signal frames per architecture, `rt_sigreturn` (with a
  restorer on x86_64 and AArch64, a vDSO trampoline on RISC-V), `kill`, `tgkill`, pending
  sets checked on the way back to user mode, and `AT_MINSIGSTKSZ` (D51).
- **For later**, recorded so they are not lost: `execve` resets handled signals to
  `SIG_DFL` and keeps `SIG_IGN` and the mask, and removes the alternate stack; `fork` copies
  everything; a thread made by `clone` starts with no alternate stack. `SIGPIPE` ignored
  makes writes to a closed pipe `EPIPE` (M7); `SIGCHLD` ignored, or `SA_NOCLDWAIT`, reaps
  children (`wait4`, M10); setting `SIG_IGN` discards that signal's pending instances.
- **Cost** (same Mac, QEMU `-icount`; research: a delta inside the gate is not a regression,
  as D27 defines it and PRs #48, #50 and #52 merged). `Process` grows by 48 bytes (184 to
  232 on the host), still in the heap's 256-byte class with its `Arc`. `linux_exec_load`
  against `main`: +0.09% on aarch64 from the kernel change alone (about 32 instructions per
  `exec`, the larger `Process` moved), +0.17% aarch64, +0.16% riscv64 and +0.05% x86_64 with
  the start test program's new checks, which the benchmark loads (a larger image, not a
  slower kernel). The system-call, switch, page-fault and `mmap` benchmarks are unchanged.
  `linux_exec_load` has now used about 0.4% of its 1% allowance on riscv64 against an
  unchanged baseline: the next change on the `exec` path has less room.
- **Tests.** Host tests: the default actions, the change rules (no table made for a change
  that changes nothing, `SIGKILL` and `SIGSTOP` refused, unknown flags cleared, the mask
  cleaned), the structures' layouts and the alternate stack's reported flags; the three
  calls through the dispatcher with user memory (upper halves ignored, every error, a change
  kept when the old value cannot be copied out, `EPERM` on the alternate stack, a second
  process starting from the defaults). QEMU, on aarch64, riscv64 and x86_64: the ELF-started
  test program sets `SIGPIPE` to `SIG_IGN`, reads it back, and checks that `sigaltstack`
  reports no alternate stack, with each architecture's numbers and structure.

## D61 The standard descriptors: 0, 1 and 2 are the console; `poll`, `ppoll` and the console's `ioctl`s — Decided by the build loop (Linux's behaviour and the request set decided by research)
- **Why now.** Rust's std polls descriptors 0, 1 and 2 at startup (`poll` with no events and
  no wait) to learn whether they are open, and aborts when the call fails with anything but
  `EINTR`, `EINVAL`, `EAGAIN` or `ENOMEM`: with `ENOSYS`, every Rust program would stop
  before `main`, even once FP/SIMD state exists (D54). It then opens `/dev/null` over a
  descriptor that reports `POLLNVAL`, which would fail too, so descriptor 0 must be open.
  musl chooses line buffering for stdout and answers `isatty` with `TIOCGWINSZ`, glibc with
  `TCGETS`. Numbers: x86_64 `poll` 7, `ioctl` 16, `ppoll` 271; the generic table, which has
  no `poll`, `ioctl` 29, `ppoll` 73.
- **Descriptor 0 joins the console** (research): 0, 1 and 2 are one console open for reading
  and writing, as Linux starts `init` with `/dev/console` on all three and getty does with
  its terminal, so `write(0, ...)` prints, as it does in a Linux terminal. `read` stays
  `ENOSYS` until there is console input (M8). `is_console` became one compare.
- **`poll` and `ppoll`** (`linux::poll`), from the man pages:
  - the console is always writable (`POLLOUT`, `POLLWRNORM` when asked for) and never
    readable; `POLLERR` and `POLLHUP` are never reported; a negative descriptor is skipped
    (no events); any other descriptor is not open: `POLLNVAL`, whatever was asked. The
    result is how many entries report anything; the array is copied in once and back once.
  - checks, in this order (the order is to be confirmed by observation): `ppoll`'s timeout
    `EFAULT`, then `EINVAL` for negative seconds or nanoseconds outside 0 to 999999999; with
    a signal mask, `EINVAL` unless `sigsetsize` is 8, then `EFAULT` (no mask: the size is not
    looked at); `EINVAL` for more descriptors than `RLIMIT_NOFILE`'s soft limit (D59's
    1024); `EFAULT` if the array does not end within user space or is not the program's
    writable memory; `ENOMEM` if a large array finds no heap. `nfds` is an `unsigned int`,
    `poll`'s timeout an `int` of milliseconds (negative: no limit).
  - **waits sleep out their timeout.** Nothing becomes ready during a wait yet (the console's
    readiness never changes), so a call that finds nothing ready sleeps until its deadline
    (`thread::sleep_until`, rounded up, never early), or for good without a timeout, as Linux
    does when no signal comes. `poll(NULL, 0, ms)` therefore sleeps, as programs use it to.
    Waking a poller early comes with the first descriptor whose readiness changes.
  - `ppoll` writes a zero remaining time back when its wait ended with the timeout, as
    Linux's system call does (C libraries hide it), ignoring a timeout the program cannot
    write. When something is ready at once, it leaves the timeout as it was: no time passed
    that the kernel would report.
  - the signal mask would apply during the wait; with nothing delivered (D60) it is checked
    and changes nothing.
- **The console's `ioctl`s** (`linux::terminal`; research: answer only what is true of the
  console): `EBADF` for a descriptor that is not open, before the request; `ENOTTY` for every
  request not answered, as Linux answers a request a terminal lacks; `EFAULT` for a bad
  argument. Both `fd` and the request are 32-bit.
  - `TCGETS` and `TCGETS2`: a Linux serial console's settings after boot (`stty -a`): `icrnl
    ixon`, `opost onlcr` (the UART driver already writes `\n` as `\r\n`), `115200 cs8 cread
    hupcl clocal`, `isig icanon iexten echo echoe echok echoctl echoke`, and the usual
    control characters (^C, ^\, DEL, ^U, ^D, VMIN 1, VTIME 0, ...). `TCGETS2` adds both
    speeds, 115200. The kernel's `struct termios` (36 bytes) and these request numbers are the
    same on AArch64, RISC-V and x86_64; another architecture is a compile error until checked
    (Alpha, MIPS, PowerPC and SPARC differ).
  - `TIOCGWINSZ` and `TIOCSWINSZ`: one window size for the console, zero until a program
    sets it, as on a Linux serial line, whose size the kernel cannot know.
  - `FIONREAD`: 0, as there is no input.
  - `ENOTTY` until their state exists: `TCSETS`, `TCSETSW` and `TCSETSF` (a line discipline,
    with console input; storing settings nothing acts on would be dishonest), `TIOCGPGRP`
    and `TIOCSPGRP` (Linux's answer when the terminal is not the caller's controlling one,
    which is every case until sessions exist; shells then run without job control), and
    `FIONBIO`, `FIOCLEX` and `FIONCLEX` (descriptor flags, with a descriptor table, M7).
- **`fcntl` waits for the descriptor table** (research): Rust's std calls it only when `poll`
  fails with `EINVAL`, `ENOMEM` or `EAGAIN`; glibc's and musl's standard-descriptor checks run
  only for secure (setuid, `AT_SECURE`) programs.
- **Not verified** (no Linux to observe from here), recorded as follow-ups: the order of
  `ppoll`'s checks; whether `sigsetsize` is checked with no mask; `CLOCAL` on a serial
  console; whether current glibc reads the settings with `TCGETS2`; whether `ppoll` writes
  the remaining time back when something is ready at once (here the timeout is left as it
  was); whether `poll` with no descriptors and an array address past user space is `EFAULT`,
  as D58's rule for empty ranges makes it here.
- **From PR #54's independent review**: a wait is decided by "nothing ready and a timeout
  that is not zero", so a deadline that passes while the array is read still writes the zero
  remaining time back; the self-test runs twice and counts frames after the first run, as
  the other process self-tests do. Left for later: an array of up to 524288 entries (the
  highest soft limit) is one 4 MiB heap allocation, held through an endless wait and copied
  with no preemption point; copy it in pieces with a point between them, once programs poll
  that many descriptors.
- **Cost** (same Mac, QEMU `-icount`, against `main`). The new calls sit behind
  `out_of_line_call`, so the system-call path is unchanged: `linux_syscall_round_trip` and
  `process_syscall_round_trip` move by at most 0.01 ns, the switch, page-fault and `mmap`
  benchmarks by as little. `linux_exec_load` rises +0.12% on aarch64, +0.37% on riscv64 and
  +0.03% on x86_64. The cause is the new test program: the benchmark loads an image that holds
  the whole HAL test program, so it copies more bytes. The kernel's exec path did not change.
  This is inside the gate, so the baseline is unchanged. But riscv64's `linux_exec_load` now
  stands at +0.71% of its 1% allowance against the baseline (from D60's +0.4%). The next
  test program added to the HAL, or any change on the exec path, should first make the
  benchmark load only the code `linux_start` needs, so the gate measures the kernel and not
  the test image.
- **Tests.** Host tests:
  - the events per descriptor;
  - the terminal settings' words;
  - `ppoll` and (on x86_64) `poll` through the dispatcher with user memory: Rust's startup
    poll, every event case, the heap path, the upper halves ignored, every error in order, a
    read-only timeout accepted, and the count checked against a lowered `RLIMIT_NOFILE`;
  - `ioctl`: every request on every standard descriptor, the window size set and read back,
    `EBADF` before `ENOTTY`, `EFAULT`;
  - `write` on descriptor 0.

  QEMU, on aarch64, riscv64 and x86_64: a new test program (`linux_poll`) polls 0 to 2 as
  Rust's std does but asking for `POLLOUT` on 1, reads the window size, then waits 2 ms in
  `ppoll`. It checks the events and the zero remaining time. The self-test checks it ran at
  least 2 ms and freed everything; it fails if the sleep is removed (checked on aarch64).

## D62 Review of D59–D61: the exec benchmark loads only `linux_start`'s code; `writev` reads its entries in order; `ppoll` writes back the time not slept — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** The review step after three feature PRs (#52–#54): a fresh read of resource limits,
  signal state, `poll`/`ppoll` and the console's `ioctl`s against the man pages' rules. The
  limit and signal calls' checks, error order, structure layouts (`struct rlimit64`, the
  kernel's `struct sigaction` with and without `sa_restorer`, `stack_t`, `struct termios` and
  `termios2`, `TCGETS2`'s number) and the alternate-stack bounds check out. Three things change.
- **`linux_exec_load` loads only the code `linux_start` needs.** It loaded an ELF image holding
  the whole HAL test program, so every new test program grew the benchmark: D61 left riscv64 at
  +0.71% of its 1% allowance, and signal delivery needs a test program of its own. The HAL's
  `TestProgram` now marks where `linux_start`'s code ends (`linux_start_end`, a label on each
  architecture), and the benchmark loads `selftest::linux_start_only_image`, the same image cut
  there. The self-tests keep the whole image: two of them run `stack_growth` from it. Against
  `main`'s CI run: −0.24% on aarch64, −0.45% on riscv64, unchanged on x86_64 (its copies are
  `rep movsb`, which QEMU's `-icount` barely counts). The baseline is unchanged: these numbers
  are still above it, so lowering it would loosen the gate.
- **`writev` reads its entries in order** (research; supersedes part of D55's error order).
  Since Linux 5.10, `writev` first checks that the whole array ends within user space (`EFAULT`,
  nothing read), then reads the entries one at a time: an unreadable one is `EFAULT`, a length
  negative as a `ssize_t` is `EINVAL` at once; only then are the buffers' ranges checked. We
  copied the array in one piece, so an array whose readable first entry had a negative length
  and whose second was unmapped was `EFAULT` instead of `EINVAL` (found by PR #51's independent
  review). The fast path is unchanged; only when the copy fails does a cold function read the
  entries one by one to choose the error. Sources: readv(2), LTP's `writev` and `readv` tests.
- **`ppoll` writes back the time not slept for every timeout that is not zero** (research;
  supersedes D61's "left as it was when something is ready at once"). select(2) says Linux
  updates the timeout "to reflect the amount of time not slept", and ppoll(2) that the system
  call modifies it: whatever the result, once the timeout was read and found valid (and here
  once the signal mask passed its checks), Linux writes back what is left of it, zero after a
  wait that ran out. So a call with something ready at once now writes back slightly less than
  it was given, and one that fails later (`EFAULT` for the array, say) writes it back too. The
  remaining time is the deadline minus the clock, in nanoseconds rounded up, never more than was
  asked. A timeout of zero, or none, writes nothing, and a write the program's memory refuses is
  ignored, as before. A timeout too long to count in nanoseconds (over 584 years) or to reach in
  clock ticks is left as it was, which is about what is left of it. Go and static programs call
  the system call directly; C libraries hide it.
- **Noted for signal delivery (next):** x86_64 handlers return only through `sa_restorer`, and
  Linux delivers nothing without `SA_RESTORER` there; frames on x86_64 skip the 128-byte red
  zone below the stack pointer, which our own test programs use; a fault whose signal is blocked
  or ignored still kills, as today.
- **Follow-up added:** `poll` and `ppoll` copy the whole `struct pollfd` array back, where Linux
  writes only the `revents` fields; that differs once `clone` lets another thread change the
  array during a wait.
- **Tests.** Host: `writev` with an array that runs off its page (`EINVAL` with a negative
  length before the unreadable entry, `EFAULT` without) and with an array past the end of user
  space whose readable entry is negative (`EFAULT`, nothing read); `ppoll` writing back a little
  under 5 s when ready at once, after a later `EFAULT`, zero after a wait that ran out, nothing
  for a too-long timeout, and nothing to a read-only one. The `writev` `EINVAL` case and the
  ready-at-once and after-`EFAULT` `ppoll` cases fail without their fixes; the rest guard the
  error order. `cargo xtask ci` passes: fmt, clippy, 259 host tests, boot tests and benchmarks
  on aarch64, riscv64 and x86_64.

## D63 Signals sent and delivered, part 1: `kill`, `tkill`, `tgkill`, `rt_sigpending`; default actions delivered before the call returns — Decided by the build loop (Linux's rules decided by research)
- **Why now.** `abort()` (C's, and Rust's `panic = "abort"`) sends itself `SIGABRT` with
  `tgkill` or `kill` and expects to die of it. Until now that was `ENOSYS`, and musl's
  `abort` then crashed on an illegal instruction, so the process ended with `SIGILL` instead.
  Programs also send themselves signals with `raise`, and use `kill(pid, 0)` to check that a
  process exists. Signal delivery is split in two. Part 1 (this) covers sending and the
  default actions. Part 2 adds handlers: signal frames on each architecture, `rt_sigreturn`,
  `SA_ONSTACK`, `SA_RESETHAND` and `SA_NODEFER`, and real-time signals queued with their
  `siginfo`.
- **Targets** (research, kill(2) and tgkill(2)). Only the caller is reachable, as for
  `prlimit64` (D59).
  - `kill` reaches the caller through its own id, through 0 (its process group), or through
    minus its id (that group). Each process leads its own group until `setpgid` exists.
  - `-1` means every process the caller may signal except itself, so it is `ESRCH`. Linux
    returns `ESRCH` when nothing was signalled. `-1` is never process 1's group.
  - Any other id is `ESRCH`. The target is found before the signal is looked at, so a bad
    signal to a missing process is `ESRCH`.
  - `tkill` and `tgkill` return `EINVAL` for an id that is not positive, then `ESRCH` unless
    the ids are the caller's (its thread's id is the process's), then `EINVAL` for a signal
    outside 0 to 64.
  - Signal 0 only checks the target. The ids are `pid_t`: the registers' upper halves are
    ignored.
- **Pending signals and `rt_sigpending`** (research).
  - Each thread has a pending set beside its blocked mask: 8 bytes in the existing `Signals`
    lock, no allocation.
  - A signal that is ignored (`SIG_IGN`, or a default of ignore) and not blocked is discarded
    when sent. A blocked one stays pending even if ignored, as on Linux.
  - Setting an action that ignores a signal discards it if pending, blocked or not, as POSIX
    requires.
  - `SIGCONT` discards pending stop signals, and a stop signal discards a pending `SIGCONT`,
    whatever their actions (POSIX).
  - `rt_sigpending` reports the pending signals that are blocked. As on Linux, it takes any
    `sigsetsize` up to 8 and copies that many bytes, `EINVAL` above 8, then `EFAULT`.
  - A set does not count a signal twice. Real-time signals' queue, and each signal's
    `siginfo`, come in part 2 with the calls that can observe them (`rt_sigtimedwait`,
    `rt_sigqueueinfo`, handlers with `SA_SIGINFO`).
- **Delivery before the call returns, with nothing added to the system-call path.**
  - POSIX requires a signal a process sends itself, or one it unblocks, to be delivered
    before `kill` or `sigprocmask` returns. Only the calls that can make a signal due do it
    today: `kill`, `tkill`, `tgkill`, `rt_sigprocmask`, and `rt_sigaction` (a pending signal
    whose handler is removed).
  - So `out_of_line_call` runs `signals::deliver` after those calls only. It sets the
    result itself and may break, which ends the thread. The inlined dispatch's catch-all arm
    now just returns its call's control flow.
  - Signals from other processes and timers will need a check on the way back to user mode:
    a per-thread "pending and not blocked" test, one load and a branch, perhaps folded into
    the reschedule check that preemption already makes there (D48).
- **Default actions** (signal(7)):
  - **Term and Core** end the process with the signal (`ProcessExit::Killed`). There are no
    core dumps yet.
  - **Ign** (`SIGCHLD`, `SIGURG`, `SIGWINCH`) and **Cont** discard the signal.
  - **Stop.** No process has a parent, so every process group is orphaned. POSIX then has the
    terminal's stop signals (`SIGTSTP`, `SIGTTIN`, `SIGTTOU`) discarded at their default
    action.
  - `SIGSTOP` really stops: the thread sleeps until `SIGCONT` or `SIGKILL`. Only another
    process could send either, so for good, as on Linux when no one does.
  - Lowest number first, and `SIGKILL` whatever the mask (it can never be blocked). A
    signal with a handler stays pending until part 2 delivers it.
- **Process 1 is not protected yet.** On Linux, `init` receives only the signals it has a
  handler for, so a self-sent `SIGABRT` would be dropped there. Here process 1 is just the
  first program started, not `init`, so it dies like any other. The protection comes with
  `init` (M10), deliberately not before.
- **Faults** still end the process with their signal whatever its actions (D60), which is
  Linux's behaviour for a blocked or ignored fault signal. Faults go to a handler in part 2.
- **Follow-ups.**
  - `ppoll`'s signal mask applied during its wait, which ends the wait with `EINTR` (needs
    handlers).
  - `rt_sigsuspend` and `rt_sigtimedwait`.
  - Signals the kernel raises itself: `SIGPIPE` on a write to a broken pipe (M7), `SIGXFSZ`
    with the file-size limit.
  - The return-path check, once anything else can send.
  - A stopped state the scheduler and `wait4` see, once parents exist.
- **Cost** (same Mac, QEMU `-icount`, against `main`'s CI run): every benchmark within
  ±0.02%, apart from riscv64's two SBI timer benchmarks, which differ between local QEMU and
  CI on `main` too. `linux_exec_load` moves +0.01%, although a new test program was added
  (D62's cut) and the thread state grew by the pending set.
- **Tests.**
  - Host:
    - the default-action table against signal(7);
    - the pending rules (discarding at send and at delivery, blocked ignored signals kept
      and reported, a handler's signal kept, lowest first, the terminal's stop signals
      discarded, `SIGSTOP` stopping, `SIGCONT` and the stop signals cancelling);
    - through the dispatcher: every target and error in order, `rt_sigpending` at sizes 8,
      2 and 0 and its errors, a blocked `SIGUSR1` ending the process when unblocked, and
      `tgkill(SIGABRT)` ending it with `SIGABRT`.
  - QEMU, on aarch64, riscv64 and x86_64: the test program `linux_signals` checks that
    `kill(0, SIGCHLD)` is ignored, then blocks `SIGUSR1`, sends it with `tgkill`, checks
    `rt_sigpending` and unblocks it. The self-test `linux_programs_signal_themselves` requires
    the process to be killed by `SIGUSR1`, and every frame of its second run to be freed.

## D64 Signal handlers, part 2 of delivery: Linux's frames, `rt_sigreturn`, `SA_ONSTACK`, `SA_NODEFER`, `SA_RESETHAND`; a return trampoline for AArch64 and RISC-V — Decided by the build loop (the trampoline, the FP/SIMD parts and Linux's rules decided by research)
- **Why now.** Rust's std installs `SIGSEGV` and `SIGBUS` handlers on an alternate stack at
  startup, C programs catch `SIGINT`, `SIGTERM`, `SIGCHLD` and `SIGALRM`, and `raise()` with
  a handler must run it. Until now a caught signal stayed pending for ever (D63).
- **Frames are Linux's `struct rt_sigframe`** on each architecture, every offset from the
  architecture's ABI (`linux::signal_frame`):
  - x86_64: `pretcode` (the restorer), the `ucontext_t` (`uc_mcontext` before `uc_sigmask`),
    then the `siginfo_t`; 440 bytes. It goes below the 128-byte red zone and a 512-byte
    `FXSAVE` image (64-byte aligned), so that `(rsp + 8) % 16 == 0` at the handler, as after
    a `call`. `uc_flags` is `UC_SIGCONTEXT_SS | UC_STRICT_RESTORE_SS`.
  - AArch64: the `siginfo_t`, then the `ucontext_t`, 4688 bytes, with a frame record
    (`x29`, `x30`) above it that the handler's `x29` points at, so backtraces walk out of the
    handler.
  - RISC-V: the `siginfo_t`, then the `ucontext_t`, 1088 bytes.
- **The handler starts** with the signal, the `siginfo_t` and the `ucontext_t` in its first
  three argument registers (always, a harmless superset of Linux, which skips them on
  AArch64 without `SA_SIGINFO`), its return address the restorer (`x30`, `ra`, or the
  stack's top word on x86_64), and on x86_64 `rax` 0 and the trap, direction and resume
  flags clear. The frame saves the registers, the blocked mask from before the handler, and
  the alternate stack as `sigaltstack` would report it at the interrupted stack pointer
  (`SS_ONSTACK` if on it, `SS_DISABLE` if none, with `SS_AUTODISARM` if set), as research
  recommended; current Linux may write only the flags the stack was set with (0 for an
  enabled one). Either restores the same stack.
- **The HAL** gains `UserContext::registers` and `set_registers` (`UserRegisters`, each
  architecture's own set). Setting them makes x86_64 enter with `iretq`, so `rcx` and `r11`
  come back too (`sysretq` would take them for `rip` and `rflags`). The HAL's entry to user
  mode already keeps only the flags a program may set (AArch64's NZCV, x86_64's user flags)
  and refuses a `rip` outside user space, so a forged frame cannot raise privilege.
- **`rt_sigreturn`** (returning 0 after a bad frame, as Linux; the x86_64 trap flag cannot be
  restored, as the way into user mode never sets it) reads the frame at the stack pointer
  (x86_64: 8 below it, since the handler's `ret` popped `pretcode`; AArch64: it must be
  16-byte aligned), restores the registers, the mask (never `SIGKILL` or `SIGSTOP`) and the
  alternate stack with `sigaltstack`'s rules, errors other than an unreadable frame ignored,
  as on Linux. It returns the restored result register, so the dispatcher's write changes
  nothing. A frame that is not the program's memory, or whose FP/SIMD records are not in
  Linux's shape, sends `SIGSEGV`.
- **Delivery** (research, Q4): every due signal with a handler gets a frame, each on top of
  the one before, the last one's handler running first, as Linux's return-to-user loop does;
  each handler blocks its `sa_mask` and, unless `SA_NODEFER`, its own signal, so a signal the
  first handler blocks stays pending. `SA_RESETHAND` makes the action `SIG_DFL` as the signal
  is taken. `SA_ONSTACK` starts the frame at the alternate stack's top if the thread has one
  and is not on it; a nested frame that would run off the alternate stack's bottom fails. An
  alternate stack with `SS_AUTODISARM` is disabled on entry to any handler, as sigaltstack(2)
  says, and comes back from the frame with `rt_sigreturn`. After `rt_sigreturn`, the signals
  the restored mask unblocks are delivered.
- **A frame that cannot be written** sends `SIGSEGV` from the kernel (`SI_KERNEL`), as
  Linux's `force_sigsegv`: delivered whatever the program blocked or ignored (its action
  becomes `SIG_DFL` then), and by `SIG_DFL` if `SIGSEGV`'s own frame failed.
- **`siginfo_t`** (research, Q3): `si_signo`, `si_code` `SI_USER` for `kill`, `SI_TKILL` for
  `tkill` and `tgkill`, `SI_KERNEL` for the kernel's; `si_pid` the sender's process id and
  `si_uid` 0, as every process runs as root until credentials exist (M10). A signal pending
  twice keeps its first origin, as Linux keeps a standard signal's first `siginfo`.
- **The return trampoline** (research, Q1). RISC-V's `struct sigaction` has no restorer and
  glibc on AArch64 passes none (Rust's std inherits this), so Linux returns their handlers
  through the vDSO. EggOS has no vDSO yet, so `linux::trampoline` maps a private page the
  first time a handler needs it: read-only and executable, where `mmap` would put it, holding
  a `nop` then `rt_sigreturn`'s call (handlers return to the call; unwinders and gdb know
  these instructions, and some look one byte before the return address). Mapped at the first
  delivery, not at exec or `rt_sigaction` (Rust's std calls that at every startup), so a
  process that never runs a handler pays nothing, `linux_exec_load` included. A private page
  needs no "shared frame" exemption in teardown, `munmap` or `mprotect`. 4 KiB per process
  that runs a handler; the real vDSO, shared and mapped at exec, will take it over. Its
  address is kept: a program that unmaps or replaces the page breaks its own handlers'
  return, as with Linux's vDSO; `execve` must reset it with the actions.
  x86_64 has none: as on Linux, a handler without `SA_RESTORER` cannot return, so it never
  runs and the process gets `SIGSEGV` (musl and glibc always set it there).
- **FP/SIMD parts** (research, Q2). Programs get no FP/SIMD registers until D54 (PR #47),
  but every frame has its FP/SIMD part in Linux's shape, holding the initial state (all a
  program can have): x86_64's `FXSAVE` image (x87 control word 0x037f, `MXCSR` 0x1f80, its
  `fpstate` pointer set; Linux never passes a null one to 64-bit programs and crash
  reporters rely on it), AArch64's `fpsimd_context` record (which Linux's `rt_sigreturn`
  requires) and terminator, RISC-V's D state and an empty extensions header. `rt_sigreturn`
  checks the records' shape (AArch64: the FP/SIMD record, an optional `esr_context`, the
  terminator; RISC-V: no extensions) and ignores their values. With D54 the frames save the
  thread's real registers instead: a change to what is written, not to the layout.
- **Kept out of `Process`, and the table within 2 KiB.** `Process` is 240 bytes, 256 with its
  `Arc`, exactly a heap size class: one more word moves every process to the 512-byte class
  (D59's lesson). So the signals' origins and the trampoline's address live in the
  out-of-line block that already held the action table (`Caught`), made by a process's first
  action change. That table was exactly 2048 bytes, the heap's largest block (D30), so the
  block now keeps the actions field by field with the flags as 32-bit words (every flag the
  kernel keeps fits): 1816 bytes, still one 2 KiB block, not a frame (independent review's
  finding). Compile-time checks hold `Signals` at 48 bytes (so `Process` too) and `Caught`
  within a block. Only a handler sees an origin, so a signal sent before the process first
  changed an action reports `SI_USER`; they move to each thread with the pending signals when
  `clone` exists.
- **Cost** (same Mac, QEMU `-icount`, against `main`'s CI run): every benchmark within
  ±0.02%, apart from riscv64's two SBI timer benchmarks, which differ between local QEMU and
  CI on `main` too. Nothing was added to the system-call, page-fault or switch paths; the new
  test program is after `linux_start` (D62), so `linux_exec_load` does not copy it.
- **Tests.**
  - Host: the HAL's register get/set (x86_64's `iretq` re-entry); the signal state's rules
    (origins kept and dropped, the handler's mask, `SA_NODEFER`, `SA_RESETHAND`,
    `SS_AUTODISARM` disarming and a plain alternate stack kept, `rt_sigreturn`'s restore with
    Linux's ignored errors, forced signals); the trampoline's instructions; through the
    dispatcher, a handler's frame (arguments, `SI_TKILL`, the saved mask, alignment, the
    restorer or trampoline), `rt_sigreturn` restoring every register and the mask the frame
    holds, two signals stacked (the last one's handler first, back into the first's), the
    alternate stack with `SS_AUTODISARM` and `SI_USER`, `SIGSEGV` for a frame below an
    unmapped stack pointer and for `rt_sigreturn` without a frame (even blocked), and on
    x86_64 for a handler without `SA_RESTORER`.
  - QEMU, on aarch64, riscv64 and x86_64: the test program `linux_signal_handler` sets an
    alternate stack, installs a `SIGUSR1` handler with `SA_SIGINFO | SA_ONSTACK` (x86_64 with
    its restorer, the others through the trampoline) and sends itself `SIGUSR1`. The handler
    checks its arguments and that it runs on the alternate stack, sets a register in its
    frame and clobbers another; the program checks `tgkill` returned 0 with one register
    restored and the other changed, and nothing blocked. The self-test
    `linux_programs_run_signal_handlers` runs it twice and requires every frame of the second
    run freed (the trampoline's page too). With `rt_sigreturn`'s register restore broken, it
    fails (checked on x86_64).
- **Follow-ups** (the rest of D63's part 2): faults delivered to a handler (`si_addr`,
  `esr_context`, Rust's stack-overflow handler); real-time signals queued with their
  `siginfo` (`rt_sigqueueinfo`, `rt_sigtimedwait`); `rt_sigsuspend`; `ppoll`'s mask with
  `EINTR` and `SA_RESTART`; the FP/SIMD registers saved in frames once D54 lands; the vDSO.

## D65 Faults delivered to signal handlers: Linux's `si_code` and `si_addr`, x86_64's `err`, `trapno` and `cr2`, AArch64's `fault_address` and `esr_context` — Decided by the build loop (Linux's rules and the fault path's shape decided by research)
- **Why now.** Rust's std installs `SIGSEGV` and `SIGBUS` handlers on an alternate stack at
  startup to report stack overflows, and crash reporters, JITs, garbage collectors and
  language runtimes (Java, Go, .NET, JavaScript engines) catch `SIGSEGV`, `SIGBUS`, `SIGILL`,
  `SIGFPE` and `SIGTRAP` to work at all. Until now every fault ended the process by its
  signal's default action, whatever the program's handlers (D50, D64).
- **What a fault sends** (`linux::fault::classify`, research Q1): the signal, `si_code` and
  `si_addr` Linux gives for it, per architecture.
  - x86_64: `#PF` `SIGSEGV` with `SEGV_MAPERR` or `SEGV_ACCERR` at CR2; `#GP` `SIGSEGV`
    `SI_KERNEL` with no address; `#UD` `SIGILL` `ILL_ILLOPN` at `rip`; `#DE` `SIGFPE`
    `FPE_INTDIV` at `rip`; `int3` `SIGTRAP` `SI_KERNEL` with no address (`rip` is past it, so
    a handler that returns goes on); `int1` (`#DB`) `SIGTRAP` `TRAP_BRKPT`; `#NP` and `#SS`
    (a push with a non-canonical stack pointer) `SIGBUS` `SI_KERNEL`; `#AC` `SIGBUS`
    `BUS_ADRALN`.
  - AArch64: translation, access-flag and permission faults `SIGSEGV` at the FAR; alignment
    faults (data, PC, SP) `SIGBUS` `BUS_ADRALN` at the FAR, the PC or the stack pointer (the
    FAR is unknown after an SP alignment fault); synchronous external aborts `SIGBUS`
    `BUS_OBJERR`; `brk` `SIGTRAP` `TRAP_BRKPT` at the PC; undefined instructions, the FP/SIMD
    trap and every other class `SIGILL` `ILL_ILLOPC` at the PC.
  - RISC-V: page and access faults `SIGSEGV` at `stval`; misaligned accesses `SIGBUS`
    `BUS_ADRALN` at `stval`; `ebreak` `SIGTRAP` `TRAP_BRKPT` at the PC; illegal instructions
    and the rest `SIGILL` `ILL_ILLOPC` at the PC. Research was unsure whether Linux reports
    `sepc` or `stval` for access faults and misaligned accesses; sigaction(2)'s rule decides
    (`si_addr` is the faulting memory address for `SIGSEGV` and `SIGBUS`).
- **`SEGV_MAPERR` or `SEGV_ACCERR`** (research Q2) by the process's regions alone: an address
  in a region whose protection forbids the access (`PROT_NONE` included) is `SEGV_ACCERR`,
  any other, the stack's guard gap and the kernel's half included, `SEGV_MAPERR`. Looked up
  only once the page fault is known to be the program's, so demand paging pays nothing.
- **The machine context reports the exception** (research Q3; `linux::fault::Trap`):
  - x86_64's `sigcontext` has `err` (the error code), `trapno` (the vector) and `cr2`
    (page faults only). For an address outside user space, `err` always has the protection
    bit, mapped or not, as Linux does, so a program cannot learn what the kernel has mapped.
  - AArch64's has `fault_address` (`si_addr`) and an `esr_context` record (magic
    `0x45535201`, 16 bytes) after the FP/SIMD one, holding `ESR_EL1`. For an address outside
    user space the syndrome is sanitized as Linux does: the class and instruction length, and
    for an abort a level 0 translation fault (with, for a data abort, `WnR` and `CM`); 0 for
    any other class.
  - RISC-V's frame has no such fields.
  - Signals that are not faults report zeros there and no `esr_context` (Linux leaves the
    last fault's values, stale).
- **New HAL exception kinds**, so the HAL tells apart every vector a program can raise without
  costing anything (the kind is chosen per vector already): `Arithmetic` (`#DE`, and `#MF`
  and `#XM`), `Debug` (`#DB`) and `Segment` (`#NP`, `#SS`), all `Other` before. `trapno`
  comes from the kind: it cannot tell `#MF` and `#XM` from `#DE` (they cannot happen while
  programs have no FP/SIMD registers), `#NP` from `#SS` or `#NM` from `#UD` (D54 turns `#NM`
  into a first use). A first version added the vector to `Exception` (an extra field, then
  packed above x86_64's error code); either cost the kernel's own exception path 1 or 2
  instructions (`breakpoint_round_trip` +2 ns on AArch64, x86_64 59 to 61 ns), so neither
  was kept.
- **The fault path reads the exception back from the thread's context** (research, after
  measuring five shapes). Delivering a fault needs, after demand paging failed to resolve
  it, the program's registers, the raw cause and the unmasked address; keeping them in
  registers across the page's mapping cost the successful page fault too (up to +1% on
  `linux_page_fault`), and passing the `Exception` by reference made the thread's loop copy
  it on every exit (+3.9%) or moved the system-call round trips (+2.4% to +3.2%). So
  `run_user`'s way out of user mode, on its exception branch only, keeps the cause in the
  `UserContext` (x86_64: `CR2` in a padding word, the vector and error code being saved by
  the entry code already; AArch64: `ESR_EL1` and `FAR_EL1`; RISC-V: `scause` and `stval`),
  and the cold fault path decodes it again with `UserContext::last_exception`. That holds
  across preemption, a move to another CPU and the kernel's own exceptions (the CPU's cause
  registers would not, once a page-in can block). Only `context` stays live across the
  mapping, in place of the `kind` that ending the process needed before. x86_64's exit for a
  program counter outside user space is kept as the fetch's page fault (named `#PF` now).
  Host test platforms record theirs with `UserContext::simulate_exception`.
- **Forced, as Linux forces a fault's signal** (research Q4): a program that blocks or ignores
  it gets `SIG_DFL` and dies of it (`Signals::force`, D64), so does a fault inside a handler
  that blocks its own signal; a handler that returns without curing the fault runs again on
  the re-executed instruction, as on Linux; a frame that cannot be written (a stack overflow
  without an alternate stack) ends the process with `SIGSEGV`. Nothing else is due when a
  program faults (every due signal was delivered before it ran again), so the fault's signal
  is the one delivered and its frame is the one that describes it; no state is kept for it,
  and `Process` and `Signals` keep their sizes.
- **Out-of-memory page faults** still end the process with `SIGKILL`, as Linux's OOM killer.
- **Cost** (same Mac, QEMU `-icount`, against `main`'s CI run): `linux_page_fault` +0.12%
  on AArch64 (1375.25 to 1376.94), +0.20% on RISC-V (1401.51 to 1404.35) and +0.49% on
  x86_64 (1129.98 to 1135.48): the stores of the cause on the way out and the context
  passed to the fault path, all within the gate (1% + 0.5 ns), so the baseline is unchanged.
  Every other benchmark within ±0.05% (riscv64's two SBI timer benchmarks differ between
  local QEMU and CI on `main` too). The kernel's own exception path is unchanged
  (`breakpoint_round_trip` as on `main`), and so are the system-call paths.
- **Follow-ups:** `AT_MINSIGSTKSZ`; `#MF`, `#XM` and AArch64's trapped FP exceptions with
  D54; AArch64's top byte in `si_addr` with TBI; RISC-V misaligned accesses emulated as Linux
  does; `BUS_ADRERR` for file mappings (M7); `TRAP_TRACE` and `TRAP_HWBKPT` with
  `ptrace`; `/proc/self/maps` and `RLIMIT_STACK` for glibc's `pthread_getattr_np` (Rust's
  std then names a stack overflow).
- **Tests.**
  - Host: per architecture, every kind of exception ends a process without handlers with its
    Linux signal; a fault's signal blocked or ignored ends the process even with a handler;
    a handler runs for an unmapped read (`SEGV_MAPERR`), a write to a read-only region
    (`SEGV_ACCERR`) and a read in the kernel's half (`SEGV_MAPERR`, protection bit or
    sanitized syndrome), with `si_addr`, the machine context's fault fields, its signal
    blocked, and a return to the faulting instruction; a `tgkill`'s frame reports no fault;
    a frame below an unmapped stack pointer ends the process with `SIGSEGV`; AArch64's
    syndrome sanitizing; the HAL's `last_exception` after a simulated exit (on the CPU, the
    next exit of any kind replaces it).
  - QEMU, on aarch64, riscv64 and x86_64: the test program `linux_fault_handler` installs a
    `SIGSEGV` handler with `SA_SIGINFO`, maps a `PROT_NONE` page, reads an unmapped address
    and then that page; the handler checks the signal, `si_code` and `si_addr` against
    registers the program set (and `fault_address` with the `esr_context` record, or `cr2`
    with `trapno` 14), counts the fault in its frame and resumes the program past it by
    setting the frame's program counter. The self-test `linux_programs_handle_their_faults`
    runs it twice and requires an exit of 0 and every frame of the second run freed. With
    `si_addr` off by one it fails on all three.

## D66 Comments: rustdoc on public items only, built in CI; the rest removed crate by crate — Decided by the build loop (the rule's reading decided by research)
- **Why.** The owner's global rule of 2026-10-23 ("Code comments: none, the code explains
  itself") allows only doc comments a tool turns into documentation (Rust `///` and `//!` on
  public items of crates whose docs are generated), comments a tool reads (`// SAFETY:`, which
  clippy's `undocumented_unsafe_blocks` requires here; lint directives) and legal notices.
  EggOS had about 1,500 plain `//` comment lines besides its ~400 `// SAFETY:` lines, about
  4,450 doc-comment lines, and nothing that generated its docs.
- **Doc comments on public items stay, and the docs are now generated** (research). The
  playbook's rule names rustdoc among the allowed comments, and many public docs are real
  contracts (errors and their order, the man page or POSIX rule followed). `cargo xtask docs`
  builds every workspace crate's docs with rustdoc's `-D warnings`, passed as
  `CARGO_ENCODED_RUSTDOCFLAGS`, which flags set in the environment cannot override; `cargo xtask
  ci` and the CI job run it after clippy. Twenty-one doc links (most in public docs) named
  private items, a name out of scope or an ambiguous one, and were fixed to pass. xtask is a
  binary, whose docs rustdoc builds with its private items, so its doc comments count as
  generated too. No `missing_docs` lint: it would demand new comments everywhere, against the
  rule's intent. Cost: one step in the existing Linux job, about half a minute; no new job.
- **Not allowed:** plain `//` comments (but `// SAFETY:`), doc comments on private items
  (rustdoc does not show them by default), and `#` comments in TOML and YAML. Public docs keep
  to the contract; how the code works moves out.
- **Removal, crate by crate** (research): one cleanup PR per area, each a few hundred lines,
  information moved to names, constants, tests or this log first: drivers, xtask and the
  configuration files; then the HAL; then the kernel's modules. Files PR #47 (D54) changes come
  last, after it merges, so the cleanup does not conflict with it. Feature work cleans the
  files it changes in a commit of its own, or leaves a large file to its area's cleanup PR.
- **`CLAUDE.md`** says so under Rules.

## D67 Review of D63–D65: synchronous signals delivered first; AArch64 frames of `brk` and undefined instructions report no fault address or syndrome — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** The review step after three feature PRs (#56–#58): a fresh read of sending,
  delivery, signal frames and faults against Linux's observable behaviour. `force`,
  `SS_AUTODISARM` (disarmed on entry to every handler, never "on" the stack while disarmed),
  `rt_sigreturn`'s alternate-stack restore at the restored stack pointer with every error
  but an unreadable frame ignored, `SA_RESETHAND` resetting only the handler (Linux keeps
  `SA_SIGINFO`, unlike POSIX's wording), the `siginfo_t` offsets, x86_64's frame and
  `sigcontext`, and the red zone left out on the alternate stack all check out. Two things
  change.
- **Synchronous signals first** (research; a real bug). Delivery took the lowest number
  first. Linux takes the synchronous signals (`SIGILL`, `SIGTRAP`, `SIGBUS`, `SIGFPE`,
  `SIGSEGV`, `SIGSYS`) before any other, those the kernel raised before those a program sent,
  so that a fault's frame describes the faulting instruction (Linux commit a27341cd,
  "Prioritize synchronous signals over 'normal' signals", which Wine relies on; and "signal:
  Better detection of synchronous signals", for a failed frame's `SIGSEGV`). Here, with
  `SIGHUP` caught and `SIGUSR1` at its default both unblocked at once and the stack
  unwritable, `SIGHUP`'s frame failed, `SIGSEGV` was forced, and `SIGUSR1` (10) went before
  `SIGSEGV` (11): the process died of `SIGUSR1`, not `SIGSEGV`. With a `SIGUSR1` handler,
  `SIGSEGV`'s frame would have stacked on `SIGUSR1`'s and described its handler's entry.
  `Signals::next_delivery` now takes `SIGKILL` first (it ends the process whatever else is
  due), then the synchronous signals the kernel raised, then the other synchronous ones, then
  the lowest number. Not on any hot path: only the calls that make a signal due and faults
  deliver.
- **AArch64: `fault_address` 0 and no `esr_context` for `brk` and undefined instructions**
  (research; found by PR #58's independent review). Linux reports the PC in `si_addr` for
  these but leaves the machine context's fault address 0 and writes no `esr_context`, which
  only aborts and alignment faults carry. The frame wrote the PC and the raw syndrome. Now
  only data and instruction aborts and alignment faults report them; `si_addr` is unchanged.
  An alignment fault at an address outside user space keeps its class and instruction length
  in the sanitized syndrome (it had none, so its frame lost both fields; independent review).
  Whether Linux 6.4 and later pass the syndrome for undefined instructions, and what alignment
  faults and external aborts report, stay on the list to check against a real Linux under QEMU.
- **Left on the roadmap** (research): `AT_MINSIGSTKSZ` with D54, since the saved FP/SIMD state
  changes its value (Rust's std sizes its alternate stack as the larger of `SIGSTKSZ` and the
  entry, a missing one reading as 0, and every frame fits `SIGSTKSZ` today; `MIN_STACK_SIZE`
  matches Linux's `MINSIGSTKSZ`); x86_64's non-canonical `rip` from `rt_sigreturn` reported as
  `#GP` (it changes the HAL's x86_64 exit path, which PR #47 also changes); whether AArch64's
  `rt_sigreturn` sends `SIGSEGV` for a frame whose `pstate` is not EL0t (we mask it to the
  flags a program may set and continue).
- **Tests.** Host: the delivery order (`SIGKILL`, then a forced `SIGSEGV`, then a sent
  `SIGTRAP`, then `SIGHUP` and `SIGUSR1` by number); through the dispatcher, a failed `SIGHUP`
  frame with `SIGUSR1` pending ends the process with `SIGSEGV`; on AArch64, `SIGILL` and
  `SIGTRAP` handlers get the PC in `si_addr`, no fault address and no `esr_context`, and return.
  All three fail without the fixes. `cargo xtask qemu --arch all` passes on aarch64, riscv64
  and x86_64.

## D68 Real-time signals queued with their `siginfo_t`; `rt_sigqueueinfo`, `rt_tgsigqueueinfo` and `rt_sigtimedwait` — Decided by the build loop (the queue's shape, the limit and Linux's rules decided by research)
- **Why.** M6's next signal step. Until now a pending signal was one bit plus an origin
  (`kill`, `tkill` or the kernel), so a handler's `siginfo_t` could only say who sent it, a
  real-time signal sent twice was delivered once, and nothing could wait for a signal. glibc's
  `sigqueue`, `sigwaitinfo` and `sigtimedwait`, and every program that passes data with
  `si_value`, need the real thing.
- **One queue per process, in send order** (research). The out-of-line block a process gets on
  its first action change (D60) now also holds a `VecDeque` of 48-byte `siginfo_t` records
  (`si_signo`, `si_errno`, `si_code` and the union after them; a program always gets the rest of
  the 128 bytes as zeros); it is renamed `ActionsAndQueue`. A standard signal already pending is
  not queued again and keeps the info it was first sent with; each instance of a real-time
  signal (32 to 64) gets its own record, taken in the order sent. The pending bitmask stays the
  fast check; a signal's bit clears when its last record is taken. `Origin` and the two origin
  masks are gone: the record's `si_code` says it all, and the synchronous signals the kernel
  raised are those with a positive `si_code`. `Signals` stays 48 bytes, so `Process` stays in
  its heap size class (D64).
- **When info is kept** (research). A signal's info is kept if the signal is blocked or the
  process has its own block. A process with every action at `SIG_DFL` whose signal is not
  blocked delivers it by its default action before any program code could read the info, so it
  allocates nothing; a blocked signal in such a process allocates the block. A signal pending
  without a record is taken as Linux takes one: `SI_USER` from pid 0.
- **`RLIMIT_SIGPENDING` is enforced** (research), with one global counter of queued records,
  since every process runs as one user until processes have credentials (D59). As on Linux: a
  standard signal sent with `kill` or by the kernel (`si_code` 0 or more) is queued past the
  limit; any other signal at the limit, or with no memory for a record, is a problem only for a
  real-time signal not sent with `kill`, which fails with `EAGAIN` (`rt_sigqueueinfo`, `tkill`,
  `tgkill`); the others stay pending without their info. The counter moves with the records:
  queued, taken, discarded, and dropped with the process.
- **The calls.** `rt_sigqueueinfo(tgid, sig, info)` and `rt_tgsigqueueinfo(tgid, tid, sig, info)`
  read the program's `siginfo_t` and send it with `si_signo` set to `sig`, in Linux's error
  order: `EFAULT` for the buffer; for `rt_tgsigqueueinfo` `EINVAL` for an id that is not
  positive; `EPERM` for a `si_code` only the kernel and `kill` send (0 or more, or `SI_TKILL`)
  to anyone but the caller; `ESRCH` for any target but the caller (the only one it can reach
  yet); then `EINVAL` for a signal outside 0 to 64, and `EAGAIN` at the limit.
  `rt_sigtimedwait(set, info, timeout, sigsetsize)` takes the first pending signal in the set,
  blocked or not, in delivery order, copies its `siginfo_t` out and returns its number; with
  none pending it sleeps until the timeout (the `struct timespec` parsing is shared with
  `ppoll`), for good with a null one and not at all with a zero one, then fails with `EAGAIN`.
  Linux's order: `EINVAL` for `sigsetsize`, `EFAULT` for the set, `EFAULT` or `EINVAL` for the
  timeout, then `EFAULT` for `info` after the signal was taken (it is lost, as on Linux).
  `SIGKILL` and `SIGSTOP` in the set are ignored. Nothing can send the waiting thread a signal
  until there are other processes or threads, so a wait is slept out.
- **Known deviation** (research): Linux's `E2BIG` for a queued `siginfo_t` whose unknown
  `si_code` layout has non-zero bytes past the kept part is not checked; those bytes are dropped.
  Only the 48 kept bytes are read from the program, as Linux reads only its kernel part for a
  known layout (independent review).
- **For later** (independent review): once exited processes wait as zombies (`wait4`), drop the
  queue at exit, not when the `Process` is freed, so a zombie holds no `RLIMIT_SIGPENDING` charge;
  and a host test that the global counter returns to where it started, `Drop` included, once a
  test can run without other tests sending signals in parallel.
- **Cost.** Nothing on the system-call, switch or fault paths changes: the queue is touched only
  by the calls that send, take or discard a signal. The benchmarks show no change beyond noise.
- **Tests.** Host: queueing order, standard signals kept once with their first info, the limit's
  charge and refund, discards (`SIGCONT`, `SIG_IGN`) dropping records, `rt_sigtimedwait` taking
  synchronous signals first and never `SIGKILL` or `SIGSTOP`; through the dispatcher, both
  queueing calls' errors in Linux's order, `rt_sigtimedwait`'s errors, timeout and lost signal,
  a full queue refusing real-time signals but `kill`'s, and a handler getting a queued signal's
  `si_value`. QEMU: a new test program, `linux_signal_queue`, blocks signal 40, queues it twice
  with `rt_sigqueueinfo` and sends it once with `kill`, then `rt_sigtimedwait` must return the
  three in order with their `siginfo_t` and a fourth call `EAGAIN`; it passes on aarch64,
  riscv64 and x86_64.

## D69 Fast to get: download in seconds, install in seconds, ready right away — Decided (owner, 2026-10-23)
A must-have, with the same weight as the fastest (D13) and the lightest (D24): getting EggOS is
faster than getting any other OS. The user downloads it in seconds, installs it in seconds, and is at
a usable system right after.

- **Download in seconds.** The installer image carries only the base (D18), compressed, so it
  downloads in seconds on a typical home connection. Optional components are not in it; the
  component manager fetches them when the user picks them, at setup or later, in parallel with the
  rest of the install.
- **Install in seconds.** Installing writes a ready-made base image to the disk instead of
  unpacking and configuring packages one by one: no dependency resolution and no long
  post-install scripts at install time. The installer detects the hardware, asks only what it
  cannot decide itself (everything else gets a sensible default the user can change later), and
  the base installs fully offline from the image. The image carries the base's drivers compactly,
  and hardware detection at install time picks the ones the machine needs (D18's "drivers for the
  hardware present").
- **Ready right away.** The first boot goes straight to a usable system (D24's boot budget).
  Anything that can wait (component downloads, indexing, updates) runs in the background and never
  blocks the user.
- **Updates the same way.** The base updates with small deltas applied atomically (the running
  system is never left half-updated), with no "installing updates" screen at boot. Optional
  components update, install and remove one by one, as D18 says.

**Order.** M10's first installer installs the base only; components on demand arrive with the
component manager (M14).

**Measured like D24.** Every release measures and publishes the installer image's download size, the
time from starting the install to the first usable boot, and the first boot's time to a usable CLI
and GUI, on QEMU and on reference hardware once there is some. The numbers may only go down; any
growth needs a stated reason in the PR and is handled like a performance regression. Concrete
budgets are set when the base first installs (M10), from measurements against current Linux,
Windows and macOS installers on the same hardware and connection.

The installer is something users see, so its design comes from Claude Fable (D16).

## D70 Waiting for a signal: `rt_sigsuspend` and `pause` — Decided by the build loop
The C libraries' `sigsuspend` and `pause` are these calls, and programs that wait for a signal
in a loop (shells waiting for children, daemons waiting for `SIGTERM` or `SIGHUP`) use them, so
they are next in M6 after D68's queued signals.

- **`rt_sigsuspend(mask, sigsetsize)`** replaces the thread's blocked mask with the set at `mask`
  (never `SIGKILL` or `SIGSTOP`), then delivers what is due under it until a signal starts its
  handler, and fails with `EINTR`. As on Linux, the first handler's frame saves the mask from
  before the call, so `rt_sigreturn` puts it back; a second handler's frame, stacked on top,
  saves the mask the first one runs with. A due signal whose action ignores it, or that stops
  the thread, does not end the wait: Linux restarts the call after one (`-ERESTARTNOHAND` with
  no handler). A signal whose default action ends the process ends it there. Errors as Linux,
  with the mask unchanged: `EINVAL` if `sigsetsize` is not 8, then `EFAULT` for the set.
- **`pause()`** (x86_64 only; the generic table has no number for it, and AArch64's and RISC-V's
  C libraries call `rt_sigsuspend` with the current mask) waits the same way with the thread's
  own mask.
- **No new state.** The mask to restore stays on the kernel stack of the call: the delivery loop
  (`deliver_with`) takes it as an argument and hands it to the first frame it writes, so
  `Process` and `Signals` keep their sizes (256 and 48 bytes).
- **Waiting.** Nothing can send the thread a signal while it waits yet (one thread per process,
  no other process can reach it), so with nothing due under the new mask the wait lasts for good,
  as on Linux when no signal comes, the same as `rt_sigtimedwait` with a null timeout (D68). Waking
  the waiter comes with the first sender from outside the thread (other processes, `clone`,
  timers).
- **Cost.** The two calls sit in `out_of_line_call`, checked before the other calls' match, so the
  system-call path's inlined part is unchanged. The benchmarks are the same as `main`'s.
- **Tests.** Host, through the dispatcher: two blocked, pending signals unblocked by the call's
  mask each get a frame, the lower one's saving the mask from before the call and the upper
  one's the mask its handler runs under, `EINTR` returned with every register as it was and
  the old mask back after both handlers return; the errors in Linux's order with the mask
  unchanged; a pending `SIGTERM` unblocked by the mask ending the process. QEMU: a new test
  program, `linux_signal_suspend`, blocks `SIGUSR1`, sends it to itself and calls
  `rt_sigsuspend` with an empty set; its handler checks that the frame saved `SIGUSR1` as the
  old mask and changes a register in the frame; the call must fail with `EINTR`, the register
  must hold the change and `SIGUSR1` must be blocked again. It passes on aarch64, riscv64 and
  x86_64.
- **For later.** `ppoll`'s and `pselect6`'s signal masks during the wait use the same
  "mask to restore" handoff (the frame saves the caller's mask when the wait is interrupted).

## D71 `ppoll`'s signal mask applies during the call — Decided by the build loop
D61 only checked `ppoll`'s `sigmask`. Programs pass one so that a signal they keep blocked
elsewhere can interrupt the wait: the race-free way to wait for descriptors or a signal, the
reason `ppoll` exists. With D70's handoff in place, it now does what Linux does:

- **The mask applies for the whole call.** After the timeout's and the mask's checks (unchanged:
  D61's order), the set at `sigmask` is blocked instead of the thread's mask (never `SIGKILL` or
  `SIGSTOP`). A null `sigmask` leaves the thread's mask alone.
- **Interrupted when nothing is ready.** If no descriptor reports events and a signal is due
  under the call's mask, the wait does not start, even with a zero timeout (Linux checks
  `signal_pending` before its `timed_out` exit). The returned events and the time left are
  written back as for any other return, then the signal is delivered. Once a handler starts,
  the call fails with `EINTR` (Linux's `-ERESTARTNOHAND`, whatever `SA_RESTART` says), and the
  first frame saves the thread's own mask, which `rt_sigreturn` restores. A signal that ends the
  process ends it there. A signal its action ignores is discarded and the call goes on, the way
  Linux restarts it, and waits out what is left of its timeout.
- **Otherwise the thread's mask is back when the call returns**, with any result: descriptors
  ready, the timeout passed, or an error such as `EFAULT` for the array. A signal the call's mask
  unblocked while descriptors were ready stays pending, as on Linux.
- **x86_64's `poll`** has no mask, and every signal due under the thread's own mask was
  delivered before the call, so it is not interrupted.
- **Shared with D70.** `signals` gains `replace_blocked`, `is_due` and `deliver_restoring_mask`,
  which `rt_sigsuspend`, `pause` and `ppoll` use. The mask to restore stays on the call's stack,
  with no new state per process or thread.
- **Cost.** `ppoll` moved to the early match in `out_of_line_call`, next to `rt_sigsuspend`, so
  the inlined system-call path is unchanged. The benchmarks are the same as `main`'s.
- **Tests.** Host, through the dispatcher: a ready descriptor returns 1 with the mask back and
  the signals still pending; with nothing to poll, the mask unblocks a handled `SIGUSR1`, whose
  handler starts with a frame saving the thread's mask, then `EINTR` with every register as it
  was, and the `SIGCHLD` due with it discarded; an ignored signal alone lets a 2 ms wait run out
  (result 0, a zero time left written back); a default Term action ends the process even with a
  zero timeout; an error after the mask was set (more descriptors than the open-file limit)
  restores the thread's mask. In QEMU, the frames and the handoff are D70's, already covered by
  `linux_signal_suspend`, and `linux_poll` checks `ppoll` without a mask on every architecture.
- **For later.** `pselect6` with the same handoff (M7, with `select`); signals sent during a
  wait waking it, once a sender from outside the thread exists.

## D72 Review of D68–D71: `ppoll` with a read-only timeout fails with `EINTR`; `rt_sigtimedwait`'s order; kernel-raised signals in the order raised; a drained queue shrunk — Decided by the build loop
- **Why.** The review step after three feature PRs (#62, #64, #65), with a fresh read by an
  independent reviewer. The error orders of the queueing calls, `rt_sigtimedwait` and
  `rt_sigsuspend`, the masks the frames save (the first the mask from before the call, the
  next ones the previous handler's), `ppoll`'s write-back and mask restore, the
  `RLIMIT_SIGPENDING` rules and the `QUEUED_SIGNALS` charge on every path, and the test
  programs all check out. Four things change.
- **`ppoll` interrupted by an ignored signal, with a timeout it cannot write back, fails with
  `EINTR`.** Linux restarts an interrupted `ppoll` whose handler did not run, with the time
  left written back to the program's timeout; when that write fails, it cannot restart with
  the time left, so it fails with `EINTR` instead (the "sticky" timeout path of its poll
  code). Here the call went on and waited out its timeout. A timeout too long to count in
  nanoseconds is now written back as the program wrote it (Linux writes the time left, which
  differs from it only by the time slept), so a read-only one is noticed as on Linux.
- **`rt_sigtimedwait` takes the lowest synchronous signal, whoever raised it.** D67's
  preference for the synchronous signals the kernel raised belongs to delivery only; Linux's
  `sigtimedwait` takes them by number. With `SIGSEGV` queued with a kernel `si_code` (which a
  program may queue to itself) and `SIGILL` sent with `kill`, both blocked, the call returned
  `SIGSEGV`; Linux returns `SIGILL`.
- **Kernel-raised synchronous signals are delivered in the order raised**, the first record
  in the queue with a positive `si_code`, as Linux does, not the lowest number among them.
  One deliberate difference: Linux may take such a record for a blocked signal once any
  synchronous signal is due (only a program queuing a kernel `si_code` to itself can make
  one); here only records of due signals are taken.
- **A drained queue gives its memory back.** The queue's buffer grows by doubling and never
  shrank, so a burst of tens of thousands of queued real-time signals left megabytes per
  process after they were taken (note 8: the lightest). When the last record is taken or
  discarded, it shrinks to room for 32 records, so a signal sent and taken at a time still
  needs no allocation.
- **Left on the roadmap** (independent review): a queue that grows by fixed-size chunks
  instead of one contiguous buffer, so a long queue never needs a large contiguous block (and
  never copies itself under the signals lock) before it reaches `RLIMIT_SIGPENDING`.
- **Cost.** None of these paths is hot: the benchmarks are the same as `main`'s.
- **Tests.** Host: `ppoll` with a read-only 2 ms timeout and only an ignored signal due fails
  with `EINTR`, the mask restored and the timeout unchanged; kernel-raised `SIGSEGV` then
  `SIGILL` are delivered in that order; `rt_sigtimedwait` takes `SIGILL` (sent) before
  `SIGSEGV` (kernel); the queue's capacity drops to at most 32 once drained, by taking or
  discarding. Each fails without its fix. `cargo xtask ci` passes on aarch64, riscv64 and x86_64.

## D73 Clocks and sleeping: `clock_gettime`, `clock_getres`, `gettimeofday`, `time`, `nanosleep`, `clock_nanosleep`; the wall clock starts at the epoch; no process sets it yet — Decided by the build loop (the wall clock's source, the clock ids and Linux's rules decided by research)
- **Why.** M6's next startup step. Rust's std reads `CLOCK_MONOTONIC` for `Instant` and
  `CLOCK_REALTIME` for `SystemTime` and sleeps with `nanosleep`; the C libraries' `time`,
  `gettimeofday`, `clock_gettime` and `sleep` reach the same calls when there is no vDSO, as
  here. None existed: every one answered `ENOSYS`.
- **One clock, in nanoseconds.** Every clock reads the monotonic clock
  (`Platform::clock_ticks`) times a 32.32 fixed-point nanoseconds-per-tick factor, rounded up
  and computed once: one multiplication, no division, 7 to 11 instructions over the raw clock
  read (benchmark `linux_clock_monotonic_read`). Rounding the factor up means a reading never
  shows less time passing than the ticks did, so a program that sleeps for a duration always
  measures at least that duration.
- **The wall clock starts at the Unix epoch** (research), as Linux's does on a machine with no
  real-time clock driver: `CLOCK_REALTIME` reads the same as `CLOCK_MONOTONIC`. Reading a
  real-time clock now would take QEMU's fixed device addresses into early boot, which ties the
  core to one machine; the PL031, Goldfish and CMOS clocks come as ordinary drivers after the
  driver model (M5) and set the wall clock's offset at boot. Nothing a program can do yet
  depends on the date (no files, M7, no network, M8).
- **The clocks** (research): `REALTIME`, `MONOTONIC`, `BOOTTIME` (no suspend) and `TAI` (TAI
  offset 0) with their `_RAW` and `_COARSE` variants, which read the same precise value (a
  coarse clock may only lag). `EINVAL` for the CPU-time clocks (process, thread and the
  dynamic `-pid` ids: there is no per-thread CPU time yet, and measuring it costs the thread
  switch, a benchmarked hot path; glibc's and musl's `clock()` then return `-1`, which POSIX
  allows, and Rust's std never uses them), the alarm clocks (Linux's need a real-time clock
  device), and every other id. `clock_getres` reports 1 ns for each, as Linux does for its
  high-resolution clocks; the coarse ones read precisely, so 1 ns is true for them too.
- **Sleeping** (research): `nanosleep` and `clock_nanosleep` on the realtime, monotonic,
  boot-time and TAI clocks (`EOPNOTSUPP` on the raw, coarse and thread CPU-time ones, and on
  the alarm clocks after the request's checks, as Linux without a real-time clock), relative or
  with `TIMER_ABSTIME` (other flag bits ignored, as on Linux), in Linux's error order:
  `EINVAL` for the clock, `EOPNOTSUPP`, `EFAULT` for the request, `EINVAL` for negative seconds
  or nanoseconds outside 0 to 999999999. The deadline is computed in nanoseconds and woken at
  the first tick that reads at or past it, so a sleep never ends early; one too long to count
  lasts for good; an absolute deadline already passed returns at once. Nothing can send the
  sleeping thread a signal yet, so a sleep is never interrupted and `rem` is never written;
  `EINTR` with `rem` comes with the first sender from outside the thread.
- **Setting the clock** (research): `clock_settime` and `settimeofday` make Linux's checks
  (`EINVAL` for any clock but `CLOCK_REALTIME`, `EFAULT`, `EINVAL` for a bad time or one past
  Linux's latest settable second, 30 years of uptime short of the 64-bit nanosecond limit, and for
  `settimeofday` its microseconds and time zone checks in Linux's order) and then answer
  `EPERM`: no process may set the wall clock until processes have credentials
  (`CAP_SYS_TIME`), as none may raise a hard resource limit (D59). The clock is shared by
  every process, and an agent confined to its own permissions (D21) must not move it.
- **`time`** is x86_64's only (the generic table has none). `gettimeofday` writes a zeroed
  time zone (UTC).
- **Cost.** The calls sit in `out_of_line_result`; the system-call path's inlined part is
  unchanged and the round-trip benchmarks are the same as `main`'s. New benchmark:
  `linux_clock_monotonic_read`, 12 (aarch64), 15 (riscv64) and 19 (x86_64) instructions.
- **Tests.** Host, through the dispatcher: every clock reads forward and reports 1 ns, the
  refused clocks `EINVAL` before `EFAULT`, unwritable buffers `EFAULT`; `gettimeofday` in
  microseconds with a zeroed time zone, x86_64's `time`; `nanosleep` and both kinds of
  `clock_nanosleep` never wake early, with Linux's errors in order, the alarm clocks included;
  the setting calls' checks then `EPERM`; and, at frequencies from 1 MHz to 3.7 GHz, the first
  tick at a time reads at least it and the tick before reads less. QEMU: a new test program, `linux_clock`, reads `CLOCK_MONOTONIC`, sleeps 1 ms
  with `nanosleep`, reads it again and checks that at least 1 ms passed, then checks that
  `CLOCK_PROCESS_CPUTIME_ID` is `EINVAL`; it passes on aarch64, riscv64 and x86_64.
- **For later.** Real-time clock drivers setting the wall clock (after M5); a vDSO for clock
  reads without a system call (it needs the counter readable from user mode again, which D49
  turned off: `CNTKCTL_EL0`, `scounteren`, `CR4.TSD`); per-thread CPU time for the CPU-time
  clocks, `getrusage` and `times`, with the owner's OK on its switch-path cost; the invariant-TSC
  check, so `CLOCK_MONOTONIC` never goes backwards when a thread moves between x86_64 CPUs;
  absolute realtime sleepers re-evaluated once the wall clock can be set; `EINTR` and `rem`
  once signals can arrive during a sleep.

## D74 The console's status: `fstat`, `newfstatat` and `statx` on the standard descriptors; `lseek` refused — Decided by the build loop (the console's values, the lookup rules and the scope decided by research)
- **Why.** M6's next startup step. glibc's stdio calls `fstat` (since 2.33 on 64-bit as
  `newfstatat(fd, "", buf, AT_EMPTY_PATH)`) on a stream before its first write: a character
  device that is a terminal is line-buffered, and `st_blksize` sizes the buffer. musl calls
  `fstat`; Rust's std calls `statx` first (probing for it with a null path and buffer, taking
  only `EFAULT` to mean it exists) and falls back to `fstatat`. `fseek` and `ftell` on a
  terminal reach `lseek` and expect `ESPIPE`. Every one of these answered `ENOSYS`.
- **The console's status** (research): what `stat /dev/console` shows on a Linux distribution
  with devtmpfs, the device Linux opens on 0, 1 and 2 for `init` (D61): a character device
  (`S_IFCHR | 0600`, 0o020600), device number 5:1 (`st_rdev` 0x501 in the C libraries'
  `dev_t` encoding), one link, owned by root (uid and gid 0), size and blocks 0, `st_blksize`
  4096 (the page size, as on devtmpfs). The device files' file system is the anonymous device
  0:5, devtmpfs's usual number, and the console's inode 5, neither 0 (taken by some programs
  for "unknown") nor 1 (a file system's root); both are named constants
  (`stat::DEVICE_FILES`, `stat::CONSOLE_INODE`) for M7's `/dev` to keep. Every time is 0: the
  node exists from boot, and boot is the epoch on the wall clock (D73). The three descriptors
  report the same status: they are one open file.
- **`statx`** (research): `stx_mask` 0x7ff (`STATX_BASIC_STATS`) whatever the caller asked, as
  `statx(2)` allows; not `STATX_MNT_ID`, as there is no mount and so no honest mount id, and
  not `STATX_BTIME`, so Rust's `created()` says it is not available. Attributes and their mask
  0; the device numbers as major and minor fields; all 256 bytes written.
- **Lookups** (research), as Linux: flags first (`EINVAL` for any but `AT_SYMLINK_NOFOLLOW`,
  `AT_NO_AUTOMOUNT` and `AT_EMPTY_PATH`, and for `statx` its reserved mask bit and both
  synchronisation bits at once, before the path is read, so Rust's probe gets `EFAULT`), then
  the path copied (`EFAULT`, or `ENAMETOOLONG` with no end within 4096 bytes; chunks never
  cross a page, so nothing past the end is read), then an empty path `ENOENT` without
  `AT_EMPTY_PATH` and the descriptor itself with it (`EBADF` if not open, `ENOENT` for
  `AT_FDCWD`, as there is no working directory yet). A null path with `AT_EMPTY_PATH` is
  accepted without being read, as Linux has since 6.11, though `uname` says 6.1 (D55): it is
  strictly more permissive and skips a copy. Any other path finds no file: `ENOENT` (absolute
  or from `AT_FDCWD`), `ENOTDIR` relative to the console, which is not a directory, `EBADF`
  relative to a descriptor that is not open. `ENOENT` rather than `ENOSYS`: it is true, and a
  program probing for an optional file carries on, where `ENOSYS` would make Rust's std give
  up on `statx`. `EFAULT` for the result buffer comes last.
- **`lseek`** (research): `EBADF`, then `EINVAL` for a `whence` past `SEEK_HOLE` (an `unsigned
  int`), then `ESPIPE`; the offset is never looked at. To confirm against a running Linux: that
  `EINVAL` for `whence` comes before `ESPIPE` on a terminal, and whether `newfstatat` accepts
  `statx`'s synchronisation bits (0x2000, 0x4000), which `fstatat(2)` does not list and this
  refuses with `EINVAL`.
- **Left for the descriptor table (M7)** (research): `fcntl`, `close`, `dup` and their kin
  change or report per-descriptor state (`FD_CLOEXEC`, a closed descriptor), which needs the
  table; `read` waits for console input (M8), as D61 decided. For the record, the console's
  `F_GETFL` will be `O_RDWR | O_LARGEFILE` (0x8002 on x86_64 and RISC-V, 0x20002 on AArch64).
  `pread64` and `pwrite64` (`ESPIPE` on the console) come with them.
- **Cost.** The calls sit in `out_of_line_result`; `number::LAST` rises to `statx` (332 on
  x86_64, 291 in the generic table), still one compare. The system-call path's inlined part is
  unchanged, and every benchmark matches `main`'s CI run within 0.3 ns.
- **Tests.** Host, through the dispatcher: the host's `struct stat` layout field by field
  (x86_64's in CI's `check` job, the generic one on an AArch64 host) on 0, 1 and 2 (and a
  descriptor with its upper half set), `newfstatat` with an empty and a null path
  writing the same bytes as `fstat`, `statx`'s fields and mask whatever was asked, every error
  in Linux's order (Rust's probe included), paths read to their end and no further,
  `ENAMETOOLONG` at 4096 bytes, `lseek`'s three answers, and the `dev_t` encoding of large
  numbers. QEMU: a new test program, `linux_stat`, checks `fstat(1)`, `newfstatat(1, "",
  AT_EMPTY_PATH)` and `statx` report the character device 5:1, then that `lseek` is `ESPIPE`
  and `fstat(3)` `EBADF`; it passes on aarch64, riscv64 and x86_64.
- **For later.** The terminal's access and change times updated as it is used (`w` reads them
  for idle time), with console input (M8); the console as `/dev/console` in M7's device file
  system, keeping this status; `STATX_MNT_ID` once there are mounts.

## D75 Credentials: user and group ids, supplementary groups, `getuid`, `setuid`, `setresuid`, `setgroups` and their kin; root may raise hard limits — Decided by the build loop (the storage, the privilege model, Linux's rules and the scope decided by research)
- **Why.** M6's next step. Every process ran as an unnamed root: the auxiliary vector said
  uid 0, `getuid` and its kin answered `ENOSYS`, and D59 and D73 left "until processes have
  credentials" behind. Shells, `id`, `sudo`-like tools and daemons that drop privileges call
  these at startup.
- **What a process has.** Real, effective, saved and file system user ids, the same four
  group ids, and its supplementary groups, kept sorted with duplicates, as Linux keeps them.
  A process starts as root, as Linux starts `init`: every id 0, no groups.
- **Privilege is "effective user id 0"** (research). With no file capabilities, `capset`,
  `PR_SET_KEEPCAPS` or securebits, capabilities(7)'s rules for uid changes reduce to exactly
  that for `CAP_SETUID`, `CAP_SETGID` and `CAP_SYS_RESOURCE`: the effective set is cleared when
  the effective uid leaves 0 and restored from the permitted set when it returns, and the
  permitted set is lost only once no real, effective or saved uid is 0, after which no
  unprivileged call can return to 0. The group calls test the effective uid too. When
  capabilities come, this becomes explicit permitted and effective sets; file permission
  checks (M7) will use the file system uid, since `setfsuid` to a non-zero id drops the file
  capabilities.
- **The rules** (research, from setuid(2), setreuid(2), setresuid(2), setfsuid(2),
  setgroups(2), getgroups(2), credentials(7)), the same for users and groups:
  - `setuid(id)`: `EINVAL` for -1; privileged, every id becomes `id`; otherwise the effective
    and file system ids, if `id` is the real or saved id; else `EPERM`.
  - `setreuid(real, effective)`, -1 to keep one: unprivileged, the real id may become the real
    or effective id and the effective id any of the three; the saved id becomes the new
    effective id when the real id is given or the effective id is given and differs from the
    old real id.
  - `setresuid`: unprivileged, each given id must be one the process holds.
  - The file system id follows the effective id on each of these. `setfsuid(id)` always
    returns the old file system id, changing it only when privileged or `id` is held (real,
    effective, saved or file system), never to -1.
  - `getresuid` writes real, effective, then saved, `EFAULT` at the first bad pointer.
    `getgroups(size, list)`: `EINVAL` for a negative `size` or one below the count unless 0,
    which returns the count. `setgroups`: `EPERM` unless privileged, then `EINVAL` above
    `NGROUPS_MAX` (65536), `ENOMEM`, `EFAULT`; 0 empties the list without reading it.
  - A call that fails changes nothing. Ids are 32-bit: registers' upper halves are ignored.
- **Storage** (research). Per process, as the one thread a process has; Linux keeps them per
  thread, so at `clone(CLONE_THREAD)` they become a per-thread record shared until a thread
  changes it (glibc and musl apply set*id calls to every thread themselves). Root's are a
  shared static; a process's own record is made on its first change (`ENOMEM` then), as D59's
  limits. `Process` is exactly the heap's 256-byte class, so the credentials share the
  resource limits' lock (`CredentialsAndLimits`; `setrlimit` checks one and changes the other
  under it), and the program break's start became a `NonZeroU64` so its `Option` costs
  nothing: `Process` stays 240 bytes. The supplementary groups are copied to and from the
  program 64 at a time through a buffer on the stack, so the lock is never held across a copy.
- **Raising hard limits** (research): a privileged process may now raise its hard limits
  (D59's `CAP_SYS_RESOURCE`), up to `fs.nr_open` for open files. As every process starts as
  root, a lowered hard limit stays final only for a process that has given up root, as on
  Linux.
- **`execve`'s step and the auxiliary vector** (research): at a program's start the saved and
  file system ids become the effective ones; `AT_UID`, `AT_EUID`, `AT_GID` and `AT_EGID` come
  from the credentials, and `AT_SECURE` is 1 when an effective id differs from the real one.
  Set-uid and set-gid files come with M7. Signals sent with `kill` and `tgkill` carry the
  sender's real uid in `si_uid`.
- **Left for later** (research): setting the wall clock stays `EPERM` even for root (D73): it
  needs a settable clock and its own decision, as the clock is the whole machine's. Permission
  checks for signals to other processes wait for a process table (an unprivileged sender's
  real or effective uid must match the target's real or saved uid). Linux also resets the
  dumpable flag and the parent-death signal when an effective or file system id changes, and
  marks a process whose real uid changed so its next `execve` checks `RLIMIT_NPROC`: each
  comes with `prctl`, parents and per-user process counts. To confirm against a running Linux:
  `setgroups`' `EPERM` before `EINVAL` and its sorted list; whether `getresuid` with a bad
  second pointer has written the first id; whether `setresuid(-1, -1, -1)` resets a file
  system id set by `setfsuid`.
- **Cost.** The calls sit in `out_of_line_result`, below `number::LAST`: the system-call,
  switch and page-fault paths are unchanged. `linux_exec_load` matches `main` on aarch64 and is
  8 ns (riscv64) and 24 ns (x86_64) above it, 0.02% and 0.08%, from the per-process state
  built and dropped with each process; root's after-exec step is two constants.
- **Tests.** Host: each rule as a pure function on the ids (privileged and not, -1, the saved
  id's update), the record made only on a real change, groups sorted, `after_exec`, and a
  privileged process raising a hard limit but never past `fs.nr_open`; through the dispatcher,
  every call with its errors in order (`EINVAL`, `EPERM`, `EFAULT`, upper halves ignored). QEMU:
  a new test program, `linux_credentials`, checks `geteuid()` is 0, `setuid(1000)` succeeds,
  `getuid()` is then 1000 and `setuid(0)` fails with `EPERM`; it passes on aarch64, riscv64 and
  x86_64.

## D76 Review of D73–D75: `setgroups` refuses group id -1; `setresuid` that changes nothing keeps the file system id; `newfstatat` takes `statx`'s synchronisation flags — Decided by the build loop (each Linux behaviour decided by research)
- **Why.** The review step after three feature PRs (#67, #68, #69). The system-call numbers on
  both tables, the clocks' error orders and rounding, the `struct stat` and `struct statx`
  layouts, the path lookup, `lseek`, the id rules of `setuid`, `setreuid` and `setfsuid`, the
  privilege model, `AT_SECURE`, `si_uid` and root's hard limits all check out. Three things
  change, two of them the "to confirm" items D74 and D75 left.
- **`setgroups` refuses (gid_t) -1** with `EINVAL`, as Linux does: -1 is no group in any user
  namespace (`user_namespaces(7)`), and Linux's id check fails for it. The words are checked
  in list order, so a -1 before a word that is not the program's memory is `EINVAL` and one
  after it `EFAULT`. Words are still copied 64 at a time; only when a copy fails are its words
  copied one by one, to find the first that cannot be read. Here -1 was accepted as a group.
- **`setresuid` and `setresgid` that change nothing leave every id as it is.** Since Linux 6.4,
  backported to 6.1.y (research, from the patch's description and the stable trees), a call
  whose every id is -1 or the one it has, with the effective id also equal to the file system
  id, returns 0 before anything else. So `setresuid(-1, -1, -1)` keeps a file system id
  `setfsuid` set; here it reset it to the effective id. `setuid` and `setreuid` have no such
  shortcut on Linux: they still reset it.
- **`newfstatat` takes `AT_STATX_FORCE_SYNC` and `AT_STATX_DONT_SYNC`**, either or both, as
  Linux has since 4.11, when `fstatat` began to share `statx`'s flag check (research); only
  `statx` refuses both at once. `fstatat(2)` does not list them; this refused them with
  `EINVAL`. They change nothing here: there is no remote file system to ask.
- **For `execve` (M7 and later).** `exec::load` applies the after-exec ids (saved and file
  system ids from the effective one) before the stack and segments are mapped. That is sound
  while a failed load drops the process, as every caller does today; an `execve` system call
  that returns to the old program on failure must apply them only once it cannot fail.
- **Cost.** None of these paths is hot; `setgroups`' copies are unchanged unless one fails.
- **Tests.** Host: `setgroups` with -1 first, -1 in the middle, -1 just before an unmapped
  page and a word straddling one (`EINVAL`, `EINVAL`, `EFAULT`), the groups unchanged after
  each; `setresuid` no-ops keeping a different file system id, and the effective id given
  equal to it but unequal to the file system id resetting it; `setresuid(-1, -1, -1)` through
  the dispatcher after `setfsuid`; `newfstatat` with 0x2000, 0x4000 and 0x6000 writing the
  same bytes as `fstat`. Each fails without its fix.

## D77 `madvise` and `mremap` on anonymous memory: pages discarded, populated and moved with their frames — Decided by the build loop (Linux's rules and the deviations decided by research)
- **Why.** The calls memory allocators make while a program runs: glibc's `malloc`, musl's
  mallocng, jemalloc and mimalloc give memory back with `madvise(MADV_DONTNEED)` or
  `MADV_FREE`, and `realloc` of a large block grows it with `mremap` instead of copying.
  Linux 6.1's numbers: `mremap` 25 and `madvise` 28 on x86_64, 216 and 233 on the generic
  table.
- **`madvise`** (research, Linux 6.1 with a distribution's configuration). Checked in Linux's
  order: an advice Linux does not know (5–7, above 25 but 100 and 101), an unaligned address,
  a length that wraps when rounded up to pages or past the end of the address space are all
  `EINVAL` (unlike `mmap`, where a wrapping length is `ENOMEM`); then an empty range is 0.
  A range partly in no region gets the advice where it has regions and fails with `ENOMEM`
  afterwards, for every advice (`MADV_POPULATE_*` too: failing at the first hole came only in
  2024). Each advice:
  - `DONTNEED`, `DONTNEED_LOCKED` and `FREE` unmap the pages and free their frames, a
    `PROT_NONE` region's kept frames too; the regions stay, and their pages are mapped,
    zeroed, when next touched. `FREE` may discard later on Linux; with nothing to reclaim
    memory here, now is the only time it goes back. Touching regions are freed as one run.
  - `POPULATE_WRITE` maps a zeroed frame at every page of the range not mapped yet
    (`ENOMEM` when frames run out, so a program learns it up front); `POPULATE_READ` only
    checks, as an unmapped page reads zeros all the same, until there is a shared zero frame.
    A region that does not allow the access stops the call with `EINVAL` (a `PROT_NONE`
    region, or a write to a read-only one).
  - `NORMAL`, `RANDOM`, `SEQUENTIAL`, `WILLNEED`, `COLD`, `PAGEOUT`, `MERGEABLE`,
    `UNMERGEABLE`, `HUGEPAGE`, `NOHUGEPAGE`, `DONTDUMP`, `DODUMP`, `DONTFORK`, `DOFORK`,
    `WIPEONFORK` and `KEEPONFORK` succeed and change nothing: there is no swap, no read-ahead,
    no page merging, no huge page, no core dump and no `fork`. **`fork` must keep
    `DONTFORK` and `WIPEONFORK` per region when it comes**; research suggested recording them
    now, but with no `fork` nothing could observe or test them, so the region records them
    with `fork` (roadmap).
  - `REMOVE` needs memory a file backs (shared anonymous memory): `EINVAL`, as anonymous
    memory is private here (D53). `COLLAPSE` needs huge pages: `EINVAL`. Both are `ENOMEM` for
    a range with no region at all, as Linux finds no mapping first.
  - `HWPOISON` and `SOFT_OFFLINE` are `EPERM` for a process that is not privileged (D75) and
    `EINVAL` for root: no page is ever taken offline here, as on a kernel built without
    memory-failure handling.
- **`mremap`** (research). Checked in Linux's order: unknown flags, `MREMAP_FIXED` without
  `MREMAP_MAYMOVE`, `MREMAP_DONTUNMAP` without it or with sizes that differ (before rounding),
  an unaligned old address, a size that wraps or a new size of 0 once rounded up are
  `EINVAL`; then an old address in no region is `EFAULT`.
  - **Shrinking** unmaps the tail, holes and all, and returns the old address; the same size
    returns it at once.
  - **Growing** needs the old range in one region (`EFAULT` otherwise, as Linux needs it in
    one mapping) and an old size that is not 0 (`EINVAL`: on Linux that duplicates a shared
    mapping, and anonymous memory is private here). It grows in place when the range ends
    its region and the addresses after it are free; otherwise `MREMAP_MAYMOVE` moves it to
    where `mmap` would place it (top-down), else `ENOMEM`.
  - **Moved, never copied.** A move relinks each page's entry, mapped or kept
    (`PROT_NONE`), at the new address (`PageTables::move_user_pages`): the frame and its
    contents go with it, and the old entries are dropped from the TLB as `munmap` drops them
    (D53). If a table for the new place cannot be allocated, the pages moved so far go back
    and the call fails with `ENOMEM`, the old range as it was (with `MREMAP_FIXED`, what was
    at `new_address` is already unmapped, as on Linux), and every CPU drops what it may have
    cached of the entries that held pages at the new place. The region keeps its attributes; what
    it grew by is mapped, zeroed, when first touched.
  - **`MREMAP_FIXED`** moves the first `new_size` bytes to `new_address`, replacing whatever
    was there, and unmaps the rest of the old range; `MREMAP_DONTUNMAP` leaves the old range a
    region with no pages, which read zeros when next touched, and takes `new_address` as a
    hint. Either checks the new range first: unaligned, past user space or overlapping the
    old range is `EINVAL`, below the lowest address a program may map `EPERM`.
- **Deviations from Linux** (research):
  - Linux's `MREMAP_FIXED` unmaps the target and the old range's tail before it checks the
    old range, so a call that then fails with `EFAULT` has unmapped both. Here everything is
    checked first and a call that fails a check unmaps nothing (only running out of frames
    for page tables, after the target is unmapped, leaves it unmapped, as on Linux); no
    program can depend on the half-done state.
  - Growing in place stops below where `mmap` places mappings (the stack's reserve and guard
    gap, D52), where Linux lets a mapping grow up to the stack. The stack is one fixed 8 MiB
    region here, so the gap below it is what makes an overflow fault instead of running into
    another mapping; a range that cannot grow there moves (`MREMAP_MAYMOVE`) or fails with
    `ENOMEM`.
  - Regions with the same attributes are joined more eagerly than Linux joins mappings, so
    a range across two `mmap`s can be one region here and grow where Linux says `EFAULT`.
- **Cost.** Both calls sit in `out_of_line_result`; the system-call, switch and page-fault
  paths are unchanged. `PageTables::map_leaf` and `change_user_pages` now share their walks
  with the move (`leaf_table_entry`, `for_each_user_page`, always inlined). New callers
  of shared helpers changed how LLVM compiled `munmap` and `mmap`: `AddressSpace::change_pages`
  became one out-of-line copy for `free_pages`' closures, called from both `munmap` and
  `madvise`, and `Regions::insert` lost the specialisation it had with fewer callers. That made
  `linux_mmap_munmap` 6 to 9% slower than `main` (x86_64 388 to 421 ns). `free_pages`,
  `AddressSpace::change_pages`, `PageTables::change_user_pages` and `Regions::insert` are now
  always inlined, which makes `linux_mmap_munmap` faster than on `main`: aarch64 356.52 to
  335.52 ns (−5.9%), riscv64 385.10 to 362.11 ns (−6.0%), x86_64 388.08 to 360.09 ns (−7.2%).
  Only those three baseline lines change. Other benchmarks are unchanged. Kernel text grows
  by about 15 KiB per architecture: about 10 KiB for the two calls and about 5 KiB (2%) for
  the inlining. That 5 KiB buys the 6–7% on every `mmap` and `munmap`; without it, the gain
  is gone (`free_pages` out of line measures 388 ns on x86_64, the same as `main`).
  Research accepted both parts of the size cost (D24, D78). The two calls are what every C
  library's allocator uses: `MADV_DONTNEED` or `MADV_FREE` to give memory back, `mremap` to
  grow a large block. Their scope is already cut to anonymous memory only. Windows and macOS
  pay for the same thing too: `MEM_RESET`/`DiscardVirtualMemory`, and `MADV_FREE_REUSABLE`/
  `mach_vm_remap`. Cutting the inlining would bring back a regression that can be avoided,
  for 5 KiB.
- **Tests.** Host: `move_user_pages` moving mapped and kept pages, refusing a target with a
  page in it or overlapping the source, and putting every page back when tables run out;
  `madvise`'s errors in Linux's order, every advice that changes nothing, `DONTNEED`, `FREE`
  and `DONTNEED_LOCKED` freeing frames (kept ones too) with zeros read after and `ENOMEM` for a
  hole, `POPULATE_WRITE` mapping frames and `POPULATE_READ` mapping none, `EINVAL` for a
  region that does not allow the access, `EPERM` for an unprivileged `HWPOISON`; `mremap`'s
  errors in Linux's order, shrinking, growing in place up to the next region and never into
  the guard gap, moving `PROT_NONE` pages with their contents and dropping the old
  translations, `MREMAP_FIXED` replacing the target and unmapping the tail, and
  `MREMAP_DONTUNMAP` leaving an empty region at the hint's source. QEMU: a new test program,
  `linux_move_and_discard`, populates a page with `MADV_POPULATE_WRITE`, writes a word, moves
  the page 1 MiB down with `MREMAP_MAYMOVE | MREMAP_FIXED` and reads the word there, maps a
  new page at the old address and reads 0 (no stale translation of the moved page), then
  discards the moved page with `MADV_DONTNEED` and reads 0; it passes on aarch64, riscv64 and
  x86_64.

## D78 Performance regressions are settled by research, not by waiting for the owner — Decided (owner, 2026-10-23); supersedes D13's "unless the owner explicitly accepts it"
A PR that makes a benchmark slower is still blocked until the regression is fixed, or until a
research subagent concludes the cost is unavoidable: the price of correctness, security or a
feature EggOS must have, already cut to the minimum, and compared with how Linux, Windows and macOS
pay for the same thing. Then the PR records the reasoning under "Decided by research", this file
records the accepted cost, the baseline lines for exactly the affected benchmarks change in the same
PR, and it merges. Nobody waits for the owner, who can overrule any accepted cost later. The same
applies to the size, memory, boot and install budgets (D24, D69) and to UX regressions, which are
treated like performance regressions.

Why: PR #47 (FP/SIMD state per thread) waited a day for an owner OK on a cost that every OS pays.
The owner: "the agent should make the research to solve it instead wait for me".

## D79 License: MIT OR Apache-2.0 — Decided (owner, 2026-10-23); supersedes D5's license and its "no dual licensing" rule
EggOS is licensed under either the MIT License or the Apache License 2.0, at the user's option
(`LICENSE-MIT`, `LICENSE-APACHE`), the same as Rust and most of its ecosystem. The owner replaced
D5's "never relicense, no dual licensing" rule on 2026-10-23: *"switch EggOS to MIT OR Apache-2.0
too and replace the never-dual-license rule."* The same change was made to the other eggspot
open-source projects.

Nothing is taken away: anyone can still use EggOS under MIT, and Apache-2.0 adds an express patent
grant. It stays free forever, with no extra terms, no proprietary edition and no CLA. Contributions are
accepted under the same terms. Every contributor so far is the owner, its bots or its AI
assistants, so the owner can relicense. D5's clean-room rule is unchanged. Only the owner changes
the license, by an explicit instruction recorded here.

## D80 Self-tests count the heap's slab frames as free when they check that frames were freed — Decided by the build loop
- **Why.** On `main` after PR #72, CI's aarch64 boot test failed once in
  `processes_run_on_every_cpu`: "the processes' frames and tables were not all freed". A rerun
  passed, and it passed 5 times out of 5 locally. The self-tests run a workload twice and
  check that the second round gave back every frame, on the assumption that the first round
  grew the heap's size classes as far as they need (D30): those classes never give frames
  back. With threads on several CPUs, the second round can allocate in another order and
  reach a higher peak, so a class takes one more frame. The check then fails with nothing
  leaked.
- **What.** The heap counts the frames its size classes take (`Heap::slab_frames`), one
  increment in the out-of-line refill and nothing on the block path.
  `KernelHeap::free_and_slab_frames` reads that count with the frame allocator's free frames
  under the heap's lock, and the self-tests that run processes and threads compare that sum,
  so a class that grows does not count as a leak. A leaked frame, page table or larger
  allocation still changes the sum. Tests that check the frame or heap allocator itself still
  count free frames only. What the sum no longer catches is a leak of small heap blocks
  alone (2048 bytes or less, with no frame or page table behind them); the old check caught
  one only when it happened to make a size class take a frame.
- **Tests.** Host: the count grows by one when a class takes a frame and not for a frame-sized
  allocation, and the sum is unchanged after both are freed.

## D81 A CPU records that it has no address space loaded only after it dropped its reference — Decided by the build loop (the order and the fences decided by research)
- **Why.** D80 did not stop `processes_run_on_every_cpu` from failing with "the processes'
  frames and tables were not all freed" (main's CI on #75; about 1 boot in 16 on x86_64
  locally). The cause was a race. `unload` switched to the kernel's tables, recorded "none
  loaded", and only then dropped the CPU's reference, which frees the process's tables and
  pages when it is the last. The self-test's warm-up wait returns once every CPU records
  none, so it could read its starting count while another CPU was still freeing. With
  diagnostics: the count was 7 frames low (one test process) with no process alive, and those
  7 came back during the next round. D80's change stays: it removes a separate cause.
- **What.** `unload` drops the reference first, then a Release fence, then records none.
  `release_loaded_address_spaces` takes an Acquire fence once every CPU records none. So "none
  loaded" means everything the CPUs let go is freed and visible.
- **Cost and safety (research).** `unload` is a cold path, and the fence is its only new
  instruction. The hardware switch still comes before the drop, as `AddressSpace::drop`
  requires. While the drop runs, other CPUs may still see this CPU recording the old root:
  - `LazilyLoaded::only_cpus_hold_it` may answer wrongly. That is harmless, as `Arc` counts
    the real holders.
  - `Cpu::mark_stale` may mark it. The store of "none" overwrites that.
  - `wake_cpus_holding` may send a spurious wake-up.

  All of these hold even if the freed root frame is reused at once, and D48 already accepts
  them. Nothing the drop runs uses this CPU's record: `Process` has no `Drop` of its own, and
  interrupts are masked.
- **Tests.** 40 x86_64 boots in a row pass (before: about 1 failure in 16). The diagnosis
  used temporary prints of the count, the live processes and each CPU's slot; they are not
  kept.

## D82 Signal frames hold the thread's FP/SIMD registers, and `rt_sigreturn` loads them — Decided by the build loop (Linux's rules decided by research); completes D64's FP/SIMD parts and one of D54's follow-ups
- **Why now.** D64 gave every frame an FP/SIMD part in Linux's shape holding only the initial
  state, and `rt_sigreturn` ignored its values: all a program could have before D54. Since
  D54, programs have FP/SIMD registers, so a handler saw the wrong values in its frame, a
  handler that changed them there changed nothing, and on AArch64 and RISC-V, where a
  handler starts with the interrupted code's registers, whatever the handler left in them
  leaked back into the interrupted code. C libraries' `swapcontext`-style code, Rust's and
  Go's runtimes, and crash reporters read or edit these parts.
- **What a frame holds.** The running thread's registers as its program left them
  (`thread::with_running_fp`: under the scheduler's lock, saved from the CPU, read or
  changed, loaded again), or the initial state for a thread that has none: x86_64's
  `FXSAVE64` image where `fpstate` points (bytes 464 to 511, where Linux puts the `XSAVE`
  magic when it uses `XSAVE`, zero), AArch64's `fpsimd_context` (`fpsr`, `fpcr`, `v0` to
  `v31`) and RISC-V's D state (`f0` to `f31`, `fcsr`). The layouts are D64's.
- **How a handler starts (research).** On x86_64 with the initial state (x87 control word
  `0x037F`, `MXCSR` `0x1F80`, registers zero) once its frame is written, as Linux does; on
  AArch64 and RISC-V with the interrupted code's registers, as Linux does (it resets only
  SVE, SME and V state there, which EggOS does not have yet).
- **What `rt_sigreturn` loads (research).** The frame's values, before anything else is
  restored, so a refused frame changes nothing:
  - x86_64: a null `fpstate` means the initial state; one not 16-byte aligned (Linux runs
    `FXRSTOR64` on the program's memory, which needs it), or unreadable, is `SIGSEGV`. An
    image setting an `MXCSR` bit outside the CPU's `MXCSR_MASK` (`0xFFBF` when the CPU stores
    0) is `SIGSEGV`, as on Linux since 5.14; the image's own `MXCSR_MASK` and bytes 464 on
    are never used. The mask comes from the thread's own state, which `FXSAVE64` wrote just
    before (`FpState::set_fxsave_image`), so a bad value never reaches the kernel's
    `FXRSTOR64`, which would fault in ring 0.
  - AArch64 and RISC-V: every value as it is, as on Linux; the reserved bits of `FPCR`,
    `FPSR` and `fcsr` ignore writes. The records' shape is still checked as in D64.
  - A thread with no FP/SIMD registers gets them (`thread::start_fp`) when the frame holds
    anything but the initial state, so a handler's change is not lost; with the initial
    state it stays without, costing nothing. No memory for them is `SIGSEGV`, as a failed
    `rt_sigreturn` is on Linux. (A thread that has registers by then always loads the
    frame's, initial or not, so a handler's own values never leak back.) A kernel thread
    running a program directly (self-tests and benchmarks only) ignores the frame's values,
    as D54 says it gets whatever the CPU has.
- **The HAL** gains accessors on `FpState`: `fxsave_image` and `set_fxsave_image` (with the
  `MXCSR` check) on x86_64, `v`, `v_mut`, `fpcr`, `fpsr` and `set_fpcr_and_fpsr` on AArch64,
  `f`, `f_mut`, `fcsr` and `set_fcsr` on RISC-V. `restore_fp`'s contract now allows states
  changed through them.
- **Cost.** Signal delivery and `rt_sigreturn` save and load the registers once more under
  the scheduler's lock, out of line (`#[cold]`); x86_64 delivery pays one more for the reset.
  Nothing changes on the system-call, page-fault or switch paths (benchmarks in the PR).
- **Tests.** QEMU, on aarch64, riscv64 and x86_64: `linux_signal_handler` now sets `d8`, `fs0`
  or `xmm8` and a non-initial rounding mode before `tgkill`; its handler checks the frame's
  FP/SIMD part holds both and that it starts as Linux's would (inherited, or x86_64's initial
  state), sets `d9`, `fs1` or `xmm9` in the frame and clobbers the first; after the return
  both registers and the rounding mode must be the frame's. With `rt_sigreturn`'s load taken
  out, the self-test fails. Host: the parts' layouts and round trip per architecture,
  x86_64's software bytes and `MXCSR` refusal, `set_fxsave_image`'s mask rules,
  `with_running_fp`'s save and load of the running thread's state, a frame's FP/SIMD part
  for a thread without registers through the dispatcher, x86_64's null `fpstate` accepted and
  misaligned one `SIGSEGV`, and the simulated test program changing a register in its frame
  (so a thread without registers gets them on `rt_sigreturn`).
- **Follow-ups.** `XSAVE` frames with AVX state (the `fx_sw_bytes` magic, `XSTATE` records)
  when AVX comes (D54); SVE, SME and V records with their resets; `fork` and `clone` copying
  the FP/SIMD state (D54).

## D83 Review of D54, D77 and D82: x86_64 clears `CR4.OSXSAVE`, `PKE`, `CET` and `UINTR` whatever firmware left; `mremap` with `MREMAP_FIXED` checks the old range's end before moving; `MADV_POPULATE_WRITE` skips mapped pages — Decided by the build loop
- **Why.** The review step after three feature PRs (#47, #72, #77). The switch path's owner
  check (thread ids are never reused, so a stale owner cannot match), the preemption and
  interrupt masking around `start_fp`, `with_running_fp` and `CpuFp::switch`, RISC-V's
  Clean/Dirty skip, the FP-free kernel targets, x86_64's `fnclex` before the loads and the
  `MXCSR` check against the mask `FXSAVE64` just wrote, every signal frame's FP/SIMD layout,
  how handlers start, `madvise`'s checks and holes, and `mremap`'s moves, rollbacks and error
  order all check out. Three things change.
- **x86_64 clears `CR4.OSXSAVE`, `PKE`, `CET` and `UINTR` on every CPU, whatever firmware left.**
  D54 said `OSXSAVE` "stays clear", but `set_up_user_mode` only set bits in CR4 and cleared
  `FSGSBASE`, and the boot CPU keeps the loader's CR4. A loader that hands over with
  `OSXSAVE` set (UEFI loaders often do) would let programs use AVX and AVX-512, whose upper
  register halves and mask registers `FXSAVE64` neither saves nor clears: one program would
  see another's (LazyFP, CVE-2018-3665, again), and a thread that checked `CPUID` on the boot
  CPU would get `SIGILL` on another. With `PKE` left set, `WRPKRU` works in ring 3 and PKRU
  is never switched, so one program could take pkey 0 away from the next. Shadow stacks
  and indirect-branch tracking (`CET`, with `IA32_U_CET` and the ring-3 shadow-stack
  pointer) and user interrupts need state that is never switched too; the kernel uses
  neither (no `-Zcf-protection`). Clearing them takes away nothing the kernel uses;
  `CPUID` then tells programs there is no `XSAVE` and no protection keys (Intel SDM Vol. 1
  §13 and Vol. 3A §2.5). AArch64 writes `CPACR_EL1` outright and RISC-V clears `sstatus.VS`,
  so neither has the problem. The `FSGSBASE` reason (D49, D57) is unchanged: the way in
  loads the FS base only when it changed, and a program-chosen GS base needs the SWAPGS
  fence first.
- **`mremap` with `MREMAP_FIXED` or `MREMAP_DONTUNMAP` and an old size larger than the new
  one checks that the old range ends inside user space before it moves anything**
  (`EINVAL`, before the target's `EPERM`, as Linux's `munmap` of the tail comes first). It checked that only when unmapping the tail, after the move, so such a call
  moved the pages, replaced what was at the target, and then failed: the program's data was
  at the target while the call said nothing happened. Linux unmaps the tail first, under
  `munmap`'s rule that a range past user space is `EINVAL`, so its old range is intact. This
  keeps D77's promise that a call that fails a check unmaps nothing.
- **`MADV_POPULATE_WRITE` leaves pages that are already mapped for the access alone.** It
  took, zeroed and freed a frame for each of them (the fault path's order, D53, where a page
  is almost never mapped yet), so populating 1 GiB that a program had touched zeroed 1 GiB.
  It now walks the tables first, as Linux does. No result changes; there is no benchmark
  for `madvise` (it is not on a hot path, D77).
- **Recorded, not changed** (follow-ups in the roadmap):
  - Regions do not say what backs them, so `madvise` and `mremap` treat the program's ELF
    segments and the signal trampoline's page as anonymous memory: `MADV_DONTNEED` on
    `.data` reads zeros where Linux reads the file's contents again, `MADV_FREE` there
    succeeds where Linux says `EINVAL`, and moving the trampoline's page leaves the recorded
    return address behind (Linux updates the vDSO's). Fixed with file mappings (M7), when
    regions learn their backing.
  - Frames of threads without FP/SIMD registers carry an `MXCSR_MASK` of 0 on x86_64
    (meaning `0xFFBF`) where Linux writes the CPU's own mask; the kernel's own check uses the
    thread's saved mask, so only a program reading the field sees the difference.
  - AArch64's trapped floating-point exceptions (EC 0x2C) end the program with `SIGILL`
    where Linux sends `SIGFPE`; QEMU never raises them.
  - Tests to add: a build check that the kernel image has FP/SIMD instructions only in the
    HAL's `fp` code and the test programs; a QEMU test that `rt_sigreturn` with reserved
    `MXCSR` bits is `SIGSEGV` with no kernel fault.
- **Cost.** One more bit pattern in a CR4 write at each CPU's start; `mremap`'s check is one
  compare on a cold path; `MADV_POPULATE_WRITE` skips the zeroing of a frame for each mapped
  page, and pays one more table walk for each unmapped one, small next to zeroing its frame.
  No benchmark changes.
- **Tests.** QEMU: `processes_keep_their_fp_registers_across_switches` first checks, in the
  kernel (not in host tests, which run on the host's CPU), that `CPUID` reports neither
  `OSXSAVE` nor `OSPKE` on x86_64. Under `-cpu max` with CR4's `OSXSAVE` and `PKE` set before
  `set_up_user_mode`, as such a loader would leave them, it fails without the fix and passes
  with it (QEMU's PVH boot leaves them clear, so in CI it guards against a regression). Host:
  `mremap` with `MREMAP_FIXED` and an old size reaching past user space is `EINVAL` with the
  region and its contents where they were (fails without the fix); `MADV_POPULATE_WRITE` over
  populated pages keeps their frames and contents.

## D84 Test programs built from our own Rust source with Rust's std, for the official `*-unknown-linux-musl` targets — Decided by the build loop (acceptability and approach decided by research; the owner can overrule it)
- **Why.** M6's goal is that real static Linux programs run. So far every test program is a
  few dozen instructions of HAL assembly wrapped in an ELF image (D51); none goes through a C
  library's startup or Rust's std. Research traced what a static Rust std program (musl),
  a static musl C program and a static glibc C program call from start to exit: every call is
  either implemented or fails gracefully with `ENOSYS` (`rseq`, `readlink` of
  `/proc/self/exe`, `/proc/self/maps` for std's main-thread guard on glibc), so the next step
  is to run one, from our own source, on every architecture.
- **How.** The pinned toolchain gains `aarch64-unknown-linux-musl`,
  `riscv64gc-unknown-linux-musl` and `x86_64-unknown-linux-musl` (all tier 2 with std from
  rustup in 1.99). The programs are linked as static executables by `rust-lld` with the
  targets' self-contained musl and start files (`-C linker=rust-lld -C link-self-contained=yes
  -C target-feature=+crt-static`; RISC-V's target is dynamic by default), so they link with no
  C toolchain on macOS, Windows and Linux alike (checked on macOS: about 350–380 KiB each for a
  std hello world with `opt-level = "s"`, LTO and `strip`).
- **Why this fits "everything is ours, in Rust, no third-party crates"** (research): the
  program's source is ours and in Rust; std and musl come pre-built in the pinned toolchain's
  sysroot, as `core` and `alloc` already do for the kernel, and `Cargo.lock` still lists only
  our crates. The binary is a Linux program for EggOS to run, the kind of foreign software the
  OS exists for, so like firmware it is outside EggOS: it is a test fixture, built only for
  the QEMU tests, never linked into the kernel's code, never shipped in a release image and
  never committed: like firmware, nothing of it is copied into the repo.
  Licences fit MIT OR Apache-2.0 (musl is MIT; LLVM's libunwind and start files are
  Apache-2.0 with the LLVM exception). glibc programs cannot be built this way (rustup ships no
  self-contained glibc) and are left to observed behaviour.
- **Rejected:** `#![no_std]` programs of our own on the `*-unknown-none` targets. They are
  allowed, but exercise no C library startup or std, and two of those targets cannot even
  emit FP/SIMD code, so they would not move M6 toward "a real program runs".
- **Cost.** The CI image grows by three `rust-std` components; CI's macOS and Windows host
  jobs install three more targets with the toolchain (measured in the PR). Release and
  benchmark kernels are unchanged: the programs go only into the `qemu-test` build.
- **Next.** The first program (a std hello world that prints its arguments, uses a `HashMap`,
  reads `Instant`, sleeps and exits with a status) runs in a QEMU self-test on aarch64, riscv64
  and x86_64, with `AT_MINSIGSTKSZ` added to the auxiliary vector.

## D85 A first Linux program built from our own Rust source, on a runtime of our own with no C library — Decided by the build loop (targets, relocation, embedding and layout decided by research); complements D84
- **Why.** D84 pinned the `*-unknown-linux-musl` targets so programs built from our own Rust
  source can run in the QEMU tests. Before a std program brings a C library's start-up into
  play, the first program checks the kernel's half of the start-up contract on its own: the
  stack Linux hands a program, the auxiliary vector, a static PIE relocating itself, and each
  call a C library's and std's start-up make, answered as Linux answers it. A failure then
  names the kernel's missing piece, not a C library's.
- **How.** `userland/runtime` (`eggos-runtime`, a library M10's init, shell and utilities will
  share): `_start` per architecture, self-relocation of a static PIE (`R_*_RELATIVE` from
  `DT_RELA`; `DT_REL`, `DT_RELR`, other types or symbols refused with exit 127),
  `PT_GNU_RELRO` made read-only with `mprotect` as Linux's loader does, the start-up stack
  read in place, raw system calls, the memory functions the compiler calls (the crate is
  `no_builtins`) and a panic handler. `userland/selftest` (`eggos-selftest`): 17 start-up
  checks in a table of function pointers, so the binary really carries relocations
  (arguments and environment, the auxiliary vector, zeroed statics, thread ids, the robust
  list, the thread pointer, `RLIMIT_STACK`, descriptors 0–2, `SIGPIPE` ignored, an alternate
  signal stack behind a guard page, a `SIGSEGV` handled on it that `mprotect`s the page and
  resumes, `getrandom`, `brk`, a 1 MiB anonymous mapping, IEEE double arithmetic around system
  calls and the handler, the console's `TIOCGWINSZ` and `fstat`, `CLOCK_MONOTONIC`, `uname`).
  Both are `#![no_std]` for the musl targets and linked by `rust-lld` with
  `-C link-self-contained=no` and `-nostdlib`: no musl or other C code is linked. On the host
  they build as a library with host tests (stack parsing, relocation) and a stub `main`.
- **Embedding.** `cargo xtask` builds the program first and names its path in
  `EGGOS_USERLAND_SELFTEST` for every `qemu-test` and `qemu-bench` kernel build and for
  `clippy-targets`; the kernel reads it with `include_bytes!(env!(..))` only under
  `qemu-test`. Release kernels are unchanged. Rejected: a nested cargo in `kernel/build.rs`
  (target-directory deadlocks, leaked environment, a dozen extra builds under
  `clippy-targets`).
- **Static PIE rather than a fixed-address executable**: it exercises D51's PIE path, and
  compiled Rust emits `R_*_RELATIVE` relocations as soon as it has vtables or pointer tables.
- **Linking flags and D84.** `.cargo/config.toml` gives the three musl targets `rust-lld` with
  `link-self-contained=no`. D84's std program needs `link-self-contained=yes`, so its build
  will pass its own linking flags (through `cargo xtask`), and both kinds of program share the
  targets.
- **Tests.** The self-test `linux_programs_built_from_rust_source_start` runs the program
  twice, the second run freeing every frame it took, and the QEMU runner requires the
  program's line on every architecture; a deliberately wrong expectation fails the boot naming
  the check.
- **Next.** D84's std program, with `AT_MINSIGSTKSZ`.

## D86 A program built with Rust's standard library and musl runs in the QEMU self-tests — Decided by the build loop; carries out D84's plan
- **What.** `userland/std-selftest` (`eggos-std-selftest`) is an ordinary Rust program on
  Rust's standard library, built for the `*-unknown-linux-musl` targets and linked statically
  with the toolchain's own musl and start files. It checks what a typical Rust program uses
  from its first line: its arguments and environment, a `HashMap` (random keys from
  `getrandom`), the heap (a 2 MiB vector and many small strings), a boxed trait object, float
  formatting, `Instant` across a 2 ms `thread::sleep`, and the realtime clock; it writes
  `EGGOS-USER: a std program built from Rust source ran its checks` and exits with 0, or names
  each failed check and exits with 1. Before `main` it goes through musl's and std's start-up,
  whose calls D84's research listed.
- **How it is built.** `cargo xtask` builds it with `CARGO_ENCODED_RUSTFLAGS` replacing
  `.cargo/config.toml`'s flags for the target (D85's `link-self-contained=no` is for our own
  runtime): `-C link-self-contained=yes -C target-feature=+crt-static -C relocation-model=pie
  -C debuginfo=0 -C strip=symbols`. x86_64's target makes a static PIE and AArch64's and
  RISC-V's a static executable at a fixed address, so the self-tests load both kinds through
  D51's loader. Each is about 370–430 KiB. Like D85's program it is named to the `qemu-test`
  kernel build in an environment variable (`EGGOS_USERLAND_STD_SELFTEST`) and embedded only
  there; release kernels are unchanged, and the program is never committed (D84).
- **Tests.** The self-test `linux_programs_built_with_rust_std_run` runs it twice (the second
  run must free every frame it took), and the QEMU runner requires its line on aarch64,
  riscv64 and x86_64. A deliberately wrong expectation fails the boot naming the check. On the
  host it builds as the same program, so `cargo clippy` and `cargo build` cover it.
- **Next.** `AT_MINSIGSTKSZ` in the auxiliary vector (std then sizes its alternate stack from
  it rather than from `SIGSTKSZ`); C programs linked with musl; `init` as pid 1 (M10).

## D87 `AT_MINSIGSTKSZ` in the auxiliary vector: our `MINSIGSTKSZ`, which every frame is checked to fit — Decided by the build loop (where and what value decided by research)
- **What.** Every program's auxiliary vector starts with `AT_MINSIGSTKSZ` (51), before
  `AT_HWCAP`, where Linux's architecture block puts it (we have no vDSO or cache entries to
  precede it): 2048 on x86_64 and riscv64, 5120 on AArch64, our `MIN_STACK_SIZE`, the size
  `sigaltstack` refuses below with `ENOMEM`. Rust's std sizes its alternate stacks from it
  (the larger of it and `SIGSTKSZ`), and glibc's `sysconf(_SC_MINSIGSTKSZ)` returns it.
- **Why this value.** Linux documents it as the space the kernel needs to deliver one signal
  (frame, `siginfo_t`, FP state, alignment). Ours, `signal_frame::LARGEST_FRAME_SPACE`, is
  computed from the frame placement for every alignment of the stack's top: 1031 bytes on
  x86_64 (FXSAVE image and frame with their padding), 4719 on AArch64 (matching the 4720
  Linux reports there without SVE), 1103 on riscv64. Research found that reporting those bare
  sizes would make glibc's `_SC_MINSIGSTKSZ` smaller than what `sigaltstack` accepts, so a
  program sizing its stack from it would get `ENOMEM`. Reporting `MIN_STACK_SIZE` instead,
  with a compile-time check that `LARGEST_FRAME_SPACE` fits in it, gives a value that both
  holds every frame and is always accepted.
- **riscv64.** Linux gives it there only from 6.5 (with vector support), and we report 6.1;
  research judged emitting it harmless (absent, libraries fall back to `MINSIGSTKSZ`) and kept
  the three architectures alike.
- **Cost.** The entry alone added 80, 87 and 43 instructions to `linux_exec_load` (aarch64,
  riscv64, x86_64). The same PR writes the initial stack's words straight into its image
  instead of through a vector, so the benchmark ends 0.6%, 1.2% and 0.7% faster than before.
- **Tests.** Host: the vector's order and value; `LARGEST_FRAME_SPACE` per architecture and
  no frame placed below it for 256 consecutive tops. QEMU: the program of D85 checks the entry,
  that `sigaltstack` refuses one byte less with `ENOMEM`, and takes a stack of exactly that
  size.

## D88 Review of D84–D87: user programs build whatever `RUSTFLAGS` holds; stricter relocation tables; exit statuses read per program — Decided by the build loop
- **Why.** The review step after three feature PRs (#80–#82, with #79's toolchain change): a
  fresh read of the runtime, the two test programs, their self-tests, the xtask build and
  `AT_MINSIGSTKSZ`. The start-up code, raw system calls, stack parsing, `PT_GNU_RELRO`
  rounding, the initial stack's new layout and `LARGEST_FRAME_SPACE` check out. Five things
  change, besides a comment cleanup.
- **`RUSTFLAGS` no longer breaks the build** (a real bug). Cargo's `RUSTFLAGS` replaces a
  target's `rustflags` from `.cargo/config.toml` rather than adding to them, so with any
  `RUSTFLAGS` in the environment the runtime program lost `link-self-contained=no` and failed
  to link (`duplicate symbol: _start`, reproduced). xtask now passes every user program's
  flags in `CARGO_ENCODED_RUSTFLAGS`, which takes precedence, as D86 already did for the std
  program.
- **Relocation tables checked whole.** `relocate` refuses a `DT_RELASZ` that is not a whole
  number of entries, or one with no `DT_RELA` (it would have read the ELF header as
  relocations), and reads entries unaligned instead of assuming the table's alignment.
- **`memmove` no longer calls `memcpy`.** Both share one upward byte copy, so the overlap
  rule never rests on `no_builtins` alone.
- **Exit statuses mean what each program says.** 127 (could not relocate) and 101 (panic)
  come from our runtime only; a std program built with `panic = "abort"` ends with `SIGABRT`
  instead, and its self-test names that signal.
- **One failure, one failed check.** The `AT_MINSIGSTKSZ` check always disables the stack it
  set and unmaps it, so a wrongly accepted size no longer fails the next check too.
- **D66.** Doc comments on private items that #80–#81 added are removed (in a commit of
  their own); their content is in D85 and D86 or in the names (`null_ended_strings`,
  `ARGUMENTS_AFTER_PROGRAM_NAME`, `arguments_after_path`).

## D89 `futex`: `FUTEX_WAIT`, `FUTEX_WAKE` and their bitset forms, on a timed park in the scheduler — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** Threads are M6's next step, and every thread library is built on `futex`: Rust's
  std (its `Mutex`, `Condvar`, `RwLock`, `Once` and thread parking), musl's locks, `once`,
  `pthread_join` and semaphores, and the `CLONE_CHILD_CLEARTID` wake at thread exit. It comes
  before `clone`, which needs it.
- **What.** `futex` with `FUTEX_WAIT` (relative timeout on the monotonic clock),
  `FUTEX_WAKE`, `FUTEX_WAIT_BITSET` (absolute timeout on the monotonic clock, or the realtime
  clock with `FUTEX_CLOCK_REALTIME`) and `FUTEX_WAKE_BITSET`, with and without
  `FUTEX_PRIVATE_FLAG`. Waiters wake in the order they began to wait. The other operations
  are `ENOSYS` for now.
- **Linux's rules** (research, from the man pages, LWN and musl's and std's sources): a
  wait's timeout is read and checked first (`EFAULT`, then `EINVAL` for negative seconds or
  nanoseconds outside 0–999999999); then the operation: an unknown one, or
  `FUTEX_CLOCK_REALTIME` on anything but `FUTEX_WAIT_BITSET`, is `ENOSYS` (Linux 5.13 reverted
  the 4.5 change that allowed it with `FUTEX_WAIT`); a bitset of 0 or a word not 4-byte
  aligned `EINVAL`; then the word: unreadable `EFAULT`, another value `EAGAIN`, and only then
  the deadline (`ETIMEDOUT`), so a past deadline with the wrong value is `EAGAIN`. The word
  and the value are compared as `u32`. A wake's count is an `int`, and 0 or less still wakes
  one waiter, as Linux does. A private wake never touches memory (only an address past user
  space is `EFAULT`); a shared one needs the word readable. Private and shared waits on one
  word are kept apart, as on Linux, where glibc and musl wait shared on the
  `CLONE_CHILD_CLEARTID` word and a private wake must not reach them.
- **How.** A futex is keyed by its process's id (the address space, while a process has one
  thread and shares no memory), its address and whether it is shared. Waiters are linked into
  one of 256 buckets, each a spin lock over a list of waiters that live on the waiting
  threads' stacks, so nothing allocates. A waiter checks the word with its bucket locked, so a
  wake that changes the word first cannot be missed. It waits on a new scheduler primitive,
  `thread::Parker`: a thread with a deadline sleeps with its CPU's sleepers and `unpark` takes
  it out of them early; a thread without one is held by the `Parker` alone, off every list. A
  waker unlinks the waiter and unparks it with the bucket locked, and the woken thread takes
  that lock before its stack frame goes, so a waker never touches a waiter that is gone.
- **Signals.** A waiting thread cannot be interrupted yet: only its own process could send it
  a signal, and its only thread is waiting. With `clone`, a handler that runs ends the wait
  with `EINTR` (musl and std retry), and restarting waits come later.
- **Cost.** No existing benchmark moves (the system-call round trips are unchanged). New:
  `futex_wake_empty` (a wake nobody waits for: 20, 27 and 25 instructions on aarch64, riscv64
  and x86_64, against 15, 13 and 14 for a wait queue's) and `futex_wait_wake_round_trip` (two
  kernel threads handing a turn back and forth through two futexes: 473, 567 and 480, against
  218, 222 and 206 for `thread_wait_wake_round_trip` with wait queues; the difference is the
  bucket locks and the separate park). Measured while building it: parks without a deadline
  held outside the sleepers list and in their own small closure (−10%), and the shared bit
  folded into the key's address-space word (a third field cost +3–5%).
- **Tests.** Kernel self-test `futex_waiters_wake_in_order_and_time_out` (three kernel-thread
  waiters with different bitsets: a wake's limit and bitset, FIFO order, other address
  spaces' and shared wakes left alone, `EAGAIN` from the check, `ETIMEDOUT` never early, a
  timed-out waiter not left queued, an unpark before the park); the runtime test program's
  check of every error above from user mode on aarch64, riscv64 and x86_64; a host test of
  the wake count.
- **Next.** `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` (musl's condition variables need
  `FUTEX_REQUEUE`), then `clone` for threads with `CLONE_CHILD_CLEARTID`'s shared wake and the
  robust list. Later: `FUTEX_WAKE_OP`, the PI operations, `futex_waitv`, and keys by physical
  page for memory shared between processes.

## D90 `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** musl's condition variables hand their waiters over to the mutex with
  `FUTEX_REQUEUE` (`futex(l, FUTEX_REQUEUE | FUTEX_PRIVATE, 0, 1, r)` in `unlock_requeue`); a
  requeue that fails leaves that waiter asleep for good, so C programs with condition
  variables need it before musl's threads run (D89's "Next"). `FUTEX_CMP_REQUEUE` is the same
  path with one comparison more. Rust's std uses neither, and glibc's condition variables
  stopped using requeue in 2.25.
- **What, as on Linux** (research: the man pages, LTP's `futex_cmp_requeue01` and `02`, the
  CVE-2018-6927 fix's description, musl's sources): `val` waiters of `uaddr` are woken, oldest
  first, and the next `val2` (the `timeout` argument's low 32 bits, as an `int`) are moved, in
  their order, behind `uaddr2`'s own waiters, whatever their bitsets, which they keep for later
  `FUTEX_WAKE_BITSET`s. Both operations return how many were woken and moved together (the
  man page's "woken" for `FUTEX_REQUEUE` understates what Linux returns). A `val` of 0 wakes
  nobody (the "at least one" of `FUTEX_WAKE` is its own). Negative counts are `EINVAL`;
  `FUTEX_CLOCK_REALTIME` is `ENOSYS`, as for every operation but `FUTEX_WAIT_BITSET`; each
  word is checked like `FUTEX_WAKE`'s (misaligned `EINVAL`, past user space `EFAULT`, and for
  a shared futex unreadable `EFAULT`), `uaddr` before `uaddr2`; `FUTEX_CMP_REQUEUE` then reads
  `uaddr` with both buckets locked, `EFAULT` or `EAGAIN` if it does not hold `val3`. A requeue
  to the same futex is allowed: it wakes as asked and leaves the others where they are,
  counted as moved. A moved waiter that times out leaves `uaddr2`'s queue with `ETIMEDOUT`.
- **Check order.** Research listed negative counts before the clock flag's `ENOSYS`, with
  medium confidence; the independent review recalled the clock flag being checked first, for
  every operation, before the requeue's own checks. We keep D89's order (the operation and
  its flags first), which only matters for a call that has both errors.
- **How.** A waiter now records its key in its link and the index of the bucket it is in; a
  requeue locks both buckets in index order (one when they are the same), checks the word,
  wakes, then relinks the moved waiters into the second bucket and updates their index. A
  waiter that wakes at its deadline finds its current bucket by locking the one its index
  names and checking the index again. A waiter that was unparked returns at once without
  taking any lock: its waker unlinked it before unparking it, and `Parker::unpark` touches the
  `Parker` last (D89's handshake is kept only for the timed-out case).
- **Cost.** `futex_wait_wake_round_trip` −1.5% on aarch64, −0.7% on riscv64, +0.4% on x86_64
  (within the gate): the bucket index and the key in the link cost about what skipping the
  second bucket lock after an unpark saves. Nothing else moves.
- **Tests.** Kernel self-test `futex_waiters_are_requeued_in_order` (five waiters: a check's
  error moves nothing, one woken and two moved, the third left behind, the moved ones behind
  the second futex's own waiter in their order with their bitsets, a requeue to the same
  futex, and a moved waiter with a deadline timing out from its new bucket, retried with a
  doubled deadline if the host stalls the guest past it before the requeue); the runtime test
  program's check of the requeue errors from user mode on aarch64, riscv64 and x86_64.
- **Next.** `clone` for threads (`CLONE_VM | CLONE_THREAD | ...`, `CLONE_SETTLS`,
  `CLONE_CHILD_CLEARTID`'s shared wake, `CLONE_PARENT_SETTID`), with a futex wait interrupted
  by a signal handler then returning `EINTR`.

## D91 `clone` for threads: each thread's own Linux state, `exit` for one thread and `exit_group` for all — Decided by the build loop (errors and exit status decided by research)
- **Why.** Rust's `std::thread`, musl's and glibc's `pthread_create` and every threaded
  program start threads with `clone(CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND |
  CLONE_THREAD | CLONE_SYSVSEM | CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID,
  ...)` and join them through the futex `CLONE_CHILD_CLEARTID` wakes (D89's and D90's
  "Next"). A process had one thread until now, with that thread's state in the process.
- **Each thread's own state.** A `LinuxThread` (`kernel/src/linux/thread.rs`) holds the
  thread's id, its `clear_child_tid` (`set_tid_address`, now remembered, and
  `CLONE_CHILD_CLEARTID`) and its signal state (`ThreadSignals`: blocked, pending, alternate
  stack). The thread's loop owns it, boxed, and hands it to every system call
  (`linux::handle`); the process keeps what threads share (`signals::Signals`: the actions
  and the queued `siginfo_t`s, each queued one now tagged with its thread's id). A signal call
  joins the two under the process's lock (`LockedSignals`). The first thread's id is the
  process's; the others' come from the same counter, as Linux gives threads and processes
  ids from one space. `gettid` is the thread's.
- **`clone`.** Only a new thread of the caller's process: `CLONE_VM | CLONE_SIGHAND |
  CLONE_THREAD`, with any of `CLONE_FS`, `CLONE_FILES`, `CLONE_SYSVSEM` (shared anyway),
  `CLONE_SETTLS`, `CLONE_PARENT_SETTID`, `CLONE_CHILD_SETTID`, `CLONE_CHILD_CLEARTID`,
  `CLONE_DETACHED` (ignored, as on Linux), `CLONE_PTRACE`, `CLONE_UNTRACED`, `CLONE_IO`. The
  child returns 0 on the given stack (the caller's if 0) with the caller's other registers,
  its thread pointer `tls` with `CLONE_SETTLS`; its id goes to the id words before it runs
  (faults ignored); it blocks what the caller blocks, with nothing pending and no alternate
  stack. Arguments in each architecture's order (x86_64 `child_tid` before `tls`, AArch64
  and RISC-V after). The flags' upper 32 bits and the exit signal are ignored.
- **Errors (research: clone(2), fork(2), musl's and glibc's callers, LTP's `clone*`
  descriptions).** Linux's `EINVAL` combinations in Linux's order (`CLONE_PIDFD` with
  `CLONE_PARENT_SETTID`; `CLONE_NEWNS` or `CLONE_NEWUSER` with `CLONE_FS`; `CLONE_THREAD`
  without `CLONE_SIGHAND`; `CLONE_SIGHAND` without `CLONE_VM`; `CLONE_THREAD` with
  `CLONE_NEWUSER` or `CLONE_NEWPID`; `CLONE_PIDFD` with `CLONE_DETACHED` or `CLONE_THREAD`),
  then `EINVAL` for namespaces and `CLONE_PIDFD`, as on a Linux built without them (sandbox
  and pidfd probes read it as unavailable); then `ENOSYS` for a new process (no `CLONE_VM`,
  `CLONE_VFORK`, `CLONE_PARENT`) until processes can be created: fork(2) documents it as
  "not supported", and `fork`, `posix_spawn` and their callers report it instead of retrying
  as they would on `EAGAIN`. x86_64's `EPERM` for a thread pointer outside user space, as
  `arch_prctl`. `EAGAIN` when ids run out, `ENOMEM` without memory for the thread.
- **Ending threads.** `exit` ends the calling thread: if others go on, the 32-bit word at its
  `clear_child_tid` becomes 0 (faults ignored) and one waiter on it as a shared futex is woken
  if it is aligned; its queued signals go. The last thread to end ends the process, with the
  first thread's status if that one ended earlier with `exit` (research: Linux reports the
  thread group leader's), else its own. `exit_group`, a fatal signal and the out-of-memory
  kill end the process: `Process::exit` records how, marks it ended, and, if it has other
  threads, asks every other CPU to look at its scheduler and interrupts it; a thread of an
  ended process stops at its next preemption point before user mode that looks
  (`preemption_point_before_user_mode` now says whether it looked) and after any system call
  handled out of line, and its futex waits are woken (`futex::wake_space`). None of this is
  on the system-call path.
- **One CPU per process, for now.** Changed mappings are dropped from other CPUs' caches
  lazily (D53): a CPU that keeps the address space loaded is marked stale and switches again
  before it next uses it, with no interrupt, which was sound while a process had one thread.
  Two threads of a process running on two CPUs at once would let one keep using
  translations, and the frames behind them, that the other just unmapped. Until changed
  mappings are shot down on other CPUs, `clone` pins the calling thread and the new one to
  the caller's CPU (`thread::CpuPlacement::WithCaller`): load balancing never moves a pinned
  thread, so every thread of a multi-threaded process runs on that one CPU, which is the one
  that changes its mappings. Threads run concurrently there (preemption), not in parallel;
  the shootdown that lets them spread is the next item.
- **Not yet (next).** The shootdown, then threads spread over CPUs; until then the
  interrupts `Process::exit` sends other CPUs find none of the process's threads there, and
  a pinned thread at the front of a busy CPU's ready threads hides the others behind it from
  an idle CPU (stealing looks at the front only). A new thread starts with the initial
  FP/SIMD state, where Linux copies the caller's (its rounding mode and exception masks
  included). A thread stopped by `SIGSTOP`'s default action sleeps for good, so it outlives
  its process's end like the waits below. Signals between threads: `kill`, `tkill`, `tgkill` and the queueing
  calls still reach only the caller (another thread's id is `ESRCH`), a process-directed
  signal goes to the calling thread, and `SIG_IGN` discards only the caller's pending signal.
  A thread of an ended process that sleeps in `nanosleep`, `ppoll`, `rt_sigsuspend`, `pause`
  or `rt_sigtimedwait` ends only when its wait does. Both need the same thing: a way to wake
  another thread's wait, then a pending signal checked on the way back to user mode. Then the
  robust list (`set_robust_list` is still checked and not remembered), `clone3`, and new
  processes (`fork`, `wait4`).
- **Cost.** In the first version the thread's state sat in the loop's closure next to the
  benchmark's own counter; the out-of-line calls take the state's address, so the counter
  lived in memory: `linux_syscall_round_trip` +3.1% to +5.4%. Boxed, the loop holds one
  pointer. Ending a single-threaded process scanned the 256 futex buckets and took the signal
  lock to drop the thread's queue (`linux_page_fault`, whose program exits each run, +1.2%);
  both now happen only when other threads exist. On AArch64 the unknown call's `ENOSYS` then
  shared its result store with the other calls, two branches more (+1.6%); it has its own
  now. Against `main` on the same machine afterwards: `linux_syscall_round_trip` −0.7%
  (x86_64), 0.0% (aarch64), −0.7% against the baseline (riscv64); `linux_page_fault` +0.2% to
  +0.4% (the boxed state and the thread count per spawn, within the gate); nothing else
  moves.
- **Tests.** The std test program (D86) starts four threads that add to a shared atomic and
  return their `ThreadId`s, joins them, and hands a value to a thread through a `Mutex` and a
  `Condvar`, on aarch64, riscv64 and x86_64. Host tests: `clone`'s refusals in order (and
  x86_64's `EPERM`), thread ids, `set_tid_address`, a thread's exit clearing its word and
  leaving the process, the last one ending it, the first thread's status, `exit_group`, a
  thread of an ended process stopping after an out-of-line call, and each thread's own mask
  and queued `siginfo_t`; load balancing never taking a pinned thread.

## D92 Review of D89–D91: a process's end wakes its threads' sleeps; `Parker::unpark` takes a pointer — Decided by the build loop
- **Why.** The review step after D89–D91 (`futex`, its requeue, `clone` for threads): a
  fresh, independent read of the futex table, the timed park, `clone`, `exit` and
  `exit_group`.
- **Bug: an ended process's sleeping threads stayed asleep.** D91 listed it under "Not yet":
  a thread in `nanosleep`, `clock_nanosleep`, `ppoll`, `poll`, `rt_sigsuspend`, `pause`,
  `rt_sigtimedwait` or a `SIGSTOP` stop slept on the scheduler's sleepers, which nothing
  but the clock wakes, so after another thread's `exit_group` it ran out its timeout, or for
  good without one. That held the process, its address space and its thread stacks, and a
  launcher joining the main thread (the self-tests, the benchmarks) waited forever: a daemon's
  main thread in `pause()` with a worker calling `exit()` hung. Every Linux sleep now goes
  through `linux::thread::sleep_until`, which waits in the futex table on a word of the
  process's address space past every user address (`SLEEP_WORD`, so no program's `futex` can
  name it), with the process's end checked under the bucket's lock before it parks, so
  `end_process`'s `futex::wake_space` wakes it and a sleep that starts after the end returns
  at once. It says whether the process has ended: `rt_sigsuspend`, `pause` and a stop then
  end the thread; the other calls return as they would and the thread stops on its way back
  to user mode (D91). Waking another thread's sleep is also what signals between threads
  need (D91's "Not yet"): a per-thread word can follow then.
- **Soundness: `Parker::unpark` took `&self`.** A timed waiter whose timer already made it
  ready can see `unparked` set without the scheduler's lock and return, dropping its `Waiter`
  and `Parker` from its stack while the waker is still inside `unpark`, with a reference
  argument that must stay valid for the whole call. Nothing was read after the store, so no
  memory was misused, but code added after it would have been a use after free.
  `unpark` is now `unsafe fn unpark(parker: *const Parker)`: it reaches the `Parker` only
  through the pointer, last with the store; the futex table calls it through
  `Bucket::unpark`, with the bucket's lock held.
- **Checked and fine.** Waiter lifetime across wake, timeout and requeue (a timed-out waiter
  re-locks its current bucket and checks `queued`; a requeue's move is seen through the
  bucket index re-check); lock order (bucket, then memory, then scheduler; a requeue's two
  buckets in index order); `futex`'s checks and errors in Linux's order, relative and absolute
  timeouts, the bitset rules and requeue counts; `clone`'s flags, argument order per
  architecture, child stack and return value, `CLONE_SETTLS`; the last thread's status, the
  `CLONE_CHILD_CLEARTID` clear and wake, resources released once; pinned threads never
  migrate.
- **Left as is.** `clone` writes the `CLONE_PARENT_SETTID` and `CLONE_CHILD_SETTID` words
  before the thread is spawned, so an `ENOMEM` leaves an id there for a thread that never
  existed: writing them after the spawn would let the child run before its own word holds
  its id if a write ever passed a preemption point, and callers ignore the words on failure.
  `CLONE_THREAD` with `CLONE_PARENT` stays `ENOSYS` with the other new-process flags.
- **Not yet.** A launcher still waits for the first thread, not the process: a main thread
  that calls `pthread_exit` while workers go on reads as "ended without ending it" to the
  self-tests. A process-wide completion comes with `wait4`. No test yet has a thread waiting
  on a `CLONE_CHILD_CLEARTID` word when the exiting thread wakes it (musl's `pthread_join`
  usually sees the word cleared first).
- **Tests.** A new QEMU self-test, `linux_programs_end_from_a_thread_while_others_wait`: the
  std test program, given `exit-from-a-thread`, exits from a thread while its main thread
  sleeps in `nanosleep` for an hour and another thread waits on a futex for good; the main
  thread's `join` returns, the status is 0 and every frame is freed, on aarch64, riscv64 and
  x86_64 (with `nanosleep` on the old sleep, x86_64 hangs until the runner's 30 s limit).
  Host test `a_thread_of_an_ended_process_does_not_sleep`: after `exit_group`, an hour's
  `clock_nanosleep` and an `rt_sigsuspend` on another thread end that thread at once.
- **Cost.** `futex_wait_wake_round_trip` is faster than on `main` (aarch64 466.05 → 457.05 ns,
  riscv64 563.06 → 550.07, x86_64 482.03 on `main`, 480.03 in the baseline → 473.03; `-icount` makes these exact): a woken waiter's `Parker` is no longer
  reached through a `&Waiter` held across the unpark, and those baseline lines are tightened.
  `linux_page_fault` reads 0.4–0.8% above its baseline on this branch and on `main` alike
  (aarch64 1384.65 against 1384.64 on `main`); nothing else moves. The sleeps themselves are
  not on a benchmarked path; they take a futex bucket lock as well as the scheduler's.

## D93 Robust futex lists: `set_robust_list` remembered, `get_robust_list`, released when a thread exits — Decided by the build loop (Linux's behaviour decided by research)
- **Why.** The robust list is how a lock held by a thread that dies is handed on: the next
  locker sees `EOWNERDEAD` instead of waiting for good. `set_robust_list` was checked and
  ignored (D55), and `get_robust_list` was `ENOSYS`, which makes musl's
  `pthread_mutexattr_setrobust` fail: musl asks for the list once before it allows robust
  mutexes. D91 listed the list as next.
- **Research (set_robust_list(2), get_robust_list(2), futex(2), the kernel's prose documents
  on robust futexes and their ABI, musl's and glibc's mutex code; no kernel source).** The
  head is three words: the first entry (the head's own address when empty), the signed
  offset from each entry to its 32-bit lock word, and the entry being taken or released. An
  entry's first word points to the next; bit 0 of a pointer marks the entry it points to as a
  priority-inheritance lock. At a thread's exit, each lock word whose owner (the low 30 bits)
  is the thread's id keeps its waiters bit (bit 31), gets the owner-died bit (bit 30) and no
  owner, and if it had waiters one waiter on it as a shared futex is woken (never for a
  priority-inheritance lock); the walk stops at the head, after 2048 entries (Linux's cap;
  the ABI document's "more than a million" is a ceiling, not what Linux does), or at the
  first word or pointer that is not the program's memory; the pending entry comes last, and
  is skipped in the list so each lock is handled once. The walk comes before the
  `CLONE_CHILD_CLEARTID` clear and wake: the head lives in the exiting thread's memory,
  which a woken joiner may free. glibc waits on robust mutexes as shared futexes because the
  kernel's wake is shared; musl registers the list only for process-shared robust mutexes
  and releases its private ones itself in `pthread_exit`.
- **What we do.** Each `LinuxThread` keeps its head (0 for none, as a new thread starts);
  `set_robust_list` sets it after its length check (`EINVAL` unless 24, nothing else
  checked). `get_robust_list(tid, head, length)` writes the head and 24 for the caller (tid 0
  or its own id), `EFAULT` if either is not writable, `ESRCH` for another thread until one
  thread can reach another's state, as `tkill` (D91). `exit` of a thread that leaves others
  running walks its list (`futex::release_robust_list`) before the tid clear; a thread with
  no list pays one compare, and nothing is added to the system-call path.
- **Not yet.** The walk is skipped when the process ends (`exit_group`, a fatal signal, the
  last thread's `exit`) and on `execve`: the lock words are the process's own memory and its
  waiters are its own threads, which the end releases anyway (D92), so nothing can observe it
  until memory is shared between processes, when every way a thread ends must walk it. A
  word is read and then written, not swapped atomically: exact while a process's threads run
  on one CPU and the kernel is not preempted (D91), and it needs a compare-and-exchange on
  user memory once they spread. Another thread's list through `get_robust_list`, and the
  credential check Linux makes for another process's, come with signals between threads.
- **From the independent review, open.** A pending entry whose word is 0 (a thread that died
  between unlocking and waking) should still wake one waiter: it matters once a thread can be
  killed alone or the walk runs at the process's end. `execve`, when it comes, clears the
  head. `get_robust_list` writes the head before the size, and a `next` that cannot be read
  ends the walk before its entry's lock is released; both differ from Linux only for programs
  passing bad pointers. No test watches the wake itself yet.
- **Tests.** Host tests: `get_robust_list` for the caller (0 before `set_robust_list`, its
  head after, with size 24), `ESRCH` for another thread's id, `EFAULT`; `set_robust_list`'s
  `EINVAL` keeping the old head; an exit releasing its locks with a negative offset, the
  waiters bit kept, a priority-inheritance entry released, another owner's lock untouched,
  the pending entry released, the tid word cleared after; a list that loops ends at the cap
  and still releases the pending entry; a pointer that is not memory ends the walk, the
  pending entry included. Nothing in the QEMU self-tests uses a robust mutex yet.

## D94 TLB shootdown: a process's threads run in parallel on every CPU — Decided by the build loop (user-memory races and the cost decided by research)
- **Why.** D91 kept every thread of a multi-threaded process on the CPU that called `clone`:
  changed mappings were only marked stale on other CPUs (D53), which is sound only while no
  other CPU runs the process meanwhile. Threads ran concurrently there, not in parallel, so a
  threaded program used one CPU of the machine. D91 named the shootdown as next.
- **The shootdown.** `clone` marks the process's address space spread over CPUs
  (`AddressSpace::spread_over_cpus`, never cleared) before the new thread can run, and new
  threads are placed as any other (`CpuPlacement` and pinning are gone). When a spread
  address space's mappings are removed or changed (`change_pages`, `move_pages`,
  `unmap_pages`), the changing CPU drops its own translations, then asks every other CPU that
  keeps the address space loaded to drop every user translation it has cached
  (`Cpu::request_shootdown`, a per-CPU counter), sends it the wake-up interrupt, and waits
  until each has served the requests asked of it until then (`shootdowns_served` reaching
  what `shootdowns_requested` said). A CPU serves (`cpu::serve_shootdowns`: one full drop of
  its user translations, then the counter) on the wake-up interrupt, so one running the
  program in user mode stops using the old entries before the change returns. An address space
  that was never spread keeps D53's lazy marking: a single-threaded process pays one flag load.
- **No deadlock with interrupts masked.** The kernel runs with interrupts masked, and the
  waiting CPU holds the process's regions lock, which a CPU taking a page fault or copying user
  memory may be waiting for, as may a CPU waiting for a futex bucket whose holder waits for the
  regions lock (the futex value check). So a CPU serves the requests asked of it while it waits:
  in its own shootdown's wait, and while it waits for a contended `SpinLock` (the lock's wait
  duty, set when the secondary CPUs start: `sync::set_wait_duty`). A `LeafSpinLock` skips the
  duty, so taking it stays free of calls: the heap, the scheduler, the frame allocator, page
  tables and each CPU's own locks, whose holders take only leaf locks and never wait for
  another CPU, so a CPU waiting for one waits only for code that finishes on its own.
- **A CPU loading the address space is never missed.** The changing CPU writes the entries,
  makes a full fence, then reads the other CPUs' records; a CPU loading the address space
  stores its record, then switches. That is a store-load pattern on both sides: the loading
  CPU also needs a full fence between its record and its first table walk, or the changer can
  read its old record and skip it while its walk still reads the old entry. x86's `mov cr3`
  and AArch64's `dsb` before the switch give that order, RISC-V's `sfence.vma` does not, so
  `Platform::switch_address_space` now promises it and RISC-V's starts with `fence rw, rw`
  (from the independent review; a fence in the kernel's switch path cost every process switch
  on x86_64 and AArch64 0.5–0.9% for an order they already had).
- **Frames reused only once no CPU can write them.** `munmap`'s frame list is linked through
  the frames themselves (`FramesToFree`), written as each page is unmapped; a CPU still
  writing through its old translation would overwrite the links (the new self-test crashed the
  kernel exactly so before this was fixed). On a spread address space `unmap_pages` therefore
  first makes the pages inaccessible, keeping their frames (`PageChange::Inaccessible`), shoots
  the old translations down, and only then unmaps them and hands the frames over; a thread that
  touches them meanwhile faults and waits for the regions lock.
- **User memory other threads use at the same time (research: Rust-for-Linux's `UserSlice`,
  Asterinas' `VmReader`/`VmWriter`, futex(2), the kernel's robust-futex documents, Linux's
  futex wait path).** Bulk copies stay plain copies through the direct map: the kernel reaches
  user memory only through raw pointers and reads each byte once into its own buffer, so a
  racing write leaves the bytes copied unspecified, as on Linux, and cannot change the
  kernel's control flow (volatile or atomic word copies would cost every large `write` and buy
  nothing). Words used as atomics go through `Process::with_user_word`, an `AtomicU32` on the
  frame's direct-map alias (same physical memory and memory type as the program's mapping, so
  coherent on all three architectures): the futex value check is an acquire load under the
  bucket lock, and a robust lock's release a compare-and-exchange loop, which no longer loses a
  waiter that sets the waiters bit while the owner exits (D93's open point). A lock word on a
  page the program may only read is read and skipped unless the exiting thread owns it, as
  Linux reads before it writes; one it owns ends the walk with `EFAULT`.
- **`getcpu`.** `getcpu(cpu, node, cache)` writes the CPU's number and node 0, each unless
  null, `EFAULT` for memory the program cannot write; musl's `sched_getcpu` uses it, and the new
  self-test checks with it that two threads ran on two CPUs.
- **Not yet.** Every CPU that keeps the address space loaded is asked, not only those running
  the process's threads, and each drops every user translation instead of the range;
  broadcast invalidation (AArch64's inner-shareable `tlbi`, RISC-V's SBI remote fence or
  Svinval) would need no interrupt. `with_active` and `with_user_memory` of a spread address
  space from a CPU that is not running its thread still need it unchanged meanwhile. Signals
  between threads, `wait4` and a process-wide completion come next.
- **Tests.** QEMU self-tests on aarch64, riscv64 and x86_64 with every CPU running: the std
  program's checks again (`std::thread`, `Mutex`, `Condvar` and joins across CPUs), and a new
  mode of it in which one thread unmaps a page another thread writes in a loop on another CPU:
  the writer must fault into its `SIGSEGV` handler at once (without the shootdown the kernel
  crashes); skipped when one CPU runs, as in the benchmarks. Host tests: a shootdown asks only
  CPUs holding the address space and waits until they serve, a CPU serves the requests up to
  when it looked, a spread address space's `unmap_pages` hands every frame over (a mapped and
  a kept page), `getcpu`.
- **Cost (decided by research: unavoidable).** Measured against `main` on the same machine
  (`-icount`, so instruction counts): `futex_wait_wake_round_trip` +1.3% on x86_64 (473.03 →
  479.03 ns) and +1.5% on riscv64 (550.06 → 558.06), unchanged on aarch64;
  `linux_mmap_munmap` +1.7% on x86_64 (360.09 → 366.09), +0.8% on riscv64, +0.6% on
  aarch64; `linux_page_fault` +0.3% on each (its fault takes the regions lock, which keeps
  the duty), which puts riscv64 (1411.10 on `main`, 0.8% above its line already) just past
  the gate at 1415.13; nothing else moves. The first versions cost far more (yield +3–6%, the timer
  interrupt +7%, the heap +4–16%, munmap +3–4%): serving on every interrupt, a call in every
  lock's contended path and a second page walk on munmap; serving on the wake-up interrupt
  only, leaf locks without the duty and one walk with one flag load cut them to the above.
  Experiments placed what is left: the futex cost is the call in the bucket lock's contended
  path (with buckets as leaf locks it is gone), the munmap cost is the `spread` flag and the
  branches of the two-step unmap. Research: a bucket or regions holder can block a shootdown,
  so their waiters must serve, and Linux, Windows and macOS pay for the same deadlock freedom
  with interrupts that stay deliverable while a CPU spins (IRQs on while spinning, the IPI
  level above spin locks' level, a pmap loop that serves pending flushes), which a kernel
  running with interrupts masked has only in software. The cheaper options cost more (the
  futex wait enqueuing, dropping the bucket and re-checking: a second bucket round trip) or
  break a rule (an assembly trampoline that hides the call); broadcast invalidation removes
  the need on AArch64 and RISC-V but not on x86, where the cost is. The baseline lines of
  those three results and riscv64's `linux_page_fault` are updated; the others stay within
  the gate.

## D95 Signals between a process's threads — Decided by the build loop

- **What.** `tkill`, `tgkill` and `rt_tgsigqueueinfo` reach every thread of the caller's
  process, not only the caller (D63, D68, D91). Threads of other processes stay `ESRCH` until
  there are other Linux processes to reach.
- **Where a sent signal waits.** Each thread's blocked mask and pending set stay its own, in
  its `LinuxThread`, which only its loop touches, so the system-call path reads them without a
  lock. A signal for another thread goes into the process's signal block (`sent_to_threads`,
  under the signals lock, next to the actions and the queued `siginfo_t`s), tagged with the
  thread's id, and counts toward `RLIMIT_SIGPENDING` there. The target takes it in later
  (`LockedSignals::take_in_sent`) and handles it exactly as if it were sent then
  (`LockedSignals::send`): blocked or ignored by its mask at that moment, `SIGCONT` and the stop
  signals cancelling, a standard signal kept once with its first `siginfo_t`. Moving every
  thread's state into the process (a table under the lock) would let the sender decide at
  once, but costs a lookup on every signal call, a heap table for single-threaded processes,
  and room in `Process`, which is at its 256-byte class. A real-time signal over the limit is
  `EAGAIN` for the sender, as on Linux; a standard one is queued past it; without memory to
  record any signal it is `EAGAIN` too. What taking in would undo is undone when a signal is
  left, so what waits for a thread that never takes in (blocked in a `futex` wait for good)
  stays bounded however often it is sent signals: a standard signal already waiting is not
  left again (Linux's second instance of a pending standard signal is a no-op too), `SIGCONT`
  removes the stop signals waiting and a stop signal a `SIGCONT` waiting (its discarding of
  the stop signals stays), and a discard request (below) removes the signals it covers: at
  most one entry per standard signal, the real-time ones the limit allows, and one request
  more than signals.
- **How the target notices.** `Process::signals_sent`, a wrapping 16-bit count in the padding
  next to `ended` (no growth), goes up after each signal sent to another thread; each thread
  keeps the count it last saw (a thread that missed exactly 65,536 sends would take them in at
  the next one: the count wraps). The sender then interrupts every other CPU (as `exit_group`
  does, D91) and wakes the target if it sleeps. Sleeps now wait on a futex word per thread
  (`!3 - 4 * tid` in the kernel's half, which no program's futex can name, D92 had one word per
  process), and `sleep_until` returns once a signal was taken in (after a wake it takes in
  whatever the count says). A running thread compares the counts on an interrupt (out of line),
  after `sched_yield` and `futex`, the calls that may switch it out, and before the signal
  calls of its own (`rt_sigprocmask`, `rt_sigpending`, `rt_sigtimedwait`, a signal to itself),
  so they see what was sent before them; the sleeps deliver after they end. Nothing else on
  the system-call path looks: the calls after which signals are delivered are a bit set now
  (one test on the other calls' path).
- **A thread switched back in.** While signals sent to threads wait to be taken in
  (`Process::set_sent_signals_waiting`, set and cleared under the signals lock), a thread the
  scheduler switches back in raises the wake-up interrupt on its own CPU before it runs the
  program (`cpu::interrupt_here`), so it comes back at once and takes them in: without it, a
  thread alone on its CPU (no tick armed) running a loop or calls that never switch would never
  see them. The check is in the branch the scheduler's look already takes (one more load
  there, none on the system-call path), and after the preemption points `write` and
  `getrandom` pass, where a thread switched out resumes without that look. `Platform::wake_cpu` now takes the calling CPU too:
  AArch64 sends the SGI with GICv2's "this CPU only" filter, which a uniprocessor GIC (the
  benchmarks' one CPU) needs; x86_64's fixed IPI and RISC-V's SBI IPI name their own CPU as
  any other. A boot self-test checks that a CPU takes the interrupt it raised on itself.
- **`SIGKILL`** sent to another thread ends the process at once, as on Linux, so a thread
  blocked in a `futex` wait cannot keep it alive.
- **An exited first thread** (Linux's thread group leader, a zombie while others run) takes
  signals and never delivers them: `tgkill` to it returns 0, `SIGKILL` included, which on Linux
  leaves the group alone too (`complete_signal` returns for a thread-directed signal that no
  live thread wants). Other exited threads are `ESRCH`.
- **`SIG_IGN` for every thread.** Setting an action that newly ignores a signal, with other
  threads running, leaves each of them a discard request in the same list as the signals sent
  to them, in order, and counts as a signal sent: each discards the instances pending when it
  was asked, blocked or not, as POSIX requires, and keeps those sent after. A blocked signal
  that its action ignores, by `SIG_IGN` or by default (`SIGCHLD`), stays pending otherwise, as
  on Linux (the first version discarded every ignored pending signal on each take-in, which
  lost a `SIGCHLD` blocked for `sigwaitinfo`).
- **The waits.** `rt_sigtimedwait` waits on after a signal outside its set that is not due, and
  fails with `EINTR` for one that is due (delivered after it); `rt_sigsuspend` and `pause` are
  woken; `poll` and `ppoll` are interrupted by a due signal with or without a mask (Linux's
  `EINTR`; musl's AArch64 and RISC-V `pause` is `ppoll` without one); `nanosleep` and
  `clock_nanosleep` sleep out their time and deliver after; a stopped thread stays stopped.
- **Not yet.** Process-directed signals (`kill`, `rt_sigqueueinfo` to the process) still go to
  the caller, which Linux allows when it does not block them; one it blocks should go to any
  thread that does not (Linux's shared pending). A `futex` wait and the sleeps are not
  interrupted (`EINTR`, `rem`). Every other CPU is interrupted for each signal sent, not only
  the target's (no thread records its CPU yet). A thread that never takes in (blocked in a
  `futex` wait for good) keeps the process's switched-in threads interrupting themselves once
  each time they are switched in.
- **Tests.** Host tests: threads queue real-time signals for each other and each takes its
  own in send order; a signal sent to another thread is pending only once it comes back, then
  delivered (`SIGUSR1` ends the process); `SIGKILL` ends it at once; an exited first thread
  takes signals, `SIGKILL` included, and the process goes on; `SIG_IGN` discards another
  thread's blocked pending signal and leaves a blocked `SIGCHLD` pending; an exited thread is
  `ESRCH`; the GIC's SGI to this CPU. QEMU self-tests on aarch64, riscv64 and x86_64: the std
  program sends `SIGUSR1` with `pthread_kill` to a thread waiting in `sigsuspend` and to one
  running a `sched_yield` loop, and each runs the handler itself; a CPU takes the wake-up
  interrupt it raises on itself (four CPUs, and one in the benchmarks).
- **Cost.** Measured against `main` on the same machine (`-icount`): `linux_syscall_round_trip`
  129.18 ns on both on x86_64; aarch64 124.65 and riscv64 163.27 ns, under their baseline
  lines; every benchmark within the gate. The first versions cost +14% (the thread loop's
  handler inlined twice, for the scheduler's look) and then +2.3% on x86_64 (handing the
  handler an interrupt from the look's branch, with the loop's exit value merged); the look
  now only interrupts its own CPU, out of the loop's way, and costs nothing measurable.
- **Review (independent, before merge).** It found the thread switched back in that never
  took in (above: first left as "within its time slice", which a CPU with no tick armed breaks)
  and the discarded blocked signals; both fixed, with the tests above. Its second pass found
  the same for a thread switched out inside `write` or `getrandom` (fixed above), and that
  repeated `SIG_IGN` requests should merge (they do) and flush again as on Linux (they do).
  Its third found that repeated standard signals to a thread that never takes in grew without
  bound and inflated the system's queued-signal count (now compacted as above, with a host
  test sending one signal a hundred times).

## D96 Signals sent to the process — Decided by the build loop

- **What.** `kill` and `rt_sigqueueinfo` to the caller's process (its id, its group) reach any
  of its threads that does not block the signal, as Linux's shared pending signals do, not
  only the caller (D63, D68, D95).
- **Who takes it.** The caller, at once, as before, when it does not block the signal or is
  the process's only thread: Linux may pick any thread that does not block it, and this one
  costs nothing new. Otherwise the signal waits in the process's signal block, in the list
  D95 keeps for signals sent to threads, tagged for any thread (tag 0, which no thread id is),
  and the first thread that takes in without blocking it takes it, as if sent to it then
  (`LockedSignals::take_in_sent`). Keeping it there, rather than in a shared pending set that
  every delivery check would read, leaves each thread's checks as they are: the system-call
  path and the thread loop read only the thread's own state, without the lock.
- **How the threads notice.** As for a signal sent to a thread (D95): the count of signals
  sent goes up and every other CPU is interrupted, and since the sender cannot see the other
  threads' masks, which are private, every other thread's sleep is woken (one futex wake
  each, under the signals lock, which no futex path takes). A woken thread that blocks the
  signal goes back to sleep.
- **A thread that unblocks it later.** Changing the mask takes in at once a waiting signal the
  new mask no longer blocks: `rt_sigprocmask`, `rt_sigreturn`'s restored mask, and the masks
  of `rt_sigsuspend` and `ppoll` (`LockedSignals::set_blocked`), so a signal every thread
  blocked is delivered by the first to unblock it, as on Linux. `rt_sigtimedwait` takes in
  the signals it waits for while it waits (`ThreadSignals::waited`, Linux's temporary
  unblocking), so `sigwait` in a thread that blocks the signal, the usual way to handle
  signals in one thread, gets it. `rt_sigpending` reports the waiting ones the caller blocks,
  as Linux reports the shared pending set with the thread's.
- **Bounded, and `SIG_IGN`.** What waits stays bounded as in D95: a standard signal already
  waiting is not left again, `SIGCONT` removes the stop signals waiting for any thread and a
  stop signal a `SIGCONT`, a real-time signal over `RLIMIT_SIGPENDING` is `EAGAIN` unless
  sent with `kill`, which Linux queues past the limit too (independent review). An
  action that newly ignores a signal removes its waiting instances too (POSIX).
- **Differences from Linux kept.** A standard signal taken in by a thread that already has
  it pending is merged with it (Linux keeps one in the thread's set and one in the shared
  set, so it may run twice; POSIX allows either). Once taken in, a signal is the thread's: a
  handler's mask blocking it delays it until the handler returns, where Linux could hand it
  to another thread meanwhile; likewise a thread that takes it in on its way into
  `rt_sigprocmask` (which takes in before it changes the mask, so the call sees what was sent
  before it) keeps it if the new mask blocks it. `SIGCONT` sent to the process discards the stop signals waiting
  for any thread and the receiver's, not every thread's pending ones (as D95 does for a
  thread). A signal every thread blocks keeps the process's threads interrupting themselves
  once each time they are switched in, until one takes it (the cost D95 accepted for a thread
  that never takes in).
- **Tests.** Host tests: a signal sent to the process by a thread that blocks it is pending
  for none, reported by `rt_sigpending`, and delivered by the other thread (`SIGUSR1` ends the
  process); real-time signals every thread blocks are taken by `rt_sigtimedwait` in either
  thread in send order, and a standard one by the first thread that unblocks it; a hundred
  `kill`s leave one entry, `SIGCONT` and `SIGTSTP` cancel each other, `SIG_IGN` removes the
  waiting one; the only thread keeps a signal it blocks. QEMU self-tests on aarch64, riscv64
  and x86_64: the std program blocks `SIGUSR1`, `kill`s its own process, and a thread waiting
  in `sigsuspend` with `SIGUSR1` unblocked, and then one running a `sched_yield` loop that
  never blocked it, runs the handler.
- **Cost.** No hot path changes: the caller-takes-it test is on `kill`'s path only, the mask
  changes add one test of the block's list when they unblock something, and the system-call
  round trip and the thread loop are untouched.

## D97 Signals interrupt `futex` waits and sleeps: `EINTR`, `rem`, `SA_RESTART` — Decided by the build loop (the wake-up path decided by research)

- **What.** A signal another thread sends (`tkill`, `tgkill`, `rt_tgsigqueueinfo`, or `kill`
  and `rt_sigqueueinfo` to the process, D95, D96) ends the target's `futex` wait or sleep, and
  the call ends as Linux's does. This replaces D95's "`nanosleep` and `clock_nanosleep` sleep
  out their time and deliver after" and its "Not yet" for `futex`.
- **How the call ends**, decided once the first handler to start is known (Linux decides it in
  `handle_signal` from the call's `-ERESTART*` code):
  - `nanosleep` and a relative `clock_nanosleep` write the time left to `rem`, unless it is
    null, as soon as a signal is due (Linux's `nanosleep_copyout`), then fail with `EINTR` if
    a handler starts, whatever its `SA_RESTART` (Linux's `-ERESTART_RESTARTBLOCK`); an
    unwritable `rem` makes the call fail with `EFAULT`, the sleep over, as on Linux. An
    absolute `clock_nanosleep` writes nothing.
  - A `futex` wait with no timeout is made again once the handler returns if its action has
    `SA_RESTART`, and fails with `EINTR` otherwise (Linux's `-ERESTARTSYS`); with a timeout it
    fails with `EINTR` (`-ERESTART_RESTARTBLOCK`).
  - Time comes before signals, as on Linux: a sleep whose time is up returns 0 and a `futex`
    wait whose deadline has passed `ETIMEDOUT` (after `EAGAIN` for a changed word), a signal
    due or not, which is delivered after.
  - If no handler starts (the due signals were discarded: a stop signal in an orphaned process
    group, an action changed to `SIG_IGN` meanwhile), the sleep or wait goes on in the kernel to
    the same deadline, the futex word checked again, which is what Linux's restart does. A
    `SIGKILL` or a default action that ends the process ends it; `SIGSTOP` stops the thread.
- **Restarting a call** is the HAL's new `UserContext::restart_syscall(number,
  first_argument)`: the program counter goes back over the call instruction (`svc` and `ecall`
  are 4 bytes, `syscall` 2) and the registers the result overwrote get the call's number and
  first argument back, so the handler's frame saves a context that makes the call again when
  `rt_sigreturn` restores it, as Linux's restart does. It is applied right before the first
  handler's frame is written (`signals::deliver_interrupting`, `Interrupted`).
- **How the sender reaches a waiting thread** (research: three options weighed). Each Linux
  thread has a wait slot (`futex::WaitSlot`, the index of the futex bucket its waiter is in,
  kept by `requeue` when it moves the waiter), shared with the process's signal block, which
  records every thread's slot (the first thread's once it starts the second). A sender bumps
  the count of signals sent and calls `futex::interrupt` on the target's slot: it locks that one
  bucket, finds the waiter by its slot and ends its wait with `EINTR`; every sleep waits on the
  thread's own sleep word in the same table, so one path covers sleeps and `futex` waits. A
  waiter publishes its slot and a sender bumps the count, each followed by a `SeqCst` fence,
  so either the sender finds the slot or the waiter sees the count before it waits (its
  `still_expected` check fails with `EINTR`). Rejected: scanning all 256 buckets per signal
  (every `tgkill` would touch every bucket of every process: Go's runtime sends `SIGURG` to
  threads every 10 ms, and its idle threads almost always sit in a `futex` wait), and Linux's
  model of an interruptible park in the scheduler (it changes the scheduler core to cover the
  same two kinds of wait). Linux's `signal_wake_up` likewise touches only the target.
- **A stopped thread** (`SIGSTOP`) now waits for its process's end without taking in signals,
  so one sent to the process stays for its other threads (the D96 review's follow-up).
- **Differences from Linux kept.** A signal the waiting thread blocks still ends its `futex`
  wait for a moment (the sender cannot see the target's mask, D96); the thread checks the word
  again and waits on, so a word that changed with no `FUTEX_WAKE` returns `EAGAIN` where Linux
  would sleep on; callers loop on `futex` anyway (a spurious return is allowed). Every other
  CPU is still interrupted for each signal sent (D95).
- **Not yet.** The signal block records the first thread's wait slot once, when it starts the
  second; `execve` (and `fork`), when they replace or copy a process's threads, must record
  the new first thread's slot, or signals to it stop ending its waits (independent review).
  `send_to_process` interrupts each thread with the signals lock held, which grows with the
  thread count; taking the slots out and interrupting after the lock, as `send_to_thread`
  does, goes with interrupting only the target's CPU. The std program's parked thread cannot
  tell a restarted wait from one that returned (`park` loops); the host test checks the
  restart.
- **Tests.** HAL host test: a restarted call's program counter, number and arguments on the
  host's architecture. Kernel host tests: a signal sent by another thread interrupts
  `nanosleep` and a relative `clock_nanosleep` (handler runs, `rem` holds the time left, the
  call returns `EINTR` after `rt_sigreturn`), an absolute one (`rem` untouched), and one with
  an unwritable `rem` (`EFAULT`); a `futex` wait interrupted with and without `SA_RESTART` and
  with a timeout (restarted only in the second case: program counter, number and arguments as
  made), one whose word changed (`EAGAIN`) and one whose deadline has passed (`ETIMEDOUT`),
  the handler still run, and a sleep of no time with a signal due (0). QEMU self-tests on
  aarch64, riscv64 and x86_64: the std program's thread sleeping in `nanosleep` for 10 s ends
  with `EINTR` and the time left once another thread's `pthread_kill` runs its handler, and a
  thread parked in `std::thread::park` (a `futex` wait) runs the handler and stays parked
  until unparked.
- **Cost.** Measured against `main` on the same machine (`-icount`), every benchmark identical
  but `futex_wait_wake_round_trip`, which got faster: 459.05 → 455.05 ns (aarch64), 558.06 →
  554.06 (riscv64), 479.03 → 473.03 (x86_64); its baseline lines are updated. The kernel's own
  waits (`futex::wait`) are not interruptible and keep their bucket index in the waiter as
  before, with one more word (the thread's slot, none for them); a new waiter is linked in with
  the link it was built with instead of writing it again, which pays for that word and more;
  an interruption is marked by the waiter's key, with no flag of its own. The first version
  made every wait publish and clear a slot: +4.4% (aarch64), +2.5% (riscv64), +1.7% (x86_64).
  The Linux thread's waits pay the slot's store, a fence and a clear, against a park and a
  thread switch. The system-call round trip is untouched: the sleeps and `futex` moved from
  the calls delivered after into the ones that deliver themselves, on the out-of-line path.

## D98 Device interrupt routing and console input — Decided by the build loop

- **What.** The console UART's receive interrupt reaches the boot CPU on every architecture
  brought up, and the bytes reach a queue in the kernel. This finishes M3: device interrupt
  routing, which D26 left "until the first interrupting device needs it". It is item 1 of the
  v0.3.0 plan (D99); item 2 (console `read` with a line discipline) reads the queue.
- **Routing, per architecture.**
  - aarch64: GICv2 SPI 33 (QEMU `virt`'s UART0, level-triggered), targeted at the calling CPU,
    priority below the timer's and the wake-up's. The GIC driver routes any SPI with its
    trigger mode (`route_shared_interrupt`).
  - riscv64: a PLIC driver of our own (priorities, per-context enables, threshold,
    claim/complete). UART0 is source 10 (QEMU `virt`), routed to the boot hart's
    supervisor-mode context, found from the device tree: the PLIC's `interrupts-extended` pair
    that names the hart's `interrupt-controller` with cause 9, never `2 × hart + 1`. The kernel
    maps the PLIC's priority and enable pages and 32 contexts' pages (16 harts), not its whole
    64 MiB.
  - x86_64: an I/O APIC driver of our own. COM1's ISA line 4 goes to the global system
    interrupt and signalling the MADT's Interrupt Source Override gives (ISA's own, active high
    and edge, when none does), on the I/O APIC whose range holds it, as vector 0x32 to the boot
    CPU. The MADT's I/O APIC pages are early device regions.
  - The UARTs: PL011 FIFO on (with the UART disabled around the line-control write, as the PL011
    manual requires) and receive and receive-timeout interrupts on; NS16550 FIFOs on with a
    one-byte trigger, receive-data interrupt on, OUT2 raised (a PC wires the interrupt through
    it).
- **Ending the interrupt.** A device interrupt is ended at its controller after the kernel has
  read the UART (`Interrupt::ConsoleInput`'s contract), so a level-triggered line is no longer
  asserted and the controller does not raise it again for bytes already taken; an edge-triggered
  one sees a new edge for each new byte, since the UART stays asserted while bytes wait. The
  timer and wake-up are still ended first, as before.
- **The kernel side.** `console::receive_input` drains the UART into a 1 KiB queue under a leaf
  lock (interrupts are only taken at preemption windows, in idle and in user mode, D37, so no
  handler finds it held on its own CPU); what does not fit is dropped and counted.
  `console::read_input` takes from it. 1 KiB rather than Linux's 4 KiB terminal buffer: the
  queue is emptied by the interrupt that fills it and read by the line discipline (D99 item 2),
  and every byte of static memory counts against D24.
- **Testing.** The QEMU runner waits for `EGGOS-CONSOLE: input ready`, which the kernel prints
  once input is on, then types a script on the serial console (QEMU empties a UART's FIFO when
  the kernel turns it on, so earlier bytes would be lost). The self-test
  `console_input_arrives_by_interrupt` checks the script arrives whole and in order through the
  interrupt, the only path that fills the queue, on aarch64, riscv64 and x86_64. Only kernels
  built for the boot tests (`qemu-test`) check it: a kernel booted by `cargo xtask run` or from
  a release image has nobody typing the script, and its boot must not wait. It waits by
  taking interrupts without sleeping, so under `-icount sleep=off` (benchmarks) virtual time
  does not run ahead of the host typing. The host test platform types the same script.
- **Cost.** Measured against main on the same machine: no benchmark slower. The first version
  added 2–9% to the AArch64 and RISC-V trap round trips: the new call made the trap handlers
  save the return address on entry, and a switch on the interrupt id tested the wake-up SGI
  before the timer. The fixes keep the timer's and the wake-up's paths instruction for
  instruction as they were: the console paths are cold and out of line (`console_input`); on
  AArch64 UART0's id is tested after the timer's, with `cold_path`; on RISC-V the PLIC is
  reached through a tail call after the timer and wake-up checks, so the handler's prologue is
  main's; the kernel's `receive_input` is out of line too.
- **Not yet.** Addresses and interrupt numbers stay QEMU's, as D12's, until the driver model's
  discovery (M5, issue #97) reads them from the device tree and ACPI; GICv3, AIA and MSI-X come
  with it. Only the boot CPU takes console input.

## D99 The v0.3.0 plan: the first interactive release on 2026-11-01 — Decided (owner, 2026-10-26)

- **What.** The owner set the next release: v0.3.0, the first interactive one (pre-alpha, QEMU
  only), on 2026-11-01. One file per architecture (aarch64, riscv64, x86_64) and one script boot
  EggOS in under a second into a QEMU window with its own graphical desktop and terminal (and on
  the serial console), at an `eggsh` prompt where `ls`, `cat`, `uname`, `echo`, `sleep` and
  `poweroff` run as Linux-ABI programs of our own from an initramfs embedded in the kernel ELF.
- **Order.** The roadmap's "v0.3.0 plan" section lists the must-haves and nice-to-haves in the
  owner's order; until v0.3.0 ships it comes before the milestones' own order and the CTO
  review's issues (only `P0` and `release` issues jump it). Feature freeze 2026-10-30 07:00;
  checkpoints and cuts as the plan says.
- **Releases held.** An owner-requested exception to "release every finished milestone"
  (CLAUDE.md): milestones finished this week (M3 by D98, among others) are not released on their
  own but batched into v0.3.0. The only other release before it is the `v0.3.0-rc.1`
  pre-release on 2026-10-31, to rehearse the release steps. This ends once v0.3.0 is out.

## D100 The console's line discipline: `read`, its settings, `POLLIN` and `^C` — Decided by the build loop

- **What.** Item 2 of the v0.3.0 plan (D99): the console's input (D98) goes through a line
  discipline, and programs read it with `read` on descriptors 0 to 2, change its settings with
  the terminal `ioctl`s and wait for it with `poll` and `ppoll`. This supersedes D61's "`read`
  stays `ENOSYS`", its "never readable" and its fixed `TCGETS` answer with `TCSETS*` `ENOTTY`,
  and D98's raw input queue.
- **The discipline** (`console::line_discipline`, POSIX's general terminal interface as
  termios(3) describes it, Linux's flag values) is pure logic in the kernel's core, one for the
  console, fed from the receive interrupt and echoing through the UART there. Canonical mode:
  input is edited a line at a time and a read returns at most one line (the rest of a long one
  next); delete (`VERASE`) and backspace (`^H`) erase a character (a whole UTF-8 one with
  `IUTF8`), rubbed out with backspace-space-backspace with `ECHOE`; `^U` (`VKILL`) kills the
  line, rubbed out with `ECHOE`, `ECHOK` and `ECHOKE` (otherwise echoed, with a newline with
  `ECHOK`); with `IEXTEN`, `^W` erases a word (letters, digits and `_`, after whatever else ends
  the line, as BSD's `ALTWERASE`) and `^V` takes the next byte as it is (`ISTRIP` applies, and
  it is echoed only with `ECHO`, so a password typed without echo never shows); newline, `VEOL`
  and `VEOL2` end a line and are read with it; `^D` (`VEOF`) pushes the line without a newline,
  or, at the start of a line, reads as an end of file (`read` returns 0). `ICRNL`, `INLCR`,
  `IGNCR` and `ISTRIP` map input first. With `ISIG`, `^C` and `^\` discard the input (but with
  `NOFLSH`), echo as `^C` and `^\` (no newline, as on Linux: the shell writes its own), and
  raise `SIGINT` and `SIGQUIT`. Control characters echo as `^X` with `ECHOCTL`; `ECHONL` echoes
  a newline without `ECHO`. Non-canonical mode makes each byte readable as it arrives, and
  `VMIN` and `VTIME` decide how a read waits, as termios(3) says (both 0: what is there; `VMIN`
  0: up to `VTIME` tenths for a byte; `VTIME` 0: `VMIN` bytes, or `count` if fewer; both: a
  first byte, then until `VMIN` or a `VTIME` with nothing new). Switching to non-canonical mode
  makes the line being edited readable; switching back makes what is unread one line.
- **Backspace erases too.** Linux reads `^H` as a character when `VERASE` is delete; the plan
  asks for both, as terminals send either for the backspace key. BSD has the same with its
  second erase character (`VERASE2`, `^H` by default); termios on Linux has no slot for one, so
  `^H` is a fixed second erase in canonical mode.
- **Not acted on yet, kept and reported:** `VSUSP` (`^Z` would send `SIGTSTP`, whose default
  stops the thread for good until job control can continue it, D97), `VREPRINT`, `VDISCARD`,
  `IXON` flow control (`VSTART`, `VSTOP`), and the output flags: the UART driver writes newline
  as carriage return and newline whatever `OPOST` and `ONLCR` say. The control flags are kept
  as set; the line's speed and format do not change (`TCGETS2` reports 115200 baud).
- **One buffer of 2048 bytes** holds the unread input and the line being edited, with two
  256-byte bitmaps (where lines end, and end-of-file markers that are never read): 2.5 KiB of
  static memory in place of D98's 1 KiB queue. Linux has 4096 bytes, macOS's `MAX_CANON` is
  1024, POSIX's minimum 255; a serial console's lines are short, and every byte counts against
  D24. The static discipline starts all zero (every setting off) and gets a serial console's
  settings when the kernel enables input at boot, so it lives in `.bss` and adds nothing to
  the kernel image (with the settings in its initial value it was 2.6 KiB of `.data`). A line
  keeps room for the byte that ends it; bytes that find no room are dropped and counted.
- **`read`** (`linux::terminal::read`): `EBADF`, then `EFAULT` for a buffer that does not end in
  user space, then 0 for a count of 0; at most `MAX_RW_COUNT` bytes, copied a page-bounded chunk
  at a time (a chunk that cannot be written is lost, and the call returns what it copied, or
  `EFAULT`). It waits on one futex key in no process's address space (`console::INPUT_KEY`),
  whose waiters the interrupt wakes after it drops the discipline's leaf lock whenever input
  becomes readable, a signal character is typed or the settings change; a process's end wakes
  them too when it has other threads. A reader woken for input that went before it could take it
  (another thread read it first, or `^C` discarded it) waits again rather than return 0, which
  only an end of file, `VTIME` passing, or `VMIN` and `VTIME` both 0 return (independent
  review). A signal due before input is taken interrupts it as Linux's `-ERESTARTSYS` (restarted
  after an `SA_RESTART` handler, `EINTR` otherwise, D97); one that came with the input is
  delivered once it is taken, as Linux returns the data first.
- **`poll` and `ppoll`** report `POLLIN` (and `POLLRDNORM`) on the console when a read would
  take input without waiting for more: a whole line (or an end of file) in canonical mode,
  otherwise `VMIN` bytes, at least one, or one when `VTIME` is set. A call asking for the
  console's input waits on the same key, so input wakes it; the others sleep as before. x86_64's
  `poll` moved to the calls that deliver signals themselves.
- **The `ioctl`s:** `TCGETS`, `TCGETS2`; `TCSETS`, `TCSETSW` (output is written at once, so it
  does not wait), `TCSETSF` (discards the unread input first) and their `termios2` forms;
  `TCFLSH` (`TCIFLUSH`, `TCIOFLUSH` discard the input; `TCOFLUSH` has nothing to discard; any
  other argument `EINVAL`); `FIONREAD`, the bytes of whole lines in canonical mode; the window
  size as before.
- **Where `^C` goes** until process groups and `TIOCSPGRP` (D99, item 7): the console's
  foreground is the thread that last read it or polled it for input (`console::Foreground`). The
  interrupt sends its process the signal with Linux's kernel `siginfo_t` (`SI_KERNEL`), queued
  for the process (the signal state's block made on demand, so a process with one thread that
  never changed an action can take it), and ends the wait of that thread, whose slot the
  process's signal state does not record when it is the only thread (D97), and of the others it
  records. The console holds the foreground's process (an `Arc`) until the process ends
  (`linux::end_process` forgets it), so the interrupt never drops the last reference to a
  process; a thread that becomes the foreground after its process's end forgot it forgets it
  again at once. A reader that already is the foreground pays one atomic load; a process that
  ends without being it, one load and a compare. The echo of each chunk of input is written to
  the UART after the discipline's lock is released, so other CPUs do not spin on it for the
  UART's time (only an echo longer than 256 bytes, such as a long line killed, is written while
  it is held).
- **Testing.** Host unit tests of the discipline (each behaviour above by name); kernel host
  tests of `read` (lines, end of file, partial reads, `EBADF`, `EFAULT`), non-canonical reads
  with `VMIN` and `VTIME`, a read interrupted with and without `SA_RESTART` and one that finds
  input with a signal, `^C` ending the process that read the console, `ppoll` reporting
  `POLLIN`, and the `ioctl`s. In QEMU on aarch64, riscv64 and x86_64: the runner types a
  script each time the kernel or a test program writes that script's line (`EGGOS-CONSOLE:
  type ...`), and checks the console shows the lines' echo, rubbed out as edited; the std test
  program, given `read-the-console`, checks the settings (`tcgetattr`), polls for a line, reads
  the edited lines and an end of file, is interrupted by `^C` in `ppoll` (`EINTR`, its handler
  run) and in a `read` restarted after its `SA_RESTART` handler, then reads raw bytes with `VMIN`
  1, returns 0 at once with `VMIN` 0 and after a tenth of a second with `VTIME` 1, and restores
  the settings with `tcsetattr(TCSAFLUSH)`. The kernel's own input self-test now reads its
  script as one line. A script is typed only once its line has ended (a partial line that
  matched a shorter request typed an extra `^C` once). The benchmark kernels skip the
  program's test, and their verdict needs neither its line nor the echo: under `-icount
  sleep=off` QEMU does not wake an idle RISC-V hart for serial input (observed: the same
  kernel wakes without `-icount` and with `sleep=on`), so the benchmark boot hung once the
  program waited.
- **Cost.** Every benchmark as on `main` (within 0.25%, same machine): the receive path stays
  cold and out of line (D98), and the system-call dispatch did not move. Kernel image (release,
  `.text` / `.rodata` / `.data` / `.bss` against `main`): aarch64 +16.1 KiB / +0.4 / 0 / +1.6,
  riscv64 +12.7 KiB / +0.6 / 0 / +1.6, x86_64 +15.5 KiB / +0.7 / 0 / +1.6, about 6% of
  `.text`: the line discipline, `read` with its waits, the settings `ioctl`s and `poll`'s
  wait for input, the console's first interactive feature (D24: growth justified by a
  must-have of D99; nothing is added to a path a program pays without reading the console).
- **Not yet.** `readv` (musl's stdio reads with it, so C programs reading standard input wait
  for it); `O_NONBLOCK` reads (no `fcntl` yet); the actions listed above; tab erase counts
  columns from where the line began, not from the column the output left (after a prompt), which
  needs the output's column tracked; a long echo written under the lock; virtio-console input
  (M8).

## D101 A descriptor table per process: `close`, `dup`, `dup2`, `dup3`, `fcntl`, `close_range`; `readv` and `O_NONBLOCK` on the console — Decided by the build loop (where the table lives and the accepted cost decided by research)

- **What.** Item 3 of the v0.3.0 plan (D99): each process has a descriptor table, and the
  console is an ordinary open file description in it. This supersedes D61's "descriptors 0 to 2
  are the console" and finishes D100's `readv` and `O_NONBLOCK` follow-ups.
- **The model** (`linux::descriptors`, POSIX's file descriptors and open file descriptions,
  as close(2), dup(2), fcntl(2) and close_range(2) describe them). A descriptor names a
  reference-counted open file description (`OpenFile`): what was opened (`FileKind`), its access
  mode, its status flags and its offset, which every descriptor duplicated from it shares; the
  close-on-exec flag is the descriptor's own. `FileKind` is an enum with the console as its
  first kind: the VFS (item 4, issue #94) adds its nodes as more kinds, and `read`, `write`,
  `ioctl`, `lseek`, `fstat` and `poll` already go through one match on the kind. A process
  starts with descriptors 0, 1 and 2 on one description of the console, opened for reading and
  writing, as Linux starts `init` with `/dev/console` opened once and duplicated, so
  `O_NONBLOCK` set through one holds for all three, as on Linux.
- **Where the table lives** (decided by research). Each thread holds an `Arc` of the table
  (`Descriptors`, in its `LinuxThread`), as Linux's tasks point to a files table, rather than a
  field in `Process`: `Process` is exactly at the heap's 256-byte class (D59), a thread is the
  caller of every call that uses the table, and the table is dropped with the last thread that
  holds it, while `Process` can outlive its threads. `clone` with `CLONE_FILES` shares it; without
  it the new thread gets a copy (Linux allows `CLONE_THREAD` without `CLONE_FILES`), and `fork`
  (item 6) will copy it the same way. `execve` (item 5) closes the descriptors marked
  close-on-exec (`remove_first_closed_on_exec`). Nothing reaches the table except through a
  thread, so `/proc/<pid>/fd` will need the process's threads.
- **The table** is a vector of slots, one per number, behind a leaf lock: its holder takes no
  lock but the heap's and never waits for another CPU. It is never held while user memory is
  copied or a thread waits, and no open file description is dropped under it (closed ones are
  handed back to be dropped after it is released), so a later description whose last close
  takes other locks can join it. A lookup takes the lock, counts one more reference and releases
  it: no allocation. A new descriptor takes the lowest free number, from a hint below which every
  slot is taken, below the process's `RLIMIT_NOFILE` soft limit (D59); the table grows by
  doubling (8 slots at least, never past the limit) and shrinks when its highest descriptors
  close. A table of the largest allowed limit (524,288 slots of 16 bytes) needs 8 MiB, past the
  heap's 4 MiB blocks (D31), so `dup2` that high fails with `ENOMEM`; Linux allocates it.
- **The calls**, with Linux's errors in Linux's order: `close` (`EBADF`); `dup` (`EBADF`,
  `EMFILE`); `dup2` on x86_64 only (AArch64 and RISC-V have no such call; their C libraries'
  `dup2` calls `dup3`), returning the descriptor when both are the same and it is open; `dup3`
  (`EINVAL` for a flag other than `O_CLOEXEC` and for the same descriptor twice, then `EBADF`
  for a target not below the limit, then for a source not open); `fcntl` (`EBADF` first whatever
  the command; its argument an `int`, the register's upper half ignored, as C libraries leave it
  undefined; `F_DUPFD` and `F_DUPFD_CLOEXEC`, `EINVAL` for a lowest descriptor not below the
  limit (a negative one never is) and `EMFILE`; `F_GETFD` and `F_SETFD` with `FD_CLOEXEC`; `F_GETFL`; `F_SETFL` changing
  `O_APPEND` and `O_NONBLOCK`; `EINVAL` for every other command); `close_range` (`EINVAL` for an
  unknown flag or a reversed range, `CLOSE_RANGE_UNSHARE` giving the caller its own copy first,
  `CLOSE_RANGE_CLOEXEC` marking instead of closing, descriptors not open skipped); and the
  `ioctl`s every descriptor answers before the console's, `FIOCLEX`, `FIONCLEX`, `FIONBIO` and
  `FIOASYNC` (its `int` read, `EFAULT` if it cannot be, and ignored, as `F_SETFL` ignores
  `O_ASYNC`).
  `read`, `readv`, `write`, `writev`, `ioctl`, `lseek`, `fstat`, `newfstatat`, `statx`, `poll`
  and `ppoll` look descriptors up in the table: `EBADF` for one not open, and for `read`/`readv`
  or `write`/`writev` on one not open for that.
- **`readv` on the console** takes one read's worth of input (a line in canonical mode) into its
  buffers one after another, checked as `writev`'s are (D62); musl's stdio reads with two
  buffers, the caller's and the stream's own, so `fgets` on standard input works.
- **`O_NONBLOCK` reads.** Where a read would wait, it returns what is there or fails with
  `EAGAIN` if nothing is: no whole line in canonical mode, no byte otherwise. POSIX lets a
  non-canonical read return 0 or `EAGAIN` there (termios(3)); `VMIN` and `VTIME` both 0 never
  wait, so that read returns 0 with or without `O_NONBLOCK`. `poll` is unchanged.
- **Kept out for now** (each ignored or `EINVAL` until something acts on it): `F_SETFL`'s
  `O_ASYNC`, `O_DIRECT` and `O_NOATIME`, and `FIOASYNC`'s; record locks (`F_GETLK`,
  `F_SETLK`, `F_OFD_*`), `F_SETOWN` and the other commands; `O_LARGEFILE` in `F_GETFL`, which
  `openat` will set.
- **Follow-ups** (from the independent review): a borrowed lookup with no reference counted
  while the table is not shared, as Linux's `fdget` does; whether a reader spinning in
  `take_input` for an echo should serve TLB shootdowns meanwhile (it holds no lock, and the
  echo's writer runs to its end with interrupts masked); `O_LARGEFILE`; the older `///` docs on
  private items in the large files this change touched (`linux.rs`, `terminal.rs`, `poll.rs`,
  `bench.rs`), removed file by file in cleanup PRs.
- **Review fixes to D100** (in the same change): a byte the line discipline drops for lack of
  room is no longer echoed; a reader that took input waits until the receive interrupt has
  written its echo (a count of echoes being written, one atomic load on the read path), so the
  echo comes before the reader's output when the interrupt runs on another CPU; becoming the
  foreground and a process's end are ordered by SeqCst fences, so the console never keeps an
  ended process; a non-canonical read with `VMIN` 0 keeps its `VTIME` deadline when woken for
  input another reader took; one with `VMIN` and `VTIME` set whose bytes another reader took
  waits for a first byte again instead of returning 0.
- **Testing.** Host unit tests of the table (lowest-free allocation, the hint, the limit and
  `EMFILE`, shared descriptions and per-descriptor close-on-exec, duplication over an open
  descriptor, shrinking, `close_range` with its flags, copies, the register's upper half) and of
  the calls through the Linux layer (every call's errors, a closed standard input, `RLIMIT_NOFILE`
  lowered, `FIONBIO`, `O_NONBLOCK` reads in both modes, `readv` across buffers, a read with
  `VMIN` and `VTIME`). In QEMU on aarch64, riscv64 and x86_64, the std program duplicates,
  flags and closes descriptors through musl (`dup`, `dup2`, `dup3`, `fcntl`, `close`,
  `close_range`), reads `EBADF` from a closed standard input and restores it, and sees a
  descriptor closed by another thread; given `read-the-console`, it reads a line with
  `O_NONBLOCK` (`EAGAIN` before and after it), then a line with musl's `fgets` and one with
  Rust's `read_line`.
- **Cost.** Every call on a descriptor (`read`, `write`, `writev`, `ioctl`, `lseek`, `fstat`,
  `poll`, ...) now finds it in the table: the table's lock taken and released and the open file
  description's count taken and dropped, where it compared the number with 2 before. Two new
  benchmarks gate it (docs/benchmarks.md): `linux_descriptor_lookup`, that lookup alone, and
  `linux_dup3_close` (`dup3` to descriptor 3, then `close`, without the trap): 36.03, 28.03
  and 27.02 ns, and 184.52, 201.11 and 189.09 ns, on aarch64, riscv64 and x86_64. The system-call dispatch and the calls that take
  no descriptor are unchanged. Every other benchmark is as on `main` but for the one-time table
  of each process: about 245 instructions per process (three allocations and their frees),
  +0.03% on `linux_page_fault` (one process per 512 pages) and +0.02% on
  `linux_syscall_round_trip` (one per run). aarch64's `linux_page_fault` baseline line was already 1.02%
  below what `main` measures, so only it crossed the gate; research judged the cost the minimal
  price of a feature every process needs, as Linux pays for its files table at `clone` (a lazy
  table would put a branch on every lookup instead), and the line moved to 1388.94 (D78). Kernel
  image (release, `.text` / `.rodata` / `.data` / `.bss` against `main`): aarch64 +12.0 KiB /
  +0.7 / 0 / +16 bytes, riscv64 +9.7 KiB / +1.7 / 0 / 0, x86_64 +9.5 KiB / +1.1 / 0 / +16
  bytes: the table, seven calls, `readv` and the D100 review fixes (D24: growth justified by a
  must-have of D99).

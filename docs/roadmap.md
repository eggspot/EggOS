# EggOS roadmap

Status keys: **done**, **in progress**, **next**, planned (no marker).
Updated by every build-loop run; the order is refined as we learn (reasons in `docs/decisions.md`).

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo skeleton: workspace, pinned toolchain, HAL layout, CI, docs | **done** |
| M1 | Kernel boots in QEMU and prints, on the first architectures, with an automated QEMU test runner | **done** — aarch64, riscv64, x86_64 |
| M2 | Memory management: physical frames, paging, kernel heap | **done** (0.2.0) — memory map from the device tree / PVH and a frame allocator on aarch64, riscv64, x86_64 (D28); kernel page tables: a direct map in the high half, W^X kernel sections and a boot-stack guard page on aarch64, riscv64, x86_64 (D29), with the image's direct-map alias keeping its protections (D32); kernel heap: `alloc` works, size classes up to 2 KiB (D30); buddy allocator: contiguous, aligned blocks of up to 4 MiB, so heap allocations up to 4 MiB work (D31); the kernel runs in the top 2 GiB with boot page tables built before any Rust code, and the low half is empty (D33) |
| M3 | Interrupts and timers | **in progress** — exception vectors (D25); monotonic clock and one-shot timer interrupts through GICv2, SBI and the local APIC on aarch64, riscv64, x86_64 (D26); still to do: device interrupt routing (GIC SPIs, PLIC/AIA, I/O APIC), when the first interrupting device driver needs it (M8) |
| M4 | Scheduler, processes and threads (SMP-ready) | **in progress** — kernel threads with a context switch on aarch64, riscv64, x86_64 and a cooperative round-robin scheduler on one CPU, 16 KiB stacks with an overflow canary (D35); voluntary preemption: a 10 ms timer tick ends time slices and threads switch at preemption points, the scheduler's lock handed over across the switch (D37); sleeping and wait queues: an idle thread, `sleep`, `WaitQueue`, a blocking `join`, threads linked through themselves, and one timer shared by the tick and the sleepers with no tick while one thread is runnable (D39); secondary CPUs: discovered from the device tree / MADT, started (PSCI, SBI HSM, INIT-SIPI with a real-mode trampoline) on the kernel's page tables, each with a per-CPU block behind its per-CPU register, reporting in and parking (D40); next: per-CPU run queues so the other CPUs run threads, then processes (address spaces, user mode, preempted on return to user mode) |
| M5 | Common driver model (bus enumeration, device tree / ACPI discovery) | |
| M6 | Linux syscall ABI: what a static hello-world needs (`write`, `exit_group`, `brk`, `mmap`, ...) | |
| M7 | VFS and a first file system | |
| M8 | virtio drivers (console, block, net, rng, input) | |
| M9 | More architectures through the HAL | |
| M10 | Userland: init, shell, core utilities, and the built-in MCP server with its permission model (default deny, local only): every command is also an MCP tool (D21, D22); first size/memory/boot budgets (D24) | |
| M11 | Graphics stack (virtio-gpu first) and compositor | |
| M12 | Adaptive shell, designed by Claude Fable from `docs/ux/` (D16, D23): desktop first, then phone/tablet and TV | |
| M13 | Real hardware and vendor GPU drivers | |
| M14 | Component manager (D18) and app capability registry: installed apps published as CLI commands and MCP tools; agent permissions and audit log (D21) | |
| M15 | Compatibility layers, native not emulated (D17), each an optional component exposed to AI through M14: Linux userland, Windows and macOS side by side, then Android and iOS (order: D34) | |
| M16 | Editions as component presets (D18): personal, professional, enterprise, server | |

Timer follow-ups (D26): on x86_64, read the TSC/APIC frequencies from CPUID 0x15 and hypervisor
leaves instead of measuring, TSC-deadline mode, and the ACPI PM timer or HPET as fallback
reference clocks when there is no 8254; `stimecmp` directly when RISC-V Sstc is present; GICv3; device-tree timebase and GIC
addresses (M5).

Paging follow-ups (D29, D33): a separate exception stack (x86 IST, AArch64/RISC-V stack switch) so a guard-page hit is reported; TLB maintenance for changing live tables (M4); honour device-tree `no-map` reservations (M5); AArch64 data-cache maintenance for memory written with the MMU off, the boot tables included, and a boot direct map built from the device tree instead of QEMU `virt`'s fixed device/RAM split (real hardware, M13); secondary CPUs start on the kernel's tables, global kernel pages and PCID (M4); a device window behind a mapping call instead of the direct map's (M5); RAM above 4 GiB on x86_64; on x86_64, map ACPI tables on demand and reserved PVH ranges uncached instead of direct-mapping them cached (M5; early device pages are already left out, D38); keep the PVH start info and memory map in `physical_memory` even when a loader map has more than 62 disjoint entries; hardware probes in the boot self-tests that the low half, the image's direct-map alias and the stack guard page really fault (they check the tables in software today); 1 GiB pages on x86_64 (CPUID); Sv48/Sv57 chosen at boot on RISC-V when Sv39's 256 GiB is not enough; Svpbmt.

Thread follow-ups (D35): guard pages below thread stacks in a kernel virtual area, with a per-CPU
cache of free stacks, once exception stacks exist; a self-test measuring stack high-water to
confirm 16 KiB on every architecture (interrupt frames now land on thread stacks at
preemption points); revisit the stack size when FP/SIMD state or nested interrupts add frames.

Preemption follow-ups (D37): involuntary preemption on return to user mode (processes);
per-CPU tick state and run queues (SMP); a measured worst-case gap between
preemption points as a latency benchmark; a debug-build check that no lock is held at a
preemption point.

Sleep and wait-queue follow-ups (D39): waits with a timeout; a condition-variable wait for data
behind a `SpinLock` and a futex-style wait (M6); per-CPU idle threads and wait-queue locks
(SMP); waking threads from device interrupt handlers (M8); RISC-V Sstc, arming the timer
without an SBI call; per-thread CPU time from clock reads at each switch; a pairing heap or
timer wheel for the sleepers if a CPU ever has many.

SMP follow-ups (D40): per-CPU run queues, idle threads, tick state and wait-queue locks;
per-CPU interrupt controller and timer setup (GIC CPU interface and timer PPI, `sie.STIE`,
local APIC and LVT); TLB shootdown when live tables change; x2APIC for APIC ids of 255 and up;
an invariant-TSC check before threads move between CPUs; booting from a CPU whose affinity is
not 0 on aarch64, the PSCI conduit from the device tree and a full known `SCTLR_EL1` (M13); a
console lock for messages from any CPU; starting the CPUs in `boot`, not in a self-test.

Heap and buddy follow-ups (D30, D31): returning empty slabs to the frame allocator (a live-block
count per frame); per-CPU caches instead of one heap lock and a high watermark on the free stack
(M4); a physical address limit for 32-bit DMA (M8); one buddy bitmap per range; anti-fragmentation
grouping.

Setup follow-ups from the 2026-10-22 review (all done: exception vectors in D25, the benchmark harness
in D27):
- **Benchmarks: done** (D27). `cargo xtask bench` boots each architecture under QEMU `-icount`
  and fails on a regression against `benchmarks/baseline.txt`; CI runs it. Every new hot path
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
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (PSCI `CPU_ON`)) | QEMU `virt`, EL1, PSCI power-off (D9, D10) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary harts start (SBI HSM)) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off (D9) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (INIT-SIPI)) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off (D15) |
| loongarch64, ppc64, s390x, mips64, arm (32-bit), riscv32, sparc64 | planned | M9 |

### GPUs
| GPU | Status |
|-----|--------|
| virtio-gpu | planned (M11) |
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
| PL011 UART (Arm), transmit | **done** |
| ACPI tables: RSDP/RSDT/XSDT/FADT, `\_S5` from the DSDT (power-off), MADT local APICs (CPU discovery) | **done** (full AML interpreter: planned, M5) |
| Flattened device tree: memory nodes and reservations, `/cpus` | **done** (device discovery: planned, M5) |
| NS16550 UART (MMIO, any `reg-shift`, and x86 port I/O), transmit | **done** |
| Arm GICv2 (distributor + CPU interface) | **done** (GICv3: planned, M5) |
| Arm generic timer (EL1 virtual timer) | **done** |
| RISC-V SBI timer | **done** |
| x86 local APIC (xAPIC): its timer and INIT/start-up IPIs; 8259 PICs (remapped and masked); 8254 PIT (frequency measurement) | **done** (x2APIC, I/O APIC, HPET: planned) |
| virtio (console, block, net, rng, input, gpu) | planned (M8, M11) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~1%**. The kernel boots and prints on three architectures (x86_64, aarch64, riscv64); nothing a user can run yet.

| Area | Missing |
|------|---------|
| Kernel | device interrupt routing, threads on every CPU (per-CPU run queues), processes, syscalls (CPU exceptions are caught and reported; timer interrupts work; physical frames are allocated; the kernel runs on its own page tables in the top 2 GiB with W^X, a direct map and an empty low half; it has a heap and contiguous frame allocation; kernel threads switch on yield and, with time slices, at preemption points; they sleep and wait in wait queues, and an idle CPU takes no tick; every CPU starts and has its per-CPU block) |
| Hardware | the other architectures; every driver beyond the early UARTs |
| Compatibility | Linux ABI, Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

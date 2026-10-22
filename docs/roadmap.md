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
| M4 | Scheduler, processes and threads (SMP-ready) | **in progress** — kernel threads with a context switch on aarch64, riscv64, x86_64 and a cooperative round-robin scheduler on one CPU, 16 KiB stacks with an overflow canary (D35); voluntary preemption: a 10 ms timer tick ends time slices and threads switch at preemption points, the scheduler's lock handed over across the switch (D37); sleeping and wait queues: an idle thread, `sleep`, `WaitQueue`, a blocking `join`, threads linked through themselves, and one timer shared by the tick and the sleepers with no tick while one thread is runnable (D39); secondary CPUs: discovered from the device tree / MADT, started (PSCI, SBI HSM, INIT-SIPI with a real-mode trampoline) on the kernel's page tables, each with a per-CPU block behind its per-CPU register, reporting in and parking (D40); every CPU takes its own interrupts: per-CPU timer and interrupt controller setup, wake-up interrupts between CPUs (GICv2 SGI, SBI IPI, local APIC fixed IPI), per-CPU interrupt counts, and a one-load per-CPU read for hot paths (D41); every CPU runs threads: per-CPU run queues, sleepers and idle threads, new threads placed on idle CPUs first, threads woken from any CPU onto their own with the wake-up interrupt or a reschedule request, one scheduler lock for now (D43); load balancing: an idle CPU takes ready threads from busy ones, and a slice that ends with threads ready wakes an idle CPU (D44); user address spaces: a low half of their own with the kernel half shared, user mappings on aarch64, riscv64, x86_64, switching with the CPU's user translations dropped, and kernel access to user pages only in a window (D45); user mode: the kernel runs a program in EL0, U-mode or ring 3 until a system call (`svc`, `ecall`, `syscall`), an interrupt or an exception brings the CPU back, with the program's registers in a `UserContext` and Linux's system-call registers; a per-CPU GDT and TSS on x86_64; a system-call round trip in about 106 (aarch64), 148 (riscv64) and 107 (x86_64) instructions (D47); processes: threads that run their program in a loop and hand each exit to a handler, address spaces loaded lazily per CPU at the point of use and kept until another is needed, preemption on return to user mode, processes on every CPU (D48); next: the review of D47–D48, then the Linux system calls a static hello-world binary needs (M6); per-CPU and per-wait-queue locks wait for a workload that shows contention (D45) |
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

Paging follow-ups (D29, D33): a separate exception stack (x86 IST, AArch64/RISC-V stack switch) so a guard-page hit is reported; TLB maintenance for changing live tables (M4; user address spaces need none yet, D45); honour device-tree `no-map` reservations (M5); AArch64 data-cache maintenance for memory written with the MMU off, the boot tables included, and a boot direct map built from the device tree instead of QEMU `virt`'s fixed device/RAM split (real hardware, M13); secondary CPUs start on the kernel's tables, global kernel pages and PCID (M4); a device window behind a mapping call instead of the direct map's (M5); RAM above 4 GiB on x86_64; on x86_64, map ACPI tables on demand and reserved PVH ranges uncached instead of direct-mapping them cached (M5; early device pages are already left out, D38); keep the PVH start info and memory map in `physical_memory` even when a loader map has more than 62 disjoint entries; hardware probes in the boot self-tests that the low half, the image's direct-map alias and the stack guard page really fault (they check the tables in software today); 1 GiB pages on x86_64 (CPUID); Sv48/Sv57 chosen at boot on RISC-V when Sv39's 256 GiB is not enough; Svpbmt.

Address-space follow-ups (D45): (threads that carry an address space, switched only when it
differs: done, lazily at the point of use, D48; the first trip to user mode and back: done, D47);
ASIDs and PCID; per-page TLB maintenance and cross-CPU shootdown once mappings change
while active; PAN, SMAP and SMEP; user faults that end the process.

User-mode follow-ups (D47): (processes whose threads run their program in a loop over `run_user`,
preempted on return to user mode: done, D48); on x86_64, IST stacks for NMI, double fault, machine check and debug exceptions before any
untrusted program runs (`syscall` and `sysretq` leave a moment in ring 0 on the program's stack),
the paranoid GS check in those handlers, and reloading SS for AMD's `sysretq`; FP/SIMD state per
thread, saved lazily, once programs may use it (it traps today); TLS registers (`TPIDR_EL0`,
`FS` base) in the context, and `KERNEL_GS_BASE` per program; the Linux system calls themselves
(M6); a measured system-call cost outside QEMU `-icount` (M13). From PR #39's independent review:
a self-test that interrupts a program mid-pattern and checks every register (the `iretq` and
interrupt-exit paths); `SCTLR_EL1.UMA` and `E0E` written explicitly, not left to firmware (M13);
`IA32_CSTAR` at a safe stub before any 32-bit user segment exists; DPL-3 gates for `int3` and
`into` once user debuggers matter; UMIP where the CPU has it; `TSS.RSP0` cleared when `run_user`
returns.

Process follow-ups (D48): preemption on whenever user programs run (with it off, as for kernel
threads, a program that never traps keeps its CPU); waking CPUs that keep an ended process's
address space loaded while they stay busy with kernel threads, or idle after a look that raced with
another CPU switching away from it (an idle CPU lets it go otherwise); TLB shootdown, to every CPU that has the
address space loaded, once a shared process's mappings may change; pages owned by the address
space (`mmap`, M6); user faults that end the process with a signal (M6); a process's threads and
exit status (`exit_group`, `wait4`).

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

Sleep and wait-queue follow-ups (D39): waits with a timeout; a condition-variable wait for data
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
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (PSCI `CPU_ON`) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, processes) | QEMU `virt`, EL1, PSCI power-off (D9, D10) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary harts start (SBI HSM) and take their own timer and wake-up interrupts, threads on every hart, user address spaces, user mode with system calls, processes) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off (D9) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (INIT-SIPI) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, per-CPU GDT and TSS, processes) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off (D15) |
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
| Arm GICv2 (distributor + CPU interface, per-CPU setup, SGIs between CPUs) | **done** (GICv3: planned, M5) |
| Arm generic timer (EL1 virtual timer) | **done** |
| RISC-V SBI timer and IPI | **done** |
| x86 local APIC (xAPIC), on every CPU: its timer, INIT/start-up IPIs and fixed IPIs; 8259 PICs (remapped and masked); 8254 PIT (frequency measurement) | **done** (x2APIC, I/O APIC, HPET: planned) |
| virtio (console, block, net, rng, input, gpu) | planned (M8, M11) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~1%**. The kernel boots and prints on three architectures (x86_64, aarch64, riscv64); nothing a user can run yet.

| Area | Missing |
|------|---------|
| Kernel | device interrupt routing, per-CPU scheduler locks, the Linux syscalls (processes run their programs in user mode on every CPU, preempted on return to user mode, and make system calls; user address spaces exist; CPU exceptions are caught and reported; timer interrupts work; physical frames are allocated; the kernel runs on its own page tables in the top 2 GiB with W^X, a direct map and an empty low half; it has a heap and contiguous frame allocation; kernel threads switch on yield and, with time slices, at preemption points; they sleep and wait in wait queues, and an idle CPU takes no tick; every CPU starts, has its per-CPU block, takes its own timer and wake-up interrupts and runs threads, and idle CPUs take threads from busy ones) |
| Hardware | the other architectures; every driver beyond the early UARTs |
| Compatibility | Linux ABI, Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

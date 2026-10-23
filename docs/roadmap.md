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
| M4 | Scheduler, processes and threads (SMP-ready) | **in progress** — kernel threads with a context switch on aarch64, riscv64, x86_64 and a cooperative round-robin scheduler on one CPU, 16 KiB stacks with an overflow canary (D35); voluntary preemption: a 10 ms timer tick ends time slices and threads switch at preemption points, the scheduler's lock handed over across the switch (D37); sleeping and wait queues: an idle thread, `sleep`, `WaitQueue`, a blocking `join`, threads linked through themselves, and one timer shared by the tick and the sleepers with no tick while one thread is runnable (D39); secondary CPUs: discovered from the device tree / MADT, started (PSCI, SBI HSM, INIT-SIPI with a real-mode trampoline) on the kernel's page tables, each with a per-CPU block behind its per-CPU register, reporting in and parking (D40); every CPU takes its own interrupts: per-CPU timer and interrupt controller setup, wake-up interrupts between CPUs (GICv2 SGI, SBI IPI, local APIC fixed IPI), per-CPU interrupt counts, and a one-load per-CPU read for hot paths (D41); every CPU runs threads: per-CPU run queues, sleepers and idle threads, new threads placed on idle CPUs first, threads woken from any CPU onto their own with the wake-up interrupt or a reschedule request, one scheduler lock for now (D43); load balancing: an idle CPU takes ready threads from busy ones, and a slice that ends with threads ready wakes an idle CPU (D44); user address spaces: a low half of their own with the kernel half shared, user mappings on aarch64, riscv64, x86_64, switching with the CPU's user translations dropped, and kernel access to user pages only in a window (D45); user mode: the kernel runs a program in EL0, U-mode or ring 3 until a system call (`svc`, `ecall`, `syscall`), an interrupt or an exception brings the CPU back, with the program's registers in a `UserContext` and Linux's system-call registers; a per-CPU GDT and TSS on x86_64; a system-call round trip in about 106 (aarch64), 148 (riscv64) and 107 (x86_64) instructions (D47); processes: threads that run their program in a loop and hand each exit to a handler, address spaces loaded lazily per CPU at the point of use and kept until another is needed, preemption on return to user mode, processes on every CPU (D48); review of D47–D48: user programs get no FP/SIMD, vector unit or timer access whatever firmware left, `int3` after x86 returns, speculation mitigations per CPU with recorded triggers (D49); next: the Linux system calls (M6); per-CPU and per-wait-queue locks wait for a workload that shows contention (D45) |
| M5 | Common driver model (bus enumeration, device tree / ACPI discovery) | |
| M6 | Linux syscall ABI: what a static hello-world needs (`write`, `exit_group`, `brk`, `mmap`, ...) | **in progress** — the Linux system-call layer: dispatch by Linux number per architecture, `write` to the console, `exit` and `exit_group`, `ENOSYS` for the rest, exceptions end the process with Linux's signal, user memory copied after a table walk (D50); programs start from ELF images: a checked ELF parser, executables and static PIEs mapped page by page with W^X and a 0x10000 floor, Linux's initial stack with `argv`, `envp` and the auxiliary vector, Linux's `execve` errors (D51); demand paging: processes have regions (Linux's mappings), a page is mapped with a zeroed frame when the program first touches it, the stack is one 8 MiB region mapped as it grows, kernel copies map untouched pages, and live address spaces gain mappings (D52); the memory calls: `brk`, anonymous `mmap`, `munmap` and `mprotect` on the regions with Linux's checks and errors, top-down placement, `PROT_NONE` pages keeping their frame in an invalid entry, and other CPUs that keep a changed address space loaded marked stale so they switch again before using it (D53); the first startup calls: `writev`, `getpid`, `getppid`, `gettid`, `uname` (Linux 6.1.0-eggos), `set_tid_address` and `set_robust_list`, and process ids (D55); the kernel's random number generator (BLAKE2s input, ChaCha20 with fast key erasure and per-CPU keys, seeded from the device tree's `rng-seed` and `RDSEED`/`RDRAND`), `AT_RANDOM` from it and `getrandom`, with frames and page tables zeroed a line at a time (`linux_exec_load` −27%, −18%, −1%) (D56); the thread pointer in each thread's user context, loaded on every way into user mode (`TPIDR_EL0` saved and loaded, `tp`, the FS base written only when it changed), x86_64's `arch_prctl` and `sched_yield`, with no cost on the thread switch and the system-call round trips slightly faster (D57); review of D55–D57: empty ranges past user space `EFAULT` as on Linux, `writev`'s count an `unsigned int`, BLAKE2s wiped with volatile stores (D58); resource limits: `getrlimit`, `setrlimit` and `prlimit64` on per-process limits with a current distribution's defaults, copied on a process's first change, no hard limit raised until processes have credentials, not yet enforced (D59); next: signals (`rt_sigaction`, `rt_sigprocmask`, `sigaltstack`), `poll` and `ioctl`, the rest of what Rust's std and the C libraries call at startup; FP/SIMD state per thread (PR #47, D54, waiting for the owner's OK on its switch-path cost) (no compiled Rust or C program runs on AArch64 or x86_64 without it); a first program built from our own Rust source; the SWAPGS fence before programs can set GS or map low pages (D49) |
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
ASIDs and PCID; (per-page TLB maintenance on this CPU: done, D52, `drop_user_translations`;
mappings added while active: done, D52); (cross-CPU TLB maintenance for the memory calls: done, D53, CPUs that keep the address space
loaded are marked stale and switch again before using it); a shootdown (IPIs, waiting for each
CPU) once `clone` runs a process on several CPUs at once, or anything uses an address space on
two CPUs (`with_active`, `with_user_memory` from another CPU), D53; fault-around
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

Process follow-ups (D48): preemption on whenever user programs run (with it off, as for kernel
threads, a program that never traps keeps its CPU); waking CPUs that keep an ended process's
address space loaded while they stay busy with kernel threads, or idle after a look that raced with
another CPU switching away from it (an idle CPU lets it go otherwise); TLB shootdown, to every CPU that has the
address space loaded, once a shared process's mappings may change; (pages owned by the address
space: done, freed with it, D52, and by `munmap`, D53); (user faults that end the process with Linux's default signal, and the
exit status `exit_group` records: done, D50); signal handlers; a process's threads (`clone`)
and `wait4`; user-memory copies at user addresses with fault fixups with `clone` (D52 moved
D50's trigger there: the regions lock keeps pages from being unmapped during a copy, but a
plain copy racing another thread's writes to the same page is undefined in Rust: volatile or
per-architecture copy routines); (a cap per `write` call, Linux's `MAX_RW_COUNT`, and a preemption
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

Memory-call follow-ups (D53): a "shared" mark on `MAP_SHARED` anonymous regions before
`fork`, so it shares their pages instead of copying them; `madvise` (`MADV_DONTNEED` zeroes
private pages), `mremap`, `MAP_POPULATE` and `MAP_GROWSDOWN` when a program needs them;
`PROT_BTI` enforced (the `GP` bit) once BTI is reported in `AT_HWCAP2`; file mappings (M7);
`RLIMIT_DATA` and `RLIMIT_AS` for `brk` and `mmap`; randomized `mmap` base and break with the
random number generator; from PR #45's independent review: (`munmap` and a shrinking `brk`
free their frames after the translations are dropped, taking the frame allocator's lock once:
done, PR #46), and anonymous `MAP_SHARED_VALIDATE` and `MAP_SHARED | MAP_GROWSDOWN`
checked against Linux's observed behaviour (`EINVAL` there, possibly, where we accept or say
`EOPNOTSUPP`).

Startup-call follow-ups (D55): process ids reused after `wait4` reaps them (a process table);
`set_tid_address`'s word cleared and woken and the robust list released at thread exit, with
`clone` and futexes; parents for `getppid` with `fork`; `sethostname` and `setdomainname`; the
same release in `/proc/sys/kernel/osrelease` and `/proc/version` (procfs); a `writev`
benchmark once a sink faster than the console's UART exists (M7), and `getpid`'s round trip
as the cheapest real call; (`writev`'s two edge cases from PR #48's review checked against
Linux: done, D58); from PR #51's independent review: `writev` with a partly unreadable array
whose readable entries hold a negative length is `EINVAL` on Linux, `EFAULT` here; `init` as pid 1 (M10).

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
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (PSCI `CPU_ON`) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt`, EL1, PSCI power-off (D9, D10) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary harts start (SBI HSM) and take their own timer and wake-up interrupts, threads on every hart, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off (D9) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (INIT-SIPI) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, per-CPU GDT and TSS, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off (D15) |
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
| Flattened device tree: memory nodes and reservations, `/cpus`, `/chosen` seeds (taken and removed, D56) | **done** (device discovery: planned, M5) |
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
| Compatibility | Linux ABI (static ELF programs load and start with Linux's stack; `write`, `writev`, `exit`, `brk`, `mmap`, `munmap`, `mprotect`, `getpid`, `gettid`, `uname`, `set_tid_address`, `set_robust_list`, `getrandom`, `sched_yield` and `arch_prctl` work, with `AT_RANDOM` from the kernel's generator; pages are mapped on first touch and the stack grows on demand; each thread keeps its own thread pointer; signals, FP state per thread and files still missing), Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

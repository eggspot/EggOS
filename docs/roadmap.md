# EggOS roadmap

Status keys: **done**, **in progress**, **next**, planned (no marker).
Updated by every build-loop run; the order is refined as we learn (reasons in `docs/decisions.md`).

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo skeleton: workspace, pinned toolchain, HAL layout, CI, docs | **done** |
| M1 | Kernel boots in QEMU and prints, on the first architectures, with an automated QEMU test runner | **done** — aarch64, riscv64, x86_64 |
| M2 | Memory management: physical frames, paging, kernel heap | |
| M3 | Interrupts and timers | **in progress** — exception vectors (D25); monotonic clock and one-shot timer interrupts through GICv2, SBI and the local APIC on aarch64, riscv64, x86_64 (D26); next: device interrupt routing (GIC SPIs, PLIC/AIA, I/O APIC) |
| M4 | Scheduler, processes and threads (SMP-ready) | |
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
| M15 | Compatibility layers, native not emulated (D17), each an optional component exposed to AI through M14: Linux userland, then Windows, Android, macOS, iOS (order: D19) | |
| M16 | Editions as component presets (D18): personal, professional, enterprise, server | |

Timer follow-ups (D26): on x86_64, read the TSC/APIC frequencies from CPUID 0x15 and hypervisor
leaves instead of measuring, TSC-deadline mode, and the ACPI PM timer or HPET as fallback
reference clocks when there is no 8254; `stimecmp` directly when RISC-V Sstc is present; GICv3; device-tree timebase and GIC
addresses (M5).

Setup follow-ups from the 2026-10-22 review (exception vectors done in D25, the benchmark harness
in D27):
- **Benchmarks: done** (D27). `cargo xtask bench` boots each architecture under QEMU `-icount`
  and fails on a regression against `benchmarks/baseline.txt`; CI runs it. Every new hot path
  (frame allocator, page faults, scheduler, syscalls) adds its benchmark there. Cycle-accurate
  numbers on real hardware come with M13.
- **A BSS self-test that can fail:** QEMU hands over zeroed RAM, so fill `.bss` with a pattern
  before boot (QEMU's `loader` device) to prove the zeroing loop works.

UX track (runs alongside, from the start): `docs/ux/` — **in progress** (switcher-habit research).

## Support matrix

### CPU architectures (QEMU boot test in CI is the bar for "done")
| Architecture | Status | Notes |
|--------------|--------|-------|
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts) | QEMU `virt`, EL1, PSCI power-off (D9, D10) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off (D9) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off (D15) |
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
| ACPI tables: RSDP/RSDT/XSDT/FADT, `\_S5` from the DSDT (power-off only) | **done** (full AML interpreter: planned, M5) |
| NS16550 UART (MMIO, any `reg-shift`, and x86 port I/O), transmit | **done** |
| Arm GICv2 (distributor + CPU interface) | **done** (GICv3: planned, M5) |
| Arm generic timer (EL1 virtual timer) | **done** |
| RISC-V SBI timer | **done** |
| x86 local APIC (xAPIC) and its timer; 8259 PICs (remapped and masked); 8254 PIT (frequency measurement) | **done** (x2APIC, I/O APIC, HPET: planned) |
| virtio (console, block, net, rng, input, gpu) | planned (M8, M11) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~1%**. The kernel boots and prints on three architectures (x86_64, aarch64, riscv64); nothing a user can run yet.

| Area | Missing |
|------|---------|
| Kernel | memory management, device interrupt routing, scheduling, syscalls (CPU exceptions are caught and reported; timer interrupts work) |
| Hardware | the other architectures; every driver beyond the early UARTs |
| Compatibility | Linux ABI, Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

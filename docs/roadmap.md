# EggOS roadmap

Status keys: **done**, **in progress**, **next**, planned (no marker).
Updated by every build-loop run; the order is refined as we learn (reasons in `docs/decisions.md`).

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo skeleton: workspace, pinned toolchain, HAL layout, CI, docs | **done** |
| M1 | Kernel boots in QEMU and prints, on the first architectures, with an automated QEMU test runner | **done** — aarch64, riscv64, x86_64 |
| M2 | Memory management: physical frames, paging, kernel heap | |
| M3 | Interrupts and timers | |
| M4 | Scheduler, processes and threads (SMP-ready) | |
| M5 | Common driver model (bus enumeration, device tree / ACPI discovery) | |
| M6 | Linux syscall ABI: what a static hello-world needs (`write`, `exit_group`, `brk`, `mmap`, ...) | |
| M7 | VFS and a first file system | |
| M8 | virtio drivers (console, block, net, rng, input) | |
| M9 | More architectures through the HAL | |
| M10 | Userland: init, shell, core utilities | |
| M11 | Graphics stack (virtio-gpu first) and compositor | |
| M12 | Desktop shell, designed from `docs/ux/` | |
| M13 | Real hardware and vendor GPU drivers | |
| M14 | Compatibility layers, native not emulated (D17): Linux userland, then Windows, Android, macOS, iOS (order: D19) | |
| M15 | AI-native agent interface (D3) | |
| M16 | Component manager and editions as component presets (D18): personal, professional, enterprise, server | |

UX track (runs alongside, from the start): `docs/ux/` — **in progress** (switcher-habit research).

## Support matrix

### CPU architectures (QEMU boot test in CI is the bar for "done")
| Architecture | Status | Notes |
|--------------|--------|-------|
| aarch64 | **done** (boots, prints, self-tests) | QEMU `virt`, EL1, PSCI power-off (D9, D10) |
| riscv64 | **done** (boots, prints, self-tests) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off (D9) |
| x86_64 | **done** (boots, prints, self-tests) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off (D15) |
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
| virtio (console, block, net, rng, input, gpu) | planned (M8, M11) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~1%**. The kernel boots and prints on three architectures (x86_64, aarch64, riscv64); nothing a user can run yet.

| Area | Missing |
|------|---------|
| Kernel | memory management, interrupts, timers, scheduling, syscalls |
| Hardware | the other architectures; every driver beyond the early UARTs |
| Compatibility | Linux ABI, Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | agent interface design and implementation |
| Editions | component manager; packaging and policy per edition |

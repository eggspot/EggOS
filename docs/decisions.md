# Architecture decisions

Numbered, append-only. To change a decision, add a new entry that supersedes the old one and say so in
both.

## D1. Our own kernel, in Rust, Linux-compatible
EggOS runs on its own kernel, written in Rust. We do not use the Linux kernel's C code. "Linux-compatible"
means the kernel implements the Linux interfaces (system calls, ABI) so Linux software runs on it.

## D2. Foreign apps through our own compatibility layers
Windows, macOS and Android apps run on EggOS through compatibility layers we write ourselves, with the
same features on each. Which layer comes first is not decided yet.

## D3. AI-native
The OS exposes its capabilities to AI agents as a first-class interface.

## D4. Every architecture, GPU, device type and driver, behind one HAL and one driver model
- The kernel core is architecture-independent; architecture code lives only behind a hardware abstraction
  layer, one module per architecture, never mixed into the core.
- Drivers sit behind one common driver model, so adding a device never touches the core.
- Each supported architecture boots in QEMU in automated tests.
- Bring-up order is an engineering choice, not a scope limit: start where QEMU emulation is good and most
  users are (x86_64, aarch64, riscv64; virtio devices and virtio-gpu first), then real hardware and vendor
  GPUs behind the same interfaces.

## D5. MIT license, clean-room
All EggOS code is MIT and everything in the repository stays MIT-compatible. Linux compatibility is
implemented from public specifications, man pages and observed behaviour, never by copying or translating
code from GPL/LGPL/copyleft projects, including the Linux kernel.

## D6. Workspace layout
One Cargo workspace:

| Crate | Directory | Role |
| --- | --- | --- |
| `eggos-kernel-core` | `kernel-core/` | The architecture-independent kernel core. Builds and is unit tested on development hosts. |
| `eggos-hal` | `hal/` | The `Platform` trait the core uses, and `arch/<architecture>/`: boot code, linker script and machine bring-up. The only crate with architecture code. |
| `eggos-drivers` | `drivers/` | The driver model (`Driver`, one trait per device class such as `CharDevice`) and the drivers. Drivers reach registers through `io` traits, so one driver serves every architecture and bus that has its device. |
| `eggos-kernel` | `kernel/` | The bootable kernel image: joins the HAL to the core. Bare-metal targets only. |
| `xtask` | `xtask/` | Build, QEMU run and boot-test automation (`cargo xtask`). |

Dependencies point one way: `eggos-kernel` -> `eggos-kernel-core` -> `eggos-hal` -> `eggos-drivers`.

## D7. Build automation in Rust (`cargo xtask`), not shell scripts
Development hosts are Windows and macOS, so build, run and test automation is a Rust program in the
workspace (the `xtask` convention) rather than shell, make or PowerShell scripts. CI runs the same
`cargo xtask ci` a developer runs locally.

## D8. First bring-up targets and their boot protocols
| Architecture | Rust target | QEMU machine | Boot protocol |
| --- | --- | --- | --- |
| x86_64 | `x86_64-unknown-none` | `q35` | PVH (ELF note `XEN_ELFNOTE_PHYS32_ENTRY`); the stub switches from 32-bit protected mode to long mode. |
| aarch64 | `aarch64-unknown-none-softfloat` | `virt`, `cortex-a72` | ELF loaded by QEMU `-kernel`, entered at EL1 with the MMU off. |
| riscv64 | `riscv64gc-unknown-none-elf` | `virt` | OpenSBI (`-bios default`) enters the ELF image in S-mode. |

Machine addresses (UART bases) are fixed per QEMU machine until the kernel reads the boot information
(PVH start info, device tree). AArch64 uses the soft-float target until the kernel saves FP/SIMD state on
context switch. All three are interim choices for bring-up; UEFI boot for real hardware is a later
decision.

## D9. Boot tests watch the serial console
A boot test passes when the kernel logs `EggOS: boot OK` on its serial console, and fails on
`EggOS: KERNEL PANIC`, on QEMU exiting, or on a timeout. The kernel halts rather than powering off, so
there is no test-only shutdown path in the kernel; `xtask` stops QEMU.

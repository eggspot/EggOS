# EggOS — Claude Code instructions

AI-native operating system, written entirely by us in Rust, kernel included.
Editions: personal, professional, enterprise and server, from one codebase.

## Architecture decisions so far
- **Kernel: our own, in Rust, Linux-compatible.** We do not use the Linux kernel's C code. "Linux-compatible"
  means our kernel implements the Linux interfaces (system calls, ABI) so Linux software runs on it.
- **Foreign apps:** Windows, macOS and Android apps run on EggOS through compatibility layers we write
  ourselves, with the same features on each.
- **AI-native:** the OS exposes its capabilities to AI agents as a first-class interface.
- **Hardware: we support everything.** Every CPU architecture, every GPU, every device type (desktop,
  laptop, phone, tablet, server, embedded) and every driver, all written by us. So from day one:
  - the kernel core is architecture-independent; architecture code lives only behind a hardware
    abstraction layer (one module per architecture), never mixed into the core;
  - drivers sit behind one common driver model, so adding a device never touches the core;
  - each supported architecture boots in QEMU in automated tests (this Mac's QEMU emulates x86_64,
    aarch64, riscv64, loongarch64, ppc64, s390x, mips64 and more).
  Bring-up order is an engineering choice, not a scope limit: start where QEMU emulation is good and most
  users are (e.g. x86_64, aarch64, riscv64; virtio devices and virtio-gpu first), then real hardware and
  vendor GPUs (Intel, AMD, NVIDIA, Apple, Arm Mali, Qualcomm Adreno, ...) behind the same interfaces.
- Record every further architecture decision as a numbered entry in docs/decisions.md (start it with the
  decisions above and the MIT license below as D1-D5).

## Priorities
1. Performance first: benchmark every hot path; never merge a performance regression.
2. Correctness and security: a kernel bug is a system-wide bug.
3. Feature parity across the compatibility layers.
4. UI/UX that feels familiar to anyone switching from macOS, Windows or Linux desktops: their habits
   (shortcuts, window controls, file locations, settings) should just work, with polish good enough that
   switching feels like an upgrade. Treat UX regressions like performance regressions.

The long-term goal is to be better than macOS, Windows and every Linux distribution for the people who use
them.

## Rules
- Rust only. Write it ourselves; do not add third-party crates without the owner's OK, and list each
  approved one with its reason in docs/dependencies.md.
- `unsafe` only where unavoidable, each block with a `// SAFETY:` comment explaining why it is sound.
- Development hosts are Windows and macOS; changes must build and be testable on both.
- Build/test: `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
- CI follows the global billing rules: Linux runners by default; macOS (10x cost) and Windows (2x) jobs
  only on PRs into main.

## License: MIT, 100% open source
- All EggOS code is MIT (see LICENSE). Keep everything in the repo MIT-compatible.
- Clean-room only: never copy or translate code from GPL/LGPL/copyleft projects, including the Linux kernel
  (GPL-2.0). Implement Linux compatibility from public specifications, man pages and observed behaviour,
  not from Linux source.

## Not decided yet (ask the owner, don't assume)
- Which compatibility layer comes first (Linux, Android, Windows or macOS apps).

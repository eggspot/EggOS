# EggOS — Claude Code instructions

AI-native operating system, written entirely by us in Rust, kernel included.
Editions: personal, professional, enterprise and server, from one codebase.

## Architecture decisions so far
- **Kernel: our own, in Rust, Linux-compatible.** We do not use the Linux kernel's C code. "Linux-compatible"
  means our kernel implements the Linux interfaces (system calls, ABI) so Linux software runs on it.
- **Foreign apps:** Windows, macOS and Android apps run on EggOS through compatibility layers we write
  ourselves, with the same features on each.
- **AI-native:** the OS exposes its capabilities to AI agents as a first-class interface.
- Record every further architecture decision as a numbered entry in docs/decisions.md (start it with the
  three above as D1-D3).

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

## Not decided yet (ask the owner, don't assume)
- License (the repo is public but has no license yet).
- Target CPU architectures (e.g. x86_64, aarch64) and how the kernel is run in development (e.g. QEMU).
- Which compatibility layer comes first (Linux, Android, Windows or macOS apps).

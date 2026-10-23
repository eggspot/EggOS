# Contributing to EggOS

Thanks for helping build EggOS. A few rules keep the project fast, clean and free forever under MIT OR Apache-2.0.

## The rules
- **Everything is our own Rust.** No third-party crates, ever, and no C, C++ or shell build scripts.
  If you need something a crate would provide, write it. Assembly only where the CPU requires it,
  inside `hal/src/arch/<arch>/`.
- **Clean-room only.** Never copy, translate or closely paraphrase code from other projects, especially
  GPL/LGPL/copyleft ones such as the Linux kernel, Wine or Darling. Work from public specifications,
  documentation, man pages and observed behaviour. If you have studied a copyleft implementation of the
  thing you are writing, say so in the PR.
- **Performance first.** A PR that touches a hot path includes before/after numbers from the same
  machine, and a new hot path adds its benchmark to `kernel/src/bench.rs` (`cargo xtask bench`,
  D27). Performance, memory, install-size and boot-time regressions are not merged.
- **Every feature has a GUI, a CLI command and an MCP tool** (see `docs/decisions.md`, D22).
- **`unsafe` only where unavoidable**, each block with a `// SAFETY:` comment explaining why it is sound.

## Before you open a PR
```
cargo xtask ci
```
This runs everything CI runs, cheapest first: formatting, clippy (host and every kernel target), host
tests, QEMU boot tests on every architecture (a self-test boot and a fatal-exception boot each), and
the benchmarks against `benchmarks/baseline.txt`. It needs QEMU installed (`brew install qemu` on
macOS; QEMU's Windows installer or MSYS2 on Windows; your distribution's `qemu-system-*` packages on
Linux).

Architecture decisions go in `docs/decisions.md` as a new numbered entry.

## License
EggOS is licensed under either the MIT License or the Apache License 2.0, at your option (see
`LICENSE-MIT` and `LICENSE-APACHE`). By contributing you agree that your contribution is licensed under
the same terms. There is no CLA.

# EggOS
AI-native operating system written from scratch in Rust, with its own kernel (no Linux code) that can also run Linux programs unchanged.

- **Fast and light:** built to beat macOS, Windows and Linux on speed, memory and install size.
- **AI-native:** a built-in MCP server lets AI agents use the whole OS, and every app you install, with your permission.
- **CLI, GUI and AI at 100% parity:** anything you can do in one, you can do in the others.
- **Runs Windows, macOS, Android and iOS apps natively,** without emulation.
- **One OS for every device:** desktop, laptop, phone, tablet, TV, wearable, car, server, embedded.
- **Modular:** beyond a small base, every feature is optional, so you install only what you choose.
- **Editions:** personal, professional, enterprise and server.

100% open source under either the [MIT License](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at your option.

EggOS is open source and open for funding. If you want to help build it, [sponsor EggOS on GitHub Sponsors](https://github.com/sponsors/eggspot).

## Layout
| Path | What |
|------|------|
| `kernel/` | architecture-independent kernel core and the kernel binary |
| `hal/` | hardware abstraction layer: the `Platform` trait and one module per architecture |
| `drivers/` | device drivers behind one driver model |
| `userland/` | our own Linux programs: init, the [shell](docs/shell.md), the [core utilities](docs/coreutils.md) and the runtime they share |
| `xtask/` | developer commands (`cargo xtask ...`) |
| `docs/` | [roadmap](docs/roadmap.md), [decisions](docs/decisions.md), [dependencies](docs/dependencies.md), [UX research](docs/ux/README.md) |

## Build and test
Requires [rustup](https://rustup.rs); the pinned toolchain in `rust-toolchain.toml` installs itself.

```
cargo xtask ci                   # everything CI runs, cheapest first
cargo xtask qemu --arch all      # boot every architecture in QEMU headless, check the verdict
cargo xtask bench --arch all     # benchmarks (instruction counts), checked against benchmarks/baseline.txt
cargo xtask run --arch aarch64   # boot interactively: display in a window, console here (quit: Ctrl-A then X)
```
QEMU (`qemu-system-aarch64`, `qemu-system-riscv64`, `qemu-system-x86_64`) must be on `PATH` for the
boot commands.
Kernel architectures so far: aarch64, riscv64, x86_64.

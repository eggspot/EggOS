# EggOS
AI-native operating system written from scratch in Rust, including its own Linux-compatible kernel; runs Windows, macOS, Android and iOS apps natively, without emulation; beyond a small base every feature is optional, so you install only what you choose; personal, professional, enterprise and server editions.

100% open source under the [MIT License](LICENSE).

EggOS is built in the open and funded by the people who use it. If you want an OS that is fast, private and yours to shape, [sponsor EggOS on GitHub Sponsors](https://github.com/sponsors/eggspot).

## Layout
| Path | What |
|------|------|
| `kernel/` | architecture-independent kernel core and the kernel binary |
| `hal/` | hardware abstraction layer: the `Platform` trait and one module per architecture |
| `drivers/` | device drivers behind one driver model |
| `xtask/` | developer commands (`cargo xtask ...`) |
| `docs/` | [roadmap](docs/roadmap.md), [decisions](docs/decisions.md), [dependencies](docs/dependencies.md), [UX research](docs/ux/README.md) |

## Build and test
Requires [rustup](https://rustup.rs); the pinned toolchain in `rust-toolchain.toml` installs itself.

```
cargo xtask ci                   # everything CI runs, cheapest first
cargo xtask qemu --arch all      # boot every architecture in QEMU headless, check the verdict
cargo xtask run --arch aarch64   # boot interactively (quit: Ctrl-A then X)
```
QEMU (`qemu-system-aarch64`, `qemu-system-riscv64`, `qemu-system-x86_64`) must be on `PATH` for the
boot commands.
Kernel architectures so far: aarch64, riscv64, x86_64.

# EggOS
AI-native operating system written from scratch in Rust, including its own Linux-compatible kernel; runs Windows, macOS and Android apps; personal, professional, enterprise and server editions.

100% open source under the [MIT License](LICENSE).

## Building
You need [rustup](https://rustup.rs) (the toolchain, targets and components install themselves
from `rust-toolchain.toml`) and [QEMU](https://www.qemu.org) to boot the kernel. Windows and macOS hosts
are supported.

```
cargo build                           # host crates
cargo test                            # unit tests of the kernel core, HAL and drivers
cargo xtask build                     # kernel images for x86_64, aarch64 and riscv64
cargo xtask run --arch x86_64         # boot one in QEMU; quit with Ctrl-A X
cargo xtask boot-test                 # boot each one headless and check it comes up
cargo xtask ci                        # everything CI runs
```

If QEMU is not on `PATH` (common on Windows), set `EGGOS_QEMU_DIR` to its install directory.

See [docs/decisions.md](docs/decisions.md) for the architecture and the workspace layout.

# The image CI's Linux job runs in (D36): Ubuntu 24.04 with QEMU, OpenSBI and the pinned Rust
# toolchain already installed, so a run neither downloads QEMU from the Ubuntu mirror (normally
# 15 s, sometimes over 20 minutes) nor installs the toolchain (about 12 s).
# Built and pushed by .github/workflows/ci-image.yml; the build context is the repository root.
# The QEMU packages are Ubuntu 24.04's, as benchmarks/baseline.txt requires.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# git: actions/checkout; zstd: actions/cache's compression (as on GitHub's runners); gcc and
# libc6-dev: the linker for host builds; gh: the docs-only check.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git zstd gcc libc6-dev gh \
      qemu-system-arm qemu-system-misc qemu-system-x86 qemu-system-data opensbi \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# The toolchain, targets and components rust-toolchain.toml pins.
COPY rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain none \
 && cd /tmp/toolchain \
 && rustup toolchain install \
 && rm -rf /tmp/toolchain

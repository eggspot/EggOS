FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git zstd gcc libc6-dev gh \
      qemu-system-arm qemu-system-misc qemu-system-x86 qemu-system-data opensbi \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

COPY rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain none \
 && cd /tmp/toolchain \
 && rustup toolchain install \
 && rm -rf /tmp/toolchain

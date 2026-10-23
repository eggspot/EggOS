# Build, test and CI configuration

What the configuration files do and why. They carry no comments themselves (D66); this page
holds what used to be written in them. (The CI image's `Dockerfile` and `ci-image.yml` lose
theirs with the next change to the image, which rebuilds it anyway.)

## Cargo workspace (`Cargo.toml`)
- **Lints.** `clippy::undocumented_unsafe_blocks` is denied, so every `unsafe` block carries a
  `// SAFETY:` comment (repo rule). `clippy::multiple_unsafe_ops_per_block` is denied, so each
  `// SAFETY:` comment justifies exactly one operation.
- **Release profile** (`lto = true`, `codegen-units = 1`): what we measure and ship
  (`CLAUDE.md`, "The target: the fastest OS"). Both profiles use `panic = "abort"`.

## Cargo configuration (`.cargo/config.toml`)
- **`cargo xtask`** is an alias that runs the `xtask` package: host-side developer commands
  written in Rust, so they work the same on macOS, Windows and Linux.
- **x86_64 kernel: `relocation-model=static`.** The x86_64 entry code runs 32-bit code that uses
  absolute addresses, so the kernel is linked as a static, non-PIE executable at its load
  address (the target defaults to static PIE). A `RUSTFLAGS` environment variable replaces
  these flags, so `kernel/build.rs` also passes `--no-pie` to the linker: the kernel still links,
  as non-PIE, when `RUSTFLAGS` is set.

## Kernel features (`kernel/Cargo.toml`)
- **`qemu-test`**: power the machine off after boot, so the QEMU test runner sees the verdict.
- **`qemu-test-fatal-exception`**: after the self-tests pass, write to the kernel's read-only
  data, to test that it faults and that the fatal exception is reported with the fail marker
  (the QEMU runner then expects that failure, at that address).
- **`qemu-bench`**: after the self-tests pass, run the boot-time benchmarks and print their
  results, which `cargo xtask bench` reads (it boots with `-icount shift=0`).

## CI (`.github/workflows/ci.yml`, D8, D36)
- **One cheap Linux job (`check`)**, its steps ordered cheapest first so a typo fails in
  seconds: format, clippy (host, then the kernel targets), docs, host tests, QEMU boot tests and
  benchmarks on every architecture.
- **`check` is a required status check on `main`**, so the workflow always runs: a workflow
  skipped by a `paths` filter never reports, and would leave docs PRs pending for ever. Its first
  step detects a docs-only change (Markdown, `docs/`, `LICENSE-MIT`, `LICENSE-APACHE`, `.github/FUNDING.yml`) and
  skips every later step, which still reports success in a few seconds. The job reads the PR's
  changed files, hence `pull-requests: read`; it pulls the CI image, hence `packages: read`.
- **It runs in the CI image** (below), with QEMU, OpenSBI and the pinned toolchain
  preinstalled. CI always runs in `main`'s image, so a change that needs something new in the
  image lands in its own PR first. The toolchain step installs the pinned toolchain only when a
  change moved the pin and the image has not been rebuilt yet.
- **Cache.** Every run restores the newest `target/` cache; only pushes to `main` save one, so
  PR commits do not each upload their own. There are no dependencies, so only `target/` is worth
  caching.
- **Hosts.** Development hosts are macOS and Windows, so on PRs into `main`, after `check`
  passed and only when code changed, a small job on each builds, lints and tests the workspace
  and builds the kernel for every architecture.
- **Billing**: the global rules in `~/.claude/CLAUDE.md` (CI section) and D8: concurrency
  cancels superseded PR runs only, every job has a timeout, checkouts are shallow.

## CI image (`.github/ci-image/Dockerfile`, `.github/workflows/ci-image.yml`, D36)
- **What it is.** Ubuntu 24.04 with QEMU, OpenSBI and the pinned Rust toolchain installed, so a
  run neither downloads QEMU from the Ubuntu mirror (normally 15 s, sometimes over 20 minutes)
  nor installs the toolchain (about 12 s). The QEMU packages are Ubuntu 24.04's, as
  `benchmarks/baseline.txt` requires. Besides QEMU it holds `git` (for `actions/checkout`),
  `zstd` (`actions/cache`'s compression, as on GitHub's runners), `gcc` and `libc6-dev` (the
  linker for host builds) and `gh` (the docs-only check). The toolchain, targets and components
  are the ones `rust-toolchain.toml` pins.
- **How it is built.** `ci-image.yml` builds it, with the repository root as the build context,
  and pushes it to `ghcr.io/eggspot/eggos-ci:main`, where `ci.yml` pulls it. It runs only when the
  image's inputs change (or by hand), so it bills a few minutes per toolchain or package change,
  not per PR. On a PR, or a manual run on another branch, it builds and checks the image; on
  `main` it also pushes. Only `main`'s image is ever pushed.
- **Layers** are reused from the current image (`--cache-from`), so a toolchain bump does not
  reinstall QEMU. Without registry access (a PR) the cache import logs a 403 "failed to
  configure registry cache importer" error, which is expected, and the image builds from
  scratch.
- **Timeout 30 minutes**: a rebuild installs QEMU from the Ubuntu mirror, which has stalled for
  over 20 minutes.
- **The check step** mounts the checkout into the image, so the checkout keeps no credentials.
  It runs in the checkout so that rustup picks the toolchain `rust-toolchain.toml` pins, as CI
  does, and fails if that toolchain is not the image's own (rustup must not install it there).
- **Concurrency**: a superseded PR build is cancelled; a push to the registry is never cut off
  halfway.

## Other repository files
- **`.github/dependabot.yml`** keeps the GitHub Actions used by CI current; EggOS has no
  third-party crates to update.
- **`.github/FUNDING.yml`** shows the "Sponsor" button on the repository page
  (<https://github.com/sponsors/eggspot>). Other platforms (Open Collective, Liberapay, a custom
  URL) are added there once their accounts exist; each takes the account name, not a URL.

# Build, test and CI configuration

What the configuration files do and why. They carry no comments themselves (D66); this page
holds what used to be written in them.

## Toolchain (`rust-toolchain.toml`)
- **Pinned** so every host (macOS, Windows, CI) builds the kernel with the same compiler and
  lints.
- **Kernel targets**: `aarch64-unknown-none-softfloat`, `riscv64imac-unknown-none-elf` and
  `x86_64-unknown-none`, none of which lets the compiler use FP/SIMD registers in the kernel
  (D54).
- **Test-program targets** (D84): `aarch64-unknown-linux-musl`, `riscv64gc-unknown-linux-musl`
  and `x86_64-unknown-linux-musl` build Linux programs from our own Rust source, with Rust's std,
  for the QEMU tests: static executables linked by `rust-lld` with the targets' own
  self-contained C library and start files (`-C linker=rust-lld -C link-self-contained=yes -C
  target-feature=+crt-static`), so they link the same way on macOS, Windows and Linux with no C
  toolchain. They are test fixtures, never part of EggOS.

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
- **Linux test-program targets: `rust-lld`, `link-self-contained=no`, `relocation-model=pie`**
  (D85), for programs on our own runtime, which links no C library or start files. `cargo
  xtask` passes each test program's flags in `CARGO_ENCODED_RUSTFLAGS` (D86, D88), which
  `RUSTFLAGS` cannot override, so their flags do not depend on these `rustflags` lines or on
  the environment (the `linker` lines still apply).

## Kernel features (`kernel/Cargo.toml`)
- **`qemu-test`**: power the machine off after boot, so the QEMU test runner sees the verdict.
- **`qemu-test-fatal-exception`**: after the self-tests pass, write to the kernel's read-only
  data, to test that it faults and that the fatal exception is reported with the fail marker
  (the QEMU runner then expects that failure, at that address).
- **`qemu-bench`**: after the self-tests pass, run the boot-time benchmarks and print their
  results, which `cargo xtask bench` reads (it boots with `-icount shift=0`).

## Developer commands (`xtask/`)
- **`cargo xtask ci`** runs CI's checks in CI's order, cheapest first. **`clippy-targets`**
  lints the user crates as compiled for each architecture's Linux target and the kernel crates
  for each bare-metal target (where the architecture modules a host build cannot see are
  compiled), without features and with each test feature, since they change code paths; the
  user programs the test kernels embed are built first.
- **Our Linux programs' flags.** A program on our own runtime (init, the shell, the core
  utilities, the runtime's self-test program) links no C library or start files, as a static
  PIE (D85); the ones the release initramfs holds also drop debug information and symbols. A
  program on Rust's standard library (the std test program) links the toolchain's own musl and
  start files statically, as a static PIE where the target has one, without debug information
  or symbols (D84, D86). The kernel build finds each test program through an environment
  variable naming its path.
- **`cargo xtask qemu`** boots, for each architecture: the self-test kernel, which must pass
  every self-test with the std program reading the console as the runner types (D100); the
  fatal-exception kernel, which must report a write to read-only data after its self-tests
  (D29), both with the `.bss` probe poisoned so the `.bss` self-test can fail; the release
  kernel with a test init whose login program, the std program, ends and init powers off, then
  resets the machine, which ends QEMU under `-no-reboot` (D106); init starting the shell, which
  the runner drives on the serial console (D107); and last the release kernel itself, booted to
  the shell's prompt, where the runner runs the core utilities, reports the time from QEMU's
  start to the first prompt, and powers off with `poweroff` (D108). A boot that has not powered
  off within 30 seconds (90 for the benchmarks) has hung.
- **What the runner matches.** The lines it looks for are the ones the kernel and the test
  programs write, and must be changed with them: the self-tests' `EGGOS-SELFTEST: PASS` and
  `FAIL` markers and `EGGOS-CONSOLE: input ready` (`eggos_kernel::selftest`,
  `eggos_kernel::console`), the user-mode lines `EGGOS-USER: ...` (`eggos_hal::user`,
  `eggos_kernel::selftest`, `userland/selftest`, `userland/std-selftest`), the std program's
  `EGGOS-CONSOLE: type ...` requests and what is typed for each (`console::request` in
  `userland/std-selftest`), `kernel exception: ` and the write-protect probe's line
  (`eggos_kernel::exception`, `eggos_kernel::selftest`), the boot banner `EggOS <version>
  (<arch>)`, `boot: input keyboard pointer` and the input self-test's `EGGOS-INPUT: send
  pointer and key events` (`eggos_kernel::boot`, `eggos_kernel::devices::selftest`, D113), and
  init's, the shell's and the utilities' lines in the D106 to D108 sessions.
  What it types answers a request each time the request appears as a whole line: after
  `EGGOS-CONSOLE: input ready`, `eggos_kernel::selftest::CONSOLE_INPUT_SCRIPT`, which a
  self-test checks arrives through the receive interrupt; then the std program's requests, among
  them lines edited with backspace, delete, `^W`, `^U` and `^D`, whose echo (each erased
  character rubbed out with backspace, space, backspace) the runner checks. The shell sessions
  type a line at each prompt instead, as a person would, so the echo never mixes with what a
  command writes.
- **QEMU's arguments.** Every boot has 128 MiB and 4 CPUs, so the secondary CPUs start (D40;
  `eggos_kernel::selftest::QEMU_CPUS` must match), and never `-no-reboot` unless a test wants a
  reset to end QEMU: with it an x86 triple fault (in the power-off path, say) ends QEMU with
  status 0, which looks like a clean power-off; without it the machine resets and prints the
  banner again, which fails the test. The `.bss` probe (`eggos_selftest_bss_probe`) is filled
  with `0xa5` bytes by one QEMU `loader` device per 8-byte word, which QEMU applies at reset
  after placing the image and before any firmware runs (observed on QEMU 10 on all three
  architectures, not a documented guarantee: a QEMU upgrade that changes it shows up as `.bss`
  failing). QEMU hands over zeroed memory, so without this the `.bss` self-test would pass even
  if the entry point never cleared `.bss`.
- **Devices** (D110, D111). Every run (tests, benchmarks, `cargo xtask run`) gets a virtio
  entropy device, `-device virtio-rng-device` on the Arm and RISC-V `virt` machines and
  `-device virtio-rng-pci,disable-legacy=on` (modern only) on q35, which the self-tests read.
  The `virt` machines also get `-global virtio-mmio.force-legacy=false`: QEMU (10.2 included)
  still makes virtio-mmio devices legacy (version 1) by default, which EggOS refuses. A release
  kernel has no entropy driver yet, so that device only shows in its device count.
- **Display** (D112). Every run but the two init boots gets a virtio GPU asking for 1280x800:
  `-device virtio-gpu-device,xres=1280,yres=800` on the `virt` machines, and `-vga none
  -device virtio-gpu-pci,disable-legacy=on,xres=1280,yres=800` on q35 (without `-vga none`,
  QEMU's window would show q35's standard VGA first). The tests stay headless with `-display
  none`, which hides the window but keeps the device. The two init boots run without it, to
  check that a kernel without a display behaves as before (`boot: no display`). The
  boot-to-prompt test also opens QEMU's machine protocol (QMP) on a free local TCP port and, at
  the first prompt, saves the display with `screendump` (`target/screendump-<arch>.ppm`) and
  checks it is the release kernel's first frame: 1280x800, pixel hash
  `eggos_kernel::display::FIRST_FRAME_HASH_1280X800`. `cargo xtask run` shows the display in a
  window and the serial console on the terminal (`-serial mon:stdio`, so Ctrl-A then X still
  quits); `--nographic`, anywhere among the options, keeps both on the terminal for a machine
  without a screen.
- **Input** (D113). Every run with the display also gets a virtio keyboard, tablet and mouse:
  `-device virtio-keyboard-device -device virtio-tablet-device -device virtio-mouse-device` on
  the `virt` machines, the `-pci` devices with `disable-legacy=on` on q35; the kernel prints
  `boot: input keyboard pointer`, which the boot-to-prompt test requires. Every boot with them
  but the benchmarks opens QMP: the self-test kernels write `EGGOS-INPUT: send pointer and key
  events` (`eggos_kernel::devices::selftest::INPUT_EVENTS_LINE`) and the runner answers with
  `input-send-event` reports 100 ms apart (the tablet at `TABLET_POSITION`, a left click, the
  mouse moved by `MOUSE_MOTION`, a wheel notch up, Shift pressed and released; the constants
  must match the self-test's), and the boot-to-prompt test types `uname -a` and `echo
  EGGOS-KEYBOARD Typed_On+Virtio:OK` on the virtio keyboard instead of the serial console, a
  report per key 10 ms apart and at most 8 keys ahead of their echo, so the device's 64-event
  queue never fills between the kernel's polls, however slow the machine. In `cargo xtask run`, QEMU's window types on the virtio keyboard and points with the
  tablet.
- **JSON.** xtask reads the path of each executable cargo built from cargo's JSON messages with
  just enough JSON for them (string escapes such as Windows paths' `\\`).

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
- **Layers** are reused from the current image (`--cache-from`, which fetches only the layers
  it reuses), so a toolchain bump does not reinstall QEMU. Without registry access (a PR) the cache import logs a 403 "failed to
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

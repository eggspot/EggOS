# EggOS — Claude Code instructions

AI-native operating system, written entirely by us in Rust, kernel included.
Editions: personal, professional, enterprise and server, from one codebase.

## IMPORTANT: notes for every task
Check each task against these before starting and again before opening the PR.
1. **GUI design is done by Claude Fable.** Any task that designs something a user sees (desktop
   shell, window chrome, system apps, settings, installer, icons, the design system, mockups,
   layouts, motion) gets its design from Claude Fable: delegate the design work to an agent running
   on the `fable` model (Agent tool, `model: "fable"`; currently `claude-fable-5-1`), record the
   design in `docs/ux/`, then implement it in Rust. Non-visual work (kernel, drivers, layers) does
   not need Fable. (D16)
2. **Foreign apps run natively, never emulated.** Windows, macOS, Android and iOS apps run from
   their own unmodified files (`.exe`/`.dll`, Mach-O `.app`, `.apk`, `.ipa`) with no virtual machine,
   emulator, simulator or interpreter. Their machine code runs directly on the CPU; their system
   calls and system libraries (NT/Win32, Darwin/Cocoa/UIKit, Android framework) are implemented
   natively by us on our kernel; their graphics APIs (DirectX, Metal, OpenGL ES, Vulkan) go straight
   to our GPU drivers. The target is **100% of native speed or better**: a macOS app must run faster
   on EggOS than on macOS, a Windows app faster than on Windows, on the same hardware. Every
   compatibility-layer hot path is benchmarked against the app on its own OS. When an app was
   built for a different CPU architecture than the machine (an x86_64 Windows app on an Arm laptop),
   its code is translated once, ahead of time, and cached, never interpreted; report those numbers
   separately. No App Store access: macOS apps come from the vendor's own download (`.dmg`,
   `.pkg`, `.zip`), which EggOS opens and installs itself; iOS apps only when they are distributed
   outside the App Store. (D17)
3. **Nothing is installed by default beyond the base** (kernel, drivers for the hardware present,
   base userland, the agent interface and the component manager). Every feature and service (each
   compatibility layer, the desktop, system apps, AI services, server roles, ...) is an optional
   component the user chooses to install, at setup or later, and can remove cleanly. Code must
   never assume an optional component is present, and a component that is not installed costs
   nothing: no disk, no memory, no running service, no boot time. (D18)

## Architecture decisions so far
- **Kernel: our own, in Rust, Linux-compatible.** We do not use the Linux kernel's C code. "Linux-compatible"
  means our kernel implements the Linux interfaces (system calls, ABI) so Linux software runs on it.
- **Foreign apps:** Windows, macOS, Android and iOS apps run on EggOS natively (no emulation, see
  important note 2) through compatibility layers we write ourselves, with the same features on each.
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

## The target: the fastest OS
EggOS must be the fastest operating system: faster boot, lower latency, higher throughput and a smaller
memory footprint than macOS, Windows and Linux on the same hardware, and the foreign apps it runs must
run at least as fast as on their native OS. This target outranks convenience and speed of development.
Every change is judged against it:
- **Measure, don't guess.** Every hot path (syscalls, scheduling, interrupts, page faults, IPC, I/O,
  memory allocation, graphics, the compatibility layers' translation paths) gets a benchmark when it is
  written. A PR that touches a hot path states before/after numbers from the same machine.
- **Never merge a performance regression.** A slower result blocks the PR until it is fixed or the owner
  explicitly accepts it.
- **Design for speed from the start**, not as a later optimisation pass: choose data structures and
  algorithms for their real cost (cache misses, allocations, locks, copies, context switches), avoid
  allocation and locking on hot paths, prefer zero-copy, and keep per-CPU state per-CPU.
- **Release builds are what we measure and ship** (LTO, one codegen unit); debug builds are for
  debugging.
- When performance and convenience conflict, choose performance. When a faster design would weaken
  correctness or security, do not choose silently: resolve it with a research subagent (see "Open
  questions" below) and record the decision.

## Priorities
1. Performance first: see "The target: the fastest OS" above.
2. Correctness and security: a kernel bug is a system-wide bug.
3. Feature parity across the compatibility layers.
4. UI/UX that feels familiar to anyone switching from macOS, Windows or Linux desktops: their habits
   (shortcuts, window controls, file locations, settings) should just work, with polish good enough that
   switching feels like an upgrade. Treat UX regressions like performance regressions.

The long-term goal is to be better than macOS, Windows and every Linux distribution for the people who use
them.

## Rules
- **Everything is written by us, in Rust.** Kernel, drivers, compatibility layers, userland, libraries,
  tools and build automation: all Rust, all our own code. No C or C++, no shell/Python/PowerShell build
  scripts: host automation is Rust, via `cargo xtask` (CI steps that only install tools or call cargo are
  fine).
  - Assembly only where the CPU leaves no choice (boot entry, trap vectors, context switch), kept to the
    minimum and only inside the HAL's architecture modules.
  - **No third-party crates, ever.** There are no exceptions and nothing to ask for: when we need
    something a crate would give us, we write it ourselves, so we control its performance, correctness
    and license. "There is a crate for it" is never a reason. `Cargo.lock` lists only our own crates.
  - Firmware the machine provides (UEFI, OpenSBI, QEMU's loaders) is outside EggOS and is not a
    dependency; nothing from it is copied into the repo.
- `unsafe` only where unavoidable, each block with a `// SAFETY:` comment explaining why it is sound.
- Development hosts are Windows and macOS; changes must build and be testable on both.
- Build/test: `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
- CI follows the global billing rules: Linux runners by default; macOS (10x cost) and Windows (2x) jobs
  only on PRs into main.

## Pull requests: review, then merge automatically
The owner does not merge PRs by hand. After opening a PR, in the same session:
1. Run both review passes from the global PR workflow, fixing what they find and pushing the fixes.
2. Wait for CI to finish (`gh pr checks <n> --watch`); a docs-only PR that CI skips has no checks.
3. **Merge it yourself** when all of these hold: both passes are clean, every CI check passed, the PR
   has no merge conflicts, it adds no third-party crate (never allowed; replace it with our own code
   first), and nothing in it needs the owner (below). Squash-merge to match the history
   (`gh pr merge <n> --squash --delete-branch`), then say in the report that it was merged.
4. If any condition fails, do not merge: fix what you can, and otherwise report plainly why the PR is
   still open.

An open question is never a reason to leave a PR unmerged: answer it with a research subagent (see
"Open questions" below), record the answer in the PR under "Decided by research", and merge.
Never auto-merge a PR that needs the owner: one that accepts a performance or UX regression, touches
credentials or money, or goes against a rule the owner stated. Those wait for the owner's OK. A PR that changes the license is never merged at all (see License).

## Releases: ship every major feature, don't ask
A major feature is a roadmap milestone (`docs/roadmap.md`, M-numbers). When one is finished, its PRs
are merged and CI on `main` passed, release it in the same session without asking the owner:
1. **Version** (semantic versioning, tags `vMAJOR.MINOR.PATCH`): until 1.0, each finished milestone
   bumps MINOR (`0.2.0`, `0.3.0`, ...) and fixes in between bump PATCH. `1.0.0` is the first release
   someone can install and use as their everyday OS.
2. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and mark the milestone **done** in
   the roadmap, in one PR, reviewed and merged like any other.
3. On the merged commit, run `cargo xtask ci` locally (no extra CI minutes); if anything fails, fix it
   first and do not release.
4. Build the kernel image for every architecture that boots (`cargo xtask build --arch <arch>`, release
   profile) and name each `eggos-<version>-<arch>.elf`.
5. Tag `v<version>` on that commit, push the tag, and publish a GitHub Release
   (`gh release create v<version> --title "EggOS <version>" --notes-file ... <images>`). The notes say
   what's new, what works, how to boot it in QEMU (`cargo xtask run --arch <arch>`), and known limits.
6. Report the release link.
Still the owner's: anything that needs new accounts, signing keys or other credentials, or money
(e.g. app-store or code-signing certificates).

## License: MIT, 100% open source, permanently
- All EggOS code is MIT (see LICENSE), and the license will never change: no relicensing, no dual
  licensing, no extra terms, no CLA that would allow it. Keep everything in the repo MIT-compatible.
- Clean-room only: never copy or translate code from GPL/LGPL/copyleft projects, including the Linux kernel
  (GPL-2.0). Implement Linux compatibility from public specifications, man pages and observed behaviour,
  not from Linux source.

## Open questions: answer them with a research subagent
Don't stop work to wait for the owner. When a question comes up (a design tradeoff, an assumption about
intent, a choice between approaches, a name), launch a research subagent with the question, the context
and the options; it researches the repo and, where useful, the web, and returns a recommendation with
reasons. Follow it, record it (PR description under "Decided by research", and docs/decisions.md for
architecture), and keep working. The owner can overrule any of these later.
Still open: none.

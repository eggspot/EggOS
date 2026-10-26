# EggOS roadmap

Status keys: **done**, **in progress**, **next**, planned (no marker).
Updated by every build-loop run; the order is refined as we learn (reasons in `docs/decisions.md`).

## v0.3.0 plan (next, owner 2026-10-26, D99)

The first interactive release, 2026-11-01. Until it ships this list comes before the milestones'
own order. Feature freeze: 2026-10-30 07:00.

Must-have, in order:
1. **done** — device interrupt routing (GIC SPIs, PLIC, I/O APIC) and UART receive (D98, M3).
2. **done** — console `read` with a line discipline (echo, erase, ^C/^D, `TCSETS*`, `POLLIN`),
   and an xtask QEMU test that feeds scripted serial input when the test program asks (D100).
3. **done** — a descriptor table per process (`close`, `dup`, `dup2`, `dup3`, `fcntl`,
   `close_range`), the console an ordinary open file description, `readv` and `O_NONBLOCK`
   reads (D101).
4. **done** — VFS core and a read-only initramfs (cpio, embedded by xtask): `openat`, `read`,
   `pread64`, `lseek`, `getdents64`, `fstat`/`newfstatat`/`statx` on files, `getcwd`, `chdir`,
   `fchdir` (D102). Later slices of issue #94: tmpfs, devfs, symbolic links, the dentry cache.
5. **done** — `execve` and `execveat` from a file: a new process with the caller's id, handlers
   reset, close-on-exec descriptors closed, other threads ended (D103).
6. **done** — process creation: `clone` without `CLONE_THREAD`, `fork` and `vfork`, the
   memory copied at once or shared with `CLONE_VM`, the parent sleeping until a `vfork` child
   execs or ends, children's exit records kept for `wait4` (D104). Follow-up: copy-on-write,
   and with it shared anonymous memory across `fork` (refused with `ENOSYS` until then).
7. **done** — `wait4` and `waitid`, zombies kept in a process table until reaped, orphans
   reparented (nearest subreaper, then init, then the kernel), `SIGCHLD`, `setpgid`,
   `getpgid`, `setsid`, `getsid`, `TIOCSCTTY`/`TIOCGPGRP`/`TIOCSPGRP`/`TIOCGSID`, `^C` to the
   console's foreground group (D105). Follow-ups: stop and continue (`WUNTRACED`, `SIGTTIN`,
   `SIGTTOU`), `kill` to other processes and groups (init is named as pid 1 by item 8).
8. **done** — init as pid 1: the release kernel starts every CPU and runs `/init` from the
   initramfs as process 1, the process orphans go to, instead of halting; `userland/init`
   starts the login program from `/etc/eggos/init.conf` (default `/bin/eggsh`, a banner and
   reaping only while it is missing) in its own session with the console, reaps every child,
   respawns it with a back-off or takes the machine down as configured; `reboot(2)` powers
   off, halts and resets through the HAL on all three, parking the other CPUs first; a QEMU
   test per architecture boots the release kernel into a test init that powers off, and
   another that resets (D106). Follow-ups: init's shutdown signals once `kill` reaches other
   processes, a protected init (`SIGKILL` and default actions not delivered to pid 1).
9. **done** — the `eggsh` shell (`userland/eggsh`, at `/bin/eggsh` in the release kernel's
   initramfs, which init starts): a prompt, quoting, `$` expansion, assignments and an
   environment prefix, `;` and comments, `PATH` search, the builtins `cd`, `pwd`, `exit`,
   `help`, `export`, `unset`, `set`, `echo`, `type`, `true`, `false`, `:` and `.`, `eggsh file`
   and `eggsh -c`, POSIX exit statuses (127, 126, 128 plus the signal); each program in a
   process group of its own that has the console, so `^C` reaches only it, and `^C` at the
   prompt a fresh prompt; pipes, redirections, `&&`, `||`, `&` refused as not supported yet
   (item 16); a QEMU test per architecture drives a session on the serial console (D107,
   docs/shell.md). Follow-ups: globbing, `$(...)`, functions and control flow, line editing
   and history.
10. **done** — the core utilities as one multi-call program (`userland/coreutils`, at
    `/bin/coreutils` with `cat`, `echo`, `false`, `halt`, `ls`, `poweroff`, `reboot`, `sleep`,
    `true` and `uname` as hard links to it, which the initramfs now supports: one copy of the
    data), and a boot-to-prompt QEMU test on all three architectures in CI: the release kernel
    boots to the shell's prompt, the test runs each utility and powers off with `poweroff`, and
    reports the time to the first prompt (D108, docs/coreutils.md). Follow-ups: `ls` in
    columns on a terminal and with user names, more utilities (`mkdir`, `rm`, `cp` once there
    is a writable file system), `poweroff` through init.
11. **design done** (D109) — UI design (Claude Fable, D16) for the boot screen, the terminal
    and a minimal desktop is in `docs/ux/desktop-v0.3.md` (with `terminal-v0.3.md` and the
    font `font-8x16.txt`), including the rendering plan and the acceptance criteria for items
    11-14; next, the virtio transport and a virtio-gpu 2D driver on all three, with the boot
    screen and the first-frame hashes.
12. virtio-input keyboard and mouse feeding the line discipline and an input event queue.
13. Graphical terminal: our own font rendering and the ANSI subset on the framebuffer.
14. Minimal desktop: wallpaper, top bar, the terminal in a window; a first-frame QEMU test.
15. Release engineering: `cargo xtask release`, boot-to-prompt time in the benchmarks, README
    "Try EggOS" and a limitations doc.

Nice-to-have, in order, only after the must-haves and before the freeze: an MCP server over the
serial console, pipes, a tmpfs at `/tmp`, a minimal `/proc`, an RTC driver for `date`, line
history. Checkpoint 2026-10-28 23:00: if `execve` or process creation is not green on all three,
fall back to one `eggsh` with built-in commands. Cut order when behind: `/proc`, RTC, tmpfs,
pipes, MCP; `fork`; the desktop's window controls and mouse; the fallback; last, ship a late
architecture as "boots, no shell". The graphical terminal is the owner's priority.

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo skeleton: workspace, pinned toolchain, HAL layout, CI, docs | **done** |
| M1 | Kernel boots in QEMU and prints, on the first architectures, with an automated QEMU test runner | **done** — aarch64, riscv64, x86_64 |
| M2 | Memory management: physical frames, paging, kernel heap | **done** (0.2.0) — memory map from the device tree / PVH and a frame allocator on aarch64, riscv64, x86_64 (D28); kernel page tables: a direct map in the high half, W^X kernel sections and a boot-stack guard page on aarch64, riscv64, x86_64 (D29), with the image's direct-map alias keeping its protections (D32); kernel heap: `alloc` works, size classes up to 2 KiB (D30); buddy allocator: contiguous, aligned blocks of up to 4 MiB, so heap allocations up to 4 MiB work (D31); the kernel runs in the top 2 GiB with boot page tables built before any Rust code, and the low half is empty (D33) |
| M3 | Interrupts and timers | **done** (released with v0.3.0, D99) — exception vectors (D25); monotonic clock and one-shot timer interrupts through GICv2, SBI and the local APIC on aarch64, riscv64, x86_64 (D26); device interrupt routing: GICv2 SPIs, a PLIC driver with the boot hart's context from the device tree, an I/O APIC driver with the MADT's overrides, and the console UART's receive interrupt on all three, its bytes queued in the kernel (D98); GICv3, AIA and MSI-X come with the driver model (M5) |
| M4 | Scheduler, processes and threads (SMP-ready) | **in progress** — kernel threads with a context switch on aarch64, riscv64, x86_64 and a cooperative round-robin scheduler on one CPU, 16 KiB stacks with an overflow canary (D35); voluntary preemption: a 10 ms timer tick ends time slices and threads switch at preemption points, the scheduler's lock handed over across the switch (D37); sleeping and wait queues: an idle thread, `sleep`, `WaitQueue`, a blocking `join`, threads linked through themselves, and one timer shared by the tick and the sleepers with no tick while one thread is runnable (D39); secondary CPUs: discovered from the device tree / MADT, started (PSCI, SBI HSM, INIT-SIPI with a real-mode trampoline) on the kernel's page tables, each with a per-CPU block behind its per-CPU register, reporting in and parking (D40); every CPU takes its own interrupts: per-CPU timer and interrupt controller setup, wake-up interrupts between CPUs (GICv2 SGI, SBI IPI, local APIC fixed IPI), per-CPU interrupt counts, and a one-load per-CPU read for hot paths (D41); every CPU runs threads: per-CPU run queues, sleepers and idle threads, new threads placed on idle CPUs first, threads woken from any CPU onto their own with the wake-up interrupt or a reschedule request, one scheduler lock for now (D43); load balancing: an idle CPU takes ready threads from busy ones, and a slice that ends with threads ready wakes an idle CPU (D44); user address spaces: a low half of their own with the kernel half shared, user mappings on aarch64, riscv64, x86_64, switching with the CPU's user translations dropped, and kernel access to user pages only in a window (D45); user mode: the kernel runs a program in EL0, U-mode or ring 3 until a system call (`svc`, `ecall`, `syscall`), an interrupt or an exception brings the CPU back, with the program's registers in a `UserContext` and Linux's system-call registers; a per-CPU GDT and TSS on x86_64; a system-call round trip in about 106 (aarch64), 148 (riscv64) and 107 (x86_64) instructions (D47); processes: threads that run their program in a loop and hand each exit to a handler, address spaces loaded lazily per CPU at the point of use and kept until another is needed, preemption on return to user mode, processes on every CPU (D48); review of D47–D48: user programs get no FP/SIMD, vector unit or timer access whatever firmware left, `int3` after x86 returns, speculation mitigations per CPU with recorded triggers (D49); next: the Linux system calls (M6); per-CPU and per-wait-queue locks wait for a workload that shows contention (D45) |
| M5 | Common driver model (bus enumeration, device tree / ACPI discovery) | |
| M6 | Linux syscall ABI: what a static hello-world needs (`write`, `exit_group`, `brk`, `mmap`, ...) | **in progress** — the Linux system-call layer: dispatch by Linux number per architecture, `write` to the console, `exit` and `exit_group`, `ENOSYS` for the rest, exceptions end the process with Linux's signal, user memory copied after a table walk (D50); programs start from ELF images: a checked ELF parser, executables and static PIEs mapped page by page with W^X and a 0x10000 floor, Linux's initial stack with `argv`, `envp` and the auxiliary vector, Linux's `execve` errors (D51); demand paging: processes have regions (Linux's mappings), a page is mapped with a zeroed frame when the program first touches it, the stack is one 8 MiB region mapped as it grows, kernel copies map untouched pages, and live address spaces gain mappings (D52); the memory calls: `brk`, anonymous `mmap`, `munmap` and `mprotect` on the regions with Linux's checks and errors, top-down placement, `PROT_NONE` pages keeping their frame in an invalid entry, and other CPUs that keep a changed address space loaded marked stale so they switch again before using it (D53); FP/SIMD registers per thread: given on first use, saved on every switch away and loaded again only when the CPU no longer holds them, cleared before a program that has none runs (LazyFP), the RISC-V kernel soft-float (D54); the first startup calls: `writev`, `getpid`, `getppid`, `gettid`, `uname` (Linux 6.1.0-eggos), `set_tid_address` and `set_robust_list`, and process ids (D55); the kernel's random number generator (BLAKE2s input, ChaCha20 with fast key erasure and per-CPU keys, seeded from the device tree's `rng-seed` and `RDSEED`/`RDRAND`), `AT_RANDOM` from it and `getrandom`, with frames and page tables zeroed a line at a time (`linux_exec_load` −27%, −18%, −1%) (D56); the thread pointer in each thread's user context, loaded on every way into user mode (`TPIDR_EL0` saved and loaded, `tp`, the FS base written only when it changed), x86_64's `arch_prctl` and `sched_yield`, with no cost on the thread switch and the system-call round trips slightly faster (D57); review of D55–D57: empty ranges past user space `EFAULT` as on Linux, `writev`'s count an `unsigned int`, BLAKE2s wiped with volatile stores (D58); resource limits: `getrlimit`, `setrlimit` and `prlimit64` on per-process limits with a current distribution's defaults, copied on a process's first change, no hard limit raised until processes have credentials, not yet enforced (D59); signal state: `rt_sigaction`, `rt_sigprocmask` and `sigaltstack` with Linux's rules, actions per process copied on first change, the blocked mask and alternate stack per thread, handlers accepted but nothing delivered yet (D60); the standard descriptors: 0, 1 and 2 are the console, open for reading and writing as Linux starts `init`; `poll` (x86_64) and `ppoll` with Linux's checks, the console always writable and never readable, waits that sleep out their timeout and write back the remaining time; the console's `ioctl`s: `TCGETS` and `TCGETS2` with a Linux serial console's settings, `TIOCGWINSZ` and `TIOCSWINSZ`, `FIONREAD`, `ENOTTY` for the rest (D61); review of D59–D61: `linux_exec_load` loads only the code up to the end of `linux_start`, so new HAL test programs no longer grow it, `writev` checks its entries in order as Linux does, and `ppoll` writes the time not slept back for every timeout that is not zero (D62); signals sent and delivered, part 1: `kill`, `tkill` and `tgkill` to the caller (its own group for 0 and minus its id), `rt_sigpending`, a pending set per thread, and delivery by the default action or `SIG_IGN` before the call that made a signal due returns (Term and Core end the process with the signal, so `abort()` ends with `SIGABRT`; the terminal's stop signals are discarded in an orphaned process group; `SIGSTOP` stops for good; `SIGCONT` and the stop signals cancel each other), with nothing added to the system-call path (D63); signal handlers: Linux's signal frames on each architecture (`siginfo_t`, `ucontext_t`, FP/SIMD parts in Linux's shape holding the initial state), `rt_sigreturn` restoring every register, the mask and the alternate stack, `SA_SIGINFO`, `SA_ONSTACK` with `SS_AUTODISARM`, `SA_NODEFER`, `SA_RESETHAND`, one frame per due signal stacked as on Linux, `SIGSEGV` for a frame that cannot be written or read, and a return trampoline mapped on first use for AArch64 and RISC-V handlers without a restorer, with nothing added to the hot paths (D64); faults delivered to signal handlers: the signal, `si_code` and `si_addr` Linux gives for each exception on each architecture (`SEGV_MAPERR` or `SEGV_ACCERR` by the process's regions), x86_64's `err`, `trapno` and `cr2` and AArch64's `fault_address` and `esr_context` in the frame, sanitized for addresses outside user space, forced as on Linux (a blocked or ignored fault signal ends the process), new HAL kinds for `#DE`, `#DB`, `#NP` and `#SS` (D65); review of D63–D65: synchronous signals delivered before others, those the kernel raised first, so a failed frame's `SIGSEGV` is not overtaken, and AArch64 frames of `brk` and undefined instructions with no fault address or `esr_context`, as on Linux (D67); real-time signals queued with their `siginfo_t` (each instance in send order, a standard signal keeping its first info), `RLIMIT_SIGPENDING` enforced with Linux's exceptions, `rt_sigqueueinfo`, `rt_tgsigqueueinfo` and `rt_sigtimedwait`, nothing added to the hot paths (D68); waiting for a signal: `rt_sigsuspend` with its mask in place until a handler starts (the frame saving the mask from before), then `EINTR`, and x86_64's `pause` (D70); `ppoll`'s signal mask applied during the call, a signal it unblocks interrupting a wait that finds nothing ready (`EINTR` once a handler starts, its frame saving the thread's mask) (D71); review of D68–D71: `ppoll` interrupted by an ignored signal fails with `EINTR` when its timeout cannot be written back, `rt_sigtimedwait` takes synchronous signals by number, kernel-raised ones delivered in the order raised, a drained signal queue shrunk (D72); clocks and sleeping: `clock_gettime`, `clock_getres`, `gettimeofday`, x86_64's `time`, `nanosleep` and `clock_nanosleep` (relative and absolute, never early) on the realtime, monotonic, boot-time and TAI clocks, the wall clock starting at the epoch until a real-time clock driver sets it, the CPU-time clocks `EINVAL` until there is CPU time, `clock_settime` and `settimeofday` `EPERM` until processes have credentials (D73); the console's status: `fstat`, `newfstatat` and `statx` on descriptors 0 to 2 report `/dev/console`'s character device 5:1 (root, `0600`, epoch times, `statx` with the basic fields), with Linux's path lookup and errors (a null path with `AT_EMPTY_PATH` accepted, other paths `ENOENT`, `ENOTDIR` or `EBADF` until there are files), and `lseek` refused with `ESPIPE` (D74); credentials: real, effective, saved and file system user and group ids and supplementary groups, starting as root, with `getuid`, `geteuid`, `getgid`, `getegid`, `getresuid`, `getresgid`, `getgroups`, `setuid`, `setgid`, `setreuid`, `setregid`, `setresuid`, `setresgid`, `setfsuid`, `setfsgid` and `setgroups` on Linux's rules, privilege as effective uid 0 (exact until capabilities exist), root raising hard limits, the auxiliary vector's ids and `AT_SECURE` from them, `si_uid` the sender's (D75); review of D73–D75: `setgroups` refuses group id -1 in list order, a `setresuid` or `setresgid` that changes nothing keeps the file system id, `newfstatat` takes `statx`'s synchronisation flags (D76); the memory calls allocators make at run time: `madvise` (`DONTNEED`, `DONTNEED_LOCKED` and `FREE` free the pages' frames and leave zeros, `POPULATE_WRITE` maps frames up front, `POPULATE_READ` checks, the advice about paging, merging, huge pages, dumps and `fork` accepted, `REMOVE` and `COLLAPSE` `EINVAL`, `HWPOISON` and `SOFT_OFFLINE` `EPERM` unprivileged) and `mremap` (shrinking, growing in place below the stack's guard gap, moving with `MREMAP_MAYMOVE` by relinking each page's entry so frames and contents go along and the pages go back if tables run out, `MREMAP_FIXED`, `MREMAP_DONTUNMAP`), Linux's checks in Linux's order (D77); signal frames hold the thread's FP/SIMD registers and `rt_sigreturn` loads them with Linux's checks (x86_64's handlers start with the initial state, a null `fpstate` is the initial state, a misaligned one or reserved `MXCSR` bits `SIGSEGV`; a thread without registers gets them when its frame holds other values) (D82); review of D54, D77 and D82: x86_64 clears `CR4.OSXSAVE`, `PKE`, `CET` and `UINTR` whatever firmware left, so no program sees AVX, protection-key, shadow-stack or user-interrupt state that is never switched, `mremap` with `MREMAP_FIXED` checks the old range's end before it moves anything, `MADV_POPULATE_WRITE` leaves mapped pages alone (D83); the official `*-unknown-linux-musl` targets pinned for test programs built from our own Rust source with std (D84); a first program built from our own Rust source runs in the QEMU self-tests on aarch64, riscv64 and x86_64, on a runtime of our own with no C library (`userland/runtime`: `_start`, a static PIE relocating itself, `PT_GNU_RELRO` protected, raw system calls), checking 17 start-up calls a C library and std make (D85); a program on Rust's standard library, statically linked with musl, runs in the QEMU self-tests on all three: musl's and std's start-up, a `HashMap` with random keys, the heap, clocks, sleeping and standard output, loaded as a static PIE on x86_64 and a fixed-address executable on AArch64 and RISC-V (D86); `AT_MINSIGSTKSZ` in the auxiliary vector, our `MINSIGSTKSZ`, checked at compile time to hold the largest signal frame at any alignment (D87); review of D84–D87: user programs build whatever `RUSTFLAGS` holds, relocation tables checked whole, exit statuses read per program (D88); `futex`: `FUTEX_WAIT`, `FUTEX_WAKE` and their bitset forms, private and shared kept apart, Linux's checks in Linux's order, FIFO waiters on the waiting threads' stacks in 256 buckets, on a new timed park in the scheduler (`thread::Parker`) (D89); `FUTEX_REQUEUE` and `FUTEX_CMP_REQUEUE` with Linux's counts, order and errors, for musl's condition variables (D90); threads: `clone` with `pthread_create`'s flags (`CLONE_SETTLS`, the id words, `CLONE_CHILD_CLEARTID`'s clear and shared wake), each thread's own id, blocked mask, pending signals and alternate stack, `set_tid_address` remembered, `exit` ending one thread and `exit_group`, fatal signals and the out-of-memory kill ending every thread (other CPUs asked to look, futex waits woken), the first thread's status when all end alone, Linux's `EINVAL`s and `ENOSYS` for new processes; Rust's `std::thread`, `Mutex` and `Condvar` run in the QEMU self-tests; a process's threads stay on one CPU until changed mappings are shot down on others (D91); review of D89–D91: every Linux sleep (`nanosleep`, `ppoll`, `rt_sigsuspend`, `pause`, `rt_sigtimedwait`, a stop) waits in the futex table, so a process's end wakes it and its thread ends, and `Parker::unpark` takes a pointer, since the parked thread may drop its `Parker` before the call returns (D92); robust futex lists: `set_robust_list` remembered, `get_robust_list` for the caller, and a thread's exit releasing the locks on its list that it holds (owner-died bit, waiters bit kept, one shared wake, 2048 entries at most, the pending entry last) before its tid word is cleared (D93); a process's threads run in parallel on every CPU: a TLB shootdown (each CPU holding the address space asked to drop its user translations, the wake-up interrupt, a wait for each, served on wake-ups, in the wait itself and while waiting for any lock but a leaf one), pages unmapped in two steps so no CPU writes a frame after it is reused, futex words read and robust locks released with atomics on user memory, `getcpu`, and no thread pinned any more (D94); signals between a process's threads: `tkill`, `tgkill` and `rt_tgsigqueueinfo` reach every thread of the caller's process (Linux's `ESRCH`, an exited first thread taking signals it never delivers, `SIGKILL` included), `SIGKILL` ends the process at once, any other signal waits in the process's signal state until its thread takes it in, as if sent then: woken from its sleep (a futex word per thread), interrupted on another CPU, interrupting its own CPU when switched back in, or on its way back from an interrupt, `sched_yield`, `futex`, a sleep or before its own signal calls; `SIG_IGN` discards every thread's pending instances in order with what was sent; `rt_sigtimedwait`, `rt_sigsuspend`, `pause`, `poll` and `ppoll` (with or without a mask) are woken and interrupted by them, `nanosleep` sleeps out its time and delivers after; Rust's std program handles a signal sent to a thread waiting in `sigsuspend` and to one that keeps running; nothing added to the system-call round trip (D95); signals sent to the process (`kill`, `rt_sigqueueinfo`) taken by the caller or, if it blocks them, by the first thread that does not, woken or interrupted to look, a thread that unblocks one later or waits for it in `rt_sigtimedwait` taking it, `rt_sigpending` reporting them; Rust's std program handles a `kill` to itself in a thread that does not block it (D96); signals interrupt `futex` waits and sleeps as on Linux: `nanosleep` and `clock_nanosleep` write the time left and fail with `EINTR`, an untimed `futex` wait restarts after an `SA_RESTART` handler and fails with `EINTR` otherwise, a timed one fails with `EINTR`, calls restarted by the HAL's `restart_syscall`, the sender reaching the waiter through its thread's wait slot in one futex bucket, a stopped thread leaving the process's signals to the others (D97); the console's line discipline: `read` from descriptors 0 to 2 in canonical mode (echo, erase with delete and backspace, word erase, kill, end of file, `^C` and `^\` discarding the input and signalling the console's foreground, the last thread that read it or polled it) and non-canonical mode (`VMIN`, `VTIME`), restarted after an `SA_RESTART` handler, `TCSETS`, `TCSETSW`, `TCSETSF` and their `termios2` forms, `TCFLSH`, `FIONREAD`, and `poll`/`ppoll` reporting `POLLIN` and woken by input (D100); a descriptor table per process shared by its threads (`close`, `dup`, `dup2`, `dup3`, `fcntl`, `close_range`, `FIONBIO`, `FIOCLEX`, `FIONCLEX`), the console an open file description on descriptors 0 to 2, `readv` and `O_NONBLOCK` reads (D101); children's ends: a process table (parent, exit signal, group, session, zombie), `wait4` and `waitid` (interruptible, `WNOHANG`, `WNOWAIT`, `__WCLONE`, `__WALL`), `SIGCHLD` with `CLD_EXITED`/`CLD_KILLED`, ignored `SIGCHLD` and `SA_NOCLDWAIT` reaping at once, orphans to the nearest `PR_SET_CHILD_SUBREAPER` ancestor, init or the kernel, `setpgid`, `getpgid`, `getpgrp`, `setsid`, `getsid`, the console a session's controlling terminal with a foreground group (`TIOCSCTTY`, `TIOCGPGRP`, `TIOCSPGRP`, `TIOCGSID`) that `^C` and `^\` signal, its leader's end hanging it up (D105); next: interrupting only the target's CPU; a process-wide completion for launchers, with `wait4`; C programs linked with musl; the SWAPGS fence before programs can set GS or map low pages (D49) |
| M7 | VFS and a first file system | **in progress** — the VFS core (a `FileSystem` trait, nodes, Linux's path walk), a read-only initramfs built by xtask from `initramfs/` as a `newc` cpio archive, embedded in the kernel and mounted as `/`, served in place; `openat`, `read`, `pread64`, `lseek`, `getdents64`, `fstat`/`newfstatat`/`statx` on files, `getcwd`, `chdir`, `fchdir`, a working directory per process (D102); `execve` and `execveat` from a file of the initramfs, loaded into a new process with the caller's id and exit record, with Linux's checks and errors before the point of no return, signal actions reset, close-on-exec descriptors closed, other threads ended, and the test kernels' programs in `usr/libexec/` (D103); next: tmpfs, devfs, `#!` scripts, setuid on exec (issue #94) |
| M8 | virtio drivers (console, block, net, rng, input) | |
| M9 | More architectures through the HAL | |
| M10 | Userland: init, shell, core utilities, and the built-in MCP server with its permission model (default deny, local only): every command is also an MCP tool (D21, D22); first size/memory/boot budgets (D24); the first installer: a compressed base image that downloads and installs in seconds, with download size, install time and first-boot time measured (D69) | **in progress** — init as pid 1, started by the release kernel, with `reboot(2)` (D106); next: the `eggsh` shell and the core utilities |
| M11 | Graphics stack (virtio-gpu first) and compositor | |
| M12 | Adaptive shell, designed by Claude Fable from `docs/ux/` (D16, D23): desktop first, then phone/tablet and TV | |
| M13 | Real hardware and vendor GPU drivers | |
| M14 | Component manager (D18) and app capability registry: installed apps published as CLI commands and MCP tools; agent permissions and audit log (D21); components downloaded on demand in the background, delta updates applied atomically (D69) | |
| M15 | Compatibility layers, native not emulated (D17), each an optional component exposed to AI through M14: Linux userland, Windows and macOS side by side, then Android and iOS (order: D34) | |
| M16 | Editions as component presets (D18): personal, professional, enterprise, server | |

Timer follow-ups (D26): on x86_64, read the TSC/APIC frequencies from CPUID 0x15 and hypervisor
leaves instead of measuring, TSC-deadline mode, and the ACPI PM timer or HPET as fallback
reference clocks when there is no 8254; `stimecmp` directly when RISC-V Sstc is present; GICv3; device-tree timebase and GIC
addresses (M5).

Paging follow-ups (D29, D33): a separate exception stack (x86 IST, AArch64/RISC-V stack switch) so a guard-page hit is reported; TLB maintenance for changing live tables (M4; user address spaces need none yet, D45); honour device-tree `no-map` reservations (M5); AArch64 data-cache maintenance for memory written with the MMU off, the boot tables included, and a boot direct map built from the device tree instead of QEMU `virt`'s fixed device/RAM split (real hardware, M13); secondary CPUs start on the kernel's tables, global kernel pages and PCID (M4); a device window behind a mapping call instead of the direct map's (M5); RAM above 4 GiB on x86_64; on x86_64, map ACPI tables on demand and reserved PVH ranges uncached instead of direct-mapping them cached (M5; early device pages are already left out, D38); keep the PVH start info and memory map in `physical_memory` even when a loader map has more than 62 disjoint entries; hardware probes in the boot self-tests that the low half, the image's direct-map alias and the stack guard page really fault (they check the tables in software today); 1 GiB pages on x86_64 (CPUID); Sv48/Sv57 chosen at boot on RISC-V when Sv39's 256 GiB is not enough; Svpbmt.

Address-space follow-ups (D45): (threads that carry an address space, switched only when it
differs: done, lazily at the point of use, D48; the first trip to user mode and back: done, D47);
ASIDs and PCID; (per-page TLB maintenance on this CPU: done, D52, `drop_user_translations`;
mappings added while active: done, D52); (cross-CPU TLB maintenance for the memory calls: done, D53, CPUs that keep the address space
loaded are marked stale and switch again before using it); (a shootdown once `clone` runs a
process on several CPUs at once: done, D94, the wake-up interrupt and a wait per CPU); the
shootdown by broadcast invalidation where the CPU has it (AArch64 `tlbi ...is`, RISC-V's SBI
remote fence or Svinval), ranged instead of every user translation, and only to CPUs running
the process rather than every CPU that keeps it loaded (D94); `with_active` and
`with_user_memory` of a spread address space from another CPU; fault-around
and large pages for demand-zero regions after measuring; a region tree that tracks gap sizes past
about a thousand regions (D52); from PR #44's review: table entries written with an atomic
release store, kernel copies that drop the regions lock between pages once `clone` exists, a
shared zero page for kernel reads of untouched memory, a region for the HAL test program's code; PAN, SMAP and SMEP; (user faults that end the process: done, D50).

User-mode follow-ups (D47): (processes whose threads run their program in a loop over `run_user`,
preempted on return to user mode: done, D48); on x86_64, IST stacks for NMI, double fault, machine check and debug exceptions before any
untrusted program runs (`syscall` and `sysretq` leave a moment in ring 0 on the program's stack),
the paranoid GS check in those handlers, and reloading SS for AMD's `sysretq`; FP/SIMD state per
thread, saved lazily, once programs may use it (it traps on every architecture, whatever firmware left: D49); (TLS registers (`TPIDR_EL0`,
`FS` base) in the context: done, D57), and `KERNEL_GS_BASE` per program with `ARCH_SET_GS` (D57); the Linux system calls themselves
(M6); a measured system-call cost outside QEMU `-icount` (M13). From PR #39's independent review:
a self-test that interrupts a program mid-pattern and checks every register (the `iretq` and
interrupt-exit paths); `SCTLR_EL1.UMA` and `E0E` written explicitly, not left to firmware (M13);
`IA32_CSTAR` at a safe stub before any 32-bit user segment exists; on AArch64, `SCTLR_EL1.UCT`,
`UCI`, `DZE` and `nTWE`/`nTWI` set as Linux allows (EL0 `mrs ctr_el0`, `dc cvau`, `ic ivau` for
`__clear_cache` and JITs, `dc zva`, `wfe`), found by the D52 research; DPL-3 gates for `int3` and
`into` once user debuggers matter; UMIP where the CPU has it; `TSS.RSP0` cleared when `run_user`
returns. From the D47–D48 review (D49): an NMI from ring 3 handled by the kernel, not as the
program's exception, and NMIs unblocked with an `iretq` (with the IST stacks); AArch64
`PSTATE.DIT`, `SSBS` and `TCO` for programs; `-Zharden-sls` for rustc's own `ret`s; RISC-V
`scounteren` written explicitly (harmless today: counter reads cannot touch the timer).

Speculation mitigations (D49), each chosen per CPU at boot: the SWAPGS `lfence` (a second copy
of the x86 entry stubs) **before a program can set its GS base or map pages near address 0**
(M6; `mmap`'s floor, 64 KiB as Linux's `mmap_min_addr`, is in place, D53); KPTI with PCID, MDS buffer clearing and Retbleed **before x86_64
runs anything but our own test programs on a real Intel CPU** (M13 at the latest); KPTI for
affected Arm cores (Cortex-A75).

Fault follow-ups (D65): `AT_MINSIGSTKSZ` in the auxiliary vector (Rust's std sizes its
alternate stacks from it); `#MF` and `#XM` with their `FPE_*` codes and `trapno`, and AArch64's
trapped FP exceptions, with D54; AArch64's top byte cleared from `si_addr` unless
`SA_EXPOSE_TAGBITS`, if TBI is turned on for programs; RISC-V misaligned accesses emulated
as Linux does (QEMU's harts do them in hardware); `SIGBUS` `BUS_ADRERR` past the end of a
file mapping (M7); `TRAP_TRACE` and `TRAP_HWBKPT` with single-step and debug registers
(`ptrace`); `/proc/self/maps` and `RLIMIT_STACK` for glibc's `pthread_getattr_np`, so Rust's
std finds the main thread's guard page and reports an overflow by name. From PR #58's
independent review: RISC-V's `si_addr` (`sepc` or `stval`) and `si_code` for access faults
and misaligned accesses checked against a real Linux under QEMU; `TRAP_TRACE` for a single
step a program set with `popf`; a non-canonical `rip` (only through `rt_sigreturn`) reported
as `#GP` (`SI_KERNEL`, `trapno` 13) rather than a fetch page fault; NMIs and machine checks
from user mode handled by the kernel instead of becoming the program's `SIGSEGV`;
(AArch64's `fault_address` 0 for `SIGILL` and `SIGTRAP`: done, D67); checked against a real
Linux: whether 6.4 and later pass the syndrome for undefined instructions, and what alignment
faults and external aborts report in `fault_address` and `esr_context`; whether AArch64's
`rt_sigreturn` sends `SIGSEGV` for a frame whose `pstate` is not EL0t (we mask it and
continue); a QEMU test of `SIGILL` and `SIGTRAP` handlers (`ud2`, `brk`, `ebreak`) with their
`si_addr` and resume point.

Process follow-ups (D48): preemption on whenever user programs run (with it off, as for kernel
threads, a program that never traps keeps its CPU); waking CPUs that keep an ended process's
address space loaded while they stay busy with kernel threads, or idle after a look that raced with
another CPU switching away from it (an idle CPU lets it go otherwise); (TLB shootdown, to every CPU that has the
address space loaded, once a shared process's mappings may change: done, D94); (pages owned by the address
space: done, freed with it, D52, and by `munmap`, D53); (user faults that end the process with Linux's default signal, and the
exit status `exit_group` records: done, D50); signal delivery to handlers (their state: done, D60; default actions and `SIG_IGN`: done, D63; handlers: done, D64; queued `siginfo_t`: done, D68; between a process's threads: done, D95, process-directed ones taken by any thread: done, D96; `futex` waits and sleeps interrupted, `EINTR`, `rem`, `SA_RESTART`: done, D97; `rt_sigsuspend` and `pause`: done, D70); (a process's threads, `clone`: done, D91, in parallel on every CPU, D94)
and `wait4`; (user-memory copies racing another thread's writes: decided by research, D94: the
copy stays a plain one through the direct map, each byte read once into a kernel buffer, the
bytes unspecified as on Linux, and words used as atomics go through `Process::with_user_word`);
user-memory copies at user addresses with fault fixups when something needs them; (a cap per `write` call, Linux's `MAX_RW_COUNT`, and a preemption
point per chunk, so one large write cannot hold its CPU: done, PR #46).

Exec follow-ups (D51): (a kernel random number generator for `AT_RANDOM`, and `getrandom`:
done, D56); layout randomization
of the PIE base, stack and `mmap` area; (demand paging, so the stack grows into its 8 MiB
reserve and its pages cost frames only when touched: done, D52); `execve` itself, with the image read from
a file (M7) and the process's old address space replaced; dynamically linked programs, with
the interpreter loaded at its own base and `AT_BASE` set (M7); `AT_SYSINFO_EHDR` with a vDSO
(fast clock reads), `AT_MINSIGSTKSZ` with signal frames; `AT_HWCAP` recomputed when FP/SIMD
state is saved per thread; RWX segments or a JIT path only if a real binary needs one; a
(faster zeroing of frames and page tables: done, D56); pre-zeroed frames; before `execve`
takes outside input, a cap on a program's eagerly mapped pages and the initial stack written
straight into its frames (no 2 MiB heap buffer); `CPUID` for `AT_HWCAP` read once at boot;
two segments on one page if a real binary needs it.

FP/SIMD follow-ups (D54): AVX and AVX-512 through `XSAVE`/`XSAVEOPT`, SVE/SME on AArch64 and V
on RISC-V, each with a first-use trap of its own; FP/SIMD state in signal frames (`MXCSR`
checked on `sigreturn`); `fork` and `clone` copying or starting it; #XM and #MF with their own `FPE_*` codes (they are `SIGFPE` with `FPE_INTDIV` now);
the AMD x87 pointer reset skipped where CPUID 0x80000008:EBX[2] says it is not needed. From
the D83 review: the CPU's own `MXCSR_MASK` in the frames of threads without registers;
`SIGFPE` for AArch64's trapped floating-point exceptions (EC 0x2C); a build check that the
kernel image has FP/SIMD instructions only in the HAL's `fp` code and the test programs; a
QEMU test of `rt_sigreturn` with reserved `MXCSR` bits; `CR4.PKS` cleared too, whatever
firmware left (it affects only the kernel).

Memory-call follow-ups (D53, D77): a "shared" mark on `MAP_SHARED` anonymous regions before
`fork`, so it shares their pages instead of copying them (then `MADV_REMOVE`, `MADV_FREE`'s
`EINVAL` and `mremap` with an old size of 0 on shared memory, as on Linux); `fork` keeping
`MADV_DONTFORK` and `MADV_WIPEONFORK` per region (D77 accepts them with nothing to apply them
to yet); a shared zero frame for `MADV_POPULATE_READ` and read faults; (`madvise` and
`mremap`: done, D77); a `mremap` benchmark once `realloc` of large blocks shows in a real
workload; `MAP_POPULATE` and `MAP_GROWSDOWN` when a program needs them;
`PROT_BTI` enforced (the `GP` bit) once BTI is reported in `AT_HWCAP2`; file mappings (M7);
`RLIMIT_DATA` and `RLIMIT_AS` for `brk` and `mmap`; randomized `mmap` base and break with the
random number generator; from PR #45's independent review: (`munmap` and a shrinking `brk`
free their frames after the translations are dropped, taking the frame allocator's lock once:
done, PR #46), and anonymous `MAP_SHARED_VALIDATE` and `MAP_SHARED | MAP_GROWSDOWN`
checked against Linux's observed behaviour (`EINVAL` there, possibly, where we accept or say
`EOPNOTSUPP`). From the D83 review: regions that know what backs them (M7), so
`MADV_DONTNEED` on a program's ELF segments reads the file's contents again, `MADV_FREE` there
is `EINVAL`, and moving the signal trampoline's page moves its recorded address too.

Clock follow-ups (D73): real-time clock drivers (PL031, Goldfish, CMOS) setting the wall
clock at boot, after M5; a vDSO for clock reads without a system call, with the counter
readable from user mode again (D49 turned it off); per-thread CPU time for the CPU-time
clocks, `getrusage` and `times`, with the owner's OK on its switch-path cost; the
invariant-TSC check (SMP follow-ups) so `CLOCK_MONOTONIC` never goes backwards across x86_64
CPUs; absolute realtime sleepers re-evaluated once the wall clock can be set; `EINTR` with the
time left in `rem` once signals can arrive during a sleep.

Signal-queue follow-ups (D72): a queue that grows by fixed-size chunks instead of one
contiguous buffer, so a long queue of real-time signals never needs a large contiguous block
or copies itself under the signals lock before it reaches `RLIMIT_SIGPENDING`.

Startup-call follow-ups (D55): process ids reused after `wait4` reaps them (the process table
exists, D105);
`set_tid_address`'s word cleared and woken and the robust list released at thread exit, with
`clone` and futexes; (parents for `getppid` with `fork`: done, D104, reparenting D105); `sethostname` and `setdomainname`; the
same release in `/proc/sys/kernel/osrelease` and `/proc/version` (procfs); a `writev`
benchmark once a sink faster than the console's UART exists (M7), and `getpid`'s round trip
as the cheapest real call; (`writev`'s two edge cases from PR #48's review checked against
Linux: done, D58); (`writev` with a partly unreadable array whose readable entries hold a
negative length `EINVAL`, as Linux reads the entries in order: done, D62); `init` as pid 1 (M10).

Descriptor follow-ups (D61): (a descriptor table per process with `close`, `dup`, `dup2`,
`dup3`, `fcntl` (`F_DUPFD`, `F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`),
`close_range`, `FIONBIO`, `FIOCLEX` and `FIONCLEX`: done, D101); the descriptor table's
follow-ups (D101): `F_SETFL`'s `O_ASYNC`, `O_DIRECT` and `O_NOATIME`, record locks
(`F_GETLK`, `F_SETLK`, `F_OFD_*`), `F_SETOWN` and `SIGIO`, `O_LARGEFILE` in `F_GETFL` once
`openat` sets it, a descriptor table above 4 MiB of slots; (console input with `read`,
`POLLIN` and a line discipline behind `TCSETS`, `TCSETSW` and
`TCSETSF`, pollers woken when input arrives: done, D100); the line discipline's follow-ups
(D100): (`readv`, `O_NONBLOCK` reads: done, D101), `VREPRINT`, `VDISCARD`, `VSUSP` with job control, `IXON`
flow control, output flags that the UART driver obeys, tab erase from the column the output
left (output column tracking), virtio-console input (M8); `SIGWINCH` on
`TIOCSWINSZ`; sessions
and job control (`TIOCGPGRP`, `TIOCSPGRP`, `TIOCSCTTY`) with `setsid` (M10); `select`,
`pselect6` and `epoll` (M7); `poll` arrays copied in pieces with preemption points
between them, once programs poll many thousands of descriptors; checked by observation on a
real Linux: the order of `ppoll`'s checks, whether `sigsetsize` is checked with no mask,
`CLOCAL` on a serial console, whether current glibc reads settings with `TCGETS2`, and whether `poll`
of no descriptors at an address past user space is `EFAULT`; (the time not slept written back
by every `ppoll` with a timeout that is not zero, also when something is ready at once: done,
D62); only the `revents` fields written back, as Linux does, once `clone` lets another thread
change the array during a wait (today the whole array is copied back).

Thread-pointer follow-ups (D57): `CLONE_SETTLS` with `clone`; FS and GS selectors saved and
reset per program; `ARCH_SET_GS` with the SWAPGS fence (D49); FSGSBASE for programs (the FS
base then saved on the way out); a per-CPU check instead of AArch64's unconditional `msr
tpidr_el0` if real hardware shows it costs; `set_thread_area` and 32-bit programs; with
FSGSBASE, a new `fs_base_id` whenever the saved FS base changes, and `ARCH_GET_FS` reporting 0
after a program loads an FS selector (D58).

Random-generator follow-ups (D56): RNDR on AArch64 and Zkr on RISC-V; timer jitter, UEFI
and TPM seeds (M13); virtio-rng and periodic reseeding (M8); `/dev/random` and `/dev/urandom`
(M7); `random.trust_cpu` and `random.trust_bootloader`; vmgenid reseeding; a vDSO `getrandom`;
`EINTR` with signals; `dc zva` and Zicboz for zeroing; the seeded state as an MCP and CLI
capability (D22); ChaCha20's working state wiped within the benchmark gate (BLAKE2s's: done,
D58).

Thread follow-ups (D35): guard pages below thread stacks in a kernel virtual area, with a per-CPU
cache of free stacks, once exception stacks exist; a self-test measuring stack high-water to
confirm 16 KiB on every architecture (interrupt frames now land on thread stacks at
preemption points); revisit the stack size when FP/SIMD state or nested interrupts add frames.

Preemption follow-ups (D37): (involuntary preemption on return to user mode: done, D48);
(preemption on a secondary CPU exercised by a boot self-test: done, D44; per-CPU tick state and
run queues: done, D43); a measured worst-case gap between
preemption points as a latency benchmark; a debug-build check that no lock is held at a
preemption point, nor an address space or user-access window open (D46: an address space made
active over another on one CPU is already caught; RISC-V's `sstatus.SUM` is per CPU, not per thread).

Sleep and wait-queue follow-ups (D39): waits with a timeout (a timed park: done, D89, `thread::Parker`; wait queues themselves still wait without one); a condition-variable wait for data
behind a `SpinLock` and a futex-style wait (M6); wait-queue locks of their own (per-CPU idle
threads: done, D43); waking threads from device interrupt handlers (M8); RISC-V Sstc, arming the timer
without an SBI call; per-thread CPU time from clock reads at each switch; a pairing heap or
timer wheel for the sleepers if a CPU ever has many.

SMP follow-ups (D40, D41, D43, D44): per-CPU run-queue locks and wait-queue locks instead of the
one scheduler lock, once a multi-CPU workload or a benchmark outside `-icount` can show the
contention (D45: a lock-free local run queue with per-CPU wake lists is the intended design); an idle-CPU mask for placement and balancing; wake-time placement onto idle
CPUs, balancing with preemption off, CPU affinity, and balancing by load, cache and NUMA distance
(D44); canaried idle stacks on the secondaries (load balancing: done, D44; per-CPU run queues, idle
threads, waking an idle CPU with `cpu::wake`, and a secondary waking the boot CPU: done, D43;
per-CPU interrupt setup and timer interrupt counts: done, D41); GICv3 SGIs and more than 8 CPUs on
aarch64; TLB shootdown when live tables change; x2APIC for APIC ids of 255 and up;
an invariant-TSC check, since threads now move between CPUs (D44) and may read two CPUs' clocks; booting from a CPU whose affinity is
not 0 on aarch64, the PSCI conduit from the device tree and a full known `SCTLR_EL1` (M13); a
console lock for messages from any CPU; starting the CPUs in `boot`, not in a self-test.

Heap and buddy follow-ups (D30, D31): returning empty slabs to the frame allocator (a live-block
count per frame); per-CPU caches instead of one heap lock and a high watermark on the free stack
(M4); a physical address limit for 32-bit DMA (M8); one buddy bitmap per range; anti-fragmentation
grouping.

Comment cleanup (D66): code keeps only `// SAFETY:`, lint directives and rustdoc on public
items, which `cargo xtask docs` checks in CI (done, PR #59). Done: drivers; xtask's `bench`,
`elf` and `qemu`; the Cargo, CI, Dependabot and funding files, whose notes are now in
`docs/build.md`; the kernel's `linux.rs`, whose test comments only repeated their assertions
or what D50–D75 record (PR #71, after PR #70 touched it); the kernel's `linux/memory.rs`
(PR #78); `rust-toolchain.toml` and the CI image's `Dockerfile` and `ci-image.yml`, whose notes
are in `docs/build.md` (PR #79). Next: the HAL, then the kernel's other modules, a few hundred
lines per PR. Last: the other files PR #47 changed (`xtask/src/main.rs` among them).

Setup follow-ups from the 2026-10-22 review (all done: exception vectors in D25, the benchmark harness
in D27):
- **Benchmarks: done** (D27). `cargo xtask bench` boots each architecture under QEMU `-icount`
  and fails on a regression against `benchmarks/baseline.txt`; CI runs it. Every new hot path
  (frame allocator, page faults, scheduler, syscalls) adds its benchmark there. Cycle-accurate
  numbers on real hardware come with M13.
- **A BSS self-test that can fail: done.** The QEMU boot tests look the kernel's `.bss` probe
  up in its ELF symbol table and fill it with a non-zero pattern through QEMU's `loader`
  device before the kernel starts, so a missing zeroing loop fails `bss_is_zeroed` (checked on
  aarch64, riscv64 and x86_64 by removing the loop).

UX track (runs alongside, from the start): `docs/ux/` — **in progress** (switcher-habit research).

## Support matrix

### CPU architectures (QEMU boot test in CI is the bar for "done")
| Architecture | Status | Notes |
|--------------|--------|-------|
| aarch64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (PSCI `CPU_ON`) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt`, EL1, PSCI power-off and reset (D9, D10, D106) |
| riscv64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary harts start (SBI HSM) and take their own timer and wake-up interrupts, threads on every hart, user address spaces, user mode with system calls, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `virt` + OpenSBI, S-mode, SBI SRST power-off and reset, legacy SBI shutdown fallback (D9, D106) |
| x86_64 | **done** (boots, prints, self-tests, exception vectors, timer interrupts, memory map and frame allocator, own page tables, high-half kernel, kernel heap, kernel threads, time-slice preemption, sleeping, wait queues, tickless idle, secondary CPUs start (INIT-SIPI) and take their own timer and wake-up interrupts, threads on every CPU, user address spaces, user mode with system calls, per-CPU GDT and TSS, processes, first Linux system calls (`write`, `exit`, D50), Linux programs started from ELF images (D51), demand paging (D52), the memory calls (D53), FP/SIMD registers per thread (D54), the first startup calls (`writev`, `getpid`, `uname`, ..., D55), the random number generator and `getrandom` (D56)) | QEMU `q35`, PVH entry → long mode, COM1 over port I/O, ACPI S5 power-off, reset through the reset control register or the 8042 (D15, D106) |
| loongarch64, ppc64, s390x, mips64, arm (32-bit), riscv32, sparc64 | planned | M9 |

### GPUs
| GPU | Status |
|-----|--------|
| virtio-gpu | planned (M11) |
| Intel, AMD, NVIDIA, Apple, Arm Mali, Qualcomm Adreno | planned (M13) |

### Device classes
| Class | Status |
|-------|--------|
| Server, desktop, laptop (QEMU first) | planned |
| Phone, tablet | planned |
| Embedded | planned |

### Drivers
| Driver | Status |
|--------|--------|
| PL011 UART (Arm), transmit and interrupt-driven receive | **done** |
| ACPI tables: RSDP/RSDT/XSDT/FADT, `\_S5` from the DSDT (power-off), MADT local APICs (CPU discovery), I/O APICs and interrupt source overrides | **done** (full AML interpreter: planned, M5) |
| Flattened device tree: memory nodes and reservations, `/cpus`, `/chosen` seeds (taken and removed, D56), a hart's PLIC context | **done** (device discovery: planned, M5) |
| NS16550 UART (MMIO, any `reg-shift`, and x86 port I/O), transmit and interrupt-driven receive | **done** |
| Arm GICv2 (distributor + CPU interface, per-CPU setup, SGIs between CPUs, SPIs routed with their trigger mode) | **done** (GICv3: planned, M5) |
| RISC-V PLIC (priorities, per-context enables, claim/complete; the hart's context from the device tree) | **done** (AIA: planned, M5) |
| Arm generic timer (EL1 virtual timer) | **done** |
| RISC-V SBI timer and IPI | **done** |
| x86 local APIC (xAPIC), on every CPU: its timer, INIT/start-up IPIs and fixed IPIs; 8259 PICs (remapped and masked); 8254 PIT (frequency measurement) | **done** (x2APIC, HPET: planned) |
| x86 I/O APIC (lines routed by the MADT's overrides) | **done** (MSI-X: planned, M5) |
| virtio (console, block, net, rng, input, gpu) | planned (M8, M11) |

## Toward "better than macOS, Windows and Linux"

Rough estimate: **~1%**. The kernel boots and prints on three architectures (x86_64, aarch64, riscv64); nothing a user can run yet.

| Area | Missing |
|------|---------|
| Kernel | per-CPU scheduler locks, the Linux syscalls (processes run their programs in user mode on every CPU, preempted on return to user mode, and make system calls; user address spaces exist; CPU exceptions are caught and reported; timer interrupts work; physical frames are allocated; the kernel runs on its own page tables in the top 2 GiB with W^X, a direct map and an empty low half; it has a heap and contiguous frame allocation; kernel threads switch on yield and, with time slices, at preemption points; they sleep and wait in wait queues, and an idle CPU takes no tick; every CPU starts, has its per-CPU block, takes its own timer and wake-up interrupts and runs threads, and idle CPUs take threads from busy ones) |
| Hardware | the other architectures; every driver beyond the early UARTs |
| Compatibility | Linux ABI (static ELF programs load and start with Linux's stack; `write`, `writev`, `exit`, `brk`, `mmap`, `munmap`, `mprotect`, `getpid`, `gettid`, `uname`, `set_tid_address`, `set_robust_list`, `getrandom`, `sched_yield`, `arch_prctl`, the resource limits and the signal state's calls, `poll`, `ppoll` and the console's `ioctl`s work, with `AT_RANDOM` from the kernel's generator; pages are mapped on first touch and the stack grows on demand; each thread keeps its own thread pointer; signals are sent and delivered to handlers on Linux's frames, faults included, real-time signals are queued with their `siginfo_t`, and `rt_sigsuspend` and `pause` wait for a handler; the clocks read and programs sleep; the console's status is a character device and it cannot seek; `futex` waits and wakes; `clone` starts threads, Rust's `std::thread` runs, each on its process's one CPU until TLB shootdown; FP state per thread; signals between a process's threads and to the process sent and delivered, interrupted futex waits and sleeps still missing; new processes and files still missing), Android, Windows, macOS and iOS layers |
| Desktop / UX | compositor, shell, apps; research started in `docs/ux/` |
| AI-native | built-in MCP server, app capability registry, agent permissions |
| Editions | component manager; packaging and policy per edition |

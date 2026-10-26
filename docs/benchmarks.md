# Benchmarks

What some of the boot-time benchmarks in `kernel/src/bench.rs` measure, and why they are built
the way they are. `cargo xtask bench` runs every benchmark under QEMU `-icount shift=0`, so each
result is an instruction count per iteration (D27), gated against `benchmarks/baseline.txt`.

## Timers

- `timer_arm`: arms the one-shot timer for a deadline far ahead, so it never fires during the
  run. The deadline is clamped so `far + i` cannot overflow, which keeps the loop body a plain
  add.
- `timer_interrupt_round_trip`: arms the timer for a deadline already passed and sleeps until
  its interrupt has been handled: interrupt entry, acknowledgement, delivery to the kernel and
  the return (D26). The CPU's block is found once, so polling its count costs what polling a
  global did. The word of what the scheduler has not seen is read whole: nothing in the loop
  runs the scheduler and no other CPU asks it to look (the benchmarks run on one CPU), so it
  only grows with the timer interrupts.

## Linux layer

- `linux_page_fault`: a page fault on a page the program touches for the first time (D52): the
  trap from user mode, the Linux layer's dispatch, the region looked up, a frame allocated,
  zeroed and mapped, the CPU's stale translation dropped, and the return to the program, which
  stores to the page and goes on to the next. The test program's `stack_growth` walks down a
  region of `PAGE_FAULT_CHUNK` (512) pages, its stack pointer starting just below the top so
  its first store is to the region's last page, until it runs off the end and is killed.
  Starting each process and its thread, the final fault and freeing it all are timed too,
  amortized over the chunk: a cost paid once per process (such as its descriptor table, D101)
  shows up here divided by 512.
- `linux_dup3_close`: `dup3` of standard output to descriptor 3, close-on-exec, then `close`
  of it (D101): the open-file limit read, the descriptor table locked twice, the open file
  description's count taken and dropped, as a shell redirects a command's output. Without the
  trap from user mode (`linux_syscall_round_trip` has it).
- `linux_descriptor_lookup`: what every call on a descriptor (`read`, `write`, `ioctl`,
  `lseek`, `fstat`, `poll`, ...) pays to find what the descriptor names (D101): the table's
  lock taken and released, and the open file description's count taken and dropped.
- `linux_openat_close`: `openat(AT_FDCWD, "/etc/hostname", O_RDONLY)` then `close` (D102):
  the path copied from the program, two components looked up in the embedded initramfs
  (binary searches in its index), an open file description allocated, a descriptor taken and
  closed again. Without the trap from user mode.
- `linux_file_read`: `pread64` of `/etc/motd`'s 18 bytes from offset 0 into the program's
  memory (D102): the descriptor looked up and its kind matched, the bytes copied straight from
  the archive in the kernel image, no allocation. Without the trap from user mode.
- `linux_execve`: `execve` of a small static program from the initramfs, per call (D103): the
  runtime's self-test program (`/usr/libexec/eggos-selftest`, a static PIE), started with
  `exec-loop <n>`, runs its start-up and execs itself with one less until 0, so each iteration
  is one `execve` from user mode (the path and arguments copied, the file found, a new process
  with the program loaded, the old one freed, the close-on-exec descriptors closed) and the
  program's start-up up to its next `execve` (relocation, `PT_GNU_RELRO`). The first process's
  load and its exit are spread over the iterations. Only test kernels hold the program; the
  benchmark measures nothing elsewhere.
- `linux_fork`: a process creation with an eager copy and the child's end, per call (D104):
  the runtime's self-test program, started with `fork-loop <n>`, calls
  `clone(CLONE_VFORK | SIGCHLD)` `n` times; each child (a copy of every page the program has
  mapped, its regions, descriptors, credentials and signal actions, and a thread) calls
  `exit_group` at once, and the parent goes on when it has ended. The children's exit records
  stay with the parent until it ends (no `wait4` yet). Only test kernels hold the program.
- `linux_vfork_exec`: `posix_spawn`'s path, per call (D104): the same program, started with
  `spawn-loop <n>`, calls `clone(CLONE_VM | CLONE_VFORK | SIGCHLD)` `n` times; each child,
  on the parent's memory, `execve`s the program with `exec-loop 0`, which runs its start-up
  and exits, and the parent goes on at the child's point of no return. Includes the children's
  start-ups and exits, the last ones waited for after the parent ends.

## Host tests of the benchmarks

The benchmarks' host tests run on a counting platform: its clock advances one tick per read,
its timer fires on the next wait whatever its deadline, and it has one CPU. In user mode, the
test program's `stack_growth` stores to each page from the stack pointer's down, each faulting
the first time; the stack pointer stands in for the walk, and the kernel resolves the fault and
runs the program again. Any other program makes the null system call, and nothing else.

- `prints_one_parsable_line_per_benchmark` makes the page tables for `run` first: the
  benchmarks run there must find the frame allocator as the other tests leave it, so the
  benchmarks that take frames, from the user-mode ones on, have a test of their own.
- `the_user_mode_benchmarks_run_their_programs_and_free_them` holds the timer, since the
  benchmarks' threads pass preemption points where the platform's timer fires. Its page tables
  come from the same allocator as the processes' (not `page_tables_for_test`, whose allocator
  may be gone once it returns). The idle thread is started as the other tests start it, since
  an idle thread of the counting platform would take its timer with it, and its stack, taken
  once, is not counted against the benchmarks. `linux_page_fault` maps fewer pages there than
  the test allocator has frames, so none runs out.

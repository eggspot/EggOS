# Benchmarks

What some of the boot-time benchmarks in `kernel/src/bench.rs` measure, and why they are built
the way they are. `cargo xtask bench` runs every benchmark under QEMU `-icount shift=0`, so each
result is an instruction count per iteration (D27), compared with `benchmarks/baseline.txt`: gated
exactly by default (what `cargo xtask ci` and the weekly performance pass run), report-only with
`--report-only` (CI's mode: every change reported, a result more than 2x slower fails, D122).

## Kernel core

- `empty_loop`: the loop alone, so the per-iteration overhead every other result includes is
  visible.
- `linux_clock_monotonic_read` and `linux_clock_realtime_read`: what
  `clock_gettime(CLOCK_MONOTONIC)` and every other clock read cost beyond the system-call round
  trip (D73): the clock read and its conversion to nanoseconds.
- `random_fill_16` and `random_fill_4096`: `N` bytes from the random number generator (D56): with
  16, one draw, what `AT_RANDOM` costs each program; with 4096, a stream from a draw's key,
  `getrandom`'s long path without the copy to the program.
- `breakpoint_round_trip`: exception entry, the kernel's handler and the return: the path syscalls
  and page faults will take (D25).
- `frame_allocate_untouched`: allocating never-used frames, the path every frame takes first.
  Untouched frames are not written, so the range can be any addresses. Twice the frames allocated,
  so the buddy allocator's bitmap (taken from the top, never written here) leaves `n` untouched
  frames.
- `frame_free_allocate`: freeing a frame and allocating it again: the free stack's push and pop,
  the steady-state path once memory has been recycled. The frame is a page of this function's
  stack, which the allocator reaches at its own address (offset 0).
- `frame_contiguous_allocate_free`: allocating and freeing a block of 4 contiguous frames from the
  kernel's frame allocator: the buddy allocator's split on allocation and merge on free (D31), the
  path of heap allocations larger than a frame. Runs after the self-tests, so the allocator is in
  place; without one, nothing is timed.
- `heap_allocate_free_small`: allocating and freeing a small block through the kernel's global
  heap: taking and releasing its lock around a size class's free-list pop and push, the steady
  state of `Box` and small `Vec`s. Runs after the self-tests, so the frame allocator is in place;
  without one every allocation fails and only the failure path is timed.
- `thread_yield_round_trip`: the boot thread and one other passing the CPU back and forth through
  `yield_now`: per iteration two switches (D35), each with the scheduler's queue rotation and a
  stack canary check. Includes starting and joining the thread once per run, a few hundred
  instructions spread over the iterations. Without a frame allocator for the stack, nothing is
  timed.
- `preemption_point_idle`: a preemption point in a time slice that is not over: the window for a
  pending interrupt and the check, which long-running kernel code pays per call (D37).
- `wait_queue_wake_empty`: waking a wait queue nobody waits in: what code that changes a condition
  pays when no thread waits for it (D39).
- `thread_wait_wake_round_trip`: the boot thread and one other handing a turn back and forth
  through two wait queues: per iteration each thread wakes the other and blocks until its turn
  comes, so two wakes, two condition checks that fail and two that hold, and two switches (D39).
  Includes starting and joining the thread once per run, as in `thread_yield_round_trip`.
- `syscall_round_trip`: a null system call from user mode and back (D47): the HAL's test program
  makes system call 2 in a loop; each iteration is its `svc`/`ecall`/`syscall` into the kernel, the
  kernel looking at why, and the return to the program. The program's address space stays active
  throughout, as for a process making system calls. Setting the program up and freeing it is timed
  too, amortized over the iterations.
- `process_syscall_round_trip`: a null system call from a process's own thread (D48):
  `syscall_round_trip` plus what the thread's loop adds per call, its preemption point, the check
  that the address space is loaded and the call to its handler. Includes starting the process and
  its thread and joining it once per run.
- `linux_syscall_round_trip`: an unknown Linux system call from a Linux program's thread (M6):
  `process_syscall_round_trip` with the Linux layer's handler, whose dispatch answers `ENOSYS`, its
  cheapest path, as the thread's handler. Includes starting the process and its thread and joining
  it once per run.
- `linux_exec_load`: starting a Linux program from its ELF image (D51): a new process, the
  self-test's image with only the code `linux_start` needs (D62) loaded into it with the
  self-test's arguments and environment (its code, data and `.bss` mapped, its stack's region
  recorded and its arguments' page mapped and laid out), then the process freed. Each page is a
  frame of its own, zeroed: 1 for the stack (the rest of it is mapped as the program touches it,
  D52), 6 for the program, and the tables, so the frames dominate.
- `linux_mmap_munmap`: `mmap` of 16 anonymous pages and `munmap` of them again (D53), as a
  program's allocator reserves and returns memory: the arguments checked, the region placed
  top-down, inserted and removed, the tables walked for pages to unmap (none: they were never
  touched), this CPU's translations of the range dropped (the address space is active, as for the
  program's own call) and the other CPUs' records looked at. Without the trap from user mode
  (`linux_syscall_round_trip` has it).
- `process_switch_round_trip`: two processes' threads taking turns on one CPU, each yielding at
  every null system call (D48): per iteration two system calls, two thread switches and two
  address-space switches, with what each switch costs on the CPU afterwards (the user translations
  it drops). Compare with `process_syscall_round_trip` and `thread_yield_round_trip`. Includes
  starting and joining both once per run.
- `fp_process_switch_round_trip`: `process_switch_round_trip` with programs that use their FP/SIMD
  registers (D54), each checking every one after each call (`TestProgram::fp_registers`): per
  iteration also two saves and two loads of FP/SIMD registers, and the program's checks. Compare
  with `process_switch_round_trip`.

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
- `linux_unix_stream_send_recv`: 64 bytes sent and received on a socket pair (D138): the
  connection's lock taken twice, the bytes queued and taken back, no allocation once the queue
  has grown. Without the trap from user mode or the copies from and to the program.
- `linux_memfd_map_unmap`: `mmap` of 16 pages of a memfd `MAP_SHARED`, read-write, then
  `munmap` (D139): the descriptor looked up, the file's frames (allocated once, before the
  loop) taken under its lock, a region added, 16 pages mapped at once and the stale
  translations dropped, then the range unmapped without freeing the frames and the mapping's
  hold on the file let go. Compare with `linux_mmap_munmap`, whose pages are mapped on first
  touch. Without the trap from user mode.
- `linux_file_map_read_unmap`: a private `mmap` of 16 pages of a file in the initramfs (the
  runtime's self-test program), each page then read once as a kernel copy reads it, then
  `munmap` (D167): the descriptor looked up, a region and a file range added, then per page a
  frame taken, the file's 4 KiB copied in from the archive and the page mapped (a file page's
  fault without the trap), then the range unmapped and the 16 frames freed. What a dynamic
  loader's library pages cost. Without the trap from user mode.
- `linux_execve`: `execve` of a small static program from the initramfs, per call (D103): the
  runtime's self-test program (`/usr/libexec/eggos-selftest`, a static PIE), started with
  `exec-loop <n>`, runs its start-up and execs itself with one less until 0, so each iteration
  is one `execve` from user mode (the path and arguments copied, the file found, a new process
  with the program loaded, the old one freed, the close-on-exec descriptors closed) and the
  program's start-up up to its next `execve` (relocation, `PT_GNU_RELRO`). The first process's
  load and its exit are spread over the iterations. Only test kernels hold the program; the
  benchmark measures nothing elsewhere.
- `linux_fork`: a process creation copy on write and the child's end, per call (D104, D169):
  the runtime's self-test program, started with `fork-loop <n>`, calls
  `clone(CLONE_VFORK | SIGCHLD)` `n` times; each child (every page the program has mapped
  shared read-only with it, its regions, descriptors, credentials and signal actions, and a
  thread) calls `exit_group` at once, and the parent goes on when it has ended, taking back
  the pages it then writes (its stack) with one fault each. The children's exit records stay
  with the parent until it ends (no `wait4` yet). Only test kernels hold the program.
- `linux_fork_touch`: `linux_fork` with one write in each child (D169): started with
  `fork-touch-loop <n>`, each child adds one to a word of the program's data before it ends,
  so a page the parent maps is copied on write; the difference from `linux_fork` is the price
  of one copy-on-write fault.
- `linux_vfork_exec`: `posix_spawn`'s path, per call (D104): the same program, started with
  `spawn-loop <n>`, calls `clone(CLONE_VM | CLONE_VFORK | SIGCHLD)` `n` times; each child,
  on the parent's memory, `execve`s the program with `exec-loop 0`, which runs its start-up
  and exits, and the parent goes on at the child's point of no return. Includes the children's
  start-ups and exits, the last ones waited for after the parent ends.
- `linux_epoll_wake`: an event loop's wake-up, per call (D169): one `eventfd` add that makes a
  level-triggered `epoll` item ready (its wake list queues it on the instance's ready list), one
  `epoll_wait` that finds it without sleeping, and the read that takes the count, through the
  Linux layer's functions on a test program's descriptors, without the trap.

## Drivers

None of these allocates or touches a device, and they run before the user-mode benchmarks, so
the frame allocator and the heap are as the other benchmarks expect (D111).

- `virtqueue_submit_one`: what a virtio driver pays per request before the doorbell: a
  one-buffer chain made available on the split virtqueue (a descriptor from the free list,
  the ring slot, a release fence, the published index) and the notification check (a full
  fence and the used ring's flag). The queue has 16384 entries in a static of the benchmark
  kernel, more than the iterations, so nothing is ever completed; a full queue would be made
  again (O(1)).
- `virtqueue_round_trip_fake`: virtio-rng's read (D111) with a device that takes no time: the
  driver submits a 32-byte device-writable buffer and checks for a notification, the fake
  device (`virtio::queue::DeviceSide`) takes the chain, fills the bytes and returns it, the
  driver pops it (freeing the chain) and copies the bytes out. Without the doorbell's MMIO
  write and the device's own time, which an emulated device makes host-dependent.
- `virtio_gpu_flush_fake`: the virtio GPU driver's flush of one damaged 16x16 rectangle (D112)
  with a device that takes no time: the fenced `TRANSFER_TO_HOST_2D` written and posted, the
  notification check and the fake device's answer, the wait, the answer checked; then the
  same for `RESOURCE_FLUSH`. The driver's own cost per frame and rectangle, without the
  doorbells' MMIO writes and the device's copy, which an emulated device makes
  host-dependent. Its 4 KiB control region and 64x64 framebuffer are statics of the benchmark
  kernel.
- `gfx_glyph_cell`: one Egg Mono glyph drawn into an 8x16 cell of an 80-column framebuffer
  (D114): 16 rows of 8 pixels, each the foreground or the background colour.
- `gfx_terminal_line`: the terminal engine's render of one changed line of 80 cells (D114):
  80 glyphs and the line's damage rectangle.
- `term_feed_line`: the terminal engine's parser and cells for one 80-column shell line with
  SGR colours, ending in a carriage return (D114): about 120 bytes, so the parser's cost per byte
  is this divided by 120.
- `term_scroll_line`: a line feed at the bottom of an 80x24 terminal and its render (D114): the
  grid moved up a line into the scrollback, the pixels of 23 lines moved up instead of drawn
  again, the new line and the cursor drawn.
- `term_reflow`: a resize of a terminal with a full scrollback between 80x24 and 100x30 (D222):
  every logical line of the 256 scrollback rows and the screen laid out again at the new width,
  the cursor kept on its cell; one resize per iteration, alternating the two sizes.
- `gfx_compose_four_windows`: a 64x64 rectangle of a 1280x800 desktop where the terminal and
  four cascaded 658x430 windows overlap, composed from scratch (D127): the wallpaper, each
  window's shadow, chrome and content, the bar. The design's full-screen budget (four windows,
  10 ms) is about 250 of these.
- `window_commit_line`: a commit of one 656x16 band of the top window's content (D127): the
  damage it makes and the band copied from the window's buffer to the framebuffer, through the
  compositor's fast path.
- `wayland_blit_rows`: a 656x16 band (the size `window_commit_line` commits) copied from a
  Wayland client's pixels into a window's buffer with each pixel's alpha byte cleared (D140):
  the work `eggway` adds per damaged band before the window server's own commit. eggway reads
  the client's memory with volatile loads; the benchmark runs the same loop over slices. The
  kernel targets have no FP/SIMD, so this is scalar code, one load, mask and store per pixel.
- `window_hit_test`: the pointer moving between two points over eight overlapping windows
  (D127): the top window under it found, and its enter, leave and motion events made; since
  D128 in the same walk its resize zones and the control under the pointer, then the pointer's
  shape chosen and its old and new rectangles damaged.
- `toolkit_table_layout_1000_rows` and `text_area_wrap_64k` (D218): the toolkit's table of 1 000
  rows laid out and drawn into a 400x300 window, and a 64 KiB text area wrapped at 384 px. They
  link the userland toolkit into the kernel only in bench builds (`qemu-bench` and host tests);
  a release kernel does not depend on it.
- `device_tree_walk`: every node of QEMU 10.2's aarch64 `virt` device tree (the host tests'
  fixture, embedded in benchmark kernels only, so all three architectures walk the same 63
  nodes): the header checked, the token stream read, each node's context and the properties
  discovery uses decoded, and the enabled nodes with a `compatible` counted. About what the
  device-tree platform bus pays per walk at boot (it walks twice, D110).

## Translation

- `translate_block_16`: the x86-64 translator (D191) decoding, forming and lowering one block
  of fifteen register `add`, `sub` and `xor` instructions and a `jnz` back, with every flag
  live and atomics on (FEAT_LSE), per iteration: a sixteenth of it is the cost of translating
  one instruction. It measures the translator on every architecture, not translated code;
  translated programs' own run-time benchmarks are reported apart from native ones.
- `translate_install_bytes` (aarch64 only, the one architecture that translates; a size, gated
  exactly like `release_image_bytes`: any growth fails): the bytes of the translation cache
  file per byte of x86_64 code, for a fixed image of 256 small loop functions (`add`, `xor`,
  `dec`, `jnz` back, `ret`) translated by `cargo xtask bench` on the host as `app install`
  translates on aarch64 (D191, slice 39e2d). It shows what installing a translated program
  costs on the disk; it appears as a "no baseline" line until the baseline is updated.
- `translated_loop_ratio` (planned, D191's "Not yet"): a fixed x86_64 loop translated and run
  on the CPU against the same loop as native aarch64 code, run by a user program on aarch64
  through the shared translation loader; the ratio is the translated code's cost against
  native, reported apart from native benchmarks.

## Image decoding

- `image_decode_jpeg_640x480` (D230): `eggos-image` decoding a 640x480 baseline JPEG photo with
  4:2:0 chroma (`image/testdata/photo-640x480.jpg`, embedded in benchmark kernels only) to
  straight-alpha pixels, per iteration: Huffman decoding, the integer IDCT, chroma upsampling
  and the colour conversion, with the image's buffers allocated and freed. One iteration is a
  whole decode, so the runner times it over 20 iterations after one untimed one
  (`LONG_BENCHMARKS` in `kernel/src/bench.rs`) instead of 10 000 after 100: under `-icount` the
  count is exact either way. Like the toolkit benchmarks, the crate is linked into the kernel
  only in bench builds; the Windows layer (and the macOS layer's AppKit) link it as part of
  their optional components.

## Release kernel

Two results per architecture come from the release kernel rather than the benchmark kernel
(D116):
- `boot_to_prompt`: guest time from the start of the kernel's `boot` to the shell waiting for
  input at its first prompt, read by a release kernel built with `qemu-boot-clock` and booted
  under the same `-icount` on one CPU, serial only (no display or input devices, whose polled
  virtio completions arrive at host-dependent times under `-icount`). Firmware runs before
  the clock starts, so the result does not depend on QEMU's firmware build. Under `-icount
  sleep=off` an idle CPU jumps to its next timer, so the time is instructions plus the timer
  deadlines the boot waits for. The wall-clock time with the display, the number a user
  feels, is printed by the boot-to-prompt test and recorded in the release notes, ungated.
  The count depends on QEMU's version (x86_64 is 20% apart between Ubuntu 24.04's QEMU 8.2.2
  and QEMU 10.2.2), so it is gated only on the version of the baseline's `<arch> qemu` line,
  CI's.
- `boot_to_desktop` (D221): guest time from the start of `boot` to the frame that ends the
  boot screen and shows the desktop, read by the same `qemu-boot-clock` kernel booted a second
  time under `-icount`, this time with the virtio GPU; QEMU-dependent like `boot_to_prompt`.
  The GPU is polled, so its completions may arrive at host-dependent times: the result is kept
  only while repeated runs on one machine give the same number. The serial console of every
  kernel also says `boot: desktop <ms>`, the wall-clock milliseconds to the desktop being made,
  just before `boot: terminal`.
- `release_image_bytes`: the size in bytes of the image `cargo xtask release` ships. Gated
  exactly: any growth fails until the baseline line is updated with the reason (CLAUDE.md,
  note 8). Since D237 the image is the base alone: the Windows and macOS layers are
  components in the store, and the base holds only links to where they install, so this is
  the base's size with no component installed (D24).
- `idle_memory_bytes` (D203): the memory in use when the shell first waits at its prompt,
  from the same boot. The release kernel boots with no component installed, so this is the
  base's idle memory (D24, D237). Gated exactly like `release_image_bytes`.
- `components_store_bytes` (D237): the size in bytes of the component store `cargo xtask
  release` ships beside the kernel image, `eggos-<version>-<arch>-components.img`, as
  `components::build_store` writes it (every component the architecture offers, padded to
  whole 4 KiB blocks; riscv64's holds the header and an empty index). Gated exactly: a
  component that grows is a size regression like the base's.

## The runner (`xtask/src/bench.rs`)

- `MARKER` (`EGGOS-BENCH:`) must match `eggos_kernel::bench::MARKER`, and `BOOT_TO_DESKTOP`
  must match `eggos_kernel::boot_clock::DESKTOP`, the release kernel's first desktop frame
  (D221), measured in a boot with a display.
- Benchmarks run on one CPU (`BENCH_CPUS`): under `-icount`, QEMU runs every CPU in one thread
  in turn, so other CPUs' instructions would count against the measured ones (D40).
- A result fails the exact gate when it is slower than its baseline by more than 1% plus
  0.5 ns: the absolute allowance keeps results of a few instructions from failing by one
  instruction. The results in `SIZES` (`release_image_bytes`, `idle_memory_bytes`,
  `translate_install_bytes`, `components_store_bytes`) are gated exactly: any growth fails.
- `FIRMWARE_DEPENDENT` lists the benchmarks that execute firmware code (the SBI implementation
  on riscv64: `timer_arm`, `timer_interrupt_round_trip`), whose counts depend on the firmware
  build QEMU loads. They are compared only when the firmware's banner version (`1.3` from
  `OpenSBI v1.3`) matches the baseline's `<arch> firmware <version>` line, and
  `--update-baseline` keeps their baseline values (with the firmware line) when they were
  measured on other firmware, so a machine whose QEMU ships another firmware build cannot move
  CI's numbers; it prints a note per kept value.
- `QEMU_DEPENDENT` lists the benchmarks whose counts depend on the QEMU version, which builds
  each machine's devices and tables (`boot_to_prompt`, `boot_to_desktop`): compared only when
  QEMU's version matches the baseline's `<arch> qemu <version>` line.
- `--update-baseline` rewrites `benchmarks/baseline.txt` with its header, replacing only the
  architectures that ran. A comparison writes `target/bench/report.md` (a table of every
  result against its baseline) and `report.json`, and lists one failure per regression, new
  benchmark without a baseline, or baseline without a result.

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

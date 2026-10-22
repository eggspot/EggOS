//! Boot-time benchmarks of the kernel's hot paths (kernel feature `qemu-bench`).
//!
//! Each benchmark runs its body a fixed number of times between two reads of the platform
//! clock and prints one [`MARKER`] line. `cargo xtask bench` boots the kernel in QEMU with
//! `-icount shift=0`, where the guest clock advances exactly 1 ns per instruction executed,
//! so the elapsed time is an instruction count: the same on every host and every run, which
//! is what lets CI compare it against a committed baseline (D27).

use crate::HEAP;
use crate::memory::{FRAMES, FrameAllocator, Memory};
use crate::process::{self, Process};
use crate::{cpu, thread};
use alloc::sync::Arc;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::hint::black_box;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::user::UserExit;
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// Starts every benchmark result line. The xtask bench runner parses these lines.
pub const MARKER: &str = "EGGOS-BENCH:";

/// Iterations timed per benchmark, after [`WARMUP_ITERATIONS`] untimed ones (which bring the
/// code into the caches and QEMU's translation-block cache).
pub const ITERATIONS: u64 = 10_000;
const WARMUP_ITERATIONS: u64 = 100;

/// A benchmark: a name and a body that runs the measured operation `n` times, with the
/// kernel's memory ([`Memory`]) for those that need its page tables.
pub type Benchmark<P> = (&'static str, fn(&mut Memory<P>, u64));

/// Every benchmark for platform `P`, in run order.
pub fn all<P: Platform>() -> [Benchmark<P>; 16] {
    [
        ("empty_loop", |_, n| empty_loop(n)),
        ("clock_read", |_, n| clock_read::<P>(n)),
        ("breakpoint_round_trip", |_, n| {
            breakpoint_round_trip::<P>(n)
        }),
        ("timer_arm", |_, n| timer_arm::<P>(n)),
        ("timer_interrupt_round_trip", |_, n| {
            timer_interrupt_round_trip::<P>(n)
        }),
        ("frame_allocate_untouched", |_, n| {
            frame_allocate_untouched(n)
        }),
        ("frame_free_allocate", |_, n| frame_free_allocate(n)),
        ("frame_contiguous_allocate_free", |_, n| {
            frame_contiguous_allocate_free(n)
        }),
        ("heap_allocate_free_small", |_, n| {
            heap_allocate_free_small(n)
        }),
        ("thread_yield_round_trip", |_, n| {
            thread_yield_round_trip::<P>(n)
        }),
        ("preemption_point_idle", |_, n| {
            preemption_point_idle::<P>(n)
        }),
        ("wait_queue_wake_empty", |_, n| {
            wait_queue_wake_empty::<P>(n)
        }),
        ("thread_wait_wake_round_trip", |_, n| {
            thread_wait_wake_round_trip::<P>(n)
        }),
        ("syscall_round_trip", syscall_round_trip::<P>),
        (
            "process_syscall_round_trip",
            process_syscall_round_trip::<P>,
        ),
        ("process_switch_round_trip", process_switch_round_trip::<P>),
    ]
}

/// Runs `benchmarks` and prints one line per benchmark:
/// `EGGOS-BENCH: <name> iterations=<n> ticks=<clock ticks> frequency=<clock Hz>`.
pub fn run<P: Platform, W: Write>(
    console: &mut W,
    benchmarks: &[Benchmark<P>],
    memory: &mut Memory<P>,
) -> core::fmt::Result {
    let frequency = P::clock_frequency();
    for (name, body) in benchmarks {
        body(memory, WARMUP_ITERATIONS);
        let start = P::clock_ticks();
        body(memory, ITERATIONS);
        let ticks = P::clock_ticks().wrapping_sub(start);
        writeln!(
            console,
            "{MARKER} {name} iterations={ITERATIONS} ticks={ticks} frequency={frequency}"
        )?;
    }
    Ok(())
}

/// The loop alone, so the per-iteration overhead every other result includes is visible.
fn empty_loop(n: u64) {
    for i in 0..n {
        black_box(i);
    }
}

fn clock_read<P: Platform>(n: u64) {
    for _ in 0..n {
        black_box(P::clock_ticks());
    }
}

/// Exception entry, the kernel's handler and the return: the path syscalls and page faults
/// will take (D25).
fn breakpoint_round_trip<P: Platform>(n: u64) {
    for _ in 0..n {
        P::breakpoint();
    }
}

/// Arming the one-shot timer for a deadline far ahead, so it never fires during the run.
fn timer_arm<P: Platform>(n: u64) {
    // Clamped so `far + i` cannot overflow, keeping the loop body a plain add.
    let far = P::clock_ticks()
        .saturating_add(P::clock_frequency().saturating_mul(3600))
        .min(u64::MAX - n);
    for i in 0..n {
        P::set_timer_deadline(far + i);
    }
}

/// Arming the timer for a deadline already passed and sleeping until its interrupt has been
/// handled: interrupt entry, acknowledgement, delivery to the kernel and the return (D26).
fn timer_interrupt_round_trip<P: Platform>(n: u64) {
    // This CPU's block, found once, so polling its count costs what polling a global did. The
    // word of what the scheduler has not seen, read whole: nothing here runs the scheduler and
    // no other CPU asks it to look (the benchmarks run on one), so it only grows by the timer
    // interrupts.
    let this = cpu::current::<P>();
    for _ in 0..n {
        let before = this.unseen_by_scheduler();
        P::set_timer_deadline(0);
        while this.unseen_by_scheduler() == before {
            P::wait_for_interrupt();
        }
    }
}

/// Allocating never-used frames, the path every frame takes first. Untouched frames are not
/// written, so the range can be any addresses. Twice the frames allocated, so the buddy
/// allocator's bitmap (taken from the top, never written here) leaves `n` untouched frames.
fn frame_allocate_untouched(n: u64) {
    let mut map = MemoryMap::new();
    map.add_ram(PhysicalRange::from_start_length(
        1 << 40,
        2 * n * FRAME_SIZE,
    ));
    let mut frames = FrameAllocator::new(&map, 0);
    for _ in 0..n {
        black_box(frames.allocate());
    }
}

/// Freeing a frame and allocating it again: the free stack's push and pop, the steady-state
/// path once memory has been recycled. The frame is a page of this function's stack, which
/// the allocator reaches at its own address (offset 0).
fn frame_free_allocate(n: u64) {
    #[repr(C, align(4096))]
    struct Page(core::mem::MaybeUninit<[u8; FRAME_SIZE as usize]>);
    let mut page = Page(core::mem::MaybeUninit::uninit());
    let mut map = MemoryMap::new();
    map.add_ram(PhysicalRange::from_start_length(
        (&raw mut page).expose_provenance() as u64,
        FRAME_SIZE,
    ));
    let mut frames = FrameAllocator::new(&map, 0);
    let Some(mut frame) = frames.allocate() else {
        return;
    };
    for _ in 0..n {
        // SAFETY: `frame` came from `frames` and nothing uses it; it is `page`, which lives
        // until the end of this function, so the allocator may write its link there.
        unsafe { frames.free(black_box(frame)) };
        frame = frames.allocate().unwrap_or(frame);
    }
    black_box(&mut page);
}

/// Allocating and freeing a block of 4 contiguous frames from the kernel's frame allocator: the
/// buddy allocator's split on allocation and merge on free (D31), the path of heap allocations
/// larger than a frame. Runs after the self-tests, so the allocator is in place; without one,
/// nothing is timed.
fn frame_contiguous_allocate_free(n: u64) {
    let mut frames = FRAMES.lock();
    let Some(frames) = frames.as_mut() else {
        return;
    };
    for _ in 0..n {
        if let Some(block) = frames.allocate_contiguous(black_box(2)) {
            // SAFETY: allocated just above with order 2 and not used.
            unsafe { frames.free_contiguous(black_box(block), 2) };
        }
    }
}

/// Allocating and freeing a small block through the kernel's global heap: taking and releasing
/// its lock around a size class's free-list pop and push, the steady state of `Box` and small
/// `Vec`s. Runs after the self-tests, so the frame allocator is in place; without one every
/// allocation fails and only the failure path is timed.
fn heap_allocate_free_small(n: u64) {
    let layout = Layout::new::<[u64; 4]>();
    for _ in 0..n {
        // SAFETY: the layout has a non-zero size.
        let pointer = unsafe { HEAP.alloc(black_box(layout)) };
        if !pointer.is_null() {
            // SAFETY: allocated just above with `layout` and not used.
            unsafe { HEAP.dealloc(black_box(pointer), layout) };
        }
    }
}

/// The boot thread and one other passing the CPU back and forth through `yield_now`: per
/// iteration two switches (D35), each with the scheduler's queue rotation and a stack canary
/// check. Includes starting and joining the thread once per run, a few hundred instructions
/// spread over the iterations. Without a frame allocator for the stack, nothing is timed.
fn thread_yield_round_trip<P: Platform>(n: u64) {
    let Ok(other) = thread::spawn::<P, _>(move || {
        for _ in 0..n {
            thread::yield_now::<P>();
        }
    }) else {
        return;
    };
    for _ in 0..n {
        thread::yield_now::<P>();
    }
    other.join::<P>();
}

/// A preemption point in a time slice that is not over: the window for a pending interrupt
/// and the check, which long-running kernel code pays per call (D37).
fn preemption_point_idle<P: Platform>(n: u64) {
    for _ in 0..n {
        thread::preemption_point::<P>();
    }
}

/// Waking a wait queue nobody waits in: what code that changes a condition pays when no
/// thread waits for it (D39).
fn wait_queue_wake_empty<P: Platform>(n: u64) {
    static QUEUE: thread::WaitQueue = thread::WaitQueue::new();
    for _ in 0..n {
        black_box(black_box(&QUEUE).wake_one::<P>());
    }
}

/// The boot thread and one other handing a turn back and forth through two wait queues: per
/// iteration each thread wakes the other and blocks until its turn comes, so two wakes, two
/// condition checks that fail and two that hold, and two switches (D39). Includes starting and
/// joining the thread once per run, as in `thread_yield_round_trip`.
fn thread_wait_wake_round_trip<P: Platform>(n: u64) {
    static TURN: AtomicU64 = AtomicU64::new(0);
    static BOOT: thread::WaitQueue = thread::WaitQueue::new();
    static OTHER: thread::WaitQueue = thread::WaitQueue::new();
    TURN.store(0, Ordering::Relaxed);
    let Ok(other) = thread::spawn::<P, _>(move || {
        for turn in 0..n {
            OTHER.wait_until::<P>(|| TURN.load(Ordering::Relaxed) == 2 * turn + 1);
            TURN.store(2 * turn + 2, Ordering::Relaxed);
            BOOT.wake_one::<P>();
        }
    }) else {
        return;
    };
    for turn in 0..n {
        TURN.store(2 * turn + 1, Ordering::Relaxed);
        OTHER.wake_one::<P>();
        BOOT.wait_until::<P>(|| TURN.load(Ordering::Relaxed) == 2 * turn + 2);
    }
    other.join::<P>();
}

/// A null system call from user mode and back (D47): the HAL's test program makes system call
/// 2 in a loop; each iteration is its `svc`/`ecall`/`syscall` into the kernel, the kernel
/// looking at why, and the return to the program. The program's address space stays active
/// throughout, as for a process making system calls. Setting the program up and freeing it is
/// timed too, amortized over the iterations.
fn syscall_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    // SAFETY: `memory.page_tables` are the kernel's tables, active on this CPU, the only one
    // the benchmarks run on.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let mut context = Process::<P>::test_program_context(|program| program.null_syscalls);
    program.with_active(|| {
        let mut calls = 0;
        while calls < n {
            // SAFETY: interrupts are masked, as the kernel runs; the program's address space is
            // active, sharing the kernel's half; nothing here switches threads or address
            // spaces before `run_user` returns. An interrupt in between is not counted.
            if unsafe { P::run_user(&mut context) } == UserExit::Syscall {
                calls += 1;
            }
        }
    });
}

/// A null system call from a process's own thread (D48): `syscall_round_trip` plus what the
/// thread's loop adds per call, its preemption point, the check that the address space is
/// loaded and the call to its handler. Includes starting the process and its thread and
/// joining it once per run.
fn process_syscall_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    // SAFETY: `memory.page_tables` are the kernel's tables, active on this CPU, the only one
    // the benchmarks run on.
    let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
        .expect("the test program fits in memory");
    let context = Process::<P>::test_program_context(|program| program.null_syscalls);
    let mut calls = 0;
    let Ok(thread) = process::spawn::<P, _>(Arc::new(program), context, move |_, _, exit| {
        if exit == UserExit::Syscall {
            calls += 1;
        }
        if calls < n {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    }) else {
        return;
    };
    thread.join::<P>();
}

/// Two processes' threads taking turns on one CPU, each yielding at every null system call
/// (D48): per iteration two system calls, two thread switches and two address-space switches,
/// with what each switch costs on the CPU afterwards (the user translations it drops). Compare
/// with `process_syscall_round_trip` and `thread_yield_round_trip`. Includes starting and
/// joining both once per run.
fn process_switch_round_trip<P: Platform>(memory: &mut Memory<P>, n: u64) {
    let mut threads = [None, None];
    for thread in &mut threads {
        // SAFETY: as in `process_syscall_round_trip`.
        let program = unsafe { Process::<P>::with_test_program(&memory.page_tables) }
            .expect("the test program fits in memory");
        let context = Process::<P>::test_program_context(|program| program.null_syscalls);
        let mut calls = 0;
        *thread = process::spawn::<P, _>(Arc::new(program), context, move |_, _, exit| {
            if exit == UserExit::Syscall {
                calls += 1;
                thread::yield_now::<P>();
            }
            if calls < n {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        })
        .ok();
    }
    for thread in threads.into_iter().flatten() {
        thread.join::<P>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt;
    use core::sync::atomic::{AtomicU64, Ordering};
    use eggos_hal::{ExitStatus, Interrupt};

    /// A platform whose clock advances one tick per read and whose timer fires on the next
    /// wait, whatever its deadline.
    struct CountingPlatform;

    static CLOCK: AtomicU64 = AtomicU64::new(0);

    impl Platform for CountingPlatform {
        const ARCH: &'static str = "counting";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {}
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            CLOCK.fetch_add(1, Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1_000_000_000
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {
            interrupt::handle::<Self>(Interrupt::Timer);
        }
        fn interrupt_window() {}
        fn memory_map() -> MemoryMap {
            MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            0
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            unreachable!("not used in tests")
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn early_device_regions(_region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {}
        unsafe fn activate_page_tables(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        unsafe fn switch_address_space(_roots: &[u64]) {}
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            // The test program's null system calls, and nothing else.
            let pc = context.pc();
            let null = eggos_hal::user::TestProgram::SYSCALL_NULL;
            context.simulate_syscall(null, [0; 6], pc);
            UserExit::Syscall
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            eggos_hal::user::TestProgram {
                code: &[0; 4],
                checks: 0,
                floating_point: 0,
                null_syscalls: 0,
                spin: 0,
            }
        }
        fn make_instructions_visible(_start: usize, _length: usize) {}
        unsafe fn prepare_thread(
            _stack_top: usize,
            entry: extern "C" fn(usize) -> !,
            argument: usize,
        ) -> usize {
            thread::host::prepare_thread(entry, argument)
        }
        unsafe fn switch_thread(save: *mut usize, resume: usize) {
            // SAFETY: the caller's guarantees are the host switch's.
            unsafe { thread::host::switch_thread(save, resume) }
        }
        fn cpus(_cpu: &mut dyn FnMut(u64)) {
            unreachable!("not used in tests")
        }
        fn boot_cpu() -> u64 {
            unreachable!("not used in tests")
        }
        unsafe fn start_cpu(
            _cpu: u64,
            _start: &eggos_hal::cpu::CpuStart,
        ) -> Result<(), eggos_hal::cpu::StartCpuError> {
            unreachable!("not used in tests")
        }
        unsafe fn set_cpu_local(block: usize) {
            // SAFETY: the caller's guarantees are the test platform's.
            unsafe { crate::tests::TestPlatform::set_cpu_local(block) }
        }
        fn cpu_local() -> usize {
            // The test machine's one CPU.
            crate::tests::TestPlatform::cpu_local()
        }
        fn wake_cpu(_cpu: u64) {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn prints_one_parsable_line_per_benchmark() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        // Page tables for `run`, made first: the benchmarks run here must find the frame
        // allocator as the other tests leave it, so the user-mode benchmarks, which take
        // frames, have a test of their own.
        let mut memory = page_tables_for_test();
        let _scheduler = thread::host::lock_kernel_state_for_test();
        let mut out = String::new();
        let all = all::<CountingPlatform>();
        let (benchmarks, [(syscall, _), (process_syscall, _), (process_switch, _)]) =
            all.split_at(all.len() - 3)
        else {
            unreachable!()
        };
        assert_eq!(
            [*syscall, *process_syscall, *process_switch],
            [
                "syscall_round_trip",
                "process_syscall_round_trip",
                "process_switch_round_trip"
            ]
        );
        assert_eq!(
            run::<CountingPlatform, _>(&mut out, benchmarks, &mut memory),
            Ok(())
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), benchmarks.len());
        for (line, (name, _)) in lines.iter().zip(benchmarks) {
            let prefix = format!("{MARKER} {name} iterations={ITERATIONS} ticks=");
            assert!(line.starts_with(&prefix), "{line}");
            assert!(line.ends_with(" frequency=1000000000"), "{line}");
        }
        // The clock is read once per iteration here, plus the measurement's own end read.
        let clock_line = lines[1];
        assert!(
            clock_line.contains(&format!("ticks={} ", ITERATIONS + 1)),
            "{clock_line}"
        );
    }

    /// Kernel memory for the benchmarks: page tables, active (frozen) as the kernel's are.
    fn page_tables_for_test() -> Memory<CountingPlatform> {
        let _frames = thread::host::frames_for_test();
        let mut frames = FRAMES.lock();
        let mut page_tables = crate::paging::PageTables::new(frames.as_mut().unwrap()).unwrap();
        page_tables.mark_active();
        Memory { page_tables }
    }

    #[test]
    fn the_user_mode_benchmarks_run_their_programs_and_free_them() {
        // Their threads pass preemption points, where this platform's timer fires.
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let _frames = thread::host::frames_for_test();
        // Page tables from the same allocator as the processes' (not `page_tables_for_test`,
        // whose allocator may be gone once it returns).
        let mut memory = {
            let mut frames = FRAMES.lock();
            let mut page_tables = crate::paging::PageTables::new(frames.as_mut().unwrap()).unwrap();
            page_tables.mark_active();
            Memory { page_tables }
        };
        // The idle thread's stack, taken once, is not the benchmarks'. Started as the other
        // tests start it: an idle thread of this platform's would take its timer with it.
        thread::start::<crate::tests::TestPlatform>().unwrap();
        let free = || FRAMES.lock().as_ref().unwrap().free_frames();
        let before = free();
        syscall_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        process_syscall_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
        process_switch_round_trip::<CountingPlatform>(&mut memory, 5);
        assert_eq!(free(), before);
    }

    #[test]
    fn frame_benchmarks_run_on_the_host() {
        let _lock = crate::thread::host::lock_kernel_state_for_test();
        frame_allocate_untouched(ITERATIONS);
        frame_free_allocate(ITERATIONS);
        heap_allocate_free_small(ITERATIONS);
        frame_contiguous_allocate_free(ITERATIONS);
    }

    #[test]
    fn timer_interrupt_round_trip_waits_for_each_interrupt() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let before = interrupt::timer_interrupts::<CountingPlatform>();
        timer_interrupt_round_trip::<CountingPlatform>(5);
        assert_eq!(
            interrupt::timer_interrupts::<CountingPlatform>(),
            before + 5
        );
    }
}

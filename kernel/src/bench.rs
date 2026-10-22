//! Boot-time benchmarks of the kernel's hot paths (kernel feature `qemu-bench`).
//!
//! Each benchmark runs its body a fixed number of times between two reads of the platform
//! clock and prints one [`MARKER`] line. `cargo xtask bench` boots the kernel in QEMU with
//! `-icount shift=0`, where the guest clock advances exactly 1 ns per instruction executed,
//! so the elapsed time is an instruction count: the same on every host and every run, which
//! is what lets CI compare it against a committed baseline (D27).

use crate::HEAP;
use crate::memory::{FRAMES, FrameAllocator};
use crate::{cpu, thread};
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::hint::black_box;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_hal::memory::FRAME_SIZE;
use eggos_hal::{MemoryMap, PhysicalRange, Platform};

/// Starts every benchmark result line. The xtask bench runner parses these lines.
pub const MARKER: &str = "EGGOS-BENCH:";

/// Iterations timed per benchmark, after [`WARMUP_ITERATIONS`] untimed ones (which bring the
/// code into the caches and QEMU's translation-block cache).
pub const ITERATIONS: u64 = 10_000;
const WARMUP_ITERATIONS: u64 = 100;

/// A benchmark: a name and a body that runs the measured operation `n` times.
pub type Benchmark = (&'static str, fn(u64));

/// Every benchmark for platform `P`, in run order.
pub fn all<P: Platform>() -> [Benchmark; 13] {
    [
        ("empty_loop", empty_loop),
        ("clock_read", clock_read::<P>),
        ("breakpoint_round_trip", breakpoint_round_trip::<P>),
        ("timer_arm", timer_arm::<P>),
        (
            "timer_interrupt_round_trip",
            timer_interrupt_round_trip::<P>,
        ),
        ("frame_allocate_untouched", frame_allocate_untouched),
        ("frame_free_allocate", frame_free_allocate),
        (
            "frame_contiguous_allocate_free",
            frame_contiguous_allocate_free,
        ),
        ("heap_allocate_free_small", heap_allocate_free_small),
        ("thread_yield_round_trip", thread_yield_round_trip::<P>),
        ("preemption_point_idle", preemption_point_idle::<P>),
        ("wait_queue_wake_empty", wait_queue_wake_empty::<P>),
        (
            "thread_wait_wake_round_trip",
            thread_wait_wake_round_trip::<P>,
        ),
    ]
}

/// Runs `benchmarks` and prints one line per benchmark:
/// `EGGOS-BENCH: <name> iterations=<n> ticks=<clock ticks> frequency=<clock Hz>`.
pub fn run<P: Platform, W: Write>(console: &mut W, benchmarks: &[Benchmark]) -> core::fmt::Result {
    let frequency = P::clock_frequency();
    for (name, body) in benchmarks {
        body(WARMUP_ITERATIONS);
        let start = P::clock_ticks();
        body(ITERATIONS);
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
    // count of interrupts the scheduler has not seen: nothing here runs the scheduler, so it
    // only grows.
    let this = cpu::current::<P>();
    for _ in 0..n {
        let before = this.timer_interrupts_unseen();
        P::set_timer_deadline(0);
        while this.timer_interrupts_unseen() == before {
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
        let _scheduler = thread::host::lock_kernel_state_for_test();
        let mut out = String::new();
        let benchmarks = all::<CountingPlatform>();
        assert_eq!(run::<CountingPlatform, _>(&mut out, &benchmarks), Ok(()));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), benchmarks.len());
        for (line, (name, _)) in lines.iter().zip(&benchmarks) {
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

//! Boot-time benchmarks of the kernel's hot paths (kernel feature `qemu-bench`).
//!
//! Each benchmark runs its body a fixed number of times between two reads of the platform
//! clock and prints one [`MARKER`] line. `cargo xtask bench` boots the kernel in QEMU with
//! `-icount shift=0`, where the guest clock advances exactly 1 ns per instruction executed,
//! so the elapsed time is an instruction count: the same on every host and every run, which
//! is what lets CI compare it against a committed baseline (D27).

use crate::interrupt;
use crate::memory::FrameAllocator;
use core::fmt::Write;
use core::hint::black_box;
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
pub fn all<P: Platform>() -> [Benchmark; 7] {
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
    for _ in 0..n {
        let before = interrupt::timer_interrupts();
        P::set_timer_deadline(0);
        while interrupt::timer_interrupts() == before {
            P::wait_for_interrupt();
        }
    }
}

/// Allocating never-used frames, the path every frame takes first. Untouched frames are not
/// written, so the range can be any addresses.
fn frame_allocate_untouched(n: u64) {
    let mut map = MemoryMap::new();
    map.add_ram(PhysicalRange::from_start_length(1 << 40, n * FRAME_SIZE));
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

#[cfg(test)]
mod tests {
    use super::*;
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
            interrupt::handle(Interrupt::Timer);
        }
        fn memory_map() -> MemoryMap {
            MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
    }

    #[test]
    fn prints_one_parsable_line_per_benchmark() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
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
        frame_allocate_untouched(ITERATIONS);
        frame_free_allocate(ITERATIONS);
    }

    #[test]
    fn timer_interrupt_round_trip_waits_for_each_interrupt() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let before = interrupt::timer_interrupts();
        timer_interrupt_round_trip::<CountingPlatform>(5);
        assert_eq!(interrupt::timer_interrupts(), before + 5);
    }
}

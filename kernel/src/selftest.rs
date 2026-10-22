//! Boot-time self-tests: checks that only mean something on the real (or emulated) machine,
//! such as "did the architecture entry point zero `.bss`". The QEMU test runner reads the
//! verdict markers from the console.

use crate::memory::FrameAllocator;
use crate::{exception, interrupt};
use core::fmt::Write;
use core::sync::atomic::AtomicU64;
use eggos_hal::Platform;

/// Printed when every self-test passed. The QEMU runner requires this exact line.
pub const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
/// Printed when a self-test failed or the kernel panicked.
pub const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";

/// A self-test: a name and a check returning `Err(reason)` on failure.
pub type SelfTest = (&'static str, fn() -> Result<(), &'static str>);

/// Every boot self-test for platform `P`, in run order.
pub fn all<P: Platform>() -> [SelfTest; 4] {
    [
        ("bss_is_zeroed", bss_is_zeroed),
        ("breakpoint_resumes", breakpoint_resumes::<P>),
        ("timer_interrupt_fires", timer_interrupt_fires::<P>),
        ("frames_are_usable_ram", frames_are_usable_ram::<P>),
    ]
}

/// Runs `tests`, reporting each and then the verdict marker. Returns whether all passed.
pub fn run<W: Write>(console: &mut W, tests: &[SelfTest]) -> Result<bool, core::fmt::Error> {
    let mut all_passed = true;
    for (name, test) in tests {
        match test() {
            Ok(()) => writeln!(console, "selftest {name} ... ok")?,
            Err(reason) => {
                all_passed = false;
                writeln!(console, "selftest {name} ... FAILED: {reason}")?;
            }
        }
    }
    let marker = if all_passed { PASS_MARKER } else { FAIL_MARKER };
    writeln!(console, "{marker}")?;
    Ok(all_passed)
}

/// Lives in `.bss` (zero-initialised and interior-mutable, so it cannot be placed in
/// read-only data). Reading anything but zero means the entry point did not clear `.bss`, as
/// on real hardware and warm reboots, where RAM holds whatever was there before.
///
/// QEMU hands over zeroed RAM, which would let a missing zeroing loop pass, so the QEMU boot
/// tests look the probe up by its symbol name and fill it with a non-zero pattern before the
/// kernel starts. The name must match `BSS_PROBE_SYMBOL` in `xtask`.
#[unsafe(export_name = "eggos_selftest_bss_probe")]
static BSS_PROBE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bss_is_zeroed() -> Result<(), &'static str> {
    let all_zero = BSS_PROBE.iter().all(|word| {
        // SAFETY: `word.as_ptr()` points to a live, aligned `u64` (an `AtomicU64` has the
        // layout of `u64`) that nothing writes concurrently. The read must be volatile:
        // since nothing ever stores to the probe, the compiler would otherwise fold an
        // ordinary load to the initial value and the check would test nothing.
        unsafe { core::ptr::read_volatile(word.as_ptr()) == 0 }
    });
    if all_zero {
        Ok(())
    } else {
        Err(".bss was not zeroed by the architecture entry point")
    }
}

/// A breakpoint must reach the kernel's exception handler through the architecture's vectors
/// and come back to the next instruction with the interrupted code's state intact.
fn breakpoint_resumes<P: Platform>() -> Result<(), &'static str> {
    // Values live across the breakpoint, in registers or on the stack (the `asm!` block
    // declares no clobbers), so a handler that corrupts either changes the result.
    let live = [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| core::hint::black_box(n * 0x0101_0101_0101));
    let before = exception::breakpoints_taken();
    P::breakpoint();
    // Greater rather than exactly one more: host unit tests run in parallel and may take
    // breakpoints of their own at the same time.
    if exception::breakpoints_taken() <= before {
        return Err("the breakpoint did not reach the kernel's exception handler");
    }
    let expected = [1u64, 2, 3, 4, 5, 6, 7, 8].map(|n| n * 0x0101_0101_0101);
    if core::hint::black_box(live) != expected {
        return Err("values live across the breakpoint changed (registers or stack not restored)");
    }
    Ok(())
}

/// A timer deadlined 1 ms ahead must interrupt the CPU, not before the deadline, and the
/// clock must move forward. Gives up after 1 s of clock time; a timer that never fires while
/// the clock also stops leaves the CPU asleep, which the QEMU runner's timeout reports.
fn timer_interrupt_fires<P: Platform>() -> Result<(), &'static str> {
    let frequency = P::clock_frequency();
    if frequency < 1000 {
        return Err("the clock runs slower than 1 kHz");
    }
    let before = interrupt::timer_interrupts();
    let start = P::clock_ticks();
    let deadline = start + frequency / 1000;
    let give_up = start + frequency;
    P::set_timer_deadline(deadline);
    // Greater rather than exactly one more, as in `breakpoint_resumes`.
    while interrupt::timer_interrupts() <= before {
        if P::clock_ticks() > give_up {
            return Err("no timer interrupt within 1 s of its deadline");
        }
        P::wait_for_interrupt();
    }
    if P::clock_ticks() < deadline {
        return Err("the timer interrupt came before its deadline");
    }
    Ok(())
}

/// Frames written in [`frames_are_usable_ram`].
const FRAMES_WRITTEN: usize = 64;

/// The boot memory map must offer memory, and its frames must be RAM the kernel can write and
/// read back, handed out once each: write a pattern to the first frames allocated, check them
/// all, free them, see them come back in reverse order, then count every frame up to the end
/// of memory. A frame overlapping the kernel image or its stack would most likely crash the
/// kernel here instead of passing.
fn frames_are_usable_ram<P: Platform>() -> Result<(), &'static str> {
    let map = P::memory_map();
    if map.total() == 0 {
        return Err("the boot memory map has no free memory");
    }
    let offset = P::physical_memory_offset();
    let mut frames = FrameAllocator::new(&map, offset);
    let total = frames.free_frames();
    let mut taken = [0u64; FRAMES_WRITTEN];
    let mut count = 0;
    for slot in &mut taken {
        let Some(frame) = frames.allocate() else {
            break;
        };
        *slot = frame;
        count += 1;
    }
    let taken = &taken[..count];
    let words = |frame: u64| {
        let start = (frame as usize).wrapping_add(offset) as *mut u64;
        let length = eggos_hal::memory::FRAME_SIZE as usize / 8;
        (0..length).map(move |i| (start.wrapping_add(i), frame ^ i as u64))
    };
    for &frame in taken {
        for (word, value) in words(frame) {
            // SAFETY: `frame` was just allocated, so the kernel owns it, and the platform says
            // free RAM is reachable at physical address + offset; `word` is inside the frame
            // and aligned.
            unsafe { word.write_volatile(value) };
        }
    }
    for &frame in taken {
        for (word, value) in words(frame) {
            // SAFETY: as above; the frame is still owned and was written in full.
            if unsafe { word.read_volatile() } != value {
                return Err("a frame did not read back what was written (not RAM, or shared)");
            }
        }
    }
    for &frame in taken {
        // SAFETY: each frame came from `frames` and is no longer used.
        unsafe { frames.free(frame) };
    }
    if taken
        .iter()
        .rev()
        .any(|&frame| frames.allocate() != Some(frame))
    {
        return Err("freed frames did not come back last in, first out");
    }
    let mut handed_out = taken.len() as u64;
    while frames.allocate().is_some() {
        handed_out += 1;
    }
    if handed_out != total {
        return Err("the allocator handed out a different number of frames than the map holds");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes() -> Result<(), &'static str> {
        Ok(())
    }
    fn fails() -> Result<(), &'static str> {
        Err("broken")
    }

    #[test]
    fn all_passing_prints_each_result_and_pass_marker() {
        let mut out = String::new();
        assert_eq!(run(&mut out, &[("a", passes), ("b", passes)]), Ok(true));
        assert_eq!(
            out,
            format!("selftest a ... ok\nselftest b ... ok\n{PASS_MARKER}\n")
        );
    }

    #[test]
    fn one_failure_runs_the_rest_and_prints_fail_marker() {
        let mut out = String::new();
        assert_eq!(run(&mut out, &[("a", fails), ("b", passes)]), Ok(false));
        assert_eq!(
            out,
            format!("selftest a ... FAILED: broken\nselftest b ... ok\n{FAIL_MARKER}\n")
        );
    }

    /// A platform whose breakpoint never reaches the kernel's handler.
    struct LostBreakpoints;

    impl Platform for LostBreakpoints {
        const ARCH: &'static str = "lost";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: eggos_hal::ExitStatus) -> ! {
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
            0
        }
        fn clock_frequency() -> u64 {
            1
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {}
        fn memory_map() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
    }

    #[test]
    fn breakpoint_that_never_reaches_the_handler_fails() {
        assert!(breakpoint_resumes::<LostBreakpoints>().is_err());
    }

    #[test]
    fn clock_too_slow_to_time_a_millisecond_fails() {
        assert_eq!(
            timer_interrupt_fires::<LostBreakpoints>(),
            Err("the clock runs slower than 1 kHz")
        );
    }

    /// A platform whose clock runs (100 ms per read) but whose timer never interrupts.
    struct DeadTimer;

    static DEAD_TIMER_CLOCK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    impl Platform for DeadTimer {
        const ARCH: &'static str = "dead-timer";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: eggos_hal::ExitStatus) -> ! {
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
            DEAD_TIMER_CLOCK.fetch_add(100, core::sync::atomic::Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1000
        }
        fn set_timer_deadline(_deadline: u64) {}
        fn wait_for_interrupt() {}
        fn memory_map() -> eggos_hal::MemoryMap {
            eggos_hal::MemoryMap::new()
        }
        fn physical_memory_offset() -> usize {
            0
        }
    }

    #[test]
    fn timer_that_never_fires_fails_after_a_second() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        assert_eq!(
            timer_interrupt_fires::<DeadTimer>(),
            Err("no timer interrupt within 1 s of its deadline")
        );
    }

    #[test]
    fn no_free_memory_fails() {
        assert_eq!(
            frames_are_usable_ram::<LostBreakpoints>(),
            Err("the boot memory map has no free memory")
        );
    }

    #[test]
    fn bss_probe_is_zero_on_host() {
        assert_eq!(bss_is_zeroed(), Ok(()));
    }
}

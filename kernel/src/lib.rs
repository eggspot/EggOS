//! The architecture-independent EggOS kernel core.
//!
//! Nothing in this crate may depend on a particular CPU architecture or device: the machine
//! is reached only through [`eggos_hal::Platform`], so the core builds and is unit-tested on
//! the development host as well as on every bare-metal target.

#![cfg_attr(not(test), no_std)]

#[cfg(any(test, feature = "qemu-bench"))]
pub mod bench;
pub mod console;
pub mod exception;
pub mod interrupt;
pub mod memory;
pub mod selftest;

use core::fmt::Write;
use eggos_hal::{ExitStatus, Platform};

/// Kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs the architecture-independent part of boot on platform `P`, writing progress to
/// `console`.
///
/// Returns how the boot went; the architecture entry point turns that into a machine exit.
pub fn boot<P: Platform, W: Write>(console: &mut W) -> ExitStatus {
    let report = writeln!(console, "EggOS {VERSION} ({})", P::ARCH)
        .and_then(|()| writeln!(console, "clock: {} Hz", P::clock_frequency()))
        .and_then(|()| report_memory(console, &P::memory_map()))
        .and_then(|()| selftest::run(console, &selftest::all::<P>()));
    // Benchmarks only mean something on a kernel that passed its self-tests.
    #[cfg(feature = "qemu-bench")]
    let report = report.and_then(|passed| {
        if passed {
            bench::run::<P, _>(console, &bench::all::<P>())?;
        }
        Ok(passed)
    });
    match report {
        Ok(true) => ExitStatus::Success,
        Ok(false) | Err(_) => ExitStatus::Failure,
    }
}

/// Prints how much memory is free and where.
fn report_memory<W: Write>(console: &mut W, map: &eggos_hal::MemoryMap) -> core::fmt::Result {
    let ranges = map.ranges();
    let plural = if ranges.len() == 1 { "" } else { "s" };
    writeln!(
        console,
        "memory: {} KiB free in {} range{plural}",
        map.total() / 1024,
        ranges.len()
    )?;
    ranges
        .iter()
        .try_for_each(|range| writeln!(console, "  {range}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::sync::atomic::{AtomicU64, Ordering};
    use eggos_hal::{Exception, ExceptionKind, Interrupt};

    /// A platform whose breakpoint and timer go straight to the kernel's handlers, as the
    /// architecture's vectors do on a real machine. Its clock advances one tick per read.
    struct TestPlatform;

    static CLOCK: AtomicU64 = AtomicU64::new(0);
    static DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);

    /// Physical address of the test platform's RAM.
    const RAM_BASE: u64 = 0x10_0000;
    const RAM_FRAMES: u64 = 128;

    /// Real host memory standing in for the test platform's RAM: its first frame-aligned
    /// address, which the platform's physical memory offset maps `RAM_BASE` to.
    fn ram() -> usize {
        static RAM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *RAM.get_or_init(|| {
            let frame = eggos_hal::memory::FRAME_SIZE as usize;
            let memory = vec![0u8; (RAM_FRAMES as usize + 1) * frame].leak();
            (memory.as_mut_ptr() as usize).next_multiple_of(frame)
        })
    }

    impl Platform for TestPlatform {
        const ARCH: &'static str = "testarch";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {
            let breakpoint = Exception {
                kind: ExceptionKind::Breakpoint,
                name: "test breakpoint",
                code: 0,
                pc: 0,
                address: None,
            };
            assert_eq!(exception::handle(&breakpoint), exception::Outcome::Resume);
        }
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            CLOCK.fetch_add(1, Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1_000_000
        }
        fn set_timer_deadline(deadline: u64) {
            DEADLINE.store(deadline, Ordering::Relaxed);
        }
        fn wait_for_interrupt() {
            // Sleeping until the deadline: jump the clock there and take the interrupt.
            let deadline = DEADLINE.swap(u64::MAX, Ordering::Relaxed);
            if deadline != u64::MAX {
                CLOCK.fetch_max(deadline, Ordering::Relaxed);
                interrupt::handle(Interrupt::Timer);
            }
        }
        fn memory_map() -> eggos_hal::MemoryMap {
            let mut map = eggos_hal::MemoryMap::new();
            map.add_ram(eggos_hal::PhysicalRange::from_start_length(
                RAM_BASE,
                RAM_FRAMES * eggos_hal::memory::FRAME_SIZE,
            ));
            map
        }
        fn physical_memory_offset() -> usize {
            ram().wrapping_sub(RAM_BASE as usize)
        }
    }

    #[test]
    fn boot_prints_banner_with_version_and_arch() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let mut out = String::new();
        assert_eq!(boot::<TestPlatform, _>(&mut out), ExitStatus::Success);
        assert!(out.starts_with(&format!(
            "EggOS {VERSION} (testarch)\nclock: 1000000 Hz\n\
             memory: 512 KiB free in 1 range\n  0x100000..0x180000\n"
        )));
        assert!(out.ends_with(&format!("{}\n", selftest::PASS_MARKER)));
    }
}

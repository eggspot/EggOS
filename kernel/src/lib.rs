//! The architecture-independent EggOS kernel core.
//!
//! Nothing in this crate may depend on a particular CPU architecture or device: the machine
//! is reached only through [`eggos_hal::Platform`], so the core builds and is unit-tested on
//! the development host as well as on every bare-metal target.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(any(test, feature = "qemu-bench"))]
pub mod bench;
pub mod console;
pub mod cpu;
pub mod exception;
pub mod heap;
pub mod interrupt;
pub mod memory;
pub mod paging;
pub mod selftest;
pub mod sync;
pub mod thread;

use core::fmt::Write;
use eggos_hal::{ExitStatus, Platform};

/// The kernel heap. On the bare-metal targets it is the global allocator, so `alloc` (`Box`,
/// `Vec`, ...) works once [`paging::enable`] has run; before that, allocating fails. Host
/// builds keep the host's allocator, and tests use this heap directly.
#[cfg_attr(target_os = "none", global_allocator)]
pub static HEAP: heap::KernelHeap = heap::KernelHeap::new();

/// Kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs the architecture-independent part of boot on platform `P`, writing progress to
/// `console`.
///
/// Returns how the boot went; the architecture entry point turns that into a machine exit.
pub fn boot<P: Platform, W: Write>(console: &mut W) -> ExitStatus {
    cpu::init_boot_cpu::<P>();
    let free_memory = P::memory_map();
    let report = writeln!(console, "EggOS {VERSION} ({})", P::ARCH)
        .and_then(|()| writeln!(console, "clock: {} Hz", P::clock_frequency()))
        .and_then(|()| report_memory(console, &free_memory))
        .and_then(|()| match paging::enable::<P>(&free_memory) {
            Ok(mut memory) => {
                writeln!(
                    console,
                    "paging: kernel page tables active, direct map at {:#x}",
                    P::direct_map_offset()
                )?;
                selftest::run(console, &selftest::all::<P>(), &mut memory)
            }
            Err(reason) => {
                writeln!(console, "paging: FAILED: {reason}")?;
                writeln!(console, "{}", selftest::FAIL_MARKER)?;
                Ok(false)
            }
        });
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
pub(crate) mod tests {
    use super::*;

    use core::sync::atomic::{AtomicU64, Ordering};
    use eggos_hal::{Exception, ExceptionKind, Interrupt};

    /// A platform whose breakpoint and timer go straight to the kernel's handlers, as the
    /// architecture's vectors do on a real machine. Its clock advances one tick per read.
    pub(crate) struct TestPlatform;

    static CLOCK: AtomicU64 = AtomicU64::new(0);
    static DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);

    /// The test platform's armed timer deadline; `u64::MAX` if none.
    pub(crate) fn timer_deadline() -> u64 {
        DEADLINE.load(Ordering::Relaxed)
    }

    /// Physical address of the test platform's RAM.
    const RAM_BASE: u64 = 0x10_0000;
    const RAM_FRAMES: u64 = 128;

    /// Real host memory standing in for the test platform's RAM: the address the platform's
    /// physical memory offset maps `RAM_BASE` to. The offset is aligned to the largest buddy
    /// block, as the real direct maps are, so physically aligned blocks are aligned virtually.
    fn ram() -> usize {
        static RAM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *RAM.get_or_init(|| {
            let frame = eggos_hal::memory::FRAME_SIZE as usize;
            let block = frame << memory::MAX_ORDER;
            let memory = vec![0u8; RAM_FRAMES as usize * frame + block].leak();
            // The first address in `memory` congruent to `RAM_BASE` modulo `block`.
            let start = memory.as_mut_ptr() as usize;
            let base = RAM_BASE as usize % block;
            start + (base + block - start % block) % block
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
        fn interrupt_window() {
            // Time passes; the timer interrupt is taken once its deadline has.
            let now = CLOCK.fetch_add(1, Ordering::Relaxed);
            let deadline = DEADLINE.load(Ordering::Relaxed);
            if deadline != u64::MAX && now >= deadline {
                DEADLINE.store(u64::MAX, Ordering::Relaxed);
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
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            Self::physical_memory_offset()
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            // Below the free RAM, in RAM the direct map covers (see `physical_memory`), as on
            // real machines: the tables only describe it, nothing reaches it.
            eggos_hal::paging::KernelImage {
                virtual_offset: 0xffff_ffff_8000_0000,
                start: 0x8_0000,
                text_start: 0x8_1000,
                rodata_start: 0x8_4000,
                data_start: 0x8_6000,
                stack_guard_start: 0x8_8000,
                stack_start: 0x8_9000,
                end: 0x9_9000,
            }
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            let mut map = Self::memory_map();
            let image = Self::kernel_image();
            map.add_ram(eggos_hal::PhysicalRange {
                start: image.start,
                end: image.end,
            });
            map
        }
        fn early_device_regions(region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {
            // Two register blocks in one page, as the paging code must allow.
            region(eggos_hal::PhysicalRange::from_start_length(0xa_0000, 0x100));
            region(eggos_hal::PhysicalRange::from_start_length(0xa_0800, 0x100));
        }
        unsafe fn activate_page_tables(roots: &[u64]) {
            assert_eq!(roots.len(), 1);
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
        fn cpus(cpu: &mut dyn FnMut(u64)) {
            cpu(0);
        }
        fn boot_cpu() -> u64 {
            0
        }
        unsafe fn start_cpu(
            _cpu: u64,
            _start: &eggos_hal::cpu::CpuStart,
        ) -> Result<(), eggos_hal::cpu::StartCpuError> {
            unreachable!("the test platform has one CPU")
        }
        unsafe fn set_cpu_local(block: usize) {
            CPU_LOCAL.set(block);
        }
        fn cpu_local() -> usize {
            CPU_LOCAL.get()
        }
    }

    std::thread_local! {
        /// The test platform's per-CPU register: one per host thread, each test's "CPU".
        static CPU_LOCAL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn boot_prints_banner_with_version_and_arch() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let _scheduler = thread::host::lock_kernel_state_for_test();
        let mut out = String::new();
        assert_eq!(
            boot::<TestPlatform, _>(&mut out),
            ExitStatus::Success,
            "{out}"
        );
        assert!(out.starts_with(&format!(
            "EggOS {VERSION} (testarch)\nclock: 1000000 Hz\n\
             memory: 512 KiB free in 1 range\n  0x100000..0x180000\n\
             paging: kernel page tables active, direct map at {:#x}\n",
            TestPlatform::direct_map_offset()
        )));
        assert!(out.ends_with(&format!("{}\n", selftest::PASS_MARKER)));
    }
}

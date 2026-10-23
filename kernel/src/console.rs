//! The kernel's view of the early boot console.

use core::fmt;
use core::marker::PhantomData;
use eggos_hal::Platform;

/// A [`fmt::Write`] sink that forwards to the platform's early boot console.
pub struct PlatformConsole<P: Platform>(PhantomData<P>);

impl<P: Platform> PlatformConsole<P> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<P: Platform> Default for PlatformConsole<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Platform> fmt::Debug for PlatformConsole<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlatformConsole")
    }
}

impl<P: Platform> fmt::Write for PlatformConsole<P> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        P::console_write(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;
    use eggos_hal::ExitStatus;
    use std::cell::RefCell;

    std::thread_local! {
        static WRITTEN: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    struct FakePlatform;

    impl Platform for FakePlatform {
        const ARCH: &'static str = "fake";
        fn console_write(bytes: &[u8]) {
            WRITTEN.with(|w| w.borrow_mut().extend_from_slice(bytes));
        }
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {
            unreachable!("not used in tests")
        }
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            unreachable!("not used in tests")
        }
        fn clock_frequency() -> u64 {
            unreachable!("not used in tests")
        }
        fn set_timer_deadline(_deadline: u64) {
            unreachable!("not used in tests")
        }
        fn wait_for_interrupt() {
            unreachable!("not used in tests")
        }
        fn interrupt_window() {
            unreachable!("not used in tests")
        }
        fn memory_map() -> eggos_hal::MemoryMap {
            unreachable!("not used in tests")
        }
        fn physical_memory_offset() -> usize {
            unreachable!("not used in tests")
        }
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            unreachable!("not used in tests")
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            unreachable!("not used in tests")
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            unreachable!("not used in tests")
        }
        fn early_device_regions(_region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {
            unreachable!("not used in tests")
        }
        unsafe fn activate_page_tables(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        unsafe fn switch_address_space(_roots: &[u64]) {
            unreachable!("not used in tests")
        }
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            _context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            unreachable!()
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            unreachable!()
        }
        fn linux_hardware_capabilities() -> [u64; 2] {
            [0, 0]
        }
        fn random_seeds(_seed: &mut dyn FnMut(eggos_hal::random::SeedSource, &[u8])) {}
        fn make_instructions_visible(_start: usize, _length: usize) {}
        fn drop_user_translations(_start: u64, _length: u64) {}
        fn page_fault(
            kind: eggos_hal::ExceptionKind,
            code: u64,
        ) -> Option<eggos_hal::paging::PageFault> {
            crate::tests::page_fault(kind, code)
        }
        unsafe fn prepare_thread(
            _stack_top: usize,
            _entry: extern "C" fn(usize) -> !,
            _argument: usize,
        ) -> usize {
            unreachable!("not used in tests")
        }
        unsafe fn switch_thread(_save: *mut usize, _resume: usize) {
            unreachable!("not used in tests")
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
        unsafe fn set_cpu_local(_block: usize) {
            unreachable!("not used in tests")
        }
        fn cpu_local() -> usize {
            unreachable!("not used in tests")
        }
        fn wake_cpu(_cpu: u64) {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn forwards_formatted_text_to_platform() {
        let mut console = PlatformConsole::<FakePlatform>::new();
        write!(console, "a{}c", 1 + 1).unwrap();
        WRITTEN.with(|w| assert_eq!(w.borrow().as_slice(), b"a2c"));
    }
}

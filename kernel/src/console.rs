//! The kernel's view of the early boot console: output, and the bytes it receives, which its
//! interrupt queues for readers.

use crate::sync::LeafSpinLock;
use core::fmt;
use core::marker::PhantomData;
use eggos_hal::Platform;

/// The line the kernel writes once received bytes reach [`read_input`], so a host feeding the
/// console knows from then on nothing it sends is lost. Must match the QEMU runner's.
pub const INPUT_READY_LINE: &str = "EGGOS-CONSOLE: input ready";

/// Received bytes the queue holds before it drops new ones.
pub const INPUT_CAPACITY: usize = 1024;

/// Bytes received and not read yet, oldest first.
#[derive(Debug)]
pub struct InputQueue {
    bytes: [u8; INPUT_CAPACITY],
    start: usize,
    len: usize,
    dropped: u64,
}

impl InputQueue {
    pub const fn new() -> Self {
        Self {
            bytes: [0; INPUT_CAPACITY],
            start: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Appends `received`; what does not fit is dropped and counted.
    pub fn push(&mut self, received: &[u8]) {
        let fits = received.len().min(INPUT_CAPACITY - self.len);
        for &byte in &received[..fits] {
            self.bytes[(self.start + self.len) % INPUT_CAPACITY] = byte;
            self.len += 1;
        }
        self.dropped += (received.len() - fits) as u64;
    }

    /// Moves the oldest bytes into `buffer`, as many as fit; returns how many.
    pub fn pop(&mut self, buffer: &mut [u8]) -> usize {
        let taken = buffer.len().min(self.len);
        for slot in &mut buffer[..taken] {
            *slot = self.bytes[self.start];
            self.start = (self.start + 1) % INPUT_CAPACITY;
        }
        self.len -= taken;
        taken
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Received bytes dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new()
    }
}

static INPUT: LeafSpinLock<InputQueue> = LeafSpinLock::new_leaf(InputQueue::new());

/// Has the console's received bytes interrupt the boot CPU and queue for [`read_input`].
/// Called once, on the boot CPU, once the kernel's page tables are active.
pub fn enable_input<P: Platform>() -> Result<(), &'static str> {
    P::enable_console_input()
}

/// Queues every byte the console's UART holds; called for [`eggos_hal::Interrupt::ConsoleInput`].
/// Out of line, so the timer's and the wake-up's way through the interrupt handler keep their
/// length.
#[cold]
#[inline(never)]
pub fn receive_input<P: Platform>() {
    let mut chunk = [0u8; 64];
    let mut input = INPUT.lock();
    loop {
        let received = P::console_read(&mut chunk);
        if received == 0 {
            break;
        }
        input.push(&chunk[..received]);
    }
}

/// Moves the oldest received bytes into `buffer`, as many as are queued and fit; never waits.
pub fn read_input(buffer: &mut [u8]) -> usize {
    INPUT.lock().pop(buffer)
}

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
        fn console_read(_buffer: &mut [u8]) -> usize {
            unreachable!("not used in tests")
        }
        fn enable_console_input() -> Result<(), &'static str> {
            unreachable!("not used in tests")
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
        unsafe fn set_user_fp(_on: bool) {
            unreachable!("not used in tests")
        }
        unsafe fn save_fp(_state: *mut eggos_hal::fp::FpState) {
            unreachable!("not used in tests")
        }
        unsafe fn restore_fp(_state: *const eggos_hal::fp::FpState) {
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
    fn the_input_queue_hands_out_bytes_in_order_across_its_end() {
        let mut queue = InputQueue::new();
        let mut buffer = [0u8; INPUT_CAPACITY];
        queue.push(&[1; INPUT_CAPACITY - 3]);
        assert_eq!(
            queue.pop(&mut buffer[..INPUT_CAPACITY - 4]),
            INPUT_CAPACITY - 4
        );
        queue.push(b"abcdef");
        assert_eq!(queue.len(), 7);
        assert_eq!(queue.pop(&mut buffer), 7);
        assert_eq!(&buffer[..7], b"\x01abcdef");
        assert!(queue.is_empty());
        assert_eq!(queue.pop(&mut buffer), 0);
    }

    #[test]
    fn a_full_input_queue_drops_and_counts_what_does_not_fit() {
        let mut queue = InputQueue::new();
        queue.push(&[7; INPUT_CAPACITY - 1]);
        queue.push(b"xyz");
        assert_eq!(queue.len(), INPUT_CAPACITY);
        assert_eq!(queue.dropped(), 2);
        let mut buffer = [0u8; INPUT_CAPACITY + 1];
        assert_eq!(queue.pop(&mut buffer), INPUT_CAPACITY);
        assert_eq!(buffer[INPUT_CAPACITY - 1], b'x');
    }

    #[test]
    fn the_console_input_interrupt_queues_everything_the_uart_holds() {
        use crate::tests::{CONSOLE_INPUT, TestPlatform};
        let _boot_test = crate::interrupt::lock_timer_interrupts_for_test();
        let received: Vec<u8> = (0..200u8).collect();
        CONSOLE_INPUT.lock().unwrap().extend(&received);
        crate::interrupt::handle::<TestPlatform>(eggos_hal::Interrupt::ConsoleInput);
        assert!(CONSOLE_INPUT.lock().unwrap().is_empty());
        let mut buffer = [0u8; 256];
        assert_eq!(read_input(&mut buffer[..150]), 150);
        assert_eq!(read_input(&mut buffer[150..]), 50);
        assert_eq!(&buffer[..200], &received[..]);
        assert_eq!(read_input(&mut buffer), 0);
    }

    #[test]
    fn forwards_formatted_text_to_platform() {
        let mut console = PlatformConsole::<FakePlatform>::new();
        write!(console, "a{}c", 1 + 1).unwrap();
        WRITTEN.with(|w| assert_eq!(w.borrow().as_slice(), b"a2c"));
    }
}

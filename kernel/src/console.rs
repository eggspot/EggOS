//! The kernel's view of the early boot console: output, and the input its interrupt receives,
//! which goes through the console's line discipline ([`line_discipline`], D100) to readers.
//! Readers wait on [`INPUT_KEY`]; the signal characters reach the console's [`Foreground`].

pub mod line_discipline;

use crate::linux::futex;
use crate::sync::{LeafSpinLock, SpinLock};
use alloc::sync::Arc;
use core::fmt;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use eggos_hal::Platform;
use line_discipline::{LineDiscipline, Received, Taken, Termios};

/// The line the kernel writes once received bytes reach the line discipline, so a host feeding
/// the console knows from then on nothing it sends is lost. Must match the QEMU runner's.
pub const INPUT_READY_LINE: &str = "EGGOS-CONSOLE: input ready";

/// The futex key threads waiting for console input wait on: in no process's address space
/// (process ids start at 1), so no program's futex names it. Woken whenever input becomes
/// readable, a signal character is typed or the settings change.
pub const INPUT_KEY: futex::Key = futex::Key::new(0, 0, false);

static LINE_DISCIPLINE: LeafSpinLock<LineDiscipline> =
    LeafSpinLock::new_leaf(LineDiscipline::new());

/// Gives the console a Linux serial console's settings ([`Termios::SERIAL_CONSOLE`]) and has
/// its received bytes interrupt the boot CPU and reach the line discipline. Called once, on
/// the boot CPU, once the kernel's page tables are active.
pub fn enable_input<P: Platform>() -> Result<(), &'static str> {
    LINE_DISCIPLINE
        .lock()
        .set_settings(Termios::SERIAL_CONSOLE, true);
    P::enable_console_input()
}

/// Passes every byte the console's UART holds through the line discipline, a chunk at a time,
/// echoing each chunk as the discipline says once its lock is released (but for an echo too
/// long to hold, such as a long line killed), then sends the signals typed to the
/// [`Foreground`] and wakes the threads waiting for input; called for
/// [`eggos_hal::Interrupt::ConsoleInput`]. Out of line, so the timer's and the wake-up's way
/// through the interrupt handler keep their length.
#[cold]
#[inline(never)]
pub fn receive_input<P: Platform>() {
    let mut chunk = [0u8; INPUT_CHUNK];
    let mut received = Received::default();
    loop {
        let mut echo = EchoBuffer::<P>::new();
        let (length, echoing) = {
            let mut discipline = LINE_DISCIPLINE.lock();
            let length = P::console_read(&mut chunk);
            received |= discipline.receive(&chunk[..length], &mut |bytes| echo.push(bytes));
            let echoing = echo.length > 0;
            if echoing {
                ECHOES_BEING_WRITTEN.fetch_add(1, Ordering::Relaxed);
            }
            (length, echoing)
        };
        echo.write();
        if echoing {
            ECHOES_BEING_WRITTEN.fetch_sub(1, Ordering::Release);
        }
        if length == 0 {
            break;
        }
    }
    if received.interrupt {
        signal_foreground(TerminalSignal::Interrupt);
    }
    if received.quit {
        signal_foreground(TerminalSignal::Quit);
    }
    if received.readable || received.interrupt || received.quit {
        wake_input_waiters::<P>();
    }
}

const INPUT_CHUNK: usize = 64;

static ECHOES_BEING_WRITTEN: AtomicU32 = AtomicU32::new(0);

const ECHO_BUFFER: usize = 4 * INPUT_CHUNK;

struct EchoBuffer<P: Platform> {
    bytes: [u8; ECHO_BUFFER],
    length: usize,
    platform: PhantomData<P>,
}

impl<P: Platform> EchoBuffer<P> {
    fn new() -> Self {
        Self {
            bytes: [0; ECHO_BUFFER],
            length: 0,
            platform: PhantomData,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.length + bytes.len() > ECHO_BUFFER {
            self.write();
        }
        if bytes.len() > ECHO_BUFFER {
            P::console_write(bytes);
            return;
        }
        self.bytes[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }

    fn write(&mut self) {
        P::console_write(&self.bytes[..self.length]);
        self.length = 0;
    }
}

/// Wakes every thread waiting on [`INPUT_KEY`], which looks again.
pub fn wake_input_waiters<P: Platform>() {
    futex::wake::<P>(INPUT_KEY, u32::MAX, u32::MAX);
}

/// Moves unread input into `buffer`, as [`LineDiscipline::take`] says; never waits for input.
/// Returns once the echo of what it took is written, which the receive interrupt on another
/// CPU may still be writing.
pub fn take_input(buffer: &mut [u8]) -> Taken {
    let taken = LINE_DISCIPLINE.lock().take(buffer);
    if taken.length > 0 || taken.line_ended {
        while ECHOES_BEING_WRITTEN.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }
    }
    taken
}

/// Moves unread input into `buffer` and returns how many bytes it moved; never waits.
pub fn read_input(buffer: &mut [u8]) -> usize {
    take_input(buffer).length
}

/// Whether a read would find input ([`LineDiscipline::is_readable`]).
pub fn input_ready() -> bool {
    LINE_DISCIPLINE.lock().is_readable()
}

/// The unread bytes a non-canonical read can take.
pub fn available_input() -> usize {
    LINE_DISCIPLINE.lock().available()
}

/// The bytes a read can take without waiting ([`LineDiscipline::pending`]).
pub fn pending_input() -> usize {
    LINE_DISCIPLINE.lock().pending()
}

/// The console's settings.
pub fn settings() -> Termios {
    LINE_DISCIPLINE.lock().settings()
}

/// Changes the console's settings, discarding its input first if `flush`
/// ([`LineDiscipline::set_settings`]), and wakes the threads waiting for input.
pub fn set_settings<P: Platform>(settings: Termios, flush: bool) {
    LINE_DISCIPLINE.lock().set_settings(settings, flush);
    wake_input_waiters::<P>();
}

/// Discards the console's unread input and the line being edited.
pub fn flush_input() {
    LINE_DISCIPLINE.lock().flush();
}

/// A signal the console's input raises for its [`Foreground`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSignal {
    /// `VINTR`, `^C`: `SIGINT`.
    Interrupt,
    /// `VQUIT`, `^\`: `SIGQUIT`.
    Quit,
}

/// What the console's signal characters reach: for now the program that last read the
/// console or waited for its input (D100), until process groups and `TIOCSPGRP` name a
/// foreground process group (D99, item 7).
pub trait Foreground: Send + Sync {
    /// Sends `signal`. Called from the console's interrupt, with the foreground's lock held.
    fn send(&self, signal: TerminalSignal);
}

static FOREGROUND: SpinLock<Option<Arc<dyn Foreground>>> = SpinLock::new(None);

static FOREGROUND_READER: AtomicU64 = AtomicU64::new(NO_READER);

const NO_READER: u64 = 0;

/// The id of thread `thread` of process `process` as a [`Foreground`]: never 0, as process ids
/// start at 1.
pub fn reader_id(process: u32, thread: u32) -> u64 {
    u64::from(process) << 32 | u64::from(thread)
}

/// Whether reader `reader` ([`reader_id`]) is the console's foreground.
pub fn is_foreground(reader: u64) -> bool {
    FOREGROUND_READER.load(Ordering::Relaxed) == reader
}

/// Makes `foreground`, reader `reader` ([`reader_id`]), the console's foreground. A caller
/// that then checks whether the reader's process ended, and forgets it if so, is ordered
/// against [`forget_foreground`] by the fence here.
pub fn set_foreground(reader: u64, foreground: Arc<dyn Foreground>) {
    let previous = {
        let mut current = FOREGROUND.lock();
        FOREGROUND_READER.store(reader, Ordering::Relaxed);
        current.replace(foreground)
    };
    fence(Ordering::SeqCst);
    drop(previous);
}

/// Forgets the foreground if it is one of process `process`'s threads: called as the process
/// ends, after it is marked ended, so the console holds no reference to it. The fence pairs
/// with the one in [`set_foreground`]: either this sees the new reader, or that reader sees
/// the process ended.
pub fn forget_foreground(process: u32) {
    fence(Ordering::SeqCst);
    if (FOREGROUND_READER.load(Ordering::Relaxed) >> 32) as u32 != process {
        return;
    }
    let previous = {
        let mut current = FOREGROUND.lock();
        if (FOREGROUND_READER.load(Ordering::Relaxed) >> 32) as u32 != process {
            return;
        }
        FOREGROUND_READER.store(NO_READER, Ordering::Relaxed);
        current.take()
    };
    drop(previous);
}

/// Forgets `foreground` if it is still the console's foreground ([`set_foreground`]), and
/// nothing else: for a reader whose process ended as it became the foreground, which must not
/// forget a later reader of the program `execve` started with the same process id.
pub fn forget_this_foreground(foreground: &Arc<dyn Foreground>) {
    let previous = {
        let mut current = FOREGROUND.lock();
        if !current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, foreground))
        {
            return;
        }
        FOREGROUND_READER.store(NO_READER, Ordering::Relaxed);
        current.take()
    };
    drop(previous);
}

#[cfg(test)]
pub(crate) fn forget_foreground_for_test() {
    let previous = {
        let mut current = FOREGROUND.lock();
        FOREGROUND_READER.store(NO_READER, Ordering::Relaxed);
        current.take()
    };
    drop(previous);
}

fn signal_foreground(signal: TerminalSignal) {
    if let Some(foreground) = FOREGROUND.lock().as_ref() {
        foreground.send(signal);
    }
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

    struct ConsoleForTest {
        _interrupts: std::sync::MutexGuard<'static, ()>,
        _kernel: std::sync::MutexGuard<'static, ()>,
    }

    impl ConsoleForTest {
        fn take() -> Self {
            let interrupts = crate::interrupt::lock_timer_interrupts_for_test();
            let kernel = crate::thread::host::lock_kernel_state_for_test();
            LINE_DISCIPLINE
                .lock()
                .set_settings(Termios::SERIAL_CONSOLE, true);
            crate::tests::console_output();
            Self {
                _interrupts: interrupts,
                _kernel: kernel,
            }
        }

        fn types(&self, input: &[u8]) {
            use crate::tests::{CONSOLE_INPUT, TestPlatform};
            CONSOLE_INPUT.lock().unwrap().extend(input);
            crate::interrupt::handle::<TestPlatform>(eggos_hal::Interrupt::ConsoleInput);
            assert!(CONSOLE_INPUT.lock().unwrap().is_empty());
        }
    }

    impl Drop for ConsoleForTest {
        fn drop(&mut self) {
            flush_input();
        }
    }

    #[test]
    fn the_console_input_interrupt_edits_echoes_and_hands_out_lines() {
        let console = ConsoleForTest::take();
        console.types(b"hi\x7fey\r");
        assert_eq!(crate::tests::console_output(), b"hi\x08 \x08ey\n");
        assert!(input_ready());
        assert_eq!(pending_input(), 4);
        let mut buffer = [0u8; 16];
        assert_eq!(read_input(&mut buffer), 4);
        assert_eq!(&buffer[..4], b"hey\n");
        assert_eq!(read_input(&mut buffer), 0);
        assert!(!input_ready());
    }

    #[test]
    fn a_long_burst_of_input_is_taken_whole_in_one_interrupt() {
        let console = ConsoleForTest::take();
        let burst: Vec<u8> = (0..300).map(|i| b'a' + (i % 26) as u8).collect();
        console.types(&burst);
        console.types(b"\n");
        let mut buffer = [0u8; 400];
        assert_eq!(read_input(&mut buffer), 301);
        assert_eq!(&buffer[..300], &burst[..]);
    }

    #[test]
    fn echo_longer_than_its_buffer_reaches_the_console_whole_and_in_order() {
        let console = ConsoleForTest::take();
        let line: Vec<u8> = (0..300).map(|i| b'a' + (i % 26) as u8).collect();
        console.types(&[&line[..], b"\x15ok\r"].concat());
        let rub_out = b"\x08 \x08".repeat(line.len());
        let expected = [&line[..], &rub_out, b"ok\n"].concat();
        assert_eq!(crate::tests::console_output(), expected);
        let mut buffer = [0u8; 8];
        assert_eq!(read_input(&mut buffer), 3);
    }

    static SIGNALS_SENT: std::sync::Mutex<Vec<TerminalSignal>> = std::sync::Mutex::new(Vec::new());

    #[derive(Debug)]
    struct RecordingForeground;

    impl Foreground for RecordingForeground {
        fn send(&self, signal: TerminalSignal) {
            SIGNALS_SENT.lock().unwrap().push(signal);
        }
    }

    #[test]
    fn signal_characters_reach_the_foreground_until_its_process_ends() {
        let console = ConsoleForTest::take();
        let (process, thread) = (u32::MAX, 7);
        let reader = reader_id(process, thread);
        assert!(!is_foreground(reader));
        set_foreground(reader, Arc::new(RecordingForeground));
        assert!(is_foreground(reader));
        console.types(b"x\x03y\x1c");
        assert_eq!(
            core::mem::take(&mut *SIGNALS_SENT.lock().unwrap()),
            [TerminalSignal::Interrupt, TerminalSignal::Quit]
        );
        assert_eq!(crate::tests::console_output(), b"x^Cy^\\");
        forget_foreground(process - 1);
        assert!(is_foreground(reader));
        forget_foreground(process);
        assert!(!is_foreground(reader));
        console.types(b"\x03");
        assert!(SIGNALS_SENT.lock().unwrap().is_empty());
    }

    #[test]
    fn forwards_formatted_text_to_platform() {
        let mut console = PlatformConsole::<FakePlatform>::new();
        write!(console, "a{}c", 1 + 1).unwrap();
        WRITTEN.with(|w| assert_eq!(w.borrow().as_slice(), b"a2c"));
    }
}

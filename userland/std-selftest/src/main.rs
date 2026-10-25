//! The kernel's boot self-tests run this program, built with Rust's standard library and
//! linked with musl, as a Linux program: it uses what a typical Rust program uses from its
//! first line (arguments, the environment, a `HashMap` with random keys, the heap, the
//! clocks, sleeping, formatting floats, threads, standard output), writes
//! `EGGOS-USER: a std program built from Rust source ran its checks` if every check passed,
//! and exits with 0; otherwise it names each check that failed and exits with 1. Given
//! [`EXIT_FROM_A_THREAD`] as its only argument, it instead ends from a thread while its other
//! threads wait ([`exit_from_a_thread`]). Given [`UNMAP_UNDER_A_WRITING_THREAD`], it unmaps a
//! page another thread is writing on another CPU ([`unmap_under_a_writing_thread`]).

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The line written when every check passed.
pub const PASSED_LINE: &str = "EGGOS-USER: a std program built from Rust source ran its checks";

const ARGUMENTS_AFTER_PROGRAM_NAME: [&str; 2] = ["first", "second"];

/// The argument that runs [`exit_from_a_thread`] instead of the checks.
pub const EXIT_FROM_A_THREAD: &str = "exit-from-a-thread";

/// The argument that runs [`unmap_under_a_writing_thread`] instead of the checks.
pub const UNMAP_UNDER_A_WRITING_THREAD: &str = "unmap-under-a-writing-thread";

/// [`unmap_under_a_writing_thread`]'s exit statuses other than 0.
pub mod unmap_status {
    pub const KEPT_WRITING: u8 = 1;
    pub const ONE_CPU: u8 = 3;
    pub const NO_PAGE: u8 = 4;
}

const SLEEP: Duration = Duration::from_millis(2);

const PAGE: usize = 4096;
const PROT_READ_WRITE: i32 = 3;
const MAP_PRIVATE_ANONYMOUS: i32 = 0x22;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
const SIGSEGV: i32 = 11;
const WRITES_BEFORE_UNMAP: u64 = 100_000;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
    fn sched_getcpu() -> i32;
    fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
    fn gettid() -> i32;
    fn pthread_self() -> usize;
    fn pthread_kill(thread: usize, signal: i32) -> i32;
    fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32;
    fn sigsuspend(mask: *const u64) -> i32;
    fn kill(process: i32, signal: i32) -> i32;
    fn getpid() -> i32;
    fn nanosleep(request: *const Timespec, remaining: *mut Timespec) -> i32;
    fn _exit(status: i32) -> !;
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

type Check = (&'static str, fn() -> bool);

const THREADS: u64 = 4;

const SIGUSR1: i32 = 10;
const SIG_BLOCK: i32 = 0;
const SIG_UNBLOCK: i32 = 1;
const SIGSET_WORDS: usize = 16;

static HANDLED_BY: AtomicI32 = AtomicI32::new(0);
static READY_TARGET: AtomicUsize = AtomicUsize::new(0);
static WAITING_THREAD: AtomicI32 = AtomicI32::new(0);
static HANDLED_WHILE_WAITING: AtomicI32 = AtomicI32::new(0);
static RELEASED: AtomicBool = AtomicBool::new(false);

const EINTR: i32 = 4;
const LONG_SLEEP: Timespec = Timespec {
    seconds: 10,
    nanoseconds: 0,
};
const SEND_INTERVAL: Duration = Duration::from_millis(10);
const MOST_SENDS: u32 = 500;

const CHECKS: [Check; 13] = [
    ("the arguments are the program's", arguments),
    ("the environment holds TERM", environment),
    ("a HashMap with random keys counts words", hash_map),
    ("the heap holds a large vector", heap),
    ("a boxed trait object dispatches", trait_object),
    ("floats format as on Linux", float_formatting),
    ("sleeping takes at least its time", sleeping),
    ("the realtime clock reads", realtime_clock),
    ("threads run, share memory and are joined", threads),
    (
        "a mutex and its condition variable hand work over",
        condition_variable,
    ),
    (
        "a signal sent to another thread runs its handler there",
        signals_between_threads,
    ),
    (
        "a signal sent to the process runs its handler in a thread that does not block it",
        signals_to_the_process,
    ),
    (
        "a signal ends a sleep with EINTR and the time left, and a futex wait runs its handler",
        signals_interrupt_waits,
    ),
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some(EXIT_FROM_A_THREAD) => exit_from_a_thread(),
        Some(UNMAP_UNDER_A_WRITING_THREAD) => unmap_under_a_writing_thread(),
        _ => {}
    }
    let mut output = std::io::stdout().lock();
    let mut failed = false;
    for (name, check) in CHECKS {
        if !check() {
            failed = true;
            let _ = writeln!(output, "EGGOS-USER: the std program's check failed: {name}");
        }
    }
    if failed {
        return ExitCode::FAILURE;
    }
    match writeln!(output, "{PASSED_LINE}").and_then(|()| output.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Exits with 0 from a thread while the main thread sleeps in `nanosleep` for an hour and
/// another thread waits on a futex for good; exits with 1 if the main thread wakes first.
pub fn exit_from_a_thread() -> ! {
    std::thread::spawn(|| {
        loop {
            std::thread::park();
        }
    });
    std::thread::spawn(|| {
        std::thread::sleep(SLEEP);
        std::process::exit(0)
    });
    std::thread::sleep(Duration::from_secs(3600));
    std::process::exit(1)
}

/// Has a thread write a page in a loop on another CPU, then unmaps the page: the writer must
/// fault, and its `SIGSEGV` handler exits with 0, within a second. Exits with
/// [`unmap_status::KEPT_WRITING`] if it goes on writing, [`unmap_status::ONE_CPU`] if the two
/// threads ran on one CPU, [`unmap_status::NO_PAGE`] if the page could not be mapped.
pub fn unmap_under_a_writing_thread() -> ! {
    // SAFETY: an anonymous private mapping the kernel places; nothing else uses it.
    let page = unsafe {
        mmap(
            std::ptr::null_mut(),
            PAGE,
            PROT_READ_WRITE,
            MAP_PRIVATE_ANONYMOUS,
            -1,
            0,
        )
    };
    if page == MAP_FAILED {
        std::process::exit(unmap_status::NO_PAGE.into());
    }
    // SAFETY: `exit_on_fault` only calls `_exit`, which is async-signal-safe.
    unsafe { signal(SIGSEGV, exit_on_fault) };
    let writer_cpu = Arc::new(AtomicI32::new(-1));
    let address = page as usize;
    std::thread::spawn({
        let writer_cpu = Arc::clone(&writer_cpu);
        move || {
            // SAFETY: a libc call with no arguments.
            writer_cpu.store(unsafe { sched_getcpu() }, Ordering::SeqCst);
            // SAFETY: the page is mapped, writable and aligned until the main thread unmaps it;
            // only atomics reach it, and the write after the unmap faults, ending the process.
            let word = unsafe { AtomicU64::from_ptr(address as *mut u64) };
            let mut count = 0u64;
            loop {
                count += 1;
                word.store(count, Ordering::Relaxed);
            }
        }
    });
    // SAFETY: the page stays mapped, aligned, until the `munmap` below; only atomics reach it.
    let word = unsafe { AtomicU64::from_ptr(page.cast::<u64>()) };
    while word.load(Ordering::Relaxed) < WRITES_BEFORE_UNMAP {
        std::hint::spin_loop();
    }
    // SAFETY: a libc call with no arguments.
    if unsafe { sched_getcpu() } == writer_cpu.load(Ordering::SeqCst) {
        std::process::exit(unmap_status::ONE_CPU.into());
    }
    // SAFETY: the page came from `mmap` above; only the writer uses it, and its fault after
    // this ends the process.
    unsafe { munmap(page, PAGE) };
    std::thread::sleep(Duration::from_secs(1));
    std::process::exit(unmap_status::KEPT_WRITING.into())
}

extern "C" fn exit_on_fault(_signal: i32) {
    // SAFETY: `_exit` ends the process at once.
    unsafe { _exit(0) }
}

fn arguments() -> bool {
    let mut arguments = std::env::args();
    let program = arguments.next();
    program.is_some_and(|program| program.ends_with("eggos-std-selftest"))
        && arguments.eq(ARGUMENTS_AFTER_PROGRAM_NAME
            .iter()
            .map(|argument| argument.to_string()))
}

fn environment() -> bool {
    std::env::var("TERM").as_deref() == Ok("linux") && std::env::var_os("EGGOS_UNSET").is_none()
}

fn hash_map() -> bool {
    let text = "the quick brown fox jumps over the lazy dog the end";
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word).or_default() += 1;
    }
    counts.len() == 9 && counts["the"] == 3 && counts["fox"] == 1
}

fn heap() -> bool {
    const LENGTH: usize = 1 << 18;
    let squares: Vec<u64> = (0..LENGTH as u64).map(|value| value * value).collect();
    let small: Vec<String> = (0..1000).map(|value| value.to_string()).collect();
    squares.len() == LENGTH
        && squares[LENGTH - 1] == (LENGTH as u64 - 1).pow(2)
        && small.iter().map(String::len).sum::<usize>() == 2890
}

trait Shape {
    fn area(&self) -> f64;
}

struct Square(f64);
struct Circle(f64);

impl Shape for Square {
    fn area(&self) -> f64 {
        self.0 * self.0
    }
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.0 * self.0
    }
}

fn trait_object() -> bool {
    let shapes: Vec<Box<dyn Shape>> = vec![Box::new(Square(3.0)), Box::new(Circle(1.0))];
    let total: f64 = shapes.iter().map(|shape| shape.area()).sum();
    (total - (9.0 + std::f64::consts::PI)).abs() < 1e-12
}

fn float_formatting() -> bool {
    let root = std::hint::black_box(2.0f64).sqrt();
    format!("{root:.6} {:e} {}", 1.5e300, 0.1 + 0.2) == "1.414214 1.5e300 0.30000000000000004"
}

fn sleeping() -> bool {
    let start = Instant::now();
    std::thread::sleep(SLEEP);
    start.elapsed() >= SLEEP
}

fn threads() -> bool {
    let total = Arc::new(AtomicU64::new(0));
    let handles: Vec<_> = (1..=THREADS)
        .map(|value| {
            let total = Arc::clone(&total);
            std::thread::spawn(move || {
                total.fetch_add(value, Ordering::SeqCst);
                std::thread::current().id()
            })
        })
        .collect();
    let mut ids: Vec<_> = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect();
    ids.push(std::thread::current().id());
    ids.sort_by_key(|id| format!("{id:?}"));
    ids.dedup();
    ids.len() == THREADS as usize + 1 && total.load(Ordering::SeqCst) == THREADS * (THREADS + 1) / 2
}

fn condition_variable() -> bool {
    let shared = Arc::new((Mutex::new(None), std::sync::Condvar::new()));
    let worker = {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let (slot, ready) = &*shared;
            let mut slot = slot.lock().unwrap();
            while slot.is_none() {
                slot = ready.wait(slot).unwrap();
            }
            slot.map(|value: u64| value * 2)
        })
    };
    std::thread::sleep(SLEEP);
    let (slot, ready) = &*shared;
    *slot.lock().unwrap() = Some(21);
    ready.notify_one();
    worker.join().ok().flatten() == Some(42)
}

extern "C" fn record_handling_thread(_signal: i32) {
    // SAFETY: `gettid` has no preconditions and is async-signal-safe.
    HANDLED_BY.store(unsafe { gettid() }, Ordering::SeqCst);
}

fn handled_here() -> bool {
    // SAFETY: as in `record_handling_thread`.
    HANDLED_BY.load(Ordering::SeqCst) == unsafe { gettid() }
}

fn publish_self_as_target() {
    // SAFETY: `pthread_self` has no preconditions.
    READY_TARGET.store(unsafe { pthread_self() }, Ordering::SeqCst);
}

fn ready_target() -> usize {
    loop {
        match READY_TARGET.swap(0, Ordering::SeqCst) {
            0 => std::thread::yield_now(),
            target => break target,
        }
    }
}

fn send_when_ready() -> bool {
    let target = ready_target();
    // SAFETY: the target thread published its own `pthread_t` and is joined only after this.
    unsafe { pthread_kill(target, SIGUSR1) == 0 }
}

/// Sends `SIGUSR1` to the thread that publishes itself next, every [`SEND_INTERVAL`], until
/// `done` holds, or [`MOST_SENDS`] were sent: whether it held.
fn send_until(done: impl Fn() -> bool) -> bool {
    let target = ready_target();
    for _ in 0..MOST_SENDS {
        if done() {
            return true;
        }
        // SAFETY: the target thread published its own `pthread_t` and is joined only after
        // `done` holds or the sends end.
        unsafe { pthread_kill(target, SIGUSR1) };
        std::thread::sleep(SEND_INTERVAL);
    }
    done()
}

extern "C" fn record_handled_while_waiting(_signal: i32) {
    // SAFETY: `gettid` has no preconditions and is async-signal-safe.
    let here = unsafe { gettid() };
    if WAITING_THREAD.load(Ordering::SeqCst) == here {
        HANDLED_WHILE_WAITING.store(here, Ordering::SeqCst);
    }
}

fn start_waiting() -> i32 {
    // SAFETY: `gettid` has no preconditions.
    let here = unsafe { gettid() };
    WAITING_THREAD.store(here, Ordering::SeqCst);
    here
}

fn sleep_until_interrupted() -> bool {
    publish_self_as_target();
    loop {
        let here = start_waiting();
        let mut left = Timespec::default();
        // SAFETY: both pointers are to `struct timespec`s that live across the call.
        let slept = unsafe { nanosleep(&LONG_SLEEP, &mut left) };
        WAITING_THREAD.store(0, Ordering::SeqCst);
        if slept != 0 {
            let interrupted = std::io::Error::last_os_error().raw_os_error() == Some(EINTR);
            let left_some = (0..LONG_SLEEP.seconds).contains(&left.seconds);
            return interrupted
                && left_some
                && HANDLED_WHILE_WAITING.load(Ordering::SeqCst) == here;
        }
    }
}

fn park_until_released() -> bool {
    publish_self_as_target();
    let here = start_waiting();
    while !RELEASED.load(Ordering::SeqCst) {
        std::thread::park();
    }
    WAITING_THREAD.store(0, Ordering::SeqCst);
    HANDLED_WHILE_WAITING.load(Ordering::SeqCst) == here
}

fn signals_interrupt_waits() -> bool {
    // SAFETY: the handler only reads and stores atomics, which is async-signal-safe.
    unsafe { signal(SIGUSR1, record_handled_while_waiting) };
    HANDLED_WHILE_WAITING.store(0, Ordering::SeqCst);
    let sleeper = std::thread::spawn(sleep_until_interrupted);
    let sleep_ended = send_until(|| sleeper.is_finished());
    let sleep_interrupted = sleeper.join().unwrap_or(false);
    HANDLED_WHILE_WAITING.store(0, Ordering::SeqCst);
    RELEASED.store(false, Ordering::SeqCst);
    let parked = std::thread::spawn(park_until_released);
    let handled_in_park = send_until(|| HANDLED_WHILE_WAITING.load(Ordering::SeqCst) != 0);
    let still_parked = !parked.is_finished();
    RELEASED.store(true, Ordering::SeqCst);
    parked.thread().unpark();
    let park_handled = parked.join().unwrap_or(false);
    sleep_ended && sleep_interrupted && handled_in_park && still_parked && park_handled
}

fn signals_between_threads() -> bool {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe { signal(SIGUSR1, record_handling_thread) };
    let waiter = std::thread::spawn(|| {
        let mut blocked = [0u64; SIGSET_WORDS];
        blocked[0] = 1 << (SIGUSR1 - 1);
        // SAFETY: both sets are `sigset_t`-sized arrays that live across the calls.
        unsafe { pthread_sigmask(SIG_BLOCK, blocked.as_ptr(), std::ptr::null_mut()) };
        publish_self_as_target();
        let none = [0u64; SIGSET_WORDS];
        // SAFETY: as above.
        unsafe { sigsuspend(none.as_ptr()) };
        handled_here()
    });
    let sent_to_waiter = send_when_ready();
    let waiter_handled = waiter.join().unwrap_or(false);
    let runner = std::thread::spawn(|| {
        publish_self_as_target();
        while !handled_here() {
            std::thread::yield_now();
        }
        true
    });
    let sent_to_runner = send_when_ready();
    let runner_handled = runner.join().unwrap_or(false);
    sent_to_waiter && waiter_handled && sent_to_runner && runner_handled
}

fn change_mask(how: i32, signal: i32) {
    let mut set = [0u64; SIGSET_WORDS];
    set[0] = 1 << (signal - 1);
    // SAFETY: `set` is a `sigset_t`-sized array that lives across the call.
    unsafe { pthread_sigmask(how, set.as_ptr(), std::ptr::null_mut()) };
}

fn kill_own_process_when_ready() -> bool {
    while READY_TARGET.swap(0, Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    // SAFETY: `getpid` has no preconditions.
    let own = unsafe { getpid() };
    // SAFETY: `kill` has no preconditions.
    unsafe { kill(own, SIGUSR1) == 0 }
}

fn signals_to_the_process() -> bool {
    // SAFETY: as in `signals_between_threads`.
    unsafe { signal(SIGUSR1, record_handling_thread) };
    change_mask(SIG_BLOCK, SIGUSR1);
    let waiter = std::thread::spawn(|| {
        publish_self_as_target();
        let none = [0u64; SIGSET_WORDS];
        // SAFETY: `none` is a `sigset_t`-sized array that lives across the call.
        unsafe { sigsuspend(none.as_ptr()) };
        handled_here()
    });
    let sent_for_waiter = kill_own_process_when_ready();
    let waiter_handled = waiter.join().unwrap_or(false);
    let runner = std::thread::spawn(|| {
        change_mask(SIG_UNBLOCK, SIGUSR1);
        publish_self_as_target();
        while !handled_here() {
            std::thread::yield_now();
        }
        true
    });
    let sent_for_runner = kill_own_process_when_ready();
    let runner_handled = runner.join().unwrap_or(false);
    change_mask(SIG_UNBLOCK, SIGUSR1);
    sent_for_waiter && waiter_handled && sent_for_runner && runner_handled && !handled_here()
}

fn realtime_clock() -> bool {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok()
}

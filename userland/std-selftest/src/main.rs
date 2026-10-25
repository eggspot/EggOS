//! The kernel's boot self-tests run this program, built with Rust's standard library and
//! linked with musl, as a Linux program: it uses what a typical Rust program uses from its
//! first line (arguments, the environment, a `HashMap` with random keys, the heap, the
//! clocks, sleeping, formatting floats, threads, standard output), writes
//! `EGGOS-USER: a std program built from Rust source ran its checks` if every check passed,
//! and exits with 0; otherwise it names each check that failed and exits with 1. Given
//! [`EXIT_FROM_A_THREAD`] as its only argument, it instead ends from a thread while its other
//! threads wait ([`exit_from_a_thread`]).

use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The line written when every check passed.
pub const PASSED_LINE: &str = "EGGOS-USER: a std program built from Rust source ran its checks";

const ARGUMENTS_AFTER_PROGRAM_NAME: [&str; 2] = ["first", "second"];

/// The argument that runs [`exit_from_a_thread`] instead of the checks.
pub const EXIT_FROM_A_THREAD: &str = "exit-from-a-thread";

const SLEEP: Duration = Duration::from_millis(2);

type Check = (&'static str, fn() -> bool);

const THREADS: u64 = 4;

const CHECKS: [Check; 10] = [
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
];

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some(EXIT_FROM_A_THREAD) {
        exit_from_a_thread();
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

fn realtime_clock() -> bool {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok()
}

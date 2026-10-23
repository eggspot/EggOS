//! The kernel's boot self-tests run this program, built with Rust's standard library and
//! linked with musl, as a Linux program: it uses what a typical Rust program uses from its
//! first line (arguments, the environment, a `HashMap` with random keys, the heap, the
//! clocks, sleeping, formatting floats, standard output), writes
//! `EGGOS-USER: a std program built from Rust source ran its checks` if every check passed,
//! and exits with 0; otherwise it names each check that failed and exits with 1.

use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

/// The line written when every check passed.
pub const PASSED_LINE: &str = "EGGOS-USER: a std program built from Rust source ran its checks";

const ARGUMENTS_AFTER_PROGRAM_NAME: [&str; 2] = ["first", "second"];

const SLEEP: Duration = Duration::from_millis(2);

type Check = (&'static str, fn() -> bool);

const CHECKS: [Check; 8] = [
    ("the arguments are the program's", arguments),
    ("the environment holds TERM", environment),
    ("a HashMap with random keys counts words", hash_map),
    ("the heap holds a large vector", heap),
    ("a boxed trait object dispatches", trait_object),
    ("floats format as on Linux", float_formatting),
    ("sleeping takes at least its time", sleeping),
    ("the realtime clock reads", realtime_clock),
];

fn main() -> ExitCode {
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

fn realtime_clock() -> bool {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok()
}

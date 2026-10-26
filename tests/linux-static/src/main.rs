//! An ordinary Linux console program, built from our own source with Rust's standard library
//! and linked statically with musl by the standard toolchain, that the Linux programs boot
//! test adds to the image with `cargo xtask image --add` and runs from `eggsh` (D117). It
//! uses what typical console programs use and writes one `LINUX-STATIC` line for each:
//! its arguments, the environment, a file, a directory listing, the clocks, a `HashMap`
//! with random keys, threads, the CPUs it may use and standard error, then exits with 0.
//! `exit <status>` exits with that status instead, and `panic` panics, which aborts the
//! program.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

const PREFIX: &str = "LINUX-STATIC";
const THREADS: u32 = 4;
const WORDS: &str = "egg yolk egg shell egg";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("exit") => {
            let status = args.get(2).and_then(|status| status.parse().ok());
            return ExitCode::from(status.unwrap_or(1));
        }
        Some("panic") => panic!("{PREFIX} panicked as asked"),
        _ => {}
    }
    let name = args
        .first()
        .and_then(|path| path.rsplit('/').next())
        .unwrap_or_default();
    let arguments = args.get(1..).unwrap_or_default();
    println!("{PREFIX} {name} args {}", arguments.join("|"));
    let greeting = std::env::var("GREETING").unwrap_or_else(|_| "unset".into());
    println!("{PREFIX} env GREETING={greeting}");
    match std::fs::read_to_string("/etc/hostname") {
        Ok(text) => println!("{PREFIX} file {}", text.trim_end()),
        Err(error) => println!("{PREFIX} file error {error}"),
    }
    println!("{PREFIX} dir {}", listing("/usr/share/eggos"));
    println!("{PREFIX} time {}", clocks());
    println!("{PREFIX} hashmap {}", word_counts());
    println!("{PREFIX} threads {}", threads());
    match std::thread::available_parallelism() {
        Ok(cpus) => println!("{PREFIX} cpus {cpus}"),
        Err(error) => println!("{PREFIX} cpus error {error}"),
    }
    eprintln!("{PREFIX} stderr");
    ExitCode::SUCCESS
}

fn listing(path: &str) -> String {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => return format!("error {error}"),
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.join(" ")
}

fn clocks() -> &'static str {
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(1));
    let monotonic = started.elapsed() >= Duration::from_millis(1);
    let realtime = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok();
    match (monotonic, realtime) {
        (true, true) => "monotonic realtime",
        (false, _) => "the monotonic clock went backwards",
        (true, false) => "the realtime clock is before the epoch",
    }
}

fn word_counts() -> String {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for word in WORDS.split(' ') {
        *counts.entry(word).or_default() += 1;
    }
    let mut counts: Vec<(&str, u32)> = counts.into_iter().collect();
    counts.sort();
    counts
        .iter()
        .map(|(word, count)| format!("{word}={count}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn threads() -> String {
    let handles: Vec<_> = (1..=THREADS)
        .map(|index| std::thread::spawn(move || index * index))
        .collect();
    let mut sum = 0;
    for handle in handles {
        match handle.join() {
            Ok(square) => sum += square,
            Err(_) => return "a thread panicked".into(),
        }
    }
    format!("{THREADS} joined sum {sum}")
}

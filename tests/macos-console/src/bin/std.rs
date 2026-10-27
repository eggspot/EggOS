//! A macOS console tool written on Rust's standard library, built for the `*-apple-darwin`
//! targets and linked by Apple's linker against libSystem, that the macOS programs boot test
//! runs from `eggsh` (D123): it prints its arguments and an environment variable, reads a file,
//! lists a directory, reports a missing file's error, and runs threads that share a mutex and
//! keep thread-local counters. `exit <status>` exits with that status instead.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

const PREFIX: &str = "MACOS-STD";
const THREADS: u64 = 4;

thread_local! {
    static CALLS: Cell<u32> = const { Cell::new(0) };
}

fn count_call() -> u32 {
    CALLS.with(|calls| {
        calls.set(calls.get() + 1);
        calls.get()
    })
}

fn threads() -> String {
    let total = Arc::new(Mutex::new(0u64));
    let workers: Vec<_> = (1..=THREADS)
        .map(|value| {
            let total = Arc::clone(&total);
            std::thread::Builder::new()
                .name(format!("worker-{value}"))
                .spawn(move || {
                    count_call();
                    *total.lock().unwrap_or_else(|e| e.into_inner()) += value;
                    (
                        count_call(),
                        std::thread::current().name().map(str::to_owned),
                    )
                })
        })
        .collect();
    let mut names = Vec::new();
    let mut thread_local_calls = Vec::new();
    for worker in workers {
        match worker.map(|handle| handle.join()) {
            Ok(Ok((calls, name))) => {
                thread_local_calls.push(calls);
                names.push(name.unwrap_or_default());
            }
            _ => return "spawn or join failed".to_owned(),
        }
    }
    let sum = *total.lock().unwrap_or_else(|e| e.into_inner());
    format!(
        "{} sum {sum} tls {:?} main {} names {}",
        names.len(),
        thread_local_calls,
        count_call(),
        names.join(",")
    )
}

fn listed<'a>(names: &[String], name: &'a str) -> &'a str {
    if names.iter().any(|listed| listed == name) {
        name
    } else {
        "-"
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) == Some("exit") {
        let status = arguments.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
        std::process::exit(status);
    }
    println!("{PREFIX} hello from Rust std");
    println!("{PREFIX} args {}", arguments[1..].join("|"));
    println!(
        "{PREFIX} env GREETING={}",
        std::env::var("GREETING").unwrap_or_default()
    );
    let about = std::fs::read_to_string("/usr/share/eggos/about").unwrap_or_default();
    println!("{PREFIX} file {}", about.split_whitespace().next().unwrap_or("?"));
    let mut names: Vec<String> = std::fs::read_dir("/bin")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    println!(
        "{PREFIX} dir /bin {} {}",
        listed(&names, "eggsh"),
        listed(&names, "macos-std")
    );
    let missing = std::fs::read("/no/such/file").map_or_else(|e| format!("{:?}", e.kind()), |_| "found".to_owned());
    println!("{PREFIX} missing {missing}");
    println!("{PREFIX} threads {}", threads());
}

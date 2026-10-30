//! An ordinary Windows console program on Rust's standard library, built from our own source
//! for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` by MSVC's linker on the C
//! runtime's DLLs, that the Windows C runtime boot test runs from `eggsh` (D189). It writes one
//! `EGGRUST` line for each part of the standard library programs lean on: starting a copy of
//! itself with `Command::output` and with a piped standard output, files, threads with a
//! `Mutex`, a `Condvar` and thread-locals, sleeping and the clocks, and a `HashMap`. `child`
//! runs as that copy; `panic` panics, which prints the panic on standard error and ends with
//! status 101.

use std::cell::Cell;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

const CHILD: &str = "child";
const PANIC: &str = "panic";
const CHILD_EXIT_CODE: i32 = 7;
const FILE: &str = r"C:\tmp\eggrust.txt";
const THREADS: usize = 4;
const ADDS_PER_THREAD: u64 = 1000;
const THREAD_LOCAL_ADDED: u32 = 5;
const SLEEP: Duration = Duration::from_millis(20);
const SECONDS_TO_2020: u64 = 1_577_836_800;

thread_local! {
    static COUNTED: Cell<u32> = const { Cell::new(0) };
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some(CHILD) => child(&arguments[1..]),
        Some(PANIC) => panic!("EGGRUST panicking on purpose"),
        _ => parent(),
    }
}

fn child(words: &[String]) {
    println!("{}", words.join("|"));
    eprintln!("child error line");
    std::process::exit(CHILD_EXIT_CODE);
}

fn parent() {
    println!("EGGRUST hello from Rust's standard library");
    started_with_output();
    started_with_a_piped_output();
    files();
    threads();
    clocks();
    hash_map();
    println!("EGGRUST done");
}

fn this_program() -> Command {
    Command::new(std::env::current_exe().expect("current_exe"))
}

fn started_with_output() {
    let output = this_program()
        .args([CHILD, "two words", "q\"uote"])
        .output()
        .expect("Command::output");
    let said = String::from_utf8_lossy(&output.stdout);
    let error = String::from_utf8_lossy(&output.stderr);
    println!(
        "EGGRUST output: {}; error: {}; exit {}",
        said.trim_end(),
        error.trim_end(),
        output.status.code().unwrap_or(-1)
    );
}

fn started_with_a_piped_output() {
    let mut child = this_program()
        .args([CHILD, "read", "end"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Command::spawn");
    let mut said = String::new();
    child
        .stdout
        .take()
        .expect("piped standard output")
        .read_to_string(&mut said)
        .expect("reading the pipe");
    let status = child.wait().expect("Child::wait");
    println!(
        "EGGRUST piped: {}; exit {}",
        said.trim_end(),
        status.code().unwrap_or(-1)
    );
}

fn files() {
    std::fs::write(FILE, "hello world").expect("writing the file");
    std::fs::write(FILE, "short").expect("rewriting the file");
    let back = std::fs::read_to_string(FILE).expect("reading the file");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(FILE)
        .expect("opening the file");
    file.set_len(3).expect("File::set_len");
    drop(file);
    let cut = std::fs::read_to_string(FILE).expect("reading the cut file");
    let length = std::fs::metadata(FILE).expect("fs::metadata").len();
    std::fs::remove_file(FILE).expect("removing the file");
    let removed = !std::path::Path::new(FILE).exists();
    println!("EGGRUST file read back {back}, cut to {cut}, length {length}, removed {removed}");
}

fn threads() {
    let shared = Arc::new((Mutex::new((0u64, 0usize)), Condvar::new()));
    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                COUNTED.with(|counted| counted.set(counted.get() + THREAD_LOCAL_ADDED));
                let (state, finished) = &*shared;
                for _ in 0..ADDS_PER_THREAD {
                    state.lock().expect("the lock").0 += 1;
                }
                state.lock().expect("the lock").1 += 1;
                finished.notify_all();
                COUNTED.with(Cell::get)
            })
        })
        .collect();
    let (state, finished) = &*shared;
    let total = finished
        .wait_while(state.lock().expect("the lock"), |(_, done)| *done < THREADS)
        .expect("Condvar::wait_while")
        .0;
    let thread_local = workers
        .into_iter()
        .map(|worker| worker.join().expect("a thread"))
        .max()
        .unwrap_or(0);
    let main_local = COUNTED.with(Cell::get);
    println!(
        "EGGRUST threads {THREADS} added {total}, thread-local {thread_local} main {main_local}"
    );
}

fn clocks() {
    let start = Instant::now();
    std::thread::sleep(SLEEP);
    let slept = start.elapsed() >= SLEEP;
    let after_2020 = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok_and(|since| since.as_secs() > SECONDS_TO_2020);
    println!(
        "EGGRUST slept at least {} ms {slept}, system time after 2020 {after_2020}",
        SLEEP.as_millis()
    );
}

fn hash_map() {
    let mut map = HashMap::new();
    for (key, value) in [("one", 1), ("two", 2), ("three", 3)] {
        map.insert(key, value);
    }
    println!("EGGRUST hash map {} entries", map.len());
}

use std::ffi::{CString, c_char, c_int, c_ulong, c_void};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

const MARKER: &str = "BUILD-TIMER";
const DEFAULT_ROOT: &str = "/work";
const POWER_OFF: c_int = 0x4321_fedc;
const FIRST_PROCESS: u32 = 1;
const MOUNTS: [(&str, &str); 3] = [("proc", "/proc"), ("devtmpfs", "/dev"), ("tmpfs", "/tmp")];

unsafe extern "C" {
    fn mount(
        source: *const c_char,
        target: *const c_char,
        file_system: *const c_char,
        flags: c_ulong,
        data: *const c_void,
    ) -> c_int;
    fn reboot(command: c_int) -> c_int;
    fn sync();
    fn dup2(old: c_int, new: c_int) -> c_int;
}

struct Phase {
    name: &'static str,
    program: PathBuf,
    arguments: &'static [&'static str],
}

pub fn main() -> ExitCode {
    let first_process = std::process::id() == FIRST_PROCESS;
    if first_process {
        start_linux();
    }
    let root = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_ROOT), PathBuf::from);
    let built = build(&root);
    if first_process {
        power_off();
    }
    match built {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}

fn start_linux() {
    for (file_system, target) in MOUNTS {
        if let Err(error) = std::fs::create_dir_all(target) {
            println!("{MARKER} error mkdir {target} {error}");
        }
        let (Ok(source), Ok(at)) = (CString::new(file_system), CString::new(target)) else {
            continue;
        };
        // SAFETY: three NUL-terminated strings that outlive the call, no flags and no data.
        let mounted = unsafe {
            mount(
                source.as_ptr(),
                at.as_ptr(),
                source.as_ptr(),
                0,
                std::ptr::null(),
            )
        };
        if mounted != 0 {
            println!(
                "{MARKER} error mount {target} {}",
                std::io::Error::last_os_error()
            );
        }
    }
    if let Ok(console) = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/console")
    {
        for standard in 0..3 {
            // SAFETY: `console` is an open descriptor; replacing the standard streams with it
            // affects nothing this program holds.
            unsafe { dup2(console.as_raw_fd(), standard) };
        }
    }
}

fn power_off() -> ! {
    // SAFETY: no arguments; it only flushes the file systems' caches.
    unsafe { sync() };
    // SAFETY: the first process may power the machine off; nothing runs after it.
    unsafe { reboot(POWER_OFF) };
    loop {
        std::thread::park();
    }
}

fn environment(root: &Path, jobs: usize) -> Vec<(String, String)> {
    let at = |name: &str| root.join(name).to_string_lossy().into_owned();
    vec![
        ("PATH".into(), format!("{}:/bin:/usr/bin", at("rustc/bin"))),
        ("HOME".into(), at("")),
        ("RUSTC".into(), at("rustc/bin/rustc")),
        ("CARGO_HOME".into(), at("cargo-home")),
        ("TMPDIR".into(), at("tmp")),
        ("LD_LIBRARY_PATH".into(), at("glibc")),
        ("CARGO_INCREMENTAL".into(), "0".into()),
        ("CARGO_BUILD_JOBS".into(), jobs.to_string()),
        ("CARGO_NET_OFFLINE".into(), "true".into()),
        ("CARGO_TERM_COLOR".into(), "never".into()),
        ("CARGO_TERM_PROGRESS_WHEN".into(), "never".into()),
    ]
}

fn build(root: &Path) -> bool {
    let jobs = std::thread::available_parallelism().map_or(1, usize::from);
    println!("{MARKER} jobs {jobs}");
    let environment = environment(root, jobs);
    let workspace = root.join("workspace");
    let cargo = root.join("rustc/bin/cargo");
    let phases = [
        Phase {
            name: "rustc-version",
            program: root.join("rustc/bin/rustc"),
            arguments: &["--version"],
        },
        Phase {
            name: "build",
            program: cargo.clone(),
            arguments: &["build", "--locked"],
        },
        Phase {
            name: "rebuild",
            program: cargo.clone(),
            arguments: &["build", "--locked"],
        },
    ];
    for phase in &phases {
        if timed(phase, &workspace, &environment, None) != Some(0) {
            println!("{MARKER} done");
            return false;
        }
    }
    let run = Phase {
        name: "run",
        program: cargo,
        arguments: &["run", "--locked", "--quiet"],
    };
    let mut output = Vec::new();
    let status = timed(&run, &workspace, &environment, Some(&mut output));
    for line in String::from_utf8_lossy(&output).lines() {
        println!("{MARKER} program {line}");
    }
    println!("{MARKER} done");
    status == Some(0)
}

fn timed(
    phase: &Phase,
    folder: &Path,
    environment: &[(String, String)],
    captured: Option<&mut Vec<u8>>,
) -> Option<i32> {
    println!("{MARKER} begin {}", phase.name);
    let started = Instant::now();
    let mut command = Command::new(&phase.program);
    command
        .args(phase.arguments)
        .current_dir(folder)
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stderr(Stdio::inherit());
    let status = match captured {
        Some(output) => command.output().map(|done| {
            *output = done.stdout;
            done.status
        }),
        None => command.status(),
    };
    let milliseconds = started.elapsed().as_millis();
    let code = match &status {
        Ok(status) => status.code(),
        Err(error) => {
            println!("{MARKER} error {} {error}", phase.name);
            None
        }
    };
    println!(
        "{MARKER} end {} status {} milliseconds {milliseconds}",
        phase.name,
        code.unwrap_or(-1)
    );
    code
}

//! EggOS build, run and test automation, written in Rust so it works the same
//! on every development host (Windows, macOS, Linux). Run `cargo xtask help`.

use std::env;
use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use eggos_kernel_core::{BOOT_OK_MARKER, PANIC_MARKER};

const USAGE: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  build       Build the kernel image
  run         Boot the kernel image in QEMU with the serial console on this terminal
              (quit with Ctrl-A X)
  boot-test   Boot the kernel image in QEMU and check that it reaches the boot marker
  ci          Everything CI runs: fmt, clippy, unit tests, build and boot-test on every
              architecture, cheapest check first

Options:
  --arch <arch>   x86_64, aarch64, riscv64 or all (default: all; run needs one)
  --release       Use the release profile

QEMU is looked up on PATH, or in the directory named by EGGOS_QEMU_DIR.";

/// How long a boot test waits for the boot marker.
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);

type Result<T = ()> = std::result::Result<T, String>;

/// A CPU architecture the kernel is built and booted for.
#[derive(Clone, Copy)]
enum Arch {
    X86_64,
    Aarch64,
    Riscv64,
}

impl Arch {
    const ALL: [Arch; 3] = [Arch::X86_64, Arch::Aarch64, Arch::Riscv64];

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arch| arch.name() == name)
    }

    fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::Riscv64 => "riscv64",
        }
    }

    fn target(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64-unknown-none",
            // Soft-float until the kernel saves FP/SIMD state on context switch.
            Arch::Aarch64 => "aarch64-unknown-none-softfloat",
            Arch::Riscv64 => "riscv64gc-unknown-none-elf",
        }
    }

    /// QEMU machine options; the kernel image must suit this machine's boot
    /// protocol (see the HAL's architecture modules).
    fn qemu_machine(self) -> &'static [&'static str] {
        match self {
            Arch::X86_64 => &["-machine", "q35"],
            Arch::Aarch64 => &["-machine", "virt", "-cpu", "cortex-a72"],
            Arch::Riscv64 => &["-machine", "virt", "-bios", "default"],
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

struct Options {
    arches: Vec<Arch>,
    release: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result {
    let Some((command, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };
    match command.as_str() {
        "build" => {
            let options = parse_options(rest)?;
            options
                .arches
                .iter()
                .try_for_each(|&arch| build_kernel(arch, options.release).map(drop))
        }
        "run" => {
            let options = parse_options(rest)?;
            let [arch] = options.arches[..] else {
                return Err("run needs exactly one --arch".into());
            };
            let kernel = build_kernel(arch, options.release)?;
            let status = qemu(arch, &kernel, "mon:stdio").status().map_err(|error| {
                format!("cannot start {}: {error}", qemu_program(arch).display())
            })?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("QEMU exited with {status}"))
            }
        }
        "boot-test" => {
            let options = parse_options(rest)?;
            boot_test_all(&options)
        }
        "ci" => ci(rest),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

fn parse_options(args: &[String]) -> Result<Options> {
    let mut options = Options {
        arches: Arch::ALL.to_vec(),
        release: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--arch" => {
                let name = args.next().ok_or("--arch needs a value")?;
                options.arches = match name.as_str() {
                    "all" => Arch::ALL.to_vec(),
                    name => vec![
                        Arch::parse(name)
                            .ok_or_else(|| format!("unknown architecture `{name}`"))?,
                    ],
                };
            }
            "--release" => options.release = true,
            other => return Err(format!("unknown option `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

fn ci(args: &[String]) -> Result {
    let options = parse_options(args)?;
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&["clippy", "--all-targets", "--", "-D", "warnings"])?;
    cargo(&["test"])?;
    for &arch in &options.arches {
        cargo(&[
            "clippy",
            "--package",
            "eggos-kernel",
            "--target",
            arch.target(),
            "--",
            "-D",
            "warnings",
        ])?;
    }
    boot_test_all(&options)
}

fn boot_test_all(options: &Options) -> Result {
    for &arch in &options.arches {
        let kernel = build_kernel(arch, options.release)?;
        boot_test(arch, &kernel)?;
        println!("xtask: {arch}: boot test passed");
    }
    Ok(())
}

/// Build the kernel image for `arch` and return its path.
fn build_kernel(arch: Arch, release: bool) -> Result<PathBuf> {
    let mut args = vec![
        "build",
        "--package",
        "eggos-kernel",
        "--target",
        arch.target(),
    ];
    if release {
        args.push("--release");
    }
    cargo(&args)?;
    let profile = if release { "release" } else { "debug" };
    Ok(target_dir()
        .join(arch.target())
        .join(profile)
        .join("eggos-kernel"))
}

/// Boot `kernel` headless and wait for the boot marker on the serial console.
fn boot_test(arch: Arch, kernel: &Path) -> Result {
    println!("xtask: {arch}: booting {}", kernel.display());
    let mut qemu = qemu(arch, kernel, "stdio")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", qemu_program(arch).display()))?;
    let serial = qemu.stdout.take().expect("stdout is piped");
    let (lines_tx, lines_rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(serial).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line).trim_end().to_owned();
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + BOOT_TIMEOUT;
    let outcome = loop {
        match lines_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                println!("  {arch} | {line}");
                if line.contains(BOOT_OK_MARKER) {
                    break Ok(());
                }
                if line.contains(PANIC_MARKER) {
                    break Err(format!("{arch}: the kernel panicked"));
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                break Err(format!(
                    "{arch}: no boot marker within {}s",
                    BOOT_TIMEOUT.as_secs()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                break Err(format!("{arch}: QEMU exited before the boot marker"));
            }
        }
    };
    // The kernel halts rather than powering off, so stop QEMU ourselves.
    let _ = qemu.kill();
    let _ = qemu.wait();
    outcome
}

/// The QEMU command that boots `kernel` with its serial console on `serial`.
fn qemu(arch: Arch, kernel: &Path, serial: &str) -> Command {
    let mut command = Command::new(qemu_program(arch));
    command
        .args(arch.qemu_machine())
        .args([
            "-m",
            "128M",
            "-nodefaults",
            "-display",
            "none",
            "-no-reboot",
            "-serial",
            serial,
        ])
        .arg("-kernel")
        .arg(kernel);
    command
}

fn qemu_program(arch: Arch) -> PathBuf {
    let program = format!("qemu-system-{arch}");
    match env::var_os("EGGOS_QEMU_DIR") {
        Some(dir) => Path::new(&dir).join(program),
        None => PathBuf::from(program),
    }
}

/// Run cargo with `args` in the workspace root, failing if it fails.
fn cargo(args: &[&str]) -> Result {
    println!("xtask: cargo {}", args.join(" "));
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed", args.join(" ")))
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_owned()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| workspace_root().join("target"), PathBuf::from)
}

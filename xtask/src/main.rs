//! Developer commands for EggOS, run with `cargo xtask <command>`.
//!
//! Written in Rust (no shell scripts) so the same commands work on macOS, Windows and Linux,
//! locally and in CI.

mod qemu;

use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command> [options]

commands:
  ci                     every check CI runs, cheapest first: fmt, clippy (host and kernel
                         targets), host tests, QEMU boot tests on every architecture
  clippy-targets         clippy -D warnings on the kernel crates for every kernel target
  build --arch <arch>    build the kernel image for <arch>
  qemu  --arch <arch>    boot the kernel in QEMU headless and check its self-test verdict
                         (<arch> may be `all`)
  run   --arch <arch>    boot the kernel in QEMU interactively (quit: Ctrl-A then X)

architectures: aarch64, riscv64, x86_64";

/// A kernel architecture: its Rust target and how to boot it in QEMU.
#[derive(Debug)]
pub struct Arch {
    pub name: &'static str,
    pub target: &'static str,
    pub qemu: &'static str,
    pub machine_args: &'static [&'static str],
}

/// Every architecture brought up so far, cheapest to test first.
pub const ARCHES: &[Arch] = &[
    Arch {
        name: "aarch64",
        target: "aarch64-unknown-none-softfloat",
        qemu: "qemu-system-aarch64",
        machine_args: &["-machine", "virt", "-cpu", "cortex-a72"],
    },
    Arch {
        name: "riscv64",
        target: "riscv64gc-unknown-none-elf",
        qemu: "qemu-system-riscv64",
        machine_args: &["-machine", "virt", "-bios", "default"],
    },
    Arch {
        name: "x86_64",
        target: "x86_64-unknown-none",
        qemu: "qemu-system-x86_64",
        machine_args: &["-machine", "q35"],
    },
];

/// Crates compiled for kernel targets (everything but this host tool).
const KERNEL_CRATES: &[&str] = &["eggos-kernel", "eggos-hal", "eggos-drivers"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            if message.starts_with("usage") {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str);
    let arch = || arch_option(&args[1..]);
    match command {
        Some("ci") => ci(),
        Some("clippy-targets") => clippy_targets(),
        Some("build") => {
            for arch in arch()? {
                build_kernel(arch, false)?;
            }
            Ok(())
        }
        Some("qemu") => {
            for arch in arch()? {
                let kernel = build_kernel(arch, true)?;
                qemu::boot_test(arch, &kernel)?;
            }
            Ok(())
        }
        Some("run") => {
            let arches = arch()?;
            let [arch] = arches.as_slice() else {
                return Err("`run` boots one architecture; pass a single --arch".into());
            };
            let kernel = build_kernel(arch, false)?;
            qemu::run_interactive(arch, &kernel)
        }
        _ => Err(USAGE.into()),
    }
}

/// Parses `--arch <name|all>`.
fn arch_option(args: &[String]) -> Result<Vec<&'static Arch>, String> {
    let [flag, name] = args else {
        return Err(USAGE.into());
    };
    if flag != "--arch" {
        return Err(USAGE.into());
    }
    if name == "all" {
        return Ok(ARCHES.iter().collect());
    }
    ARCHES
        .iter()
        .find(|a| a.name == name)
        .map(|a| vec![a])
        .ok_or_else(|| format!("unknown architecture `{name}`\n\n{USAGE}"))
}

/// Runs the same checks as CI, in the same order (cheapest first).
fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    clippy_targets()?;
    cargo(&["test", "--workspace"])?;
    for arch in ARCHES {
        let kernel = build_kernel(arch, true)?;
        qemu::boot_test(arch, &kernel)?;
    }
    Ok(())
}

/// Lints the kernel crates as compiled for each bare-metal target, where the architecture
/// modules (invisible to a host build) are compiled. With and without `qemu-test`, since the
/// feature changes code paths.
fn clippy_targets() -> Result<(), String> {
    for arch in ARCHES {
        for features in [&[][..], &["--features", "eggos-kernel/qemu-test"]] {
            let mut args = vec!["clippy", "--target", arch.target];
            for krate in KERNEL_CRATES {
                args.extend(["-p", krate]);
            }
            args.extend(features);
            args.extend(["--", "-D", "warnings"]);
            cargo(&args)?;
        }
    }
    Ok(())
}

/// Builds the kernel image for `arch` (release profile) and returns its path.
fn build_kernel(arch: &Arch, qemu_test: bool) -> Result<std::path::PathBuf, String> {
    let mut args = vec![
        "build",
        "--release",
        "-p",
        "eggos-kernel",
        "--target",
        arch.target,
    ];
    if qemu_test {
        args.extend(["--features", "qemu-test"]);
    }
    cargo(&args)?;
    Ok(workspace_root()
        .join("target")
        .join(arch.target)
        .join("release")
        .join("eggos-kernel"))
}

fn workspace_root() -> std::path::PathBuf {
    let xtask_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

/// Runs `cargo <args>` in the workspace root with the toolchain that is running this xtask.
fn cargo(args: &[&str]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("xtask: cargo {}", args.join(" "));
    let status = Command::new(&cargo)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("could not run {cargo}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed ({status})", args.join(" ")))
    }
}

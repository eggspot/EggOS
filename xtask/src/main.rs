//! Developer commands for EggOS, run with `cargo xtask <command>`.
//!
//! Written in Rust (no shell scripts) so the same commands work on macOS, Windows and Linux,
//! locally and in CI. What each command does and the flags our Linux programs are built with:
//! docs/build.md, "Developer commands".

mod bench;
mod elf;
mod initramfs;
mod qemu;
mod qmp;

use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command> [options]

commands:
  ci                     every check CI runs, cheapest first: fmt, clippy (host and kernel
                         targets), docs, host tests, QEMU boot tests and benchmarks on
                         every architecture
  clippy-targets         clippy -D warnings on the user crates for their Linux targets
                         and the kernel crates for every kernel target, with the user
                         programs built for the test kernels
  docs                   build the workspace's API docs with rustdoc, warnings as errors
  build --arch <arch>    build the kernel image for <arch>
  qemu  --arch <arch>    boot the kernel in QEMU headless and check its self-test verdict,
                         then boot it again to check that writing read-only data faults
                         and the fatal exception is reported
                         (<arch> may be `all`)
  run   --arch <arch> [--nographic]
                         boot the kernel in QEMU interactively, its display in a window
                         and its console on this terminal (quit: Ctrl-A then X); with
                         --nographic, no window
  bench --arch <arch> [--update-baseline]
                         boot the benchmark kernel in QEMU (deterministic instruction counts)
                         and fail on a regression against benchmarks/baseline.txt; with
                         --update-baseline, record the results there instead

architectures: aarch64, riscv64, x86_64";

/// A kernel architecture: its Rust target and how to boot it in QEMU.
#[derive(Debug)]
pub struct Arch {
    pub name: &'static str,
    pub target: &'static str,
    /// Rust's Linux target for the architecture, which our own Linux programs are built for.
    pub user_target: &'static str,
    pub qemu: &'static str,
    pub machine_args: &'static [&'static str],
    /// The display device every boot but the no-display ones adds: a virtio GPU asking for
    /// 1280x800 (D112).
    pub display_args: &'static [&'static str],
}

/// Every architecture brought up so far, cheapest to test first.
pub const ARCHES: &[Arch] = &[
    Arch {
        name: "aarch64",
        target: "aarch64-unknown-none-softfloat",
        user_target: "aarch64-unknown-linux-musl",
        qemu: "qemu-system-aarch64",
        machine_args: &[
            "-machine",
            "virt,gic-version=2",
            "-cpu",
            "cortex-a72",
            "-global",
            "virtio-mmio.force-legacy=false",
            "-device",
            "virtio-rng-device",
        ],
        display_args: &["-device", "virtio-gpu-device,xres=1280,yres=800"],
    },
    Arch {
        name: "riscv64",
        target: "riscv64imac-unknown-none-elf",
        user_target: "riscv64gc-unknown-linux-musl",
        qemu: "qemu-system-riscv64",
        machine_args: &[
            "-machine",
            "virt",
            "-bios",
            "default",
            "-global",
            "virtio-mmio.force-legacy=false",
            "-device",
            "virtio-rng-device",
        ],
        display_args: &["-device", "virtio-gpu-device,xres=1280,yres=800"],
    },
    Arch {
        name: "x86_64",
        target: "x86_64-unknown-none",
        user_target: "x86_64-unknown-linux-musl",
        qemu: "qemu-system-x86_64",
        machine_args: &[
            "-machine",
            "q35",
            "-cpu",
            "qemu64,+rdrand,+rdseed",
            "-device",
            "virtio-rng-pci,disable-legacy=on",
        ],
        display_args: &[
            "-vga",
            "none",
            "-device",
            "virtio-gpu-pci,disable-legacy=on,xres=1280,yres=800",
        ],
    },
];

const KERNEL_CRATES: &[&str] = &["eggos-kernel", "eggos-hal", "eggos-drivers"];

const USER_CRATES: &[&str] = &[
    "eggos-runtime",
    "eggos-init",
    "eggos-eggsh",
    "eggos-coreutils",
    "eggos-selftest",
    "eggos-std-selftest",
];

struct UserProgram {
    package: &'static str,
    variable: &'static str,
    rustflags: &'static [&'static str],
}

const USER_PROGRAMS: [UserProgram; 2] = [
    UserProgram {
        package: "eggos-selftest",
        variable: "EGGOS_USERLAND_SELFTEST",
        rustflags: RUNTIME_PROGRAM_RUSTFLAGS,
    },
    UserProgram {
        package: "eggos-std-selftest",
        variable: "EGGOS_USERLAND_STD_SELFTEST",
        rustflags: STD_PROGRAM_RUSTFLAGS,
    },
];

const RUNTIME_PROGRAM_RUSTFLAGS: &[&str] =
    &["-C", "link-self-contained=no", "-C", "relocation-model=pie"];

const INIT_RUSTFLAGS: &[&str] = &[
    "-C",
    "link-self-contained=no",
    "-C",
    "relocation-model=pie",
    "-C",
    "debuginfo=0",
    "-C",
    "strip=symbols",
];

const INIT_PATH: &str = "init";

const SHELL_PATH: &str = "bin/eggsh";

const COREUTILS_PATH: &str = "bin/coreutils";

const COREUTILS: [&str; 10] = [
    "cat", "echo", "false", "halt", "ls", "poweroff", "reboot", "sleep", "true", "uname",
];

const SHELL_TEST_CONFIG: &str = "login-exit=poweroff\n";

const SHELL_TEST_SCRIPT_PATH: &str = "usr/share/eggsh/test-script";

const SHELL_TEST_SCRIPT: &str = "# the shell boot test's script (D107)\n\
    echo EGGSH-TEST script $0 $1 $#\n\
    . /usr/share/eggsh/sourced\n\
    echo EGGSH-TEST sourced $SOURCED\n\
    exit 5\n";

const SHELL_TEST_SOURCED_PATH: &str = "usr/share/eggsh/sourced";

const SHELL_TEST_SOURCED: &str = "SOURCED=yes\n";

const INIT_TEST_LOGIN_PATH: &str = "usr/libexec/eggos-std-selftest";

const INIT_TEST_LOGIN: &str = "login=/usr/libexec/eggos-std-selftest init-login\n";

const STD_PROGRAM_RUSTFLAGS: &[&str] = &[
    "-C",
    "link-self-contained=yes",
    "-C",
    "target-feature=+crt-static",
    "-C",
    "relocation-model=pie",
    "-C",
    "debuginfo=0",
    "-C",
    "strip=symbols",
];

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
        Some("docs") => docs(),
        Some("build") => {
            for arch in arch()? {
                build_kernel(arch, None)?;
            }
            Ok(())
        }
        Some("qemu") => {
            for arch in arch()? {
                boot_tests(arch)?;
            }
            Ok(())
        }
        Some("bench") => {
            let (options, update) = match &args[1..] {
                [options @ .., last] if last == "--update-baseline" => (options, true),
                options => (options, false),
            };
            bench::run(&arch_option(options)?, update)
        }
        Some("run") => {
            let (options, window) = match &args[1..] {
                [options @ .., last] if last == "--nographic" => (options, false),
                options => (options, true),
            };
            let arches = arch_option(options)?;
            let [arch] = arches.as_slice() else {
                return Err("`run` boots one architecture; pass a single --arch".into());
            };
            let kernel = build_kernel(arch, None)?;
            qemu::run_interactive(arch, &kernel, window)
        }
        _ => Err(USAGE.into()),
    }
}

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
    docs()?;
    cargo(&["test", "--workspace"])?;
    for arch in ARCHES {
        boot_tests(arch)?;
    }
    bench::run(&ARCHES.iter().collect::<Vec<_>>(), false)
}

fn boot_tests(arch: &Arch) -> Result<(), String> {
    for (feature, expect) in [
        ("qemu-test", qemu::Expect::Pass),
        ("qemu-test-fatal-exception", qemu::Expect::FatalException),
    ] {
        let kernel = build_kernel(arch, Some(feature))?;
        let poison = qemu::bss_poison_args(&kernel)?;
        let poison: Vec<&str> = poison.iter().map(String::as_str).collect();
        qemu::boot_test(
            arch,
            &kernel,
            expect,
            qemu::TEST_CPUS,
            qemu::Display::Attached,
            &poison,
        )?;
    }
    for (login_exit, expect, args) in [
        ("poweroff", qemu::Expect::Init, &[][..]),
        ("restart", qemu::Expect::InitRestart, &["-no-reboot"][..]),
    ] {
        let kernel = build_init_test_kernel(arch, login_exit)?;
        qemu::boot_test(
            arch,
            &kernel,
            expect,
            qemu::TEST_CPUS,
            qemu::Display::Absent,
            args,
        )?;
    }
    let kernel = build_shell_test_kernel(arch)?;
    qemu::boot_test(
        arch,
        &kernel,
        qemu::Expect::Shell,
        qemu::TEST_CPUS,
        qemu::Display::Attached,
        &[],
    )?;
    let kernel = build_kernel(arch, None)?;
    qemu::boot_test(
        arch,
        &kernel,
        qemu::Expect::BootToPrompt,
        qemu::TEST_CPUS,
        qemu::Display::Attached,
        &[],
    )?;
    Ok(())
}

fn clippy_targets() -> Result<(), String> {
    for arch in ARCHES {
        let mut args = vec!["clippy", "--target", arch.user_target];
        for krate in USER_CRATES {
            args.extend(["-p", krate]);
        }
        args.extend(["--", "-D", "warnings"]);
        cargo(&args)?;
        let mut programs = build_user_programs(arch)?;
        programs.push(initramfs_variable(arch, &programs)?);
        let env = program_variables(&programs);
        for features in [
            &[][..],
            &["--features", "eggos-kernel/qemu-test"],
            &["--features", "eggos-kernel/qemu-test-fatal-exception"],
            &["--features", "eggos-kernel/qemu-bench"],
        ] {
            let mut args = vec!["clippy", "--target", arch.target];
            for krate in KERNEL_CRATES {
                args.extend(["-p", krate]);
            }
            args.extend(features);
            args.extend(["--", "-D", "warnings"]);
            cargo_with_env(&args, &env)?;
        }
    }
    Ok(())
}

/// Builds the kernel image for `arch` (release profile) with the kernel feature `feature`, if
/// any, and returns its path. The image embeds the initramfs ([`initramfs::build`]): without a
/// feature, the release kernel's, with init, the shell and the core utilities; every kernel
/// feature is a test kernel's, which runs the programs [`build_user_programs`] builds first.
pub fn build_kernel(arch: &Arch, feature: Option<&str>) -> Result<std::path::PathBuf, String> {
    let Some(feature) = feature else {
        let init = build_init(arch)?;
        let shell = build_shell(arch)?;
        let coreutils = build_runtime_program(arch, "eggos-coreutils")?;
        let links: Vec<String> = COREUTILS.iter().map(|name| format!("bin/{name}")).collect();
        let mut members = vec![
            (INIT_PATH, initramfs::Content::Program(&init)),
            (SHELL_PATH, initramfs::Content::Program(&shell)),
            (COREUTILS_PATH, initramfs::Content::Program(&coreutils)),
        ];
        members.extend(
            links
                .iter()
                .map(|link| (link.as_str(), initramfs::Content::HardLink(COREUTILS_PATH))),
        );
        let initramfs =
            initramfs::build(&members, &format!("initramfs-{}.cpio", arch.user_target))?;
        return build_release_kernel(arch, &initramfs);
    };
    let args = [
        "build",
        "-p",
        "eggos-kernel",
        "--target",
        arch.target,
        "--features",
        feature,
    ];
    let mut programs = build_user_programs(arch)?;
    programs.push(initramfs_variable(arch, &programs)?);
    let env = program_variables(&programs);
    build_executable(
        &args,
        &env,
        &format!("kernel executable for {}", arch.target),
    )
}

fn build_release_kernel(
    arch: &Arch,
    initramfs: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let initramfs = initramfs.to_string_lossy();
    build_executable(
        &["build", "-p", "eggos-kernel", "--target", arch.target],
        &[("EGGOS_INITRAMFS", &initramfs)],
        &format!("kernel executable for {}", arch.target),
    )
}

fn build_init_test_kernel(arch: &Arch, login_exit: &str) -> Result<std::path::PathBuf, String> {
    let init = build_init(arch)?;
    let programs = build_user_programs(arch)?;
    let login = programs
        .iter()
        .find(|(variable, _)| *variable == "EGGOS_USERLAND_STD_SELFTEST")
        .map(|(_, path)| path.as_str())
        .ok_or("the std program was not built")?;
    let config = format!("{INIT_TEST_LOGIN}login-exit={login_exit}\n");
    let initramfs = initramfs::build(
        &[
            (INIT_PATH, initramfs::Content::Program(&init)),
            ("etc/eggos/init.conf", initramfs::Content::Text(&config)),
            (INIT_TEST_LOGIN_PATH, initramfs::Content::Program(login)),
        ],
        &format!("initramfs-init-{login_exit}-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs)
}

fn build_shell_test_kernel(arch: &Arch) -> Result<std::path::PathBuf, String> {
    let init = build_init(arch)?;
    let shell = build_shell(arch)?;
    let programs = build_user_programs(arch)?;
    let commands = programs
        .iter()
        .find(|(variable, _)| *variable == "EGGOS_USERLAND_STD_SELFTEST")
        .map(|(_, path)| path.as_str())
        .ok_or("the std program was not built")?;
    let initramfs = initramfs::build(
        &[
            (INIT_PATH, initramfs::Content::Program(&init)),
            (
                "etc/eggos/init.conf",
                initramfs::Content::Text(SHELL_TEST_CONFIG),
            ),
            (SHELL_PATH, initramfs::Content::Program(&shell)),
            (INIT_TEST_LOGIN_PATH, initramfs::Content::Program(commands)),
            (
                SHELL_TEST_SCRIPT_PATH,
                initramfs::Content::Text(SHELL_TEST_SCRIPT),
            ),
            (
                SHELL_TEST_SOURCED_PATH,
                initramfs::Content::Text(SHELL_TEST_SOURCED),
            ),
        ],
        &format!("initramfs-shell-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs)
}

fn build_init(arch: &Arch) -> Result<String, String> {
    build_runtime_program(arch, "eggos-init")
}

fn build_shell(arch: &Arch) -> Result<String, String> {
    build_runtime_program(arch, "eggos-eggsh")
}

fn build_runtime_program(arch: &Arch, package: &str) -> Result<String, String> {
    let rustflags = INIT_RUSTFLAGS.join("\x1f");
    let path = build_executable(
        &["build", "-p", package, "--target", arch.user_target],
        &[("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str())],
        &format!("{package} executable for {}", arch.user_target),
    )?;
    Ok(path.to_string_lossy().into_owned())
}

/// Builds [`USER_PROGRAMS`] for `arch`'s Linux target (release profile) and returns each
/// one's variable and path.
pub fn build_user_programs(arch: &Arch) -> Result<Vec<(&'static str, String)>, String> {
    USER_PROGRAMS
        .iter()
        .map(|program| {
            let rustflags = program.rustflags.join("\x1f");
            let path = build_executable(
                &["build", "-p", program.package, "--target", arch.user_target],
                &[("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str())],
                &format!("{} executable for {}", program.package, arch.user_target),
            )?;
            Ok((program.variable, path.to_string_lossy().into_owned()))
        })
        .collect()
}

const INITRAMFS_PROGRAMS: [(&str, &str); 2] = [
    ("EGGOS_USERLAND_SELFTEST", "eggos-selftest"),
    ("EGGOS_USERLAND_STD_SELFTEST", "eggos-std-selftest"),
];

fn initramfs_variable(
    arch: &Arch,
    programs: &[(&'static str, String)],
) -> Result<(&'static str, String), String> {
    let paths: Vec<(String, &str)> = programs
        .iter()
        .filter_map(|(variable, path)| {
            INITRAMFS_PROGRAMS
                .iter()
                .find(|(program, _)| program == variable)
                .map(|(_, name)| (format!("usr/libexec/{name}"), path.as_str()))
        })
        .collect();
    let members: Vec<(&str, initramfs::Content)> = paths
        .iter()
        .map(|(name, path)| (name.as_str(), initramfs::Content::Program(path)))
        .collect();
    let path = initramfs::build(
        &members,
        &format!("initramfs-test-{}.cpio", arch.user_target),
    )?;
    Ok(("EGGOS_INITRAMFS", path.to_string_lossy().into_owned()))
}

fn program_variables<'a>(programs: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    programs
        .iter()
        .map(|(variable, path)| (*variable, path.as_str()))
        .collect()
}

fn build_executable(
    args: &[&str],
    env: &[(&str, &str)],
    what: &str,
) -> Result<std::path::PathBuf, String> {
    let mut args = args.to_vec();
    args.extend(["--release", "--message-format=json-render-diagnostics"]);
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("xtask: cargo {}", args.join(" "));
    let output = Command::new(&cargo_bin)
        .args(&args)
        .envs(env.iter().copied())
        .current_dir(workspace_root())
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("could not run {cargo_bin}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo {}` failed ({})",
            args.join(" "),
            output.status
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| {
            l.contains(r#""reason":"compiler-artifact""#) && l.contains(r#""kind":["bin"]"#)
        })
        .filter_map(|l| json_string_field(l, "executable"))
        .next_back()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| format!("cargo reported no {what}"))
}

fn json_string_field(line: &str, field: &str) -> Option<String> {
    let start = line.find(&format!("\"{field}\":\""))? + field.len() + 4;
    let mut value = String::new();
    let mut chars = line[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    value.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => value.push(other),
            },
            c => value.push(c),
        }
    }
    None
}

pub fn workspace_root() -> std::path::PathBuf {
    let xtask_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    xtask_dir
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn docs() -> Result<(), String> {
    cargo_with_env(
        &["doc", "--workspace", "--no-deps"],
        &[("CARGO_ENCODED_RUSTDOCFLAGS", "-D\u{1f}warnings")],
    )
}

fn cargo(args: &[&str]) -> Result<(), String> {
    cargo_with_env(args, &[])
}

fn cargo_with_env(args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("xtask: cargo {}", args.join(" "));
    let status = Command::new(&cargo)
        .args(args)
        .envs(env.iter().copied())
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("could not run {cargo}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed ({status})", args.join(" ")))
    }
}

#[cfg(test)]
mod tests {
    use super::json_string_field;

    #[test]
    fn reads_executable_path_from_cargo_json() {
        let unix = r#"{"reason":"compiler-artifact","executable":"/w/target/x/release/eggos-kernel","fresh":true}"#;
        assert_eq!(
            json_string_field(unix, "executable").as_deref(),
            Some("/w/target/x/release/eggos-kernel")
        );
        let windows = r#"{"executable":"C:\\w\\target\\eggos-kernel"}"#;
        assert_eq!(
            json_string_field(windows, "executable").as_deref(),
            Some(r"C:\w\target\eggos-kernel")
        );
        assert_eq!(
            json_string_field(r#"{"executable":null}"#, "executable"),
            None
        );
    }
}

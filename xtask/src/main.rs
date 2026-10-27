//! Developer commands for EggOS, run with `cargo xtask <command>`.
//!
//! Written in Rust (no shell scripts) so the same commands work on macOS, Windows and Linux,
//! locally and in CI. What each command does and the flags our Linux programs are built with:
//! docs/build.md, "Developer commands".

mod android;
mod bench;
mod elf;
mod image;
mod initramfs;
mod macos;
mod png;
mod qemu;
mod qmp;
mod release;
mod sha256;

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
  image --arch <arch> [--add <file> [--as <path>]]...
                         build the release image with each file added to its initramfs at
                         /bin/<file's name> or <path>, executable, and write it to
                         target/image/eggos-<arch>.elf (<arch> may be `all`)
  run   --arch <arch> [--nographic] [--add <file> [--as <path>]]...
                         boot the kernel in QEMU interactively, its display in a window
                         with a keyboard and a pointer, and its console on this terminal
                         (quit: Ctrl-A then X); with --nographic, no window; with --add,
                         the release image with those files, as `image` builds it
  bench --arch <arch> [--update-baseline]
                         boot the benchmark kernel in QEMU (deterministic instruction counts)
                         and fail on a regression against benchmarks/baseline.txt; with
                         --update-baseline, record the results there instead
  release                build and boot the release image of every architecture, then write
                         eggos-<version>-<arch>.elf, SHA256SUMS, run-eggos.sh, run-eggos.ps1
                         and RELEASE-NOTES.md to target/release-dist/
  screenshot --arch <arch>
                         boot the release image headless and save its desktop to
                         docs/images/desktop.png
  macos-programs         build the macOS programs boot test's Mach-O programs with Apple's
                         linker (macOS hosts only) into target/macos-console/programs and
                         print that folder, which EGGOS_MACOS_PROGRAMS can name elsewhere

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
    /// The input devices every boot with a display adds: a virtio keyboard, tablet and mouse
    /// (D113).
    pub input_args: &'static [&'static str],
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
        input_args: MMIO_INPUT_ARGS,
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
        input_args: MMIO_INPUT_ARGS,
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
            "qemu64,+rdrand,+rdseed,+ssse3,+sse4.1,+sse4.2,+popcnt",
            "-device",
            "virtio-rng-pci,disable-legacy=on",
        ],
        display_args: &[
            "-vga",
            "none",
            "-device",
            "virtio-gpu-pci,disable-legacy=on,xres=1280,yres=800",
        ],
        input_args: &[
            "-device",
            "virtio-keyboard-pci,disable-legacy=on",
            "-device",
            "virtio-tablet-pci,disable-legacy=on",
            "-device",
            "virtio-mouse-pci,disable-legacy=on",
        ],
    },
];

const MMIO_INPUT_ARGS: &[&str] = &[
    "-device",
    "virtio-keyboard-device",
    "-device",
    "virtio-tablet-device",
    "-device",
    "virtio-mouse-device",
];

const KERNEL_CRATES: &[&str] = &["eggos-kernel", "eggos-hal", "eggos-drivers"];

const USER_CRATES: &[&str] = &[
    "eggos-runtime",
    "eggos-init",
    "eggos-eggsh",
    "eggos-coreutils",
    "eggos-selftest",
    "eggos-std-selftest",
    "eggos-linux-static",
    "eggos-windows",
    "eggos-macos",
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
        Some("macos-programs") if args.len() == 1 => {
            let programs = macos::find_or_build_programs(&target_directory())?.ok_or(
                "Apple's linker builds the Mach-O test programs: run this on a macOS host",
            )?;
            println!("{}", programs.directory.display());
            Ok(())
        }
        Some("release") if args.len() == 1 => release::run(),
        Some("screenshot") => {
            let arches = arch_option(&args[1..])?;
            let [arch] = arches.as_slice() else {
                return Err("`screenshot` boots one architecture; pass a single --arch".into());
            };
            release::screenshot(arch)
        }
        Some("image") => {
            let (options, added) = image::added_files(&args[1..])?;
            for arch in arch_option(&options)? {
                let image = image::build(arch, &added)?;
                println!("{}", image.display());
            }
            Ok(())
        }
        Some("run") => {
            let (options, added) = image::added_files(&args[1..])?;
            let (options, nographic) = without_flag(&options, "--nographic");
            let arches = arch_option(&options)?;
            let [arch] = arches.as_slice() else {
                return Err("`run` boots one architecture; pass a single --arch".into());
            };
            let kernel = build_release_image(arch, &added)?;
            qemu::run_interactive(arch, &kernel, !nographic)
        }
        _ => Err(USAGE.into()),
    }
}

fn without_flag(args: &[String], flag: &str) -> (Vec<String>, bool) {
    let rest: Vec<String> = args.iter().filter(|arg| *arg != flag).cloned().collect();
    let found = rest.len() != args.len();
    (rest, found)
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
    let kernel = build_linux_programs_test_kernel(arch)?;
    qemu::boot_test(
        arch,
        &kernel,
        qemu::Expect::LinuxPrograms,
        qemu::TEST_CPUS,
        qemu::Display::Absent,
        &[],
    )?;
    let kernel = build_windows_programs_test_kernel(arch)?;
    qemu::boot_test(
        arch,
        &kernel,
        qemu::Expect::WindowsPrograms,
        qemu::TEST_CPUS,
        qemu::Display::Absent,
        &[],
    )?;
    if let Some(kernel) = build_android_programs_test_kernel(arch)? {
        qemu::boot_test(
            arch,
            &kernel,
            qemu::Expect::AndroidPrograms,
            qemu::TEST_CPUS,
            qemu::Display::Absent,
            &[],
        )?;
    }
    if let Some(kernel) = build_macos_programs_test_kernel(arch)? {
        qemu::boot_test(
            arch,
            &kernel,
            qemu::Expect::MacosPrograms,
            qemu::TEST_CPUS,
            qemu::Display::Absent,
            &[],
        )?;
    }
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
    cargo(&[
        "clippy",
        "--target",
        WINDOWS_TARGET,
        "-p",
        WINDOWS_CONSOLE_PACKAGE,
        "--",
        "-D",
        "warnings",
    ])?;
    for target in ARCHES.iter().filter_map(|arch| android::target(arch.name)) {
        cargo(&[
            "clippy",
            "--target",
            target,
            "-p",
            ANDROID_CONSOLE_PACKAGE,
            "--",
            "-D",
            "warnings",
        ])?;
    }
    for (_, target) in ARCHES.iter().filter_map(|arch| macos::target(arch.name)) {
        cargo(&[
            "clippy",
            "--target",
            target,
            "-p",
            macos::CONSOLE_PACKAGE,
            "--",
            "-D",
            "warnings",
        ])?;
    }
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
            &["--features", "eggos-kernel/qemu-boot-clock"],
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
        let initramfs = build_release_initramfs(arch)?;
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

fn build_release_initramfs(arch: &Arch) -> Result<std::path::PathBuf, String> {
    release_initramfs(arch, &[], &format!("initramfs-{}.cpio", arch.user_target))
}

/// Builds the release kernel for `arch` with `added` in its initramfs and returns its path.
pub fn build_release_image(
    arch: &Arch,
    added: &[image::AddedFile],
) -> Result<std::path::PathBuf, String> {
    if added.is_empty() {
        return build_kernel(arch, None);
    }
    if let Some(missing) = added
        .iter()
        .find(|file| !std::path::Path::new(&file.host).is_file())
    {
        return Err(format!("`{}` is not a file to add", missing.host));
    }
    let layer = windows_layer_for(arch, added)?;
    let macos_layer = macos_layer_for(arch, added)?;
    let mut members = image::members(added);
    members.extend(layer.iter().map(|layer| layer.member()));
    members.extend(macos_layer.iter().map(|layer| layer.member()));
    if added.iter().any(|file| android::is_program(&file.host)) {
        members.extend(
            android::system_members(SHELL_PATH)
                .into_iter()
                .filter(|(path, _)| !added.iter().any(|file| file.path == *path)),
        );
    }
    let initramfs = release_initramfs(
        arch,
        &members,
        &format!("initramfs-image-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs)
}

const WINDOWS_LAYER_PACKAGE: &str = "eggos-windows";

const WINDOWS_LAYER_PATH: &str = "usr/lib/eggos/windows";

const WINDOWS_LAYER_ARCH: &str = "x86_64";

const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";

const WINDOWS_CONSOLE_PACKAGE: &str = "eggos-windows-console";

const WINDOWS_CONSOLE_PROGRAMS: [(&str, &str); 2] = [
    ("windows-console", "bin/windows-console.exe"),
    ("windows-unsupported", "bin/windows-unsupported.exe"),
];

struct WindowsLayer {
    host: String,
}

impl WindowsLayer {
    fn member(&self) -> (&str, initramfs::Content<'_>) {
        (WINDOWS_LAYER_PATH, initramfs::Content::Program(&self.host))
    }
}

fn windows_layer_for(
    arch: &Arch,
    added: &[image::AddedFile],
) -> Result<Option<WindowsLayer>, String> {
    let windows_program = added.iter().any(|file| {
        let mut magic = [0u8; 2];
        std::fs::File::open(&file.host)
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut magic))
            .is_ok_and(|()| &magic == b"MZ")
    });
    if !windows_program || arch.name != WINDOWS_LAYER_ARCH {
        return Ok(None);
    }
    let host = build_runtime_program(arch, WINDOWS_LAYER_PACKAGE)?;
    Ok(Some(WindowsLayer { host }))
}

fn build_windows_programs_test_kernel(arch: &Arch) -> Result<std::path::PathBuf, String> {
    let mut added = Vec::new();
    for (program, path) in WINDOWS_CONSOLE_PROGRAMS {
        added.push(image::AddedFile {
            host: build_windows_program(program)?,
            path: path.to_owned(),
        });
    }
    let layer = windows_layer_for(arch, &added)?;
    let mut extra = vec![(
        "etc/eggos/init.conf",
        initramfs::Content::Text(SHELL_TEST_CONFIG),
    )];
    extra.extend(image::members(&added));
    extra.extend(layer.iter().map(|layer| layer.member()));
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-windows-programs-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs)
}

const WINDOWS_PROGRAM_RUSTFLAGS: &[&str] = &[
    "-C",
    "linker=rust-lld",
    "-C",
    "link-arg=/NODEFAULTLIB",
    "-C",
    "link-arg=/SUBSYSTEM:CONSOLE",
    "-C",
    "link-arg=/ENTRY:mainCRTStartup",
];

fn build_windows_program(program: &str) -> Result<String, String> {
    let rustflags = WINDOWS_PROGRAM_RUSTFLAGS.join("\x1f");
    let target_dir = target_directory().join("windows-console");
    let target_dir = target_dir.to_string_lossy();
    let path = build_executable(
        &[
            "build",
            "-p",
            WINDOWS_CONSOLE_PACKAGE,
            "--bin",
            program,
            "--target",
            WINDOWS_TARGET,
        ],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str()),
            ("CARGO_TARGET_DIR", &target_dir),
        ],
        &format!("{program} executable for {WINDOWS_TARGET}"),
    )?;
    Ok(path.to_string_lossy().into_owned())
}

fn release_initramfs(
    arch: &Arch,
    extra: &[(&str, initramfs::Content)],
    file_name: &str,
) -> Result<std::path::PathBuf, String> {
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
    members.extend_from_slice(extra);
    initramfs::build(&members, file_name)
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

/// Builds the release kernel for `arch` with the `qemu-boot-clock` feature, which writes the
/// `boot_to_prompt` benchmark line at the shell's first prompt (D116).
pub fn build_boot_clock_kernel(arch: &Arch) -> Result<std::path::PathBuf, String> {
    let initramfs = build_release_initramfs(arch)?;
    let initramfs = initramfs.to_string_lossy();
    build_executable(
        &[
            "build",
            "-p",
            "eggos-kernel",
            "--target",
            arch.target,
            "--features",
            "qemu-boot-clock",
        ],
        &[("EGGOS_INITRAMFS", &initramfs)],
        &format!("boot-clock kernel executable for {}", arch.target),
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

const LINUX_STATIC_PACKAGE: &str = "eggos-linux-static";

const LINUX_STATIC_BUILDS: [(&str, &str); 2] = [
    ("static", "bin/linux-static"),
    ("pie", "bin/linux-static-pie"),
];

fn build_linux_programs_test_kernel(arch: &Arch) -> Result<std::path::PathBuf, String> {
    let mut added = Vec::new();
    for (relocation_model, path) in LINUX_STATIC_BUILDS {
        added.push(image::AddedFile {
            host: build_linux_static(arch, relocation_model)?,
            path: path.to_owned(),
        });
    }
    let mut extra = vec![(
        "etc/eggos/init.conf",
        initramfs::Content::Text(SHELL_TEST_CONFIG),
    )];
    extra.extend(image::members(&added));
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-linux-programs-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs)
}

fn build_linux_static(arch: &Arch, relocation_model: &str) -> Result<String, String> {
    let relocation_model = format!("relocation-model={relocation_model}");
    let rustflags = [
        "-C",
        "link-self-contained=yes",
        "-C",
        "target-feature=+crt-static",
        "-C",
        &relocation_model,
        "-C",
        "debuginfo=0",
        "-C",
        "strip=symbols",
    ]
    .join("\x1f");
    let target_dir = target_directory().join(format!(
        "linux-static-{}",
        relocation_model.replace('=', "-")
    ));
    let target_dir = target_dir.to_string_lossy();
    let path = build_executable(
        &[
            "build",
            "-p",
            LINUX_STATIC_PACKAGE,
            "--target",
            arch.user_target,
        ],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str()),
            ("CARGO_TARGET_DIR", &target_dir),
        ],
        &format!(
            "{LINUX_STATIC_PACKAGE} executable ({relocation_model}) for {}",
            arch.user_target
        ),
    )?;
    Ok(path.to_string_lossy().into_owned())
}

struct MacosLayer {
    host: String,
}

impl MacosLayer {
    fn member(&self) -> (&str, initramfs::Content<'_>) {
        (macos::LAYER_PATH, initramfs::Content::Program(&self.host))
    }
}

fn macos_layer_for(arch: &Arch, added: &[image::AddedFile]) -> Result<Option<MacosLayer>, String> {
    if !macos::layer_runs_on(arch.name) || !added.iter().any(|file| macos::is_program(&file.host)) {
        return Ok(None);
    }
    let host = build_runtime_program(arch, macos::LAYER_PACKAGE)?;
    Ok(Some(MacosLayer { host }))
}

const MACOS_FAT_PROGRAM: &str = "macos-fat";

fn build_macos_programs_test_kernel(arch: &Arch) -> Result<Option<std::path::PathBuf>, String> {
    if macos::not_built_for_this_run(std::env::var_os(macos::PROGRAMS_VARIABLE).as_deref()) {
        eprintln!(
            "xtask: {}: macOS programs boot test skipped: {} is empty, so this CI run built no \
             Mach-O programs (pull requests that change code build them)",
            arch.name,
            macos::PROGRAMS_VARIABLE
        );
        return Ok(None);
    }
    let Some(programs) = macos::find_or_build_programs(&target_directory())? else {
        if std::env::var_os("CI").is_some() {
            return Err(format!(
                "{}: the macOS programs boot test needs the Mach-O test programs in CI: set {}",
                arch.name,
                macos::PROGRAMS_VARIABLE
            ));
        }
        eprintln!(
            "xtask: {}: macOS programs boot test skipped: Apple's linker builds its programs, \
             so it runs on a macOS host or with {} naming a folder of them; CI runs it",
            arch.name,
            macos::PROGRAMS_VARIABLE
        );
        return Ok(None);
    };
    let (own, other) = match arch.name {
        "x86_64" => ("x86_64", "arm64"),
        _ => ("arm64", "x86_64"),
    };
    let read = |program: &str, cpu: &str| {
        let path = programs.path(program, cpu);
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let fat = macos::fat(&[
        &read("macos-console", "x86_64")?,
        &read("macos-console", "arm64")?,
    ])?;
    let fat_path = programs.directory.join(MACOS_FAT_PROGRAM);
    std::fs::write(&fat_path, fat).map_err(|e| format!("{}: {e}", fat_path.display()))?;
    let host =
        |program: &str, cpu: &str| programs.path(program, cpu).to_string_lossy().into_owned();
    let added = [
        image::AddedFile {
            host: host("macos-console", own),
            path: "bin/macos-console".to_owned(),
        },
        image::AddedFile {
            host: host("macos-unsupported", own),
            path: "bin/macos-unsupported".to_owned(),
        },
        image::AddedFile {
            host: host("macos-console", other),
            path: "bin/macos-other".to_owned(),
        },
        image::AddedFile {
            host: fat_path.to_string_lossy().into_owned(),
            path: format!("bin/{MACOS_FAT_PROGRAM}"),
        },
    ];
    let layer = macos_layer_for(arch, &added)?;
    let mut extra = vec![(
        "etc/eggos/init.conf",
        initramfs::Content::Text(SHELL_TEST_CONFIG),
    )];
    extra.extend(image::members(&added));
    extra.extend(layer.iter().map(|layer| layer.member()));
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-macos-programs-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs).map(Some)
}

const ANDROID_CONSOLE_PACKAGE: &str = "eggos-android-console";

const ANDROID_CONSOLE_PATH: &str = "bin/android-console";

fn build_android_programs_test_kernel(arch: &Arch) -> Result<Option<std::path::PathBuf>, String> {
    let Some(target) = android::target(arch.name) else {
        eprintln!(
            "xtask: {}: no Android programs boot test: Rust has no prebuilt Android target \
             for it",
            arch.name
        );
        return Ok(None);
    };
    let Some(ndk) = android::find_ndk() else {
        if std::env::var_os("CI").is_some() {
            return Err(format!(
                "{}: the Android programs boot test needs the Android NDK in CI: set \
                 ANDROID_NDK_HOME or ANDROID_HOME",
                arch.name
            ));
        }
        eprintln!(
            "xtask: {}: Android programs boot test skipped: no Android NDK found (set \
             ANDROID_NDK_HOME, or ANDROID_HOME to an SDK with one in ndk/); CI runs it",
            arch.name
        );
        return Ok(None);
    };
    let program = build_android_console(&ndk, target)?;
    let added = [image::AddedFile {
        host: program.to_string_lossy().into_owned(),
        path: ANDROID_CONSOLE_PATH.to_owned(),
    }];
    if !android::is_program(&added[0].host) {
        return Err(format!(
            "{}: the NDK linked no `.note.android.ident` note into it",
            added[0].host
        ));
    }
    let mut extra = vec![(
        "etc/eggos/init.conf",
        initramfs::Content::Text(SHELL_TEST_CONFIG),
    )];
    extra.extend(image::members(&added));
    extra.extend(android::system_members(SHELL_PATH));
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-android-programs-{}.cpio", arch.user_target),
    )?;
    build_release_kernel(arch, &initramfs).map(Some)
}

fn build_android_console(ndk: &android::Ndk, target: &str) -> Result<std::path::PathBuf, String> {
    let target_dir = target_directory().join("android-console");
    let target_dir_text = target_dir.to_string_lossy();
    cargo_with_env(
        &[
            "build",
            "-p",
            ANDROID_CONSOLE_PACKAGE,
            "--target",
            target,
            "--release",
        ],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", ""),
            ("CARGO_TARGET_DIR", &target_dir_text),
        ],
    )?;
    let release = target_dir.join(target).join("release");
    let program = release.join("android-console");
    android::link(ndk, target, &release.join("libandroid_console.a"), &program)?;
    Ok(program)
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

/// Cargo's target directory: `CARGO_TARGET_DIR`, or `target/` in the workspace.
pub fn target_directory() -> std::path::PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
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

pub fn cargo_with_env(args: &[&str], env: &[(&str, &str)]) -> Result<(), String> {
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
    use super::{arch_option, json_string_field, without_flag};

    #[test]
    fn a_flag_is_taken_out_wherever_it_is() {
        let args = |list: &[&str]| -> Vec<String> { list.iter().map(|a| a.to_string()).collect() };
        for given in [
            &["--nographic", "--arch", "riscv64"][..],
            &["--arch", "--nographic", "riscv64"],
            &["--arch", "riscv64", "--nographic"],
        ] {
            let (rest, found) = without_flag(&args(given), "--nographic");
            assert!(found);
            assert_eq!(rest, args(&["--arch", "riscv64"]));
            assert_eq!(arch_option(&rest).unwrap()[0].name, "riscv64");
        }
        let (rest, found) = without_flag(&args(&["--arch", "x86_64"]), "--nographic");
        assert!(!found);
        assert_eq!(rest.len(), 2);
    }

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

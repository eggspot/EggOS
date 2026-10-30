//! `cargo xtask workspace-build` (D173, the CI track's stage 2, milestone 3): a multi-crate
//! workspace of ours (`tests/multi-crate`) built inside EggOS with cargo's jobserver and a
//! `rustc` per CPU, timed against the same build inside Linux booted in the same QEMU machine.
//!
//! The work folder is `hello-crate`'s (D171): the pinned toolchain at `rustc`, the glibc
//! libraries it needs from this Linux host at `glibc`, a copy of the workspace at `workspace`
//! with its `.cargo/config.toml`, empty `tmp` and `cargo-home`, and the build timer
//! (`tests/build-timer`, static musl) at `build-timer`. The timer runs `rustc --version`, a
//! cold `cargo build`, a `cargo build` with nothing to do and `cargo run`, with a job per CPU,
//! and writes a `BUILD-TIMER` line before and after each phase (decided by research):
//!
//! - **EggOS** boots the release image with the work folder on a work disk, the toolchain's
//!   dynamic loader in the image, 4 GiB, 4 CPUs and multi-threaded TCG, and `eggsh` runs
//!   `/work/build-timer /work`.
//! - **Linux** (Alpine's `linux-virt` kernel, test data checked against its pinned SHA-256,
//!   never committed or shipped) boots in the same QEMU machine with the same memory, CPUs and
//!   TCG, from an initramfs xtask writes with the same work folder at `/work`, the same loader
//!   and the timer as `/init`, with `mitigations=off` (Linux's best case).
//! - **On the runner** (x86_64 hosts building for x86_64 only), the timer runs natively first:
//!   a reference for what emulation costs, never a ratio.
//!
//! Each phase's time is the host's clock between the console lines that start and end it, the
//! same ruler for both guests. Both builds must succeed and their programs print the same; the
//! times are reported (a table on standard error and, in CI, in the job summary), never gated.

use crate::dynamic_linking::{download, glibc, image_path, interpreter_of_file};
use crate::hello_crate::{
    assemble_toolchain, cargo_config, copy_tree, make_new_folder, musl_target,
};
use crate::initramfs::{self, Member};
use crate::rustc_version::{copy_libraries, host_library_folders, host_target, sha256_of};
use crate::{Arch, build_linux_test_program, build_release_kernel, release_initramfs, work_disk};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The workspace's program, which `cargo run` runs.
pub const PROGRAM: &str = "eggos-multi-crate";

/// Starts every line the timer writes.
pub const MARKER: &str = "BUILD-TIMER";

/// The timer's last line.
pub const DONE_LINE: &str = "BUILD-TIMER done";

/// Where the timer is inside EggOS.
pub const TIMER_PATH: &str = "/work/build-timer";

/// The work folder inside both guests.
pub const WORK_PATH: &str = "/work";

/// The phases the timer runs, in order.
pub const PHASES: [&str; 4] = ["rustc-version", "build", "rebuild", "run"];

/// The version of Alpine's `linux-virt` package whose kernel Linux boots.
pub const LINUX_VERSION: &str = "6.18.59-r0";

const ALPINE: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.24";
const KERNEL_IN_PACKAGE: &str = "boot/vmlinuz-virt";
const TIMER_PACKAGE: &str = "eggos-build-timer";
const TIMER: &str = "build-timer";
const TOOLCHAIN: &str = "rustc";
const WORKSPACE: &str = "workspace";
const TEMPORARY: &str = "tmp";
const CARGO_HOME: &str = "cargo-home";
const WORK: &str = "work";
const DIRECTORY_MODE: u32 = 0o040_755;
const FILE_MODE: u32 = 0o100_644;
const PROGRAM_MODE: u32 = 0o100_755;
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Alpine's `linux-virt` kernel for one architecture, pinned (decided by research).
#[derive(Debug, Clone, Copy)]
pub struct LinuxKernel {
    /// The architecture.
    pub arch: &'static str,
    /// The package's SHA-256.
    pub package_sha256: &'static str,
    /// `boot/vmlinuz-virt`'s SHA-256.
    pub kernel_sha256: &'static str,
    /// For an EFI zboot image (aarch64), the SHA-256 of the gzipped `Image` inside it, which
    /// QEMU boots.
    pub payload_sha256: Option<&'static str>,
}

/// The pinned kernels.
pub const LINUX_KERNELS: [LinuxKernel; 2] = [
    LinuxKernel {
        arch: "x86_64",
        package_sha256: "73ad4774ca26c196103a00e2fb8d28ed02de4c08c11fad96e903da03dcb085f4",
        kernel_sha256: "55fcc67c770730412a505df82b80dce098083e5680a7449dc6b4eaed895de21d",
        payload_sha256: None,
    },
    LinuxKernel {
        arch: "aarch64",
        package_sha256: "89b68fc32d61a1483ab0d9db78b5a06144ac115fe2ee8102cb4ad7987f46d524",
        kernel_sha256: "200c82866453b2faa2c39e53b5bdd04368153c767f014c9e5bbb8ac6b76cde0d",
        payload_sha256: Some("1358f29b2d9fec74e21f1954d3053dd02ecd41181045f6a80caddff938c90ffe"),
    },
];

/// The URL of the `linux-virt` package for `arch`.
pub fn package_url(arch: &str) -> String {
    format!("{ALPINE}/main/{arch}/linux-virt-{LINUX_VERSION}.apk")
}

/// Linux's command line for `arch`: its serial console, the timer as its first process, a
/// panic ending QEMU, and no mitigations, so Linux runs its fastest.
pub fn linux_command_line(arch: &str) -> Option<String> {
    let console = match arch {
        "x86_64" => "ttyS0",
        "aarch64" => "ttyAMA0",
        _ => return None,
    };
    Some(format!(
        "console={console} quiet loglevel=3 rdinit=/init panic=-1 mitigations=off"
    ))
}

/// The gzipped kernel inside an EFI zboot image (`MZ`, then `zimg`, then the payload's offset
/// and size as little-endian words), which QEMU's `-kernel` takes on aarch64.
pub fn zboot_payload(image: &[u8]) -> Result<&[u8], String> {
    if image.get(..2) != Some(b"MZ") || image.get(4..8) != Some(b"zimg") {
        return Err("not an EFI zboot image".into());
    }
    let word = |at: usize| -> Result<usize, String> {
        let bytes = image.get(at..at + 4).ok_or("the zboot header ends early")?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let (offset, size) = (word(8)?, word(12)?);
    let payload = offset
        .checked_add(size)
        .and_then(|end| image.get(offset..end))
        .ok_or("the zboot payload is not inside the image")?;
    match payload.starts_with(&GZIP_MAGIC) {
        true => Ok(payload),
        false => Err("the zboot payload is not gzip".into()),
    }
}

/// One phase of a timed build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTime {
    /// One of [`PHASES`].
    pub name: String,
    /// The exit status, -1 if the program could not run or was killed.
    pub status: i32,
    /// Its time by the guest's own clock.
    pub guest: Duration,
    /// Its time by the host's clock, between the lines that started and ended it.
    pub host: Duration,
}

/// What the timer reported in one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildRun {
    /// The jobs cargo was given: the CPUs the guest has.
    pub jobs: u32,
    /// The phases, in the order they ended.
    pub phases: Vec<PhaseTime>,
    /// What the workspace's program printed.
    pub program: Vec<String>,
    /// Whether the timer wrote its last line.
    pub done: bool,
}

impl BuildRun {
    /// The phase `name`, if it ended.
    pub fn phase(&self, name: &str) -> Option<&PhaseTime> {
        self.phases.iter().find(|phase| phase.name == name)
    }
}

/// Reads the timer's lines from a console, each with when the host read it since the boot
/// started; other lines are skipped. `Err` for a malformed `end` line or one without its
/// `begin`.
pub fn parse(lines: &[(Duration, String)]) -> Result<BuildRun, String> {
    let mut run = BuildRun::default();
    let mut begun: Vec<(String, Duration)> = Vec::new();
    for (at, line) in lines {
        let Some(rest) = line.trim_end().strip_prefix(MARKER) else {
            continue;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        match words.as_slice() {
            ["jobs", jobs] => run.jobs = jobs.parse().unwrap_or(0),
            ["begin", name] => begun.push(((*name).to_owned(), *at)),
            ["end", name, "status", status, "milliseconds", milliseconds] => {
                let started = begun
                    .iter()
                    .rev()
                    .find(|(begun, _)| begun == name)
                    .map(|&(_, started)| started)
                    .ok_or_else(|| format!("`{line}` has no `begin {name}` before it"))?;
                let status = status
                    .parse()
                    .map_err(|_| format!("`{line}`: the status is not a number"))?;
                let milliseconds = milliseconds
                    .parse()
                    .map_err(|_| format!("`{line}`: the time is not a number"))?;
                run.phases.push(PhaseTime {
                    name: (*name).to_owned(),
                    status,
                    guest: Duration::from_millis(milliseconds),
                    host: at.saturating_sub(started),
                });
            }
            ["program", ..] => {
                let text = rest
                    .trim_start()
                    .strip_prefix("program")
                    .unwrap_or_default();
                run.program
                    .push(text.strip_prefix(' ').unwrap_or(text).to_owned());
            }
            ["done"] => run.done = true,
            _ => {}
        }
    }
    Ok(run)
}

/// Checks that `run`, on `system`, ran every phase with status 0 and wrote its last line.
pub fn check_run(system: &str, run: &BuildRun) -> Result<(), String> {
    for name in PHASES {
        match run.phase(name) {
            Some(phase) if phase.status == 0 => {}
            Some(phase) => {
                return Err(format!(
                    "{system}: the timer's {name} phase ended with status {}",
                    phase.status
                ));
            }
            None => return Err(format!("{system}: the timer's {name} phase did not end")),
        }
    }
    match run.done {
        true => Ok(()),
        false => Err(format!("{system}: the timer did not write `{DONE_LINE}`")),
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f64())
}

fn ratio(eggos: Duration, linux: Duration) -> String {
    match linux.is_zero() {
        true => "-".into(),
        false => format!("{:.2}", eggos.as_secs_f64() / linux.as_secs_f64()),
    }
}

/// The report for `arch`: each phase's time inside EggOS and inside Linux in the same QEMU
/// machine, their ratio, and Linux on the runner when it ran, as a Markdown table.
pub fn report(arch: &str, eggos: &BuildRun, linux: &BuildRun, native: Option<&BuildRun>) -> String {
    let mut text = format!(
        "### Workspace build on {arch}: EggOS against Linux in the same QEMU machine\n\n\
         `tests/multi-crate` built by the official toolchain with {} jobs inside EggOS and {} \
         inside Linux {LINUX_VERSION} (TCG, 4 CPUs, 4 GiB; Linux from RAM with \
         `mitigations=off`, EggOS from its work disk). Host clock; a ratio above 1 means EggOS \
         is slower.\n\n\
         | Phase | EggOS | Linux | EggOS / Linux | Linux on the runner |\n\
         |---|---:|---:|---:|---:|\n",
        eggos.jobs, linux.jobs
    );
    let mut totals = [Duration::ZERO; 3];
    for name in PHASES {
        let time = |run: &BuildRun| run.phase(name).map(|phase| phase.host);
        let eggos_time = time(eggos).unwrap_or_default();
        let linux_time = time(linux).unwrap_or_default();
        let native_time = native.and_then(|run| run.phase(name).map(|phase| phase.guest));
        totals[0] += eggos_time;
        totals[1] += linux_time;
        totals[2] += native_time.unwrap_or_default();
        text.push_str(&format!(
            "| {name} | {} | {} | {} | {} |\n",
            seconds(eggos_time),
            seconds(linux_time),
            ratio(eggos_time, linux_time),
            native_time.map_or_else(|| "-".to_owned(), seconds)
        ));
    }
    text.push_str(&format!(
        "| **total** | {} | {} | {} | {} |\n",
        seconds(totals[0]),
        seconds(totals[1]),
        ratio(totals[0], totals[1]),
        match native {
            Some(_) => seconds(totals[2]),
            None => "-".to_owned(),
        }
    ));
    text
}

fn write_summary(report: &str) -> Result<(), String> {
    let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&summary)
        .map_err(|e| format!("{}: {e}", PathBuf::from(&summary).display()))?;
    writeln!(file, "{report}").map_err(|e| format!("the job summary: {e}"))
}

fn is_program(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn member(name: &str, mode: u32, data: Vec<u8>) -> Member {
    Member {
        name: name.to_owned(),
        mode,
        data,
        link_to: None,
    }
}

fn add_with_parents(members: &mut Vec<Member>, path: &str, mode: u32, data: Vec<u8>) {
    for (end, _) in path.match_indices('/') {
        let parent = &path[..end];
        if !members.iter().any(|member| member.name == parent) {
            members.push(member(parent, DIRECTORY_MODE, Vec::new()));
        }
    }
    members.push(member(path, mode, data));
}

fn add_tree(members: &mut Vec<Member>, folder: &Path, name: &str) -> Result<(), String> {
    let mut entries = std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let child = format!("{name}/{}", entry.file_name().to_string_lossy());
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if metadata.is_dir() {
            members.push(member(&child, DIRECTORY_MODE, Vec::new()));
            add_tree(members, &path, &child)?;
        } else {
            let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mode = match is_program(&metadata) {
                true => PROGRAM_MODE,
                false => FILE_MODE,
            };
            members.push(member(&child, mode, data));
        }
    }
    Ok(())
}

/// The members of Linux's initramfs: the mount points the timer uses, the timer as `/init`,
/// glibc's dynamic loader at `loader_path` (a member's name, from `PT_INTERP`) and the work
/// folder `work` at `/work`, each directory before what it holds.
pub fn linux_members(
    work: &Path,
    timer: Vec<u8>,
    loader_path: &str,
    loader: Vec<u8>,
) -> Result<Vec<Member>, String> {
    let mut members = vec![member(".", DIRECTORY_MODE, Vec::new())];
    for folder in ["dev", "proc", "tmp"] {
        members.push(member(folder, DIRECTORY_MODE, Vec::new()));
    }
    members.push(member("init", PROGRAM_MODE, timer));
    add_with_parents(&mut members, loader_path, PROGRAM_MODE, loader);
    members.push(member(WORK, DIRECTORY_MODE, Vec::new()));
    add_tree(&mut members, work, WORK)?;
    Ok(members)
}

fn fetch_kernel_package(url: &str, file: &Path, sha256: &str) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != sha256 {
        download(url, file)?;
    }
    let found = sha256_of(file)?;
    match found == sha256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {sha256}: Alpine's mirror keeps only each \
             package's latest build, so pin the new one in xtask/src/workspace_build.rs after \
             checking it",
            file.display()
        )),
    }
}

fn linux_kernel(arch: &Arch, root: &Path) -> Result<PathBuf, String> {
    let pin = LINUX_KERNELS
        .iter()
        .find(|kernel| kernel.arch == arch.name)
        .ok_or_else(|| format!("{}: no Linux kernel is pinned for it", arch.name))?;
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| format!("{}: {e}", downloads.display()))?;
    let package = downloads.join(format!("linux-virt-{LINUX_VERSION}-{}.apk", arch.name));
    fetch_kernel_package(&package_url(arch.name), &package, pin.package_sha256)?;
    let unpacked = root.join(format!("linux-{}", arch.name));
    make_new_folder(&unpacked)?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&package)
        .arg("-C")
        .arg(&unpacked)
        .arg(KERNEL_IN_PACKAGE)
        .status()
        .map_err(|e| format!("could not run tar: {e}"))?;
    let kernel = unpacked.join(KERNEL_IN_PACKAGE);
    if !kernel.is_file() {
        return Err(format!(
            "tar -xzf {} {KERNEL_IN_PACKAGE}: {status}",
            package.display()
        ));
    }
    let found = sha256_of(&kernel)?;
    if found != pin.kernel_sha256 {
        return Err(format!(
            "{}: SHA-256 {found}, not the pinned {}",
            kernel.display(),
            pin.kernel_sha256
        ));
    }
    let Some(payload_sha256) = pin.payload_sha256 else {
        return Ok(kernel);
    };
    let image = std::fs::read(&kernel).map_err(|e| format!("{}: {e}", kernel.display()))?;
    let payload = zboot_payload(&image).map_err(|e| format!("{}: {e}", kernel.display()))?;
    let found = crate::sha256::hex_digest(payload);
    if found != payload_sha256 {
        return Err(format!(
            "{}: the kernel inside has SHA-256 {found}, not the pinned {payload_sha256}",
            kernel.display()
        ));
    }
    let gzipped = unpacked.join("Image.gz");
    std::fs::write(&gzipped, payload).map_err(|e| format!("{}: {e}", gzipped.display()))?;
    Ok(gzipped)
}

fn run_natively(arch: &Arch, work: &Path) -> Result<Option<BuildRun>, String> {
    if std::env::consts::ARCH != arch.name {
        eprintln!(
            "xtask: {}: no build on the runner for comparison: this host is {}",
            arch.name,
            std::env::consts::ARCH
        );
        return Ok(None);
    }
    let started = Instant::now();
    let mut child = Command::new(work.join(TIMER))
        .arg(work)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: could not run the timer on the runner: {e}", arch.name))?;
    let mut lines = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| format!("{}: the timer's output: {e}", arch.name))?;
            eprintln!("{line}");
            lines.push((started.elapsed(), line));
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("{}: the timer on the runner: {e}", arch.name))?;
    let run = parse(&lines)?;
    check_run("the runner", &run)?;
    if !status.success() {
        return Err(format!("{}: the timer on the runner: {status}", arch.name));
    }
    let target = work.join(WORKSPACE).join("target");
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    for folder in [TEMPORARY, CARGO_HOME] {
        make_new_folder(&work.join(folder))?;
    }
    Ok(Some(run))
}

fn prepare_work(arch: &Arch, root: &Path, work: &Path) -> Result<(), String> {
    let (Some(host), Some(target)) = (host_target(arch.name), musl_target(arch.name)) else {
        return Err(format!(
            "{}: the official toolchain has no cargo for it (x86_64 and aarch64 do)",
            arch.name
        ));
    };
    make_new_folder(work)?;
    let toolchain = work.join(TOOLCHAIN);
    let scratch = root.join(arch.name).join("unpacked");
    assemble_toolchain(root, &scratch, host, target, &toolchain)?;
    let rust_lld = toolchain
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin")
        .join("rust-lld");
    let programs = [toolchain.join("bin").join("cargo"), rust_lld];
    let copied = copy_libraries(work, &programs, host_library_folders(arch.name))?;
    eprintln!(
        "xtask: {}: {} libraries from the host for the toolchain",
        arch.name,
        copied.len()
    );
    let workspace = work.join(WORKSPACE);
    make_new_folder(&workspace)?;
    copy_tree(
        &crate::workspace_root().join("tests").join("multi-crate"),
        &workspace,
    )?;
    let config = workspace.join(".cargo");
    std::fs::create_dir_all(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let file = config.join("config.toml");
    std::fs::write(&file, cargo_config(target)).map_err(|e| format!("{}: {e}", file.display()))?;
    for folder in [TEMPORARY, CARGO_HOME] {
        make_new_folder(&work.join(folder))?;
    }
    let timer = build_linux_test_program(arch, TIMER_PACKAGE, "static")?;
    let into = work.join(TIMER);
    std::fs::copy(&timer, &into)
        .map(drop)
        .map_err(|e| format!("{timer} to {}: {e}", into.display()))
}

/// Builds the workspace inside EggOS and inside Linux on `arch` and reports the times (see the
/// module documentation). Needs a Linux host: the toolchain's C libraries come from it.
pub fn check(arch: &Arch) -> Result<(), String> {
    if std::env::consts::OS != "linux" {
        return Err(format!(
            "{}: the workspace build needs a Linux host, whose C libraries the toolchain takes",
            arch.name
        ));
    }
    let glibc =
        glibc(arch.name).ok_or_else(|| format!("{}: no glibc is known for it", arch.name))?;
    let command_line = linux_command_line(arch.name)
        .ok_or_else(|| format!("{}: no Linux console is known for it", arch.name))?;
    let root = crate::target_directory().join("workspace-build");
    let work = root.join(arch.name).join(WORK);
    prepare_work(arch, &root, &work)?;
    let native = run_natively(arch, &work)?;
    let loader = interpreter_of_file(&work.join(TOOLCHAIN).join("bin").join("rustc"))?;
    let loader_path = image_path(&loader)?;
    let host_loader = Path::new(glibc.libraries).join(glibc.loader);
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    let members = linux_members(
        &work,
        read(&work.join(TIMER))?,
        &loader_path,
        read(&host_loader)?,
    )?;
    let linux_initramfs = root.join(format!("linux-initramfs-{}.cpio", arch.name));
    std::fs::write(&linux_initramfs, initramfs::archive(&members, 0))
        .map_err(|e| format!("{}: {e}", linux_initramfs.display()))?;
    drop(members);
    let image = crate::target_directory()
        .join("image")
        .join(format!("eggos-{}-workspace-build-work.img", arch.name));
    let packed = work_disk::pack(&work, &image)?;
    eprintln!(
        "xtask: {}: packed {} files, {} bytes, into {}",
        arch.name,
        packed.files,
        packed.bytes,
        image.display()
    );
    let host_loader = host_loader.to_string_lossy().into_owned();
    let extra = [(
        loader_path.as_str(),
        initramfs::Content::Program(&host_loader),
    )];
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-workspace-build-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    let eggos = parse(&crate::qemu::workspace_build::eggos(arch, &kernel, &image)?)?;
    let linux_kernel = linux_kernel(arch, &root)?;
    let linux = parse(&crate::qemu::workspace_build::linux(
        arch,
        &linux_kernel,
        &linux_initramfs,
        &command_line,
    )?)?;
    let report = report(arch.name, &eggos, &linux, native.as_ref());
    eprintln!("{report}");
    write_summary(&report)?;
    check_run("Linux", &linux)?;
    check_run("EggOS", &eggos)?;
    if eggos.program != linux.program {
        return Err(format!(
            "{}: inside EggOS the program printed\n{}\nbut inside Linux\n{}",
            arch.name,
            eggos.program.join("\n"),
            linux.program.join("\n")
        ));
    }
    if let Some(native) = native.filter(|native| native.program != linux.program) {
        return Err(format!(
            "{}: on the runner the program printed\n{}\nbut inside Linux\n{}",
            arch.name,
            native.program.join("\n"),
            linux.program.join("\n")
        ));
    }
    eprintln!(
        "xtask: {}: the workspace built and ran inside EggOS and inside Linux, printing the same",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamped(lines: &[(u64, &str)]) -> Vec<(Duration, String)> {
        lines
            .iter()
            .map(|&(seconds, line)| (Duration::from_secs(seconds), line.to_owned()))
            .collect()
    }

    fn full_run(build_ends_at: u64) -> Vec<(Duration, String)> {
        stamped(&[
            (0, "BUILD-TIMER jobs 4"),
            (1, "BUILD-TIMER begin rustc-version"),
            (2, "BUILD-TIMER end rustc-version status 0 milliseconds 900"),
            (2, "   Compiling eggos-multi-crate-words v0.1.0"),
            (2, "BUILD-TIMER begin build\r"),
            (
                build_ends_at,
                "BUILD-TIMER end build status 0 milliseconds 7900\r",
            ),
            (build_ends_at, "BUILD-TIMER begin rebuild"),
            (
                build_ends_at + 1,
                "BUILD-TIMER end rebuild status 0 milliseconds 1000",
            ),
            (build_ends_at + 1, "BUILD-TIMER begin run"),
            (
                build_ends_at + 3,
                "BUILD-TIMER end run status 0 milliseconds 2000",
            ),
            (
                build_ends_at + 3,
                "BUILD-TIMER program words: A Multi Crate Workspace",
            ),
            (build_ends_at + 3, "BUILD-TIMER program   the 5"),
            (build_ends_at + 3, "BUILD-TIMER done"),
        ])
    }

    #[test]
    fn the_timer_s_lines_give_each_phase_s_time_by_both_clocks_and_the_program_s_output() {
        let run = parse(&full_run(10)).unwrap();
        assert_eq!(run.jobs, 4);
        assert!(run.done);
        assert_eq!(
            run.phases
                .iter()
                .map(|phase| phase.name.as_str())
                .collect::<Vec<_>>(),
            PHASES
        );
        let build = run.phase("build").unwrap();
        assert_eq!(build.host, Duration::from_secs(8));
        assert_eq!(build.guest, Duration::from_millis(7900));
        assert_eq!(run.program, ["words: A Multi Crate Workspace", "  the 5"]);
        assert_eq!(check_run("EggOS", &run), Ok(()));
    }

    #[test]
    fn a_failed_or_missing_phase_or_an_unfinished_timer_fails_the_run() {
        let mut lines = full_run(10);
        lines[5].1 = "BUILD-TIMER end build status 101 milliseconds 7900".into();
        let error = check_run("EggOS", &parse(&lines).unwrap()).unwrap_err();
        assert!(
            error.contains("build phase ended with status 101"),
            "{error}"
        );
        let unfinished = parse(&full_run(10)[..9]).unwrap();
        let error = check_run("Linux", &unfinished).unwrap_err();
        assert!(error.contains("run phase did not end"), "{error}");
        let mut no_done = full_run(10);
        no_done.pop();
        let error = check_run("Linux", &parse(&no_done).unwrap()).unwrap_err();
        assert!(error.contains(DONE_LINE), "{error}");
        let orphan = stamped(&[(3, "BUILD-TIMER end build status 0 milliseconds 1")]);
        assert!(parse(&orphan).unwrap_err().contains("no `begin build`"));
    }

    #[test]
    fn the_report_gives_each_phase_against_linux_and_the_runner_s_time() {
        let eggos = parse(&full_run(20)).unwrap();
        let linux = parse(&full_run(10)).unwrap();
        let text = report("x86_64", &eggos, &linux, None);
        assert!(text.starts_with("### Workspace build on x86_64"));
        assert!(
            text.contains("| build | 18.0 s | 8.0 s | 2.25 | - |"),
            "{text}"
        );
        assert!(
            text.contains("| **total** | 22.0 s | 12.0 s | 1.83 | - |"),
            "{text}"
        );
        let native = report("x86_64", &eggos, &linux, Some(&linux));
        assert!(
            native.contains("| build | 18.0 s | 8.0 s | 2.25 | 7.9 s |"),
            "{native}"
        );
    }

    #[test]
    fn an_aarch64_zboot_image_gives_its_gzipped_kernel() {
        let mut image = b"MZ\0\0zimg".to_vec();
        image.extend_from_slice(&64u32.to_le_bytes());
        image.extend_from_slice(&4u32.to_le_bytes());
        image.resize(64, 0);
        image.extend_from_slice(&[0x1f, 0x8b, 8, 0, 9, 9]);
        assert_eq!(zboot_payload(&image), Ok(&[0x1f, 0x8b, 8, 0][..]));
        let mut not_gzip = image.clone();
        not_gzip[64] = 0;
        assert!(zboot_payload(&not_gzip).is_err());
        let mut too_long = image.clone();
        too_long[12] = 200;
        assert!(zboot_payload(&too_long).is_err());
        assert!(zboot_payload(b"\x7fELF").is_err());
    }

    #[test]
    fn each_architecture_has_a_pinned_kernel_and_a_console() {
        for arch in ["x86_64", "aarch64"] {
            let kernel = LINUX_KERNELS.iter().find(|kernel| kernel.arch == arch);
            let kernel = kernel.unwrap();
            for sha256 in [kernel.package_sha256, kernel.kernel_sha256] {
                assert_eq!(sha256.len(), 64);
            }
            assert_eq!(kernel.payload_sha256.is_some(), arch == "aarch64");
            let line = linux_command_line(arch).unwrap();
            assert!(line.contains("rdinit=/init") && line.contains("mitigations=off"));
            assert!(package_url(arch).ends_with(&format!("/{arch}/linux-virt-6.18.59-r0.apk")));
        }
        assert_eq!(linux_command_line("riscv64"), None);
    }

    #[test]
    fn linux_s_initramfs_holds_the_timer_as_init_the_loader_and_the_work_folder() {
        let folder =
            std::env::temp_dir().join(format!("eggos-workspace-build-{}", std::process::id()));
        let work = folder.join("work");
        std::fs::create_dir_all(work.join("rustc").join("bin")).unwrap();
        std::fs::write(work.join("rustc").join("bin").join("rustc"), b"elf").unwrap();
        std::fs::write(work.join("a.txt"), b"text").unwrap();
        let members =
            linux_members(&work, b"timer".to_vec(), "lib64/ld.so", b"loader".to_vec()).unwrap();
        let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(
            names,
            [
                ".",
                "dev",
                "proc",
                "tmp",
                "init",
                "lib64",
                "lib64/ld.so",
                "work",
                "work/a.txt",
                "work/rustc",
                "work/rustc/bin",
                "work/rustc/bin/rustc"
            ]
        );
        assert_eq!(members[4].mode, PROGRAM_MODE);
        assert_eq!(members[4].data, b"timer");
        assert_eq!(members[5].mode, DIRECTORY_MODE);
        assert_eq!(members[8].mode, FILE_MODE);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_workspace_is_its_own_with_a_lock_file_and_no_dependencies_from_outside() {
        let folder = crate::workspace_root().join("tests").join("multi-crate");
        let manifest = std::fs::read_to_string(folder.join("Cargo.toml")).unwrap();
        assert!(manifest.starts_with("[workspace]\n"));
        let lock = std::fs::read_to_string(folder.join("Cargo.lock")).unwrap();
        assert!(lock.contains(&format!("name = \"{PROGRAM}\"")));
        assert!(!lock.contains("source = "), "no dependencies from outside");
        assert!(lock.matches("[[package]]").count() >= 4, "several crates");
    }
}

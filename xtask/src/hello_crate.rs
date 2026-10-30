//! `cargo xtask hello-crate` (D171, the CI track's stage 2, milestone 2): `cargo build` and
//! `cargo run` of a hello-world crate inside EggOS, with the official toolchain downloaded as
//! test data and never shipped, and the program's output compared with Linux's.
//!
//! The pinned release's `rustc` and `cargo` for the architecture's glibc target and the
//! standard library of its musl target come from static.rust-lang.org, checked against their
//! pinned SHA-256s, and are merged into one toolchain folder: the program is linked by the
//! toolchain's own `rust-lld` with the musl target's self-contained start files and C library,
//! so no C compiler or glibc development files are needed (decided by research). The work
//! folder holds the toolchain at `rustc`, the glibc libraries its programs need at `glibc`
//! (from this Linux host), a copy of `tests/hello-crate` at `hello` with a `.cargo/config.toml`
//! naming the target and the linker, and empty `tmp` and `cargo-home` folders. EggOS boots with
//! it on a work disk and 4 GiB of memory, runs `cargo build` and `cargo run` in `/work/hello`,
//! writing the program's output to `/work/hello.out`, and powers off; xtask unpacks the disk.
//! On a Linux host the same toolchain (this host's architecture's `rustc` and `cargo`, the
//! target's standard library) builds the same crate in a folder outside the repository, and the
//! program runs natively, or through `qemu-<arch>` for another architecture: its output is what
//! EggOS's must be. Elsewhere the program's known output stands in, and CI (`CI` set) refuses to
//! go without Linux's.

use crate::dynamic_linking::{glibc, image_path, interpreter_of_file};
use crate::rustc_version::{
    Component, copy_libraries, fetch, host_library_folders, host_target, pinned, sha256_of, unpack,
};
use crate::{Arch, build_release_kernel, initramfs, release_initramfs, work_disk};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The test crate's package and program name.
pub const PROGRAM: &str = "eggos-hello-crate";

/// Where the crate is inside EggOS.
pub const CRATE_FOLDER: &str = "/work/hello";

/// Where EggOS writes the program's output.
pub const OUTPUT_PATH: &str = "/work/hello.out";

const TOOLCHAIN: &str = "rustc";
const CRATE: &str = "hello";
const TEMPORARY: &str = "tmp";
const CARGO_HOME: &str = "cargo-home";
const OUTPUT: &str = "hello.out";

/// The musl target for `arch` the crate is built for.
pub fn musl_target(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

/// What the program prints on `arch`, from its source in `tests/hello-crate`.
pub fn known_output(arch: &str) -> String {
    format!(
        "Hello, world!\nbrown 1\ndog 1\nend 1\nfox 1\njumps 1\nlazy 1\nover 1\nquick 1\nthe 3\n\
         sums [500500, 1500500, 2500500, 3500500] total 8002000\nwritten and read back\n\
         linux {arch}\n"
    )
}

/// The crate's `.cargo/config.toml` for `target`: the target, and `rust-lld` as its linker.
pub fn cargo_config(target: &str) -> String {
    format!(
        "[build]\ntarget = \"{target}\"\n\n[target.{target}]\nlinker = \"rust-lld\"\n\
         rustflags = [\"-C\", \"link-arg=--threads=1\", \"-C\", \"debuginfo=0\"]\n"
    )
}

/// The environment `cargo` runs in, for a toolchain at `toolchain`, a cargo home at
/// `cargo_home` and temporary files in `temporary`; `libraries` for glibc's loader when the
/// toolchain's libraries are elsewhere than the host's.
pub fn cargo_environment(
    toolchain: &str,
    cargo_home: &str,
    temporary: &str,
    libraries: Option<&str>,
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = [
        ("RUSTC", format!("{toolchain}/bin/rustc")),
        ("CARGO_HOME", cargo_home.to_owned()),
        ("TMPDIR", temporary.to_owned()),
        ("CARGO_INCREMENTAL", "0".to_owned()),
        ("CARGO_BUILD_JOBS", "1".to_owned()),
        ("CARGO_NET_OFFLINE", "true".to_owned()),
        ("CARGO_TERM_COLOR", "never".to_owned()),
        ("CARGO_TERM_PROGRESS_WHEN", "never".to_owned()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect();
    if let Some(libraries) = libraries {
        environment.push(("LD_LIBRARY_PATH".to_owned(), libraries.to_owned()));
    }
    environment
}

/// What the program did on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxRun {
    /// What it printed.
    pub output: Vec<u8>,
    /// The SHA-256 of the program `cargo build` made.
    pub program_sha256: String,
}

/// Downloads the toolchain for `arch`, builds and runs the crate on this Linux host, packs the
/// work folder into a work disk, builds the release image with the dynamic loader `rustc`
/// names, boots it ([`crate::qemu::hello_crate::check`]) and compares the program's output.
pub fn check(arch: &Arch) -> Result<(), String> {
    let (Some(host), Some(target)) = (host_target(arch.name), musl_target(arch.name)) else {
        return Err(format!(
            "{}: the official toolchain has no cargo for it (x86_64 and aarch64 do)",
            arch.name
        ));
    };
    let glibc =
        glibc(arch.name).ok_or_else(|| format!("{}: no glibc is known for it", arch.name))?;
    let root = crate::target_directory().join("hello-crate");
    let scratch = root.join(arch.name);
    let work = scratch.join("work");
    make_new_folder(&work)?;
    let toolchain = work.join(TOOLCHAIN);
    assemble_toolchain(&root, &scratch.join("unpacked"), host, target, &toolchain)?;
    let rust_lld = toolchain
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin")
        .join("rust-lld");
    let programs = [toolchain.join("bin").join("cargo"), rust_lld];
    let copied = copy_libraries(&work, &programs, host_library_folders(arch.name))?;
    eprintln!(
        "xtask: {}: {} libraries from the host for the toolchain: {}",
        arch.name,
        copied.len(),
        copied.join(" ")
    );
    copy_crate(&work.join(CRATE), target)?;
    for folder in [TEMPORARY, CARGO_HOME] {
        make_new_folder(&work.join(folder))?;
    }
    let linux = linux_run(&root, &scratch, arch.name, host, target)?;
    let loader_path = image_path(&interpreter_of_file(&toolchain.join("bin").join("rustc"))?)?;
    let host_loader = Path::new(glibc.libraries).join(glibc.loader);
    if !host_loader.is_file() {
        return Err(format!(
            "{}: {} is missing: the check needs a Linux host with the target's C library",
            arch.name,
            host_loader.display()
        ));
    }
    let image = crate::target_directory()
        .join("image")
        .join(format!("eggos-{}-hello-crate-work.img", arch.name));
    let packed = work_disk::pack(&work, &image)?;
    eprintln!(
        "xtask: {}: packed {} files, {} bytes, into {} ({} MiB)",
        arch.name,
        packed.files,
        packed.bytes,
        image.display(),
        packed.image_bytes >> 20
    );
    let host_loader = host_loader.to_string_lossy().into_owned();
    let extra = [(
        loader_path.as_str(),
        initramfs::Content::Program(&host_loader),
    )];
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-hello-crate-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    crate::qemu::hello_crate::check(arch, &kernel, &image)?;
    let out = scratch.join("out");
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    work_disk::unpack(&image, &out)?;
    let output = std::fs::read(out.join(OUTPUT)).map_err(|e| {
        format!(
            "{}: {OUTPUT_PATH} did not come back from EggOS: {e}",
            arch.name
        )
    })?;
    let program = out
        .join(CRATE)
        .join("target")
        .join(target)
        .join("debug")
        .join(PROGRAM);
    let program_sha256 = sha256_of(&program).ok();
    compare(
        arch.name,
        &output,
        linux.as_ref(),
        program_sha256.as_deref(),
    )?;
    eprintln!(
        "xtask: {}: cargo built and ran the hello-world crate inside EggOS, printing what {} \
         printed",
        arch.name,
        match linux {
            Some(_) => "Linux",
            None => "its source says it",
        }
    );
    Ok(())
}

/// Checks that `output`, what the program printed inside EggOS on `arch`, is what it printed on
/// Linux (`linux`), or without that what its source prints; a program that differs from
/// Linux's is reported, not refused.
pub fn compare(
    arch: &str,
    output: &[u8],
    linux: Option<&LinuxRun>,
    program_sha256: Option<&str>,
) -> Result<(), String> {
    let wanted = match linux {
        Some(run) => run.output.clone(),
        None => known_output(arch).into_bytes(),
    };
    if output != wanted.as_slice() {
        return Err(format!(
            "{arch}: inside EggOS the program printed\n{}\nbut {} it printed\n{}",
            String::from_utf8_lossy(output),
            match linux {
                Some(_) => "on Linux",
                None => "by its source",
            },
            String::from_utf8_lossy(&wanted)
        ));
    }
    if let (Some(run), Some(sha256)) = (linux, program_sha256) {
        let same = match run.program_sha256 == sha256 {
            true => "the same program as",
            false => "a program different from the one",
        };
        eprintln!("xtask: {arch}: cargo inside EggOS built {same} cargo on Linux built");
    }
    Ok(())
}

fn make_new_folder(folder: &Path) -> Result<(), String> {
    if folder.exists() {
        std::fs::remove_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))
}

fn pinned_component(kind: &str, target: &str) -> Result<Component, String> {
    pinned(kind, target).ok_or_else(|| format!("no pinned {kind} for {target}"))
}

fn assemble_toolchain(
    root: &Path,
    scratch: &Path,
    host: &str,
    target: &str,
    into: &Path,
) -> Result<(), String> {
    let mut unpacked = Vec::new();
    for (kind, of) in [("rustc", host), ("cargo", host), ("rust-std", target)] {
        let component = pinned_component(kind, of)?;
        let archive = fetch(root, component)?;
        let folder = scratch.join(component.name());
        unpack(&archive, &folder)?;
        unpacked.push(folder.join(component.name()));
    }
    let [rustc, cargo, standard_library] = unpacked.as_slice() else {
        return Err("the toolchain's three components did not unpack".into());
    };
    if into.exists() {
        std::fs::remove_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    }
    let rustc = rustc.join("rustc");
    std::fs::rename(&rustc, into)
        .map_err(|e| format!("{} to {}: {e}", rustc.display(), into.display()))?;
    let cargo = cargo.join("cargo").join("bin").join("cargo");
    let to = into.join("bin").join("cargo");
    std::fs::copy(&cargo, &to)
        .map_err(|e| format!("{} to {}: {e}", cargo.display(), to.display()))?;
    let library = standard_library
        .join(format!("rust-std-{target}"))
        .join("lib")
        .join("rustlib")
        .join(target);
    let to = into.join("lib").join("rustlib").join(target);
    if to.exists() {
        std::fs::remove_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    std::fs::rename(&library, &to)
        .map_err(|e| format!("{} to {}: {e}", library.display(), to.display()))
}

fn copy_crate(into: &Path, target: &str) -> Result<(), String> {
    make_new_folder(into)?;
    let source = crate::workspace_root().join("tests").join("hello-crate");
    copy_tree(&source, into)?;
    let config = into.join(".cargo");
    std::fs::create_dir_all(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let file = config.join("config.toml");
    std::fs::write(&file, cargo_config(target)).map_err(|e| format!("{}: {e}", file.display()))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        let into = to.join(&name);
        if path.is_dir() {
            std::fs::create_dir_all(&into).map_err(|e| format!("{}: {e}", into.display()))?;
            copy_tree(&path, &into)?;
        } else {
            std::fs::copy(&path, &into)
                .map_err(|e| format!("{} to {}: {e}", path.display(), into.display()))?;
        }
    }
    Ok(())
}

fn linux_run(
    root: &Path,
    scratch: &Path,
    arch: &str,
    target_host: &str,
    target: &str,
) -> Result<Option<LinuxRun>, String> {
    let in_ci = std::env::var_os("CI").is_some();
    let host_arch = std::env::consts::ARCH;
    let host = match (std::env::consts::OS, host_target(host_arch)) {
        ("linux", Some(host)) => host,
        _ if in_ci => {
            return Err(format!(
                "{arch}: CI must compare with Linux on a Linux host"
            ));
        }
        _ => return Ok(None),
    };
    let runner = match host_arch == arch {
        true => None,
        false => match find_user_emulator(arch) {
            Some(emulator) => Some(emulator),
            None if in_ci => {
                return Err(format!(
                    "{arch}: no qemu-{arch} to run the program on this {host_arch} Linux host \
                     (on Ubuntu: the qemu-user package)"
                ));
            }
            None => return Ok(None),
        },
    };
    let folder =
        std::env::temp_dir().join(format!("eggos-hello-crate-{arch}-{}", std::process::id()));
    make_new_folder(&folder)?;
    let toolchain = match host == target_host {
        true => scratch.join("work").join(TOOLCHAIN),
        false => {
            let toolchain = root.join(format!("linux-{host_arch}")).join(TOOLCHAIN);
            assemble_toolchain(
                root,
                &root.join(format!("linux-{host_arch}")).join("unpacked"),
                host,
                target,
                &toolchain,
            )?;
            toolchain
        }
    };
    let crate_folder = folder.join(CRATE);
    copy_crate(&crate_folder, target)?;
    for name in [TEMPORARY, CARGO_HOME] {
        make_new_folder(&folder.join(name))?;
    }
    let path = |name: &str| folder.join(name).to_string_lossy().into_owned();
    let toolchain_path = toolchain.to_string_lossy().into_owned();
    let environment = cargo_environment(&toolchain_path, &path(CARGO_HOME), &path(TEMPORARY), None);
    let searched = match std::env::var("PATH") {
        Ok(path) => format!("{toolchain_path}/bin:{path}"),
        Err(_) => format!("{toolchain_path}/bin"),
    };
    let status = Command::new(toolchain.join("bin").join("cargo"))
        .args(["build", "--locked"])
        .current_dir(&crate_folder)
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .env("PATH", &searched)
        .status()
        .map_err(|e| format!("{arch}: could not run cargo on Linux: {e}"))?;
    if !status.success() {
        return Err(format!("{arch}: cargo build on Linux: {status}"));
    }
    let program = crate_folder
        .join("target")
        .join(target)
        .join("debug")
        .join(PROGRAM);
    let mut run = match &runner {
        Some(emulator) => {
            let mut command = Command::new(emulator);
            command.arg(&program);
            command
        }
        None => Command::new(&program),
    };
    let ran = run
        .env("TMPDIR", path(TEMPORARY))
        .output()
        .map_err(|e| format!("{arch}: could not run {}: {e}", program.display()))?;
    if !ran.status.success() {
        return Err(format!("{arch}: the program on Linux: {}", ran.status));
    }
    let program_sha256 = sha256_of(&program)?;
    std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    Ok(Some(LinuxRun {
        output: ran.stdout,
        program_sha256,
    }))
}

fn find_user_emulator(arch: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|folder| {
            [
                folder.join(format!("qemu-{arch}")),
                folder.join(format!("qemu-{arch}-static")),
            ]
        })
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_architecture_has_its_musl_target_and_pinned_downloads() {
        for arch in ["x86_64", "aarch64"] {
            let target = musl_target(arch).unwrap();
            assert!(target.starts_with(arch) && target.ends_with("-unknown-linux-musl"));
            let host = host_target(arch).unwrap();
            for (kind, of) in [("rustc", host), ("cargo", host), ("rust-std", target)] {
                assert!(pinned_component(kind, of).is_ok(), "{kind} {of}");
            }
        }
        assert_eq!(musl_target("riscv64"), None);
    }

    #[test]
    fn the_config_names_the_target_and_rust_lld() {
        let config = cargo_config("aarch64-unknown-linux-musl");
        assert!(config.starts_with("[build]\ntarget = \"aarch64-unknown-linux-musl\"\n"));
        assert!(config.contains("[target.aarch64-unknown-linux-musl]\nlinker = \"rust-lld\"\n"));
        assert!(!config.contains('#'));
    }

    #[test]
    fn cargo_runs_offline_with_one_job_and_the_toolchain_s_rustc() {
        let environment = cargo_environment(
            "/work/rustc",
            "/work/cargo-home",
            "/work/tmp",
            Some("/work/glibc"),
        );
        let value = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("RUSTC"), Some("/work/rustc/bin/rustc"));
        assert_eq!(value("CARGO_NET_OFFLINE"), Some("true"));
        assert_eq!(value("CARGO_BUILD_JOBS"), Some("1"));
        assert_eq!(value("LD_LIBRARY_PATH"), Some("/work/glibc"));
        assert_eq!(value("TMPDIR"), Some("/work/tmp"));
        assert!(
            cargo_environment("/t", "/h", "/tmp", None)
                .iter()
                .all(|(key, _)| key != "LD_LIBRARY_PATH")
        );
    }

    #[test]
    fn the_output_must_be_linux_s_or_without_it_what_the_source_prints() {
        let known = known_output("x86_64");
        assert!(known.starts_with("Hello, world!\n") && known.ends_with("linux x86_64\n"));
        assert_eq!(compare("x86_64", known.as_bytes(), None, None), Ok(()));
        assert!(compare("x86_64", b"Hello\n", None, None).is_err());
        let linux = LinuxRun {
            output: b"Hello, world!\n".to_vec(),
            program_sha256: "ab".into(),
        };
        assert_eq!(
            compare("x86_64", b"Hello, world!\n", Some(&linux), Some("cd")),
            Ok(())
        );
        let error = compare("x86_64", known.as_bytes(), Some(&linux), None).unwrap_err();
        assert!(error.contains("on Linux"), "{error}");
    }

    #[test]
    fn the_test_crate_is_its_own_workspace_with_a_lock_file() {
        let folder = crate::workspace_root().join("tests").join("hello-crate");
        let manifest = std::fs::read_to_string(folder.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(&format!("name = \"{PROGRAM}\"")));
        assert!(manifest.contains("\n[workspace]\n"));
        let lock = std::fs::read_to_string(folder.join("Cargo.lock")).unwrap();
        assert!(lock.contains(&format!("name = \"{PROGRAM}\"")));
        assert!(!lock.contains("source = "), "no dependencies");
    }
}

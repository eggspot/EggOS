//! `cargo xtask dynamic-linking` (D167): our dynamically linked hello (`tests/linux-dynamic`,
//! our own source, no C) built against glibc and against musl, booted with their dynamic
//! loaders and C libraries in the release image, on each architecture.
//!
//! The C libraries are test data, never shipped and never committed (decided by research):
//! glibc's loader and `libc.so.6` come from the host's own packages (x86_64) or the Ubuntu
//! cross compilers' (`gcc-aarch64-linux-gnu`, `gcc-riscv64-linux-gnu`), which also link the
//! glibc build; musl's `ld.so` (which is its `libc.so`) and its start file `Scrt1.o` come from
//! Alpine's `musl` and `musl-dev` packages of a pinned branch, downloaded with `curl` and
//! unpacked with `tar` into `target/dynamic-linking`, and the musl build is linked by our
//! toolchain's `rust-lld` against them. Each loader goes where its program's `PT_INTERP`
//! names it; glibc's `libc.so.6` goes to [`GLIBC_LIBRARY_FOLDER`], which the glibc build names
//! as its run path, and musl needs nothing more (its loader is its C library).

use crate::qemu::dynamic_linking::{GLIBC_PROGRAM_PATH, MUSL_PROGRAM_PATH};
use crate::{Arch, build_executable, build_release_kernel, initramfs, release_initramfs};
use std::path::{Path, PathBuf};
use std::process::Command;

const PACKAGE: &str = "eggos-linux-dynamic";

/// Where the glibc build finds `libc.so.6` in the image: its run path (`DT_RUNPATH`).
pub const GLIBC_LIBRARY_FOLDER: &str = "/usr/lib/eggos-tests/glibc";

const ALPINE: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.24/main";

/// The revision of Alpine's `musl` and `musl-dev` the musl build links and boots against.
pub(crate) const MUSL_VERSION: &str = "1.2.6-r3";

/// Each of the two packages' SHA-256 on each architecture, pinned like every other Alpine
/// download: a new revision on the mirror fails the check until it is pinned here.
const MUSL_PACKAGES: [(&str, [(&str, &str); 3]); 2] = [
    (
        "musl",
        [
            (
                "x86_64",
                "e142255d0fa397f14318996bc10378093dcc069dc3dd1b6818c8f28a789c21da",
            ),
            (
                "aarch64",
                "2e17864102278d9049c3a56f1bceed9e8cf287065617e579325123443daed8a5",
            ),
            (
                "riscv64",
                "f21b18f22a511de1e57f70112f761d0daad0c7d74e0734a82fcd5e037387dbb4",
            ),
        ],
    ),
    (
        "musl-dev",
        [
            (
                "x86_64",
                "310b51cff12e1d5ce5f135c558a82e2550427da5465bbd16c9ff9b05e63ee3d9",
            ),
            (
                "aarch64",
                "45afe0a9f78de3402b6b5d11f5153ed955bf59d59b45344d9c3fed9f4bbf2f66",
            ),
            (
                "riscv64",
                "f47066d861413b3d76fe200240ea2919cdf807f82770d574a16d10dbd6b56ad7",
            ),
        ],
    ),
];

/// What the glibc build needs on the host for one architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Glibc {
    /// Rust's target.
    pub(crate) target: &'static str,
    /// Cargo's variable that names the target's linker, and the linker.
    pub(crate) linker: (&'static str, &'static str),
    /// The folder of the host that holds the dynamic loader and `libc.so.6`.
    pub(crate) libraries: &'static str,
    /// The loader's file name there.
    pub(crate) loader: &'static str,
}

pub(crate) fn glibc(arch: &str) -> Option<Glibc> {
    Some(match arch {
        "x86_64" => Glibc {
            target: "x86_64-unknown-linux-gnu",
            linker: ("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER", "gcc"),
            libraries: "/lib/x86_64-linux-gnu",
            loader: "ld-linux-x86-64.so.2",
        },
        "aarch64" => Glibc {
            target: "aarch64-unknown-linux-gnu",
            linker: (
                "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER",
                "aarch64-linux-gnu-gcc",
            ),
            libraries: "/usr/aarch64-linux-gnu/lib",
            loader: "ld-linux-aarch64.so.1",
        },
        "riscv64" => Glibc {
            target: "riscv64gc-unknown-linux-gnu",
            linker: (
                "CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER",
                "riscv64-linux-gnu-gcc",
            ),
            libraries: "/usr/riscv64-linux-gnu/lib",
            loader: "ld-linux-riscv64-lp64d.so.1",
        },
        _ => return None,
    })
}

fn musl_loader(arch: &str) -> String {
    format!("ld-musl-{arch}.so.1")
}

/// Builds both programs for `arch`, the release image with them and their libraries, and
/// boots it ([`crate::qemu::dynamic_linking::check`]).
pub fn check(arch: &Arch) -> Result<(), String> {
    let glibc =
        glibc(arch.name).ok_or_else(|| format!("{}: no glibc build is known for it", arch.name))?;
    let host_loader = Path::new(glibc.libraries).join(glibc.loader);
    let host_libc = Path::new(glibc.libraries).join("libc.so.6");
    for file in [&host_loader, &host_libc] {
        if !file.is_file() {
            return Err(format!(
                "{}: {} is missing: the glibc build needs a Linux host with the target's C \
                 library (on Ubuntu, `apt-get install gcc-aarch64-linux-gnu \
                 gcc-riscv64-linux-gnu`; x86_64's is the host's own)",
                arch.name,
                file.display()
            ));
        }
    }
    let musl = fetch_musl(arch.name)?;
    let glibc_program = build_glibc(arch, &glibc)?;
    let musl_program = build_musl(arch, &musl)?;
    let glibc_loader_path = image_path(&interpreter_of_file(&glibc_program)?)?;
    let musl_loader_path = image_path(&interpreter_of_file(&musl_program)?)?;
    let glibc_libc_path = format!("{}/libc.so.6", &GLIBC_LIBRARY_FOLDER[1..]);
    let musl_loader_host = musl.join("lib").join(musl_loader(arch.name));
    let strings = [
        glibc_program.to_string_lossy().into_owned(),
        host_loader.to_string_lossy().into_owned(),
        host_libc.to_string_lossy().into_owned(),
        musl_program.to_string_lossy().into_owned(),
        musl_loader_host.to_string_lossy().into_owned(),
    ];
    let extra = [
        (GLIBC_PROGRAM_PATH, initramfs::Content::Program(&strings[0])),
        (
            glibc_loader_path.as_str(),
            initramfs::Content::Program(&strings[1]),
        ),
        (
            glibc_libc_path.as_str(),
            initramfs::Content::Program(&strings[2]),
        ),
        (MUSL_PROGRAM_PATH, initramfs::Content::Program(&strings[3])),
        (
            musl_loader_path.as_str(),
            initramfs::Content::Program(&strings[4]),
        ),
    ];
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-dynamic-linking-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    crate::qemu::dynamic_linking::check(arch, &kernel)
}

fn build_glibc(arch: &Arch, glibc: &Glibc) -> Result<PathBuf, String> {
    let run_path = format!("link-arg=-Wl,-rpath,{GLIBC_LIBRARY_FOLDER}");
    let rustflags = ["-C", "debuginfo=0", "-C", "strip=symbols", "-C", &run_path].join("\x1f");
    let target_dir = crate::target_directory()
        .join("dynamic-linking")
        .join("glibc");
    let target_dir = target_dir.to_string_lossy();
    let (linker_variable, linker) = glibc.linker;
    build_executable(
        &["build", "-p", PACKAGE, "--target", glibc.target],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str()),
            ("CARGO_TARGET_DIR", &target_dir),
            (linker_variable, linker),
        ],
        &format!("{PACKAGE} against glibc for {}", glibc.target),
    )
    .map_err(|error| {
        format!(
            "{error} ({}: the glibc build needs `rustup target add {}` and the linker `{linker}`)",
            arch.name, glibc.target
        )
    })
}

fn build_musl(arch: &Arch, sysroot: &Path) -> Result<PathBuf, String> {
    let libraries = sysroot.join("usr").join("lib");
    let start = format!("link-arg={}", libraries.join("Scrt1.o").display());
    let loader = format!("link-arg=--dynamic-linker=/lib/{}", musl_loader(arch.name));
    let search = libraries.to_string_lossy().into_owned();
    let rustflags = [
        "-C",
        "link-self-contained=no",
        "-C",
        "target-feature=-crt-static",
        "-C",
        "relocation-model=pie",
        "-C",
        "debuginfo=0",
        "-C",
        "strip=symbols",
        "-L",
        &search,
        "-C",
        &start,
        "-C",
        &loader,
    ]
    .join("\x1f");
    let target_dir = crate::target_directory()
        .join("dynamic-linking")
        .join("musl");
    let target_dir = target_dir.to_string_lossy();
    build_executable(
        &["build", "-p", PACKAGE, "--target", arch.user_target],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str()),
            ("CARGO_TARGET_DIR", &target_dir),
        ],
        &format!("{PACKAGE} against musl for {}", arch.user_target),
    )
}

fn fetch_musl(arch: &str) -> Result<PathBuf, String> {
    let root = crate::target_directory().join("dynamic-linking");
    let sysroot = root.join(format!("musl-{arch}"));
    let loader = sysroot.join("lib").join(musl_loader(arch));
    let libc = sysroot.join("usr").join("lib").join("libc.so");
    let start = sysroot.join("usr").join("lib").join("Scrt1.o");
    if loader.is_file() && libc.is_file() && start.is_file() {
        return Ok(sysroot);
    }
    let downloads = root.join(format!("downloads-{arch}"));
    for folder in [&sysroot, &downloads] {
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    for (package, pins) in MUSL_PACKAGES {
        let sha256 = pins
            .iter()
            .find_map(|&(pinned_arch, sha256)| (pinned_arch == arch).then_some(sha256))
            .ok_or_else(|| format!("{arch}: no `{package}` package is pinned for it"))?;
        let file = format!("{package}-{MUSL_VERSION}.apk");
        let apk = downloads.join(&file);
        crate::sdl_app::fetch_pinned(&format!("{ALPINE}/{arch}/{file}"), &apk, sha256)?;
        crate::sdl_app::untar(&apk, &sysroot)?;
    }
    if !loader.is_file() || !start.is_file() {
        return Err(format!(
            "{}: Alpine's musl packages did not hold {} and Scrt1.o",
            sysroot.display(),
            musl_loader(arch)
        ));
    }
    if std::fs::symlink_metadata(&libc).is_ok() {
        std::fs::remove_file(&libc).map_err(|e| format!("{}: {e}", libc.display()))?;
    }
    std::fs::copy(&loader, &libc)
        .map(drop)
        .map_err(|e| format!("{} to {}: {e}", loader.display(), libc.display()))?;
    Ok(sysroot)
}

pub(crate) fn download(url: &str, to: &Path) -> Result<(), String> {
    eprintln!("xtask: downloading {url}");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(to)
        .arg(url)
        .status()
        .map_err(|e| format!("could not run curl: {e}"))?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("curl {url}: {status}")),
    }
}

pub(crate) fn interpreter_of_file(program: &Path) -> Result<String, String> {
    let image = std::fs::read(program).map_err(|e| format!("{}: {e}", program.display()))?;
    interpreter_of(&image).map_err(|error| format!("{}: {error}", program.display()))
}

fn interpreter_of(image: &[u8]) -> Result<String, String> {
    const PT_INTERP: u32 = 3;
    let field = |at: usize, size: usize| -> Result<u64, String> {
        let bytes = image.get(at..at + size).ok_or("the ELF image ends early")?;
        let mut word = [0u8; 8];
        word[..size].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(word))
    };
    if !image.starts_with(b"\x7fELF") || image.get(4..6) != Some(&[2, 1]) {
        return Err("not a little-endian ELF-64 image".into());
    }
    let table = field(32, 8)? as usize;
    let entry_size = field(54, 2)? as usize;
    let count = field(56, 2)? as usize;
    for index in 0..count {
        let header = table + index * entry_size;
        if field(header, 4)? as u32 != PT_INTERP {
            continue;
        }
        let offset = field(header + 8, 8)? as usize;
        let size = field(header + 32, 8)? as usize;
        let path = image
            .get(offset..offset + size)
            .ok_or("the interpreter's path is not in the image")?;
        let path = path.split(|&byte| byte == 0).next().unwrap_or_default();
        return String::from_utf8(path.to_vec()).map_err(|_| "a path that is not UTF-8".into());
    }
    Err("the program names no interpreter: it was not linked dynamically".into())
}

/// An absolute path in the image as an initramfs member's name.
pub(crate) fn image_path(path: &str) -> Result<String, String> {
    path.strip_prefix('/')
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("`{path}` is not an absolute path"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn musl_is_pinned_for_every_architecture_the_job_boots_and_matches_the_sdl_app_s() {
        for (package, pins) in MUSL_PACKAGES {
            for arch in ["x86_64", "aarch64", "riscv64"] {
                assert!(glibc(arch).is_some());
                let sha256 = pins.iter().find(|&&(pinned, _)| pinned == arch).unwrap().1;
                assert_eq!(sha256.len(), 64, "{package} {arch}");
                assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
        }
        let sdl_musl = crate::sdl_app::PACKAGES
            .iter()
            .find(|package| package.name == "musl")
            .unwrap();
        assert_eq!(sdl_musl.version, MUSL_VERSION);
        assert_eq!(sdl_musl.sha256("x86_64"), Some(MUSL_PACKAGES[0].1[0].1));
        assert_eq!(sdl_musl.sha256("aarch64"), Some(MUSL_PACKAGES[0].1[1].1));
    }

    fn program_naming(path: &[u8]) -> Vec<u8> {
        let mut image = vec![0u8; 0x200];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = 2;
        image[5] = 1;
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&2u16.to_le_bytes());
        image[64..68].copy_from_slice(&6u32.to_le_bytes());
        let interp = 64 + 56;
        image[interp..interp + 4].copy_from_slice(&3u32.to_le_bytes());
        image[interp + 8..interp + 16].copy_from_slice(&0x100u64.to_le_bytes());
        image[interp + 32..interp + 40].copy_from_slice(&(path.len() as u64).to_le_bytes());
        image[0x100..0x100 + path.len()].copy_from_slice(path);
        image
    }

    #[test]
    fn the_interpreter_is_read_from_the_program_header() {
        let image = program_naming(b"/lib/ld-musl-riscv64.so.1\0");
        assert_eq!(
            interpreter_of(&image).as_deref(),
            Ok("/lib/ld-musl-riscv64.so.1")
        );
        let mut static_program = image.clone();
        static_program[64 + 56..64 + 60].copy_from_slice(&1u32.to_le_bytes());
        assert!(interpreter_of(&static_program).is_err());
        assert!(interpreter_of(b"\x7fELF").is_err());
        assert_eq!(
            image_path("/lib64/ld-linux-x86-64.so.2").as_deref(),
            Ok("lib64/ld-linux-x86-64.so.2")
        );
        assert!(image_path("lib/ld.so").is_err());
    }

    #[test]
    fn every_architecture_has_its_glibc_and_musl_names() {
        for arch in crate::ARCHES {
            let glibc = glibc(arch.name).unwrap();
            assert!(glibc.target.starts_with(arch.name));
            assert!(glibc.linker.0.ends_with("_LINKER"));
            assert!(musl_loader(arch.name).starts_with("ld-musl-"));
        }
        assert_eq!(glibc("mips64"), None);
        assert!(GLIBC_LIBRARY_FOLDER.starts_with('/'));
    }
}

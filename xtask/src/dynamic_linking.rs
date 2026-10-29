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

/// The test program's package.
const PACKAGE: &str = "eggos-linux-dynamic";

/// Where the glibc build finds `libc.so.6` in the image: its run path (`DT_RUNPATH`).
pub const GLIBC_LIBRARY_FOLDER: &str = "/usr/lib/eggos-tests/glibc";

/// Alpine's mirror and the branch whose musl the check uses, pinned so runs are reproducible.
const ALPINE: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.24/main";

/// What the glibc build needs on the host for one architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Glibc {
    /// Rust's target.
    target: &'static str,
    /// Cargo's variable that names the target's linker, and the linker.
    linker: (&'static str, &'static str),
    /// The folder of the host that holds the dynamic loader and `libc.so.6`.
    libraries: &'static str,
    /// The loader's file name there.
    loader: &'static str,
}

fn glibc(arch: &str) -> Option<Glibc> {
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

/// musl's dynamic loader's name for `arch`, which is also its C library.
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

/// Downloads and unpacks Alpine's `musl` and `musl-dev` for `arch` into
/// `target/dynamic-linking/musl-<arch>`, unless an earlier run did, and returns that folder.
/// Its `usr/lib/libc.so`, a link to `/lib/...` in the package, becomes a copy of the loader, so
/// the linker finds the C library there.
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
    let index = downloads.join("APKINDEX.tar.gz");
    download(&format!("{ALPINE}/{arch}/APKINDEX.tar.gz"), &index)?;
    unpack(&index, &downloads)?;
    let listing = downloads.join("APKINDEX");
    let listing =
        std::fs::read_to_string(&listing).map_err(|e| format!("{}: {e}", listing.display()))?;
    for package in ["musl", "musl-dev"] {
        let version = package_version(&listing, package)
            .ok_or_else(|| format!("Alpine's index for {arch} has no `{package}`"))?;
        let file = format!("{package}-{version}.apk");
        let apk = downloads.join(&file);
        download(&format!("{ALPINE}/{arch}/{file}"), &apk)?;
        unpack(&apk, &sysroot)?;
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

fn download(url: &str, to: &Path) -> Result<(), String> {
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

/// Unpacks `archive`, a gzip'd tar stream (an Alpine package is several), into `into`. `tar`
/// warns about the packages' own header records; what matters is the files it writes, which
/// the caller checks.
fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map(drop)
        .map_err(|e| format!("could not run tar: {e}"))
}

/// The version of `package` in an Alpine index (`APKINDEX`): blocks of `key:value` lines,
/// a blank line between them, `P:` the name and `V:` the version.
fn package_version(index: &str, package: &str) -> Option<String> {
    index.split("\n\n").find_map(|block| {
        let mut name = None;
        let mut version = None;
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("P:") {
                name = Some(value);
            } else if let Some(value) = line.strip_prefix("V:") {
                version = Some(value);
            }
        }
        (name == Some(package)).then(|| version.map(str::to_owned))?
    })
}

fn interpreter_of_file(program: &Path) -> Result<String, String> {
    let image = std::fs::read(program).map_err(|e| format!("{}: {e}", program.display()))?;
    interpreter_of(&image).map_err(|error| format!("{}: {error}", program.display()))
}

/// The path a little-endian ELF-64 program names as its interpreter (`PT_INTERP`).
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
fn image_path(path: &str) -> Result<String, String> {
    path.strip_prefix('/')
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("`{path}` is not an absolute path"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_gives_each_package_s_version() {
        let index = "C:Q1abc=\nP:musl\nV:1.2.6-r3\nA:x86_64\n\nC:Q1def=\nP:musl-dev\nV:1.2.6-r3\n\n\
                     P:musl-utils\nV:1.2.6-r4\n";
        assert_eq!(package_version(index, "musl").as_deref(), Some("1.2.6-r3"));
        assert_eq!(
            package_version(index, "musl-dev").as_deref(),
            Some("1.2.6-r3")
        );
        assert_eq!(package_version(index, "musl-ut"), None);
        assert_eq!(package_version(index, "glibc"), None);
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

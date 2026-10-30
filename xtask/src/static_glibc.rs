//! `cargo xtask static-glibc` (D188): our Linux console program (`tests/linux-static`, our own
//! source, no C) linked statically against glibc, as a static executable and as a static PIE
//! that relocates itself, added to the release image and run from the shell on each
//! architecture ([`crate::qemu::static_glibc`]).
//!
//! glibc's static library and start files are test data, never shipped and never committed,
//! as for the dynamic linking test (D167): they come from the host's own packages (x86_64) or
//! the Ubuntu cross compilers' (`gcc-aarch64-linux-gnu`, `gcc-riscv64-linux-gnu`), whose
//! driver links the program. Rust's `*-unknown-linux-gnu` targets make a static PIE with
//! `+crt-static` and their default relocation model, and a static executable at a fixed
//! address with `relocation-model=static`; each build is checked to be the kind it should be
//! before it is booted, since a linker without static PIE support quietly makes the other.

use crate::dynamic_linking::{Glibc, glibc};
use crate::qemu::static_glibc::{PIE_PROGRAM_PATH, STATIC_PROGRAM_PATH};
use crate::{Arch, build_executable, build_release_kernel, initramfs, release_initramfs};
use std::path::{Path, PathBuf};

const PACKAGE: &str = "eggos-linux-static";

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_INTERP: u32 = 3;

/// The two kinds of static program the test boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Linkage {
    /// A static executable at a fixed address (`ET_EXEC`, `crt1.o`).
    Executable,
    /// A static PIE (`ET_DYN` with no interpreter, `rcrt1.o`).
    PositionIndependent,
}

impl Linkage {
    fn relocation_model(self) -> &'static str {
        match self {
            Self::Executable => "static",
            Self::PositionIndependent => "pic",
        }
    }

    fn start_file(self) -> &'static str {
        match self {
            Self::Executable => "crt1.o",
            Self::PositionIndependent => "rcrt1.o",
        }
    }

    fn elf_type(self) -> u16 {
        match self {
            Self::Executable => ET_EXEC,
            Self::PositionIndependent => ET_DYN,
        }
    }

    fn image_path(self) -> &'static str {
        match self {
            Self::Executable => STATIC_PROGRAM_PATH,
            Self::PositionIndependent => PIE_PROGRAM_PATH,
        }
    }
}

const LINKAGES: [Linkage; 2] = [Linkage::Executable, Linkage::PositionIndependent];

/// Builds both programs for `arch`, the release image with them, and boots it.
pub fn check(arch: &Arch) -> Result<(), String> {
    let glibc =
        glibc(arch.name).ok_or_else(|| format!("{}: no glibc build is known for it", arch.name))?;
    let needed = ["libc.a"]
        .into_iter()
        .chain(LINKAGES.map(Linkage::start_file))
        .map(|file| Path::new(glibc.libraries).join(file));
    for file in needed {
        if !file.is_file() {
            return Err(format!(
                "{}: {} is missing: the static glibc build needs a Linux host with the \
                 target's static C library (on Ubuntu, `apt-get install libc6-dev` for x86_64, \
                 `gcc-aarch64-linux-gnu` or `gcc-riscv64-linux-gnu` for the others)",
                arch.name,
                file.display()
            ));
        }
    }
    let mut programs = Vec::new();
    for linkage in LINKAGES {
        let program = build(arch, &glibc, linkage)?;
        let image = std::fs::read(&program).map_err(|e| format!("{}: {e}", program.display()))?;
        is_static(&image, linkage).map_err(|error| format!("{}: {error}", program.display()))?;
        programs.push((linkage.image_path(), program.to_string_lossy().into_owned()));
    }
    let extra: Vec<(&str, initramfs::Content)> = programs
        .iter()
        .map(|(path, host)| (*path, initramfs::Content::Program(host)))
        .collect();
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-static-glibc-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    crate::qemu::static_glibc::check(arch, &kernel)?;
    eprintln!("xtask: {}: static glibc boot test passed", arch.name);
    Ok(())
}

fn build(arch: &Arch, glibc: &Glibc, linkage: Linkage) -> Result<PathBuf, String> {
    let relocation_model = format!("relocation-model={}", linkage.relocation_model());
    let rustflags = [
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
    let target_dir = crate::target_directory()
        .join("static-glibc")
        .join(linkage.relocation_model());
    let target_dir = target_dir.to_string_lossy();
    let (linker_variable, linker) = glibc.linker;
    build_executable(
        &["build", "-p", PACKAGE, "--target", glibc.target],
        &[
            ("CARGO_ENCODED_RUSTFLAGS", rustflags.as_str()),
            ("CARGO_TARGET_DIR", &target_dir),
            (linker_variable, linker),
        ],
        &format!(
            "{PACKAGE} statically against glibc ({relocation_model}) for {}",
            glibc.target
        ),
    )
    .map_err(|error| {
        format!(
            "{error} ({}: the static glibc build needs `rustup target add {}` and the linker \
             `{linker}`)",
            arch.name, glibc.target
        )
    })
}

/// Checks that `image` is a static program of `linkage`'s kind: its ELF type, and no
/// interpreter.
fn is_static(image: &[u8], linkage: Linkage) -> Result<(), String> {
    let field = |at: usize, size: usize| -> Result<u64, String> {
        let bytes = image.get(at..at + size).ok_or("the ELF image ends early")?;
        let mut word = [0u8; 8];
        word[..size].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(word))
    };
    if !image.starts_with(b"\x7fELF") || image.get(4..6) != Some(&[2, 1]) {
        return Err("not a little-endian ELF-64 image".into());
    }
    let elf_type = field(16, 2)? as u16;
    if elf_type != linkage.elf_type() {
        return Err(format!(
            "ELF type {elf_type}, not {} (the linker made the other kind of static program)",
            linkage.elf_type()
        ));
    }
    let table = field(32, 8)? as usize;
    let entry_size = field(54, 2)? as usize;
    let count = field(56, 2)? as usize;
    for index in 0..count {
        if field(table + index * entry_size, 4)? as u32 == PT_INTERP {
            return Err("it names an interpreter: it was linked dynamically".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(elf_type: u16, first_segment: u32) -> Vec<u8> {
        let mut image = vec![0u8; 0x100];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = 2;
        image[5] = 1;
        image[16..18].copy_from_slice(&elf_type.to_le_bytes());
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&1u16.to_le_bytes());
        image[64..68].copy_from_slice(&first_segment.to_le_bytes());
        image
    }

    #[test]
    fn each_build_must_be_the_static_kind_it_was_asked_for() {
        const PT_LOAD: u32 = 1;
        assert_eq!(
            is_static(&image(ET_EXEC, PT_LOAD), Linkage::Executable),
            Ok(())
        );
        assert_eq!(
            is_static(&image(ET_DYN, PT_LOAD), Linkage::PositionIndependent),
            Ok(())
        );
        assert!(is_static(&image(ET_EXEC, PT_LOAD), Linkage::PositionIndependent).is_err());
        assert!(is_static(&image(ET_DYN, PT_LOAD), Linkage::Executable).is_err());
        assert!(is_static(&image(ET_DYN, PT_INTERP), Linkage::PositionIndependent).is_err());
        assert!(is_static(b"\x7fELF", Linkage::Executable).is_err());
    }

    #[test]
    fn each_linkage_has_its_own_start_file_path_and_target_folder() {
        assert_ne!(
            Linkage::Executable.start_file(),
            Linkage::PositionIndependent.start_file()
        );
        assert_ne!(
            Linkage::Executable.image_path(),
            Linkage::PositionIndependent.image_path()
        );
        assert_ne!(
            Linkage::Executable.relocation_model(),
            Linkage::PositionIndependent.relocation_model()
        );
    }
}

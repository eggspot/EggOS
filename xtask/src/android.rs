//! Android console programs (D119): static programs linked by the Android NDK against bionic,
//! which run through the Linux layer. An image gets `/system/bin/sh`, the shell bionic's
//! `system` and `popen` run, only when an added file is an Android program; the boot test's
//! program is built from `tests/android-console` and linked by the NDK's clang, found through
//! the NDK's usual environment variables.

use crate::initramfs::Content;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const API_LEVEL: u32 = 29;

const SYSTEM_SHELL_PATH: &str = "system/bin/sh";
const NOTE_SEGMENT: u32 = 4;
const NOTE_OWNER: &[u8] = b"Android\0";
const NOTE_TYPE_IDENT: u32 = 1;
const PROGRAM_HEADER_SIZE: usize = 56;
const NDK_ROOT_VARIABLES: [&str; 3] = [
    "ANDROID_NDK_HOME",
    "ANDROID_NDK_LATEST_HOME",
    "ANDROID_NDK_ROOT",
];
const SDK_ROOT_VARIABLES: [&str; 2] = ["ANDROID_HOME", "ANDROID_SDK_ROOT"];

/// Rust's (and the NDK's) Android target for the architecture named `arch`, if Rust ships one
/// with a prebuilt standard library.
pub fn target(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("aarch64-linux-android"),
        "x86_64" => Some("x86_64-linux-android"),
        _ => None,
    }
}

/// The API level in the `.note.android.ident` note of a 64-bit little-endian ELF file (the
/// note the NDK's start-up objects put in every Android program), `None` without one.
pub fn api_level(elf: &[u8]) -> Option<u32> {
    let u16_at = |at: usize| {
        Some(u16::from_le_bytes(
            elf.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    let u32_at = |bytes: &[u8], at: usize| {
        Some(u32::from_le_bytes(
            bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let u64_at = |at: usize| {
        Some(u64::from_le_bytes(
            elf.get(at..at.checked_add(8)?)?.try_into().ok()?,
        ))
    };
    if elf.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let headers = usize::try_from(u64_at(0x20)?).ok()?;
    let count = usize::from(u16_at(0x38)?);
    (0..count).find_map(|index| {
        let header = headers.checked_add(index.checked_mul(PROGRAM_HEADER_SIZE)?)?;
        if u32_at(elf, header)? != NOTE_SEGMENT {
            return None;
        }
        let offset = usize::try_from(u64_at(header.checked_add(8)?)?).ok()?;
        let size = usize::try_from(u64_at(header.checked_add(32)?)?).ok()?;
        android_ident(elf.get(offset..offset.checked_add(size)?)?, u32_at)
    })
}

fn android_ident(mut notes: &[u8], u32_at: impl Fn(&[u8], usize) -> Option<u32>) -> Option<u32> {
    let padded = |length: u32| usize::try_from(length.checked_next_multiple_of(4)?).ok();
    while notes.len() >= 12 {
        let owner_length = u32_at(notes, 0)?;
        let description_length = u32_at(notes, 4)?;
        let kind = u32_at(notes, 8)?;
        let description_start = padded(owner_length)?.checked_add(12)?;
        let end = description_start.checked_add(padded(description_length)?)?;
        let owner = notes.get(12..usize::try_from(owner_length).ok()?.checked_add(12)?)?;
        if owner == NOTE_OWNER && kind == NOTE_TYPE_IDENT && description_length >= 4 {
            return u32_at(notes, description_start);
        }
        notes = notes.get(end..)?;
    }
    None
}

/// Whether the file at `path` is an Android program ([`api_level`] finds its note).
pub fn is_program(path: &str) -> bool {
    std::fs::read(path).is_ok_and(|elf| api_level(&elf).is_some())
}

/// What an image with an Android program gets: `/system/bin/sh`, another name for `eggsh`
/// (at `shell`, an earlier member).
pub fn system_members(shell: &str) -> [(&'static str, Content<'_>); 1] {
    [(SYSTEM_SHELL_PATH, Content::HardLink(shell))]
}

/// An Android NDK on this host: its clang, which links for every Android target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ndk {
    pub clang: PathBuf,
}

/// The NDK the environment names: `ANDROID_NDK_HOME`, `ANDROID_NDK_LATEST_HOME` or
/// `ANDROID_NDK_ROOT`, else the newest version in `ndk/` of the SDK at `ANDROID_HOME` or
/// `ANDROID_SDK_ROOT`. `None` if none of them holds an NDK with clang.
pub fn find_ndk() -> Option<Ndk> {
    candidates(|name| std::env::var_os(name))
        .iter()
        .find_map(|root| clang_in(root).map(|clang| Ndk { clang }))
}

fn candidates(variable: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = NDK_ROOT_VARIABLES
        .iter()
        .filter_map(|name| variable(name))
        .map(PathBuf::from)
        .collect();
    for sdk in SDK_ROOT_VARIABLES.iter().filter_map(|name| variable(name)) {
        let versions = Path::new(&sdk).join("ndk");
        let Ok(entries) = std::fs::read_dir(&versions) else {
            continue;
        };
        let names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        if let Some(newest) = newest_version(names.iter().map(String::as_str)) {
            roots.push(versions.join(newest));
        }
    }
    roots
}

fn newest_version<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let parts = |name: &str| -> Option<Vec<u64>> {
        name.split('.').map(|part| part.parse().ok()).collect()
    };
    names
        .filter_map(|name| parts(name).map(|version| (version, name)))
        .max()
        .map(|(_, name)| name)
}

fn clang_in(root: &Path) -> Option<PathBuf> {
    let prebuilt = root.join("toolchains").join("llvm").join("prebuilt");
    let mut hosts: Vec<PathBuf> = std::fs::read_dir(prebuilt)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    hosts.sort();
    hosts.iter().find_map(|host| {
        ["clang", "clang.exe"]
            .iter()
            .map(|name| host.join("bin").join(name))
            .find(|clang| clang.is_file())
    })
}

/// The arguments that link `archive` (a static library defining `main`) into a static
/// Android program at `output` for `target`, with bionic's start-up code and C library.
pub fn link_arguments(target: &str, archive: &Path, output: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        format!("--target={target}{API_LEVEL}"),
        "-static".into(),
        "-Wl,--gc-sections".into(),
        "-o".into(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    arguments.push(output.into());
    arguments.push(archive.into());
    arguments
}

/// Links `archive` into the static Android program `output` with the NDK's clang.
pub fn link(ndk: &Ndk, target: &str, archive: &Path, output: &Path) -> Result<(), String> {
    let arguments = link_arguments(target, archive, output);
    eprintln!(
        "xtask: {} {}",
        ndk.clang.display(),
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let status = Command::new(&ndk.clang)
        .args(&arguments)
        .status()
        .map_err(|e| format!("could not run {}: {e}", ndk.clang.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "linking {} with the NDK's clang failed ({status})",
            output.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(owner: &[u8], kind: u32, description: &[u8]) -> Vec<u8> {
        let mut note = Vec::new();
        note.extend((owner.len() as u32).to_le_bytes());
        note.extend((description.len() as u32).to_le_bytes());
        note.extend(kind.to_le_bytes());
        note.extend(owner);
        note.resize(note.len().next_multiple_of(4), 0);
        note.extend(description);
        note.resize(note.len().next_multiple_of(4), 0);
        note
    }

    fn elf_with_notes(notes: &[u8]) -> Vec<u8> {
        let headers = 64;
        let notes_at = headers + 2 * PROGRAM_HEADER_SIZE;
        let mut elf = vec![0u8; notes_at];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[0x20..0x28].copy_from_slice(&(headers as u64).to_le_bytes());
        elf[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes());
        let load = headers;
        elf[load..load + 4].copy_from_slice(&1u32.to_le_bytes());
        let segment = headers + PROGRAM_HEADER_SIZE;
        elf[segment..segment + 4].copy_from_slice(&NOTE_SEGMENT.to_le_bytes());
        elf[segment + 8..segment + 16].copy_from_slice(&(notes_at as u64).to_le_bytes());
        elf[segment + 32..segment + 40].copy_from_slice(&(notes.len() as u64).to_le_bytes());
        elf.extend(notes);
        elf
    }

    #[test]
    fn the_android_note_gives_the_api_level_after_other_notes() {
        let mut notes = note(b"GNU\0", 3, &[0xab; 20]);
        let mut description = 29u32.to_le_bytes().to_vec();
        description.extend([0u8; 128]);
        notes.extend(note(NOTE_OWNER, NOTE_TYPE_IDENT, &description));
        assert_eq!(api_level(&elf_with_notes(&notes)), Some(29));
        let short = note(NOTE_OWNER, NOTE_TYPE_IDENT, &35u32.to_le_bytes());
        assert_eq!(api_level(&elf_with_notes(&short)), Some(35));
    }

    #[test]
    fn programs_without_the_android_note_are_not_android_programs() {
        assert_eq!(
            api_level(&elf_with_notes(&note(b"GNU\0", 1, &[1; 16]))),
            None
        );
        let wrong_kind = note(NOTE_OWNER, 2, &29u32.to_le_bytes());
        assert_eq!(api_level(&elf_with_notes(&wrong_kind)), None);
        let empty = note(NOTE_OWNER, NOTE_TYPE_IDENT, &[]);
        assert_eq!(api_level(&elf_with_notes(&empty)), None);
        let mut cut = elf_with_notes(&note(NOTE_OWNER, NOTE_TYPE_IDENT, &[29, 0, 0, 0]));
        cut.truncate(cut.len() - 2);
        assert_eq!(api_level(&cut), None);
        assert_eq!(api_level(b"MZ\x90\x00"), None);
        assert_eq!(api_level(&[]), None);
    }

    #[test]
    fn the_newest_numbered_ndk_version_wins() {
        let names = [
            "26.3.11579264",
            "27.3.13750724",
            "9.9",
            "27.10.1",
            "latest",
            ".tmp",
        ];
        assert_eq!(newest_version(names.into_iter()), Some("27.10.1"));
        assert_eq!(newest_version(["notes"].into_iter()), None);
    }

    #[test]
    fn ndk_variables_come_before_the_sdk_s_ndk_directory() {
        let roots = candidates(|name| match name {
            "ANDROID_NDK_ROOT" => Some("/opt/ndk-root".into()),
            "ANDROID_NDK_HOME" => Some("/opt/ndk-home".into()),
            _ => None,
        });
        assert_eq!(
            roots,
            [
                PathBuf::from("/opt/ndk-home"),
                PathBuf::from("/opt/ndk-root")
            ]
        );
    }

    #[test]
    fn the_program_is_linked_statically_for_the_api_level() {
        let arguments = link_arguments(
            "aarch64-linux-android",
            Path::new("lib.a"),
            Path::new("out"),
        );
        let arguments: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            [
                "--target=aarch64-linux-android29",
                "-static",
                "-Wl,--gc-sections",
                "-o",
                "out",
                "lib.a"
            ]
        );
    }

    #[test]
    fn only_architectures_with_a_prebuilt_android_target_have_one() {
        assert_eq!(target("aarch64"), Some("aarch64-linux-android"));
        assert_eq!(target("x86_64"), Some("x86_64-linux-android"));
        assert_eq!(target("riscv64"), None);
    }
}

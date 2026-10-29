//! `cargo xtask rustc-version` (D168, the CI track's stage 2, milestone 1): the official Rust
//! toolchain's `rustc`, downloaded as test data and never shipped, runs `rustc --version` inside
//! EggOS.
//!
//! The `rustc` component of the pinned release for the architecture's `-unknown-linux-gnu`
//! target comes from static.rust-lang.org with `curl`, is checked against its pinned SHA-256 and
//! unpacked with `tar` into `target/rustc-version` (decided by research). Its `rustc` folder goes
//! into a work folder at `rustc`, beside `glibc`, which gets every shared library the toolchain's
//! programs and libraries need (`DT_NEEDED`, followed through the libraries copied) that the
//! toolchain does not hold itself, from this Linux host's C library folders: the host's own for
//! x86_64, the cross compilers' packages and the multiarch ones for aarch64. The folder is packed
//! into a work disk (D166), the dynamic loader `rustc` names (`PT_INTERP`) goes into the release
//! image at that path, as for the dynamic linking check (D167), and the boot runs
//! `/work/rustc/bin/rustc --version` with `LD_LIBRARY_PATH=/work/glibc`; the toolchain finds its
//! own libraries through its run path, `$ORIGIN/../lib`, which glibc's loader resolves by
//! reading `/proc/self/exe`.

use crate::dynamic_linking::{download, glibc, image_path, interpreter_of_file};
use crate::{Arch, build_release_kernel, initramfs, release_initramfs, work_disk};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Rust release whose `rustc` runs: the one `rust-toolchain.toml` pins.
pub const RUST_VERSION: &str = "1.99.1";

/// The day static.rust-lang.org published [`RUST_VERSION`], whose folder keeps it for good.
const RELEASE_DATE: &str = "2026-10-15";

/// What the line `rustc --version` writes starts with.
pub const VERSION_LINE_PREFIX: &str = "rustc 1.99.1 (";

/// Where the work folder's libraries are inside EggOS, for `LD_LIBRARY_PATH`.
pub const LIBRARY_FOLDER: &str = "/work/glibc";

/// Where `rustc` is inside EggOS.
pub const RUSTC_PATH: &str = "/work/rustc/bin/rustc";

/// The `rustc` component for one architecture: its target and the SHA-256 static.rust-lang.org
/// lists for its `.tar.xz`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Component {
    target: &'static str,
    sha256: &'static str,
}

fn component(arch: &str) -> Option<Component> {
    Some(match arch {
        "x86_64" => Component {
            target: "x86_64-unknown-linux-gnu",
            sha256: "2ff05ff9c08a90861e7f6e4065b40a8b87463b4576186fd613871c3e37f2091f",
        },
        "aarch64" => Component {
            target: "aarch64-unknown-linux-gnu",
            sha256: "acdcdfe09ae1f103aa9a2b7f88903a9588bc2e99288af10e6fbf5514e2e49f25",
        },
        _ => return None,
    })
}

impl Component {
    fn name(self) -> String {
        format!("rustc-{RUST_VERSION}-{}", self.target)
    }

    fn url(self) -> String {
        format!(
            "https://static.rust-lang.org/dist/{RELEASE_DATE}/{}.tar.xz",
            self.name()
        )
    }
}

/// The host folders a library for `arch` is looked for in, in order.
fn host_library_folders(arch: &str) -> &'static [&'static str] {
    match arch {
        "x86_64" => &["/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu"],
        "aarch64" => &[
            "/usr/aarch64-linux-gnu/lib",
            "/usr/lib/aarch64-linux-gnu",
            "/lib/aarch64-linux-gnu",
        ],
        _ => &[],
    }
}

/// Downloads and unpacks `rustc` for `arch`, packs it with its libraries into a work disk,
/// builds the release image with its dynamic loader and boots it
/// ([`crate::qemu::rustc_version::check`]).
pub fn check(arch: &Arch) -> Result<(), String> {
    let component = component(arch.name).ok_or_else(|| {
        format!(
            "{}: the official toolchain has no `rustc` for it (x86_64 and aarch64 do)",
            arch.name
        )
    })?;
    let glibc =
        glibc(arch.name).ok_or_else(|| format!("{}: no glibc is known for it", arch.name))?;
    let root = crate::target_directory().join("rustc-version");
    let archive = fetch(&root, component)?;
    let work = root.join(arch.name).join("work");
    unpack_into(&archive, &root.join(arch.name), component, &work)?;
    let copied = copy_libraries(&work, host_library_folders(arch.name))?;
    eprintln!(
        "xtask: {}: {} libraries from the host for rustc: {}",
        arch.name,
        copied.len(),
        copied.join(" ")
    );
    let rustc = work.join("rustc").join("bin").join("rustc");
    let loader_path = image_path(&interpreter_of_file(&rustc)?)?;
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
        .join(format!("eggos-{}-rustc-work.img", arch.name));
    if image.exists() {
        std::fs::remove_file(&image).map_err(|e| format!("{}: {e}", image.display()))?;
    }
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
        &format!("initramfs-rustc-version-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    crate::qemu::rustc_version::check(arch, &kernel, &image)
}

/// Downloads `component`'s archive into `root/downloads` unless an earlier run did, checks its
/// SHA-256 and returns its path.
fn fetch(root: &Path, component: Component) -> Result<PathBuf, String> {
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| format!("{}: {e}", downloads.display()))?;
    let archive = downloads.join(format!("{}.tar.xz", component.name()));
    if !archive.is_file() || sha256_of(&archive)? != component.sha256 {
        download(&component.url(), &archive)?;
    }
    let found = sha256_of(&archive)?;
    if found != component.sha256 {
        return Err(format!(
            "{}: SHA-256 {found}, not the pinned {}",
            archive.display(),
            component.sha256
        ));
    }
    Ok(archive)
}

/// The SHA-256 of the file at `path`, in lowercase hexadecimal.
fn sha256_of(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(crate::sha256::hex_digest(&bytes))
}

/// Unpacks `archive` into `scratch/unpacked` and moves its `rustc` folder to `work/rustc`, both
/// folders made anew.
fn unpack_into(
    archive: &Path,
    scratch: &Path,
    component: Component,
    work: &Path,
) -> Result<(), String> {
    let unpacked = scratch.join("unpacked");
    for folder in [&unpacked, work] {
        if folder.exists() {
            std::fs::remove_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .map_err(|e| format!("could not run tar: {e}"))?;
    if !status.success() {
        return Err(format!(
            "tar -xJf {}: {status} (unpacking needs xz)",
            archive.display()
        ));
    }
    let rustc = unpacked.join(component.name()).join("rustc");
    if !rustc.join("bin").join("rustc").is_file() {
        return Err(format!(
            "{}: the archive has no rustc/bin/rustc",
            archive.display()
        ));
    }
    std::fs::rename(&rustc, work.join("rustc"))
        .map_err(|e| format!("{} to {}: {e}", rustc.display(), work.display()))
}

/// Copies into `work/glibc` every library `rustc`'s programs and libraries need that the
/// toolchain does not hold, and every library those need in turn, from the first of `folders`
/// that has it (symbolic links followed). The dynamic loader is left out: it goes where
/// `PT_INTERP` names it. Returns the names copied; an error lists those found nowhere.
fn copy_libraries(work: &Path, folders: &[&str]) -> Result<Vec<String>, String> {
    let toolchain_libraries = work.join("rustc").join("lib");
    let into = work.join("glibc");
    std::fs::create_dir_all(&into).map_err(|e| format!("{}: {e}", into.display()))?;
    let mut waiting = vec![work.join("rustc").join("bin").join("rustc")];
    for entry in std::fs::read_dir(&toolchain_libraries)
        .map_err(|e| format!("{}: {e}", toolchain_libraries.display()))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if path.is_file() && name.is_some_and(|name| name.contains(".so")) {
            waiting.push(path);
        }
    }
    let mut copied = Vec::new();
    let mut seen = BTreeSet::new();
    let mut missing = BTreeSet::new();
    while let Some(file) = waiting.pop() {
        let image = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let needed = needed_libraries(&image).map_err(|e| format!("{}: {e}", file.display()))?;
        for name in needed {
            if name.starts_with("ld-linux") || !seen.insert(name.clone()) {
                continue;
            }
            if toolchain_libraries.join(&name).is_file() {
                continue;
            }
            let Some(found) = folders
                .iter()
                .map(|folder| Path::new(folder).join(&name))
                .find(|path| path.is_file())
            else {
                missing.insert(name);
                continue;
            };
            let to = into.join(&name);
            std::fs::copy(&found, &to)
                .map_err(|e| format!("{} to {}: {e}", found.display(), to.display()))?;
            copied.push(name);
            waiting.push(to);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "rustc needs {} and the host has none of them in {} (on Ubuntu: the target's \
             libc6, libgcc-s1 and zlib1g packages)",
            missing.into_iter().collect::<Vec<_>>().join(", "),
            folders.join(", ")
        ));
    }
    copied.sort();
    Ok(copied)
}

/// The libraries a little-endian ELF-64 file needs (its `DT_NEEDED` entries), in order: none
/// for a file without a dynamic section.
fn needed_libraries(image: &[u8]) -> Result<Vec<String>, String> {
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const DT_NULL: u64 = 0;
    const DT_NEEDED: u64 = 1;
    const DT_STRTAB: u64 = 5;
    let field = |at: usize, size: usize| -> Result<u64, String> {
        let bytes = at
            .checked_add(size)
            .and_then(|end| image.get(at..end))
            .ok_or("the ELF image ends early")?;
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
    let mut loads = Vec::new();
    let mut dynamic = None;
    for index in 0..count {
        let header = table + index * entry_size;
        let offset = field(header + 8, 8)?;
        let address = field(header + 16, 8)?;
        let file_size = field(header + 32, 8)?;
        match field(header, 4)? as u32 {
            PT_LOAD => loads.push((address, offset, file_size)),
            PT_DYNAMIC => dynamic = Some((offset, file_size)),
            _ => {}
        }
    }
    let Some((offset, size)) = dynamic else {
        return Ok(Vec::new());
    };
    let mut strings = None;
    let mut needed = Vec::new();
    for entry in 0..size / 16 {
        let at = (offset + entry * 16) as usize;
        let value = field(at + 8, 8)?;
        match field(at, 8)? {
            DT_NULL => break,
            DT_NEEDED => needed.push(value),
            DT_STRTAB => strings = Some(value),
            _ => {}
        }
    }
    if needed.is_empty() {
        return Ok(Vec::new());
    }
    let strings = strings.ok_or("DT_NEEDED without a string table")?;
    let strings = loads
        .iter()
        .find(|&&(address, _, file_size)| strings >= address && strings < address + file_size)
        .map(|&(address, offset, _)| strings - address + offset)
        .ok_or("the string table is in no segment")? as usize;
    needed
        .into_iter()
        .map(|name| {
            let start = strings + name as usize;
            let rest = image.get(start..).ok_or("a name past the image's end")?;
            let end = rest
                .iter()
                .position(|&byte| byte == 0)
                .ok_or("a name without its NUL")?;
            String::from_utf8(rest[..end].to_vec()).map_err(|_| "a name that is not UTF-8".into())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_needing(names: &[&str]) -> Vec<u8> {
        let mut image = vec![0u8; 0x400];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = 2;
        image[5] = 1;
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&2u16.to_le_bytes());
        let load = 64;
        image[load..load + 4].copy_from_slice(&1u32.to_le_bytes());
        image[load + 16..load + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        image[load + 32..load + 40].copy_from_slice(&0x400u64.to_le_bytes());
        let dynamic = 64 + 56;
        image[dynamic..dynamic + 4].copy_from_slice(&2u32.to_le_bytes());
        image[dynamic + 8..dynamic + 16].copy_from_slice(&0x200u64.to_le_bytes());
        image[dynamic + 32..dynamic + 40].copy_from_slice(&0x100u64.to_le_bytes());
        let strings = 0x300;
        let mut entry = 0x200;
        let mut put = |image: &mut Vec<u8>, tag: u64, value: u64| {
            image[entry..entry + 8].copy_from_slice(&tag.to_le_bytes());
            image[entry + 8..entry + 16].copy_from_slice(&value.to_le_bytes());
            entry += 16;
        };
        put(&mut image, 5, 0x1000 + strings as u64);
        let mut at = 1;
        for name in names {
            put(&mut image, 1, at as u64);
            image[strings + at..strings + at + name.len()].copy_from_slice(name.as_bytes());
            at += name.len() + 1;
        }
        image
    }

    #[test]
    fn needed_libraries_are_read_from_the_dynamic_section() {
        let image = library_needing(&["libc.so.6", "libgcc_s.so.1"]);
        assert_eq!(
            needed_libraries(&image),
            Ok(vec!["libc.so.6".to_owned(), "libgcc_s.so.1".to_owned()])
        );
        assert_eq!(needed_libraries(&library_needing(&[])), Ok(Vec::new()));
        let mut without_dynamic = image.clone();
        without_dynamic[64 + 56..64 + 60].copy_from_slice(&6u32.to_le_bytes());
        assert_eq!(needed_libraries(&without_dynamic), Ok(Vec::new()));
        assert!(needed_libraries(b"\x7fELF").is_err());
        assert!(needed_libraries(b"MZ").is_err());
    }

    #[test]
    fn each_architecture_with_a_toolchain_has_its_pinned_download() {
        for arch in ["x86_64", "aarch64"] {
            let component = component(arch).unwrap();
            assert!(component.target.starts_with(arch));
            assert_eq!(component.sha256.len(), 64);
            assert!(
                component
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            );
            assert!(
                component
                    .url()
                    .starts_with("https://static.rust-lang.org/dist/")
            );
            assert!(component.url().ends_with(&format!(
                "/rustc-{RUST_VERSION}-{arch}-unknown-linux-gnu.tar.xz"
            )));
            assert!(!host_library_folders(arch).is_empty());
            assert!(glibc(arch).is_some());
        }
        assert_eq!(component("riscv64"), None);
        assert!(VERSION_LINE_PREFIX.starts_with(&format!("rustc {RUST_VERSION} ")));
        let toolchain =
            std::fs::read_to_string(crate::workspace_root().join("rust-toolchain.toml")).unwrap();
        assert!(toolchain.contains(&format!("channel = \"{RUST_VERSION}\"")));
    }

    #[test]
    fn the_sha256_of_a_file_is_written_in_lowercase_hexadecimal() {
        let folder = std::env::temp_dir().join(format!("eggos-sha256-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("abc");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_of(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}

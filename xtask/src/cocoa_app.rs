//! `cargo xtask cocoa-app` (D175, the Real apps track's first macOS app): a real Objective-C
//! Mac app, CocoaGame 1.0.0, installed from its `.zip` and played inside EggOS through the
//! macOS layer's own AppKit, on aarch64 and x86_64 (its binary is universal).
//!
//! It is test data, downloaded and never shipped or committed (decided by research): the
//! author's release archive, checked against its pinned SHA-256 (a release asset can be
//! replaced, so a new one is pinned again by hand after checking it). The host also reads the
//! archive's icon with the installer's own code, so the About panel's frame can be compared
//! with the icon the installer made ([`crate::qemu::cocoa_app`]).

use crate::dynamic_linking::download;
use crate::rustc_version::sha256_of;
use crate::{Arch, build_kernel};
use eggos_app::archive::{Archive, extract};
use eggos_app::icon::{self, OwnIcon, Workspace};
use eggos_app::reader::Bytes;
use eggos_png::inflate::{Sink, Window};
use std::path::{Path, PathBuf};

/// The app's name once installed: its bundle's name.
pub const APP: &str = "CocoaGame";
/// The archive the author publishes.
pub const ARCHIVE: &str = "CocoaGame.zip";
/// The app's version, its `CFBundleShortVersionString`.
pub const VERSION: &str = "1.0.0";
/// The app's build, its `CFBundleVersion`.
pub const BUILD: &str = "1";
/// Where the author publishes the archive.
pub const ARCHIVE_URL: &str =
    "https://github.com/reallukee/cocoagame/releases/download/v1.0.0/CocoaGame.zip";
/// The pinned archive's SHA-256.
pub const ARCHIVE_SHA256: &str = "ac9a2d976533c19e19042af3da9f6f0e25a9922777c426a926cf13790410b4cd";
/// The icon inside the archive, which its `Info.plist` names.
pub const ICON_ENTRY: &str = "CocoaGame.app/Contents/Resources/CocoaGame.icns";
/// The architectures the app's universal binary has code for, which EggOS runs.
pub const ARCHITECTURES: [&str; 2] = ["aarch64", "x86_64"];

fn root() -> PathBuf {
    crate::target_directory().join("cocoa-app")
}

fn fetch_pinned(url: &str, file: &Path, sha256: &str) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != sha256 {
        download(url, file)?;
    }
    let found = sha256_of(file)?;
    match found == sha256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {sha256}: a release asset can be replaced, so \
             pin the new one in xtask/src/cocoa_app.rs after checking it",
            file.display()
        )),
    }
}

struct Collected(Vec<u8>);

impl Sink for Collected {
    fn write(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

/// The bytes of the archive entry `name`, extracted with the installer's own reader.
pub fn entry_bytes(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut reader = Bytes(archive);
    let opened = Archive::open(&mut reader).map_err(|e| format!("{ARCHIVE}: {e:?}"))?;
    let mut cursor = opened.first();
    while let Some(entry) = opened
        .next(&mut reader, &mut cursor)
        .map_err(|e| format!("{ARCHIVE}: {e:?}"))?
    {
        if entry.name() != name.as_bytes() {
            continue;
        }
        let mut window = Box::new(Window::new());
        let mut collected = Collected(Vec::new());
        extract(&mut reader, &entry, &mut window, &mut collected)
            .map_err(|e| format!("{ARCHIVE}: {name}: {e:?}"))?;
        return Ok(collected.0);
    }
    Err(format!("{ARCHIVE} has no {name}"))
}

/// The installed app's own icon file (`app.icon`) the installer makes of the archive's
/// `.icns`, or `None` when its code makes none (the panel then shows the generic tile).
pub fn expected_icon_file(archive: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let icns = entry_bytes(archive, ICON_ENTRY)?;
    let mut work = Box::new(Workspace::new());
    let mut made = OwnIcon::new();
    Ok(icon::from_icns(&mut Bytes(&icns), &mut work, &mut made).then(|| made.file().to_vec()))
}

fn host_folder(arch: &Arch) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    fetch_pinned(ARCHIVE_URL, &folder.join(ARCHIVE), ARCHIVE_SHA256)?;
    Ok(folder)
}

/// Whether EggOS runs the app on `arch`.
pub fn runs_on(arch: &str) -> bool {
    ARCHITECTURES.contains(&arch)
}

/// Downloads `CocoaGame.zip` into a host folder and runs the boot test
/// ([`crate::qemu::cocoa_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: macOS apps run on aarch64 and x86_64 only, as the macOS layer does",
            arch.name
        ));
    }
    let folder = host_folder(arch)?;
    let archive = folder.join(ARCHIVE);
    let bytes = std::fs::read(&archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let icon = expected_icon_file(&bytes)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::cocoa_app::check(arch, &kernel, &folder, icon.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_is_pinned_and_runs_on_both_layer_architectures() {
        assert_eq!(ARCHIVE_SHA256.len(), 64);
        assert!(ARCHIVE_SHA256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(ARCHIVE_URL.ends_with(ARCHIVE));
        assert!(ARCHIVE_URL.contains(VERSION));
        assert!(ICON_ENTRY.starts_with(APP));
        assert!(runs_on("aarch64"));
        assert!(runs_on("x86_64"));
        assert!(!runs_on("riscv64"));
    }

    #[test]
    fn an_entry_is_read_with_the_installer_s_own_archive_code() {
        let member = |name: &str, data: &[u8]| crate::zip::Member {
            name: name.to_owned(),
            data: data.to_vec(),
            mode: 0o644,
        };
        let archive = crate::zip::stored(&[
            member("CocoaGame.app/Contents/Info.plist", b"<plist/>"),
            member(ICON_ENTRY, b"icns"),
        ])
        .unwrap();
        assert_eq!(entry_bytes(&archive, ICON_ENTRY).unwrap(), b"icns");
        assert!(entry_bytes(&archive, "missing").is_err());
    }
}

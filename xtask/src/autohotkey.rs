//! `cargo xtask autohotkey` (D178, the Real apps track's second Windows app): AutoHotkey v2, a
//! real Win32 program built on comctl32's common controls, GDI+ and COM, installed from its
//! release `.zip` and run inside EggOS through the Windows layer with a script of ours,
//! `tests/autohotkey/controls.ahk`, that builds the window of `docs/ux/windows-controls-v0.3.md`
//! section 30: a toolbar, a tab control, a list view, a tree view, a progress bar, a picture
//! loaded through GDI+ and a status bar, its rows from a `Scripting.Dictionary`.
//!
//! The release archive is test data (decided by research): downloaded from AutoHotkey's GitHub
//! release, checked against its pinned SHA-256, never shipped or committed. AutoHotkey publishes
//! x64 and 32-bit x86 builds and no ARM64 one, so the check runs on x86_64 alone
//! ([`crate::qemu::autohotkey`]).

use crate::dynamic_linking::download;
use crate::qmp::Picture;
use crate::rustc_version::sha256_of;
use crate::{Arch, build_kernel};
use std::path::{Path, PathBuf};

/// The app's name once installed.
pub const APP: &str = "autohotkey";
/// The release AutoHotkey's archive comes from.
pub const VERSION: &str = "2.0.31";
/// The release archive's file name.
pub const ARCHIVE: &str = "AutoHotkey_2.0.31.zip";
/// The program inside the archive the app opens.
pub const PROGRAM: &str = "AutoHotkey64.exe";
/// Where AutoHotkey publishes the release archive.
pub const ARCHIVE_URL: &str =
    "https://github.com/AutoHotkey/AutoHotkey/releases/download/v2.0.31/AutoHotkey_2.0.31.zip";
/// The pinned archive's SHA-256.
pub const ARCHIVE_SHA256: &str = "ca1a595aca4528755d8d4cd7fed23d497a37bd9a0b6892072612d68dee1bd00a";
/// The script the app runs.
pub const SCRIPT: &str = "controls.ahk";
/// The picture the script shows through GDI+.
pub const PICTURE: &str = "egg.png";
/// The architectures AutoHotkey ships a build for that EggOS runs.
pub const ARCHITECTURES: [&str; 1] = ["x86_64"];

const SCRIPT_TEXT: &str = include_str!("../../tests/autohotkey/controls.ahk");
const PICTURE_SIDE: u32 = 32;
const PICTURE_RING: u32 = 2;
const PICTURE_SQUARE: (u32, u32) = (12, 8);
/// The picture's ring colour.
pub const PICTURE_RING_COLOUR: u32 = 0x0000_78d7;
/// The picture's fill colour.
pub const PICTURE_FILL_COLOUR: u32 = 0x00f2_b33d;
/// The colour of the square in the picture's middle.
pub const PICTURE_SQUARE_COLOUR: u32 = 0x0000_0000;

fn root() -> PathBuf {
    crate::target_directory().join("autohotkey")
}

fn fetch_pinned(file: &Path) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != ARCHIVE_SHA256 {
        download(ARCHIVE_URL, file)?;
    }
    let found = sha256_of(file)?;
    match found == ARCHIVE_SHA256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {ARCHIVE_SHA256}",
            file.display()
        )),
    }
}

/// The 32 by 32 picture the script loads: a 2 pixel ring, a fill and an 8 by 8 square at
/// (12, 12), as section 30 of the design draws it.
pub fn picture() -> Picture {
    let side = PICTURE_SIDE;
    let (square_at, square_side) = PICTURE_SQUARE;
    let pixels = (0..side * side)
        .map(|at| {
            let (x, y) = (at % side, at / side);
            let ring = x < PICTURE_RING
                || y < PICTURE_RING
                || x >= side - PICTURE_RING
                || y >= side - PICTURE_RING;
            let square = (square_at..square_at + square_side).contains(&x)
                && (square_at..square_at + square_side).contains(&y);
            match (ring, square) {
                (true, _) => PICTURE_RING_COLOUR,
                (false, true) => PICTURE_SQUARE_COLOUR,
                (false, false) => PICTURE_FILL_COLOUR,
            }
        })
        .collect();
    Picture {
        width: side,
        height: side,
        pixels,
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn host_folder(arch: &Arch) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    fetch_pinned(&folder.join(ARCHIVE))?;
    write(&folder.join(SCRIPT), SCRIPT_TEXT.as_bytes())?;
    write(
        &folder.join(PICTURE),
        &crate::release::encode_png(&picture())?,
    )?;
    Ok(folder)
}

/// Whether AutoHotkey ships a build for `arch` that EggOS runs.
pub fn runs_on(arch: &str) -> bool {
    ARCHITECTURES.contains(&arch)
}

/// Downloads the release archive into a host folder with the script and its picture, and runs
/// the boot test ([`crate::qemu::autohotkey::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: AutoHotkey publishes Windows builds for x64 and x86 only, so the AutoHotkey \
             check runs on x86_64",
            arch.name
        ));
    }
    let folder = host_folder(arch)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::autohotkey::check(arch, &kernel, &folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_is_pinned_and_runs_on_x86_64_only() {
        assert_eq!(ARCHIVE_SHA256.len(), 64);
        assert!(ARCHIVE_SHA256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(ARCHIVE_URL.ends_with(ARCHIVE));
        assert!(ARCHIVE_URL.contains(VERSION));
        assert!(runs_on("x86_64"));
        assert!(!runs_on("aarch64"));
    }

    #[test]
    fn the_script_builds_the_designed_window() {
        for wanted in [
            "Gui(\"-DPIScale\", \"EggOS controls\")",
            "ComObject(\"Scripting.Dictionary\")",
            "ClassToolbarWindow32 x0 y0 w480 h30 0x184C",
            "AddListView(\"x12 y64 w456 h208 -Multi\"",
            "AddTreeView(\"x12 y64 w456 h208\")",
            "AddProgress(\"x8 y282 w424 h20 Range0-100\", 40)",
            "AddPicture(\"x440 y276\", \"egg.png\")",
            "Show(\"w480 h360\")",
        ] {
            assert!(SCRIPT_TEXT.contains(wanted), "{wanted}");
        }
    }

    #[test]
    fn the_picture_has_its_ring_fill_and_square() {
        let picture = picture();
        let at = |x: u32, y: u32| picture.pixels[(y * picture.width + x) as usize];
        assert_eq!(at(0, 0), PICTURE_RING_COLOUR);
        assert_eq!(at(1, 30), PICTURE_RING_COLOUR);
        assert_eq!(at(2, 2), PICTURE_FILL_COLOUR);
        assert_eq!(at(12, 12), PICTURE_SQUARE_COLOUR);
        assert_eq!(at(19, 19), PICTURE_SQUARE_COLOUR);
        assert_eq!(at(20, 19), PICTURE_FILL_COLOUR);
        assert_eq!(picture.pixels.len(), 32 * 32);
    }
}

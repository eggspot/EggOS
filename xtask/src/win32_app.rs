//! `cargo xtask win32-app` (D174, the Real apps track's first Windows app): a real classic
//! Win32 application, Pegs from Simon Tatham's Portable Puzzle Collection, installed and played
//! inside EggOS through the Windows layer.
//!
//! It is test data, downloaded and never shipped or committed (decided by research): the
//! collection's official x64 `pegs.exe`, checked against its pinned SHA-256. The collection
//! publishes each puzzle at one address that every new build replaces (the versioned installer
//! keeps the same file), so a new build is pinned again by hand after checking it. Its Windows
//! downloads are x64 only, so the check runs on x86_64 alone ([`crate::qemu::win32_app`]).

use crate::dynamic_linking::download;
use crate::rustc_version::sha256_of;
use crate::{Arch, build_kernel};
use std::path::{Path, PathBuf};

/// The app's name once installed.
pub const APP: &str = "pegs";
/// The program the collection publishes.
pub const PROGRAM: &str = "pegs.exe";
/// The collection's build the pinned program comes from.
pub const PUZZLES_VERSION: &str = "20260923.616da16";
/// Where the collection publishes the program.
pub const PEGS_URL: &str = "https://www.chiark.greenend.org.uk/~sgtatham/puzzles/pegs.exe";
/// The pinned program's SHA-256.
pub const PEGS_SHA256: &str = "d093bd486964bd19ccf6147800aa2ee0cfe13bfd19ea145df2ccc62cf904ecc7";
/// The architectures the collection ships a Windows build for.
pub const ARCHITECTURES: [&str; 1] = ["x86_64"];

fn root() -> PathBuf {
    crate::target_directory().join("win32-app")
}

fn fetch_pinned(url: &str, file: &Path, sha256: &str) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != sha256 {
        download(url, file)?;
    }
    let found = sha256_of(file)?;
    match found == sha256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {sha256}: the collection publishes each \
             puzzle at one address that every new build replaces, so pin the new one in \
             xtask/src/win32_app.rs after checking it",
            file.display()
        )),
    }
}

fn host_folder(arch: &Arch) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    fetch_pinned(PEGS_URL, &folder.join(PROGRAM), PEGS_SHA256)?;
    Ok(folder)
}

/// Whether the app ships a build for `arch`.
pub fn runs_on(arch: &str) -> bool {
    ARCHITECTURES.contains(&arch)
}

/// Downloads `pegs.exe` into a host folder and runs the boot test
/// ([`crate::qemu::win32_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: Simon Tatham's Puzzles publishes Windows builds for x64 only, so the Win32 app \
             check runs on x86_64",
            arch.name
        ));
    }
    let folder = host_folder(arch)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::win32_app::check(arch, &kernel, &folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_pinned_and_runs_on_x86_64_only() {
        assert_eq!(PEGS_SHA256.len(), 64);
        assert!(PEGS_SHA256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(PEGS_URL.ends_with(PROGRAM));
        assert!(runs_on("x86_64"));
        assert!(!runs_on("aarch64"));
        assert!(!runs_on("riscv64"));
    }
}

//! `cargo xtask udcalc-app` (D410, the Real apps track's second macOS app): UDCalc 0.2.0
//! (BSD-2-Clause), a calculator that carries its Sparkle framework in its bundle and builds
//! its window from nibs, installed from its release `.zip` and used inside EggOS through the
//! macOS layer, on aarch64 and x86_64 (its binary is universal).
//!
//! The archive is test data, downloaded and never shipped or committed: the author's GitHub
//! release asset, checked against the SHA-256 GitHub publishes for it (a release asset can be
//! replaced, so a new one is pinned again by hand after checking it).

use crate::dynamic_linking::download;
use crate::rustc_version::sha256_of;
use crate::{Arch, build_kernel_with_layers};
use std::path::{Path, PathBuf};

/// The app's name once installed: its bundle's name.
pub const APP: &str = "UDCalc";
/// The archive the author publishes.
pub const ARCHIVE: &str = "UDCalc-macOS-0.2.0.zip";
/// The app's version.
pub const VERSION: &str = "0.2.0";
/// Where the author publishes the archive.
pub const ARCHIVE_URL: &str =
    "https://github.com/ashalkhakov/UDCalc/releases/download/v0.2.0/UDCalc-macOS-0.2.0.zip";
/// The pinned archive's SHA-256, as GitHub publishes it for the release asset.
pub const ARCHIVE_SHA256: &str = "022b842501055c64b758e77923919804d930d8ae40f7689dc25fabcb35f9b500";
/// The architectures the app's universal binary has code for, which EggOS runs.
pub const ARCHITECTURES: [&str; 2] = ["aarch64", "x86_64"];

fn root() -> PathBuf {
    crate::target_directory().join("udcalc-app")
}

fn fetch_pinned(file: &Path) -> Result<(), String> {
    if !file.is_file() || sha256_of(file)? != ARCHIVE_SHA256 {
        download(ARCHIVE_URL, file)?;
    }
    let found = sha256_of(file)?;
    match found == ARCHIVE_SHA256 {
        true => Ok(()),
        false => Err(format!(
            "{}: SHA-256 {found}, not the pinned {ARCHIVE_SHA256}: a release asset can be \
             replaced, so pin the new one in xtask/src/udcalc_app.rs after checking it",
            file.display()
        )),
    }
}

/// Whether EggOS runs the app on `arch`.
pub fn runs_on(arch: &str) -> bool {
    ARCHITECTURES.contains(&arch)
}

/// Downloads the archive into a host folder and runs the boot test
/// ([`crate::qemu::udcalc_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: macOS apps run on aarch64 and x86_64 only, as the macOS layer does",
            arch.name
        ));
    }
    let folder = root().join(arch.name).join("host");
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    fetch_pinned(&folder.join(ARCHIVE))?;
    let kernel = build_kernel_with_layers(arch)?;
    crate::qemu::udcalc_app::check(arch, &kernel, &folder)
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
        assert!(runs_on("aarch64"));
        assert!(runs_on("x86_64"));
        assert!(!runs_on("riscv64"));
    }
}

//! `cargo xtask swift-app` (D389, the Real apps track's first Swift app): Minesweeper 1.0.0 by
//! codemaki, a Swift app on AppKit built in code, installed from its `.zip` and played inside
//! EggOS through the macOS layer's AppKit and Swift runtime, on aarch64 and x86_64 (its binary
//! is universal). Like CocoaGame (D175) it is test data, downloaded and checked against its
//! pinned SHA-256, never shipped or committed.

use crate::dynamic_linking::download;
use crate::rustc_version::sha256_of;
use crate::{Arch, build_kernel_with_layers};
use std::path::{Path, PathBuf};

/// The app's name once installed: its bundle's name.
pub const APP: &str = "Minesweeper";
/// The archive's name in the host folder.
pub const ARCHIVE: &str = "Minesweeper-1.0.0.zip";
/// The release it is.
pub const VERSION: &str = "1.0.0";
/// Where the author publishes it.
pub const ARCHIVE_URL: &str =
    "https://github.com/codemaki/Minesweeper/releases/download/v1.0.0/Minesweeper-1.0.0.zip";
/// Its SHA-256, pinned when it was chosen (docs/compatibility.md).
pub const ARCHIVE_SHA256: &str = "53bbd3cc0f3158adf70f5a843d995438f9f1b2c62aaf0f57fdd09d16fa041858";
/// The architectures its universal binary and the macOS layer share.
pub const ARCHITECTURES: [&str; 2] = ["aarch64", "x86_64"];

fn root() -> PathBuf {
    crate::target_directory().join("swift-app")
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
             replaced, so pin the new one in xtask/src/swift_app.rs after checking it",
            file.display()
        )),
    }
}

/// Whether EggOS runs the app on `arch`.
pub fn runs_on(arch: &str) -> bool {
    ARCHITECTURES.contains(&arch)
}

/// Downloads the archive into a host folder and runs the boot test
/// ([`crate::qemu::swift_app::check`]) on the release kernel with the layers.
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
    crate::qemu::swift_app::check(arch, &kernel, &folder)
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

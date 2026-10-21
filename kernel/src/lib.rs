//! The architecture-independent EggOS kernel core.
//!
//! Nothing in this crate may depend on a particular CPU architecture or device: the machine
//! is reached only through [`eggos_hal::Platform`], so the core builds and is unit-tested on
//! the development host as well as on every bare-metal target.

#![cfg_attr(not(test), no_std)]

pub mod console;

use core::fmt::Write;
use eggos_hal::ExitStatus;

/// Kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs the architecture-independent part of boot, writing progress to `console`.
///
/// Returns how the boot went; the architecture entry point turns that into a machine exit.
pub fn boot<W: Write>(console: &mut W, arch: &str) -> ExitStatus {
    if writeln!(console, "EggOS {VERSION} ({arch})").is_err() {
        return ExitStatus::Failure;
    }
    ExitStatus::Success
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_prints_banner_with_version_and_arch() {
        let mut out = String::new();
        assert_eq!(boot(&mut out, "testarch"), ExitStatus::Success);
        assert_eq!(out, format!("EggOS {VERSION} (testarch)\n"));
    }
}

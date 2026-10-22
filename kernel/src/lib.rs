//! The architecture-independent EggOS kernel core.
//!
//! Nothing in this crate may depend on a particular CPU architecture or device: the machine
//! is reached only through [`eggos_hal::Platform`], so the core builds and is unit-tested on
//! the development host as well as on every bare-metal target.

#![cfg_attr(not(test), no_std)]

pub mod console;
pub mod exception;
pub mod selftest;

use core::fmt::Write;
use eggos_hal::{ExitStatus, Platform};

/// Kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs the architecture-independent part of boot on platform `P`, writing progress to
/// `console`.
///
/// Returns how the boot went; the architecture entry point turns that into a machine exit.
pub fn boot<P: Platform, W: Write>(console: &mut W) -> ExitStatus {
    let report = writeln!(console, "EggOS {VERSION} ({})", P::ARCH)
        .and_then(|()| selftest::run(console, &selftest::all::<P>()));
    match report {
        Ok(true) => ExitStatus::Success,
        Ok(false) | Err(_) => ExitStatus::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use eggos_hal::{Exception, ExceptionKind};

    /// A platform whose breakpoint goes straight to the kernel's handler, as the
    /// architecture's exception vectors do on a real machine.
    struct TestPlatform;

    impl Platform for TestPlatform {
        const ARCH: &'static str = "testarch";
        fn console_write(_bytes: &[u8]) {}
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {
            let breakpoint = Exception {
                kind: ExceptionKind::Breakpoint,
                name: "test breakpoint",
                code: 0,
                pc: 0,
                address: None,
            };
            assert_eq!(exception::handle(&breakpoint), exception::Outcome::Resume);
        }
        fn illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
    }

    #[test]
    fn boot_prints_banner_with_version_and_arch() {
        let mut out = String::new();
        assert_eq!(boot::<TestPlatform, _>(&mut out), ExitStatus::Success);
        assert!(out.starts_with(&format!("EggOS {VERSION} (testarch)\n")));
        assert!(out.ends_with(&format!("{}\n", selftest::PASS_MARKER)));
    }
}

//! The architecture-independent EggOS kernel core.
//!
//! Nothing here may depend on a CPU architecture: the core reaches the
//! hardware only through [`eggos_hal::Platform`], so it builds and is unit
//! tested on development hosts as well as on every kernel target.

#![cfg_attr(not(test), no_std)]

use core::fmt::{self, Write};
use core::panic::PanicInfo;

use eggos_hal::{CharDevice, Driver, Platform};

/// The kernel's version, shared by the whole workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Logged once boot has completed. Automated boot tests wait for this line.
pub const BOOT_OK_MARKER: &str = "EggOS: boot OK";

/// Starts the log line of a kernel panic. Automated boot tests fail on it.
pub const PANIC_MARKER: &str = "EggOS: KERNEL PANIC";

/// Boot the kernel on `platform`. Returns once there is nothing left to do;
/// the caller then halts the CPU.
pub fn boot<P: Platform>(platform: &P) {
    let console = platform.console();
    let mut log = ConsoleLog(console);
    // `ConsoleLog` never fails, so neither can these writes.
    let _ = writeln!(log, "EggOS {VERSION} ({})", P::ARCH);
    let _ = writeln!(log, "console: {}", console.name());
    let _ = writeln!(log, "{BOOT_OK_MARKER}");
}

/// Log a kernel panic to `console`, on one line so log scanners see the
/// message next to the marker.
pub fn report_panic(console: &impl CharDevice, info: &PanicInfo<'_>) {
    let mut log = ConsoleLog(console);
    let _ = match info.location() {
        Some(location) => writeln!(log, "{PANIC_MARKER} at {location}: {}", info.message()),
        None => writeln!(log, "{PANIC_MARKER}: {}", info.message()),
    };
}

/// Formatted text output to a [`CharDevice`].
struct ConsoleLog<'a, C>(&'a C);

impl<C: CharDevice> Write for ConsoleLog<'_, C> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.write_bytes(text.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_hal::DriverError;
    use std::cell::RefCell;

    struct FakeConsole(RefCell<Vec<u8>>);

    impl Driver for FakeConsole {
        fn name(&self) -> &'static str {
            "fake-console"
        }

        fn init(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    impl CharDevice for FakeConsole {
        fn write_bytes(&self, bytes: &[u8]) {
            self.0.borrow_mut().extend_from_slice(bytes);
        }
    }

    struct FakePlatform(FakeConsole);

    impl Platform for FakePlatform {
        const ARCH: &'static str = "test-arch";
        type Console = FakeConsole;

        fn console(&self) -> &FakeConsole {
            &self.0
        }

        fn halt(&self) -> ! {
            unreachable!("boot must return to its caller to halt")
        }
    }

    #[test]
    fn boot_logs_banner_console_and_boot_marker() {
        let platform = FakePlatform(FakeConsole(RefCell::new(Vec::new())));
        boot(&platform);
        let log = String::from_utf8(platform.0.0.into_inner()).unwrap();
        assert_eq!(
            log,
            format!("EggOS {VERSION} (test-arch)\nconsole: fake-console\n{BOOT_OK_MARKER}\n")
        );
    }
}

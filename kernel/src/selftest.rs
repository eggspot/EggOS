//! Boot-time self-tests: checks that only mean something on the real (or emulated) machine,
//! such as "did the architecture entry point zero `.bss`". The QEMU test runner reads the
//! verdict markers from the console.

use core::fmt::Write;
use core::sync::atomic::AtomicU64;

/// Printed when every self-test passed. The QEMU runner requires this exact line.
pub const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
/// Printed when a self-test failed or the kernel panicked.
pub const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";

/// A self-test: a name and a check returning `Err(reason)` on failure.
pub type SelfTest = (&'static str, fn() -> Result<(), &'static str>);

/// Every boot self-test, in run order.
pub const ALL: &[SelfTest] = &[("bss_is_zeroed", bss_is_zeroed)];

/// Runs `tests`, reporting each and then the verdict marker. Returns whether all passed.
pub fn run<W: Write>(console: &mut W, tests: &[SelfTest]) -> Result<bool, core::fmt::Error> {
    let mut all_passed = true;
    for (name, test) in tests {
        match test() {
            Ok(()) => writeln!(console, "selftest {name} ... ok")?,
            Err(reason) => {
                all_passed = false;
                writeln!(console, "selftest {name} ... FAILED: {reason}")?;
            }
        }
    }
    let marker = if all_passed { PASS_MARKER } else { FAIL_MARKER };
    writeln!(console, "{marker}")?;
    Ok(all_passed)
}

/// Lives in `.bss` (zero-initialised and interior-mutable, so it cannot be placed in
/// read-only data). Reading anything but zero means the entry point did not clear `.bss`.
/// QEMU hands over zeroed RAM, so this guards real hardware and warm reboots, where RAM
/// holds whatever was there before.
static BSS_PROBE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bss_is_zeroed() -> Result<(), &'static str> {
    let all_zero = BSS_PROBE.iter().all(|word| {
        // SAFETY: `word.as_ptr()` points to a live, aligned `u64` (an `AtomicU64` has the
        // layout of `u64`) that nothing writes concurrently. The read must be volatile:
        // since nothing ever stores to the probe, the compiler would otherwise fold an
        // ordinary load to the initial value and the check would test nothing.
        unsafe { core::ptr::read_volatile(word.as_ptr()) == 0 }
    });
    if all_zero {
        Ok(())
    } else {
        Err(".bss was not zeroed by the architecture entry point")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes() -> Result<(), &'static str> {
        Ok(())
    }
    fn fails() -> Result<(), &'static str> {
        Err("broken")
    }

    #[test]
    fn all_passing_prints_each_result_and_pass_marker() {
        let mut out = String::new();
        assert_eq!(run(&mut out, &[("a", passes), ("b", passes)]), Ok(true));
        assert_eq!(
            out,
            format!("selftest a ... ok\nselftest b ... ok\n{PASS_MARKER}\n")
        );
    }

    #[test]
    fn one_failure_runs_the_rest_and_prints_fail_marker() {
        let mut out = String::new();
        assert_eq!(run(&mut out, &[("a", fails), ("b", passes)]), Ok(false));
        assert_eq!(
            out,
            format!("selftest a ... FAILED: broken\nselftest b ... ok\n{FAIL_MARKER}\n")
        );
    }

    #[test]
    fn bss_probe_is_zero_on_host() {
        assert_eq!(bss_is_zeroed(), Ok(()));
    }
}

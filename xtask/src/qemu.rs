//! Booting kernel images in QEMU.

use crate::{Arch, elf};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Must match `eggos_kernel::selftest::{PASS_MARKER, FAIL_MARKER}`.
const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";

/// Must match the start of `eggos_kernel::exception::report`'s line.
const EXCEPTION_PREFIX: &str = "kernel exception: ";

/// What a boot test expects the kernel to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Pass every self-test and power off.
    Pass,
    /// Pass every self-test, then report a fatal exception with the fail marker and power off
    /// (the kernel's `qemu-test-fatal-exception` feature).
    FatalException,
}

/// A boot test that has not powered off by then has hung.
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);

/// Start of the kernel's boot banner (`EggOS <version> (<arch>)`, see `eggos_kernel::boot`).
/// Seeing it twice means the machine reset and booted again.
const BANNER_PREFIX: &str = "EggOS ";

const COMMON_ARGS: &[&str] = &["-m", "128M", "-smp", "1"];

/// Symbol name of the kernel's `.bss` self-test probe (`BSS_PROBE` in
/// `eggos_kernel::selftest`).
const BSS_PROBE_SYMBOL: &str = "eggos_selftest_bss_probe";

/// What the boot tests fill the `.bss` probe with before the kernel starts. The same byte
/// everywhere, so it reads the same in either byte order.
const BSS_POISON: u64 = 0xa5a5_a5a5_a5a5_a5a5;

/// QEMU arguments that fill the kernel's `.bss` probe with [`BSS_POISON`] before the first
/// instruction runs (one `loader` device per 8-byte word). QEMU applies them at reset after
/// placing the kernel image and before any firmware runs: observed on QEMU 10 for all three
/// architectures (a kernel without its zeroing loop fails), not a documented guarantee, so a
/// QEMU upgrade that changes it shows up as `.bss` failing here. QEMU hands over zeroed RAM, so
/// without this the `bss_is_zeroed` self-test would pass even if the entry point never
/// cleared `.bss`.
pub fn bss_poison_args(kernel: &Path) -> Result<Vec<String>, String> {
    let image = std::fs::read(kernel).map_err(|e| format!("{}: {e}", kernel.display()))?;
    let probe = elf::find_symbol(&image, BSS_PROBE_SYMBOL)
        .map_err(|e| format!("{}: {e}", kernel.display()))?;
    if probe.size == 0 || probe.size % 8 != 0 {
        return Err(format!(
            "{}: `{BSS_PROBE_SYMBOL}` is {} bytes, not a whole number of 8-byte words",
            kernel.display(),
            probe.size
        ));
    }
    Ok((0..probe.size / 8)
        .flat_map(|word| {
            let address = probe.physical_address + word * 8;
            [
                "-device".to_string(),
                format!("loader,addr={address:#x},data={BSS_POISON:#x},data-len=8"),
            ]
        })
        .collect())
}

/// Boots `kernel` headless with `extra_args` added to QEMU's and passes only if QEMU powers
/// off cleanly within the timeout and the console shows what `expect` asks for (see
/// [`verdict`]). Returns the console output.
pub fn boot_test(
    arch: &Arch,
    kernel: &Path,
    expect: Expect,
    extra_args: &[&str],
) -> Result<String, String> {
    eprintln!("xtask: booting {} in {} ({expect:?})", arch.name, arch.qemu);
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        // No `-no-reboot` here: with it, an x86 triple fault (e.g. in the power-off path) ends
        // QEMU with exit code 0 and would look like a clean power-off. Without it the machine
        // resets and prints the banner again, which fails the test below.
        .args(COMMON_ARGS)
        .args(extra_args)
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;

    // Stream the console to our stderr while collecting it, so a hang is still visible.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = stdout.read(&mut buf) {
            let _ = std::io::stderr().write_all(&buf[..n]);
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let started = Instant::now();
    let mut console = Vec::new();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        console.extend(rx.try_iter().flatten());
        if banner_count(&String::from_utf8_lossy(&console)) > 1 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: the machine reset and booted again (crash or triple fault?)",
                arch.name
            ));
        }
        if started.elapsed() > BOOT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: no power-off within {}s (kernel hung?)",
                arch.name,
                BOOT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // The reader thread ends when QEMU closes stdout; collect the rest of what it saw.
    console.extend(rx.iter().flatten());
    let console = String::from_utf8_lossy(&console).into_owned();
    verdict(arch.name, expect, status.success(), &console)?;
    Ok(console)
}

/// How many times the kernel's boot banner appears on the console.
fn banner_count(console: &str) -> usize {
    console
        .lines()
        .filter(|l| l.starts_with(BANNER_PREFIX) && l.trim_end().ends_with(')'))
        .count()
}

/// [`Expect::Pass`]: a clean exit, the pass marker and no fail marker.
/// [`Expect::FatalException`]: a clean exit, the pass marker (the self-tests ran), then an
/// exception report followed by the fail marker.
/// Either way, a second boot banner (a reset) fails.
fn verdict(arch: &str, expect: Expect, clean_exit: bool, console: &str) -> Result<(), String> {
    let lines = || console.lines().map(str::trim_end);
    let passed = lines().any(|l| l == PASS_MARKER);
    let failed = lines().any(|l| l == FAIL_MARKER);
    if banner_count(console) > 1 {
        return Err(format!(
            "{arch}: the machine reset and booted again (crash or triple fault?)"
        ));
    }
    if expect == Expect::FatalException {
        let reported = lines()
            .skip_while(|l| !l.starts_with(EXCEPTION_PREFIX))
            .skip(1)
            .any(|l| l == FAIL_MARKER);
        return match (clean_exit, passed, reported) {
            (true, true, true) => {
                eprintln!("xtask: {arch}: fatal exception test passed");
                Ok(())
            }
            (false, _, _) => Err(format!("{arch}: QEMU exited with an error")),
            (true, false, _) => Err(format!(
                "{arch}: the self-tests did not pass before the fatal exception test"
            )),
            (true, true, false) => Err(format!(
                "{arch}: the illegal instruction was not reported as a fatal exception"
            )),
        };
    }
    match (clean_exit, passed, failed) {
        (true, true, false) => {
            eprintln!("xtask: {arch}: boot test passed");
            Ok(())
        }
        (_, _, true) => Err(format!(
            "{arch}: kernel reported a self-test failure or panic"
        )),
        (false, _, _) => Err(format!("{arch}: QEMU exited with an error")),
        (true, false, false) => Err(format!("{arch}: kernel powered off without a verdict")),
    }
}

/// Boots `kernel` with the console on this terminal, until the user quits QEMU.
pub fn run_interactive(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let status = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(COMMON_ARGS)
        .args(["-no-reboot", "-nographic"])
        .arg("-kernel")
        .arg(kernel)
        .status()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("QEMU exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_only_on_clean_exit_with_pass_marker() {
        let ok = format!("EggOS\r\n{PASS_MARKER}\r\n");
        assert!(verdict("x", Expect::Pass, true, &ok).is_ok());
        assert!(verdict("x", Expect::Pass, false, &ok).is_err());
        assert!(verdict("x", Expect::Pass, true, "EggOS\r\n").is_err());
    }

    #[test]
    fn fail_marker_wins_over_pass_marker() {
        let mixed = format!("{PASS_MARKER}\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::Pass, true, &mixed).is_err());
    }

    #[test]
    fn a_second_boot_banner_fails_even_after_pass() {
        let rebooted = format!("EggOS 0.1.0 (x86_64)\n{PASS_MARKER}\nEggOS 0.1.0 (x86_64)\n");
        assert!(verdict("x", Expect::Pass, true, &rebooted).is_err());
    }

    #[test]
    fn fatal_exception_needs_the_report_then_the_fail_marker() {
        let reported = format!(
            "{PASS_MARKER}\n\n{EXCEPTION_PREFIX}#UD invalid opcode (code 0x0) at pc 0x1000\n{FAIL_MARKER}\n"
        );
        assert!(verdict("x", Expect::FatalException, true, &reported).is_ok());
        assert!(verdict("x", Expect::FatalException, false, &reported).is_err());
        // A plain pass, a panic or a fail marker before the report is not the exception path.
        let pass = format!("{PASS_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &pass).is_err());
        let panic = format!("{PASS_MARKER}\nkernel panic: x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &panic).is_err());
        let early = format!("{FAIL_MARKER}\n{EXCEPTION_PREFIX}x\n");
        assert!(verdict("x", Expect::FatalException, true, &early).is_err());
        // Without the self-tests passing first, the run proves nothing about the vectors.
        let no_pass = format!("{EXCEPTION_PREFIX}x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &no_pass).is_err());
        // And the fatal exception run is a failure when a pass is expected.
        assert!(verdict("x", Expect::Pass, true, &reported).is_err());
    }

    #[test]
    fn marker_must_be_a_whole_line() {
        let embedded = format!("not {PASS_MARKER}\n");
        assert!(verdict("x", Expect::Pass, true, &embedded).is_err());
    }
}

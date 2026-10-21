//! Booting kernel images in QEMU.

use crate::Arch;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Must match `eggos_kernel::selftest::{PASS_MARKER, FAIL_MARKER}`.
const PASS_MARKER: &str = "EGGOS-SELFTEST: PASS";
const FAIL_MARKER: &str = "EGGOS-SELFTEST: FAIL";

/// A boot test that has not powered off by then has hung.
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);

const COMMON_ARGS: &[&str] = &["-m", "128M", "-smp", "1", "-no-reboot"];

/// Boots `kernel` headless and passes only if QEMU powers off cleanly within the timeout and
/// the console shows the pass marker and no fail marker.
pub fn boot_test(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!("xtask: booting {} in {}", arch.name, arch.qemu);
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(COMMON_ARGS)
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
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
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

    // The reader thread ends when QEMU closes stdout; collect everything it saw.
    let output: Vec<u8> = rx.iter().flatten().collect();
    let output = String::from_utf8_lossy(&output);
    verdict(arch.name, status.success(), &output)
}

fn verdict(arch: &str, clean_exit: bool, console: &str) -> Result<(), String> {
    let lines = || console.lines().map(str::trim_end);
    let passed = lines().any(|l| l == PASS_MARKER);
    let failed = lines().any(|l| l == FAIL_MARKER);
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
        .arg("-nographic")
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
        assert!(verdict("x", true, &ok).is_ok());
        assert!(verdict("x", false, &ok).is_err());
        assert!(verdict("x", true, "EggOS\r\n").is_err());
    }

    #[test]
    fn fail_marker_wins_over_pass_marker() {
        let mixed = format!("{PASS_MARKER}\n{FAIL_MARKER}\n");
        assert!(verdict("x", true, &mixed).is_err());
    }

    #[test]
    fn marker_must_be_a_whole_line() {
        let embedded = format!("not {PASS_MARKER}\n");
        assert!(verdict("x", true, &embedded).is_err());
    }
}

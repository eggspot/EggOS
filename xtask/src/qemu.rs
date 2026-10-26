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

/// Must match `eggos_hal::user::TestProgram::LINUX_HELLO`, without its newline: the line the
/// self-tests' Linux program writes from user mode, through the `write` system call (M6).
const USER_HELLO: &str = "EGGOS-USER: hello from user mode";

/// Must match `eggos_kernel::selftest::LINUX_START_LINE`, without its newline: the line the
/// self-tests' program started from an ELF image writes from its arguments and environment.
const ELF_START_LINE: &str =
    "EGGOS-USER: started from an ELF image with arguments and an environment";

/// Must match `PASSED_LINE` in `userland/selftest`, without its newline: the line our own
/// program built from Rust source writes when every start-up check passed (D85).
const RUST_PROGRAM_LINE: &str =
    "EGGOS-USER: a program built from Rust source passed its startup checks";

/// Must match `PASSED_LINE` in `userland/std-selftest`: the line our own program built with
/// Rust's standard library writes when every check passed (D86).
const STD_PROGRAM_LINE: &str = "EGGOS-USER: a std program built from Rust source ran its checks";

/// Must match `console::PASSED_LINE` in `userland/std-selftest`: the line the std program
/// writes when it read the console as Linux's line discipline would have it (D100).
const CONSOLE_PROGRAM_LINE: &str =
    "EGGOS-USER: a std program read the console through its line discipline";

/// The lines the Linux test programs write, which a passing boot must show whole.
const USER_LINES: [&str; 4] = [
    USER_HELLO,
    ELF_START_LINE,
    RUST_PROGRAM_LINE,
    STD_PROGRAM_LINE,
];

/// Must match the start of `eggos_kernel::exception::report`'s line.
const EXCEPTION_PREFIX: &str = "kernel exception: ";

/// Must match `eggos_kernel::selftest::WRITE_PROTECT_PROBE`.
const WRITE_PROTECT_PROBE: &str = "fatal exception test: writing read-only data at ";

/// Must match `eggos_kernel::console::INPUT_READY_LINE`: once the kernel writes it, nothing
/// typed on the serial console is lost.
const INPUT_READY_LINE: &str = "EGGOS-CONSOLE: input ready";

/// Must match `eggos_kernel::selftest::CONSOLE_INPUT_SCRIPT`: what the boot tests type on the
/// serial console once it is ready, which a self-test checks arrives through the console's
/// receive interrupt.
const CONSOLE_INPUT_SCRIPT: &[u8] = b"EggOS reads its console\r";

/// What the boot tests type on the serial console, each time its line (the first) appears: the
/// kernel's [`INPUT_READY_LINE`], then the std program's requests, which must match
/// `console::request` in `userland/std-selftest` (the program checks what it reads, D100).
const CONSOLE_SCRIPTS: [(&str, &[u8]); 7] = [
    (INPUT_READY_LINE, CONSOLE_INPUT_SCRIPT),
    ("EGGOS-CONSOLE: type lines", LINES_SCRIPT),
    ("EGGOS-CONSOLE: type an interrupt", b"doomed\x03"),
    (
        "EGGOS-CONSOLE: type an interrupt and a line",
        b"gone\x03after\r",
    ),
    ("EGGOS-CONSOLE: type raw bytes", b"xy\x7fz\r"),
    (
        "EGGOS-CONSOLE: type a line to read without waiting",
        b"ready\r",
    ),
    (
        "EGGOS-CONSOLE: type lines for stdio",
        b"from musl's stdio\rfrom Rust's std\r",
    ),
];

/// Lines edited with backspace, delete, word erase (`^W`) and kill (`^U`), an end of file
/// (`^D`) after bytes and one at the start of a line, then a last line.
const LINES_SCRIPT: &[u8] = b"hello\rab\x08c\rwrongx\x7f\x7f\x7f\x7f\x7f\x7fright\r\
    two words \x17\x17one\rdiscard me\x15kept\rpartial\x04\x04last\r";

/// How the line discipline echoes [`LINES_SCRIPT`] on the console: each erased character
/// rubbed out with backspace, space, backspace, newlines written as carriage return and
/// newline.
const LINES_ECHO: &str = concat!(
    "hello\r\n",
    "ab\x08 \x08c\r\n",
    "wrongx",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "right\r\n",
    "two words ",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "one\r\n",
    "discard me",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "\x08 \x08\x08 \x08\x08 \x08\x08 \x08\x08 \x08",
    "kept\r\n",
    "partiallast\r\n",
);

/// What a boot test expects the kernel to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Pass every self-test and power off, the std program reading the console as the
    /// runner types (D100).
    Pass,
    /// Pass every self-test but the interactive console test, which the benchmark kernels skip
    /// (under `-icount sleep=off`, QEMU does not wake an idle RISC-V hart for serial input),
    /// and power off.
    Benchmarks,
    /// Pass every self-test, then report a fatal exception with the fail marker and power off
    /// (the kernel's `qemu-test-fatal-exception` feature).
    FatalException,
}

/// A boot test that has not powered off by then has hung.
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);

const BENCHMARK_BOOT_TIMEOUT: Duration = Duration::from_secs(90);

/// Start of the kernel's boot banner (`EggOS <version> (<arch>)`, see `eggos_kernel::boot`).
/// Seeing it twice means the machine reset and booted again.
const BANNER_PREFIX: &str = "EggOS ";

/// Arguments every boot gets. Never `-no-reboot`: with it, an x86 triple fault (in the
/// power-off path, say) ends QEMU with exit code 0, which looks like a clean power-off; without
/// it the machine resets and prints the banner again, which fails the test.
const COMMON_ARGS: &[&str] = &["-m", "128M"];

/// CPUs in the boot tests and interactive runs, so every boot starts the secondary CPUs
/// (D40). Must match `eggos_kernel::selftest::QEMU_CPUS`.
pub const TEST_CPUS: u32 = 4;

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

/// Boots `kernel` headless on `cpus` CPUs with `extra_args` added to QEMU's and passes only if
/// QEMU powers off cleanly within the timeout and the console shows what `expect` asks for
/// (see [`verdict`]). Returns the console output.
pub fn boot_test(
    arch: &Arch,
    kernel: &Path,
    expect: Expect,
    cpus: u32,
    extra_args: &[&str],
) -> Result<String, String> {
    eprintln!("xtask: booting {} in {} ({expect:?})", arch.name, arch.qemu);
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(COMMON_ARGS)
        .arg("-smp")
        .arg(cpus.to_string())
        .args(extra_args)
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;

    let rx = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");

    let timeout = match expect {
        Expect::Benchmarks => BENCHMARK_BOOT_TIMEOUT,
        _ => BOOT_TIMEOUT,
    };
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = [0; CONSOLE_SCRIPTS.len()];
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        console.extend(rx.try_iter().flatten());
        for script in scripts_to_type(&String::from_utf8_lossy(&console), &mut typed) {
            if let Err(e) = keyboard.write_all(script).and_then(|()| keyboard.flush()) {
                eprintln!("xtask: {}: could not type on the console: {e}", arch.name);
            }
        }
        if banner_count(&String::from_utf8_lossy(&console)) > 1 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: the machine reset and booted again (crash or triple fault?)",
                arch.name
            ));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: no power-off within {}s (kernel hung?)",
                arch.name,
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let rest_until_qemu_closes_stdout = rx.iter().flatten();
    console.extend(rest_until_qemu_closes_stdout);
    let console = String::from_utf8_lossy(&console).into_owned();
    verdict(arch.name, expect, status.success(), &console)?;
    Ok(console)
}

/// Copies QEMU's console to our stderr as it arrives, so a hang is still visible, and sends
/// each piece to the returned receiver, until QEMU closes its stdout.
fn echo_console_to_stderr(mut stdout: std::process::ChildStdout) -> mpsc::Receiver<Vec<u8>> {
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
    rx
}

/// The scripts of [`CONSOLE_SCRIPTS`] to type now: each as many more times as its line
/// appears whole in `console`, ended by a newline (trailing whitespace aside), beyond the times
/// `typed` counts, which it then counts.
fn scripts_to_type(
    console: &str,
    typed: &mut [usize; CONSOLE_SCRIPTS.len()],
) -> Vec<&'static [u8]> {
    let mut scripts = Vec::new();
    for ((line, script), typed) in CONSOLE_SCRIPTS.iter().zip(typed) {
        let asked = console
            .split_inclusive('\n')
            .filter(|l| l.ends_with('\n') && l.trim_end() == *line)
            .count();
        while *typed < asked {
            scripts.push(*script);
            *typed += 1;
        }
    }
    scripts
}

/// How many times the kernel's boot banner appears on the console.
fn banner_count(console: &str) -> usize {
    console
        .lines()
        .filter(|l| l.starts_with(BANNER_PREFIX) && l.trim_end().ends_with(')'))
        .count()
}

/// [`Expect::Pass`]: a clean exit, the pass marker, no fail marker, the lines the Linux test
/// programs write, and the line discipline's echo of [`LINES_SCRIPT`]. [`Expect::Benchmarks`]:
/// the same but the console program's line and the echo.
/// [`Expect::FatalException`]: a clean exit, the pass marker (the self-tests ran), the address
/// of the read-only data the kernel writes, then a report of an exception at that address
/// followed by the fail marker.
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
        let target = lines().find_map(|l| l.strip_prefix(WRITE_PROTECT_PROBE));
        let fault = target.map(|target| format!(", address {target}"));
        let reported = lines()
            .skip_while(|l| {
                !(l.starts_with(EXCEPTION_PREFIX)
                    && fault
                        .as_ref()
                        .is_some_and(|fault| l.ends_with(fault.as_str())))
            })
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
                "{arch}: the write to read-only data was not reported as a fatal exception at its address"
            )),
        };
    }
    let interactive = expect == Expect::Pass;
    let user_lines = USER_LINES
        .iter()
        .chain(interactive.then_some(&CONSOLE_PROGRAM_LINE))
        .all(|&user_line| lines().any(|l| l == user_line));
    let echoed = !interactive || console.contains(LINES_ECHO);
    match (clean_exit, passed, failed) {
        (true, true, false) if user_lines && echoed => {
            eprintln!("xtask: {arch}: boot test passed");
            Ok(())
        }
        (true, true, false) if user_lines => Err(format!(
            "{arch}: the console did not echo the lines typed as its line discipline should"
        )),
        (true, true, false) => Err(format!(
            "{arch}: a Linux test program's line did not reach the console"
        )),
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
        .arg("-smp")
        .arg(TEST_CPUS.to_string())
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

    fn all_user_lines() -> Vec<String> {
        USER_LINES
            .iter()
            .chain([&CONSOLE_PROGRAM_LINE])
            .map(|line| line.to_string())
            .collect()
    }

    fn passing_console(user_lines: &[String]) -> String {
        format!(
            "EggOS\r\n{LINES_ECHO}{}\r\n{PASS_MARKER}\r\n",
            user_lines.join("\r\n")
        )
    }

    #[test]
    fn passes_only_on_clean_exit_with_pass_marker() {
        let ok = passing_console(&all_user_lines());
        assert!(verdict("x", Expect::Pass, true, &ok).is_ok());
        assert!(verdict("x", Expect::Pass, false, &ok).is_err());
        assert!(verdict("x", Expect::Pass, true, "EggOS\r\n").is_err());
    }

    #[test]
    fn a_pass_needs_every_linux_test_program_s_line_whole() {
        let silent = format!("EggOS\r\n{LINES_ECHO}{PASS_MARKER}\r\n");
        assert!(verdict("x", Expect::Pass, true, &silent).is_err());
        for (index, line) in all_user_lines().iter().enumerate() {
            let mut garbled = all_user_lines();
            garbled[index] = format!("{line}x");
            assert!(verdict("x", Expect::Pass, true, &passing_console(&garbled)).is_err());
            let mut missing = all_user_lines();
            missing.remove(index);
            assert!(verdict("x", Expect::Pass, true, &passing_console(&missing)).is_err());
        }
    }

    #[test]
    fn a_benchmark_boot_needs_no_console_program_line_or_echo() {
        let console = format!("EggOS\r\n{}\r\n{PASS_MARKER}\r\n", USER_LINES.join("\r\n"));
        assert!(verdict("x", Expect::Benchmarks, true, &console).is_ok());
        assert!(verdict("x", Expect::Pass, true, &console).is_err());
        let without_hello = console.replace(USER_HELLO, "");
        assert!(verdict("x", Expect::Benchmarks, true, &without_hello).is_err());
    }

    #[test]
    fn a_pass_needs_the_typed_lines_echoed_as_edited() {
        let ok = passing_console(&all_user_lines());
        let unechoed = ok.replace(LINES_ECHO, "");
        assert!(verdict("x", Expect::Pass, true, &unechoed).is_err());
        let raw = ok.replace(LINES_ECHO, "hello\r\nab\x08c\r\n");
        assert!(verdict("x", Expect::Pass, true, &raw).is_err());
    }

    #[test]
    fn each_script_is_typed_once_each_time_its_line_appears() {
        let mut typed = [0; CONSOLE_SCRIPTS.len()];
        assert!(scripts_to_type("EGGOS-CONSOLE: input rea", &mut typed).is_empty());
        assert!(scripts_to_type("x EGGOS-CONSOLE: input ready", &mut typed).is_empty());
        let console = "boot\r\nEGGOS-CONSOLE: input ready\r\n";
        assert_eq!(scripts_to_type(console, &mut typed), [CONSOLE_INPUT_SCRIPT]);
        assert!(scripts_to_type(console, &mut typed).is_empty());
        let console = format!(
            "{console}EGGOS-CONSOLE: type lines\r\n{LINES_ECHO}EGGOS-CONSOLE: type raw bytes\r\n\
             EGGOS-CONSOLE: type lines\r\nnot EGGOS-CONSOLE: type an interrupt\r\n"
        );
        assert_eq!(
            scripts_to_type(&console, &mut typed),
            [LINES_SCRIPT, LINES_SCRIPT, CONSOLE_SCRIPTS[4].1]
        );
        assert_eq!(typed, [1, 2, 0, 0, 1, 0, 0]);
        let partial = format!("{console}EGGOS-CONSOLE: type an interrupt");
        assert!(scripts_to_type(&partial, &mut typed).is_empty());
        let whole = format!("{partial} and a line\r\n");
        assert_eq!(scripts_to_type(&whole, &mut typed), [CONSOLE_SCRIPTS[3].1]);
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

    fn reported_fatal_exception() -> String {
        format!(
            "{PASS_MARKER}\n{WRITE_PROTECT_PROBE}0x5000\n\n\
             {EXCEPTION_PREFIX}#PF page fault (code 0x3) at pc 0x1000, address 0x5000\n{FAIL_MARKER}\n"
        )
    }

    #[test]
    fn fatal_exception_needs_the_report_then_the_fail_marker() {
        let reported = reported_fatal_exception();
        assert!(verdict("x", Expect::FatalException, true, &reported).is_ok());
        assert!(verdict("x", Expect::FatalException, false, &reported).is_err());
    }

    #[test]
    fn a_plain_pass_a_panic_or_an_early_fail_marker_is_not_the_exception_path() {
        let pass = format!("{PASS_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &pass).is_err());
        let panic = format!("{PASS_MARKER}\nkernel panic: x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &panic).is_err());
        let early = format!("{FAIL_MARKER}\n{EXCEPTION_PREFIX}x\n");
        assert!(verdict("x", Expect::FatalException, true, &early).is_err());
    }

    #[test]
    fn a_fatal_exception_without_the_self_tests_passing_first_proves_nothing() {
        let no_pass = format!("{EXCEPTION_PREFIX}x\n{FAIL_MARKER}\n");
        assert!(verdict("x", Expect::FatalException, true, &no_pass).is_err());
    }

    #[test]
    fn a_fault_elsewhere_or_the_illegal_instruction_after_an_unprotected_write_is_not_caught() {
        let elsewhere = reported_fatal_exception().replace("address 0x5000", "address 0x6000");
        assert!(verdict("x", Expect::FatalException, true, &elsewhere).is_err());
        let unprotected = format!(
            "{PASS_MARKER}\n{WRITE_PROTECT_PROBE}0x5000\n\n\
             {EXCEPTION_PREFIX}#UD invalid opcode (code 0x0) at pc 0x1000\n{FAIL_MARKER}\n"
        );
        assert!(verdict("x", Expect::FatalException, true, &unprotected).is_err());
    }

    #[test]
    fn the_fatal_exception_run_fails_when_a_pass_is_expected() {
        assert!(verdict("x", Expect::Pass, true, &reported_fatal_exception()).is_err());
    }

    #[test]
    fn marker_must_be_a_whole_line() {
        let embedded = format!("not {PASS_MARKER}\n");
        assert!(verdict("x", Expect::Pass, true, &embedded).is_err());
    }
}

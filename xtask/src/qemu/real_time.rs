//! The real-time clock boot test (D150), on every architecture: one boot of the release
//! kernel, whose real-time clock (x86_64's CMOS clock, AArch64's PL031, RISC-V's Goldfish)
//! follows the host's clock (QEMU's `-rtc base=utc`, its default). The boot log names the clock
//! the kernel read; `date` reads within [`TOLERANCE_SECONDS`] of the host's time; a file made
//! in `/tmp` has that time as its modification time (`date -r`); `hwclock -r` reads the clock
//! itself; and `date -s` sets both the wall clock and the real-time clock, which `hwclock -r`
//! then reads back.

use super::settings::Boot;
use super::*;

const TOLERANCE_SECONDS: u64 = 5 * 60;
const FILE_TOLERANCE_SECONDS: u64 = 2;
const SET_TO: &str = "2030-01-02 03:04:05";
const SET_TO_SECONDS: u64 = 1_893_553_445;
const SESSION: [&[u8]; 2] = [
    b"date '+EGGOS-DATE %s'; touch /tmp/rtc-check; date -r /tmp/rtc-check '+EGGOS-FILE %s'; \
      hwclock -r; echo EGGOS-HWCLOCK $?\r",
    b"date -s '2030-01-02 03:04:05'; date '+EGGOS-SET %s'; hwclock -r; \
      echo EGGOS-SET-HWCLOCK $?\r",
];
const POWER_OFF: &[u8] = b"poweroff\r";
const HWCLOCK_SUFFIX: &str = "+00:00";

fn clock_driver(arch: &str) -> &'static str {
    match arch {
        "x86_64" => "rtc-cmos",
        "aarch64" => "rtc-pl031",
        _ => "rtc-goldfish",
    }
}

fn number_after(console: &str, prefix: &str) -> Option<u64> {
    console
        .lines()
        .map(str::trim_end)
        .find_map(|line| line.strip_prefix(prefix)?.parse().ok())
}

fn hwclock_readings(console: &str) -> Vec<u64> {
    console
        .lines()
        .map(str::trim_end)
        .filter_map(|line| {
            let moment = line.strip_suffix(HWCLOCK_SUFFIX)?;
            u64::try_from(eggos_calendar::parse_setting(moment, 0).ok()?).ok()
        })
        .collect()
}

fn near(read: u64, expected: u64, tolerance: u64) -> bool {
    read.abs_diff(expected) <= tolerance
}

fn verdict(arch: &str, console: &str, host: u64, clean_exit: bool) -> Result<(), String> {
    let driver = clock_driver(arch);
    let started = console.lines().map(str::trim_end).any(|line| {
        line.starts_with("clock: ") && line.ends_with(&format!(" UTC from {driver}"))
    });
    if !started {
        return Err(format!(
            "{arch}: the boot log has no `clock: ... UTC from {driver}` line"
        ));
    }
    let date = number_after(console, "EGGOS-DATE ")
        .ok_or_else(|| format!("{arch}: no `EGGOS-DATE` line from `date`"))?;
    if !near(date, host, TOLERANCE_SECONDS) {
        return Err(format!(
            "{arch}: `date` read {date}, the host {host}: more than {TOLERANCE_SECONDS} s apart"
        ));
    }
    let file = number_after(console, "EGGOS-FILE ")
        .ok_or_else(|| format!("{arch}: no `EGGOS-FILE` line from `date -r`"))?;
    if !near(file, date, FILE_TOLERANCE_SECONDS) {
        return Err(format!(
            "{arch}: a file made at {date} has the modification time {file}"
        ));
    }
    let readings = hwclock_readings(console);
    let &[hardware, hardware_after_set] = readings.as_slice() else {
        return Err(format!(
            "{arch}: `hwclock -r` printed {} times, not twice",
            readings.len()
        ));
    };
    if !near(hardware, date, TOLERANCE_SECONDS) {
        return Err(format!(
            "{arch}: `hwclock -r` read {hardware}, `date` {date}"
        ));
    }
    let set = number_after(console, "EGGOS-SET ")
        .ok_or_else(|| format!("{arch}: no `EGGOS-SET` line after `date -s`"))?;
    for (what, read) in [("date", set), ("hwclock -r", hardware_after_set)] {
        if !near(read, SET_TO_SECONDS, FILE_TOLERANCE_SECONDS) {
            return Err(format!(
                "{arch}: after `date -s '{SET_TO}'`, `{what}` read {read}, not {SET_TO_SECONDS}"
            ));
        }
    }
    for wanted in ["EGGOS-HWCLOCK 0", "EGGOS-SET-HWCLOCK 0", POWER_DOWN_LINE] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly after the real-time clock test"
        ));
    }
    Ok(())
}

/// Runs the real-time clock boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?;
    let host = host_seconds();
    for _ in SESSION {
        boot.type_next(&SESSION)?;
    }
    boot.wait_for_prompts(SESSION.len() + 1)?;
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, host, clean_exit)?;
    eprintln!("xtask: {}: real-time clock boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(date: u64, file: u64, hardware: &str, set: u64, set_hardware: &str) -> String {
        format!(
            "EggOS 0.2.0 (aarch64)\nclock: 2026-10-29 14:05:09 UTC from rtc-pl031\n\
             eggsh:/# date '+EGGOS-DATE %s'; ...\nEGGOS-DATE {date}\nEGGOS-FILE {file}\n\
             {hardware}+00:00\nEGGOS-HWCLOCK 0\nWed Jan  2 03:04:05 UTC 2030\nEGGOS-SET {set}\n\
             {set_hardware}+00:00\nEGGOS-SET-HWCLOCK 0\n{POWER_DOWN_LINE}\n"
        )
    }

    #[test]
    fn the_verdict_wants_every_reading_near_the_host_s_time_and_the_set_one() {
        let host = 1_793_282_709;
        let good = console(
            host + 30,
            host + 31,
            "2026-10-29 14:05:40",
            SET_TO_SECONDS,
            SET_TO,
        );
        assert_eq!(verdict("aarch64", &good, host, true), Ok(()));
        assert!(verdict("x86_64", &good, host, true).is_err());
        assert!(verdict("aarch64", &good, host, false).is_err());
        assert!(verdict("aarch64", &good, host + 3600, true).is_err());
        let stale_file = console(host, host - 60, "2026-10-29 14:05:09", SET_TO_SECONDS, SET_TO);
        assert!(verdict("aarch64", &stale_file, host, true).is_err());
        let unsaved = console(
            host,
            host,
            "2026-10-29 14:05:09",
            SET_TO_SECONDS,
            "2026-10-29 14:05:12",
        );
        assert!(verdict("aarch64", &unsaved, host, true).is_err());
    }

    #[test]
    fn the_session_sets_the_moment_the_check_expects() {
        assert!(String::from_utf8_lossy(SESSION[1]).contains(SET_TO));
        assert_eq!(
            eggos_calendar::parse_setting(SET_TO, 0),
            Ok(SET_TO_SECONDS as i64)
        );
        assert_eq!(clock_driver("riscv64"), "rtc-goldfish");
    }
}

//! The static glibc boot test (D188), on every architecture: one boot of the release kernel
//! with our Linux console program (`tests/linux-static`) linked statically against glibc,
//! as a static executable and as a static PIE (`static_glibc.rs`). The session runs both from
//! the shell, as the Linux programs boot test runs the musl builds: arguments, environment, a
//! file, a directory, the clocks, a `HashMap`, threads, the CPU count, standard error, an exit
//! status and an aborting panic; then `poweroff` ends the boot.

use super::settings::Boot;
use super::*;

/// The static executable's path in the image.
pub const STATIC_PROGRAM_PATH: &str = "bin/linux-static-glibc";
/// The static PIE's path in the image.
pub const PIE_PROGRAM_PATH: &str = "bin/linux-static-glibc-pie";

const SESSION: [&[u8]; 4] = [
    b"GREETING=hi linux-static-glibc a 'b c'; echo LINUX-STATIC status $?\r",
    b"GREETING=hi /bin/linux-static-glibc-pie a 'b c'; echo LINUX-STATIC status $?\r",
    b"linux-static-glibc-pie exit 42; echo LINUX-STATIC status $?\r",
    b"linux-static-glibc panic; echo LINUX-STATIC status $?\r",
];
const POWER_OFF: &[u8] = b"poweroff\r";

fn outputs() -> [Vec<Line>; SESSION.len()] {
    [
        linux_static_lines("linux-static-glibc"),
        linux_static_lines("linux-static-glibc-pie"),
        linux_static_exit_lines(),
        linux_static_panic_lines(),
    ]
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    session_verdict(
        arch,
        clean_exit,
        console,
        &SESSION,
        outputs(),
        "static glibc",
    )?;
    if !has_line(console, POWER_DOWN_LINE) {
        return Err(format!(
            "{arch}: the console has no `{POWER_DOWN_LINE}` line after the static glibc programs"
        ));
    }
    Ok(())
}

/// Runs the static glibc boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch` with both builds in its initramfs.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?;
    for _ in SESSION {
        boot.type_next(&SESSION)?;
    }
    boot.wait_for_prompts(SESSION.len() + 1)?;
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> String {
        let mut console = String::new();
        for (typed, lines) in SESSION.iter().zip(outputs()) {
            let typed = String::from_utf8_lossy(typed);
            let command = typed.trim_end_matches('\r');
            console.push_str(&format!("{SHELL_PROMPT}# {command}\n"));
            for line in lines {
                match line {
                    Line::Whole(whole) => console.push_str(&whole),
                    Line::Around(start, end) => console.push_str(&format!("{start}x{end}")),
                }
                console.push('\n');
            }
        }
        console.push_str(&format!("{SHELL_PROMPT}# poweroff\n{POWER_DOWN_LINE}\n"));
        console
    }

    #[test]
    fn the_verdict_wants_each_program_s_lines_and_a_clean_power_off() {
        let good = console();
        assert_eq!(verdict("aarch64", &good, true), Ok(()));
        assert!(verdict("aarch64", &good, false).is_err());
        let failed = good.replacen("LINUX-STATIC status 0", "LINUX-STATIC status 127", 1);
        assert!(verdict("aarch64", &failed, true).is_err());
        let not_pie = good.replace("linux-static-glibc-pie args", "linux-static-glibc args");
        assert!(verdict("riscv64", &not_pie, true).is_err());
        let no_power_off = good.replace(POWER_DOWN_LINE, "");
        assert!(verdict("x86_64", &no_power_off, true).is_err());
    }

    #[test]
    fn the_session_runs_the_programs_at_the_paths_the_image_has() {
        let typed: String = SESSION
            .iter()
            .map(|line| String::from_utf8_lossy(line))
            .collect();
        let name = STATIC_PROGRAM_PATH.rsplit('/').next().unwrap();
        assert!(typed.contains(&format!("GREETING=hi {name} ")));
        assert!(typed.contains(&format!("GREETING=hi /{PIE_PROGRAM_PATH} ")));
    }
}

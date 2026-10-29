//! The dynamic linking boot test (D167), on every architecture: one boot of the release
//! kernel with our dynamically linked hello built against glibc and against musl, each with
//! its dynamic loader and C library added to the image (`dynamic_linking.rs`). The test
//! runs both from the shell, with arguments; each must print its line through its C library's
//! `printf` and exit with 0, then `poweroff` ends the boot.

use super::settings::Boot;
use super::*;

/// The glibc build's path in the image.
pub const GLIBC_PROGRAM_PATH: &str = "bin/linux-dynamic-glibc";
/// The musl build's path in the image.
pub const MUSL_PROGRAM_PATH: &str = "bin/linux-dynamic-musl";

const SESSION: [&[u8]; 2] = [
    b"linux-dynamic-glibc one two; echo LINUX-DYNAMIC glibc status $?\r",
    b"linux-dynamic-musl one; echo LINUX-DYNAMIC musl status $?\r",
];
const POWER_OFF: &[u8] = b"poweroff\r";
const LINES: [&str; 4] = [
    "LINUX-DYNAMIC glibc hello, 3 arguments",
    "LINUX-DYNAMIC glibc status 0",
    "LINUX-DYNAMIC musl hello, 2 arguments",
    "LINUX-DYNAMIC musl status 0",
];

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    for wanted in LINES.iter().copied().chain([POWER_DOWN_LINE]) {
        if !has_line(console, wanted) {
            return Err(format!(
                "{arch}: the console has no `{wanted}` line (a dynamically linked program \
                 did not run)"
            ));
        }
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly after the dynamic linking test"
        ));
    }
    Ok(())
}

/// Runs the dynamic linking boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch` with both builds and their libraries in its initramfs.
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
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: dynamic linking boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_wants_both_programs_lines_and_a_clean_power_off() {
        let good = format!(
            "eggsh:/# linux-dynamic-glibc one two\n{}\n{}\neggsh:/# linux-dynamic-musl one\n{}\n\
             {}\n{POWER_DOWN_LINE}\n",
            LINES[0], LINES[1], LINES[2], LINES[3]
        );
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let failed = good.replace("musl status 0", "musl status 127");
        assert!(verdict("x86_64", &failed, true).is_err());
    }

    #[test]
    fn the_session_runs_the_programs_at_the_paths_the_image_has() {
        for (line, path) in SESSION.iter().zip([GLIBC_PROGRAM_PATH, MUSL_PROGRAM_PATH]) {
            let name = path.rsplit('/').next().unwrap();
            assert!(line.starts_with(name.as_bytes()));
        }
    }
}

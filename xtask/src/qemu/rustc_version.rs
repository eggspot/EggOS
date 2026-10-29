//! The `rustc --version` boot test (D168), on x86_64 and aarch64: one boot of the release
//! kernel with the official toolchain's `rustc` on a work disk (`rustc_version.rs`) and its
//! dynamic loader in the image. The shell points glibc's loader at the work disk's libraries,
//! runs `rustc --version`, which must print its version line and exit with 0, and powers off.

use super::settings::Boot;
use super::*;
use crate::rustc_version::{LIBRARY_FOLDER, RUSTC_PATH, VERSION_LINE_PREFIX};

/// The guest memory: the toolchain's libraries are mapped from the work disk and each page
/// `rustc` touches takes a frame of its own (D167).
const MEMORY: &str = "2G";

/// Loading LLVM and the compiler's libraries page by page from the work disk, without KVM.
const TIMEOUT: Duration = Duration::from_secs(900);

const STATUS_PREFIX: &str = "RUSTC-VERSION status ";

fn session() -> [String; 2] {
    [
        format!("export LD_LIBRARY_PATH={LIBRARY_FOLDER}\r"),
        format!("{RUSTC_PATH} --version; echo {STATUS_PREFIX}$?\r"),
    ]
}

const POWER_OFF: &[u8] = b"poweroff\r";

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    if !console
        .lines()
        .any(|line| line.trim_end().starts_with(VERSION_LINE_PREFIX))
    {
        let status = console
            .lines()
            .find(|line| line.starts_with(STATUS_PREFIX))
            .unwrap_or("no status line");
        return Err(format!(
            "{arch}: the console has no `{VERSION_LINE_PREFIX}...` line ({status}): the \
             official rustc did not run"
        ));
    }
    let wanted = format!("{STATUS_PREFIX}0");
    for wanted in [wanted.as_str(), POWER_DOWN_LINE] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly after the rustc test"
        ));
    }
    Ok(())
}

/// Runs the `rustc --version` boot test (see the module documentation) on `kernel`, the
/// release kernel for `arch` with `rustc`'s dynamic loader in its initramfs, with the work disk
/// `image` holding the toolchain and its libraries.
pub fn check(arch: &Arch, kernel: &Path, image: &Path) -> Result<(), String> {
    let mut args = work_disk_args(arch, &image.to_string_lossy());
    args.extend(["-m".to_owned(), MEMORY.to_owned()]);
    let session = session();
    let lines: Vec<&[u8]> = session.iter().map(|line| line.as_bytes()).collect();
    let mut boot = Boot::start(arch, kernel, &args)?.with_timeout(TIMEOUT);
    for _ in &lines {
        boot.type_next(&lines)?;
    }
    boot.wait_for_prompts(lines.len() + 1)?;
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)?;
    eprintln!("xtask: {}: rustc --version boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_wants_the_version_line_status_0_and_a_clean_power_off() {
        let good = format!(
            "eggsh:/# {RUSTC_PATH} --version; echo {STATUS_PREFIX}$?\n\
             rustc 1.99.1 (abcdef012 2026-10-13)\n{STATUS_PREFIX}0\n{POWER_DOWN_LINE}\n"
        );
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let failed = good
            .replace("rustc 1.99.1 (abcdef012 2026-10-13)\n", "")
            .replace(&format!("{STATUS_PREFIX}0"), &format!("{STATUS_PREFIX}127"));
        let error = verdict("x86_64", &failed, true).unwrap_err();
        assert!(error.contains(&format!("{STATUS_PREFIX}127")), "{error}");
    }

    #[test]
    fn the_session_sets_the_library_path_before_it_runs_rustc() {
        let [library, run] = session();
        assert_eq!(library, "export LD_LIBRARY_PATH=/work/glibc\r");
        assert!(run.starts_with("/work/rustc/bin/rustc --version;"));
        assert!(run.ends_with("$?\r"));
    }
}

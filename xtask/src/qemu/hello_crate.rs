//! The hello-world crate boot test (D171), on x86_64 and aarch64: one boot of the release kernel
//! with 4 GiB of memory, the official toolchain and a copy of `tests/hello-crate` on a work disk
//! (`hello_crate.rs`), and glibc's dynamic loader in the image. The shell sets the environment
//! cargo runs in, runs `cargo build` and `cargo run` in the crate's folder with the program's
//! output going to the work disk, which must both exit with 0, and powers off.

use super::settings::Boot;
use super::*;
use crate::hello_crate::{CRATE_FOLDER, OUTPUT_PATH, cargo_environment};
use crate::rustc_version::LIBRARY_FOLDER;

const MEMORY: &str = "4G";

const TIMEOUT: Duration = Duration::from_secs(3600);

const TOOLCHAIN: &str = "/work/rustc";

const BUILD_STATUS: &str = "HELLO-CRATE build status ";

const RUN_STATUS: &str = "HELLO-CRATE run status ";

const POWER_OFF: &[u8] = b"poweroff\r";

fn session() -> Vec<String> {
    let mut lines: Vec<String> = cargo_environment(
        TOOLCHAIN,
        "/work/cargo-home",
        "/work/tmp",
        Some(LIBRARY_FOLDER),
    )
    .into_iter()
    .map(|(name, value)| format!("export {name}={value}\r"))
    .collect();
    lines.push(format!("export PATH={TOOLCHAIN}/bin:/bin\r"));
    lines.push("export HOME=/work\r".to_owned());
    lines.push(format!("cd {CRATE_FOLDER}\r"));
    lines.push(format!(
        "{TOOLCHAIN}/bin/cargo build --locked; echo {BUILD_STATUS}$?\r"
    ));
    lines.push(format!(
        "{TOOLCHAIN}/bin/cargo run --locked --quiet > {OUTPUT_PATH}; echo {RUN_STATUS}$?\r"
    ));
    lines
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    for prefix in [BUILD_STATUS, RUN_STATUS] {
        let wanted = format!("{prefix}0");
        if !has_line(console, &wanted) {
            let found = console
                .lines()
                .find(|line| line.starts_with(prefix))
                .unwrap_or("no status line");
            return Err(format!(
                "{arch}: the console has no `{wanted}` line ({found}): cargo did not {} the \
                 crate inside EggOS",
                match prefix {
                    BUILD_STATUS => "build",
                    _ => "run",
                }
            ));
        }
    }
    if !has_line(console, POWER_DOWN_LINE) {
        return Err(format!(
            "{arch}: the console has no `{POWER_DOWN_LINE}` line"
        ));
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly after the hello-crate test"
        ));
    }
    Ok(())
}

/// Runs the hello-world crate boot test (see the module documentation) on `kernel`, the release
/// kernel for `arch` with the toolchain's dynamic loader in its initramfs, with the work disk
/// `image` holding the toolchain, its libraries and the crate; the caller unpacks the image.
pub fn check(arch: &Arch, kernel: &Path, image: &Path) -> Result<(), String> {
    let mut args = work_disk_args(arch, &image.to_string_lossy());
    args.extend(memory_args(MEMORY)?);
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
    eprintln!(
        "xtask: {}: cargo build and cargo run boot test passed",
        arch.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_wants_both_statuses_0_and_a_clean_power_off() {
        let good = format!("{BUILD_STATUS}0\n{RUN_STATUS}0\n{POWER_DOWN_LINE}\n");
        assert_eq!(verdict("x86_64", &good, true), Ok(()));
        assert!(verdict("x86_64", &good, false).is_err());
        let failed = good.replace(&format!("{BUILD_STATUS}0"), &format!("{BUILD_STATUS}101"));
        let error = verdict("x86_64", &failed, true).unwrap_err();
        assert!(error.contains(&format!("{BUILD_STATUS}101")), "{error}");
        assert!(error.contains("build"), "{error}");
        let not_run = good.replace(&format!("{RUN_STATUS}0"), "");
        assert!(
            verdict("x86_64", &not_run, true)
                .unwrap_err()
                .contains("run")
        );
    }

    #[test]
    fn the_session_sets_the_environment_then_builds_and_runs_in_the_crate_s_folder() {
        let session = session();
        assert!(session.iter().all(|line| line.ends_with('\r')));
        let at = |wanted: &str| {
            session
                .iter()
                .position(|line| line.starts_with(wanted))
                .unwrap_or_else(|| panic!("{wanted}"))
        };
        let build = at(&format!("{TOOLCHAIN}/bin/cargo build --locked;"));
        let run = at(&format!(
            "{TOOLCHAIN}/bin/cargo run --locked --quiet > /work/hello.out;"
        ));
        assert!(at("export LD_LIBRARY_PATH=/work/glibc") < build);
        assert!(at("export RUSTC=/work/rustc/bin/rustc") < build);
        assert!(at("export CARGO_HOME=/work/cargo-home") < build);
        assert!(at("cd /work/hello") < build);
        assert!(build < run);
        assert_eq!(run, session.len() - 1);
    }
}

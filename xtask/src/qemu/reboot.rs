//! The reboot boot test (D153), on every architecture: the release image booted with the run
//! scripts' own serial arguments (`run-eggos.sh --serial`, [`interactive_args`]), which have no
//! `-no-reboot`. `reboot` at the first prompt restarts the machine, which boots again to a
//! second prompt in the same QEMU, where `poweroff` ends QEMU cleanly.

use super::*;

const REBOOT_TIMEOUT: Duration = Duration::from_secs(90);
const REBOOT: &[u8] = b"reboot\r";
const POWER_OFF: &[u8] = b"poweroff\r";

fn line_to_type(console: &str, typed: usize) -> Option<&'static [u8]> {
    let prompts = console.matches(SHELL_PROMPT).count();
    match (typed, prompts, banner_count(console)) {
        (0, 1.., _) => Some(REBOOT),
        (1, 2.., 2) => Some(POWER_OFF),
        _ => None,
    }
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let boots = banner_count(console);
    if boots != 2 {
        return Err(format!(
            "{arch}: EggOS booted {boots} times, not twice (once more after `reboot`)"
        ));
    }
    for wanted in [RESTART_LINE, POWER_DOWN_LINE] {
        if !has_line(console, wanted) {
            return Err(format!("{arch}: the console has no `{wanted}` line"));
        }
    }
    let restarted = console.find(RESTART_LINE).unwrap_or(console.len());
    if !console[restarted..].contains(SHELL_PROMPT) {
        return Err(format!(
            "{arch}: no prompt came back after `reboot` restarted the machine"
        ));
    }
    if !clean_exit {
        return Err(format!(
            "{arch}: QEMU did not exit cleanly at `poweroff` after the reboot"
        ));
    }
    Ok(())
}

/// Runs the reboot boot test (see the module documentation) on `kernel`, the release image for
/// `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!("xtask: booting {} in {} (reboot)", arch.name, arch.qemu);
    let mut child = Command::new(arch.qemu)
        .args(interactive_args(arch, false))
        .arg("-kernel")
        .arg(kernel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = 0;
    let status = loop {
        console.extend(output.try_iter().flatten());
        if let Some(line) = line_to_type(&String::from_utf8_lossy(&console), typed) {
            keyboard
                .write_all(line)
                .and_then(|()| keyboard.flush())
                .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
            typed += 1;
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > REBOOT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: the reboot boot test did not power off within {}s",
                arch.name,
                REBOOT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    console.extend(output.iter().flatten());
    verdict(
        arch.name,
        &String::from_utf8_lossy(&console),
        status.success(),
    )?;
    eprintln!("xtask: {}: reboot boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(arch: &str) -> String {
        format!("{BANNER_PREFIX}0.3.0 ({arch})\ninit: starting /bin/eggsh\n{SHELL_PROMPT}# ")
    }

    #[test]
    fn reboot_is_typed_at_the_first_prompt_and_poweroff_after_the_second_boot() {
        let first = boot("x86_64");
        assert_eq!(line_to_type("", 0), None);
        assert_eq!(line_to_type(&first, 0), Some(REBOOT));
        let restarting = format!("{first}reboot\n{RESTART_LINE}\n");
        assert_eq!(line_to_type(&restarting, 1), None);
        let second = format!("{restarting}{}", boot("x86_64"));
        assert_eq!(line_to_type(&second, 1), Some(POWER_OFF));
        assert_eq!(line_to_type(&second, 2), None);
    }

    #[test]
    fn the_verdict_wants_two_boots_a_restart_a_prompt_after_it_and_a_power_off() {
        let first = boot("aarch64");
        let good = format!(
            "{first}reboot\n{RESTART_LINE}\n{}poweroff\n{POWER_DOWN_LINE}\n",
            boot("aarch64")
        );
        assert_eq!(verdict("aarch64", &good, true), Ok(()));
        assert!(verdict("aarch64", &good, false).is_err());
        let no_reboot = format!("{first}reboot\n{RESTART_LINE}\n");
        assert!(verdict("aarch64", &no_reboot, true).is_err());
        let three = format!("{good}{}", boot("aarch64"));
        assert!(verdict("aarch64", &three, true).is_err());
        let no_prompt = format!(
            "{first}reboot\n{RESTART_LINE}\n{BANNER_PREFIX}0.3.0 (aarch64)\n{POWER_DOWN_LINE}\n"
        );
        assert!(verdict("aarch64", &no_prompt, true).is_err());
    }
}

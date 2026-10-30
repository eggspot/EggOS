//! The workspace build's boots (D173), on x86_64 and aarch64: EggOS's release kernel with the
//! work folder on a work disk, and Linux's kernel with the same work folder in its initramfs,
//! each in the same QEMU machine (the architecture's machine, 4 CPUs, 4 GiB, multi-threaded
//! TCG). Each returns the build timer's console lines (`workspace_build.rs`) with when the host
//! read them since the boot started, which time the phases on the host's clock.

use super::settings::Boot;
use super::*;
use crate::workspace_build::{DONE_LINE, MARKER, TIMER_PATH, WORK_PATH};

const MEMORY: &str = "4G";

const TIMEOUT: Duration = Duration::from_secs(3600);

const MULTI_THREADED_TCG: [&str; 2] = ["-accel", "tcg,thread=multi"];

const POWER_OFF: &[u8] = b"poweroff\r";

#[derive(Debug, Default)]
struct Stamped {
    partial: Vec<u8>,
    lines: Vec<(Duration, String)>,
}

impl Stamped {
    fn feed(&mut self, at: Duration, chunk: &[u8]) {
        for &byte in chunk {
            if byte != b'\n' {
                self.partial.push(byte);
                continue;
            }
            let line = String::from_utf8_lossy(&self.partial)
                .trim_end_matches('\r')
                .to_owned();
            self.partial.clear();
            if line.starts_with(MARKER) {
                self.lines.push((at, line));
            }
        }
    }

    fn is_done(&self) -> bool {
        self.lines.iter().any(|(_, line)| line == DONE_LINE)
    }
}

/// Boots `kernel`, EggOS's release kernel for `arch` with the toolchain's dynamic loader in its
/// initramfs, with the work disk `image`, runs the timer from the shell, powers off, and
/// returns the timer's stamped lines.
pub fn eggos(arch: &Arch, kernel: &Path, image: &Path) -> Result<Vec<(Duration, String)>, String> {
    let mut args = work_disk_args(arch, &image.to_string_lossy());
    args.extend(memory_args(MEMORY)?);
    args.extend(MULTI_THREADED_TCG.map(str::to_owned));
    let mut boot = Boot::start(arch, kernel, &args)?.with_timeout(TIMEOUT);
    let line = format!("{TIMER_PATH} {WORK_PATH}\r");
    boot.type_next(&[line.as_bytes()])?;
    let started = Instant::now();
    let mut stamped = Stamped::default();
    while !stamped.is_done() {
        for chunk in boot.output.try_iter() {
            stamped.feed(started.elapsed(), &chunk);
            boot.console.extend(chunk);
        }
        if started.elapsed() > TIMEOUT {
            return Err(boot.fail(format!(
                "{}: the build timer did not finish inside EggOS within {}s",
                arch.name,
                TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    boot.wait_for_prompts(2)?;
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    if !has_line(&console, POWER_DOWN_LINE) || !clean_exit {
        return Err(format!(
            "{}: EggOS did not power off cleanly after the workspace build",
            arch.name
        ));
    }
    Ok(stamped.lines)
}

/// Boots `kernel`, Linux's for `arch`, with `initramfs` (the timer as its `/init` and the work
/// folder at `/work`) and `command_line`, in the same machine EggOS's boot has, until the timer
/// powers it off, and returns the timer's stamped lines.
pub fn linux(
    arch: &Arch,
    kernel: &Path,
    initramfs: &Path,
    command_line: &str,
) -> Result<Vec<(Duration, String)>, String> {
    eprintln!("xtask: booting Linux for {} in {}", arch.name, arch.qemu);
    let cpus = TEST_CPUS.to_string();
    let mut child = Command::new(arch.qemu)
        .args(arch.machine_args)
        .args(COMMON_ARGS)
        .args(memory_args(MEMORY)?)
        .args(["-smp", cpus.as_str()])
        .args(MULTI_THREADED_TCG)
        .args(["-display", "none", "-serial", "stdio", "-monitor", "none"])
        .arg("-no-reboot")
        .arg("-kernel")
        .arg(kernel)
        .arg("-initrd")
        .arg(initramfs)
        .arg("-append")
        .arg(command_line)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
    let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let started = Instant::now();
    let mut stamped = Stamped::default();
    let status = loop {
        for chunk in output.try_iter() {
            stamped.feed(started.elapsed(), &chunk);
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{}: Linux did not power off within {}s",
                arch.name,
                TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    for chunk in output.iter() {
        stamped.feed(started.elapsed(), &chunk);
    }
    if !stamped.is_done() || !status.success() {
        return Err(format!(
            "{}: Linux ended ({status}) {} the build timer's last line",
            arch.name,
            match stamped.is_done() {
                true => "after",
                false => "without",
            }
        ));
    }
    Ok(stamped.lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_timer_s_whole_lines_are_kept_with_when_they_ended() {
        let mut stamped = Stamped::default();
        stamped.feed(
            Duration::from_secs(1),
            b"eggsh:/# /work/build-timer /work\r\nBUILD-TI",
        );
        assert!(stamped.lines.is_empty());
        stamped.feed(
            Duration::from_secs(2),
            b"MER begin build\r\n   Compiling a\nBUILD-TIMER done\n",
        );
        assert_eq!(
            stamped.lines,
            [
                (Duration::from_secs(2), "BUILD-TIMER begin build".to_owned()),
                (Duration::from_secs(2), DONE_LINE.to_owned())
            ]
        );
        assert!(stamped.is_done());
    }
}

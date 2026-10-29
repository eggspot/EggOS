//! The run scripts boot test (D154): the release image and the run scripts `cargo xtask
//! release` writes, driven the way people and Claude Code run them: `run-eggos.ps1` through
//! `powershell -File` on Windows hosts, `run-eggos.sh` through `sh` elsewhere.
//!
//! 1. `-Serial -Apps <folder> -FormatDisk` (`--serial --apps <folder> --format-disk`): the
//!    script makes the disk image itself; the shell's prompt comes on the script's standard
//!    output, `uname` prints `Linux`, `ls /media/host` lists the folder's file, and `poweroff`
//!    ends QEMU and the script with status 0.
//! 2. `-Mcp -NoWindow` with the same folder and disk, as a child with piped standard input and
//!    output, as Claude Code starts it: `initialize`, `tools/list` and a `cat` of the folder's
//!    file, every line on standard output a JSON-RPC message. While it runs, a second start on
//!    the same disk image must be refused at once, with a word about the lock.

use super::mcp::{Client, FIRST_ANSWER_TIMEOUT, result_field};
use super::*;
use std::path::PathBuf;

const SERIAL_TIMEOUT: Duration = Duration::from_secs(180);
const SECOND_START_TIMEOUT: Duration = Duration::from_secs(60);
const HOST_FILE: (&str, &str) = (
    "eggos-run-scripts.txt",
    "EGGOS-RUN-SCRIPTS hello from the host\n",
);
const SERIAL_SESSION: [&[u8]; 3] = [b"uname\r", b"ls /media/host\r", b"poweroff\r"];
const CLIENT_NAME: &str = "xtask-run-scripts";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Serial,
    FormatDisk,
}

impl Flag {
    fn spelling(self, powershell: bool) -> &'static str {
        match (self, powershell) {
            (Flag::Serial, true) => "-Serial",
            (Flag::Serial, false) => "--serial",
            (Flag::FormatDisk, true) => "-FormatDisk",
            (Flag::FormatDisk, false) => "--format-disk",
        }
    }
}

fn script_command(script: &Path, flags: &[Flag], arch: &Arch, apps: &Path, disk: &Path) -> Command {
    let powershell = cfg!(windows);
    let mut command = match powershell {
        true => {
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            command.arg(script).args(["-Arch", arch.name, "-Apps"]);
            command
        }
        false => {
            let mut command = Command::new("sh");
            command.arg(script).args([arch.name, "--apps"]);
            command
        }
    };
    command.arg(apps);
    command.arg(match powershell {
        true => "-Disk",
        false => "--disk",
    });
    command.arg(disk);
    command.args(flags.iter().map(|flag| flag.spelling(powershell)));
    command
}

fn prepare(arch: &Arch, kernel: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let dir = crate::target_directory()
        .join("run-scripts-test")
        .join(arch.name);
    let script = crate::release::write_test_dist(arch, kernel, &dir)?;
    let apps = dir.join("apps");
    std::fs::create_dir_all(&apps).map_err(|e| format!("{}: {e}", apps.display()))?;
    let (name, text) = HOST_FILE;
    std::fs::write(apps.join(name), text).map_err(|e| format!("{}: {e}", apps.display()))?;
    Ok((script, apps, dir.join("disk.img")))
}

fn serial_line_to_type(console: &str, typed: usize) -> Option<&'static [u8]> {
    let prompts = console.matches(SHELL_PROMPT).count();
    SERIAL_SESSION
        .get(typed)
        .copied()
        .filter(|_| prompts > typed)
}

fn serial_verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    let (name, _) = HOST_FILE;
    let after_ls = console
        .find("ls /media/host")
        .map_or("", |at| &console[at..]);
    let checks = [
        (has_line(console, "Linux"), "`uname` did not print `Linux`"),
        (
            after_ls.lines().skip(1).any(|line| line.contains(name)),
            "`ls /media/host` did not list the folder's file",
        ),
        (
            has_line(console, POWER_DOWN_LINE),
            "`poweroff` did not power off",
        ),
        (clean_exit, "the script did not exit with status 0"),
    ];
    match checks.iter().find(|(holds, _)| !holds) {
        Some((_, what)) => Err(format!("{arch}: run scripts check: --serial: {what}")),
        None => Ok(()),
    }
}

fn serial_boot(arch: &Arch, script: &Path, apps: &Path, disk: &Path) -> Result<(), String> {
    eprintln!(
        "xtask: {}: running {} --serial",
        arch.name,
        script.display()
    );
    let mut child = script_command(script, &[Flag::Serial, Flag::FormatDisk], arch, apps, disk)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", script.display()))?;
    let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
    let mut keyboard = child.stdin.take().expect("stdin is piped");
    let started = Instant::now();
    let mut console = Vec::new();
    let mut typed = 0;
    let status = loop {
        console.extend(output.try_iter().flatten());
        if let Some(line) = serial_line_to_type(&String::from_utf8_lossy(&console), typed) {
            keyboard
                .write_all(line)
                .and_then(|()| keyboard.flush())
                .map_err(|e| format!("{}: could not type into the script: {e}", arch.name))?;
            typed += 1;
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > SERIAL_TIMEOUT {
            kill_process_tree(&mut child);
            return Err(format!(
                "{}: run scripts check: --serial did not power off within {}s",
                arch.name,
                SERIAL_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    console.extend(output.iter().flatten());
    serial_verdict(
        arch.name,
        &String::from_utf8_lossy(&console),
        status.success(),
    )
}

fn mcp_session(client: &mut Client, arch: &str) -> Result<(), String> {
    let fail = |what: String| format!("{arch}: run scripts check: --mcp: {what}");
    client.send(br#"{"jsonrpc":"2.0","id":"probe","method":"ping","params":{}}"#)?;
    client.receive(FIRST_ANSWER_TIMEOUT)?;
    let answer = client.request(
        "initialize",
        &format!(
            r#"{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"{CLIENT_NAME}","version":"1"}}}}"#
        ),
    )?;
    if result_field(&answer, "protocolVersion").as_deref() != Some("2025-11-25") {
        return Err(fail(format!("initialize was not answered: {answer}")));
    }
    client.send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)?;
    let tools = client.request("tools/list", "{}")?;
    if !tools.contains(r#""name":"cat""#) {
        return Err(fail(format!("tools/list lacks cat: {tools}")));
    }
    let (name, text) = HOST_FILE;
    let result = client.call("cat", &format!(r#"{{"path":"/media/host/{name}"}}"#))?;
    if result.is_error || !result.all().contains(text.trim_end()) {
        return Err(fail(format!("cat of the folder's file: {}", result.all())));
    }
    Ok(())
}

fn second_start_refused(
    arch: &Arch,
    script: &Path,
    apps: &Path,
    disk: &Path,
) -> Result<(), String> {
    let mut child = script_command(script, &[Flag::Serial], arch, apps, disk)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", script.display()))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > SECOND_START_TIMEOUT {
            kill_process_tree(&mut child);
            return Err(format!(
                "{}: run scripts check: a second start on the disk image in use was not refused \
                 within {}s",
                arch.name,
                SECOND_START_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut errors = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut errors);
    }
    let _ = std::io::stderr().write_all(errors.as_bytes());
    let refused = errors.contains("another EggOS") || errors.contains("lock");
    match !status.success() && refused {
        true => Ok(()),
        false => Err(format!(
            "{}: run scripts check: a second start on the disk image in use was not refused \
             ({status}): {errors}",
            arch.name
        )),
    }
}

/// Runs the run scripts boot test (see the module documentation) on `kernel`, the release
/// image for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let (script, apps, disk) = prepare(arch, kernel)?;
    serial_boot(arch, &script, &apps, &disk)?;
    eprintln!("xtask: {}: running {} --mcp", arch.name, script.display());
    let mut client = Client::start(arch, &script, &apps, &disk, None)?;
    let outcome = mcp_session(&mut client, arch.name)
        .and_then(|()| second_start_refused(arch, &script, &apps, &disk));
    client.stop();
    outcome?;
    eprintln!("xtask: {}: run scripts boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_serial_session_types_one_line_at_each_prompt() {
        assert_eq!(serial_line_to_type("booting", 0), None);
        let one = format!("{SHELL_PROMPT}# ");
        assert_eq!(serial_line_to_type(&one, 0), Some(SERIAL_SESSION[0]));
        assert_eq!(serial_line_to_type(&one, 1), None);
        let three = one.repeat(3);
        assert_eq!(serial_line_to_type(&three, 2), Some(SERIAL_SESSION[2]));
        assert_eq!(serial_line_to_type(&one.repeat(4), 3), None);
    }

    #[test]
    fn the_serial_verdict_wants_uname_the_listing_and_a_clean_power_off() {
        let (name, _) = HOST_FILE;
        let good = format!(
            "{SHELL_PROMPT}# uname\r\nLinux\r\n{SHELL_PROMPT}# ls /media/host\r\n{name}\r\n\
             {SHELL_PROMPT}# poweroff\r\n{POWER_DOWN_LINE}\r\n"
        );
        assert_eq!(serial_verdict("x86_64", &good, true), Ok(()));
        assert!(serial_verdict("x86_64", &good, false).is_err());
        let empty_folder = good.replace(&format!("{name}\r\n"), "");
        assert!(serial_verdict("x86_64", &empty_folder, true).is_err());
        let no_uname = good.replace("Linux\r\n", "");
        assert!(serial_verdict("x86_64", &no_uname, true).is_err());
    }

    #[test]
    fn the_scripts_are_started_with_their_own_spelling_of_each_option() {
        let arch = &crate::ARCHES[2];
        let command = script_command(
            Path::new("run"),
            &[Flag::Serial, Flag::FormatDisk],
            arch,
            Path::new("apps"),
            Path::new("disk.img"),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let expected: &[&str] = match cfg!(windows) {
            true => &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "run",
                "-Arch",
                "x86_64",
                "-Apps",
                "apps",
                "-Disk",
                "disk.img",
                "-Serial",
                "-FormatDisk",
            ],
            false => &[
                "run",
                "x86_64",
                "--apps",
                "apps",
                "--disk",
                "disk.img",
                "--serial",
                "--format-disk",
            ],
        };
        assert_eq!(args, expected);
    }
}

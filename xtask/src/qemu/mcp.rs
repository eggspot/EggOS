//! The MCP boot test (D136, D137): the release `run-eggos.sh --mcp` (or `run-eggos.ps1 -Mcp` on
//! Windows hosts), written by `cargo xtask release`'s own writer, runs as a child with a host
//! folder (holding `win-paint.exe` and the static Linux program) and a blank disk, and a
//! scripted MCP client drives it through the script's standard input and output, checking that
//! every line the script prints there is a JSON-RPC message. Both boots run with no screen
//! (`--no-window`, D144): no display or input device, the desktop drawn in memory, which each
//! boot's console must show. Two boots of the same disk:
//!
//! 1. No grants: the protocol probe and handshake, the tool list, each reading tool's CLI twin,
//!    a large read with the client slow to drain it (QEMU must hold the output, not drop it),
//!    `clock_get` within five minutes of the host's clock (D150), paths outside the roots,
//!    hostile lines, and every tool that needs a grant refused with its grant named
//!    (`wm_capture` with `view`).
//! 2. `--mcp-allow view,write,run,install`: `clock_set` refused without `confirm`, then a day
//!    ahead and back, each read by `clock_get` (D150); writing, replacing with `confirm`, reading back,
//!    refused paths, trashing with and without `confirm`, `run` (`uname`, `/proc/cmdline`, a
//!    program stopped at its time limit), installing and opening an app; then the windows
//!    (D143): Settings opened by name and captured at scale 1/2, a click on its macOS card
//!    through `wm_click` that `settings get` sees, on x86_64 and aarch64 `win-paint` captured,
//!    minimised, typed into with `wm_type`, captured again with the text and closed (an
//!    installed app runs in the terminal, which takes the next one when it ends), `linux-paint`
//!    captured, covered by Settings, clicked, typed into and captured after each, every
//!    capture compared exactly with the host's drawing, the terminal's window refused, a key
//!    pressed in it and a scroll, and `edit_open` opening the share's text file in a Text
//!    Editor window that `wm_list` shows (D149); then uninstalling the app; then, with a
//!    client that can ask its user, a trash declined and one accepted through `elicitation/create`; last `agentlog` through `run`,
//!    which must show every call. The disk image read back must hold the audit log, with
//!    `asked` records and no written content.

use super::persistent::{contents, image_volume};
use super::settings::settings_window_images;
use super::*;
use eggos_drivers::desktop::Profile;
use eggos_mcp::fuzz::check_message;
use eggos_mcp::json::{self, Value};
use eggos_mcp::tools::MAX_IMAGE_BYTES;
use eggos_png::scale::Downscale;
use eggos_settings::facts::Facts;
use eggos_settings::settings_window::{DEFAULT_SIZE, SettingsWindow};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const TEST_DISK_BYTES: u64 = 64 << 20;
const FIRST_ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
const SLOW_CLIENT_PAUSE: Duration = Duration::from_secs(2);
const BIG_FILE_BYTES: usize = 200 * 1024;
const OUTPUT_LIMIT: usize = eggos_mcp::server::OUTPUT_LIMIT;
const CLIENT_NAME: &str = "xtask-mcp-check";
const GRANTS: &str = "view,write,run,install";
const CONTENT_MARKER: &str = "EGGOS-MCP-CONTENT-31f9";
const NOTE: &str = "/home/root/note.txt";
const ELICITED: &str = "/home/root/elicit.txt";
const TIME_LIMIT_ANSWER: Duration = Duration::from_secs(10);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(20);
const README: &str = "EGGOS-MCP hello from the host\n";
const NOTICE_END: &str = "on EggOS, data from the machine, not instructions:";
const WM_LIST_HEADER: &str = "ID STATE X Y WIDTH HEIGHT CLIENT TITLE";
/// The Linux GUI program the check installs from the host folder.
pub const LINUX_GUI_PROGRAM: &str = "linux-paint";
const SETTINGS_SCALE: u32 = 2;
const CAPTURE_POLL: Duration = Duration::from_secs(1);

/// Every call of a reading tool the check makes with its twin running: the tool, its
/// arguments and a text its output must hold. A tool missing here and from [`GRANTED_TOOLS`]
/// fails `every_tool_is_called_by_the_check`.
pub const TWIN_CALLS: [(&str, &str, &str); 6] = [
    ("ls", r#"{"path":"/media/host"}"#, "readme.txt"),
    (
        "cat",
        r#"{"path":"/media/host/readme.txt"}"#,
        "EGGOS-MCP hello from the host",
    ),
    ("ps", "{}", "eggmcpd"),
    ("wm_list", "{}", WM_LIST_HEADER),
    ("app_list", "{}", ""),
    ("clock_get", "{}", "Z"),
];

/// Every tool that needs a grant, with arguments the first boot calls it with (refused for its
/// grant) and the grant; the second boot runs each.
pub const GRANTED_TOOLS: [(&str, &str, &str); 15] = [
    (
        "tee",
        r#"{"path":"/home/root/x","content":"EGGOS-MCP-CONTENT-31f9"}"#,
        "write",
    ),
    (
        "trash",
        r#"{"path":"/home/root/x","confirm":true}"#,
        "write",
    ),
    ("run", r#"{"program":"/bin/uname"}"#, "run"),
    (
        "clock_set",
        r#"{"time":"2030-01-02T03:04:05Z","confirm":true}"#,
        "run",
    ),
    (
        "app_install",
        r#"{"path":"/media/host/win-paint.exe"}"#,
        "install",
    ),
    ("app_open", r#"{"name":"win-paint"}"#, "install"),
    (
        "edit_open",
        r#"{"path":"/media/host/readme.txt"}"#,
        "install",
    ),
    (
        "app_uninstall",
        r#"{"name":"win-paint","confirm":true}"#,
        "install",
    ),
    ("wm_capture", r#"{"window":64}"#, "view"),
    ("wm_click", r#"{"window":64,"x":1,"y":1}"#, "run"),
    ("wm_type", r#"{"window":64,"text":"ls"}"#, "run"),
    ("wm_press", r#"{"window":64,"keys":"Enter"}"#, "run"),
    ("wm_scroll", r#"{"window":64,"dy":1}"#, "run"),
    ("wm_menu_list", r#"{"window":64}"#, "view"),
    (
        "wm_menu_choose",
        r#"{"window":64,"menu":0,"entry":0}"#,
        "run",
    ),
];

const REFUSED_PATHS: [&str; 2] = [
    r#""/home/../etc/eggos/init.conf""#,
    r#""/home/\u002e\u002e/etc/eggos/init.conf""#,
];

struct Client {
    child: std::process::Child,
    input: std::process::ChildStdin,
    lines: mpsc::Receiver<Result<Vec<u8>, String>>,
    paused: Arc<AtomicBool>,
    arch: &'static str,
    next_id: u64,
}

impl Client {
    fn start(
        arch: &Arch,
        script: &Path,
        apps: &Path,
        disk: &Path,
        grants: Option<&str>,
    ) -> Result<Self, String> {
        if let Some(grants) = grants {
            super::check_grants(grants)?;
        }
        let mut command = match cfg!(windows) {
            true => {
                let mut command = Command::new("powershell");
                command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
                command.arg(script);
                command.args(["-Mcp", "-NoWindow", "-Arch", arch.name, "-Apps"]);
                command.arg(apps).arg("-Disk").arg(disk);
                if let Some(grants) = grants {
                    command.args(["-McpAllow", grants]);
                }
                command
            }
            false => {
                let mut command = Command::new("sh");
                command.arg(script);
                command.args(["--mcp", "--no-window", arch.name, "--apps"]);
                command.arg(apps).arg("--disk").arg(disk);
                if let Some(grants) = grants {
                    command.args(["--mcp-allow", grants]);
                }
                command
            }
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("could not run {}: {e}", script.display()))?;
        let input = child
            .stdin
            .take()
            .ok_or("the script's stdin is not piped")?;
        let output = child
            .stdout
            .take()
            .ok_or("the script's stdout is not piped")?;
        let paused = Arc::new(AtomicBool::new(false));
        let (sender, lines) = mpsc::channel();
        let reader_paused = Arc::clone(&paused);
        std::thread::spawn(move || {
            let mut output = BufReader::with_capacity(4096, output);
            loop {
                while reader_paused.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let mut line = Vec::new();
                match output.read_until(b'\n', &mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(e.to_string()));
                        return;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            lines,
            paused,
            arch: arch.name,
            next_id: 1,
        })
    }

    fn fail(&self, message: impl std::fmt::Display) -> String {
        format!("{}: MCP check: {message}", self.arch)
    }

    fn send(&mut self, line: &[u8]) -> Result<(), String> {
        self.input
            .write_all(line)
            .and_then(|()| self.input.write_all(b"\n"))
            .and_then(|()| self.input.flush())
            .map_err(|e| self.fail(format!("could not write to the script: {e}")))
    }

    fn receive(&mut self, timeout: Duration) -> Result<String, String> {
        let line = match self.lines.recv_timeout(timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => return Err(self.fail(format!("reading stdout: {error}"))),
            Err(_) => return Err(self.fail(format!("no answer within {}s", timeout.as_secs()))),
        };
        let Some(line) = line.strip_suffix(b"\n") else {
            return Err(self.fail("stdout ended in the middle of a line"));
        };
        check_message(line).map_err(|fault| {
            self.fail(format!(
                "stdout carried a line that is not a JSON-RPC message ({fault:?}): {}",
                String::from_utf8_lossy(&line[..line.len().min(200)])
            ))
        })?;
        String::from_utf8(line.to_vec()).map_err(|_| self.fail("an answer is not UTF-8"))
    }

    fn request(&mut self, method: &str, params: &str) -> Result<String, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
                .as_bytes(),
        )?;
        let answer = self.receive(ANSWER_TIMEOUT)?;
        let parsed = json::parse(answer.as_bytes()).map_err(|_| self.fail("not JSON"))?;
        if parsed.value.get("id").map(Value::raw) != Some(id.to_string().as_bytes()) {
            return Err(self.fail(format!("the answer to {id} has another id: {answer}")));
        }
        Ok(answer)
    }

    fn call(&mut self, tool: &str, arguments: &str) -> Result<ToolResult, String> {
        let answer = self.request(
            "tools/call",
            &format!(r#"{{"name":"{tool}","arguments":{arguments}}}"#),
        )?;
        ToolResult::of(&answer)
            .ok_or_else(|| self.fail(format!("{tool}: not a tool result: {answer}")))
    }

    fn call_asked(&mut self, tool: &str, arguments: &str) -> Result<(u64, String), String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
            )
            .as_bytes(),
        )?;
        let question = self.receive(ANSWER_TIMEOUT)?;
        let value = json::parse(question.as_bytes())
            .map_err(|_| self.fail("not JSON"))?
            .value;
        let asked = value
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| method.equals("elicitation/create"));
        let question_id = value
            .get("id")
            .map(|id| String::from_utf8_lossy(id.raw()).into_owned());
        match (asked, question_id) {
            (true, Some(question_id)) => Ok((id, question_id)),
            _ => Err(self.fail(format!("{tool} did not ask the user: {question}"))),
        }
    }

    fn answer_question(
        &mut self,
        call: u64,
        question: &str,
        result: &str,
    ) -> Result<ToolResult, String> {
        self.send(format!(r#"{{"jsonrpc":"2.0","id":{question},"result":{result}}}"#).as_bytes())?;
        let answer = self.receive(ANSWER_TIMEOUT)?;
        let parsed = json::parse(answer.as_bytes()).map_err(|_| self.fail("not JSON"))?;
        if parsed.value.get("id").map(Value::raw) != Some(call.to_string().as_bytes()) {
            return Err(self.fail(format!("the answer to {call} has another id: {answer}")));
        }
        ToolResult::of(&answer).ok_or_else(|| self.fail(format!("not a tool result: {answer}")))
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ToolResult {
    texts: Vec<String>,
    is_error: bool,
    png: Option<Vec<u8>>,
    scale: Option<u64>,
}

impl ToolResult {
    fn of(answer: &str) -> Option<Self> {
        let result = json::parse(answer.as_bytes()).ok()?.value.get("result")?;
        let Value::Array(content) = result.get("content")? else {
            return None;
        };
        let texts = content
            .elements()
            .filter_map(|item| item.get("text"))
            .filter_map(Value::as_str)
            .map(|text| text.chars().collect())
            .collect();
        let png = content
            .elements()
            .filter(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.equals("image"))
            })
            .filter_map(|item| item.get("data")?.as_str())
            .find_map(|data| {
                let mut bytes = vec![0u8; MAX_IMAGE_BYTES + 3];
                let length = eggos_mcp::arguments::decode_base64(data, &mut bytes).ok()?;
                bytes.truncate(length);
                Some(bytes)
            });
        let scale = result
            .get("structuredContent")
            .and_then(|structured| structured.get("scale"))
            .and_then(|scale| std::str::from_utf8(scale.raw()).ok()?.parse().ok());
        Some(Self {
            texts,
            is_error: result.get("isError") == Some(Value::Bool(true)),
            png,
            scale,
        })
    }

    fn all(&self) -> String {
        self.texts.join("\n")
    }
}

fn error_code(answer: &str) -> Option<i64> {
    let error = json::parse(answer.as_bytes()).ok()?.value.get("error")?;
    std::str::from_utf8(error.get("code")?.raw())
        .ok()?
        .parse()
        .ok()
}

fn result_field(answer: &str, key: &str) -> Option<String> {
    let result = json::parse(answer.as_bytes()).ok()?.value.get("result")?;
    Some(result.get(key)?.as_str()?.chars().collect())
}

fn big_file() -> Vec<u8> {
    let mut text = Vec::with_capacity(BIG_FILE_BYTES);
    let mut line = 0;
    while text.len() < BIG_FILE_BYTES {
        text.extend_from_slice(format!("EGGOS-MCP line {line:06} of the large file\n").as_bytes());
        line += 1;
    }
    text.truncate(BIG_FILE_BYTES);
    text
}

fn prepare(
    arch: &Arch,
    kernel: &Path,
    programs: &Path,
    linux_gui_program: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let dir = crate::target_directory().join("mcp-test").join(arch.name);
    let script = crate::release::write_test_dist(arch, kernel, &dir)?;
    let apps = dir.join("apps");
    std::fs::create_dir_all(&apps).map_err(|e| format!("{}: {e}", apps.display()))?;
    std::fs::write(apps.join("readme.txt"), README).map_err(|e| e.to_string())?;
    std::fs::write(apps.join("big.txt"), big_file()).map_err(|e| e.to_string())?;
    for name in [
        super::install::WINDOWS_PROGRAM,
        super::install::LINUX_PROGRAM,
    ] {
        let from = programs.join(name);
        std::fs::copy(&from, apps.join(name)).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    std::fs::copy(linux_gui_program, apps.join(LINUX_GUI_PROGRAM))
        .map_err(|e| format!("{}: {e}", linux_gui_program.display()))?;
    let disk = dir.join("disk.img");
    reset_disk_image(&disk, TEST_DISK_BYTES)?;
    Ok((script, apps, disk))
}

fn both(session: Result<(), String>, console: Result<(), String>) -> Result<(), String> {
    match (session, console) {
        (Err(session), Err(console)) => Err(format!("{session}; and {console}")),
        (session, console) => session.and(console),
    }
}

fn shown_offscreen(arch: &Arch, script: &Path) -> Result<(), String> {
    let log = script.with_file_name(format!("eggos-{}-serial.log", arch.name));
    let console = std::fs::read(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let console = String::from_utf8_lossy(&console);
    let lines: Vec<&str> = console.lines().map(str::trim_end).collect();
    for line in [
        super::OFFSCREEN_LINE_1280X800,
        super::DISPLAY_LINE_1280X800,
        super::NO_INPUT_LINE,
    ] {
        expect(
            lines.contains(&line),
            arch.name,
            &format!("the console in {} has no `{line}`", log.display()),
        )?;
    }
    Ok(())
}

fn expect(holds: bool, arch: &str, what: &str) -> Result<(), String> {
    match holds {
        true => Ok(()),
        false => Err(format!("{arch}: MCP check: {what}")),
    }
}

fn session(client: &mut Client) -> Result<(), String> {
    let arch = client.arch;
    client.send(br#"{"jsonrpc":"2.0","id":"probe","method":"server/discover","params":{}}"#)?;
    let probe = client.receive(FIRST_ANSWER_TIMEOUT)?;
    expect(
        error_code(&probe) == Some(-32601),
        arch,
        "server/discover is not METHOD_NOT_FOUND",
    )?;
    let answer = client.request(
        "initialize",
        &format!(
            r#"{{"protocolVersion":"2099-01-01","capabilities":{{}},"clientInfo":{{"name":"{CLIENT_NAME}","version":"0"}}}}"#
        ),
    )?;
    expect(
        result_field(&answer, "protocolVersion").as_deref() == Some("2025-11-25"),
        arch,
        "an unknown version is not answered with 2025-11-25",
    )?;
    let answer = client.request(
        "initialize",
        &format!(
            r#"{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"{CLIENT_NAME}","version":"1"}}}}"#
        ),
    )?;
    expect(
        result_field(&answer, "protocolVersion").as_deref() == Some("2025-06-18"),
        arch,
        "the client's version is not answered",
    )?;
    client.send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)?;
    let answer = client.request("ping", "{}")?;
    expect(answer.contains(r#""result":{}"#), arch, "ping")?;
    let answer = client.request("tools/list", "{}")?;
    for tool in &eggos_mcp::tools::TOOLS {
        expect(
            answer.contains(&format!(r#""name":"{}""#, tool.name)),
            arch,
            &format!("tools/list lacks {}", tool.name),
        )?;
    }
    expect(
        answer.contains(r#""readOnlyHint":true"#),
        arch,
        "tools carry no annotations",
    )?;
    for (tool, arguments, wanted) in TWIN_CALLS {
        let result = client.call(tool, arguments)?;
        expect(
            !result.is_error,
            arch,
            &format!("{tool} failed: {}", result.all()),
        )?;
        expect(
            result
                .texts
                .first()
                .is_some_and(|notice| notice.ends_with(NOTICE_END)),
            arch,
            &format!("{tool}'s result does not start with the data notice"),
        )?;
        expect(
            result.all().contains(wanted),
            arch,
            &format!("{tool}'s output lacks `{wanted}`: {}", result.all()),
        )?;
    }
    clock_near(client, host_seconds(), "the first boot")?;
    client.paused.store(true, Ordering::Release);
    let id = client.next_id;
    client.next_id += 1;
    client.send(
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"cat","arguments":{{"path":"/media/host/big.txt"}}}}}}"#
        )
        .as_bytes(),
    )?;
    std::thread::sleep(SLOW_CLIENT_PAUSE);
    client.paused.store(false, Ordering::Release);
    let answer = client.receive(ANSWER_TIMEOUT)?;
    let result = ToolResult::of(&answer).ok_or_else(|| client.fail("the large read"))?;
    let expected = big_file();
    expect(
        result.texts.get(1).map(String::as_bytes) == Some(&expected[..OUTPUT_LIMIT]),
        arch,
        "the large read did not arrive byte for byte",
    )?;
    expect(
        result.all().contains("cut at 131072"),
        arch,
        "the large read is not marked cut",
    )?;
    for path in REFUSED_PATHS {
        let result = client.call("cat", &format!(r#"{{"path":{path}}}"#))?;
        expect(
            result.is_error && result.all().contains("outside /home and /media/host"),
            arch,
            &format!("{path} was not refused: {}", result.all()),
        )?;
    }
    let long_line = vec![b' '; 2 << 20];
    for hostile in [
        &b"{\"jsonrpc\":"[..],
        &long_line,
        br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
        br#"{"jsonrpc":"2.0","id":5,"id":6,"method":"ping"}"#,
    ] {
        let mut line = hostile.to_vec();
        if line.len() > 1000 {
            line[0] = b'x';
        }
        client.send(&line)?;
        let answer = client.receive(ANSWER_TIMEOUT)?;
        expect(
            error_code(&answer).is_some(),
            arch,
            "a hostile line got no error",
        )?;
    }
    let answer = client.request("ping", "{}")?;
    expect(
        answer.contains(r#""result":{}"#),
        arch,
        "ping after the hostile lines",
    )?;
    for (tool, arguments, grant) in GRANTED_TOOLS {
        let result = client.call(tool, arguments)?;
        expect(
            result.is_error
                && result.all().contains(&format!("needs the {grant} grant"))
                && result.all().contains(&format!("--mcp-allow {grant}")),
            arch,
            &format!(
                "{tool} without grants was not refused naming {grant}: {}",
                result.all()
            ),
        )?;
    }
    Ok(())
}

fn test_app(arch: &str) -> (&'static str, &'static str, Option<&'static str>) {
    match super::runs_windows_programs(arch) {
        true => (
            super::install::WINDOWS_PROGRAM,
            "win-paint",
            Some(eggos_windows_gui_tests::TITLE),
        ),
        false => (
            super::install::LINUX_PROGRAM,
            super::install::LINUX_PROGRAM,
            None,
        ),
    }
}

fn succeeds(
    client: &mut Client,
    tool: &str,
    arguments: &str,
    wanted: &str,
) -> Result<ToolResult, String> {
    let result = client.call(tool, arguments)?;
    expect(
        !result.is_error && result.all().contains(wanted),
        client.arch,
        &format!(
            "{tool} {arguments} did not give `{wanted}`: {}",
            result.all()
        ),
    )?;
    Ok(result)
}

fn refused(client: &mut Client, tool: &str, arguments: &str, wanted: &str) -> Result<(), String> {
    let result = client.call(tool, arguments)?;
    expect(
        result.is_error && result.all().contains(wanted),
        client.arch,
        &format!(
            "{tool} {arguments} was not refused with `{wanted}`: {}",
            result.all()
        ),
    )
}

const CLOCK_TOLERANCE_SECONDS: u64 = 5 * 60;
const CLOCK_AHEAD_SECONDS: u64 = 86_400;

fn iso_utc(seconds: u64) -> String {
    let mut text = String::new();
    let _ = eggos_calendar::format::iso(&mut text, seconds as i64);
    format!("{}Z", text.replace(' ', "T"))
}

fn clock_seconds(text: &str) -> Option<u64> {
    text.split_whitespace()
        .find_map(|word| eggos_calendar::parse_setting(word, 0).ok())
        .and_then(|seconds| u64::try_from(seconds).ok())
}

fn clock_near(client: &mut Client, expected: u64, at: &str) -> Result<(), String> {
    let result = client.call("clock_get", "{}")?;
    let read = clock_seconds(&result.all());
    expect(
        !result.is_error
            && read.is_some_and(|read| read.abs_diff(expected) <= CLOCK_TOLERANCE_SECONDS),
        client.arch,
        &format!(
            "at {at}, clock_get is not near {expected}: {}",
            result.all()
        ),
    )
}

fn set_clock(client: &mut Client) -> Result<(), String> {
    let ahead = host_seconds() + CLOCK_AHEAD_SECONDS;
    let time = iso_utc(ahead);
    refused(
        client,
        "clock_set",
        &format!(r#"{{"time":"{time}"}}"#),
        "confirm: true",
    )?;
    succeeds(
        client,
        "clock_set",
        &format!(r#"{{"time":"{time}","confirm":true}}"#),
        "UTC",
    )?;
    clock_near(client, ahead, "clock_set a day ahead")?;
    let now = iso_utc(host_seconds());
    succeeds(
        client,
        "clock_set",
        &format!(r#"{{"time":"{now}","confirm":true}}"#),
        "UTC",
    )?;
    clock_near(client, host_seconds(), "clock_set back")
}

fn granted_session(client: &mut Client) -> Result<(), String> {
    let arch = client.arch;
    let answer = client.request(
        "initialize",
        &format!(
            r#"{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"{CLIENT_NAME}","version":"2"}}}}"#
        ),
    )?;
    expect(
        answer.contains("Grants on this machine: read,view,write,install,run."),
        arch,
        &format!("initialize does not name the grants: {answer}"),
    )?;
    client.send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)?;
    set_clock(client)?;
    let first = format!(r#"{{"path":"{NOTE}","content":"{CONTENT_MARKER} first\n"}}"#);
    succeeds(client, "tee", &first, "Wrote")?;
    let second = format!(r#"{{"path":"{NOTE}","content":"{CONTENT_MARKER} second\n"}}"#);
    refused(client, "tee", &second, "confirm: true")?;
    let confirmed =
        format!(r#"{{"path":"{NOTE}","content":"{CONTENT_MARKER} second\n","confirm":true}}"#);
    succeeds(client, "tee", &confirmed, "Wrote")?;
    succeeds(
        client,
        "cat",
        &format!(r#"{{"path":"{NOTE}"}}"#),
        &format!("{CONTENT_MARKER} second\n"),
    )?;
    refused(
        client,
        "cat",
        r#"{"path":"/home/../etc/eggos/init.conf"}"#,
        "outside /home and /media/host",
    )?;
    for (path, wanted) in [
        ("/home/../etc/x", "outside /home/root"),
        ("/home/root", "names /home/root itself"),
        ("/home/other/x", "outside /home/root"),
        (
            "/home/root/.local/share/Trash/info/x.trashinfo",
            "in the Trash",
        ),
    ] {
        refused(
            client,
            "tee",
            &format!(r#"{{"path":"{path}","content":"x","confirm":true}}"#),
            wanted,
        )?;
    }
    refused(
        client,
        "trash",
        &format!(r#"{{"path":"{NOTE}"}}"#),
        "confirm: true",
    )?;
    succeeds(
        client,
        "trash",
        &format!(r#"{{"path":"{NOTE}","confirm":true}}"#),
        "to the Trash",
    )?;
    refused(
        client,
        "cat",
        &format!(r#"{{"path":"{NOTE}"}}"#),
        "No such file",
    )?;
    succeeds(
        client,
        "run",
        r#"{"program":"/bin/uname","arguments":["-a"]}"#,
        "EggOS",
    )?;
    succeeds(
        client,
        "run",
        r#"{"program":"/bin/cat","arguments":["/proc/cmdline"]}"#,
        &format!("{} {}", super::OFFSCREEN_WORD, super::grant_word(GRANTS)),
    )?;
    refused(client, "run", r#"{"program":"uname"}"#, "absolute path")?;
    let started = Instant::now();
    let result = client.call(
        "run",
        r#"{"program":"/bin/sleep","arguments":["30"],"timeout_seconds":1}"#,
    )?;
    expect(
        result.is_error
            && result
                .all()
                .contains("did not finish in time and was stopped")
            && !result.all().contains("could not be stopped")
            && started.elapsed() < TIME_LIMIT_ANSWER,
        arch,
        &format!(
            "sleep 30 with a 1 s limit was not stopped in time: {}",
            result.all()
        ),
    )?;
    let (file, name, title) = test_app(arch);
    succeeds(
        client,
        "app_install",
        &format!(r#"{{"path":"/media/host/{file}"}}"#),
        "Installed",
    )?;
    succeeds(client, "app_list", "{}", name)?;
    succeeds(
        client,
        "app_open",
        &format!(r#"{{"name":"{name}"}}"#),
        "Opened",
    )?;
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        let result = succeeds(client, "wm_list", "{}", WM_LIST_HEADER)?;
        let Some(title) = title else {
            break;
        };
        if result.all().contains(title) {
            break;
        }
        expect(
            Instant::now() < deadline,
            arch,
            &format!("{title}'s window never appeared: {}", result.all()),
        )?;
        std::thread::sleep(Duration::from_millis(500));
    }
    window_session(client, title)?;
    refused(
        client,
        "app_uninstall",
        &format!(r#"{{"name":"{name}"}}"#),
        "confirm: true",
    )?;
    succeeds(
        client,
        "app_uninstall",
        &format!(r#"{{"name":"{name}","confirm":true}}"#),
        "Uninstalled",
    )?;
    client.request(
        "initialize",
        &format!(
            r#"{{"protocolVersion":"2025-11-25","capabilities":{{"elicitation":{{}}}},"clientInfo":{{"name":"{CLIENT_NAME}","version":"3"}}}}"#
        ),
    )?;
    client.send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)?;
    succeeds(
        client,
        "tee",
        &format!(r#"{{"path":"{ELICITED}","content":"asked\n"}}"#),
        "Wrote",
    )?;
    let (call, question) = client.call_asked("trash", &format!(r#"{{"path":"{ELICITED}"}}"#))?;
    let result = client.answer_question(call, &question, r#"{"action":"decline"}"#)?;
    expect(
        result.is_error && result.all().contains("declined"),
        arch,
        &format!("a declined trash was not refused: {}", result.all()),
    )?;
    succeeds(
        client,
        "cat",
        &format!(r#"{{"path":"{ELICITED}"}}"#),
        "asked",
    )?;
    let (call, question) = client.call_asked("trash", &format!(r#"{{"path":"{ELICITED}"}}"#))?;
    let result = client.answer_question(
        call,
        &question,
        r#"{"action":"accept","content":{"confirm":true}}"#,
    )?;
    expect(
        !result.is_error && result.all().contains("to the Trash"),
        arch,
        &format!("an accepted trash did not run: {}", result.all()),
    )?;
    let log = succeeds(client, "run", r#"{"program":"/bin/agentlog"}"#, "TIME")?;
    let log = log.all();
    for wanted in [
        "  tee  refused  ".to_owned(),
        "  run  refused  ".to_owned(),
        "  app_install  refused  ".to_owned(),
        "  tee  ok  ".to_owned(),
        "  trash  ok  ".to_owned(),
        "  run  ok  ".to_owned(),
        "  run  error  ".to_owned(),
        "  app_install  ok  ".to_owned(),
        "  app_open  ok  ".to_owned(),
        "  app_uninstall  ok  ".to_owned(),
        "  wm_list  ok  ".to_owned(),
        "  wm_capture  refused  ".to_owned(),
        "  wm_click  refused  ".to_owned(),
        "  wm_capture  ok  {\"window\":".to_owned(),
        "  wm_capture  error  ".to_owned(),
        "  wm_click  ok  ".to_owned(),
        "  wm_type  ok  ".to_owned(),
        "  wm_press  ok  ".to_owned(),
        "  wm_scroll  ok  ".to_owned(),
        "typed 1 of 1".to_owned(),
        "the user declined".to_owned(),
        "  cat  ok  {\"path\":\"/media/host/big.txt\"}".to_owned(),
        "server-start".to_owned(),
        "grants=read,view,write,install,run".to_owned(),
    ] {
        expect(
            log.contains(&wanted),
            arch,
            &format!("agentlog does not show `{wanted}`: {log}"),
        )?;
    }
    expect(
        !log.contains(CONTENT_MARKER),
        arch,
        "agentlog shows content a call wrote",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Image {
    fn decoded(png: &[u8]) -> Option<Self> {
        let width = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);
        let mut inflated = vec![0u8; eggos_png::decoded_size(width, height)?];
        let mut pixels = vec![0u32; (width as usize).checked_mul(height as usize)?];
        eggos_png::decode(png, &mut inflated, &mut pixels).ok()?;
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    fn of(content: WindowContent) -> Option<Self> {
        let WindowContent::Pixels { width, pixels } = content else {
            return None;
        };
        let height = u32::try_from(pixels.len()).ok()? / width.max(1);
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    fn shrunk(&self, factor: u32) -> Option<Self> {
        let mut sums = vec![0u64; 3 * self.width as usize];
        let mut downscale = Downscale::new(self.width, self.height, factor, &mut sums)?;
        let width = downscale.output_width();
        let mut row = vec![0u32; width as usize];
        let mut pixels = Vec::new();
        for source in self.pixels.chunks(self.width as usize) {
            if downscale.push_row(source, &mut row) {
                pixels.extend_from_slice(&row);
            }
        }
        let height = u32::try_from(pixels.len()).ok()? / width.max(1);
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    fn differing(&self, other: &Self) -> usize {
        match (self.width, self.height) == (other.width, other.height) {
            true => self
                .pixels
                .iter()
                .zip(&other.pixels)
                .filter(|(a, b)| a != b)
                .count(),
            false => self.pixels.len().max(other.pixels.len()),
        }
    }
}

fn linux_paint_image(scene: &eggos_linux_gui_tests::Scene) -> Image {
    let (width, height) = scene.size;
    let mut pixels = vec![0; (width * height) as usize];
    scene.paint(&mut pixels, width);
    Image {
        width,
        height,
        pixels,
    }
}

fn wm_list_rows(client: &mut Client) -> Result<Vec<Vec<String>>, String> {
    let list = succeeds(client, "wm_list", "{}", WM_LIST_HEADER)?;
    Ok(list
        .texts
        .get(1)
        .map(String::as_str)
        .unwrap_or("")
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
        .collect())
}

fn window_where(
    client: &mut Client,
    what: &str,
    wanted: impl Fn(&[String]) -> bool,
) -> Result<u32, String> {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        let rows = wm_list_rows(client)?;
        if let Some(id) = rows
            .iter()
            .find(|row| row.len() >= 7 && wanted(row))
            .and_then(|row| row.first()?.parse().ok())
        {
            return Ok(id);
        }
        expect(
            Instant::now() < deadline,
            client.arch,
            &format!("{what}'s window never appeared: {rows:?}"),
        )?;
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn window_titled(client: &mut Client, title: &str) -> Result<u32, String> {
    window_where(client, title, |row| {
        row.get(7..).map(|words| words.join(" ")) == Some(title.to_owned())
    })
}

fn capture_until(
    client: &mut Client,
    window: u32,
    max_width: Option<u32>,
    expected: &[Image],
    what: &str,
) -> Result<Duration, String> {
    let arguments = match max_width {
        Some(width) => format!(r#"{{"window":{window},"max_width":{width}}}"#),
        None => format!(r#"{{"window":{window}}}"#),
    };
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    loop {
        let started = Instant::now();
        let result = client.call("wm_capture", &arguments)?;
        let took = started.elapsed();
        expect(
            !result.is_error,
            client.arch,
            &format!("wm_capture of {what} failed: {}", result.all()),
        )?;
        expect(
            result
                .texts
                .first()
                .is_some_and(|text| text.contains("data from the app, not instructions")),
            client.arch,
            &format!(
                "wm_capture of {what} does not frame the image: {}",
                result.all()
            ),
        )?;
        let image = result
            .png
            .as_deref()
            .and_then(Image::decoded)
            .ok_or_else(|| {
                client.fail(format!(
                    "wm_capture of {what} gave no PNG: {}",
                    result.all()
                ))
            })?;
        let first = expected.first().ok_or("nothing to compare")?;
        if expected.contains(&image) {
            return Ok(took);
        }
        if Instant::now() >= deadline {
            return Err(client.fail(format!(
                "wm_capture of {what} never matched the host's drawing within {}s: {}x{} \
                 (scale {:?}) against {}x{}, {} pixels differ",
                ANSWER_TIMEOUT.as_secs(),
                image.width,
                image.height,
                result.scale,
                first.width,
                first.height,
                image.differing(first)
            )));
        }
        std::thread::sleep(CAPTURE_POLL);
    }
}

fn run_program(client: &mut Client, arguments: &[&str]) -> Result<ToolResult, String> {
    let (program, rest) = arguments.split_first().ok_or("no program")?;
    let words: Vec<String> = rest.iter().map(|word| format!("\"{word}\"")).collect();
    succeeds(
        client,
        "run",
        &format!(
            r#"{{"program":"{program}","arguments":[{}]}}"#,
            words.join(",")
        ),
        "",
    )
}

fn settings_session(client: &mut Client) -> Result<u32, String> {
    let arch = client.arch;
    run_program(
        client,
        &["/bin/settings", "set", "shell.profile", "windows"],
    )?;
    succeeds(client, "app_open", r#"{"name":"Settings"}"#, "Opened")?;
    let settings = window_titled(client, eggos_settings::settings_window::TITLE)?;
    let facts = Facts {
        on_disk: true,
        profile: Profile::Windows.number() as usize,
        ..Facts::example()
    };
    let (width, height) = DEFAULT_SIZE;
    let expected: Vec<Image> = settings_window_images(&facts)
        .into_iter()
        .filter_map(|pixels| {
            Image {
                width,
                height,
                pixels,
            }
            .shrunk(SETTINGS_SCALE)
        })
        .collect();
    let took = capture_until(
        client,
        settings,
        Some(width / SETTINGS_SCALE),
        &expected,
        "Settings at scale 1/2",
    )?;
    eprintln!(
        "xtask: {arch}: MCP check: wm_capture of Settings ({width}x{height} at scale 1/{SETTINGS_SCALE}) took {} ms",
        took.as_millis()
    );
    let card = SettingsWindow::new(facts)
        .card_rect(Profile::Macos.number() as usize)
        .ok_or_else(|| client.fail("Settings has no macOS card"))?;
    succeeds(
        client,
        "wm_click",
        &format!(
            r#"{{"window":{settings},"x":{},"y":{}}}"#,
            card.x + card.width / 2,
            card.y + 20
        ),
        "clicked left",
    )?;
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    loop {
        let profile = run_program(client, &["/bin/settings", "get", "shell.profile"])?;
        if profile.all().contains("macos") {
            return Ok(settings);
        }
        expect(
            Instant::now() < deadline,
            arch,
            &format!(
                "the click on Settings' macOS card changed nothing: {}",
                profile.all()
            ),
        )?;
        std::thread::sleep(CAPTURE_POLL);
    }
}

fn win_paint_session(client: &mut Client, title: &str) -> Result<(), String> {
    let paint = window_titled(client, title)?;
    let mut scene = eggos_windows_gui_tests::Scene::new();
    let drawn = |scene: &eggos_windows_gui_tests::Scene| {
        Image::of(super::windows_gui::scene_content(scene))
            .into_iter()
            .collect::<Vec<_>>()
    };
    capture_until(client, paint, None, &drawn(&scene), "win-paint")?;
    run_program(client, &["/bin/wm", "minimise", &paint.to_string()])?;
    succeeds(
        client,
        "wm_type",
        &format!(r#"{{"window":{paint},"text":"ab"}}"#),
        "typed 2 of 2",
    )?;
    scene.type_character(u32::from(b'a'));
    scene.type_character(u32::from(b'b'));
    capture_until(
        client,
        paint,
        None,
        &drawn(&scene),
        "win-paint minimised, after typing ab",
    )?;
    run_program(client, &["/bin/wm", "close", &paint.to_string()])?;
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    while wm_list_rows(client)?
        .iter()
        .any(|row| row.first() == Some(&paint.to_string()))
    {
        expect(
            Instant::now() < deadline,
            client.arch,
            "win-paint did not end when its window was closed",
        )?;
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn linux_paint_session(client: &mut Client, settings: u32) -> Result<u32, String> {
    let arch = client.arch;
    succeeds(
        client,
        "app_install",
        &format!(r#"{{"path":"/media/host/{LINUX_GUI_PROGRAM}"}}"#),
        "Installed",
    )?;
    succeeds(
        client,
        "app_open",
        &format!(r#"{{"name":"{LINUX_GUI_PROGRAM}"}}"#),
        "Opened",
    )?;
    let paint = window_titled(client, eggos_linux_gui_tests::TITLE)?;
    let mut scene = eggos_linux_gui_tests::Scene::new();
    let took = capture_until(
        client,
        paint,
        None,
        &[linux_paint_image(&scene)],
        "linux-paint",
    )?;
    eprintln!(
        "xtask: {arch}: MCP check: wm_capture of linux-paint ({}x{} at scale 1/1) took {} ms",
        scene.size.0,
        scene.size.1,
        took.as_millis()
    );
    run_program(client, &["/bin/wm", "focus", &settings.to_string()])?;
    let (x, y, width, height) = eggos_linux_gui_tests::TARGET;
    succeeds(
        client,
        "wm_click",
        &format!(
            r#"{{"window":{paint},"x":{},"y":{}}}"#,
            x + width / 2,
            y + height / 2
        ),
        "clicked left",
    )?;
    scene.clicked = true;
    capture_until(
        client,
        paint,
        None,
        &[linux_paint_image(&scene)],
        "linux-paint under Settings, after a click on its target",
    )?;
    succeeds(
        client,
        "wm_type",
        &format!(r#"{{"window":{paint},"text":"a"}}"#),
        "typed 1 of 1",
    )?;
    scene.typed = 1;
    capture_until(
        client,
        paint,
        None,
        &[linux_paint_image(&scene)],
        "linux-paint under Settings, after typing a",
    )?;
    succeeds(
        client,
        "wm_scroll",
        &format!(r#"{{"window":{paint},"dy":-1}}"#),
        "scrolled",
    )?;
    Ok(paint)
}

fn window_session(client: &mut Client, paint_title: Option<&str>) -> Result<(), String> {
    let settings = settings_session(client)?;
    if let Some(title) = paint_title {
        win_paint_session(client, title)?;
    }
    linux_paint_session(client, settings)?;
    let terminal = window_where(client, "the terminal", |row| {
        row.get(6).map(String::as_str) == Some("kernel")
    })?;
    refused(
        client,
        "wm_capture",
        &format!(r#"{{"window":{terminal}}}"#),
        "drawn by the kernel",
    )?;
    succeeds(
        client,
        "wm_press",
        &format!(r#"{{"window":{terminal},"keys":"shift"}}"#),
        "pressed shift",
    )?;
    succeeds(
        client,
        "edit_open",
        r#"{"path":"/media/host/readme.txt"}"#,
        "Opened /media/host/readme.txt in Text Editor.",
    )?;
    window_titled(client, "readme.txt - Text Editor")?;
    Ok(())
}

/// Runs the MCP boot test for `arch` with the release image `kernel` (with the Windows layer),
/// the folder `programs` holding the test apps and `linux_gui_program`, `linux-paint` built
/// for `arch`.
pub fn check(
    arch: &Arch,
    kernel: &Path,
    programs: &Path,
    linux_gui_program: &Path,
) -> Result<(), String> {
    let (script, apps, disk) = prepare(arch, kernel, programs, linux_gui_program)?;
    eprintln!("xtask: {}: running {} --mcp", arch.name, script.display());
    let mut client = Client::start(arch, &script, &apps, &disk, None)?;
    let outcome = session(&mut client);
    client.stop();
    both(outcome, shown_offscreen(arch, &script))?;
    eprintln!(
        "xtask: {}: running {} --mcp --mcp-allow {GRANTS}",
        arch.name,
        script.display()
    );
    let mut client = Client::start(arch, &script, &apps, &disk, Some(GRANTS))?;
    let probe = client
        .send(br#"{"jsonrpc":"2.0","id":"probe","method":"ping"}"#)
        .and_then(|()| client.receive(FIRST_ANSWER_TIMEOUT));
    let outcome = probe.and_then(|_| granted_session(&mut client));
    client.stop();
    both(outcome, shown_offscreen(arch, &script))?;
    let mut volume = image_volume(&disk)?;
    let log = contents(&mut volume, "/var/log/agent-audit.log")?
        .ok_or_else(|| format!("{}: MCP check: the disk holds no audit log", arch.name))?;
    let records: Vec<eggos_mcp::audit::Record<'_>> = log
        .split(|&byte| byte == b'\n')
        .filter_map(eggos_mcp::audit::parse_line)
        .collect();
    expect(
        records.len() >= 60,
        arch.name,
        "the audit log on the disk is missing records",
    )?;
    expect(
        records
            .iter()
            .any(|record| record.outcome == eggos_mcp::audit::Outcome::Asked),
        arch.name,
        "the audit log on the disk holds no question to the user",
    )?;
    expect(
        !log.windows(CONTENT_MARKER.len())
            .any(|window| window == CONTENT_MARKER.as_bytes()),
        arch.name,
        "the audit log on the disk holds content a call wrote",
    )?;
    eprintln!("xtask: {}: MCP boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_session_keeps_what_the_console_showed() {
        let failed = || Err::<(), String>("x".into());
        assert_eq!(both(Ok(()), Ok(())), Ok(()));
        assert_eq!(both(failed(), Ok(())), failed());
        assert_eq!(both(Ok(()), failed()), failed());
        assert_eq!(both(failed(), Err("y".into())), Err("x; and y".into()));
    }

    #[test]
    fn every_tool_is_called_by_the_check() {
        for tool in &eggos_mcp::tools::TOOLS {
            let read = TWIN_CALLS.iter().any(|(name, _, _)| *name == tool.name);
            let granted = GRANTED_TOOLS.iter().find(|(name, _, _)| *name == tool.name);
            assert!(
                read || granted.is_some(),
                "{} has no call in the MCP check",
                tool.name
            );
            assert_eq!(read, tool.grant.is_none(), "{}", tool.name);
            if let (Some((_, _, grant)), Some(needed)) = (granted, tool.grant) {
                assert_eq!(*grant, needed.name(), "{}", tool.name);
            }
        }
    }

    #[test]
    fn the_granted_boot_asks_for_every_grant() {
        assert_eq!(super::super::check_grants(GRANTS), Ok(()));
        for grant in super::super::GRANT_NAMES {
            assert!(GRANTS.split(',').any(|name| name == grant));
        }
    }

    #[test]
    fn captures_compare_with_the_host_s_drawing_at_their_scale() {
        let facts = Facts {
            on_disk: true,
            profile: Profile::Windows.number() as usize,
            ..Facts::example()
        };
        let (width, height) = DEFAULT_SIZE;
        let [shown, hidden] = settings_window_images(&facts);
        let shrunk = Image {
            width,
            height,
            pixels: shown,
        }
        .shrunk(SETTINGS_SCALE)
        .unwrap();
        assert_eq!((shrunk.width, shrunk.height), (width / 2, height / 2));
        assert_eq!(
            eggos_png::scale::factor_for(width, width / SETTINGS_SCALE),
            SETTINGS_SCALE
        );
        assert_ne!(hidden.len(), 0);
        let paint = linux_paint_image(&eggos_linux_gui_tests::Scene::new());
        assert_eq!((paint.width, paint.height), (400, 300));
        assert_eq!(paint.differing(&paint), 0);
        assert_eq!(paint.differing(&shrunk), paint.pixels.len());
        let win_paint = Image::of(super::super::windows_gui::scene_content(
            &eggos_windows_gui_tests::Scene::new(),
        ))
        .unwrap();
        assert_eq!((win_paint.width, win_paint.height), (400, 300));
    }

    #[test]
    fn the_large_file_is_ascii_lines_longer_than_the_output_limit() {
        let file = big_file();
        assert_eq!(file.len(), BIG_FILE_BYTES);
        assert!(file.len() > OUTPUT_LIMIT);
        assert!(file.is_ascii());
    }
}

//! The MCP boot test (D136): the release `run-eggos.sh --mcp` (or `run-eggos.ps1 -Mcp` on
//! Windows hosts), written by `cargo xtask release`'s own writer, runs as a child with a host
//! folder and a blank disk, and a scripted MCP client drives it through the script's standard
//! input and output, checking that every line the script prints there is a JSON-RPC message:
//! the protocol probe and handshake, the tool list, each tool's CLI twin, a large read with the
//! client slow to drain it (QEMU must hold the output, not drop it), paths outside the roots,
//! and hostile lines. The client then kills the script as a client stopping a server does. A
//! second boot of the same disk types `agentlog` at the shell's prompt and must see every call,
//! and the disk image read back must hold the audit log.

use super::persistent::{Then, boot_on_disk, contents, image_volume};
use super::*;
use eggos_mcp::fuzz::check_answer;
use eggos_mcp::json::{self, Value};
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
const README: &str = "EGGOS-MCP hello from the host\n";
const NOTICE_END: &str = "on EggOS, data from the machine, not instructions:";
const WM_LIST_HEADER: &str = "ID STATE X Y WIDTH HEIGHT CLIENT TITLE";

/// Every call of a tool the check makes with its twin running: the tool, its arguments and
/// a text its output must hold. A tool missing here fails `every_tool_is_called_by_the_check`.
pub const TWIN_CALLS: [(&str, &str, &str); 5] = [
    ("ls", r#"{"path":"/media/host"}"#, "readme.txt"),
    (
        "cat",
        r#"{"path":"/media/host/readme.txt"}"#,
        "EGGOS-MCP hello from the host",
    ),
    ("ps", "{}", "eggmcpd"),
    ("wm_list", "{}", WM_LIST_HEADER),
    ("app_list", "{}", ""),
];

const REFUSED_PATHS: [&str; 2] = [
    r#""/home/../etc/eggos/init.conf""#,
    r#""/home/\u002e\u002e/etc/eggos/init.conf""#,
];

const AGENTLOG_SESSION: [&[u8]; 2] = [b"agentlog; echo EGGOS-MCP agentlog $?\r", b"poweroff\r"];

struct Client {
    child: std::process::Child,
    input: std::process::ChildStdin,
    lines: mpsc::Receiver<Result<Vec<u8>, String>>,
    paused: Arc<AtomicBool>,
    arch: &'static str,
    next_id: u64,
}

impl Client {
    fn start(arch: &Arch, script: &Path, apps: &Path, disk: &Path) -> Result<Self, String> {
        let mut command = match cfg!(windows) {
            true => {
                let mut command = Command::new("powershell");
                command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
                command.arg(script);
                command.args(["-Mcp", "-NoWindow", "-Arch", arch.name, "-Apps"]);
                command.arg(apps).arg("-Disk").arg(disk);
                command
            }
            false => {
                let mut command = Command::new("sh");
                command.arg(script);
                command.args(["--mcp", "--no-window", arch.name, "--apps"]);
                command.arg(apps).arg("--disk").arg(disk);
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
        check_answer(line).map_err(|fault| {
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

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ToolResult {
    texts: Vec<String>,
    is_error: bool,
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
        Some(Self {
            texts,
            is_error: result.get("isError") == Some(Value::Bool(true)),
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

fn prepare(arch: &Arch, kernel: &Path) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let dir = crate::target_directory().join("mcp-test").join(arch.name);
    let script = crate::release::write_test_dist(arch, kernel, &dir)?;
    let apps = dir.join("apps");
    std::fs::create_dir_all(&apps).map_err(|e| format!("{}: {e}", apps.display()))?;
    std::fs::write(apps.join("readme.txt"), README).map_err(|e| e.to_string())?;
    std::fs::write(apps.join("big.txt"), big_file()).map_err(|e| e.to_string())?;
    let disk = dir.join("disk.img");
    reset_disk_image(&disk, TEST_DISK_BYTES)?;
    Ok((script, apps, disk))
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
    )
}

/// Runs the MCP boot test for `arch` with the release image `kernel`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let (script, apps, disk) = prepare(arch, kernel)?;
    eprintln!("xtask: {}: running {} --mcp", arch.name, script.display());
    let mut client = Client::start(arch, &script, &apps, &disk)?;
    let outcome = session(&mut client);
    client.stop();
    outcome?;
    let disk_args = disk_args(arch, &disk.to_string_lossy());
    let second = boot_on_disk(arch, kernel, &disk_args, &AGENTLOG_SESSION, Then::Nothing)?;
    let rows: Vec<&str> = second
        .console
        .lines()
        .map(str::trim_end)
        .filter(|line| line.contains(&format!("  {CLIENT_NAME}  ")))
        .collect();
    let mut wanted: Vec<String> = TWIN_CALLS
        .iter()
        .map(|(tool, _, _)| format!("  {tool}  ok  "))
        .collect();
    wanted.push("  cat  ok  {\"path\":\"/media/host/big.txt\"}".into());
    wanted.push("  cat  refused  {\"path\":\"/home/../etc/eggos/init.conf\"}".into());
    wanted.push("  -  initialize  ".into());
    for wanted in &wanted {
        expect(
            rows.iter().any(|row| row.contains(wanted.as_str())),
            arch.name,
            &format!("agentlog does not show `{wanted}`"),
        )?;
    }
    let refused = rows
        .iter()
        .filter(|row| row.contains("  cat  refused  "))
        .count();
    expect(
        refused == 2,
        arch.name,
        "agentlog does not show both refused reads",
    )?;
    expect(
        has_line(&second.console, "EGGOS-MCP agentlog 0") && second.clean_exit,
        arch.name,
        "agentlog failed or the machine did not power off",
    )?;
    let mut volume = image_volume(&disk)?;
    let log = contents(&mut volume, "/var/log/agent-audit.log")?
        .ok_or_else(|| format!("{}: MCP check: the disk holds no audit log", arch.name))?;
    let records = log
        .split(|&byte| byte == b'\n')
        .filter_map(eggos_mcp::audit::parse_line)
        .count();
    expect(
        records >= 15,
        arch.name,
        "the audit log on the disk is missing records",
    )?;
    eprintln!("xtask: {}: MCP boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_called_by_the_check() {
        for tool in &eggos_mcp::tools::TOOLS {
            assert!(
                TWIN_CALLS.iter().any(|(name, _, _)| *name == tool.name),
                "{} has no call in the MCP check",
                tool.name
            );
        }
    }

    #[test]
    fn the_large_file_is_ascii_lines_longer_than_the_output_limit() {
        let file = big_file();
        assert_eq!(file.len(), BIG_FILE_BYTES);
        assert!(file.len() > OUTPUT_LIMIT);
        assert!(file.is_ascii());
    }
}

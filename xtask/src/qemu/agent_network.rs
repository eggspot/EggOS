//! The network agents boot test (D193, slice 41d): two boots of the release kernel on a disk
//! image in `target/qemu-disks`, with QEMU's user networking forwarding a host port to EggOS's
//! 8443.
//!
//! 1. `agent network on`, then power off: the file that turns the server on is on the disk.
//! 2. Init starts `eggmcpd --network`; the session prints the server's certificate and `agent
//!    pair --name Laptop laptop`'s code. This test, as the agent's client, connects with our
//!    own TLS client pinning that certificate, trades the code for the token at `/pair`, then
//!    sends `initialize` and `tools/list` to `/mcp` with the token, and a request with another
//!    token, which must be refused with 401. `agentlog --agent laptop` must show the agent's
//!    session; the machine then powers off.

use super::*;
use eggos_tls::chain::{NoRoots, TrustAnchors};
use eggos_tls::client::{self, Buffers, Settings, Stopped, Transport};
use eggos_tls::pem::Certificates;
use std::io::{Read, Write};
use std::net::TcpStream;

const TEST_DISK_BYTES: u64 = 64 << 20;
const AGENT_NETWORK_TIMEOUT: Duration = Duration::from_secs(180);
const SERVER_PORT: u16 = 8443;
const STARTED_LINE: &str = "init: started the MCP server for agents on the network";
const TURNED_ON_LINE: &str = "agents on the network can connect on port 8443";
const PAIR_COMMAND: &str =
    "cat /var/lib/eggos/agent-network.pem; agent pair --name Laptop laptop\r";
const CODE_AFTER: &str = "8443/pair";
const FIRST_SESSION: [&str; 2] = ["agent network on\r", "poweroff\r"];
const LOG_COMMAND: &str = "agentlog --agent laptop; echo EGGOS-AGENT-NETWORK log $?\r";
const LOG_DONE: &str = "EGGOS-AGENT-NETWORK log 0";
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"elicitation":{}},"clientInfo":{"name":"xtask","version":"1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

struct Pinned(Vec<u8>);

impl TrustAnchors for Pinned {
    fn each(&mut self, visit: &mut dyn FnMut(&[u8]) -> bool) -> Result<bool, NoRoots> {
        Ok(visit(&self.0))
    }
}

struct Stream(TcpStream);

impl Transport for Stream {
    fn send(&mut self, bytes: &[u8]) -> Result<(), Stopped> {
        self.0.write_all(bytes).map_err(|_| Stopped)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Stopped> {
        self.0.read(buffer).map_err(|_| Stopped)
    }
}

/// An HTTP answer: its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The status code.
    pub status: u16,
    /// The body.
    pub body: String,
}

/// The certificate in the console's PEM block, DER.
pub fn certificate_from(console: &str) -> Option<Vec<u8>> {
    let mut der = vec![0u8; 4096];
    let mut decoder = Certificates::new(&mut der);
    let mut found = None;
    let mut visit = |certificate: &[u8]| {
        found = Some(certificate.to_vec());
        true
    };
    let text = console.replace('\r', "");
    decoder.feed(text.as_bytes(), &mut visit);
    found
}

/// The pairing code `agent pair` printed on the console.
pub fn code_from(console: &str) -> Option<String> {
    let at = console.find(CODE_AFTER)? + CODE_AFTER.len();
    let code: String = console[at..]
        .trim_start_matches(['\r', '\n', ' '])
        .chars()
        .take(9)
        .collect();
    (code.len() == 9 && code.as_bytes()[4] == b'-').then_some(code)
}

/// The value of `"key":"..."` in a JSON answer.
pub fn json_text(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = body.find(&needle)? + needle.len();
    let end = body[start..].find('"')? + start;
    Some(body[start..end].to_owned())
}

fn request(
    port: u16,
    certificate: &[u8],
    path: &str,
    token: Option<&str>,
    body: &str,
) -> Result<Answer, String> {
    let socket = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    let mut stream = Stream(socket);
    let mut buffers = Box::new(Buffers::new());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs() as i64;
    let settings = Settings {
        host: b"eggos.local",
        now,
    };
    let mut randomness = [0u8; client::RANDOMNESS_BYTES];
    for (index, byte) in randomness.iter_mut().enumerate() {
        *byte = (now as u8).wrapping_add(index as u8).wrapping_mul(31);
    }
    let mut session = client::connect(
        &mut stream,
        &mut buffers,
        &settings,
        &randomness,
        &mut Pinned(certificate.to_vec()),
    )
    .map_err(|error| format!("TLS: {}", error.description()))?;
    let authorization = token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let message = format!(
        "POST {path} HTTP/1.1\r\nHost: eggos.local:{SERVER_PORT}\r\n{authorization}\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    session
        .write(&mut stream, message.as_bytes())
        .map_err(|error| format!("sending: {}", error.description()))?;
    let mut received = Vec::new();
    let mut piece = [0u8; 4096];
    loop {
        match session.read(&mut stream, &mut piece) {
            Ok(0) => break,
            Ok(count) => received.extend_from_slice(&piece[..count]),
            Err(error) => return Err(format!("reading: {}", error.description())),
        }
    }
    parse_answer(&received)
}

/// Reads an HTTP answer: status line, headers and the body after them.
pub fn parse_answer(received: &[u8]) -> Result<Answer, String> {
    let text = String::from_utf8_lossy(received);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("no end of the headers in {text:?}"))?;
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("no status in {head:?}"))?;
    Ok(Answer {
        status,
        body: body.to_owned(),
    })
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|e| e.to_string())
}

fn forwarding_network_args(arch: &Arch, host_port: u16) -> Vec<String> {
    vec![
        "-netdev".into(),
        format!("user,id=net0,hostfwd=tcp:127.0.0.1:{host_port}-:{SERVER_PORT}"),
        "-device".into(),
        format!("{},netdev=net0", arch.network_device),
    ]
}

struct Machine {
    child: std::process::Child,
    keyboard: std::process::ChildStdin,
    output: mpsc::Receiver<Vec<u8>>,
    console: Vec<u8>,
    started: Instant,
    arch: &'static str,
}

impl Machine {
    fn boot(
        arch: &Arch,
        kernel: &Path,
        image: &Path,
        network: Vec<String>,
    ) -> Result<Self, String> {
        let mut child = Command::new(arch.qemu)
            .args(interactive_args(arch, false))
            .args(disk_args(arch, &image.display().to_string()))
            .args(network)
            .arg("-kernel")
            .arg(kernel)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {}: {e} (is QEMU installed?)", arch.qemu))?;
        let output = echo_console_to_stderr(child.stdout.take().expect("stdout is piped"));
        let keyboard = child.stdin.take().expect("stdin is piped");
        Ok(Self {
            child,
            keyboard,
            output,
            console: Vec::new(),
            started: Instant::now(),
            arch: arch.name,
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.console).into_owned()
    }

    fn wait_for(&mut self, wanted: &dyn Fn(&str) -> bool, what: &str) -> Result<(), String> {
        loop {
            self.console.extend(self.output.try_iter().flatten());
            if wanted(&self.text()) {
                return Ok(());
            }
            if self.started.elapsed() > AGENT_NETWORK_TIMEOUT {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!(
                    "{}: the network agents test never saw {what}",
                    self.arch
                ));
            }
            if let Ok(Some(_)) = self.child.try_wait() {
                return Err(format!("{}: QEMU ended before {what}", self.arch));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_prompt(&mut self, count: usize) -> Result<(), String> {
        self.wait_for(
            &|console: &str| console.matches(SHELL_PROMPT).count() >= count,
            "a shell prompt",
        )
    }

    fn type_line(&mut self, line: &str) -> Result<(), String> {
        self.keyboard
            .write_all(line.as_bytes())
            .and_then(|()| self.keyboard.flush())
            .map_err(|e| format!("{}: could not type on the console: {e}", self.arch))
    }

    fn power_off(mut self) -> Result<String, String> {
        let status = loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if self.started.elapsed() > AGENT_NETWORK_TIMEOUT {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!("{}: QEMU did not power off", self.arch));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.console.extend(self.output.iter().flatten());
        match status.success() {
            true => Ok(self.text()),
            false => Err(format!("{}: QEMU did not exit cleanly", self.arch)),
        }
    }
}

/// Runs the test on `arch` with the release `kernel`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    eprintln!(
        "xtask: booting {} in {} (network agents)",
        arch.name, arch.qemu
    );
    let directory = crate::target_directory().join("qemu-disks");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let image = directory.join(format!("{}-agent-network.img", arch.name));
    reset_disk_image(&image, TEST_DISK_BYTES)?;

    let mut first = Machine::boot(arch, kernel, &image, network_args(arch))?;
    for (index, line) in FIRST_SESSION.iter().enumerate() {
        first.wait_for_prompt(index + 1)?;
        first.type_line(line)?;
    }
    let console = first.power_off()?;
    if !console.contains(TURNED_ON_LINE) {
        return Err(format!(
            "{}: `agent network on` did not say `{TURNED_ON_LINE}`",
            arch.name
        ));
    }

    let port = free_port()?;
    let mut second = Machine::boot(arch, kernel, &image, forwarding_network_args(arch, port))?;
    second.wait_for(
        &|console: &str| console.contains(STARTED_LINE),
        STARTED_LINE,
    )?;
    second.wait_for_prompt(1)?;
    second.wait_for(
        &|console: &str| console.contains("net: eth0") && console.contains("10.0.2.15"),
        "the DHCP lease",
    )?;
    second.type_line(PAIR_COMMAND)?;
    second.wait_for(
        &|console: &str| code_from(console).is_some(),
        "the pairing code",
    )?;
    let console = second.text();
    let certificate = certificate_from(&console)
        .ok_or_else(|| format!("{}: no certificate on the console", arch.name))?;
    let code = code_from(&console).unwrap_or_default();

    let paired = request(
        port,
        &certificate,
        "/pair",
        None,
        &format!("{{\"code\":\"{code}\"}}"),
    )
    .map_err(|e| format!("{}: pairing: {e}", arch.name))?;
    if paired.status != 200 {
        return Err(format!("{}: pairing answered {paired:?}", arch.name));
    }
    let token = json_text(&paired.body, "token")
        .ok_or_else(|| format!("{}: no token in {paired:?}", arch.name))?;
    if json_text(&paired.body, "agent").as_deref() != Some("laptop") {
        return Err(format!(
            "{}: pairing named another agent: {paired:?}",
            arch.name
        ));
    }
    let again = request(
        port,
        &certificate,
        "/pair",
        None,
        &format!("{{\"code\":\"{code}\"}}"),
    )
    .map_err(|e| format!("{}: pairing again: {e}", arch.name))?;
    if again.status != 404 {
        return Err(format!("{}: a used code answered {again:?}", arch.name));
    }

    let initialized = request(port, &certificate, "/mcp", Some(&token), INITIALIZE)
        .map_err(|e| format!("{}: initialize: {e}", arch.name))?;
    if initialized.status != 200
        || !initialized.body.contains("\"serverInfo\"")
        || initialized.body.contains("\"elicitation\":true")
    {
        return Err(format!(
            "{}: initialize answered {initialized:?}",
            arch.name
        ));
    }
    let notified = request(port, &certificate, "/mcp", Some(&token), INITIALIZED)
        .map_err(|e| format!("{}: initialized: {e}", arch.name))?;
    if notified.status != 202 {
        return Err(format!(
            "{}: a notification answered {notified:?}",
            arch.name
        ));
    }
    let listed = request(port, &certificate, "/mcp", Some(&token), TOOLS_LIST)
        .map_err(|e| format!("{}: tools/list: {e}", arch.name))?;
    if listed.status != 200 || !listed.body.contains("\"tools\"") {
        return Err(format!("{}: tools/list answered {listed:?}", arch.name));
    }
    let stranger = request(
        port,
        &certificate,
        "/mcp",
        Some("eggos_not-a-token"),
        TOOLS_LIST,
    )
    .map_err(|e| format!("{}: a stranger's request: {e}", arch.name))?;
    if stranger.status != 401 {
        return Err(format!(
            "{}: an unknown token answered {stranger:?}",
            arch.name
        ));
    }

    second.wait_for_prompt(2)?;
    second.type_line(LOG_COMMAND)?;
    second.wait_for(&|console: &str| console.contains(LOG_DONE), LOG_DONE)?;
    let log = second.text();
    let logged = log
        .lines()
        .any(|line| line.contains("laptop") && line.contains("xtask"));
    if !logged {
        return Err(format!(
            "{}: agentlog shows no session of laptop",
            arch.name
        ));
    }
    second.wait_for_prompt(3)?;
    second.type_line("poweroff\r")?;
    second.power_off()?;
    eprintln!("xtask: {}: network agents boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_and_the_certificate_are_read_from_the_console() {
        let console = "eggsh:/# agent pair laptop\r\npairing laptop: its client sends this code \
                       once, within 5 minutes, to https://<this machine>:8443/pair\r\n  \
                       BCDE-FGHJ\r\n";
        assert_eq!(code_from(console).as_deref(), Some("BCDE-FGHJ"));
        assert_eq!(code_from("nothing"), None);
        let pem = "-----BEGIN CERTIFICATE-----\r\nAAEC\r\n-----END CERTIFICATE-----\r\n";
        assert_eq!(certificate_from(pem), Some(vec![0, 1, 2]));
    }

    #[test]
    fn answers_and_their_json_are_read() {
        let answer = parse_answer(
            b"HTTP/1.1 200 OK\r\nContent-Length: 30\r\n\r\n{\"agent\":\"laptop\",\"token\":\"t\"}",
        )
        .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(json_text(&answer.body, "agent").as_deref(), Some("laptop"));
        assert_eq!(json_text(&answer.body, "token").as_deref(), Some("t"));
        assert!(parse_answer(b"garbage").is_err());
    }
}

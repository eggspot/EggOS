//! The JSON-RPC parser's fuzz target (D136): hostile lines made from valid messages (bytes
//! flipped, cut, repeated, duplicate keys, deep nesting, huge numbers, bad escapes and
//! surrogates, escaped `..` and NUL in paths, every tool's arguments, answers to the server's
//! questions and cancellations) and random bytes, each handed to a server whose machine is a
//! stand-in, checking every line it writes is a valid JSON-RPC message and it never panics.
//! Host tests run a bounded number; `cargo xtask fuzz mcp` runs it for longer.

use crate::arguments::{MAX_CONTENT_BYTES, WORDS_BYTES};
use crate::audit::RECORD_BYTES;
use crate::json;
use crate::paths::MAX_PATH_BYTES;
use crate::server::{
    ARGUMENTS_BYTES, ERROR_OUTPUT_LIMIT, OUTPUT_LIMIT, RESPONSE_BYTES, Scratch, Server,
};
use crate::system::{Ended, FileState, Invocation, RunOutcome, System};
use eggos_runtime::errno::Errno;

const CORPUS: [&str; 22] = [
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fuzz","version":"1"}}}"#,
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{"elicitation":{}},"clientInfo":{"name":"fuzz","version":"1"}}}"#,
    r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"tee","arguments":{"path":"/home/root/a","content":"x\u0000y","append":true}}}"#,
    r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"tee","arguments":{"path":"/home/root/file","content_base64":"AP/+","confirm":true}}}"#,
    r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"trash","arguments":{"path":"/home/root/file"}}}"#,
    r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"run","arguments":{"program":"/bin/uname","arguments":["-a","\u202e"],"timeout_seconds":60}}}"#,
    r#"{"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"app_uninstall","arguments":{"name":"app"}}}"#,
    r#"{"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"app_open","arguments":{"name":"../x"}}}"#,
    r#"{"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"app_install","arguments":{"path":"/media/host/x.exe"}}}"#,
    r#"{"jsonrpc":"2.0","id":"eggos-elicit-0-1","result":{"action":"accept","content":{"confirm":true}}}"#,
    r#"{"jsonrpc":"2.0","id":"eggos-elicit-0-2","result":{"action":"decline"}}"#,
    r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":12}}"#,
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    r#"{"jsonrpc":"2.0","id":"x","method":"tools/call","params":{"name":"cat","arguments":{"path":"/home/a.txt"}}}"#,
    r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ls","arguments":{"path":"/home/\u002e\u002e/etc"}}}"#,
    r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"cat","arguments":{"path":"/media/host/x\u0000y"}}}"#,
    r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#,
    r#"{"jsonrpc":"2.0","id":7,"method":"server/discover","params":{}}"#,
    r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"ps","arguments":{"a":1,"a":2}}}"#,
    r#"{"jsonrpc":"2.0","id":9,"result":{"ok":true}}"#,
];

const SPLICES: [&str; 14] = [
    "\\ud800",
    "\\udc00\\ud800",
    "\\u0000",
    "\\u002e\\u002e/",
    "1e999999",
    "-0.0e-0",
    "99999999999999999999999999",
    "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[",
    "{\"a\":1,\"a\":2}",
    "\u{202e}",
    "\"\"\"",
    "\\",
    "\u{0}",
    "null",
];

/// A stand-in machine: every command prints a little and succeeds; the log is never full.
#[derive(Debug, Default)]
pub struct StandIn {
    records: usize,
}

impl System for StandIn {
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        _: &mut [u8],
    ) -> RunOutcome {
        let text = b"stand-in output \xff\n";
        let length = text.len().min(stdout.len());
        if let (Some(place), Some(source)) = (stdout.get_mut(..length), text.get(..length)) {
            place.copy_from_slice(source);
        }
        RunOutcome {
            ended: Ended::Exited(0),
            stdout: length,
            stdout_cut: false,
            stderr: 0,
            stdin_taken: invocation.stdin.map_or(0, <[u8]>::len),
            stop_failed: None,
        }
    }

    fn is_link(&mut self, path: &str) -> bool {
        path.ends_with("link")
    }

    fn file_state(&mut self, path: &str) -> FileState {
        match path.len() % 4 {
            0 => FileState::Missing,
            1 => FileState::File(path.len() as u64),
            2 => FileState::Directory,
            _ => FileState::Other,
        }
    }

    fn monotonic_ms(&mut self) -> u64 {
        self.records as u64 * 1000
    }

    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno> {
        if record.last() != Some(&b'\n')
            || record.iter().filter(|&&byte| byte == b'\n').count() != 1
        {
            return Err(Errno::EINVAL);
        }
        self.records += 1;
        Ok(())
    }

    fn now(&mut self) -> u64 {
        0
    }

    fn report(&mut self, _: &[u8]) {}
}

/// A small xorshift generator: the same seed gives the same lines.
#[derive(Debug, Clone)]
pub struct Generator {
    state: u64,
}

impl Generator {
    /// A generator from `seed`.
    pub fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    /// Writes the next hostile line into `out` and returns its length.
    pub fn line(&mut self, out: &mut [u8]) -> usize {
        let base = CORPUS.get(self.below(CORPUS.len())).copied().unwrap_or("");
        let mut length = base.len().min(out.len());
        if let (Some(place), Some(source)) = (out.get_mut(..length), base.as_bytes().get(..length))
        {
            place.copy_from_slice(source);
        }
        for _ in 0..1 + self.below(4) {
            match self.below(6) {
                0 if length > 0 => {
                    let at = self.below(length);
                    if let Some(byte) = out.get_mut(at) {
                        *byte = self.next() as u8;
                    }
                }
                1 => length = self.below(length + 1),
                2 => {
                    let splice = SPLICES
                        .get(self.below(SPLICES.len()))
                        .copied()
                        .unwrap_or("");
                    let at = self.below(length + 1);
                    length = insert(out, length, at, splice.as_bytes());
                }
                3 if length > 0 => {
                    let from = self.below(length);
                    let to = (from + self.below(64)).min(length);
                    let mut piece = [0u8; 64];
                    let count = to - from;
                    if let (Some(place), Some(source)) = (piece.get_mut(..count), out.get(from..to))
                    {
                        place.copy_from_slice(source);
                    }
                    let at = self.below(length + 1);
                    length = insert(out, length, at, piece.get(..count).unwrap_or(&[]));
                }
                4 => {
                    let count = self.below(64).min(out.len() - length);
                    for byte in out.iter_mut().skip(length).take(count) {
                        *byte = self.next() as u8;
                    }
                    length += count;
                }
                _ => {
                    let depth = self.below(64);
                    let mut nested = [b'['; 64];
                    if let Some(last) = nested.get_mut(depth) {
                        *last = b'1';
                    }
                    let at = self.below(length + 1);
                    length = insert(out, length, at, nested.get(..=depth.min(63)).unwrap_or(&[]));
                }
            }
        }
        length
    }
}

fn insert(out: &mut [u8], length: usize, at: usize, piece: &[u8]) -> usize {
    let end = (length + piece.len()).min(out.len());
    let at = at.min(length);
    if at + piece.len() >= end {
        let count = end - at;
        if let (Some(place), Some(source)) = (out.get_mut(at..end), piece.get(..count)) {
            place.copy_from_slice(source);
        }
        return end;
    }
    out.copy_within(at..end - piece.len(), at + piece.len());
    if let Some(place) = out.get_mut(at..at + piece.len()) {
        place.copy_from_slice(piece);
    }
    end
}

/// Why a line's answer is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The answer is not JSON.
    NotJson,
    /// The answer holds a line break, so it would be two lines.
    LineBreak,
    /// The answer is not a JSON-RPC 2.0 response.
    NotResponse,
}

/// The buffers the fuzz target works in.
#[derive(Debug)]
pub struct Workspace {
    stdout: [u8; OUTPUT_LIMIT],
    stderr: [u8; ERROR_OUTPUT_LIMIT],
    decoded: [u8; MAX_PATH_BYTES + 1],
    resolved: [u8; MAX_PATH_BYTES + 1],
    record: [u8; RECORD_BYTES],
    arguments: [u8; ARGUMENTS_BYTES],
    content: [u8; MAX_CONTENT_BYTES],
    words: [u8; WORDS_BYTES],
    held_path: [u8; MAX_PATH_BYTES + 1],
    held_arguments: [u8; ARGUMENTS_BYTES],
    response: [u8; RESPONSE_BYTES],
    line: [u8; 4096],
}

impl Workspace {
    /// Empty buffers (about 1.5 MiB: keep them in a static).
    pub const fn new() -> Self {
        Self {
            stdout: [0; OUTPUT_LIMIT],
            stderr: [0; ERROR_OUTPUT_LIMIT],
            decoded: [0; MAX_PATH_BYTES + 1],
            resolved: [0; MAX_PATH_BYTES + 1],
            record: [0; RECORD_BYTES],
            arguments: [0; ARGUMENTS_BYTES],
            content: [0; MAX_CONTENT_BYTES],
            words: [0; WORDS_BYTES],
            held_path: [0; MAX_PATH_BYTES + 1],
            held_arguments: [0; ARGUMENTS_BYTES],
            response: [0; RESPONSE_BYTES],
            line: [0; 4096],
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// Hands `count` hostile lines from `generator` to `server`; the first wrong answer, with the
/// line that caused it, if any.
pub fn run(
    server: &mut Server,
    system: &mut StandIn,
    generator: &mut Generator,
    workspace: &mut Workspace,
    count: usize,
) -> Result<(), (Fault, usize)> {
    for index in 0..count {
        let length = generator.line(&mut workspace.line);
        let line = workspace.line.get(..length).unwrap_or(&[]);
        let mut scratch = Scratch {
            stdout: &mut workspace.stdout,
            stderr: &mut workspace.stderr,
            decoded: &mut workspace.decoded,
            resolved: &mut workspace.resolved,
            record: &mut workspace.record,
            arguments: &mut workspace.arguments,
            content: &mut workspace.content,
            words: &mut workspace.words,
            held_path: &mut workspace.held_path,
            held_arguments: &mut workspace.held_arguments,
        };
        let mut length = server.handle_line(line, system, &mut scratch, &mut workspace.response);
        if index % 64 == 63 {
            let now = system.monotonic_ms().saturating_add(1 << 20);
            let expired = workspace.response.get_mut(length..).unwrap_or_default();
            length += server.expire(now, system, &mut scratch, expired);
        }
        let written = workspace.response.get(..length).unwrap_or(&[]);
        check_lines(written).map_err(|fault| (fault, index))?;
    }
    Ok(())
}

/// Checks what the server wrote for one line: messages that each end in a newline and are
/// valid ([`check_answer`] for a response, or a request or notification of the server's).
pub fn check_lines(written: &[u8]) -> Result<(), Fault> {
    if written.is_empty() {
        return Ok(());
    }
    let Some(body) = written.strip_suffix(b"\n") else {
        return Err(Fault::LineBreak);
    };
    for message in body.split(|&byte| byte == b'\n') {
        check_message(message)?;
    }
    Ok(())
}

/// Checks one message the server sends: a JSON-RPC 2.0 response, or a request or notification
/// of its own (`elicitation/create`, `notifications/cancelled`).
pub fn check_message(message: &[u8]) -> Result<(), Fault> {
    if check_answer(message).is_ok() {
        return Ok(());
    }
    let value = json::parse(message).map_err(|_| Fault::NotJson)?.value;
    let version = value.get("jsonrpc").and_then(json::Value::as_str);
    let method = value.get("method").and_then(json::Value::as_str);
    match version.is_some_and(|version| version.equals("2.0")) && method.is_some() {
        true => Ok(()),
        false => Err(Fault::NotResponse),
    }
}

/// Checks one answer: one line of JSON that is a JSON-RPC 2.0 response.
pub fn check_answer(answer: &[u8]) -> Result<(), Fault> {
    if answer.contains(&b'\n') {
        return Err(Fault::LineBreak);
    }
    let value = json::parse(answer).map_err(|_| Fault::NotJson)?.value;
    let version = value.get("jsonrpc").and_then(json::Value::as_str);
    let has_body = value.get("result").is_some() != value.get("error").is_some();
    match version.is_some_and(|version| version.equals("2.0"))
        && has_body
        && value.get("id").is_some()
    {
        true => Ok(()),
        false => Err(Fault::NotResponse),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_thousand_hostile_lines_never_panic_and_each_answer_is_valid() {
        static WORKSPACE: std::sync::Mutex<Workspace> = std::sync::Mutex::new(Workspace::new());
        let mut workspace = WORKSPACE.lock().unwrap();
        let mut system = StandIn::default();
        for seed in 1..=4u64 {
            let mut server = match seed % 2 {
                0 => Server::restarted(),
                _ => Server::new(),
            };
            if seed > 2 {
                server.set_grants(crate::grants::Grants::ALL, Default::default());
            }
            let mut generator = Generator::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            assert_eq!(
                run(
                    &mut server,
                    &mut system,
                    &mut generator,
                    &mut workspace,
                    5000
                ),
                Ok(())
            );
        }
        assert!(system.records > 0);
    }

    #[test]
    fn the_generator_is_repeatable() {
        let mut first = [0u8; 4096];
        let mut second = [0u8; 4096];
        let a = Generator::new(7).line(&mut first);
        let b = Generator::new(7).line(&mut second);
        assert_eq!(first[..a], second[..b]);
    }
}

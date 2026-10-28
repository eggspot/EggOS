use crate::arguments::{MAX_CONTENT_BYTES, WORDS_BYTES};
use crate::audit::RECORD_BYTES;
use crate::grants::Grants;
use crate::paths::MAX_PATH_BYTES;
use crate::server::{
    ARGUMENTS_BYTES, ERROR_OUTPUT_LIMIT, OUTPUT_LIMIT, RESPONSE_BYTES, Scratch, Server,
};
use crate::system::{Ended, FileState, Invocation, RunOutcome, System};
use eggos_runtime::errno::Errno;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct FakeSystem {
    pub files: HashMap<String, Vec<u8>>,
    pub folders: Vec<String>,
    pub links: Vec<String>,
    pub outputs: HashMap<String, (Vec<u8>, Vec<u8>, Ended)>,
    pub runs: Vec<Vec<String>>,
    pub inputs: Vec<Vec<u8>>,
    pub time_limits: Vec<u64>,
    pub stdin_taken_at_most: Option<usize>,
    pub stop_fails: Option<Errno>,
    pub audit: Vec<String>,
    pub audit_fails_from: Option<usize>,
    pub reports: Vec<String>,
    pub clock: u64,
    pub monotonic: u64,
}

impl System for FakeSystem {
    fn run_twin(
        &mut self,
        invocation: &Invocation<'_>,
        stdout: &mut [u8],
        stderr: &mut [u8],
    ) -> RunOutcome {
        let mut words = vec![invocation.program.to_owned()];
        words.extend(invocation.words.iter().map(|word| (*word).to_owned()));
        if let Some(operand) = invocation.operand {
            words.push("--".into());
            words.push(operand.into());
        }
        self.runs.push(words);
        self.time_limits.push(invocation.time_limit_ms);
        let input = invocation.stdin.unwrap_or_default();
        let taken = self
            .stdin_taken_at_most
            .map_or(input.len(), |most| most.min(input.len()));
        self.inputs.push(input[..taken].to_vec());
        let (out, err, ended) = match (invocation.program, invocation.operand) {
            ("/bin/cat", Some(path)) => match self.files.get(path) {
                Some(contents) => (contents.clone(), Vec::new(), Ended::Exited(0)),
                None => (
                    Vec::new(),
                    format!("cat: {path}: No such file or directory\n").into_bytes(),
                    Ended::Exited(1),
                ),
            },
            ("/bin/tee", Some(path)) => {
                let appending = invocation.words.contains(&"-a");
                let file = self.files.entry(path.to_owned()).or_default();
                if !appending {
                    file.clear();
                }
                file.extend_from_slice(&input[..taken]);
                let ended = match taken < input.len() {
                    true => Ended::Exited(1),
                    false => Ended::Exited(0),
                };
                (input[..taken].to_vec(), Vec::new(), ended)
            }
            ("/bin/trash", Some(path)) => {
                self.files.remove(path);
                (Vec::new(), Vec::new(), Ended::Exited(0))
            }
            (program, _) => self.outputs.get(program).cloned().unwrap_or((
                format!("output of {program}\n").into_bytes(),
                Vec::new(),
                Ended::Exited(0),
            )),
        };
        let shown = match invocation.discard_output {
            true => Vec::new(),
            false => out,
        };
        let kept = shown.len().min(stdout.len());
        stdout[..kept].copy_from_slice(&shown[..kept]);
        let kept_err = err.len().min(stderr.len());
        stderr[..kept_err].copy_from_slice(&err[..kept_err]);
        RunOutcome {
            ended: if shown.len() > kept {
                Ended::Signalled(9)
            } else {
                ended
            },
            stdout: kept,
            stdout_cut: shown.len() > kept,
            stderr: kept_err,
            stdin_taken: taken,
            stop_failed: self.stop_fails,
        }
    }

    fn is_link(&mut self, path: &str) -> bool {
        self.links.iter().any(|link| link == path)
    }

    fn file_state(&mut self, path: &str) -> FileState {
        if let Some(contents) = self.files.get(path) {
            return FileState::File(contents.len() as u64);
        }
        match self.folders.iter().any(|folder| folder == path) {
            true => FileState::Directory,
            false => FileState::Missing,
        }
    }

    fn append_audit(&mut self, record: &[u8]) -> Result<(), Errno> {
        if self
            .audit_fails_from
            .is_some_and(|from| self.audit.len() >= from)
        {
            return Err(Errno::ENOSPC);
        }
        self.audit.push(String::from_utf8(record.to_vec()).unwrap());
        Ok(())
    }

    fn now(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn monotonic_ms(&mut self) -> u64 {
        self.monotonic
    }

    fn report(&mut self, message: &[u8]) {
        self.reports
            .push(String::from_utf8_lossy(message).into_owned());
    }
}

pub struct Buffers {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub decoded: Vec<u8>,
    pub resolved: Vec<u8>,
    pub record: Vec<u8>,
    pub arguments: Vec<u8>,
    pub content: Vec<u8>,
    pub words: Vec<u8>,
    pub held_path: Vec<u8>,
    pub held_arguments: Vec<u8>,
    pub response: Vec<u8>,
}

impl Buffers {
    pub fn new() -> Self {
        Self {
            stdout: vec![0; OUTPUT_LIMIT],
            stderr: vec![0; ERROR_OUTPUT_LIMIT],
            decoded: vec![0; MAX_PATH_BYTES + 1],
            resolved: vec![0; MAX_PATH_BYTES + 1],
            record: vec![0; RECORD_BYTES],
            arguments: vec![0; ARGUMENTS_BYTES],
            content: vec![0; MAX_CONTENT_BYTES],
            words: vec![0; WORDS_BYTES],
            held_path: vec![0; MAX_PATH_BYTES + 1],
            held_arguments: vec![0; ARGUMENTS_BYTES],
            response: vec![0; RESPONSE_BYTES],
        }
    }

    pub fn scratch(&mut self) -> (Scratch<'_>, &mut [u8]) {
        (
            Scratch {
                stdout: &mut self.stdout,
                stderr: &mut self.stderr,
                decoded: &mut self.decoded,
                resolved: &mut self.resolved,
                record: &mut self.record,
                arguments: &mut self.arguments,
                content: &mut self.content,
                words: &mut self.words,
                held_path: &mut self.held_path,
                held_arguments: &mut self.held_arguments,
            },
            &mut self.response,
        )
    }
}

pub struct Harness {
    pub server: Server,
    pub system: FakeSystem,
    pub buffers: Buffers,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_grants(Grants::NONE)
    }

    pub fn with_grants(grants: Grants) -> Self {
        let mut harness = Self {
            server: Server::new(),
            system: FakeSystem::default(),
            buffers: Buffers::new(),
        };
        harness.server.set_grants(grants, Default::default());
        let (mut scratch, _) = harness.buffers.scratch();
        harness
            .server
            .start(false, &mut harness.system, &mut scratch);
        harness
    }

    pub fn send(&mut self, line: &str) -> Option<String> {
        self.send_bytes(line.as_bytes())
    }

    pub fn send_bytes(&mut self, line: &[u8]) -> Option<String> {
        let lines = self.lines(line);
        assert!(lines.len() <= 1, "more than one line: {lines:?}");
        lines.into_iter().next()
    }

    pub fn lines(&mut self, line: &[u8]) -> Vec<String> {
        let (mut scratch, response) = self.buffers.scratch();
        let length = self
            .server
            .handle_line(line, &mut self.system, &mut scratch, response);
        split(&response[..length])
    }

    pub fn expire(&mut self, now_ms: u64) -> Vec<String> {
        let (mut scratch, response) = self.buffers.scratch();
        let length = self
            .server
            .expire(now_ms, &mut self.system, &mut scratch, response);
        split(&response[..length])
    }

    pub fn initialize(&mut self, capabilities: &str) {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{capabilities},"clientInfo":{{"name":"test-client","version":"1.0"}}}}}}"#
        ));
        self.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    }

    pub fn initialized() -> Self {
        let mut harness = Self::new();
        harness.initialize("{}");
        harness
    }

    pub fn granted(grants: Grants, capabilities: &str) -> Self {
        let mut harness = Self::with_grants(grants);
        harness.initialize(capabilities);
        harness
    }

    pub fn call(&mut self, id: u64, tool: &str, arguments: &str) -> String {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
        ))
        .unwrap()
    }

    pub fn call_lines(&mut self, id: u64, tool: &str, arguments: &str) -> Vec<String> {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
        );
        self.lines(line.as_bytes())
    }
}

fn split(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.is_empty() || text.ends_with('\n'), "{text}");
    text.lines().map(str::to_owned).collect()
}

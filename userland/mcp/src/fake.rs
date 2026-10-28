use crate::audit::RECORD_BYTES;
use crate::paths::MAX_PATH_BYTES;
use crate::server::{
    ARGUMENTS_BYTES, ERROR_OUTPUT_LIMIT, OUTPUT_LIMIT, RESPONSE_BYTES, Scratch, Server,
};
use crate::system::{Ended, Invocation, RunOutcome, System};
use eggos_runtime::errno::Errno;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct FakeSystem {
    pub files: HashMap<String, Vec<u8>>,
    pub links: Vec<String>,
    pub outputs: HashMap<String, (Vec<u8>, Vec<u8>, Ended)>,
    pub runs: Vec<Vec<String>>,
    pub audit: Vec<String>,
    pub audit_fails_from: Option<usize>,
    pub reports: Vec<String>,
    pub clock: u64,
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
        if let Some(path) = invocation.path {
            words.push("--".into());
            words.push(path.into());
        }
        self.runs.push(words);
        let (out, err, ended) = match (invocation.program, invocation.path) {
            ("/bin/cat", Some(path)) => match self.files.get(path) {
                Some(contents) => (contents.clone(), Vec::new(), Ended::Exited(0)),
                None => (
                    Vec::new(),
                    format!("cat: {path}: No such file or directory\n").into_bytes(),
                    Ended::Exited(1),
                ),
            },
            (program, _) => self.outputs.get(program).cloned().unwrap_or((
                format!("output of {program}\n").into_bytes(),
                Vec::new(),
                Ended::Exited(0),
            )),
        };
        let kept = out.len().min(stdout.len());
        stdout[..kept].copy_from_slice(&out[..kept]);
        let kept_err = err.len().min(stderr.len());
        stderr[..kept_err].copy_from_slice(&err[..kept_err]);
        RunOutcome {
            ended: if out.len() > kept {
                Ended::Signalled(9)
            } else {
                ended
            },
            stdout: kept,
            stdout_cut: out.len() > kept,
            stderr: kept_err,
        }
    }

    fn is_link(&mut self, path: &str) -> bool {
        self.links.iter().any(|link| link == path)
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
        let mut harness = Self {
            server: Server::new(),
            system: FakeSystem::default(),
            buffers: Buffers::new(),
        };
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
        let (mut scratch, response) = self.buffers.scratch();
        let length = self
            .server
            .handle_line(line, &mut self.system, &mut scratch, response);
        (length > 0).then(|| String::from_utf8(response[..length].to_vec()).unwrap())
    }

    pub fn initialized() -> Self {
        let mut harness = Self::new();
        harness.send(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}"#,
        );
        harness.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        harness
    }

    pub fn call(&mut self, id: u64, tool: &str, arguments: &str) -> String {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{arguments}}}}}"#
        ))
        .unwrap()
    }
}

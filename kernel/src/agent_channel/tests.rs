use super::*;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::sync::Mutex;

#[derive(Default)]
struct Wire {
    from_host: VecDeque<u8>,
    to_host: Vec<u8>,
    staged: Vec<u8>,
    host_open: bool,
    host_changed: bool,
    holding: bool,
}

struct FakePort(Arc<Mutex<Wire>>);

impl AgentPort for FakePort {
    fn has_input(&mut self) -> bool {
        !self.0.lock().unwrap().from_host.is_empty()
    }
    fn receive(&mut self, bytes: &mut [u8]) -> usize {
        let mut wire = self.0.lock().unwrap();
        let count = bytes.len().min(wire.from_host.len()).min(3);
        for byte in bytes.iter_mut().take(count) {
            *byte = wire.from_host.pop_front().unwrap();
        }
        count
    }
    fn host_connected(&self) -> bool {
        self.0.lock().unwrap().host_open
    }
    fn poll_control(&mut self) -> bool {
        core::mem::take(&mut self.0.lock().unwrap().host_changed)
    }
    fn is_sending(&mut self) -> bool {
        self.0.lock().unwrap().holding
    }
    fn stage(&mut self, bytes: &[u8]) -> usize {
        let mut wire = self.0.lock().unwrap();
        if wire.holding {
            return 0;
        }
        let count = bytes.len().min(8 - wire.staged.len());
        wire.staged.extend_from_slice(&bytes[..count]);
        count
    }
    fn send_staged(&mut self) -> Result<(), &'static str> {
        let mut wire = self.0.lock().unwrap();
        let staged = core::mem::take(&mut wire.staged);
        wire.to_host.extend_from_slice(&staged);
        Ok(())
    }
}

fn installed() -> (std::sync::MutexGuard<'static, ()>, Arc<Mutex<Wire>>) {
    let guard = tests_lock();
    uninstall();
    let wire = Arc::new(Mutex::new(Wire {
        host_open: true,
        ..Wire::default()
    }));
    assert!(install(Box::new(FakePort(Arc::clone(&wire)))).is_ok());
    (guard, wire)
}

fn read_everything() -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = [0u8; 5];
    while let Received::Bytes(count) = receive(&mut buffer) {
        out.extend_from_slice(&buffer[..count]);
    }
    out
}

fn send_everything(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let taken = send(bytes).unwrap();
        bytes = &bytes[taken..];
    }
    flush().unwrap();
}

#[test]
fn there_is_one_channel_and_one_opener_at_a_time() {
    let (_guard, wire) = installed();
    assert!(is_present());
    assert!(install(Box::new(FakePort(wire))).is_err());
    assert_eq!(open(), Ok(()));
    assert_eq!(open(), Err(OpenError::Busy));
    close();
    assert_eq!(open(), Ok(()));
    close();
    uninstall();
    assert!(!is_present());
    assert_eq!(open(), Err(OpenError::Absent));
}

#[test]
fn bytes_flow_both_ways_through_small_pieces() {
    let (_guard, wire) = installed();
    open().unwrap();
    assert!(!ready_to_receive());
    assert!(!has_bytes());
    wire.lock()
        .unwrap()
        .from_host
        .extend(b"{\"a\":1}\n{\"b\":2}\n");
    assert!(ready_to_receive());
    assert!(has_bytes());
    assert_eq!(read_everything(), b"{\"a\":1}\n{\"b\":2}\n");
    send_everything(b"an answer longer than the port's buffer\n");
    assert_eq!(
        wire.lock().unwrap().to_host,
        b"an answer longer than the port's buffer\n"
    );
    close();
    uninstall();
}

#[test]
fn a_new_opener_skips_the_rest_of_a_line_left_half_read() {
    let (_guard, wire) = installed();
    open().unwrap();
    wire.lock().unwrap().from_host.extend(b"{\"half\":");
    assert_eq!(read_everything(), b"{\"half\":");
    close();
    wire.lock()
        .unwrap()
        .from_host
        .extend(b"\"rest\"}\n{\"next\":1}\n");
    open().unwrap();
    assert_eq!(read_everything(), b"{\"next\":1}\n");
    close();
    wire.lock().unwrap().from_host.extend(b"{\"whole\":1}\n");
    open().unwrap();
    assert_eq!(read_everything(), b"{\"whole\":1}\n");
    close();
    uninstall();
}

#[test]
fn a_new_opener_starts_a_new_line_after_an_answer_cut_short() {
    let (_guard, wire) = installed();
    open().unwrap();
    send_everything(b"{\"cut\":");
    close();
    open().unwrap();
    send_everything(b"{\"new\":1}\n");
    close();
    open().unwrap();
    send_everything(b"{\"more\":1}\n");
    assert_eq!(
        wire.lock().unwrap().to_host,
        b"{\"cut\":\n{\"new\":1}\n{\"more\":1}\n"
    );
    close();
    uninstall();
}

#[test]
fn the_host_closing_ends_input_once_and_reopening_resumes_it() {
    let (_guard, wire) = installed();
    open().unwrap();
    {
        let mut wire = wire.lock().unwrap();
        wire.from_host.extend(b"last\n");
        wire.host_open = false;
        wire.host_changed = true;
    }
    let mut buffer = [0u8; 8];
    let mut got = Vec::new();
    let ended = loop {
        match receive(&mut buffer) {
            Received::Bytes(count) => got.extend_from_slice(&buffer[..count]),
            other => break other,
        }
    };
    assert_eq!(got, b"last\n");
    assert_eq!(ended, Received::EndOfInput);
    assert_eq!(receive(&mut buffer), Received::Nothing);
    assert!(!ready_to_receive());
    {
        let mut wire = wire.lock().unwrap();
        wire.host_open = true;
        wire.host_changed = true;
        wire.from_host.extend(b"again\n");
    }
    assert!(ready_to_receive());
    assert_eq!(read_everything(), b"again\n");
    close();
    uninstall();
}

#[test]
fn a_held_transmission_takes_nothing_until_the_device_lets_it_go() {
    let (_guard, wire) = installed();
    open().unwrap();
    wire.lock().unwrap().holding = true;
    assert_eq!(send(b"x\n"), Ok(0));
    assert!(!sent_everything());
    assert_eq!(flush(), Ok(false));
    wire.lock().unwrap().holding = false;
    assert!(sent_everything());
    send_everything(b"x\n");
    assert_eq!(wire.lock().unwrap().to_host, b"x\n");
    close();
    uninstall();
}

#[test]
fn an_agent_is_connected_while_the_channel_is_open_and_the_host_side_is_there() {
    let (_guard, wire) = installed();
    assert!(!is_connected(), "nobody opened it");
    assert_eq!(open(), Ok(()));
    assert!(is_connected());
    wire.lock().unwrap().host_open = false;
    assert!(!is_connected(), "the host side went away");
    wire.lock().unwrap().host_open = true;
    close();
    assert!(!is_connected());
    uninstall();
    assert!(!is_connected());
}

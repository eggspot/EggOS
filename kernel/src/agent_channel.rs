//! The agent channel (D136): the port a host's MCP client reaches EggOS through, a named port
//! of a virtio console device (`org.eggos.mcp`), which `/dev/agent` hands to one opener at a
//! time, the built-in MCP server. Bytes flow raw both ways: no line discipline, no echo.
//!
//! Lines are the protocol's unit, so the channel remembers whether the last byte read and the
//! last byte sent ended a line. A new opener of a channel left in the middle of a line in
//! either direction does not see the rest of that line, and its first write starts a new line,
//! so a server that starts again neither parses half a message nor glues its first answer to
//! half of one. When the host side closes (QEMU's character device reaches the end of its
//! input), a read returns end of file once the bytes already sent are read; reads after that
//! wait until the host side opens again and sends more. Nothing is held while no channel is
//! present.

use crate::sync::LeafSpinLock;
use alloc::boxed::Box;

/// The port's name a virtio console must give the port for it to be the agent channel.
pub const PORT_NAME: &[u8] = b"org.eggos.mcp";

/// What a driver lends the channel: a byte stream to the host, polled.
pub trait AgentPort: Send {
    /// Whether bytes from the host wait to be read.
    fn has_input(&mut self) -> bool;
    /// Copies the waiting bytes into the start of `bytes`; returns how many.
    fn receive(&mut self, bytes: &mut [u8]) -> usize;
    /// Whether the host side is open.
    fn host_connected(&self) -> bool;
    /// Takes the device's news about the port; whether the host side opened or closed.
    fn poll_control(&mut self) -> bool;
    /// Whether a transmission is still with the device.
    fn is_sending(&mut self) -> bool;
    /// Appends the start of `bytes` to the next transmission; how many fitted (0 while one
    /// is with the device).
    fn stage(&mut self, bytes: &[u8]) -> usize;
    /// Hands the staged bytes to the device.
    fn send_staged(&mut self) -> Result<(), &'static str>;
}

/// Why the channel could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// There is no channel.
    Absent,
    /// Another opener holds it.
    Busy,
}

/// What a read found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// This many bytes, at the start of the caller's buffer.
    Bytes(usize),
    /// The host side closed and every byte it sent was read.
    EndOfInput,
    /// Nothing yet: wait and try again.
    Nothing,
}

struct Channel {
    port: Box<dyn AgentPort>,
    open: bool,
    input_mid_line: bool,
    output_mid_line: bool,
    skipping_to_newline: bool,
    newline_owed: bool,
    end_reported: bool,
}

static CHANNEL: LeafSpinLock<Option<Box<Channel>>> = LeafSpinLock::new_leaf(None);

/// Makes `port` the agent channel; gives it back when there already is one.
pub fn install(port: Box<dyn AgentPort>) -> Result<(), Box<dyn AgentPort>> {
    let mut channel = CHANNEL.lock();
    if channel.is_some() {
        return Err(port);
    }
    *channel = Some(Box::new(Channel {
        port,
        open: false,
        input_mid_line: false,
        output_mid_line: false,
        skipping_to_newline: false,
        newline_owed: false,
        end_reported: false,
    }));
    Ok(())
}

/// Whether there is an agent channel.
pub fn is_present() -> bool {
    CHANNEL.lock().is_some()
}

/// Opens the channel for its one opener.
pub fn open() -> Result<(), OpenError> {
    let mut guard = CHANNEL.lock();
    let channel = guard.as_mut().ok_or(OpenError::Absent)?;
    if channel.open {
        return Err(OpenError::Busy);
    }
    channel.open = true;
    channel.skipping_to_newline = channel.input_mid_line;
    channel.newline_owed = channel.output_mid_line;
    Ok(())
}

/// Lets the channel go for the next opener.
pub fn close() {
    if let Some(channel) = CHANNEL.lock().as_mut() {
        channel.open = false;
    }
}

/// Copies what the host sent into the start of `bytes`.
pub fn receive(bytes: &mut [u8]) -> Received {
    let mut guard = CHANNEL.lock();
    let Some(channel) = guard.as_mut() else {
        return Received::EndOfInput;
    };
    if channel.port.poll_control() && channel.port.host_connected() {
        channel.end_reported = false;
    }
    loop {
        let count = channel.port.receive(bytes);
        if count == 0 {
            break;
        }
        let mut start = 0;
        if channel.skipping_to_newline {
            match bytes[..count].iter().position(|&byte| byte == b'\n') {
                Some(newline) => {
                    channel.skipping_to_newline = false;
                    start = newline + 1;
                }
                None => continue,
            }
        }
        if start == count {
            channel.input_mid_line = false;
            continue;
        }
        bytes.copy_within(start..count, 0);
        let length = count - start;
        channel.input_mid_line = bytes[length - 1] != b'\n';
        channel.end_reported = false;
        return Received::Bytes(length);
    }
    if !channel.port.host_connected() && !channel.end_reported {
        channel.end_reported = true;
        return Received::EndOfInput;
    }
    Received::Nothing
}

/// Whether a read would find something: bytes, or the host side's end not yet reported.
pub fn ready_to_receive() -> bool {
    let mut guard = CHANNEL.lock();
    let Some(channel) = guard.as_mut() else {
        return true;
    };
    if channel.port.poll_control() && channel.port.host_connected() {
        channel.end_reported = false;
    }
    channel.port.has_input() || (!channel.port.host_connected() && !channel.end_reported)
}

/// Appends the start of `bytes` to what goes to the host; returns how many were taken, 0 while
/// the device still holds the last transmission. A full transmission is handed over at once.
pub fn send(bytes: &[u8]) -> Result<usize, &'static str> {
    let mut guard = CHANNEL.lock();
    let Some(channel) = guard.as_mut() else {
        return Err("the agent channel is gone");
    };
    if channel.newline_owed {
        if channel.port.stage(b"\n") == 0 {
            return Ok(0);
        }
        channel.newline_owed = false;
        channel.output_mid_line = false;
    }
    let taken = channel.port.stage(bytes);
    if taken > 0 {
        channel.output_mid_line = bytes[taken - 1] != b'\n';
    }
    if taken < bytes.len() {
        channel.port.send_staged()?;
    }
    Ok(taken)
}

/// Hands what was staged to the device; whether the device has taken everything (the host
/// read it all).
pub fn flush() -> Result<bool, &'static str> {
    let mut guard = CHANNEL.lock();
    let Some(channel) = guard.as_mut() else {
        return Err("the agent channel is gone");
    };
    channel.port.send_staged()?;
    Ok(!channel.port.is_sending())
}

/// Whether the device has taken every transmission.
pub fn sent_everything() -> bool {
    let mut guard = CHANNEL.lock();
    guard
        .as_mut()
        .is_none_or(|channel| !channel.port.is_sending())
}

#[cfg(test)]
pub(crate) fn uninstall() {
    *CHANNEL.lock() = None;
}

#[cfg(test)]
static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn tests_lock() -> std::sync::MutexGuard<'static, ()> {
    TESTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) fn install_fake() {
    struct Silent;
    impl AgentPort for Silent {
        fn has_input(&mut self) -> bool {
            false
        }
        fn receive(&mut self, _: &mut [u8]) -> usize {
            0
        }
        fn host_connected(&self) -> bool {
            true
        }
        fn poll_control(&mut self) -> bool {
            false
        }
        fn is_sending(&mut self) -> bool {
            false
        }
        fn stage(&mut self, bytes: &[u8]) -> usize {
            bytes.len()
        }
        fn send_staged(&mut self) -> Result<(), &'static str> {
            Ok(())
        }
    }
    uninstall();
    assert!(install(Box::new(Silent)).is_ok());
}

#[cfg(test)]
mod tests;

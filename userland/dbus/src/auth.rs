//! The SASL handshake a connection opens with, on the server's side: a NUL byte, then lines
//! of text. EggOS accepts the `EXTERNAL` mechanism only, checked against the peer's user id
//! the kernel reports for the socket, and lets the client pass descriptors when it asks.

use alloc::vec::Vec;

/// The longest line a client may send.
pub const MAX_LINE_BYTES: usize = 16 * 1024;
/// How many rejected attempts close the connection.
pub const MAX_REJECTIONS: u32 = 8;

const REJECTED: &[u8] = b"REJECTED EXTERNAL\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Nul,
    Auth,
    Data,
    Begin,
    Done,
}

/// What a client's bytes did to the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// More lines are needed; this many bytes were used.
    Going(usize),
    /// `BEGIN` came; this many bytes were the handshake's, the rest are messages.
    Begun(usize),
    /// The client broke the protocol or failed too often: close the connection.
    Failed,
}

/// One connection's handshake.
#[derive(Debug, Clone)]
pub struct Handshake {
    state: State,
    uid: u32,
    guid: [u8; 32],
    line: Vec<u8>,
    rejections: u32,
    unix_fds: bool,
}

fn decode_hex(text: &[u8]) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let digit = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
    text.chunks(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}

fn decimal(number: u32) -> Vec<u8> {
    let mut digits = Vec::new();
    let mut rest = number;
    loop {
        digits.push(b'0' + (rest % 10) as u8);
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    digits.reverse();
    digits
}

impl Handshake {
    /// The handshake with a peer whose user id is `uid`, on a bus whose id is `guid`.
    pub fn new(uid: u32, guid: [u8; 32]) -> Self {
        Self {
            state: State::Nul,
            uid,
            guid,
            line: Vec::new(),
            rejections: 0,
            unix_fds: false,
        }
    }

    /// Whether the client asked to pass descriptors and was told it may.
    pub fn unix_fds(&self) -> bool {
        self.unix_fds
    }

    /// Takes the client's `input`, writing the server's lines into `replies`.
    pub fn feed(&mut self, input: &[u8], replies: &mut Vec<u8>) -> Progress {
        let mut used = 0;
        if self.state == State::Nul {
            match input.first() {
                Some(0) => {
                    used = 1;
                    self.state = State::Auth;
                }
                Some(_) => return Progress::Failed,
                None => return Progress::Going(0),
            }
        }
        while used < input.len() {
            let byte = input[used];
            used += 1;
            self.line.push(byte);
            if self.line.len() > MAX_LINE_BYTES {
                return Progress::Failed;
            }
            if !self.line.ends_with(b"\r\n") {
                continue;
            }
            let mut line = core::mem::take(&mut self.line);
            line.truncate(line.len() - 2);
            self.command(&line, replies);
            match self.state {
                State::Done => return Progress::Begun(used),
                _ if self.rejections >= MAX_REJECTIONS => return Progress::Failed,
                _ => {}
            }
        }
        Progress::Going(used)
    }

    fn command(&mut self, line: &[u8], replies: &mut Vec<u8>) {
        let mut words = line.splitn(3, |&byte| byte == b' ');
        let verb = words.next().unwrap_or(b"");
        let first = words.next();
        let second = words.next();
        match (self.state, verb) {
            (State::Auth, b"AUTH") => match (first, second) {
                (Some(b"EXTERNAL"), Some(identity)) => self.check(Some(identity), replies),
                (Some(b"EXTERNAL"), None) => {
                    self.state = State::Data;
                    replies.extend_from_slice(b"DATA\r\n");
                }
                _ => self.reject(replies),
            },
            (State::Data, b"DATA") => self.check(first, replies),
            (State::Data | State::Begin, b"CANCEL" | b"ERROR") => self.reject(replies),
            (State::Begin, b"NEGOTIATE_UNIX_FD") => {
                self.unix_fds = true;
                replies.extend_from_slice(b"AGREE_UNIX_FD\r\n");
            }
            (State::Begin, b"BEGIN") => self.state = State::Done,
            (State::Auth, b"BEGIN" | b"DATA" | b"CANCEL") => self.reject(replies),
            (State::Auth, b"ERROR") => self.reject(replies),
            _ => {
                self.rejections += 1;
                replies.extend_from_slice(b"ERROR \"Unknown command\"\r\n");
            }
        }
    }

    fn check(&mut self, identity: Option<&[u8]>, replies: &mut Vec<u8>) {
        let claimed = match identity {
            None | Some(b"") => Some(decimal(self.uid)),
            Some(hex) => decode_hex(hex),
        };
        if claimed.as_deref() != Some(decimal(self.uid).as_slice()) {
            self.reject(replies);
            return;
        }
        self.state = State::Begin;
        replies.extend_from_slice(b"OK ");
        replies.extend_from_slice(&self.guid);
        replies.extend_from_slice(b"\r\n");
    }

    fn reject(&mut self, replies: &mut Vec<u8>) {
        self.state = State::Auth;
        self.rejections += 1;
        replies.extend_from_slice(REJECTED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: [u8; 32] = *b"0123456789abcdef0123456789abcdef";

    #[test]
    fn external_with_the_peer_s_id_then_fds_then_begin() {
        let mut handshake = Handshake::new(0, GUID);
        let mut replies = Vec::new();
        let input = b"\0AUTH EXTERNAL 30\r\nNEGOTIATE_UNIX_FD\r\nBEGIN\r\nl\x01";
        assert_eq!(handshake.feed(input, &mut replies), Progress::Begun(input.len() - 2));
        assert_eq!(
            replies,
            b"OK 0123456789abcdef0123456789abcdef\r\nAGREE_UNIX_FD\r\n"
        );
        assert!(handshake.unix_fds());
    }

    #[test]
    fn another_user_s_id_is_rejected_and_data_without_one_takes_the_peer_s() {
        let mut handshake = Handshake::new(1000, GUID);
        let mut replies = Vec::new();
        handshake.feed(b"\0AUTH EXTERNAL 30\r\n", &mut replies);
        assert_eq!(replies, REJECTED);
        replies.clear();
        handshake.feed(b"AUTH EXTERNAL\r\nDATA\r\n", &mut replies);
        assert_eq!(&replies[..6], b"DATA\r\n");
        assert!(replies[6..].starts_with(b"OK "));
    }

    #[test]
    fn a_missing_nul_or_endless_rejections_close_the_connection() {
        let mut replies = Vec::new();
        assert_eq!(Handshake::new(0, GUID).feed(b"AUTH\r\n", &mut replies), Progress::Failed);
        let mut handshake = Handshake::new(0, GUID);
        let mut input = Vec::from(&b"\0"[..]);
        for _ in 0..MAX_REJECTIONS {
            input.extend_from_slice(b"AUTH ANONYMOUS\r\n");
        }
        assert_eq!(handshake.feed(&input, &mut replies), Progress::Failed);
    }
}

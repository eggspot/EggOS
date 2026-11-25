//! A program's side of a stream on the sound server (D776), for the foreign layers' sound
//! APIs (`NSSound`, `PlaySound` and `waveOut`): [`Player::open`] connects to `/run/soundd` and
//! starts a stream, [`Player::push`] queues 16-bit little-endian samples, and
//! [`Player::pump`], called from the program's own timer, hands the server what it takes
//! without ever blocking; closing the player ends the stream once the server played it out.

use alloc::vec::Vec;
use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::{self, number};

const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const MSG_NOSIGNAL: usize = 0x4000;
const SOCKET: &[u8] = b"/run/soundd";

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: the calls pass only memory this module owns, sized as the call uses it.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

/// Whether a sound server answers on `/run/soundd`.
pub fn reachable() -> bool {
    Player::connect().is_some_and(|socket| {
        let _ = call(number::CLOSE, [socket, 0, 0, 0, 0, 0]);
        true
    })
}

/// Asks the sound server to play the alert sound (D777), when one runs.
pub fn alert() {
    let Some(socket) = Player::connect() else {
        return;
    };
    let line = b"alert\n";
    let _ = call(
        number::SENDTO,
        [socket, line.as_ptr() as usize, line.len(), MSG_NOSIGNAL, 0, 0],
    );
    let _ = call(number::CLOSE, [socket, 0, 0, 0, 0, 0]);
}

/// One stream being played.
#[derive(Debug)]
pub struct Player {
    socket: usize,
    pending: Vec<u8>,
    sent: usize,
}

impl Player {
    fn connect() -> Option<usize> {
        let socket = call(
            number::SOCKET,
            [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, 0, 0, 0],
        )
        .ok()?;
        let mut address = [0u8; 110];
        address[..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
        address[2..2 + SOCKET.len()].copy_from_slice(SOCKET);
        let length = 2 + SOCKET.len() + 1;
        if call(
            number::CONNECT,
            [socket, address.as_ptr() as usize, length, 0, 0, 0],
        )
        .is_err()
        {
            let _ = call(number::CLOSE, [socket, 0, 0, 0, 0, 0]);
            return None;
        }
        Some(socket)
    }

    /// Starts a stream of `channels` channels at `rate` frames a second for the app `app`
    /// (no spaces); `None` when no sound server runs.
    pub fn open(rate: u32, channels: u8, app: &str) -> Option<Self> {
        let socket = Self::connect()?;
        let request = crate::protocol::Request::Play {
            rate,
            channels,
            app: alloc::string::String::from(app),
        };
        let mut line = request.line();
        line.push('\n');
        let mut player = Self {
            socket,
            pending: line.into_bytes(),
            sent: 0,
        };
        let flags = SOCK_NONBLOCK;
        let _ = call(number::FCNTL, [socket, 4, flags, 0, 0, 0]);
        player.pump();
        Some(player)
    }

    /// Queues samples behind those not yet handed over.
    pub fn push(&mut self, samples: &[u8]) {
        self.pending.extend_from_slice(samples);
    }

    /// Hands the server what it takes now; whether everything queued has been handed over.
    pub fn pump(&mut self) -> bool {
        while self.sent < self.pending.len() {
            let rest = &self.pending[self.sent..];
            let written = call(
                number::SENDTO,
                [
                    self.socket,
                    rest.as_ptr() as usize,
                    rest.len(),
                    MSG_NOSIGNAL,
                    0,
                    0,
                ],
            );
            match written {
                Ok(0) | Err(_) => break,
                Ok(count) => self.sent += count,
            }
        }
        if self.sent == self.pending.len() {
            self.pending.clear();
            self.sent = 0;
            return true;
        }
        false
    }

    /// How many bytes still wait to be handed over.
    pub fn waiting(&self) -> usize {
        self.pending.len() - self.sent
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = call(number::CLOSE, [self.socket, 0, 0, 0, 0, 0]);
    }
}

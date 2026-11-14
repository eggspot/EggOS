//! Between a connection's process and the network server's (D576): each connection runs in a
//! process of its own, which does its TLS and HTTP and hands every MCP message to the server's
//! process over a socket pair, where the agents' servers and sessions live. Frames are a kind
//! and a length, 4 bytes each in the machine's order, then the bytes.

use super::super::{POLLERR, POLLHUP, POLLIN, PollDescriptor, call, now_ms};
use crate::http::{MAX_SESSION_ID_BYTES, Status};
use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::{self, number};

/// The bytes before a frame's payload.
pub const FRAME_HEADER_BYTES: usize = 8;
/// The bytes a request frame adds before the message: the token's hash, the session id's
/// length and the session id.
pub const REQUEST_HEAD_BYTES: usize = 32 + 1 + MAX_SESSION_ID_BYTES;

/// The kinds of frame.
pub mod kind {
    /// Connection to server: an MCP message.
    pub const REQUEST: u32 = 1;
    /// Connection to server: open the session's event stream on this connection.
    pub const LISTEN: u32 = 2;
    /// Connection to server: end the session.
    pub const END_SESSION: u32 = 3;
    /// Server to connection: the answer to the last request.
    pub const ANSWER: u32 = 4;
    /// Server to connection: one message for the session's event stream.
    pub const EVENT: u32 = 5;
}

/// Writes one frame of `kind` made of `parts` to `descriptor`.
pub fn write_frame(descriptor: usize, kind: u32, parts: &[&[u8]]) -> bool {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    let Ok(length) = u32::try_from(length) else {
        return false;
    };
    let mut header = [0u8; FRAME_HEADER_BYTES];
    header[..4].copy_from_slice(&kind.to_ne_bytes());
    header[4..].copy_from_slice(&length.to_ne_bytes());
    syscall::write_all(descriptor, &header) && parts.iter().all(|part| syscall::write_all(descriptor, part))
}

fn read_exact(descriptor: usize, out: &mut [u8]) -> bool {
    let mut filled = 0;
    while filled < out.len() {
        let free = &mut out[filled..];
        match call(number::READ, [descriptor, free.as_mut_ptr() as usize, free.len(), 0, 0, 0]) {
            Ok(0) => return false,
            Ok(count) => filled += count,
            Err(Errno::EINTR | Errno::EAGAIN) => continue,
            Err(_) => return false,
        }
    }
    true
}

/// Reads one frame from `descriptor` into `out`: its kind and length. `None` when the other
/// side is gone or the frame does not fit.
pub fn read_frame(descriptor: usize, out: &mut [u8]) -> Option<(u32, usize)> {
    let mut header = [0u8; FRAME_HEADER_BYTES];
    if !read_exact(descriptor, &mut header) {
        return None;
    }
    let kind = u32::from_ne_bytes(header[..4].try_into().ok()?);
    let length = u32::from_ne_bytes(header[4..].try_into().ok()?) as usize;
    let place = out.get_mut(..length)?;
    read_exact(descriptor, place).then_some((kind, length))
}

/// Whether `descriptor` has something to read within `milliseconds`.
pub fn readable_within(descriptor: usize, milliseconds: u64) -> bool {
    let mut watched = [PollDescriptor {
        descriptor: descriptor as i32,
        events: POLLIN,
        returned_events: 0,
    }];
    let timeout = [(milliseconds / 1000) as i64, (milliseconds % 1000 * 1_000_000) as i64];
    let deadline = now_ms() + milliseconds;
    loop {
        match call(
            number::PPOLL,
            [watched.as_mut_ptr() as usize, 1, timeout.as_ptr() as usize, 0, 8, 0],
        ) {
            Ok(0) => return false,
            Ok(_) => return watched[0].returned_events & (POLLIN | POLLHUP | POLLERR) != 0,
            Err(Errno::EINTR) if now_ms() < deadline => continue,
            Err(_) => return false,
        }
    }
}

/// A request's head: the token's hash and the session it names.
pub fn request_head<'h>(token_hash: &[u8; 32], session: Option<&[u8]>, out: &'h mut [u8; REQUEST_HEAD_BYTES]) -> &'h [u8] {
    let session = session.unwrap_or_default();
    let length = session.len().min(MAX_SESSION_ID_BYTES);
    out[..32].copy_from_slice(token_hash);
    out[32] = length as u8;
    out[33..33 + length].copy_from_slice(&session[..length]);
    &out[..33 + length]
}

/// The parts of a request's payload: the token's hash, the session (if one) and the rest.
pub fn split_request(payload: &[u8]) -> Option<([u8; 32], Option<&[u8]>, &[u8])> {
    let token: [u8; 32] = payload.get(..32)?.try_into().ok()?;
    let length = usize::from(*payload.get(32)?);
    let session = payload.get(33..33 + length)?;
    let rest = payload.get(33 + length..)?;
    Some((token, (!session.is_empty()).then_some(session), rest))
}

/// Writes an answer frame: `status`, the session the answer names, and its body.
pub fn write_answer(descriptor: usize, status: Status, session: Option<&[u8]>, body: &[u8]) -> bool {
    let session = session.unwrap_or_default();
    let length = session.len().min(MAX_SESSION_ID_BYTES);
    let head = [status.code().to_ne_bytes()[0], status.code().to_ne_bytes()[1], length as u8];
    write_frame(descriptor, kind::ANSWER, &[&head, &session[..length], body])
}

/// The parts of an answer's payload: its status code, its session and its body.
pub fn split_answer(payload: &[u8]) -> Option<(u16, Option<&[u8]>, &[u8])> {
    let code = u16::from_ne_bytes(payload.get(..2)?.try_into().ok()?);
    let length = usize::from(*payload.get(2)?);
    let session = payload.get(3..3 + length)?;
    let body = payload.get(3 + length..)?;
    Some((code, (!session.is_empty()).then_some(session), body))
}

/// The status whose code is `code`.
pub fn status_of(code: u16) -> Status {
    [
        Status::Ok,
        Status::Accepted,
        Status::BadRequest,
        Status::Unauthorized,
        Status::Forbidden,
        Status::NotFound,
        Status::MethodNotAllowed,
        Status::NotAcceptable,
        Status::LengthRequired,
        Status::PayloadTooLarge,
        Status::UnsupportedMediaType,
        Status::TooManyRequests,
        Status::HeaderTooLarge,
        Status::NotImplemented,
    ]
    .into_iter()
    .find(|status| status.code() == code)
    .unwrap_or(Status::BadRequest)
}

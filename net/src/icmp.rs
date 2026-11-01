//! ICMP echo (RFC 792): the request `ping` sends and the reply a host answers it with, a type,
//! a code of 0, a checksum over the whole message, an identifier, a sequence number and data.

use crate::checksum;

/// Bytes of an echo message before its data.
pub const HEADER_BYTES: usize = 8;

/// The message types the stack reads or writes.
pub mod kind {
    /// An answer to an echo request.
    pub const ECHO_REPLY: u8 = 0;
    /// A request to echo the data back.
    pub const ECHO_REQUEST: u8 = 8;
}

/// An echo request or reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Echo<'a> {
    /// [`kind::ECHO_REQUEST`] or [`kind::ECHO_REPLY`].
    pub kind: u8,
    /// Which conversation it belongs to (the ping socket's).
    pub identifier: u16,
    /// Which request of the conversation.
    pub sequence: u16,
    /// The data the reply echoes.
    pub data: &'a [u8],
}

/// The echo request or reply `message` holds; `None` for another message, a code other than 0
/// or a wrong checksum.
pub fn parse_echo(message: &[u8]) -> Option<Echo<'_>> {
    let (header, data) = message.split_first_chunk::<HEADER_BYTES>()?;
    let kind = header[0];
    if !matches!(kind, kind::ECHO_REQUEST | kind::ECHO_REPLY) || header[1] != 0 {
        return None;
    }
    if checksum::of(message) != 0 {
        return None;
    }
    Some(Echo {
        kind,
        identifier: u16::from_be_bytes([header[4], header[5]]),
        sequence: u16::from_be_bytes([header[6], header[7]]),
        data,
    })
}

/// Writes the header of an echo message of `kind` whose `data_bytes` of data are already at
/// `out[HEADER_BYTES..]`, with its checksum: the message's length, or `None` when `out` is
/// shorter.
pub fn write_echo_header(
    out: &mut [u8],
    kind: u8,
    identifier: u16,
    sequence: u16,
    data_bytes: usize,
) -> Option<usize> {
    let length = HEADER_BYTES.checked_add(data_bytes)?;
    let message = out.get_mut(..length)?;
    message[0] = kind;
    message[1] = 0;
    message[2..4].fill(0);
    message[4..6].copy_from_slice(&identifier.to_be_bytes());
    message[6..8].copy_from_slice(&sequence.to_be_bytes());
    let sum = checksum::of(message);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    Some(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_echo_round_trips_and_damage_or_other_messages_are_refused() {
        let mut message = [0u8; HEADER_BYTES + 5];
        message[HEADER_BYTES..].copy_from_slice(b"hello");
        assert_eq!(
            write_echo_header(&mut message, kind::ECHO_REQUEST, 0x1234, 7, 5),
            Some(13)
        );
        let echo = parse_echo(&message).unwrap();
        assert_eq!(
            echo,
            Echo {
                kind: kind::ECHO_REQUEST,
                identifier: 0x1234,
                sequence: 7,
                data: b"hello"
            }
        );
        let mut damaged = message;
        damaged[9] ^= 0x40;
        assert_eq!(parse_echo(&damaged), None);
        let mut unreachable = message;
        unreachable[0] = 3;
        assert_eq!(parse_echo(&unreachable), None);
        assert_eq!(parse_echo(&message[..7]), None);
        assert_eq!(
            write_echo_header(&mut [0u8; 7], kind::ECHO_REPLY, 1, 1, 0),
            None
        );
    }
}

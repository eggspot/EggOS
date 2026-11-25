//! 802.1X's frames as a supplicant reads and writes them (D779): EAPOL (IEEE 802.1X-2020,
//! 11.3) carrying EAP packets (RFC 3748, section 4), and the framing EAP-TLS and PEAP put TLS
//! records in (RFC 5216 section 3.1; PEAP adds its version in the flags' low bits).

use alloc::vec::Vec;

const EAPOL_VERSION: u8 = 2;
/// EAPOL's packet type of an EAP packet.
pub const EAPOL_EAP_PACKET: u8 = 0;
/// EAPOL's packet type of EAPOL-Start.
pub const EAPOL_START: u8 = 1;
/// EAPOL's packet type of an EAPOL-Key frame.
pub const EAPOL_KEY: u8 = 3;
const EAPOL_HEADER_BYTES: usize = 4;
/// The bytes of an EAP packet's header: code, identifier and length.
pub const HEADER_BYTES: usize = 4;

/// EAP's codes.
pub mod code {
    pub const REQUEST: u8 = 1;
    pub const RESPONSE: u8 = 2;
    pub const SUCCESS: u8 = 3;
    pub const FAILURE: u8 = 4;
}

/// The EAP methods (types) the supplicant reads or names.
pub mod method {
    pub const IDENTITY: u8 = 1;
    pub const NOTIFICATION: u8 = 2;
    pub const NAK: u8 = 3;
    pub const TLS: u8 = 13;
    pub const PEAP: u8 = 25;
    pub const MSCHAPV2: u8 = 26;
    pub const EXTENSIONS: u8 = 33;
}

/// The TLS framing's flags.
pub mod flags {
    pub const LENGTH_INCLUDED: u8 = 0x80;
    pub const MORE_FRAGMENTS: u8 = 0x40;
    pub const START: u8 = 0x20;
    pub const PEAP_VERSION: u8 = 0x07;
}

/// One EAP packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub code: u8,
    pub identifier: u8,
    /// The method, for requests and responses.
    pub method: Option<u8>,
    /// What follows the method (or the header, for success and failure).
    pub data: &'a [u8],
}

/// The EAP packet `bytes` holds, its length checked against its header.
pub fn packet(bytes: &[u8]) -> Option<Packet<'_>> {
    let length = usize::from(u16::from_be_bytes([*bytes.get(2)?, *bytes.get(3)?]));
    if length < HEADER_BYTES || length > bytes.len() {
        return None;
    }
    let (code, identifier) = (bytes[0], bytes[1]);
    let body = &bytes[HEADER_BYTES..length];
    match code {
        code::REQUEST | code::RESPONSE => Some(Packet {
            code,
            identifier,
            method: Some(*body.first()?),
            data: &body[1..],
        }),
        code::SUCCESS | code::FAILURE => Some(Packet {
            code,
            identifier,
            method: None,
            data: body,
        }),
        _ => None,
    }
}

/// The EAPOL packet type of `eapol` (an EAPOL frame), if it is one.
pub fn eapol_type(eapol: &[u8]) -> Option<u8> {
    eapol.get(1).copied()
}

/// The EAP packet inside EAPOL frame `eapol`.
pub fn from_eapol(eapol: &[u8]) -> Option<Packet<'_>> {
    if eapol_type(eapol)? != EAPOL_EAP_PACKET {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([*eapol.get(2)?, *eapol.get(3)?]));
    packet(eapol.get(EAPOL_HEADER_BYTES..EAPOL_HEADER_BYTES + length)?)
}

/// An EAP packet's bytes.
pub fn write_packet(code: u8, identifier: u8, method: Option<u8>, data: &[u8]) -> Vec<u8> {
    let length = HEADER_BYTES + usize::from(method.is_some()) + data.len();
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(&[code, identifier]);
    bytes.extend_from_slice(&(length as u16).to_be_bytes());
    bytes.extend(method);
    bytes.extend_from_slice(data);
    bytes
}

/// `packet` (an EAP packet) in an EAPOL frame.
pub fn eapol(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(EAPOL_HEADER_BYTES + packet.len());
    frame.extend_from_slice(&[EAPOL_VERSION, EAPOL_EAP_PACKET]);
    frame.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    frame.extend_from_slice(packet);
    frame
}

/// The EAPOL-Start frame, which asks the authenticator to begin.
pub fn eapol_start() -> [u8; EAPOL_HEADER_BYTES] {
    [EAPOL_VERSION, EAPOL_START, 0, 0]
}

/// A response in an EAPOL frame.
pub fn response(identifier: u8, method: u8, data: &[u8]) -> Vec<u8> {
    eapol(&write_packet(code::RESPONSE, identifier, Some(method), data))
}

/// One TLS-framed message: its flags and the TLS bytes it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsMessage<'a> {
    pub flags: u8,
    pub data: &'a [u8],
}

/// The TLS framing of a request's or response's `data` (after the method).
pub fn tls_message(data: &[u8]) -> Option<TlsMessage<'_>> {
    let flags = *data.first()?;
    let start = match flags & flags::LENGTH_INCLUDED {
        0 => 1,
        _ => 5,
    };
    Some(TlsMessage {
        flags,
        data: data.get(start..)?,
    })
}

/// A TLS-framed response for `method` carrying `data`, with `flags` and, when
/// `total_length` is given, the length of the whole TLS message.
pub fn tls_response(
    identifier: u8,
    method: u8,
    flags: u8,
    total_length: Option<u32>,
    data: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + data.len());
    match total_length {
        Some(length) => {
            body.push(flags | flags::LENGTH_INCLUDED);
            body.extend_from_slice(&length.to_be_bytes());
        }
        None => body.push(flags & !flags::LENGTH_INCLUDED),
    }
    body.extend_from_slice(data);
    response(identifier, method, &body)
}

/// The response refusing a request's method and naming `wanted` instead.
pub fn nak(identifier: u8, wanted: u8) -> Vec<u8> {
    response(identifier, method::NAK, &[wanted])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_back_from_its_eapol_frame() {
        let frame = eapol(&write_packet(code::REQUEST, 7, Some(method::IDENTITY), b""));
        assert_eq!(frame, [2, 0, 0, 5, 1, 7, 0, 5, 1]);
        let read = from_eapol(&frame).unwrap();
        assert_eq!(read.code, code::REQUEST);
        assert_eq!(read.identifier, 7);
        assert_eq!(read.method, Some(method::IDENTITY));
        assert!(read.data.is_empty());
        let success = eapol(&write_packet(code::SUCCESS, 9, None, &[]));
        assert_eq!(from_eapol(&success).unwrap().method, None);
        assert!(from_eapol(&[2, 0, 0, 9, 1, 7, 0, 9, 1]).is_none());
    }

    #[test]
    fn tls_framing_carries_its_total_length_when_asked() {
        let frame = tls_response(3, method::TLS, flags::MORE_FRAGMENTS, Some(2000), b"ab");
        let read = from_eapol(&frame).unwrap();
        let message = tls_message(read.data).unwrap();
        assert_eq!(
            message.flags,
            flags::MORE_FRAGMENTS | flags::LENGTH_INCLUDED
        );
        assert_eq!(message.data, b"ab");
        let plain = tls_response(3, method::PEAP, 0, None, b"");
        assert_eq!(tls_message(from_eapol(&plain).unwrap().data).unwrap().data, b"");
    }
}

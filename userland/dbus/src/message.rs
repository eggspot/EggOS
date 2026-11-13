//! D-Bus messages: the fixed header, the header fields, and the body after them, aligned to 8.

use crate::wire::{self, Endian, Problem, Reader, Writer};
use alloc::vec::Vec;

/// The protocol version every message carries.
pub const PROTOCOL_VERSION: u8 = 1;
/// The most bytes a message may hold, header and body.
pub const MAX_MESSAGE_BYTES: usize = 1 << 27;
/// The bytes before the header fields: the fixed header and the fields' array length.
pub const FIXED_HEADER_BYTES: usize = 16;

/// The kinds of message.
pub mod kind {
    pub const METHOD_CALL: u8 = 1;
    pub const METHOD_RETURN: u8 = 2;
    pub const ERROR: u8 = 3;
    pub const SIGNAL: u8 = 4;
}

/// A message's flags.
pub mod flag {
    pub const NO_REPLY_EXPECTED: u8 = 1;
    pub const NO_AUTO_START: u8 = 2;
    pub const ALLOW_INTERACTIVE_AUTHORIZATION: u8 = 4;
}

mod field {
    pub const PATH: u8 = 1;
    pub const INTERFACE: u8 = 2;
    pub const MEMBER: u8 = 3;
    pub const ERROR_NAME: u8 = 4;
    pub const REPLY_SERIAL: u8 = 5;
    pub const DESTINATION: u8 = 6;
    pub const SENDER: u8 = 7;
    pub const SIGNATURE: u8 = 8;
    pub const UNIX_FDS: u8 = 9;
}

/// Why bytes are not a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    Endian,
    Version,
    Kind,
    TooLong,
    NoSerial,
    MissingField,
    Field,
    Wire(Problem),
}

impl From<Problem> for Malformed {
    fn from(problem: Problem) -> Self {
        Self::Wire(problem)
    }
}

/// A message's header fields; all optional on the wire, some required by its kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fields<'a> {
    pub path: Option<&'a [u8]>,
    pub interface: Option<&'a [u8]>,
    pub member: Option<&'a [u8]>,
    pub error_name: Option<&'a [u8]>,
    pub reply_serial: Option<u32>,
    pub destination: Option<&'a [u8]>,
    pub sender: Option<&'a [u8]>,
    pub signature: &'a [u8],
    pub unix_fds: u32,
}

/// A message read from bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    pub endian: Endian,
    pub kind: u8,
    pub flags: u8,
    pub serial: u32,
    pub fields: Fields<'a>,
    pub body: &'a [u8],
}

fn fixed_u32(bytes: &[u8], at: usize, endian: Endian) -> u32 {
    let word = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
    match endian {
        Endian::Little => u32::from_le_bytes(word),
        Endian::Big => u32::from_be_bytes(word),
    }
}

/// How many bytes the message starting `bytes` holds, once its first 16 have come; `None`
/// before then.
pub fn length(bytes: &[u8]) -> Result<Option<usize>, Malformed> {
    if bytes.len() < FIXED_HEADER_BYTES {
        return Ok(None);
    }
    let endian = Endian::from_flag(bytes[0]).ok_or(Malformed::Endian)?;
    if bytes[3] != PROTOCOL_VERSION {
        return Err(Malformed::Version);
    }
    let body = fixed_u32(bytes, 4, endian) as usize;
    let fields = fixed_u32(bytes, 12, endian) as usize;
    let total = (FIXED_HEADER_BYTES + fields).next_multiple_of(8) + body;
    match total <= MAX_MESSAGE_BYTES && body <= MAX_MESSAGE_BYTES && fields <= MAX_MESSAGE_BYTES {
        true => Ok(Some(total)),
        false => Err(Malformed::TooLong),
    }
}

impl<'a> Message<'a> {
    /// The message `bytes` holds, exactly.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Malformed> {
        let total = length(bytes)?.ok_or(Malformed::Wire(Problem::Truncated))?;
        if total != bytes.len() {
            return Err(Malformed::Wire(Problem::Truncated));
        }
        let endian = Endian::from_flag(bytes[0]).ok_or(Malformed::Endian)?;
        let kind = bytes[1];
        if !(kind::METHOD_CALL..=kind::SIGNAL).contains(&kind) {
            return Err(Malformed::Kind);
        }
        let flags = bytes[2];
        let body_length = fixed_u32(bytes, 4, endian) as usize;
        let serial = fixed_u32(bytes, 8, endian);
        if serial == 0 {
            return Err(Malformed::NoSerial);
        }
        let mut reader = Reader::new(bytes, endian, 12);
        let end = reader.array(8)?;
        let mut fields = Fields::default();
        while reader.position() < end {
            reader.align(8)?;
            let code = reader.byte()?;
            let signature = reader.signature()?;
            let expected: &[u8] = match code {
                field::PATH => b"o",
                field::INTERFACE | field::MEMBER | field::ERROR_NAME => b"s",
                field::DESTINATION | field::SENDER => b"s",
                field::REPLY_SERIAL | field::UNIX_FDS => b"u",
                field::SIGNATURE => b"g",
                _ => {
                    if signature.is_empty() || wire::type_end(signature, 0)? != signature.len() {
                        return Err(Malformed::Field);
                    }
                    reader.skip(signature)?;
                    continue;
                }
            };
            if signature != expected {
                return Err(Malformed::Field);
            }
            match code {
                field::PATH => fields.path = Some(reader.object_path()?),
                field::INTERFACE => fields.interface = Some(reader.string()?),
                field::MEMBER => fields.member = Some(reader.string()?),
                field::ERROR_NAME => fields.error_name = Some(reader.string()?),
                field::DESTINATION => fields.destination = Some(reader.string()?),
                field::SENDER => fields.sender = Some(reader.string()?),
                field::REPLY_SERIAL => fields.reply_serial = Some(reader.u32()?),
                field::UNIX_FDS => fields.unix_fds = reader.u32()?,
                _ => fields.signature = reader.signature()?,
            }
        }
        if reader.position() != end {
            return Err(Malformed::Field);
        }
        reader.align(8)?;
        let body = &bytes[reader.position()..];
        if body.len() != body_length {
            return Err(Malformed::Wire(Problem::Truncated));
        }
        let required = match kind {
            kind::METHOD_CALL => fields.path.is_some() && fields.member.is_some(),
            kind::METHOD_RETURN => fields.reply_serial.is_some(),
            kind::ERROR => fields.reply_serial.is_some() && fields.error_name.is_some(),
            _ => fields.path.is_some() && fields.interface.is_some() && fields.member.is_some(),
        };
        if !required {
            return Err(Malformed::MissingField);
        }
        Ok(Self {
            endian,
            kind,
            flags,
            serial,
            fields,
            body,
        })
    }

    /// Whether the sender waits for a reply.
    pub fn expects_reply(&self) -> bool {
        self.kind == kind::METHOD_CALL && self.flags & flag::NO_REPLY_EXPECTED == 0
    }

    /// A reader of the body, at its start.
    pub fn body_reader(&self) -> Reader<'a> {
        Reader::new(self.body, self.endian, 0)
    }

    /// The `index`th argument when it is a string or object path: what match rules' `argN`
    /// keys compare.
    pub fn string_argument(&self, index: usize) -> Option<(u8, &'a [u8])> {
        let signature = self.fields.signature;
        let mut reader = self.body_reader();
        let mut at = 0;
        for _ in 0..index {
            let end = wire::type_end(signature, at).ok()?;
            reader.skip(&signature[at..end]).ok()?;
            at = end;
        }
        let code = *signature.get(at)?;
        let value = match code {
            b's' => reader.string().ok()?,
            b'o' => reader.object_path().ok()?,
            _ => return None,
        };
        Some((code, value))
    }

    /// The same message with its sender set to `sender`, as the bus passes it on.
    pub fn with_sender(&self, sender: &[u8]) -> Vec<u8> {
        let mut fields = self.fields;
        fields.sender = Some(sender);
        build(self.endian, self.kind, self.flags, self.serial, &fields, self.body)
    }
}

fn string_field(writer: &mut Writer, code: u8, signature: &[u8], value: &[u8]) {
    writer.align(8);
    writer.byte(code);
    writer.signature(signature);
    writer.string(value);
}

fn number_field(writer: &mut Writer, code: u8, value: u32) {
    writer.align(8);
    writer.byte(code);
    writer.signature(b"u");
    writer.u32(value);
}

/// A message of `kind` with `flags`, `serial`, `fields` and `body` (already in `endian`
/// order, its signature in `fields`).
pub fn build(endian: Endian, kind: u8, flags: u8, serial: u32, fields: &Fields<'_>, body: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(endian);
    writer.byte(endian.flag());
    writer.byte(kind);
    writer.byte(flags);
    writer.byte(PROTOCOL_VERSION);
    writer.u32(body.len() as u32);
    writer.u32(serial);
    let array = writer.begin_array(8);
    let strings = [
        (field::PATH, &b"o"[..], fields.path),
        (field::INTERFACE, b"s", fields.interface),
        (field::MEMBER, b"s", fields.member),
        (field::ERROR_NAME, b"s", fields.error_name),
        (field::DESTINATION, b"s", fields.destination),
        (field::SENDER, b"s", fields.sender),
    ];
    for (code, signature, value) in strings {
        if let Some(value) = value {
            string_field(&mut writer, code, signature, value);
        }
    }
    if let Some(reply_serial) = fields.reply_serial {
        number_field(&mut writer, field::REPLY_SERIAL, reply_serial);
    }
    if !fields.signature.is_empty() {
        writer.align(8);
        writer.byte(field::SIGNATURE);
        writer.signature(b"g");
        writer.signature(fields.signature);
    }
    if fields.unix_fds != 0 {
        number_field(&mut writer, field::UNIX_FDS, fields.unix_fds);
    }
    writer.end_array(array);
    writer.align(8);
    writer.raw(body);
    writer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Vec<u8> {
        let fields = Fields {
            path: Some(b"/org/freedesktop/DBus"),
            interface: Some(b"org.freedesktop.DBus"),
            member: Some(b"Hello"),
            destination: Some(b"org.freedesktop.DBus"),
            ..Fields::default()
        };
        build(Endian::Little, kind::METHOD_CALL, 0, 1, &fields, &[])
    }

    #[test]
    fn a_built_message_parses_back_and_its_length_is_known_from_16_bytes() {
        let bytes = hello();
        assert_eq!(length(&bytes[..15]), Ok(None));
        assert_eq!(length(&bytes[..16]), Ok(Some(bytes.len())));
        let message = Message::parse(&bytes).unwrap();
        assert_eq!(message.fields.member, Some(&b"Hello"[..]));
        assert_eq!(message.serial, 1);
        assert!(message.expects_reply());
    }

    #[test]
    fn the_sender_is_added_and_the_body_kept_in_both_orders() {
        for endian in [Endian::Little, Endian::Big] {
            let mut body = Writer::new(endian);
            body.string(b"org.example.App");
            body.u32(4);
            let fields = Fields {
                path: Some(b"/"),
                member: Some(b"RequestName"),
                signature: b"su",
                ..Fields::default()
            };
            let bytes = build(endian, kind::METHOD_CALL, 0, 9, &fields, body.bytes());
            let message = Message::parse(&bytes).unwrap();
            let sent = message.with_sender(b":1.4");
            let again = Message::parse(&sent).unwrap();
            assert_eq!(again.fields.sender, Some(&b":1.4"[..]));
            assert_eq!(again.string_argument(0), Some((b's', &b"org.example.App"[..])));
            assert_eq!(again.string_argument(1), None);
            assert_eq!(again.body, message.body);
        }
    }

    #[test]
    fn a_call_without_a_member_or_a_zero_serial_is_refused() {
        let fields = Fields {
            path: Some(b"/"),
            ..Fields::default()
        };
        let bytes = build(Endian::Little, kind::METHOD_CALL, 0, 1, &fields, &[]);
        assert_eq!(Message::parse(&bytes), Err(Malformed::MissingField));
        let mut bytes = hello();
        bytes[8..12].fill(0);
        assert_eq!(Message::parse(&bytes), Err(Malformed::NoSerial));
    }
}

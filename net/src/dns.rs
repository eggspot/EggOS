//! DNS messages (RFC 1035) for a stub resolver: a query for one name's addresses, and what the
//! answer says (its IPv4 addresses and the canonical name an alias leads to), names compressed
//! or not. Also the `nameserver` lines of `/etc/resolv.conf`, and the answer a stand-in server
//! gives, for tests.

use crate::ipv4::Ipv4Address;

/// The servers' UDP port.
pub const PORT: u16 = 53;
/// The largest message over UDP without extensions.
pub const MAX_MESSAGE_BYTES: usize = 512;
/// The most addresses an answer keeps.
pub const MAX_ADDRESSES: usize = 8;
/// The most bytes a name takes on the wire.
pub const MAX_NAME_BYTES: usize = 255;
/// The most `nameserver` lines read, as glibc's and musl's `MAXNS`.
pub const MAX_NAMESERVERS: usize = 3;

/// The record types asked for or read.
pub mod record_type {
    /// An IPv4 address.
    pub const A: u16 = 1;
    /// An alias's canonical name.
    pub const CNAME: u16 = 5;
}

const CLASS_IN: u16 = 1;
const HEADER_BYTES: usize = 12;
const RECURSION_DESIRED: u16 = 0x0100;
const RESPONSE: u16 = 0x8000;
const TRUNCATED: u16 = 0x0200;
const RECURSION_AVAILABLE: u16 = 0x0080;
const POINTER: u8 = 0xc0;
const MAX_JUMPS: usize = 32;

/// Why a query could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryError {
    /// The name is empty or has an empty label (two dots in a row).
    EmptyLabel,
    /// A label is longer than 63 bytes.
    LabelTooLong,
    /// The name is longer than [`MAX_NAME_BYTES`] on the wire.
    NameTooLong,
    /// The buffer is too small.
    BufferTooSmall,
}

/// Writes a recursive query with id `id` for `name`'s records of type `record_type` to the
/// start of `out`: its length. A final dot on `name` is allowed.
pub fn write_query(out: &mut [u8], id: u16, name: &[u8], record_type: u16) -> Result<usize, QueryError> {
    let name = name.strip_suffix(b".").unwrap_or(name);
    if name.is_empty() {
        return Err(QueryError::EmptyLabel);
    }
    if name.len() + 2 > MAX_NAME_BYTES {
        return Err(QueryError::NameTooLong);
    }
    let length = HEADER_BYTES + name.len() + 2 + 4;
    let message = out.get_mut(..length).ok_or(QueryError::BufferTooSmall)?;
    message.fill(0);
    message[0..2].copy_from_slice(&id.to_be_bytes());
    message[2..4].copy_from_slice(&RECURSION_DESIRED.to_be_bytes());
    message[4..6].copy_from_slice(&1u16.to_be_bytes());
    let mut at = HEADER_BYTES;
    for label in name.split(|&byte| byte == b'.') {
        if label.is_empty() {
            return Err(QueryError::EmptyLabel);
        }
        if label.len() > 63 {
            return Err(QueryError::LabelTooLong);
        }
        message[at] = label.len() as u8;
        message[at + 1..at + 1 + label.len()].copy_from_slice(label);
        at += 1 + label.len();
    }
    message[at] = 0;
    message[at + 1..at + 3].copy_from_slice(&record_type.to_be_bytes());
    message[at + 3..at + 5].copy_from_slice(&CLASS_IN.to_be_bytes());
    Ok(length)
}

/// Why an answer gives no addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseError {
    /// It is not a well-formed message.
    Malformed,
    /// It is not the answer to our query: another id, or not a response.
    NotOurs,
    /// The name does not exist (`NXDOMAIN`).
    NameNotFound,
    /// The server failed (`SERVFAIL`).
    ServerFailure,
    /// The server refused to answer (`REFUSED`).
    Refused,
    /// Another error code.
    Other(u8),
    /// The answer did not fit in a datagram.
    Truncated,
}

impl ResponseError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Malformed => "the server's answer is malformed",
            Self::NotOurs => "the answer is not for this query",
            Self::NameNotFound => "no such name",
            Self::ServerFailure => "the server failed",
            Self::Refused => "the server refused to answer",
            Self::Other(_) => "the server answered with an error",
            Self::Truncated => "the answer was too long for one datagram",
        }
    }
}

/// A domain name read from a message, labels joined by dots, without the final dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; MAX_NAME_BYTES],
    length: u8,
}

impl Name {
    const fn empty() -> Self {
        Self {
            bytes: [0; MAX_NAME_BYTES],
            length: 0,
        }
    }

    /// Its text.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    fn push(&mut self, bytes: &[u8]) -> Option<()> {
        let at = usize::from(self.length);
        let end = at.checked_add(bytes.len()).filter(|&end| end <= MAX_NAME_BYTES)?;
        self.bytes[at..end].copy_from_slice(bytes);
        self.length = end as u8;
        Some(())
    }
}

/// What an answer says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answers {
    addresses: [Ipv4Address; MAX_ADDRESSES],
    count: usize,
    /// The canonical name the query's name is an alias of, if the answer says.
    pub canonical: Option<Name>,
}

impl Answers {
    /// The IPv4 addresses, in the answer's order, at most [`MAX_ADDRESSES`].
    pub fn addresses(&self) -> &[Ipv4Address] {
        &self.addresses[..self.count]
    }
}

fn word(message: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*message.get(at)?, *message.get(at + 1)?]))
}

fn read_name(message: &[u8], start: usize, mut name: Option<&mut Name>) -> Option<usize> {
    let mut at = start;
    let mut end = None;
    for _ in 0..MAX_JUMPS {
        loop {
            let length = *message.get(at)?;
            match length {
                0 => return Some(end.unwrap_or(at + 1)),
                length if length & POINTER == POINTER => {
                    let target = usize::from(word(message, at)? & 0x3fff);
                    end.get_or_insert(at + 2);
                    at = target;
                    break;
                }
                length if length & POINTER != 0 => return None,
                length => {
                    let label = message.get(at + 1..at + 1 + usize::from(length))?;
                    if let Some(name) = name.as_deref_mut() {
                        if name.length > 0 {
                            name.push(b".")?;
                        }
                        name.push(label)?;
                    }
                    at += 1 + usize::from(length);
                }
            }
        }
    }
    None
}

/// The answer `message` holds to the query with id `id`.
pub fn parse_response(message: &[u8], id: u16) -> Result<Answers, ResponseError> {
    let header = |at| word(message, at).ok_or(ResponseError::Malformed);
    if header(0)? != id || header(2)? & RESPONSE == 0 {
        return Err(ResponseError::NotOurs);
    }
    let flags = header(2)?;
    match (flags & 0x000f) as u8 {
        0 => {}
        3 => return Err(ResponseError::NameNotFound),
        2 => return Err(ResponseError::ServerFailure),
        5 => return Err(ResponseError::Refused),
        code => return Err(ResponseError::Other(code)),
    }
    if flags & TRUNCATED != 0 {
        return Err(ResponseError::Truncated);
    }
    let questions = header(4)?;
    let answers = header(6)?;
    let mut at = HEADER_BYTES;
    for _ in 0..questions {
        at = read_name(message, at, None).ok_or(ResponseError::Malformed)? + 4;
    }
    let mut parsed = Answers {
        addresses: [Ipv4Address::UNSPECIFIED; MAX_ADDRESSES],
        count: 0,
        canonical: None,
    };
    for _ in 0..answers {
        at = read_name(message, at, None).ok_or(ResponseError::Malformed)?;
        let kind = header(at)?;
        let class = header(at + 2)?;
        let length = usize::from(header(at + 8)?);
        let data_at = at + 10;
        let data = message
            .get(data_at..data_at + length)
            .ok_or(ResponseError::Malformed)?;
        match (kind, class) {
            (record_type::A, CLASS_IN) if length == 4 && parsed.count < MAX_ADDRESSES => {
                parsed.addresses[parsed.count] = Ipv4Address([data[0], data[1], data[2], data[3]]);
                parsed.count += 1;
            }
            (record_type::CNAME, CLASS_IN) => {
                let mut name = Name::empty();
                read_name(message, data_at, Some(&mut name)).ok_or(ResponseError::Malformed)?;
                parsed.canonical = Some(name);
            }
            _ => {}
        }
        at = data_at + length;
    }
    Ok(parsed)
}

/// The addresses of the first [`MAX_NAMESERVERS`] `nameserver` lines of a `resolv.conf`
/// whose address is IPv4; lines starting with `#` or `;` are notes.
pub fn nameservers(resolv_conf: &[u8]) -> [Option<Ipv4Address>; MAX_NAMESERVERS] {
    let mut servers = [None; MAX_NAMESERVERS];
    let mut found = 0;
    for line in resolv_conf.split(|&byte| byte == b'\n') {
        let mut words = line
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|word| !word.is_empty());
        if words.next() != Some(&b"nameserver"[..]) {
            continue;
        }
        if let Some(address) = words.next().and_then(Ipv4Address::parse)
            && found < MAX_NAMESERVERS
        {
            servers[found] = Some(address);
            found += 1;
        }
    }
    servers
}

/// Writes the answer a server gives to `query` to the start of `out`: the query's question
/// and an `A` record for each of `addresses` with time to live `ttl`, or `NXDOMAIN` when there
/// are none; its length, or `None` when `query` has no question or `out` is too small.
pub fn write_answer(out: &mut [u8], query: &[u8], addresses: &[Ipv4Address], ttl: u32) -> Option<usize> {
    let question_end = read_name(query, HEADER_BYTES, None)? + 4;
    let question = query.get(..question_end)?;
    let length = question_end + addresses.len() * 16;
    let message = out.get_mut(..length)?;
    message[..question_end].copy_from_slice(question);
    let code = if addresses.is_empty() { 3 } else { 0 };
    let flags = RESPONSE | RECURSION_DESIRED | RECURSION_AVAILABLE | code;
    message[2..4].copy_from_slice(&flags.to_be_bytes());
    message[4..6].copy_from_slice(&1u16.to_be_bytes());
    message[6..8].copy_from_slice(&(addresses.len() as u16).to_be_bytes());
    message[8..12].fill(0);
    let mut at = question_end;
    for address in addresses {
        let record = &mut message[at..at + 16];
        record[0..2].copy_from_slice(&[POINTER, HEADER_BYTES as u8]);
        record[2..4].copy_from_slice(&record_type::A.to_be_bytes());
        record[4..6].copy_from_slice(&CLASS_IN.to_be_bytes());
        record[6..10].copy_from_slice(&ttl.to_be_bytes());
        record[10..12].copy_from_slice(&4u16.to_be_bytes());
        record[12..16].copy_from_slice(&address.0);
        at += 16;
    }
    Some(length)
}

/// The name a query asks about, labels joined by dots; `None` for a malformed query.
pub fn query_name(query: &[u8]) -> Option<Name> {
    let mut name = Name::empty();
    read_name(query, HEADER_BYTES, Some(&mut name))?;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_laid_out_as_rfc_1035_says() {
        let mut query = [0u8; MAX_MESSAGE_BYTES];
        let length = write_query(&mut query, 0xbeef, b"example.com.", record_type::A).unwrap();
        assert_eq!(
            &query[..length],
            b"\xbe\xef\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01"
        );
        assert_eq!(query_name(&query[..length]).unwrap().as_bytes(), b"example.com");
        assert_eq!(write_query(&mut query, 1, b"a..b", 1), Err(QueryError::EmptyLabel));
        assert_eq!(write_query(&mut query, 1, b"", 1), Err(QueryError::EmptyLabel));
        assert_eq!(write_query(&mut query, 1, &[b'a'; 64], 1), Err(QueryError::LabelTooLong));
        assert_eq!(write_query(&mut query, 1, &[b'a'; 300], 1), Err(QueryError::NameTooLong));
        assert_eq!(write_query(&mut [0u8; 20], 1, b"example.com", 1), Err(QueryError::BufferTooSmall));
    }

    #[test]
    fn an_answer_gives_its_addresses_and_errors_are_named() {
        let mut query = [0u8; MAX_MESSAGE_BYTES];
        let length = write_query(&mut query, 7, b"eggos.test", record_type::A).unwrap();
        let addresses = [Ipv4Address::new(10, 0, 2, 2), Ipv4Address::new(192, 0, 2, 1)];
        let mut answer = [0u8; MAX_MESSAGE_BYTES];
        let answered = write_answer(&mut answer, &query[..length], &addresses, 60).unwrap();
        let parsed = parse_response(&answer[..answered], 7).unwrap();
        assert_eq!(parsed.addresses(), &addresses);
        assert_eq!(parsed.canonical, None);
        assert_eq!(parse_response(&answer[..answered], 8), Err(ResponseError::NotOurs));
        assert_eq!(parse_response(&query[..length], 7), Err(ResponseError::NotOurs));
        assert_eq!(parse_response(&answer[..answered - 1], 7), Err(ResponseError::Malformed));
        let missing = write_answer(&mut answer, &query[..length], &[], 60).unwrap();
        assert_eq!(parse_response(&answer[..missing], 7), Err(ResponseError::NameNotFound));
    }

    #[test]
    fn an_alias_s_canonical_name_follows_compression_pointers_and_loops_are_refused() {
        let mut message = Vec::new();
        message.extend_from_slice(&[0, 9, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0]);
        message.extend_from_slice(b"\x03www\x05eggos\x04test\x00\x00\x01\x00\x01");
        message.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 6]);
        message.extend_from_slice(b"\x03web\xc0\x10");
        message.extend_from_slice(&[0xc0, 44, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 10, 0, 2, 9]);
        let parsed = parse_response(&message, 9).unwrap();
        assert_eq!(parsed.canonical.unwrap().as_bytes(), b"web.eggos.test");
        assert_eq!(parsed.addresses(), &[Ipv4Address::new(10, 0, 2, 9)]);
        let mut looping = message.clone();
        looping[12] = 0xc0;
        looping[13] = 12;
        assert_eq!(parse_response(&looping, 9), Err(ResponseError::Malformed));
    }

    #[test]
    fn nameservers_are_the_first_three_ipv4_lines() {
        let conf = b"# from DHCP\nsearch lan\nnameserver 10.0.2.3\nnameserver ::1\n\
                     nameserver\t192.0.2.53\nnameserver 192.0.2.54\nnameserver 192.0.2.55\n";
        assert_eq!(
            nameservers(conf),
            [
                Some(Ipv4Address::new(10, 0, 2, 3)),
                Some(Ipv4Address::new(192, 0, 2, 53)),
                Some(Ipv4Address::new(192, 0, 2, 54)),
            ]
        );
        assert_eq!(nameservers(b""), [None; 3]);
    }
}

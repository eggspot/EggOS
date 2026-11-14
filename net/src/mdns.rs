//! Multicast DNS service advertisement (RFC 6762, DNS-SD RFC 6763; D577): the records that
//! say a service of some type runs on this machine, at which name, address and port, sent
//! unasked when it starts (announcements) and in answer to the questions on the link that ask
//! for them. Names are written whole, never compressed; questions may be compressed.

use crate::ipv4::Ipv4Address;

/// The port mDNS speaks on.
pub const PORT: u16 = 5353;
/// The IPv4 group mDNS speaks to.
pub const GROUP: Ipv4Address = Ipv4Address::new(224, 0, 0, 251);
/// The most bytes of one mDNS message this module writes or reads.
pub const MAX_MESSAGE_BYTES: usize = 1500 - 20 - 8;
/// How long the shared records (`PTR`) are kept, in seconds.
pub const SHARED_TTL: u32 = 4500;
/// How long the records naming this host (`SRV`, `A`) are kept, in seconds.
pub const HOST_TTL: u32 = 120;

const SERVICES_NAME: &[&[u8]] = &[b"_services", b"_dns-sd", b"_udp", b"local"];
const LOCAL: &[u8] = b"local";
const CACHE_FLUSH: u16 = 0x8000;
const CLASS_IN: u16 = 1;
const UNICAST_RESPONSE: u16 = 0x8000;
const RESPONSE_FLAGS: u16 = 0x8400;
const MAX_POINTERS: usize = 16;

/// Record types.
pub mod kind {
    pub const A: u16 = 1;
    pub const PTR: u16 = 12;
    pub const TXT: u16 = 16;
    pub const SRV: u16 = 33;
    pub const ANY: u16 = 255;
}

/// One service this machine offers.
#[derive(Debug, Clone, Copy)]
pub struct Service<'a> {
    /// Its instance's label, what people see (`eggos`).
    pub instance: &'a [u8],
    /// Its type's two labels (`_mcp`, `_tcp`).
    pub service_type: [&'a [u8]; 2],
    /// The host's label (`eggos`, for `eggos.local`).
    pub host: &'a [u8],
    /// The host's address.
    pub address: Ipv4Address,
    /// The port it listens on.
    pub port: u16,
    /// Its `TXT` strings (`path=/mcp`).
    pub text: &'a [&'a [u8]],
}

/// What questions asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Asked {
    /// The service type's instances (`PTR _mcp._tcp.local`).
    pub instances: bool,
    /// The types on the link (`PTR _services._dns-sd._udp.local`).
    pub types: bool,
    /// The instance's `SRV` and `TXT`.
    pub instance: bool,
    /// The host's address.
    pub host: bool,
    /// Whether a question asked for a unicast answer (the `QU` bit).
    pub unicast: bool,
}

impl Asked {
    /// Whether any question was this service's.
    pub fn any(&self) -> bool {
        self.instances || self.types || self.instance || self.host
    }
}

struct Writer<'o> {
    out: &'o mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, bytes: &[u8]) -> Option<()> {
        let place = self.out.get_mut(self.length..self.length + bytes.len())?;
        place.copy_from_slice(bytes);
        self.length += bytes.len();
        Some(())
    }

    fn u16(&mut self, value: u16) -> Option<()> {
        self.bytes(&value.to_be_bytes())
    }

    fn u32(&mut self, value: u32) -> Option<()> {
        self.bytes(&value.to_be_bytes())
    }

    fn name(&mut self, labels: &[&[u8]]) -> Option<()> {
        for label in labels {
            if label.is_empty() || label.len() > 63 {
                return None;
            }
            self.bytes(&[label.len() as u8])?;
            self.bytes(label)?;
        }
        self.bytes(&[0])
    }

    fn record(&mut self, owner: &[&[u8]], kind: u16, flush: bool, ttl: u32) -> Option<usize> {
        self.name(owner)?;
        self.u16(kind)?;
        self.u16(CLASS_IN | if flush { CACHE_FLUSH } else { 0 })?;
        self.u32(ttl)?;
        self.u16(0)?;
        Some(self.length)
    }

    fn end_record(&mut self, data_start: usize) -> Option<()> {
        let length = u16::try_from(self.length - data_start).ok()?;
        self.out.get_mut(data_start - 2..data_start)?.copy_from_slice(&length.to_be_bytes());
        Some(())
    }
}

fn service_name<'a>(service: &Service<'a>) -> [&'a [u8]; 3] {
    [service.service_type[0], service.service_type[1], LOCAL]
}

fn instance_name<'a>(service: &Service<'a>) -> [&'a [u8]; 4] {
    [service.instance, service.service_type[0], service.service_type[1], LOCAL]
}

fn host_name<'a>(service: &Service<'a>) -> [&'a [u8]; 2] {
    [service.host, LOCAL]
}

/// Writes a response carrying the records `asked` names, with every record's time to live
/// `ttl` scaled (0 for a goodbye) and `id` (0 but for a legacy query's answer); the bytes
/// written, or `None` when nothing was asked or it does not fit.
pub fn write_response(service: &Service<'_>, asked: Asked, id: u16, goodbye: bool, out: &mut [u8]) -> Option<usize> {
    if !asked.any() {
        return None;
    }
    let ttl = |seconds: u32| if goodbye { 0 } else { seconds };
    let mut writer = Writer { out, length: 0 };
    let answers = [asked.types, asked.instances, asked.instance, asked.instance, asked.host]
        .iter()
        .filter(|&&wanted| wanted)
        .count() as u16;
    writer.u16(id)?;
    writer.u16(RESPONSE_FLAGS)?;
    writer.u16(0)?;
    writer.u16(answers)?;
    writer.u16(0)?;
    writer.u16(0)?;
    if asked.types {
        let start = writer.record(SERVICES_NAME, kind::PTR, false, ttl(SHARED_TTL))?;
        writer.name(&service_name(service))?;
        writer.end_record(start)?;
    }
    if asked.instances {
        let start = writer.record(&service_name(service), kind::PTR, false, ttl(SHARED_TTL))?;
        writer.name(&instance_name(service))?;
        writer.end_record(start)?;
    }
    if asked.instance {
        let start = writer.record(&instance_name(service), kind::SRV, true, ttl(HOST_TTL))?;
        writer.u16(0)?;
        writer.u16(0)?;
        writer.u16(service.port)?;
        writer.name(&host_name(service))?;
        writer.end_record(start)?;
        let start = writer.record(&instance_name(service), kind::TXT, true, ttl(SHARED_TTL))?;
        if service.text.is_empty() {
            writer.bytes(&[0])?;
        }
        for string in service.text {
            let length = u8::try_from(string.len()).ok()?;
            writer.bytes(&[length])?;
            writer.bytes(string)?;
        }
        writer.end_record(start)?;
    }
    if asked.host {
        let start = writer.record(&host_name(service), kind::A, true, ttl(HOST_TTL))?;
        writer.bytes(&service.address.0)?;
        writer.end_record(start)?;
    }
    Some(writer.length)
}

/// Everything an announcement says.
pub const ANNOUNCED: Asked = Asked {
    instances: true,
    types: false,
    instance: true,
    host: true,
    unicast: false,
};

fn name_matches(message: &[u8], mut at: usize, labels: &[&[u8]]) -> Option<(bool, usize)> {
    let mut end = None;
    let mut matched = true;
    let mut index = 0;
    for _ in 0..MAX_POINTERS + 128 {
        let length = *message.get(at)? as usize;
        if length & 0xc0 == 0xc0 {
            let target = ((length & 0x3f) << 8) | *message.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            if target >= at {
                return None;
            }
            at = target;
            continue;
        }
        if length == 0 {
            let end = end.unwrap_or(at + 1);
            return Some((matched && index == labels.len(), end));
        }
        let label = message.get(at + 1..at + 1 + length)?;
        matched &= labels.get(index).is_some_and(|wanted| wanted.eq_ignore_ascii_case(label));
        index += 1;
        at += 1 + length;
    }
    None
}

/// What the questions of the query `message` ask of `service`, and the end of its question
/// section; `None` for a message that is no query or cannot be read.
pub fn questions(service: &Service<'_>, message: &[u8]) -> Option<(Asked, usize)> {
    let header: &[u8; 12] = message.get(..12)?.try_into().ok()?;
    let flags = u16::from_be_bytes([header[2], header[3]]);
    if flags & 0x8000 != 0 || flags & 0x7800 != 0 {
        return None;
    }
    let count = u16::from_be_bytes([header[4], header[5]]);
    let mut asked = Asked::default();
    let mut at = 12;
    for _ in 0..count {
        let (is_service, end) = name_matches(message, at, &service_name(service))?;
        let (is_types, _) = name_matches(message, at, SERVICES_NAME)?;
        let (is_instance, _) = name_matches(message, at, &instance_name(service))?;
        let (is_host, _) = name_matches(message, at, &host_name(service))?;
        let fixed = message.get(end..end + 4)?;
        let kind = u16::from_be_bytes([fixed[0], fixed[1]]);
        let class = u16::from_be_bytes([fixed[2], fixed[3]]);
        at = end + 4;
        if class & !UNICAST_RESPONSE != CLASS_IN && class & !UNICAST_RESPONSE != kind::ANY {
            continue;
        }
        let is = |wanted: u16| kind == wanted || kind == kind::ANY;
        let before = asked;
        asked.instances |= is_service && is(kind::PTR);
        asked.types |= is_types && is(kind::PTR);
        asked.instance |= is_instance && (is(kind::SRV) || is(kind::TXT));
        asked.host |= is_host && is(kind::A);
        if asked != before && class & UNICAST_RESPONSE != 0 {
            asked.unicast = true;
        }
    }
    if asked.instances || asked.instance {
        asked.host = true;
    }
    Some((asked, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: Service<'static> = Service {
        instance: b"eggos",
        service_type: [b"_mcp", b"_tcp"],
        host: b"eggos",
        address: Ipv4Address::new(192, 168, 1, 20),
        port: 8443,
        text: &[b"path=/mcp"],
    };

    fn query(name: &[&[u8]], kind: u16, unicast: bool) -> Vec<u8> {
        let mut message = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name {
            message.push(label.len() as u8);
            message.extend_from_slice(label);
        }
        message.push(0);
        message.extend_from_slice(&kind.to_be_bytes());
        let class = CLASS_IN | if unicast { UNICAST_RESPONSE } else { 0 };
        message.extend_from_slice(&class.to_be_bytes());
        message
    }

    #[test]
    fn a_browse_for_the_type_asks_for_the_instance_and_its_host() {
        let message = query(&[b"_MCP", b"_tcp", b"local"], kind::PTR, true);
        let (asked, end) = questions(&SERVICE, &message).unwrap();
        assert_eq!(end, message.len());
        assert!(asked.instances && asked.host && asked.unicast && !asked.types);
        let other = query(&[b"_http", b"_tcp", b"local"], kind::PTR, false);
        assert!(!questions(&SERVICE, &other).unwrap().0.any());
    }

    #[test]
    fn the_announcement_holds_ptr_srv_txt_and_a_with_their_lengths_right() {
        let mut out = [0u8; MAX_MESSAGE_BYTES];
        let length = write_response(&SERVICE, ANNOUNCED, 0, false, &mut out).unwrap();
        let message = &out[..length];
        assert_eq!(&message[..12], &[0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0]);
        let mut at = 12;
        let mut kinds = Vec::new();
        for _ in 0..4 {
            let (_, end) = name_matches(message, at, &[]).unwrap();
            kinds.push(u16::from_be_bytes([message[end], message[end + 1]]));
            let data = u16::from_be_bytes([message[end + 8], message[end + 9]]) as usize;
            at = end + 10 + data;
        }
        assert_eq!(at, length);
        assert_eq!(kinds, [kind::PTR, kind::SRV, kind::TXT, kind::A]);
        assert_eq!(&message[length - 4..], &[192, 168, 1, 20]);
    }

    #[test]
    fn a_compressed_question_and_a_response_are_read_right() {
        let mut message = query(&[b"_mcp", b"_tcp", b"local"], kind::PTR, false);
        message[5] = 2;
        message.extend_from_slice(&[5, b'e', b'g', b'g', b'o', b's', 0xc0, 12]);
        message.extend_from_slice(&kind::SRV.to_be_bytes());
        message.extend_from_slice(&CLASS_IN.to_be_bytes());
        let (asked, end) = questions(&SERVICE, &message).unwrap();
        assert!(asked.instances && asked.instance);
        assert_eq!(end, message.len());
        let mut response = message.clone();
        response[2] = 0x84;
        assert_eq!(questions(&SERVICE, &response), None);
    }
}

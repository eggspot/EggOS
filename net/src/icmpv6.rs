//! ICMPv6 (RFC 4443) and its neighbour discovery messages (RFC 4861, D565): echo, router
//! and neighbour solicitations and advertisements, with the options the stack reads.

use crate::checksum;
use crate::ethernet::MacAddress;
use crate::ipv6::{self, Ipv6Address, next_header};

/// Bytes of the type, code and checksum.
pub const HEADER_BYTES: usize = 4;

/// The message types the stack speaks.
pub mod kind {
    /// Echo request.
    pub const ECHO_REQUEST: u8 = 128;
    /// Echo reply.
    pub const ECHO_REPLY: u8 = 129;
    /// Router solicitation.
    pub const ROUTER_SOLICITATION: u8 = 133;
    /// Router advertisement.
    pub const ROUTER_ADVERTISEMENT: u8 = 134;
    /// Neighbour solicitation.
    pub const NEIGHBOUR_SOLICITATION: u8 = 135;
    /// Neighbour advertisement.
    pub const NEIGHBOUR_ADVERTISEMENT: u8 = 136;
}

const OPTION_SOURCE_LINK: u8 = 1;
const OPTION_TARGET_LINK: u8 = 2;
const OPTION_PREFIX: u8 = 3;
const OPTION_MTU: u8 = 5;
const ADVERTISEMENT_SOLICITED: u8 = 0x40;
const ADVERTISEMENT_OVERRIDE: u8 = 0x20;
const PREFIX_ON_LINK: u8 = 0x80;
const PREFIX_AUTONOMOUS: u8 = 0x40;

/// The checksum `message` (whose checksum field is zero) has between `source` and
/// `destination`.
pub fn checksum_of(source: Ipv6Address, destination: Ipv6Address, message: &[u8]) -> u16 {
    let pseudo = ipv6::pseudo_header(source, destination, next_header::ICMPV6, message.len());
    checksum::finish(checksum::add(pseudo, message))
}

/// Whether `message` holds a correct checksum between `source` and `destination`.
pub fn checks(source: Ipv6Address, destination: Ipv6Address, message: &[u8]) -> bool {
    let pseudo = ipv6::pseudo_header(source, destination, next_header::ICMPV6, message.len());
    message.len() >= HEADER_BYTES && checksum::finish(checksum::add(pseudo, message)) == 0
}

/// Writes the checksum of the message in `out` between `source` and `destination`.
pub fn seal(source: Ipv6Address, destination: Ipv6Address, out: &mut [u8]) {
    out[2..4].fill(0);
    let sum = checksum_of(source, destination, out);
    out[2..4].copy_from_slice(&sum.to_be_bytes());
}

/// A prefix a router advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    /// The prefix.
    pub prefix: Ipv6Address,
    /// Its length in bits.
    pub length: u8,
    /// Its addresses are on the link.
    pub on_link: bool,
    /// Hosts make their own addresses in it (SLAAC).
    pub autonomous: bool,
    /// How long it stays valid, in seconds.
    pub valid_seconds: u32,
}

/// A router advertisement's contents the stack uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouterAdvertisement {
    /// How long the router is a default router, in seconds; 0 for not one.
    pub lifetime_seconds: u16,
    /// The router's link-layer address.
    pub link: Option<MacAddress>,
    /// The link's MTU.
    pub mtu: Option<u32>,
    /// The first prefix it advertises for SLAAC.
    pub prefix: Option<Prefix>,
}

/// What a neighbour message says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neighbour {
    /// The address asked about or advertised.
    pub target: Ipv6Address,
    /// The link-layer address given with it.
    pub link: Option<MacAddress>,
    /// An advertisement answers a solicitation.
    pub solicited: bool,
}

/// What an ICMPv6 message that came for the stack is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message<'a> {
    /// An echo request: identifier, sequence and data, to answer.
    EchoRequest { identifier: u16, sequence: u16, data: &'a [u8] },
    /// An echo reply.
    EchoReply { identifier: u16, sequence: u16, data: &'a [u8] },
    /// A router advertisement.
    RouterAdvertisement(RouterAdvertisement),
    /// A neighbour solicitation.
    NeighbourSolicitation(Neighbour),
    /// A neighbour advertisement.
    NeighbourAdvertisement(Neighbour),
    /// Something else.
    Other,
}

/// The message `bytes` carries from `source` to `destination`: `None` when its checksum is
/// wrong, or for a neighbour discovery message whose hop limit is not 255 (it came from
/// off the link).
pub fn parse(
    source: Ipv6Address,
    destination: Ipv6Address,
    hop_limit: u8,
    bytes: &[u8],
) -> Option<Message<'_>> {
    if !checks(source, destination, bytes) {
        return None;
    }
    let half = |at: usize| Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
    let word = |at: usize| {
        Some(u32::from_be_bytes([
            *bytes.get(at)?,
            *bytes.get(at + 1)?,
            *bytes.get(at + 2)?,
            *bytes.get(at + 3)?,
        ]))
    };
    let address = |at: usize| Some(Ipv6Address(bytes.get(at..at + 16)?.try_into().ok()?));
    let discovery = matches!(
        bytes[0],
        kind::ROUTER_ADVERTISEMENT | kind::NEIGHBOUR_SOLICITATION | kind::NEIGHBOUR_ADVERTISEMENT
    );
    if discovery && (hop_limit != 255 || bytes[1] != 0) {
        return None;
    }
    Some(match bytes[0] {
        kind::ECHO_REQUEST => Message::EchoRequest {
            identifier: half(4)?,
            sequence: half(6)?,
            data: bytes.get(8..)?,
        },
        kind::ECHO_REPLY => Message::EchoReply {
            identifier: half(4)?,
            sequence: half(6)?,
            data: bytes.get(8..)?,
        },
        kind::ROUTER_ADVERTISEMENT => {
            let mut advertisement = RouterAdvertisement {
                lifetime_seconds: half(6)?,
                ..RouterAdvertisement::default()
            };
            for (option, body) in options(bytes.get(16..)?) {
                match (option, body.len()) {
                    (OPTION_SOURCE_LINK, 6) => {
                        advertisement.link = Some(MacAddress(body.try_into().ok()?));
                    }
                    (OPTION_MTU, 6) => {
                        advertisement.mtu =
                            Some(u32::from_be_bytes([body[2], body[3], body[4], body[5]]));
                    }
                    (OPTION_PREFIX, 30) if advertisement.prefix.is_none() => {
                        advertisement.prefix = Some(Prefix {
                            length: body[0],
                            on_link: body[1] & PREFIX_ON_LINK != 0,
                            autonomous: body[1] & PREFIX_AUTONOMOUS != 0,
                            valid_seconds: u32::from_be_bytes([body[2], body[3], body[4], body[5]]),
                            prefix: Ipv6Address(body[14..30].try_into().ok()?),
                        });
                    }
                    _ => {}
                }
            }
            Message::RouterAdvertisement(advertisement)
        }
        kind::NEIGHBOUR_SOLICITATION | kind::NEIGHBOUR_ADVERTISEMENT => {
            let target = address(8)?;
            if target.is_multicast() {
                return None;
            }
            let wanted = match bytes[0] {
                kind::NEIGHBOUR_SOLICITATION => OPTION_SOURCE_LINK,
                _ => OPTION_TARGET_LINK,
            };
            let link = options(bytes.get(24..)?)
                .find(|&(option, body)| option == wanted && body.len() == 6)
                .and_then(|(_, body)| Some(MacAddress(body.try_into().ok()?)));
            let neighbour = Neighbour {
                target,
                link,
                solicited: word(4)? >> 24 & u32::from(ADVERTISEMENT_SOLICITED) != 0,
            };
            match bytes[0] {
                kind::NEIGHBOUR_SOLICITATION => Message::NeighbourSolicitation(neighbour),
                _ => Message::NeighbourAdvertisement(neighbour),
            }
        }
        _ => Message::Other,
    })
}

fn options(mut bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        let kind = *bytes.first()?;
        let length = usize::from(*bytes.get(1)?) * 8;
        if length == 0 || length > bytes.len() {
            return None;
        }
        let body = &bytes[2..length];
        bytes = &bytes[length..];
        Some((kind, body))
    })
}

/// Writes an echo message of `kind` with `identifier`, `sequence` and `data` to `out`, its
/// checksum between `source` and `destination`: its length.
pub fn write_echo(
    out: &mut [u8],
    source: Ipv6Address,
    destination: Ipv6Address,
    kind: u8,
    identifier: u16,
    sequence: u16,
    data: &[u8],
) -> Option<usize> {
    let length = 8 + data.len();
    let message = out.get_mut(..length)?;
    message[0] = kind;
    message[1] = 0;
    message[4..6].copy_from_slice(&identifier.to_be_bytes());
    message[6..8].copy_from_slice(&sequence.to_be_bytes());
    message[8..].copy_from_slice(data);
    seal(source, destination, message);
    Some(length)
}

/// Writes a neighbour solicitation for `target` with this host's `link` address: its length.
pub fn write_neighbour_solicitation(
    out: &mut [u8],
    source: Ipv6Address,
    destination: Ipv6Address,
    target: Ipv6Address,
    link: Option<MacAddress>,
) -> Option<usize> {
    let length = 24 + if link.is_some() { 8 } else { 0 };
    let message = out.get_mut(..length)?;
    message.fill(0);
    message[0] = kind::NEIGHBOUR_SOLICITATION;
    message[8..24].copy_from_slice(&target.0);
    if let Some(link) = link {
        message[24] = OPTION_SOURCE_LINK;
        message[25] = 1;
        message[26..32].copy_from_slice(&link.0);
    }
    seal(source, destination, message);
    Some(length)
}

/// Writes a neighbour advertisement of `target` at this host's `link` address: its length.
pub fn write_neighbour_advertisement(
    out: &mut [u8],
    source: Ipv6Address,
    destination: Ipv6Address,
    target: Ipv6Address,
    link: MacAddress,
    solicited: bool,
) -> Option<usize> {
    let message = out.get_mut(..32)?;
    message.fill(0);
    message[0] = kind::NEIGHBOUR_ADVERTISEMENT;
    message[4] = ADVERTISEMENT_OVERRIDE | if solicited { ADVERTISEMENT_SOLICITED } else { 0 };
    message[8..24].copy_from_slice(&target.0);
    message[24] = OPTION_TARGET_LINK;
    message[25] = 1;
    message[26..32].copy_from_slice(&link.0);
    seal(source, destination, message);
    Some(32)
}

/// Writes a router solicitation with this host's `link` address: its length.
pub fn write_router_solicitation(
    out: &mut [u8],
    source: Ipv6Address,
    link: Option<MacAddress>,
) -> Option<usize> {
    let length = 8 + if link.is_some() { 8 } else { 0 };
    let message = out.get_mut(..length)?;
    message.fill(0);
    message[0] = kind::ROUTER_SOLICITATION;
    if let Some(link) = link {
        message[8] = OPTION_SOURCE_LINK;
        message[9] = 1;
        message[10..16].copy_from_slice(&link.0);
    }
    seal(source, Ipv6Address::ALL_ROUTERS, message);
    Some(length)
}

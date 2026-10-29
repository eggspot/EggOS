//! IPv4 (RFC 791): addresses, subnets and the packet header. The stack sends whole packets
//! with "don't fragment" set and drops the fragments it receives: fragments are not
//! reassembled yet.

use crate::checksum;
use core::fmt;

/// Bytes of a header without options, which is what the stack sends.
pub const HEADER_BYTES: usize = 20;
/// The time to live the stack's packets start with, as Linux's.
pub const DEFAULT_TTL: u8 = 64;

/// The protocols the stack speaks.
pub mod protocol {
    /// ICMP.
    pub const ICMP: u8 = 1;
    /// UDP.
    pub const UDP: u8 = 17;
}

const DONT_FRAGMENT: u16 = 0x4000;
const MORE_FRAGMENTS: u16 = 0x2000;
const FRAGMENT_OFFSET: u16 = 0x1fff;

/// An IPv4 address, in network order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    /// No address: `0.0.0.0`, "any" when binding.
    pub const UNSPECIFIED: Self = Self([0; 4]);
    /// The limited broadcast address: `255.255.255.255`.
    pub const BROADCAST: Self = Self([255; 4]);
    /// The loopback address: `127.0.0.1`.
    pub const LOOPBACK: Self = Self([127, 0, 0, 1]);

    /// `a.b.c.d`.
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// The address as a number, the first byte the highest.
    pub const fn to_bits(self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    /// The address a number names, the highest byte first.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits.to_be_bytes())
    }

    /// Whether it is `0.0.0.0`.
    pub fn is_unspecified(self) -> bool {
        self == Self::UNSPECIFIED
    }

    /// Whether it is `255.255.255.255`.
    pub fn is_broadcast(self) -> bool {
        self == Self::BROADCAST
    }

    /// Whether it is in `127.0.0.0/8`.
    pub fn is_loopback(self) -> bool {
        self.0[0] == 127
    }

    /// Whether it is in `224.0.0.0/4`.
    pub fn is_multicast(self) -> bool {
        self.0[0] & 0xf0 == 0xe0
    }

    /// Parses four decimal numbers from 0 to 255 separated by dots, with no sign and no more
    /// than three digits each.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let mut address = [0u8; 4];
        let mut parts = text.split(|&byte| byte == b'.');
        for byte in &mut address {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 3 || !part.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let value = part
                .iter()
                .fold(0u16, |value, digit| value * 10 + u16::from(digit - b'0'));
            *byte = u8::try_from(value).ok()?;
        }
        parts.next().is_none().then_some(Self(address))
    }
}

impl fmt::Display for Ipv4Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(formatter, "{a}.{b}.{c}.{d}")
    }
}

/// An address with the length of its network prefix, as `10.0.2.15/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subnet {
    /// The address.
    pub address: Ipv4Address,
    /// How many leading bits name the network, 0 to 32.
    pub prefix: u8,
}

impl Subnet {
    /// `address/prefix`; `None` for a prefix over 32.
    pub fn new(address: Ipv4Address, prefix: u8) -> Option<Self> {
        (prefix <= 32).then_some(Self { address, prefix })
    }

    /// The network mask, as `255.255.255.0` for a prefix of 24.
    pub fn mask(self) -> Ipv4Address {
        Ipv4Address::from_bits(mask_bits(self.prefix))
    }

    /// Whether `other` is on the same network.
    pub fn contains(self, other: Ipv4Address) -> bool {
        (self.address.to_bits() ^ other.to_bits()) & mask_bits(self.prefix) == 0
    }

    /// The network's broadcast address, every host bit set.
    pub fn broadcast(self) -> Ipv4Address {
        Ipv4Address::from_bits(self.address.to_bits() | !mask_bits(self.prefix))
    }

    /// The network's address, every host bit clear.
    pub fn network(self) -> Ipv4Address {
        Ipv4Address::from_bits(self.address.to_bits() & mask_bits(self.prefix))
    }

    /// Parses `a.b.c.d/prefix`, as [`fmt::Display`] writes it.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let slash = text.iter().position(|&byte| byte == b'/')?;
        let prefix = &text[slash + 1..];
        if prefix.is_empty() || prefix.len() > 2 || !prefix.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let prefix = prefix
            .iter()
            .fold(0u8, |value, digit| value * 10 + (digit - b'0'));
        Self::new(Ipv4Address::parse(&text[..slash])?, prefix)
    }
}

impl fmt::Display for Subnet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.address, self.prefix)
    }
}

fn mask_bits(prefix: u8) -> u32 {
    match prefix {
        0 => 0,
        prefix => u32::MAX << (32 - u32::from(prefix.min(32))),
    }
}

/// The prefix length of a network mask whose ones all lead; `None` for another mask.
pub fn prefix_of_mask(mask: Ipv4Address) -> Option<u8> {
    let bits = mask.to_bits();
    let ones = bits.leading_ones();
    (bits == mask_bits(ones as u8)).then_some(ones as u8)
}

/// The fields of a header the stack reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Where the packet comes from.
    pub source: Ipv4Address,
    /// Where it goes.
    pub destination: Ipv4Address,
    /// What it carries ([`protocol`]).
    pub protocol: u8,
    /// Hops left.
    pub ttl: u8,
}

/// The header and the payload of `packet` (to its total length); `None` for anything but a
/// whole, unfragmented IPv4 packet with a correct header checksum.
pub fn parse(packet: &[u8]) -> Option<(Header, &[u8])> {
    let first = *packet.first()?;
    let header_length = usize::from(first & 0x0f) * 4;
    if first >> 4 != 4 || header_length < HEADER_BYTES || packet.len() < header_length {
        return None;
    }
    let total = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if total < header_length || total > packet.len() {
        return None;
    }
    let flags = u16::from_be_bytes([packet[6], packet[7]]);
    if flags & (MORE_FRAGMENTS | FRAGMENT_OFFSET) != 0 {
        return None;
    }
    if checksum::of(&packet[..header_length]) != 0 {
        return None;
    }
    let address = |at: usize| Ipv4Address([packet[at], packet[at + 1], packet[at + 2], packet[at + 3]]);
    Some((
        Header {
            source: address(12),
            destination: address(16),
            protocol: packet[9],
            ttl: packet[8],
        },
        &packet[header_length..total],
    ))
}

/// Writes a header without options for `header` and a payload of `payload_bytes` to the start
/// of `out`, with the identification `identification` and "don't fragment"; `None` when `out`
/// is shorter than a header or the packet would be longer than 65535 bytes.
pub fn write_header(
    out: &mut [u8],
    header: &Header,
    identification: u16,
    payload_bytes: usize,
) -> Option<usize> {
    let total = u16::try_from(HEADER_BYTES + payload_bytes).ok()?;
    let bytes = out.first_chunk_mut::<HEADER_BYTES>()?;
    bytes[0] = 0x45;
    bytes[1] = 0;
    bytes[2..4].copy_from_slice(&total.to_be_bytes());
    bytes[4..6].copy_from_slice(&identification.to_be_bytes());
    bytes[6..8].copy_from_slice(&DONT_FRAGMENT.to_be_bytes());
    bytes[8] = header.ttl;
    bytes[9] = header.protocol;
    bytes[10..12].fill(0);
    bytes[12..16].copy_from_slice(&header.source.0);
    bytes[16..20].copy_from_slice(&header.destination.0);
    let sum = checksum::of(bytes);
    bytes[10..12].copy_from_slice(&sum.to_be_bytes());
    Some(HEADER_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_strictly_and_print_dotted() {
        assert_eq!(Ipv4Address::parse(b"10.0.2.15"), Some(Ipv4Address::new(10, 0, 2, 15)));
        for bad in [&b"10.0.2"[..], b"10.0.2.256", b"10.0.2.15.1", b"10..2.15", b"+1.0.0.0", b"0010.0.0.1", b""] {
            assert_eq!(Ipv4Address::parse(bad), None, "{bad:?}");
        }
        assert_eq!(format!("{}", Ipv4Address::new(192, 168, 1, 254)), "192.168.1.254");
        assert!(Ipv4Address::LOOPBACK.is_loopback());
        assert!(Ipv4Address::new(224, 0, 0, 251).is_multicast());
        assert_eq!(Ipv4Address::from_bits(0x0a00_020f), Ipv4Address::new(10, 0, 2, 15));
    }

    #[test]
    fn subnets_know_their_mask_network_broadcast_and_members() {
        let subnet = Subnet::parse(b"10.0.2.15/24").unwrap();
        assert_eq!(subnet.mask(), Ipv4Address::new(255, 255, 255, 0));
        assert_eq!(subnet.network(), Ipv4Address::new(10, 0, 2, 0));
        assert_eq!(subnet.broadcast(), Ipv4Address::new(10, 0, 2, 255));
        assert!(subnet.contains(Ipv4Address::new(10, 0, 2, 2)));
        assert!(!subnet.contains(Ipv4Address::new(10, 0, 3, 2)));
        assert_eq!(format!("{subnet}"), "10.0.2.15/24");
        assert_eq!(Subnet::parse(b"10.0.2.15/33"), None);
        assert_eq!(Subnet::parse(b"10.0.2.15/"), None);
        let everything = Subnet::new(Ipv4Address::UNSPECIFIED, 0).unwrap();
        assert!(everything.contains(Ipv4Address::new(8, 8, 8, 8)));
        assert_eq!(prefix_of_mask(Ipv4Address::new(255, 255, 255, 0)), Some(24));
        assert_eq!(prefix_of_mask(Ipv4Address::BROADCAST), Some(32));
        assert_eq!(prefix_of_mask(Ipv4Address::UNSPECIFIED), Some(0));
        assert_eq!(prefix_of_mask(Ipv4Address::new(255, 0, 255, 0)), None);
    }

    #[test]
    fn a_written_header_parses_back_and_damage_or_fragments_are_refused() {
        let header = Header {
            source: Ipv4Address::new(10, 0, 2, 15),
            destination: Ipv4Address::new(10, 0, 2, 2),
            protocol: protocol::UDP,
            ttl: DEFAULT_TTL,
        };
        let mut packet = [0u8; HEADER_BYTES + 4];
        assert_eq!(write_header(&mut packet, &header, 7, 4), Some(HEADER_BYTES));
        packet[HEADER_BYTES..].copy_from_slice(b"ping");
        let (parsed, payload) = parse(&packet).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(payload, b"ping");
        let mut damaged = packet;
        damaged[15] ^= 1;
        assert_eq!(parse(&damaged), None);
        let mut fragment = packet;
        fragment[6] = 0x20;
        fragment[10..12].fill(0);
        let sum = checksum::of(&fragment[..HEADER_BYTES]);
        fragment[10..12].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(parse(&fragment), None);
        assert_eq!(parse(&packet[..HEADER_BYTES + 3]), None);
        let mut padded = [0u8; HEADER_BYTES + 10];
        padded[..packet.len()].copy_from_slice(&packet);
        assert_eq!(parse(&padded).unwrap().1, b"ping");
    }
}

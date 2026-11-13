//! IPv6 (RFC 8200, D565): addresses and the packet header. The stack reads past the extension
//! headers it meets to the upper-layer protocol, drops fragments (none are reassembled, as for
//! IPv4) and sends packets with no extension headers.

use crate::checksum;
use crate::ethernet::MacAddress;
use core::fmt;

/// Bytes of the fixed header.
pub const HEADER_BYTES: usize = 40;
/// The hop limit the stack's packets start with, as Linux's.
pub const DEFAULT_HOP_LIMIT: u8 = 64;
/// The smallest MTU every IPv6 link carries.
pub const MIN_MTU: usize = 1280;

/// The next-header values the stack reads.
pub mod next_header {
    /// Hop-by-hop options.
    pub const HOP_BY_HOP: u8 = 0;
    /// TCP.
    pub const TCP: u8 = 6;
    /// UDP.
    pub const UDP: u8 = 17;
    /// The routing header.
    pub const ROUTING: u8 = 43;
    /// The fragment header.
    pub const FRAGMENT: u8 = 44;
    /// ICMPv6.
    pub const ICMPV6: u8 = 58;
    /// No next header.
    pub const NONE: u8 = 59;
    /// Destination options.
    pub const DESTINATION_OPTIONS: u8 = 60;
}

/// An IPv6 address, in network order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct Ipv6Address(pub [u8; 16]);

impl Ipv6Address {
    /// No address: `::`, "any" when binding.
    pub const UNSPECIFIED: Self = Self([0; 16]);
    /// The loopback address: `::1`.
    pub const LOOPBACK: Self = Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    /// Every node on the link: `ff02::1`.
    pub const ALL_NODES: Self = Self([0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    /// Every router on the link: `ff02::2`.
    pub const ALL_ROUTERS: Self = Self([0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);

    /// The address of eight 16-bit groups.
    pub const fn new(groups: [u16; 8]) -> Self {
        let mut bytes = [0u8; 16];
        let mut index = 0;
        while index < 8 {
            let [high, low] = groups[index].to_be_bytes();
            bytes[2 * index] = high;
            bytes[2 * index + 1] = low;
            index += 1;
        }
        Self(bytes)
    }

    /// Its eight 16-bit groups.
    pub fn groups(self) -> [u16; 8] {
        core::array::from_fn(|index| u16::from_be_bytes([self.0[2 * index], self.0[2 * index + 1]]))
    }

    /// Whether it is `::`.
    pub fn is_unspecified(self) -> bool {
        self == Self::UNSPECIFIED
    }

    /// Whether it is `::1`.
    pub fn is_loopback(self) -> bool {
        self == Self::LOOPBACK
    }

    /// Whether it is in `ff00::/8`.
    pub fn is_multicast(self) -> bool {
        self.0[0] == 0xff
    }

    /// Whether it is in `fe80::/10`.
    pub fn is_link_local(self) -> bool {
        self.0[0] == 0xfe && self.0[1] & 0xc0 == 0x80
    }

    /// The IPv4 address an IPv4-mapped address (`::ffff:a.b.c.d`) holds.
    pub fn to_mapped_ipv4(self) -> Option<crate::ipv4::Ipv4Address> {
        let mapped = self.0[..10].iter().all(|&byte| byte == 0) && self.0[10..12] == [0xff, 0xff];
        mapped.then(|| crate::ipv4::Ipv4Address([self.0[12], self.0[13], self.0[14], self.0[15]]))
    }

    /// The IPv4-mapped address of `address`.
    pub fn mapped(address: crate::ipv4::Ipv4Address) -> Self {
        let mut bytes = [0u8; 16];
        bytes[10] = 0xff;
        bytes[11] = 0xff;
        bytes[12..].copy_from_slice(&address.0);
        Self(bytes)
    }

    /// The solicited-node multicast address that neighbour solicitations for it go to
    /// (`ff02::1:ffXX:XXXX`).
    pub fn solicited_node(self) -> Self {
        let mut bytes = [0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xff, 0, 0, 0];
        bytes[13..].copy_from_slice(&self.0[13..]);
        Self(bytes)
    }

    /// The Ethernet multicast address a multicast address goes to (`33:33` and its last four
    /// bytes, RFC 2464 7).
    pub fn multicast_mac(self) -> MacAddress {
        MacAddress([0x33, 0x33, self.0[12], self.0[13], self.0[14], self.0[15]])
    }

    /// The address of `prefix` (its first 64 bits) with the interface identifier made from
    /// `mac` (modified EUI-64, RFC 4291 appendix A).
    pub fn with_interface_of(prefix: Self, mac: MacAddress) -> Self {
        let mut bytes = prefix.0;
        let mac = mac.0;
        bytes[8] = mac[0] ^ 0x02;
        bytes[9] = mac[1];
        bytes[10] = mac[2];
        bytes[11] = 0xff;
        bytes[12] = 0xfe;
        bytes[13] = mac[3];
        bytes[14] = mac[4];
        bytes[15] = mac[5];
        Self(bytes)
    }

    /// The link-local address of `mac`: `fe80::` with its interface identifier.
    pub fn link_local_of(mac: MacAddress) -> Self {
        Self::with_interface_of(Self([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), mac)
    }

    /// Whether its first `prefix` bits are `network`'s.
    pub fn in_prefix(self, network: Self, prefix: u8) -> bool {
        let prefix = usize::from(prefix.min(128));
        let whole = prefix / 8;
        if self.0[..whole] != network.0[..whole] {
            return false;
        }
        let bits = prefix % 8;
        bits == 0 || {
            let mask = 0xffu8 << (8 - bits);
            self.0[whole] & mask == network.0[whole] & mask
        }
    }

    /// Parses the text form: eight groups of up to four hexadecimal digits separated by
    /// colons, one run of zero groups written `::`, and the last 32 bits as an IPv4 address.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let (head, tail) = match find_double_colon(text) {
            Some(at) => (&text[..at], Some(&text[at + 2..])),
            None => (text, None),
        };
        let mut groups = [0u16; 8];
        let head_count = parse_groups(head, &mut groups, tail.is_none())?;
        match tail {
            None => (head_count == 8).then_some(Self::new(groups)),
            Some(tail) => {
                let mut tail_groups = [0u16; 8];
                let tail_count = parse_groups(tail, &mut tail_groups, true)?;
                if head_count + tail_count > 7 {
                    return None;
                }
                groups[8 - tail_count..].copy_from_slice(&tail_groups[..tail_count]);
                Some(Self::new(groups))
            }
        }
    }
}

fn find_double_colon(text: &[u8]) -> Option<usize> {
    let at = text.windows(2).position(|pair| pair == b"::")?;
    let again = text[at + 2..].windows(2).any(|pair| pair == b"::");
    (!again).then_some(at)
}

fn parse_groups(text: &[u8], groups: &mut [u16; 8], ipv4_tail: bool) -> Option<usize> {
    if text.is_empty() {
        return Some(0);
    }
    let parts: [Option<&[u8]>; 8] = {
        let mut parts = [None; 8];
        let mut split = text.split(|&byte| byte == b':');
        for part in &mut parts {
            *part = split.next();
        }
        if split.next().is_some() {
            return None;
        }
        parts
    };
    let mut count = 0;
    for (index, part) in parts.iter().enumerate() {
        let Some(part) = part else {
            break;
        };
        let last = parts.get(index + 1).is_none_or(Option::is_none);
        if last && ipv4_tail && part.contains(&b'.') {
            let ipv4 = crate::ipv4::Ipv4Address::parse(part)?;
            if count + 2 > 8 {
                return None;
            }
            groups[count] = u16::from_be_bytes([ipv4.0[0], ipv4.0[1]]);
            groups[count + 1] = u16::from_be_bytes([ipv4.0[2], ipv4.0[3]]);
            return Some(count + 2);
        }
        if part.is_empty() || part.len() > 4 {
            return None;
        }
        let mut value = 0u16;
        for &digit in *part {
            value = value << 4 | u16::from(hex_value(digit)?);
        }
        groups[count] = value;
        count += 1;
    }
    Some(count)
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Ipv6Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ipv4) = self.to_mapped_ipv4() {
            return write!(formatter, "::ffff:{ipv4}");
        }
        let groups = self.groups();
        let mut longest = (0usize, 0usize);
        let mut run = (0usize, 0usize);
        for (index, &group) in groups.iter().enumerate() {
            if group == 0 {
                if run.1 == 0 {
                    run.0 = index;
                }
                run.1 += 1;
                if run.1 > longest.1 {
                    longest = run;
                }
            } else {
                run.1 = 0;
            }
        }
        if longest.1 < 2 {
            longest = (8, 0);
        }
        let mut index = 0;
        while index < 8 {
            if index == longest.0 {
                formatter.write_str("::")?;
                index += longest.1;
                continue;
            }
            if index > 0 && index != longest.0 + longest.1 {
                formatter.write_str(":")?;
            }
            write!(formatter, "{:x}", groups[index])?;
            index += 1;
        }
        Ok(())
    }
}

/// An IPv6 packet's header fields the stack reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The sender.
    pub source: Ipv6Address,
    /// The receiver.
    pub destination: Ipv6Address,
    /// The upper-layer protocol, past every extension header ([`next_header`]).
    pub protocol: u8,
    /// How many more routers it may cross.
    pub hop_limit: u8,
}

/// A packet that came for the stack: its header and the upper-layer payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    /// The header.
    pub header: Header,
    /// The upper layer's bytes.
    pub payload: &'a [u8],
}

/// The packet `bytes` holds: `None` when it is not IPv6, shorter than it says, a fragment or
/// carries an extension header the stack cannot read past.
pub fn parse(bytes: &[u8]) -> Option<Packet<'_>> {
    let fixed = bytes.first_chunk::<HEADER_BYTES>()?;
    if fixed[0] >> 4 != 6 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([fixed[4], fixed[5]]));
    let end = HEADER_BYTES.checked_add(length)?;
    let mut payload = bytes.get(HEADER_BYTES..end)?;
    let mut protocol = fixed[6];
    loop {
        match protocol {
            next_header::HOP_BY_HOP | next_header::ROUTING | next_header::DESTINATION_OPTIONS => {
                let next = *payload.first()?;
                let bytes = (usize::from(*payload.get(1)?) + 1) * 8;
                payload = payload.get(bytes..)?;
                protocol = next;
            }
            next_header::FRAGMENT => return None,
            _ => break,
        }
    }
    let address = |at: usize| Ipv6Address(fixed[at..at + 16].try_into().unwrap_or([0; 16]));
    Some(Packet {
        header: Header {
            source: address(8),
            destination: address(24),
            protocol,
            hop_limit: fixed[7],
        },
        payload,
    })
}

/// Writes `header` for a packet of `payload_bytes` already at `out[HEADER_BYTES..]`: the
/// packet's length, or `None` when `out` is shorter or the payload longer than 65535 bytes.
pub fn write_header(out: &mut [u8], header: &Header, payload_bytes: usize) -> Option<usize> {
    let length = u16::try_from(payload_bytes).ok()?;
    let total = HEADER_BYTES + payload_bytes;
    let packet = out.get_mut(..total)?;
    packet[0..4].copy_from_slice(&[0x60, 0, 0, 0]);
    packet[4..6].copy_from_slice(&length.to_be_bytes());
    packet[6] = header.protocol;
    packet[7] = header.hop_limit;
    packet[8..24].copy_from_slice(&header.source.0);
    packet[24..40].copy_from_slice(&header.destination.0);
    Some(total)
}

/// The running checksum sum of the pseudo-header upper layers sum over (RFC 8200 8.1).
pub fn pseudo_header(
    source: Ipv6Address,
    destination: Ipv6Address,
    protocol: u8,
    length: usize,
) -> u32 {
    let sum = checksum::add(checksum::add(0, &source.0), &destination.0);
    let sum = checksum::add(sum, &(length as u32).to_be_bytes());
    checksum::add(sum, &[0, 0, 0, protocol])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_and_print_in_their_shortest_form() {
        let address = Ipv6Address::parse(b"2001:db8::8a2e:370:7334").unwrap();
        assert_eq!(
            address.groups(),
            [0x2001, 0xdb8, 0, 0, 0, 0x8a2e, 0x370, 0x7334]
        );
        assert_eq!(std::format!("{address}"), "2001:db8::8a2e:370:7334");
        assert_eq!(Ipv6Address::parse(b"::1"), Some(Ipv6Address::LOOPBACK));
        assert_eq!(Ipv6Address::parse(b"::"), Some(Ipv6Address::UNSPECIFIED));
        assert_eq!(std::format!("{}", Ipv6Address::LOOPBACK), "::1");
        assert_eq!(
            Ipv6Address::parse(b"::ffff:10.0.2.15").and_then(Ipv6Address::to_mapped_ipv4),
            Some(crate::ipv4::Ipv4Address::new(10, 0, 2, 15))
        );
        assert_eq!(Ipv6Address::parse(b"1::2::3"), None);
        assert_eq!(Ipv6Address::parse(b"12345::"), None);
    }

    #[test]
    fn an_interface_identifier_comes_from_the_mac_address() {
        let mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(
            std::format!("{}", Ipv6Address::link_local_of(mac)),
            "fe80::5054:ff:fe12:3456"
        );
        let address = Ipv6Address::parse(b"fe80::1:2").unwrap();
        assert_eq!(
            std::format!("{}", address.solicited_node()),
            "ff02::1:ff01:2"
        );
    }
}

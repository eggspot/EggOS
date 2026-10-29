//! Ethernet II frames: a destination and a source hardware (MAC) address, an EtherType, then
//! the payload. The frame check sequence is the adapter's business and never reaches here.

use core::fmt;

/// Bytes of the header: two addresses and the EtherType.
pub const HEADER_BYTES: usize = 14;
/// The largest payload, the standard MTU.
pub const MAX_PAYLOAD_BYTES: usize = 1500;
/// The largest frame: the header and the largest payload.
pub const MAX_FRAME_BYTES: usize = HEADER_BYTES + MAX_PAYLOAD_BYTES;
/// The shortest frame on the wire without its check sequence; shorter ones are padded.
pub const MIN_FRAME_BYTES: usize = 60;

/// The EtherTypes the stack speaks.
pub mod ether_type {
    /// IPv4.
    pub const IPV4: u16 = 0x0800;
    /// ARP.
    pub const ARP: u16 = 0x0806;
}

/// A 48-bit hardware address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Every station: `ff:ff:ff:ff:ff:ff`.
    pub const BROADCAST: Self = Self([0xff; 6]);
    /// No address: `00:00:00:00:00:00`, which `lo` reports.
    pub const ZERO: Self = Self([0; 6]);

    /// Whether it is the broadcast address.
    pub fn is_broadcast(self) -> bool {
        self == Self::BROADCAST
    }

    /// Whether it names a group (its first byte's lowest bit), the broadcast address included.
    pub fn is_multicast(self) -> bool {
        self.0[0] & 1 != 0
    }

    /// Parses six two-digit hexadecimal bytes separated by colons, as [`fmt::Display`] writes
    /// them.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let mut address = [0u8; 6];
        let mut parts = text.split(|&byte| byte == b':');
        for byte in &mut address {
            let part = parts.next()?;
            let [high, low] = part else {
                return None;
            };
            *byte = (hex_digit(*high)? << 4) | hex_digit(*low)?;
        }
        parts.next().is_none().then_some(Self(address))
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, f] = self.0;
        write!(
            formatter,
            "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}"
        )
    }
}

/// A frame's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Where it goes.
    pub destination: MacAddress,
    /// Where it comes from.
    pub source: MacAddress,
    /// What the payload is ([`ether_type`]).
    pub ether_type: u16,
}

/// The header and the payload of `frame`; `None` when it is shorter than a header.
pub fn parse(frame: &[u8]) -> Option<(Header, &[u8])> {
    let (header, payload) = frame.split_first_chunk::<HEADER_BYTES>()?;
    let mut destination = [0u8; 6];
    destination.copy_from_slice(&header[..6]);
    let mut source = [0u8; 6];
    source.copy_from_slice(&header[6..12]);
    Some((
        Header {
            destination: MacAddress(destination),
            source: MacAddress(source),
            ether_type: u16::from_be_bytes([header[12], header[13]]),
        },
        payload,
    ))
}

/// Writes `header` to the start of `out`; `false` when `out` is shorter than a header.
pub fn write_header(out: &mut [u8], header: &Header) -> bool {
    let Some(bytes) = out.first_chunk_mut::<HEADER_BYTES>() else {
        return false;
    };
    bytes[..6].copy_from_slice(&header.destination.0);
    bytes[6..12].copy_from_slice(&header.source.0);
    bytes[12..].copy_from_slice(&header.ether_type.to_be_bytes());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_written_and_parsed_as_six_hex_pairs() {
        let mac = MacAddress([0x52, 0x54, 0, 0x12, 0x34, 0x56]);
        assert_eq!(format!("{mac}"), "52:54:00:12:34:56");
        assert_eq!(MacAddress::parse(b"52:54:00:12:34:56"), Some(mac));
        assert_eq!(MacAddress::parse(b"52:54:00:12:34:5G"), None);
        assert_eq!(MacAddress::parse(b"52:54:00:12:34"), None);
        assert_eq!(MacAddress::parse(b"52:54:00:12:34:56:78"), None);
        assert!(MacAddress::BROADCAST.is_multicast() && MacAddress::BROADCAST.is_broadcast());
        assert!(!mac.is_multicast());
    }

    #[test]
    fn a_header_round_trips_and_a_short_frame_is_refused() {
        let header = Header {
            destination: MacAddress::BROADCAST,
            source: MacAddress([2, 0, 0, 0, 0, 1]),
            ether_type: ether_type::ARP,
        };
        let mut frame = [0u8; 20];
        assert!(write_header(&mut frame, &header));
        frame[14] = 7;
        let (parsed, payload) = parse(&frame).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(payload[0], 7);
        assert_eq!(payload.len(), 6);
        assert_eq!(parse(&frame[..13]), None);
        assert!(!write_header(&mut [0u8; 13], &header));
    }
}

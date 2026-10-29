//! UDP (RFC 768): ports, a length and a checksum over a pseudo-header of the IPv4 addresses,
//! the protocol and the length, then the datagram.

use crate::checksum;
use crate::ipv4::{Ipv4Address, protocol};

/// Bytes of the header.
pub const HEADER_BYTES: usize = 8;

/// The ports of a datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The sender's port.
    pub source_port: u16,
    /// The receiver's port.
    pub destination_port: u16,
}

fn pseudo_header(source: Ipv4Address, destination: Ipv4Address, length: u16) -> u32 {
    let sum = checksum::add(checksum::add(0, &source.0), &destination.0);
    sum + u32::from(protocol::UDP) + u32::from(length)
}

/// The header and the data of the datagram `segment` carries between `source` and
/// `destination`; `None` when its length field is out of range or its checksum, when it has
/// one, is wrong.
pub fn parse(
    source: Ipv4Address,
    destination: Ipv4Address,
    segment: &[u8],
) -> Option<(Header, &[u8])> {
    let header = segment.first_chunk::<HEADER_BYTES>()?;
    let length = u16::from_be_bytes([header[4], header[5]]);
    let datagram = segment.get(..usize::from(length))?;
    if datagram.len() < HEADER_BYTES {
        return None;
    }
    let carried = u16::from_be_bytes([header[6], header[7]]);
    if carried != 0
        && checksum::finish(checksum::add(
            pseudo_header(source, destination, length),
            datagram,
        )) != 0
    {
        return None;
    }
    Some((
        Header {
            source_port: u16::from_be_bytes([header[0], header[1]]),
            destination_port: u16::from_be_bytes([header[2], header[3]]),
        },
        &datagram[HEADER_BYTES..],
    ))
}

/// Writes the header of a datagram from `source`:`header.source_port` to
/// `destination`:`header.destination_port` whose `data_bytes` of data are already at
/// `out[HEADER_BYTES..]`, with its checksum: the datagram's length, or `None` when `out` is
/// shorter or the datagram longer than 65535 bytes.
pub fn write_header(
    out: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    header: Header,
    data_bytes: usize,
) -> Option<usize> {
    let length = u16::try_from(HEADER_BYTES.checked_add(data_bytes)?).ok()?;
    let datagram = out.get_mut(..usize::from(length))?;
    datagram[0..2].copy_from_slice(&header.source_port.to_be_bytes());
    datagram[2..4].copy_from_slice(&header.destination_port.to_be_bytes());
    datagram[4..6].copy_from_slice(&length.to_be_bytes());
    datagram[6..8].fill(0);
    let sum = checksum::finish(checksum::add(
        pseudo_header(source, destination, length),
        datagram,
    ));
    let sum = if sum == 0 { 0xffff } else { sum };
    datagram[6..8].copy_from_slice(&sum.to_be_bytes());
    Some(usize::from(length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datagram_round_trips_and_a_wrong_checksum_or_length_is_refused() {
        let source = Ipv4Address::new(10, 0, 2, 15);
        let destination = Ipv4Address::new(10, 0, 2, 3);
        let header = Header {
            source_port: 40000,
            destination_port: 53,
        };
        let mut datagram = [0u8; HEADER_BYTES + 3];
        datagram[HEADER_BYTES..].copy_from_slice(b"abc");
        assert_eq!(
            write_header(&mut datagram, source, destination, header, 3),
            Some(11)
        );
        assert_eq!(
            parse(source, destination, &datagram),
            Some((header, &b"abc"[..]))
        );
        assert_eq!(parse(source, Ipv4Address::new(10, 0, 2, 4), &datagram), None);
        let mut unchecked = datagram;
        unchecked[6..8].fill(0);
        unchecked[9] = b'X';
        assert_eq!(
            parse(source, destination, &unchecked),
            Some((header, &b"aXc"[..]))
        );
        let mut long = datagram;
        long[5] = 12;
        assert_eq!(parse(source, destination, &long), None);
        let mut short = datagram;
        short[5] = 7;
        assert_eq!(parse(source, destination, &short), None);
    }
}

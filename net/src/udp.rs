//! UDP (RFC 768): ports, a length and a checksum over a pseudo-header of the IP addresses,
//! the protocol and the length, then the datagram; over IPv6 too, where the checksum is
//! required (RFC 8200 8.1, D565).

use crate::checksum::{self, Completion};
use crate::ip::{self, IpAddress};
use crate::ipv4::protocol;

/// Bytes of the header.
pub const HEADER_BYTES: usize = 8;
/// Where the checksum lies in a datagram.
pub const CHECKSUM_AT: usize = 6;

/// The ports of a datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The sender's port.
    pub source_port: u16,
    /// The receiver's port.
    pub destination_port: u16,
}

fn pseudo_header(source: IpAddress, destination: IpAddress, length: u16) -> u32 {
    ip::pseudo_header(source, destination, protocol::UDP, usize::from(length))
}

/// The header and the data of the datagram `segment` carries between `source` and
/// `destination`; `None` when its length field is out of range or its checksum, when it has
/// one, is wrong.
pub fn parse(
    source: impl Into<IpAddress>,
    destination: impl Into<IpAddress>,
    segment: &[u8],
) -> Option<(Header, &[u8])> {
    let (source, destination) = (source.into(), destination.into());
    let header = segment.first_chunk::<HEADER_BYTES>()?;
    let length = u16::from_be_bytes([header[4], header[5]]);
    let datagram = segment.get(..usize::from(length))?;
    if datagram.len() < HEADER_BYTES {
        return None;
    }
    let carried = u16::from_be_bytes([header[6], header[7]]);
    if carried == 0 && matches!(source, IpAddress::V6(_)) {
        return None;
    }
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
    source: impl Into<IpAddress>,
    destination: impl Into<IpAddress>,
    header: Header,
    data_bytes: usize,
) -> Option<usize> {
    write_header_with(
        out,
        source,
        destination,
        header,
        data_bytes,
        Completion::Full,
    )
}

/// [`write_header`], with the checksum written whole or left `completion` says (D535).
pub fn write_header_with(
    out: &mut [u8],
    source: impl Into<IpAddress>,
    destination: impl Into<IpAddress>,
    header: Header,
    data_bytes: usize,
    completion: Completion,
) -> Option<usize> {
    let length = u16::try_from(HEADER_BYTES.checked_add(data_bytes)?).ok()?;
    let datagram = out.get_mut(..usize::from(length))?;
    datagram[0..2].copy_from_slice(&header.source_port.to_be_bytes());
    datagram[2..4].copy_from_slice(&header.destination_port.to_be_bytes());
    datagram[4..6].copy_from_slice(&length.to_be_bytes());
    datagram[6..8].fill(0);
    let pseudo = pseudo_header(source.into(), destination.into(), length);
    let sum = match completion {
        Completion::Full => match checksum::finish(checksum::add(pseudo, datagram)) {
            0 => 0xffff,
            sum => sum,
        },
        Completion::Partial => checksum::partial(pseudo),
    };
    datagram[CHECKSUM_AT..CHECKSUM_AT + 2].copy_from_slice(&sum.to_be_bytes());
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
        assert_eq!(
            parse(source, Ipv4Address::new(10, 0, 2, 4), &datagram),
            None
        );
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
        let mut partial = [0u8; HEADER_BYTES + 3];
        partial[HEADER_BYTES..].copy_from_slice(b"abc");
        let partially = Completion::Partial;
        let written = write_header_with(&mut partial, source, destination, header, 3, partially);
        assert_eq!(written, Some(11));
        checksum::complete(&mut partial, CHECKSUM_AT);
        assert_eq!(partial, datagram);
    }
}

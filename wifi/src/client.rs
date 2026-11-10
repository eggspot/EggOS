//! A client station's frames (IEEE 802.11-2020, 9.3.3 and 9.3.2; D278): open-system
//! authentication and the association request it sends, the answers it reads, and data frames
//! to and from the access point, carrying Ethernet payloads behind an LLC/SNAP header.

use crate::frames::{Address, WPA2_PERSONAL_RSN};
use alloc::vec::Vec;

const HEADER_BYTES: usize = 24;
const QOS_HEADER_BYTES: usize = 26;
const SUBTYPE_ASSOCIATION_REQUEST: u8 = 0x00;
const SUBTYPE_ASSOCIATION_RESPONSE: u8 = 0x10;
const SUBTYPE_AUTHENTICATION: u8 = 0xb0;
const TYPE_DATA: u8 = 0x08;
const TYPE_QOS_DATA: u8 = 0x88;
const TYPE_MASK: u8 = 0x0c;
const TO_DS: u8 = 0x01;
const FROM_DS: u8 = 0x02;
const PROTECTED: u8 = 0x40;
const OPEN_SYSTEM: u16 = 0;
const SUCCESS: u16 = 0;
const CAPABILITY_ESS: u16 = 1 << 0;
const CAPABILITY_PRIVACY: u16 = 1 << 4;
const CAPABILITY_SHORT_SLOT: u16 = 1 << 10;
const LISTEN_INTERVAL: u16 = 10;
const ELEMENT_SSID: u8 = 0;
const ELEMENT_RATES: u8 = 1;
const ELEMENT_EXTENDED_RATES: u8 = 50;
const RATES: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
const EXTENDED_RATES: [u8; 4] = [0x30, 0x48, 0x60, 0x6c];
const LLC_SNAP: [u8; 6] = [0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00];
const BEACON_INTERVAL_AT: usize = HEADER_BYTES + 8;
const ASSOCIATION_ID_BITS: u16 = 0x3fff;

/// The EAPOL ethertype, which carries the four-way handshake.
pub const ETHERTYPE_EAPOL: u16 = 0x888e;

fn header(control: u8, flags: u8, first: Address, second: Address, third: Address) -> Vec<u8> {
    let mut frame = Vec::with_capacity(64);
    frame.extend_from_slice(&[control, flags, 0, 0]);
    frame.extend_from_slice(&first);
    frame.extend_from_slice(&second);
    frame.extend_from_slice(&third);
    frame.extend_from_slice(&[0, 0]);
    frame
}

/// The open-system authentication request `own` sends to `bssid`.
pub fn authentication_request(own: Address, bssid: Address) -> Vec<u8> {
    let mut frame = header(SUBTYPE_AUTHENTICATION, 0, bssid, own, bssid);
    frame.extend_from_slice(&OPEN_SYSTEM.to_le_bytes());
    frame.extend_from_slice(&1u16.to_le_bytes());
    frame.extend_from_slice(&SUCCESS.to_le_bytes());
    frame
}

/// The association request `own` sends to `bssid` for network `ssid`, offering WPA2-Personal
/// when `protected`.
pub fn association_request(own: Address, bssid: Address, ssid: &[u8], protected: bool) -> Vec<u8> {
    let mut frame = header(SUBTYPE_ASSOCIATION_REQUEST, 0, bssid, own, bssid);
    let mut capability = CAPABILITY_ESS | CAPABILITY_SHORT_SLOT;
    if protected {
        capability |= CAPABILITY_PRIVACY;
    }
    frame.extend_from_slice(&capability.to_le_bytes());
    frame.extend_from_slice(&LISTEN_INTERVAL.to_le_bytes());
    frame.extend_from_slice(&[ELEMENT_SSID, ssid.len().min(32) as u8]);
    frame.extend_from_slice(&ssid[..ssid.len().min(32)]);
    frame.extend_from_slice(&[ELEMENT_RATES, RATES.len() as u8]);
    frame.extend_from_slice(&RATES);
    frame.extend_from_slice(&[ELEMENT_EXTENDED_RATES, EXTENDED_RATES.len() as u8]);
    frame.extend_from_slice(&EXTENDED_RATES);
    if protected {
        frame.extend_from_slice(&WPA2_PERSONAL_RSN);
    }
    frame
}

fn from(frame: &[u8], bssid: Address, subtype: u8) -> Option<&[u8]> {
    if frame.len() < HEADER_BYTES || frame[0] != subtype || frame[10..16] != bssid {
        return None;
    }
    Some(&frame[HEADER_BYTES..])
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The status of `frame` when it is `bssid`'s answer to an open-system authentication request.
pub fn authentication_status(frame: &[u8], bssid: Address) -> Option<u16> {
    let body = from(frame, bssid, SUBTYPE_AUTHENTICATION)?;
    match (half(body, 0)?, half(body, 2)?) {
        (OPEN_SYSTEM, 2) => half(body, 4),
        _ => None,
    }
}

/// The status and association id of `frame` when it is `bssid`'s association response.
pub fn association_answer(frame: &[u8], bssid: Address) -> Option<(u16, u16)> {
    let body = from(frame, bssid, SUBTYPE_ASSOCIATION_RESPONSE)?;
    Some((half(body, 2)?, half(body, 4)? & ASSOCIATION_ID_BITS))
}

/// A beacon's or probe response's beacon interval, in time units.
pub fn beacon_interval(frame: &[u8]) -> Option<u16> {
    half(frame, BEACON_INTERVAL_AT)
}

/// The data frame `own` sends to `destination` through access point `bssid`: the MAC header
/// and the LLC/SNAP-wrapped payload of `ethertype`, unencrypted (CCMP protects the part after
/// the header).
pub fn data_frame(own: Address, bssid: Address, destination: Address, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = header(TYPE_DATA, TO_DS, bssid, own, destination);
    frame.extend_from_slice(&LLC_SNAP);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// The MAC header's length of data frame `frame`.
pub fn data_header_bytes(frame: &[u8]) -> Option<usize> {
    match *frame.first()? & 0xfc {
        TYPE_DATA => Some(HEADER_BYTES),
        TYPE_QOS_DATA => Some(QOS_HEADER_BYTES),
        _ => None,
    }
}

/// Whether `frame` is a data frame with its protected bit set.
pub fn is_protected_data(frame: &[u8]) -> bool {
    frame.len() > 1 && frame[0] & TYPE_MASK == TYPE_DATA && frame[1] & PROTECTED != 0
}

/// A data frame's payload from access point `bssid`, as Ethernet sees it: destination, source,
/// ethertype and the bytes after the LLC/SNAP header. `body` is the frame's body, decrypted when
/// it was protected.
pub fn ethernet_from(frame: &[u8], body: &[u8], bssid: Address) -> Option<(Address, Address, u16, Vec<u8>)> {
    if data_header_bytes(frame)? > frame.len() || frame[1] & (TO_DS | FROM_DS) != FROM_DS || frame[10..16] != bssid {
        return None;
    }
    if body.get(..6)? != LLC_SNAP {
        return None;
    }
    let destination: Address = frame[4..10].try_into().ok()?;
    let source: Address = frame[16..22].try_into().ok()?;
    let ethertype = u16::from_be_bytes(body.get(6..8)?.try_into().ok()?);
    Some((destination, source, ethertype, body[8..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const OWN: Address = [0, 0x11, 0x22, 0x33, 0x44, 0x55];
    const AP: Address = [2, 0, 0, 0, 0, 1];

    #[test]
    fn authentication_and_association_requests_carry_what_the_access_point_reads() {
        let auth = authentication_request(OWN, AP);
        assert_eq!(auth.len(), 30);
        assert_eq!((auth[0], &auth[4..10], &auth[10..16]), (0xb0, &AP[..], &OWN[..]));
        assert_eq!(&auth[24..30], &[0, 0, 1, 0, 0, 0]);
        let association = association_request(OWN, AP, b"home", true);
        assert_eq!(&association[28..34], &[0, 4, b'h', b'o', b'm', b'e']);
        assert!(association.ends_with(&WPA2_PERSONAL_RSN));
        assert_ne!(u16::from_le_bytes([association[24], association[25]]) & CAPABILITY_PRIVACY, 0);
    }

    #[test]
    fn the_answers_are_read_only_from_the_access_point() {
        let mut answer = header(SUBTYPE_AUTHENTICATION, 0, OWN, AP, AP);
        answer.extend_from_slice(&[0, 0, 2, 0, 0, 0]);
        assert_eq!(authentication_status(&answer, AP), Some(0));
        assert_eq!(authentication_status(&answer, OWN), None);
        let mut association = header(SUBTYPE_ASSOCIATION_RESPONSE, 0, OWN, AP, AP);
        association.extend_from_slice(&[0x11, 0x04, 0, 0, 0x05, 0xc0]);
        assert_eq!(association_answer(&association, AP), Some((0, 5)));
    }

    #[test]
    fn data_frames_wrap_and_unwrap_ethernet_payloads() {
        let frame = data_frame(OWN, AP, [9; 6], ETHERTYPE_EAPOL, &[1, 2, 3]);
        assert_eq!((frame[0], frame[1]), (TYPE_DATA, TO_DS));
        assert_eq!(&frame[16..22], &[9; 6]);
        assert_eq!(&frame[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        let mut received = header(TYPE_DATA, FROM_DS, OWN, AP, [7; 6]);
        let body = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 42];
        received.extend_from_slice(&body);
        assert_eq!(ethernet_from(&received, &body, AP), Some((OWN, [7; 6], 0x0800, vec![42])));
        assert_eq!(ethernet_from(&received, &body, OWN), None);
        assert!(!is_protected_data(&received));
    }
}

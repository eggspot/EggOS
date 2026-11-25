//! Protected management frames beyond D285's (D785; IEEE 802.11-2020, 12.5.4, 12.6.18, 11.13,
//! 9.6.9 and 12.6.1.3): BIP-CMAC-128 checked on group management frames, SA Query requests
//! answered, and PMKSA caching's PMKIDs, in the RSN element and in message 1's key data.

use crate::frames::Address;
use crate::handshake::{Akm, IGTK_BYTES, PMK_BYTES};
use alloc::vec::Vec;
use eggos_crypto::aes::Aes;
use eggos_crypto::cmac::cmac;
use eggos_crypto::constant_time::equal;
use eggos_crypto::hmac::hmac;
use eggos_crypto::sha1::hmac_sha1;
use eggos_crypto::sha2::HashAlgorithm;

const HEADER_BYTES: usize = 24;
const SUBTYPE_ACTION: u8 = 0xd0;
const CATEGORY_SA_QUERY: u8 = 8;
const SA_QUERY_REQUEST: u8 = 0;
const SA_QUERY_RESPONSE: u8 = 1;
const ELEMENT_MANAGEMENT_MIC: u8 = 76;
const MANAGEMENT_MIC_BYTES: usize = 2 + 16;
const MIC_BYTES: usize = 8;
const FLAGS_MASKED: u8 = 0x08 | 0x10 | 0x20;
const KDE_TYPE: u8 = 0xdd;
const KDE_PMKID: [u8; 4] = [0x00, 0x0f, 0xac, 4];
/// The bytes of a PMKID.
pub const PMKID_BYTES: usize = 16;

/// The transaction id of an SA Query request in `body`, an action frame's body (decrypted).
pub fn sa_query_request(body: &[u8]) -> Option<[u8; 2]> {
    match body {
        [CATEGORY_SA_QUERY, SA_QUERY_REQUEST, first, second, ..] => Some([*first, *second]),
        _ => None,
    }
}

/// Whether `frame` is an action frame.
pub fn is_action(frame: &[u8]) -> bool {
    frame.first() == Some(&SUBTYPE_ACTION)
}

/// The SA Query response `own` sends `bssid` for transaction `id`: its header and its body, for
/// the caller to protect.
pub fn sa_query_response(own: Address, bssid: Address, id: [u8; 2]) -> (Vec<u8>, [u8; 4]) {
    let mut header = Vec::with_capacity(HEADER_BYTES);
    header.extend_from_slice(&[SUBTYPE_ACTION, 0, 0, 0]);
    header.extend_from_slice(&bssid);
    header.extend_from_slice(&own);
    header.extend_from_slice(&bssid);
    header.extend_from_slice(&[0, 0]);
    (header, [CATEGORY_SA_QUERY, SA_QUERY_RESPONSE, id[0], id[1]])
}

/// Checks the BIP-CMAC-128 management MIC element ending group management frame `frame` under
/// IGTK `igtk` with key id `key_id`: its IPN when the MIC matches and the IPN is above `last`.
pub fn bip_check(frame: &[u8], igtk: &[u8; IGTK_BYTES], key_id: u16, last: u64) -> Option<u64> {
    let element_at = frame.len().checked_sub(2 + MANAGEMENT_MIC_BYTES)?;
    if element_at < HEADER_BYTES
        || frame[element_at] != ELEMENT_MANAGEMENT_MIC
        || usize::from(frame[element_at + 1]) != MANAGEMENT_MIC_BYTES
    {
        return None;
    }
    let element = &frame[element_at + 2..];
    if u16::from_le_bytes([element[0], element[1]]) != key_id {
        return None;
    }
    let mut ipn_bytes = [0u8; 8];
    ipn_bytes[..6].copy_from_slice(&element[2..8]);
    let ipn = u64::from_le_bytes(ipn_bytes);
    if ipn <= last {
        return None;
    }
    let mut associated = [0u8; 20];
    associated[0] = frame[0];
    associated[1] = frame[1] & !FLAGS_MASKED;
    associated[2..20].copy_from_slice(&frame[4..22]);
    let mut body = frame[HEADER_BYTES..].to_vec();
    let mic_at = body.len() - MIC_BYTES;
    let wanted: [u8; MIC_BYTES] = body[mic_at..].try_into().ok()?;
    body[mic_at..].fill(0);
    let cipher = Aes::new(igtk).ok()?;
    let mic = cmac(&cipher, &[&associated, &body]);
    equal(&mic[..MIC_BYTES], &wanted).then_some(ipn)
}

/// The PMKID of `pmk` between authenticator `aa` and supplicant `spa` for an 802.1X or PSK
/// key management: HMAC-SHA1 (AKM 1, 2) or HMAC-SHA-256 (AKM 5, 6), truncated to 128 bits.
pub fn pmkid(akm: Akm, pmk: &[u8; PMK_BYTES], aa: Address, spa: Address) -> [u8; PMKID_BYTES] {
    let label = b"PMK Name";
    let mut data = [0u8; 8 + 6 + 6];
    data[..8].copy_from_slice(label);
    data[8..14].copy_from_slice(&aa);
    data[14..].copy_from_slice(&spa);
    let mut id = [0u8; PMKID_BYTES];
    match akm.uses_sha1() {
        true => id.copy_from_slice(&hmac_sha1(pmk, &[&data])[..PMKID_BYTES]),
        false => {
            id.copy_from_slice(&hmac(HashAlgorithm::Sha256, pmk, &data).as_bytes()[..PMKID_BYTES])
        }
    }
    id
}

/// The RSN element `rsn` with `pmkid` listed in it (after the capabilities, before a group
/// management cipher), as a station asking for a cached PMKSA sends it.
pub fn rsn_with_pmkid(rsn: &[u8], pmkid: &[u8; PMKID_BYTES]) -> Option<Vec<u8>> {
    let count_at = |at: usize| -> Option<usize> {
        Some(usize::from(u16::from_le_bytes([*rsn.get(at)?, *rsn.get(at + 1)?])))
    };
    let pairwise = 2 + 2 + 4;
    let akm_at = pairwise + 2 + 4 * count_at(pairwise)?;
    let capabilities_at = akm_at + 2 + 4 * count_at(akm_at)?;
    let after = capabilities_at + 2;
    if rsn.len() < after {
        return None;
    }
    let rest = match rsn.len() >= after + 2 {
        true => &rsn[after + 2 + 16 * count_at(after)?..],
        false => &[][..],
    };
    let mut element = rsn[..after].to_vec();
    element.extend_from_slice(&1u16.to_le_bytes());
    element.extend_from_slice(pmkid);
    element.extend_from_slice(rest);
    element[1] = (element.len() - 2) as u8;
    Some(element)
}

/// The PMKID KDE in message 1's key data, if it has one.
pub fn message_one_pmkid(key_data: &[u8]) -> Option<[u8; PMKID_BYTES]> {
    let mut rest = key_data;
    while rest.len() >= 2 {
        let (kind, length) = (rest[0], usize::from(rest[1]));
        let body = rest.get(2..2 + length)?;
        if kind == KDE_TYPE && body.len() == 4 + PMKID_BYTES && body[..4] == KDE_PMKID {
            return body[4..].try_into().ok();
        }
        rest = &rest[2 + length..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{WPA2_PERSONAL_RSN, WPA3_PERSONAL_RSN};

    #[test]
    fn a_pmkid_goes_after_the_capabilities_and_before_the_group_management_cipher() {
        let id = [9u8; 16];
        let wpa2 = rsn_with_pmkid(&WPA2_PERSONAL_RSN, &id).unwrap();
        assert_eq!(wpa2.len(), WPA2_PERSONAL_RSN.len() + 18);
        assert_eq!(&wpa2[22..24], &[1, 0]);
        let wpa3 = rsn_with_pmkid(&WPA3_PERSONAL_RSN, &id).unwrap();
        assert_eq!(wpa3.len(), WPA3_PERSONAL_RSN.len() + 16);
        assert_eq!(&wpa3[wpa3.len() - 4..], &[0x00, 0x0f, 0xac, 6]);
        assert_eq!(wpa3[1] as usize, wpa3.len() - 2);
    }

    #[test]
    fn an_sa_query_request_is_answered_with_its_transaction_id() {
        assert_eq!(sa_query_request(&[8, 0, 0x12, 0x34]), Some([0x12, 0x34]));
        assert_eq!(sa_query_request(&[8, 1, 0x12, 0x34]), None);
        let (header, body) = sa_query_response([1; 6], [2; 6], [0x12, 0x34]);
        assert_eq!(header.len(), 24);
        assert_eq!(body, [8, 1, 0x12, 0x34]);
    }

    #[test]
    fn bip_takes_its_own_mic_and_refuses_a_replayed_ipn() {
        let igtk = [3u8; 16];
        let mut frame = vec![0xc0, 0x00, 0, 0];
        frame.extend_from_slice(&[0xff; 6]);
        frame.extend_from_slice(&[2; 6]);
        frame.extend_from_slice(&[2; 6]);
        frame.extend_from_slice(&[0, 0, 7, 0]);
        frame.extend_from_slice(&[ELEMENT_MANAGEMENT_MIC, 16, 4, 0, 5, 0, 0, 0, 0, 0]);
        frame.extend_from_slice(&[0; 8]);
        let mut associated = [0u8; 20];
        associated[0] = 0xc0;
        associated[2..].copy_from_slice(&frame[4..22]);
        let cipher = Aes::new(&igtk).unwrap();
        let mic = cmac(&cipher, &[&associated, &frame[24..]]);
        let end = frame.len();
        frame[end - 8..].copy_from_slice(&mic[..8]);
        assert_eq!(bip_check(&frame, &igtk, 4, 0), Some(5));
        assert_eq!(bip_check(&frame, &igtk, 4, 5), None);
        assert_eq!(bip_check(&frame, &igtk, 5, 0), None);
    }
}

//! CCMP (IEEE 802.11-2020, 12.5.3): data frames encrypted and authenticated with AES-CCM under
//! the temporal key, with the packet number in the CCMP header and the MAC header in the AAD.
//! Frames with a fourth address (between access points) are not taken. With management frame
//! protection (WPA3, D285) unicast management frames are protected the same way, their nonce
//! flags marking them as management frames.

use eggos_crypto::aes::Aes;
use eggos_crypto::ccm::{self, NONCE_BYTES, TAG_BYTES};

pub const CCMP_HEADER_BYTES: usize = 8;
const EXTENDED_IV: u8 = 1 << 5;
const PROTECTED: u8 = 0x40;
const DATA_HEADER_BYTES: usize = 24;
const QOS_CONTROL_BYTES: usize = 2;
const QOS_SUBTYPE_BIT: u8 = 0x80;
const MAX_AAD_BYTES: usize = 30;

/// Why a frame was not decrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionError {
    /// Not a protected data frame with a CCMP header this module reads.
    NotCcmp,
    /// The output does not hold the frame.
    TooSmall,
    /// The MIC did not match.
    Forged,
}

const NONCE_MANAGEMENT: u8 = 1 << 4;

fn is_management(header: &[u8]) -> bool {
    header[0] & 0x0c == 0
}

fn header_length(frame: &[u8]) -> Option<usize> {
    let control = crate::frames::frame_control(frame)?;
    if control.kind == crate::frames::FRAME_TYPE_MANAGEMENT {
        return Some(DATA_HEADER_BYTES);
    }
    if control.kind != crate::frames::FRAME_TYPE_DATA || (control.to_ds && control.from_ds) {
        return None;
    }
    let qos = frame[0] & QOS_SUBTYPE_BIT != 0;
    Some(DATA_HEADER_BYTES + if qos { QOS_CONTROL_BYTES } else { 0 })
}

fn aad(header: &[u8], out: &mut [u8; MAX_AAD_BYTES]) -> usize {
    let management = is_management(header);
    let qos = !management && header[0] & QOS_SUBTYPE_BIT != 0;
    out[0] = if management { header[0] } else { header[0] & 0x8f };
    out[1] = (header[1] & !0x38 & if qos { 0x7f } else { 0xff }) | PROTECTED;
    out[2..20].copy_from_slice(&header[4..22]);
    out[20] = header[22] & 0x0f;
    out[21] = 0;
    if qos {
        out[22] = header[24] & 0x0f;
        out[23] = 0;
        24
    } else {
        22
    }
}

fn nonce(header: &[u8], packet_number: u64) -> [u8; NONCE_BYTES] {
    let mut nonce = [0u8; NONCE_BYTES];
    if is_management(header) {
        nonce[0] = NONCE_MANAGEMENT;
    } else if header[0] & QOS_SUBTYPE_BIT != 0 {
        nonce[0] = header[24] & 0x0f;
    }
    nonce[1..7].copy_from_slice(&header[10..16]);
    nonce[7..].copy_from_slice(&packet_number.to_be_bytes()[2..]);
    nonce
}

/// Encrypts the payload of a data frame whose MAC header is `header` with packet number
/// `packet_number` and key `key_id`, writing the whole protected frame into `out`: its length.
pub fn encrypt(
    tk: &[u8; 16],
    header: &[u8],
    packet_number: u64,
    key_id: u8,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, ProtectionError> {
    let length = header_length(header).filter(|&length| length <= header.len()).ok_or(ProtectionError::NotCcmp)?;
    let total = length + CCMP_HEADER_BYTES + payload.len() + TAG_BYTES;
    if out.len() < total {
        return Err(ProtectionError::TooSmall);
    }
    let cipher = Aes::new(tk).map_err(|_| ProtectionError::NotCcmp)?;
    out[..length].copy_from_slice(&header[..length]);
    out[1] |= PROTECTED;
    let pn = packet_number.to_le_bytes();
    out[length..length + CCMP_HEADER_BYTES].copy_from_slice(&[pn[0], pn[1], 0, EXTENDED_IV | (key_id << 6), pn[2], pn[3], pn[4], pn[5]]);
    let body = length + CCMP_HEADER_BYTES;
    out[body..body + payload.len()].copy_from_slice(payload);
    let mut associated = [0u8; MAX_AAD_BYTES];
    let associated_length = aad(&out[..length], &mut associated);
    let tag = ccm::seal(&cipher, &nonce(&out[..length], packet_number), &associated[..associated_length], &mut out[body..body + payload.len()]);
    out[body + payload.len()..total].copy_from_slice(&tag);
    Ok(total)
}

/// Decrypts a protected data frame into `out`: the payload's length and its packet number, for
/// the caller's replay check.
pub fn decrypt(tk: &[u8; 16], frame: &[u8], out: &mut [u8]) -> Result<(usize, u64), ProtectionError> {
    let length = header_length(frame).ok_or(ProtectionError::NotCcmp)?;
    if frame[1] & PROTECTED == 0 || frame.len() < length + CCMP_HEADER_BYTES + TAG_BYTES {
        return Err(ProtectionError::NotCcmp);
    }
    let ccmp = &frame[length..length + CCMP_HEADER_BYTES];
    if ccmp[3] & EXTENDED_IV == 0 {
        return Err(ProtectionError::NotCcmp);
    }
    let packet_number = u64::from_le_bytes([ccmp[0], ccmp[1], ccmp[4], ccmp[5], ccmp[6], ccmp[7], 0, 0]);
    let body = &frame[length + CCMP_HEADER_BYTES..frame.len() - TAG_BYTES];
    let tag: [u8; TAG_BYTES] = frame[frame.len() - TAG_BYTES..].try_into().map_err(|_| ProtectionError::NotCcmp)?;
    let plain = out.get_mut(..body.len()).ok_or(ProtectionError::TooSmall)?;
    plain.copy_from_slice(body);
    let cipher = Aes::new(tk).map_err(|_| ProtectionError::NotCcmp)?;
    let mut associated = [0u8; MAX_AAD_BYTES];
    let associated_length = aad(&frame[..length], &mut associated);
    ccm::open(&cipher, &nonce(&frame[..length], packet_number), &associated[..associated_length], plain, &tag)
        .map_err(|_| ProtectionError::Forged)?;
    Ok((body.len(), packet_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_header() -> [u8; 24] {
        let mut header = [0u8; 24];
        header[0] = 0x08;
        header[1] = 0x01;
        header[4..10].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        header[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        header[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 3]);
        header
    }

    #[test]
    fn a_protected_frame_decrypts_to_its_payload_and_number() {
        let tk = [0x11u8; 16];
        let mut frame = [0u8; 128];
        let length = encrypt(&tk, &data_header(), 0x0102_0304_0506, 0, b"hello, network", &mut frame).unwrap();
        assert_eq!(frame[1] & PROTECTED, PROTECTED);
        let mut payload = [0u8; 64];
        let (bytes, number) = decrypt(&tk, &frame[..length], &mut payload).unwrap();
        assert_eq!(&payload[..bytes], b"hello, network");
        assert_eq!(number, 0x0102_0304_0506);
    }

    #[test]
    fn a_changed_header_or_body_is_forged() {
        let tk = [0x11u8; 16];
        let mut frame = [0u8; 128];
        let length = encrypt(&tk, &data_header(), 7, 0, b"payload", &mut frame).unwrap();
        let mut payload = [0u8; 64];
        let mut changed = frame;
        changed[10] ^= 1;
        assert_eq!(decrypt(&tk, &changed[..length], &mut payload), Err(ProtectionError::Forged));
        let mut changed = frame;
        changed[length - 9] ^= 1;
        assert_eq!(decrypt(&tk, &changed[..length], &mut payload), Err(ProtectionError::Forged));
    }
}

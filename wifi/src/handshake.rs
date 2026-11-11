//! WPA2-Personal's keys and four-way handshake as a supplicant (IEEE 802.11-2020, 12.7): the
//! PMK from the passphrase, the PTK by PRF-384, EAPOL-Key frames with HMAC-SHA1-128 MICs (key
//! descriptor version 2), and the GTK unwrapped from message 3's key data. WPA3-Personal's
//! handshake after SAE (D285) derives the PTK with KDF-SHA-256, signs with AES-CMAC (key
//! descriptor version 0, the AKM's own) and also unwraps the IGTK protected management frames
//! use.

use crate::frames::{Address, WPA2_PERSONAL_RSN, WPA3_PERSONAL_RSN};
use crate::kdf::kdf_sha256;
use eggos_crypto::cmac::cmac;
use eggos_crypto::aes::Aes;
use eggos_crypto::constant_time::equal;
use eggos_crypto::key_wrap;
use eggos_crypto::sha1::{hmac_sha1, pbkdf2_hmac_sha1};

pub const PMK_BYTES: usize = 32;
pub const NONCE_BYTES: usize = 32;
const KCK_BYTES: usize = 16;
const KEK_BYTES: usize = 16;
pub const TK_BYTES: usize = 16;
const PTK_BYTES: usize = KCK_BYTES + KEK_BYTES + TK_BYTES;
const MIC_BYTES: usize = 16;
pub const MAX_GTK_BYTES: usize = 32;

const EAPOL_VERSION: u8 = 2;
const EAPOL_KEY: u8 = 3;
const DESCRIPTOR_RSN: u8 = 2;
const KEY_INFO_VERSION_HMAC_SHA1_AES: u16 = 2;
const KEY_INFO_VERSION_AKM_DEFINED: u16 = 0;
const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_INSTALL: u16 = 1 << 6;
const KEY_INFO_ACK: u16 = 1 << 7;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_INFO_ENCRYPTED_DATA: u16 = 1 << 12;
const EAPOL_HEADER_BYTES: usize = 4;
const KEY_FRAME_FIXED_BYTES: usize = 95;
const MIC_AT: usize = EAPOL_HEADER_BYTES + 77;
const KEY_DATA_LENGTH_AT: usize = EAPOL_HEADER_BYTES + 93;
const NONCE_AT: usize = EAPOL_HEADER_BYTES + 13;
const REPLAY_AT: usize = EAPOL_HEADER_BYTES + 5;
const KEY_INFO_AT: usize = EAPOL_HEADER_BYTES + 1;
pub const MAX_KEY_FRAME_BYTES: usize = EAPOL_HEADER_BYTES + KEY_FRAME_FIXED_BYTES + 256;
const KDE_TYPE: u8 = 0xdd;
const KDE_GTK: [u8; 4] = [0x00, 0x0f, 0xac, 1];
const KDE_IGTK: [u8; 4] = [0x00, 0x0f, 0xac, 9];
/// The bytes of a BIP-CMAC-128 integrity group key.
pub const IGTK_BYTES: usize = 16;

/// The key management a handshake follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Akm {
    /// WPA2-Personal: a PMK from the passphrase, PRF-SHA1 and HMAC-SHA1 MICs.
    Psk,
    /// WPA3-Personal: a PMK from SAE, KDF-SHA-256 and AES-CMAC MICs, management frames protected.
    Sae,
}

impl Akm {
    fn key_info_version(self) -> u16 {
        match self {
            Akm::Psk => KEY_INFO_VERSION_HMAC_SHA1_AES,
            Akm::Sae => KEY_INFO_VERSION_AKM_DEFINED,
        }
    }

    fn rsn(self) -> &'static [u8] {
        match self {
            Akm::Psk => &WPA2_PERSONAL_RSN,
            Akm::Sae => &WPA3_PERSONAL_RSN,
        }
    }
}

/// The pairwise master key of a passphrase (8 to 63 printable characters) on network `ssid`.
pub fn pmk_from_passphrase(passphrase: &[u8], ssid: &[u8]) -> Option<[u8; PMK_BYTES]> {
    if !(8..=63).contains(&passphrase.len()) || ssid.len() > 32 {
        return None;
    }
    let mut pmk = [0u8; PMK_BYTES];
    pbkdf2_hmac_sha1(passphrase, ssid, 4096, &mut pmk);
    Some(pmk)
}

/// IEEE 802.11's PRF over HMAC-SHA1 (12.7.1.2), filling `out`.
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    for (index, chunk) in out.chunks_mut(20).enumerate() {
        let block = hmac_sha1(key, &[label, &[0], data, &[index as u8]]);
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
}

/// The pairwise transient key: its confirmation key, encryption key and temporal key.
#[derive(Clone, PartialEq, Eq)]
pub struct Ptk {
    pub kck: [u8; KCK_BYTES],
    pub kek: [u8; KEK_BYTES],
    pub tk: [u8; TK_BYTES],
}

impl core::fmt::Debug for Ptk {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Ptk")
    }
}

/// The PTK of `pmk` for `akm` between authenticator `authenticator` and supplicant
/// `supplicant` with their nonces.
pub fn ptk_for(
    akm: Akm,
    pmk: &[u8; PMK_BYTES],
    authenticator: Address,
    supplicant: Address,
    a_nonce: &[u8; NONCE_BYTES],
    s_nonce: &[u8; NONCE_BYTES],
) -> Ptk {
    if akm == Akm::Psk {
        return ptk(pmk, authenticator, supplicant, a_nonce, s_nonce);
    }
    let mut data = [0u8; 6 + 6 + NONCE_BYTES + NONCE_BYTES];
    let (low, high) = if authenticator <= supplicant { (authenticator, supplicant) } else { (supplicant, authenticator) };
    let (first, second) = if a_nonce <= s_nonce { (a_nonce, s_nonce) } else { (s_nonce, a_nonce) };
    data[..6].copy_from_slice(&low);
    data[6..12].copy_from_slice(&high);
    data[12..44].copy_from_slice(first);
    data[44..].copy_from_slice(second);
    let mut bytes = [0u8; PTK_BYTES];
    kdf_sha256(pmk, b"Pairwise key expansion", &data, &mut bytes);
    Ptk {
        kck: bytes[..16].try_into().unwrap_or_default(),
        kek: bytes[16..32].try_into().unwrap_or_default(),
        tk: bytes[32..48].try_into().unwrap_or_default(),
    }
}

/// The PTK of `pmk` between authenticator `authenticator` and supplicant `supplicant` with their
/// nonces.
pub fn ptk(
    pmk: &[u8; PMK_BYTES],
    authenticator: Address,
    supplicant: Address,
    a_nonce: &[u8; NONCE_BYTES],
    s_nonce: &[u8; NONCE_BYTES],
) -> Ptk {
    let mut data = [0u8; 6 + 6 + NONCE_BYTES + NONCE_BYTES];
    let (low, high) = if authenticator <= supplicant {
        (authenticator, supplicant)
    } else {
        (supplicant, authenticator)
    };
    let (first, second) = if a_nonce <= s_nonce {
        (a_nonce, s_nonce)
    } else {
        (s_nonce, a_nonce)
    };
    data[..6].copy_from_slice(&low);
    data[6..12].copy_from_slice(&high);
    data[12..44].copy_from_slice(first);
    data[44..].copy_from_slice(second);
    let mut bytes = [0u8; PTK_BYTES];
    prf(pmk, b"Pairwise key expansion", &data, &mut bytes);
    Ptk {
        kck: bytes[..16].try_into().unwrap_or_default(),
        kek: bytes[16..32].try_into().unwrap_or_default(),
        tk: bytes[32..48].try_into().unwrap_or_default(),
    }
}

/// An EAPOL-Key frame's fields the handshake reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyFrame<'a> {
    pub info: u16,
    pub replay_counter: u64,
    pub nonce: [u8; NONCE_BYTES],
    pub mic: [u8; MIC_BYTES],
    pub data: &'a [u8],
}

/// Parses an EAPOL frame (from its version byte) holding an RSN EAPOL-Key descriptor.
pub fn key_frame(frame: &[u8]) -> Option<KeyFrame<'_>> {
    if frame.len() < EAPOL_HEADER_BYTES + KEY_FRAME_FIXED_BYTES
        || frame[1] != EAPOL_KEY
        || frame[EAPOL_HEADER_BYTES] != DESCRIPTOR_RSN
    {
        return None;
    }
    let body = usize::from(u16::from_be_bytes([frame[2], frame[3]]));
    let frame = frame.get(..EAPOL_HEADER_BYTES + body)?;
    let data_length = usize::from(u16::from_be_bytes([frame[KEY_DATA_LENGTH_AT], frame[KEY_DATA_LENGTH_AT + 1]]));
    let start = EAPOL_HEADER_BYTES + KEY_FRAME_FIXED_BYTES;
    Some(KeyFrame {
        info: u16::from_be_bytes([frame[KEY_INFO_AT], frame[KEY_INFO_AT + 1]]),
        replay_counter: u64::from_be_bytes(frame[REPLAY_AT..REPLAY_AT + 8].try_into().ok()?),
        nonce: frame[NONCE_AT..NONCE_AT + NONCE_BYTES].try_into().ok()?,
        mic: frame[MIC_AT..MIC_AT + MIC_BYTES].try_into().ok()?,
        data: frame.get(start..start + data_length)?,
    })
}

/// The MIC of an EAPOL frame under `kck`, the frame's MIC field taken as zero.
pub fn mic(kck: &[u8; KCK_BYTES], frame: &[u8]) -> [u8; MIC_BYTES] {
    let mut copy = [0u8; MAX_KEY_FRAME_BYTES];
    let length = frame.len().min(copy.len());
    copy[..length].copy_from_slice(&frame[..length]);
    if length >= MIC_AT + MIC_BYTES {
        copy[MIC_AT..MIC_AT + MIC_BYTES].fill(0);
    }
    let digest = hmac_sha1(kck, &[&copy[..length]]);
    digest[..MIC_BYTES].try_into().unwrap_or_default()
}

/// The MIC `akm` puts on an EAPOL frame under `kck`, the frame's MIC field taken as zero.
pub fn mic_for(akm: Akm, kck: &[u8; KCK_BYTES], frame: &[u8]) -> [u8; MIC_BYTES] {
    if akm == Akm::Psk {
        return mic(kck, frame);
    }
    let mut copy = [0u8; MAX_KEY_FRAME_BYTES];
    let length = frame.len().min(copy.len());
    copy[..length].copy_from_slice(&frame[..length]);
    if length >= MIC_AT + MIC_BYTES {
        copy[MIC_AT..MIC_AT + MIC_BYTES].fill(0);
    }
    match Aes::new(kck) {
        Ok(cipher) => cmac(&cipher, &[&copy[..length]]),
        Err(_) => [0; MIC_BYTES],
    }
}

/// Writes an EAPOL-Key frame into `out`: its length.
pub fn write_key_frame(
    out: &mut [u8; MAX_KEY_FRAME_BYTES],
    info: u16,
    replay_counter: u64,
    nonce: &[u8; NONCE_BYTES],
    data: &[u8],
    kck: Option<&[u8; KCK_BYTES]>,
) -> usize {
    write_key_frame_for(Akm::Psk, out, info, replay_counter, nonce, data, kck)
}

/// Writes an EAPOL-Key frame for `akm` into `out`: its length.
pub fn write_key_frame_for(
    akm: Akm,
    out: &mut [u8; MAX_KEY_FRAME_BYTES],
    info: u16,
    replay_counter: u64,
    nonce: &[u8; NONCE_BYTES],
    data: &[u8],
    kck: Option<&[u8; KCK_BYTES]>,
) -> usize {
    out.fill(0);
    let data = &data[..data.len().min(256)];
    let length = EAPOL_HEADER_BYTES + KEY_FRAME_FIXED_BYTES + data.len();
    out[0] = EAPOL_VERSION;
    out[1] = EAPOL_KEY;
    out[2..4].copy_from_slice(&((length - EAPOL_HEADER_BYTES) as u16).to_be_bytes());
    out[EAPOL_HEADER_BYTES] = DESCRIPTOR_RSN;
    out[KEY_INFO_AT..KEY_INFO_AT + 2].copy_from_slice(&info.to_be_bytes());
    out[KEY_INFO_AT + 2..KEY_INFO_AT + 4].copy_from_slice(&(TK_BYTES as u16).to_be_bytes());
    out[REPLAY_AT..REPLAY_AT + 8].copy_from_slice(&replay_counter.to_be_bytes());
    out[NONCE_AT..NONCE_AT + NONCE_BYTES].copy_from_slice(nonce);
    out[KEY_DATA_LENGTH_AT..KEY_DATA_LENGTH_AT + 2].copy_from_slice(&(data.len() as u16).to_be_bytes());
    out[EAPOL_HEADER_BYTES + KEY_FRAME_FIXED_BYTES..length].copy_from_slice(data);
    if let Some(kck) = kck {
        let code = mic_for(akm, kck, &out[..length]);
        out[MIC_AT..MIC_AT + MIC_BYTES].copy_from_slice(&code);
    }
    length
}

/// Why the handshake stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    /// Not the message the handshake waits for.
    Unexpected,
    /// A replay counter not above the last one.
    Replayed,
    /// The MIC did not match: most often the wrong passphrase.
    BadMic,
    /// Message 3's key data did not unwrap, or held no group key.
    BadKeyData,
}

/// The keys the handshake ends with, for the adapter to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub tk: [u8; TK_BYTES],
    pub gtk: [u8; MAX_GTK_BYTES],
    pub gtk_bytes: usize,
    pub gtk_index: u8,
    /// The integrity group key and its key id, when management frames are protected (WPA3).
    pub igtk: Option<([u8; IGTK_BYTES], u16)>,
}

/// A supplicant's side of the four-way handshake.
#[derive(Debug)]
pub struct Supplicant {
    akm: Akm,
    pmk: [u8; PMK_BYTES],
    own: Address,
    authenticator: Address,
    s_nonce: [u8; NONCE_BYTES],
    ptk: Option<Ptk>,
    replay_counter: Option<u64>,
}

/// What the supplicant answers a frame with.
#[derive(Debug)]
pub struct Answer {
    pub frame: [u8; MAX_KEY_FRAME_BYTES],
    pub length: usize,
    pub keys: Option<Keys>,
}

impl Supplicant {
    /// A handshake for station `own` with access point `authenticator`; `s_nonce` must be fresh
    /// random bytes.
    pub fn new(pmk: [u8; PMK_BYTES], own: Address, authenticator: Address, s_nonce: [u8; NONCE_BYTES]) -> Self {
        Self::for_akm(Akm::Psk, pmk, own, authenticator, s_nonce)
    }

    /// A handshake following `akm`, otherwise as [`Self::new`].
    pub fn for_akm(akm: Akm, pmk: [u8; PMK_BYTES], own: Address, authenticator: Address, s_nonce: [u8; NONCE_BYTES]) -> Self {
        Self {
            akm,
            pmk,
            own,
            authenticator,
            s_nonce,
            ptk: None,
            replay_counter: None,
        }
    }

    fn check_replay(&mut self, counter: u64) -> Result<(), HandshakeError> {
        if self.replay_counter.is_some_and(|last| counter <= last) {
            return Err(HandshakeError::Replayed);
        }
        self.replay_counter = Some(counter);
        Ok(())
    }

    /// Answers EAPOL frame `frame` from the authenticator: message 2 for message 1, message 4 for
    /// message 3, which also gives the keys.
    pub fn receive(&mut self, frame: &[u8]) -> Result<Answer, HandshakeError> {
        let key = key_frame(frame).ok_or(HandshakeError::Unexpected)?;
        let pairwise_ack = KEY_INFO_PAIRWISE | KEY_INFO_ACK;
        if key.info & pairwise_ack != pairwise_ack {
            return Err(HandshakeError::Unexpected);
        }
        let mut answer = Answer {
            frame: [0; MAX_KEY_FRAME_BYTES],
            length: 0,
            keys: None,
        };
        if key.info & KEY_INFO_MIC == 0 {
            self.check_replay(key.replay_counter)?;
            let ptk = ptk_for(self.akm, &self.pmk, self.authenticator, self.own, &key.nonce, &self.s_nonce);
            let info = self.akm.key_info_version() | KEY_INFO_PAIRWISE | KEY_INFO_MIC;
            answer.length = write_key_frame_for(
                self.akm,
                &mut answer.frame,
                info,
                key.replay_counter,
                &self.s_nonce,
                self.akm.rsn(),
                Some(&ptk.kck),
            );
            self.ptk = Some(ptk);
            return Ok(answer);
        }
        let ptk = self.ptk.clone().ok_or(HandshakeError::Unexpected)?;
        if !equal(&mic_for(self.akm, &ptk.kck, frame), &key.mic) {
            return Err(HandshakeError::BadMic);
        }
        if key.info & (KEY_INFO_INSTALL | KEY_INFO_SECURE | KEY_INFO_ENCRYPTED_DATA) != KEY_INFO_INSTALL | KEY_INFO_SECURE | KEY_INFO_ENCRYPTED_DATA {
            return Err(HandshakeError::Unexpected);
        }
        self.check_replay(key.replay_counter)?;
        let kek = Aes::new(&ptk.kek).map_err(|_| HandshakeError::BadKeyData)?;
        let mut data = [0u8; 248];
        let unwrapped = key.data.len().checked_sub(8).filter(|&length| length <= data.len()).ok_or(HandshakeError::BadKeyData)?;
        key_wrap::unwrap(&kek, key.data, &mut data[..unwrapped]).map_err(|_| HandshakeError::BadKeyData)?;
        let (gtk_index, gtk) = group_key(&data[..unwrapped]).ok_or(HandshakeError::BadKeyData)?;
        let igtk = integrity_group_key(&data[..unwrapped]);
        if self.akm == Akm::Sae && igtk.is_none() {
            return Err(HandshakeError::BadKeyData);
        }
        let mut keys = Keys {
            tk: ptk.tk,
            gtk: [0; MAX_GTK_BYTES],
            gtk_bytes: gtk.len(),
            gtk_index,
            igtk,
        };
        keys.gtk[..gtk.len()].copy_from_slice(gtk);
        let info = self.akm.key_info_version() | KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_INFO_SECURE;
        answer.length = write_key_frame_for(self.akm, &mut answer.frame, info, key.replay_counter, &[0; NONCE_BYTES], &[], Some(&ptk.kck));
        answer.keys = Some(keys);
        Ok(answer)
    }
}

/// The IGTK KDE in unwrapped key data: the key and its key id.
pub fn integrity_group_key(data: &[u8]) -> Option<([u8; IGTK_BYTES], u16)> {
    let mut rest = data;
    while rest.len() >= 2 {
        let (kind, length) = (rest[0], usize::from(rest[1]));
        if kind == 0 {
            return None;
        }
        let body = rest.get(2..2 + length)?;
        if kind == KDE_TYPE && body.len() == 4 + 2 + 6 + IGTK_BYTES && body[..4] == KDE_IGTK {
            let key_id = u16::from_le_bytes([body[4], body[5]]);
            return Some((body[12..].try_into().ok()?, key_id));
        }
        rest = &rest[2 + length..];
    }
    None
}

/// The group key KDE in unwrapped key data: its key index and the key.
pub fn group_key(data: &[u8]) -> Option<(u8, &[u8])> {
    let mut rest = data;
    while rest.len() >= 2 {
        let (kind, length) = (rest[0], usize::from(rest[1]));
        if kind == 0 {
            return None;
        }
        let body = rest.get(2..2 + length)?;
        if kind == KDE_TYPE && body.len() > 6 && body[..4] == KDE_GTK {
            let key = &body[6..];
            return (key.len() <= MAX_GTK_BYTES).then_some((body[4] & 0b11, key));
        }
        rest = &rest[2 + length..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const AP: Address = [2, 0, 0, 0, 0, 1];
    const STATION: Address = [2, 0, 0, 0, 0, 2];

    #[test]
    fn the_passphrase_s_pmk_is_ieee_s_vector() {
        let pmk = pmk_from_passphrase(b"password", b"IEEE").unwrap();
        assert_eq!(pmk[..4], [0xf4, 0x2c, 0x6f, 0xc5]);
        assert!(pmk_from_passphrase(b"short", b"x").is_none());
    }

    #[test]
    fn the_ptk_does_not_depend_on_who_is_first() {
        let pmk = [7u8; 32];
        let (a, s) = ([1u8; 32], [2u8; 32]);
        assert_eq!(ptk(&pmk, AP, STATION, &a, &s), ptk(&pmk, STATION, AP, &s, &a));
    }

    #[test]
    fn a_whole_handshake_ends_with_the_authenticator_s_group_key() {
        let pmk = [9u8; 32];
        let a_nonce = [3u8; 32];
        let s_nonce = [4u8; 32];
        let mut supplicant = Supplicant::new(pmk, STATION, AP, s_nonce);
        let mut message = [0u8; MAX_KEY_FRAME_BYTES];
        let length = write_key_frame(&mut message, 2 | KEY_INFO_PAIRWISE | KEY_INFO_ACK, 1, &a_nonce, &[], None);
        let second = supplicant.receive(&message[..length]).unwrap();
        let second_frame = key_frame(&second.frame[..second.length]).unwrap();
        assert_eq!(second_frame.nonce, s_nonce);
        let keys = ptk(&pmk, AP, STATION, &a_nonce, &s_nonce);
        assert_eq!(mic(&keys.kck, &second.frame[..second.length]), second_frame.mic);
        let gtk = [0x55u8; 16];
        let mut kde = vec![KDE_TYPE, 22, 0x00, 0x0f, 0xac, 1, 1, 0];
        kde.extend_from_slice(&gtk);
        kde.resize(24, 0);
        let mut wrapped = [0u8; 32];
        key_wrap::wrap(&Aes::new(&keys.kek).unwrap(), &kde, &mut wrapped).unwrap();
        let info = 2 | KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_INFO_INSTALL | KEY_INFO_MIC | KEY_INFO_SECURE | KEY_INFO_ENCRYPTED_DATA;
        let length = write_key_frame(&mut message, info, 2, &a_nonce, &wrapped, Some(&keys.kck));
        let fourth = supplicant.receive(&message[..length]).unwrap();
        let installed = fourth.keys.unwrap();
        assert_eq!(installed.tk, keys.tk);
        assert_eq!(&installed.gtk[..installed.gtk_bytes], &gtk);
        assert_eq!(installed.gtk_index, 1);
        assert_eq!(supplicant.receive(&message[..length]).unwrap_err(), HandshakeError::Replayed);
    }

    #[test]
    fn wpa3_s_handshake_signs_with_cmac_and_needs_the_igtk() {
        let pmk = [6u8; 32];
        let (a_nonce, s_nonce) = ([3u8; 32], [4u8; 32]);
        let mut supplicant = Supplicant::for_akm(Akm::Sae, pmk, STATION, AP, s_nonce);
        let mut message = [0u8; MAX_KEY_FRAME_BYTES];
        let length = write_key_frame_for(Akm::Sae, &mut message, KEY_INFO_PAIRWISE | KEY_INFO_ACK, 1, &a_nonce, &[], None);
        let second = supplicant.receive(&message[..length]).unwrap();
        let keys = ptk_for(Akm::Sae, &pmk, AP, STATION, &a_nonce, &s_nonce);
        assert_ne!(keys, ptk(&pmk, AP, STATION, &a_nonce, &s_nonce));
        let second_frame = key_frame(&second.frame[..second.length]).unwrap();
        assert_eq!(second_frame.info & 7, 0);
        assert_eq!(mic_for(Akm::Sae, &keys.kck, &second.frame[..second.length]), second_frame.mic);
        let mut kde = vec![KDE_TYPE, 22, 0x00, 0x0f, 0xac, 1, 1, 0];
        kde.extend_from_slice(&[0x55u8; 16]);
        kde.extend_from_slice(&[KDE_TYPE, 28, 0x00, 0x0f, 0xac, 9, 4, 0, 0, 0, 0, 0, 0, 0]);
        kde.extend_from_slice(&[0x66u8; 16]);
        kde.resize(56, 0);
        let mut wrapped = [0u8; 64];
        key_wrap::wrap(&Aes::new(&keys.kek).unwrap(), &kde, &mut wrapped).unwrap();
        let info = KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_INFO_INSTALL | KEY_INFO_MIC | KEY_INFO_SECURE | KEY_INFO_ENCRYPTED_DATA;
        let length = write_key_frame_for(Akm::Sae, &mut message, info, 2, &a_nonce, &wrapped, Some(&keys.kck));
        let installed = supplicant.receive(&message[..length]).unwrap().keys.unwrap();
        assert_eq!(installed.igtk, Some(([0x66u8; 16], 4)));
    }

    #[test]
    fn a_wrong_passphrase_fails_message_3_s_mic() {
        let mut supplicant = Supplicant::new([1u8; 32], STATION, AP, [4u8; 32]);
        let mut message = [0u8; MAX_KEY_FRAME_BYTES];
        let length = write_key_frame(&mut message, 2 | KEY_INFO_PAIRWISE | KEY_INFO_ACK, 1, &[3u8; 32], &[], None);
        supplicant.receive(&message[..length]).unwrap();
        let other = ptk(&[2u8; 32], AP, STATION, &[3u8; 32], &[4u8; 32]);
        let info = 2 | KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_INFO_INSTALL | KEY_INFO_MIC | KEY_INFO_SECURE | KEY_INFO_ENCRYPTED_DATA;
        let length = write_key_frame(&mut message, info, 2, &[3u8; 32], &[0; 24], Some(&other.kck));
        assert_eq!(supplicant.receive(&message[..length]).unwrap_err(), HandshakeError::BadMic);
    }
}

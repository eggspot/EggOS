//! IEEE 802.11-2020 frames, chapter 9: the MAC header, beacons and probe responses with the
//! elements a network list needs (SSID, channel, RSN).

pub type Address = [u8; 6];

pub const FRAME_TYPE_MANAGEMENT: u8 = 0;
pub const FRAME_TYPE_DATA: u8 = 2;
pub const SUBTYPE_BEACON: u8 = 8;
pub const SUBTYPE_PROBE_RESPONSE: u8 = 5;
const HEADER_BYTES: usize = 24;
const BEACON_FIXED_BYTES: usize = 12;
const ELEMENT_SSID: u8 = 0;
const ELEMENT_DS_PARAMETERS: u8 = 3;
const ELEMENT_RSN: u8 = 48;
const CAPABILITY_PRIVACY: u16 = 1 << 4;
const MAX_SSID_BYTES: usize = 32;

/// The RSN suite selector OUI 00-0F-AC and the suites a WPA2/WPA3-Personal network uses.
pub mod suite {
    pub const CCMP: u32 = 0x000f_ac04;
    pub const GCMP_256: u32 = 0x000f_ac09;
    pub const PSK: u32 = 0x000f_ac02;
    pub const PSK_SHA256: u32 = 0x000f_ac06;
    pub const SAE: u32 = 0x000f_ac08;
}

/// What a frame's control field says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameControl {
    pub kind: u8,
    pub subtype: u8,
    pub to_ds: bool,
    pub from_ds: bool,
    pub protected: bool,
}

pub fn frame_control(frame: &[u8]) -> Option<FrameControl> {
    let first = *frame.first()?;
    let flags = *frame.get(1)?;
    Some(FrameControl {
        kind: (first >> 2) & 0b11,
        subtype: first >> 4,
        to_ds: flags & 1 != 0,
        from_ds: flags & 2 != 0,
        protected: flags & 0x40 != 0,
    })
}

fn address(frame: &[u8], at: usize) -> Option<Address> {
    frame.get(at..at + 6)?.try_into().ok()
}

/// A network a beacon or probe response describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    pub bssid: Address,
    pub ssid: [u8; MAX_SSID_BYTES],
    pub ssid_length: usize,
    pub channel: Option<u8>,
    pub security: Security,
}

impl Network {
    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..self.ssid_length]
    }
}

/// How a network protects its traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WEP or WPA1: refused.
    Legacy,
    /// RSN with CCMP and a pre-shared key: WPA2-Personal.
    Wpa2Personal,
    /// RSN with SAE: WPA3-Personal, which needs SAE (not yet).
    Wpa3Personal,
    /// RSN with something else (802.1X, other ciphers).
    Other,
}

/// The pairwise cipher and key management an RSN element offers that the crate takes.
pub fn rsn_security(element: &[u8]) -> Security {
    let suites = |at: usize| -> Option<(usize, &[u8])> {
        let count = usize::from(u16::from_le_bytes([
            *element.get(at)?,
            *element.get(at + 1)?,
        ]));
        Some((at + 2 + count * 4, element.get(at + 2..at + 2 + count * 4)?))
    };
    let selector = |bytes: &[u8]| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let parse = || -> Option<Security> {
        let after_group = 2 + 4;
        let (after_pairwise, pairwise) = suites(after_group)?;
        let (_, management) = suites(after_pairwise)?;
        let ccmp = pairwise
            .chunks_exact(4)
            .any(|suite| selector(suite) == suite::CCMP);
        let has = |wanted: u32| {
            management
                .chunks_exact(4)
                .any(|suite| selector(suite) == wanted)
        };
        Some(
            match (
                ccmp,
                has(suite::PSK) || has(suite::PSK_SHA256),
                has(suite::SAE),
            ) {
                (true, true, _) => Security::Wpa2Personal,
                (true, false, true) => Security::Wpa3Personal,
                _ => Security::Other,
            },
        )
    };
    parse().unwrap_or(Security::Other)
}

/// The network a beacon or probe response (a whole frame, without its FCS) describes.
pub fn network(frame: &[u8]) -> Option<Network> {
    let control = frame_control(frame)?;
    let advertises = control.kind == FRAME_TYPE_MANAGEMENT
        && matches!(control.subtype, SUBTYPE_BEACON | SUBTYPE_PROBE_RESPONSE);
    if !advertises {
        return None;
    }
    let bssid = address(frame, 16)?;
    let capability = u16::from_le_bytes([
        *frame.get(HEADER_BYTES + 10)?,
        *frame.get(HEADER_BYTES + 11)?,
    ]);
    let mut network = Network {
        bssid,
        ssid: [0; MAX_SSID_BYTES],
        ssid_length: 0,
        channel: None,
        security: if capability & CAPABILITY_PRIVACY != 0 {
            Security::Legacy
        } else {
            Security::Open
        },
    };
    let mut elements = frame.get(HEADER_BYTES + BEACON_FIXED_BYTES..)?;
    while elements.len() >= 2 {
        let (id, length) = (elements[0], usize::from(elements[1]));
        let body = elements.get(2..2 + length)?;
        match id {
            ELEMENT_SSID if length <= MAX_SSID_BYTES => {
                network.ssid[..length].copy_from_slice(body);
                network.ssid_length = length;
            }
            ELEMENT_DS_PARAMETERS if length == 1 => network.channel = Some(body[0]),
            ELEMENT_RSN => network.security = rsn_security(body),
            _ => {}
        }
        elements = &elements[2 + length..];
    }
    Some(network)
}

/// The RSN element a WPA2-Personal station sends in its association request and in message 2:
/// version 1, CCMP group and pairwise, PSK.
pub const WPA2_PERSONAL_RSN: [u8; 22] = [
    ELEMENT_RSN,
    20,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    4,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    4,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    2,
    0,
    0,
];

/// The RSN element a WPA3-Personal station sends (D285): version 1, CCMP group and pairwise,
/// SAE, management frame protection capable and required, BIP-CMAC-128 for group management
/// frames.
pub const WPA3_PERSONAL_RSN: [u8; 28] = [
    ELEMENT_RSN,
    26,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    4,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    4,
    1,
    0,
    0x00,
    0x0f,
    0xac,
    8,
    0xc0,
    0x00,
    0,
    0,
    0x00,
    0x0f,
    0xac,
    6,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn beacon(elements: &[u8], privacy: bool) -> Vec<u8> {
        let mut frame = vec![0x80, 0, 0, 0];
        frame.extend_from_slice(&[0xff; 6]);
        frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&[0; 8]);
        frame.extend_from_slice(&[100, 0]);
        frame.extend_from_slice(&(if privacy { 0x0011u16 } else { 0x0001 }).to_le_bytes());
        frame.extend_from_slice(elements);
        frame
    }

    #[test]
    fn a_wpa2_beacon_names_its_network() {
        let mut elements = vec![0, 4, b'h', b'o', b'm', b'e', 3, 1, 6];
        elements.extend_from_slice(&WPA2_PERSONAL_RSN);
        let found = network(&beacon(&elements, true)).unwrap();
        assert_eq!(found.ssid(), b"home");
        assert_eq!(found.channel, Some(6));
        assert_eq!(found.bssid, [2, 0, 0, 0, 0, 1]);
        assert_eq!(found.security, Security::Wpa2Personal);
    }

    #[test]
    fn open_and_legacy_networks_are_told_apart() {
        assert_eq!(
            network(&beacon(&[0, 1, b'x'], false)).unwrap().security,
            Security::Open
        );
        assert_eq!(
            network(&beacon(&[0, 1, b'x'], true)).unwrap().security,
            Security::Legacy
        );
    }

    #[test]
    fn an_sae_only_network_is_wpa3() {
        let mut rsn = WPA2_PERSONAL_RSN;
        rsn[19] = 8;
        assert_eq!(rsn_security(&rsn[2..]), Security::Wpa3Personal);
    }
}

//! The high-throughput capability elements a station offers in its association request (D784;
//! IEEE 802.11-2020, 9.4.2.55, 9.4.2.157 and 9.4.2.247): HT, VHT and HE as far as both the
//! adapter and the access point have them, with two spatial streams when the adapter has two
//! chains. Short guard intervals, LDPC and STBC reception are offered; beamforming, 160 MHz
//! and the HE features beyond the mandatory ones are not.

use crate::frames::Phy;
use alloc::vec::Vec;

const ELEMENT_HT_CAPABILITIES: u8 = 45;
const ELEMENT_VHT_CAPABILITIES: u8 = 191;
const ELEMENT_EXTENSION: u8 = 255;
const EXTENSION_HE_CAPABILITIES: u8 = 35;
const HT_LDPC: u16 = 1 << 0;
const HT_40_MHZ: u16 = 1 << 1;
const HT_SM_POWER_SAVE_OFF: u16 = 3 << 2;
const HT_SHORT_GI_20: u16 = 1 << 5;
const HT_SHORT_GI_40: u16 = 1 << 6;
const HT_RX_STBC_ONE: u16 = 1 << 8;
const HT_AMPDU_64K: u8 = 3;
const VHT_RX_LDPC: u32 = 1 << 4;
const VHT_SHORT_GI_80: u32 = 1 << 5;
const VHT_RX_STBC_ONE: u32 = 1 << 8;
const VHT_AMPDU_1M: u32 = 7 << 23;
const MCS_0_TO_9: u16 = 2;
const NOT_SUPPORTED: u16 = 3;

/// What the adapter can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ours {
    pub ht: bool,
    pub vht: bool,
    pub he: bool,
    /// 1 or 2.
    pub streams: u8,
    /// Whether the channel allows 40 MHz.
    pub forty: bool,
}

/// The VHT and HE MCS map: MCS 0 to 9 (or 11 for HE) for each stream up to `streams`.
fn mcs_map(streams: u8) -> u16 {
    (0..8u16).fold(0, |map, stream| {
        let value = match stream < u16::from(streams.max(1)) {
            true => MCS_0_TO_9,
            false => NOT_SUPPORTED,
        };
        map | (value << (2 * stream))
    })
}

/// The elements offered to an access point whose beacon said `theirs`.
pub fn offered(ours: &Ours, theirs: &Phy) -> Vec<u8> {
    let mut elements = Vec::new();
    if !(ours.ht && theirs.ht) {
        return elements;
    }
    let mut info = HT_LDPC | HT_SM_POWER_SAVE_OFF | HT_SHORT_GI_20 | HT_RX_STBC_ONE;
    if ours.forty {
        info |= HT_40_MHZ | HT_SHORT_GI_40;
    }
    let mut ht = [0u8; 26];
    ht[..2].copy_from_slice(&info.to_le_bytes());
    ht[2] = HT_AMPDU_64K;
    ht[3] = 0xff;
    if ours.streams >= 2 {
        ht[4] = 0xff;
    }
    ht[15] = 1;
    elements.extend_from_slice(&[ELEMENT_HT_CAPABILITIES, ht.len() as u8]);
    elements.extend_from_slice(&ht);
    if ours.vht && theirs.vht {
        let info = VHT_RX_LDPC | VHT_SHORT_GI_80 | VHT_RX_STBC_ONE | VHT_AMPDU_1M;
        let map = mcs_map(ours.streams);
        let mut vht = [0u8; 12];
        vht[..4].copy_from_slice(&info.to_le_bytes());
        vht[4..6].copy_from_slice(&map.to_le_bytes());
        vht[8..10].copy_from_slice(&map.to_le_bytes());
        elements.extend_from_slice(&[ELEMENT_VHT_CAPABILITIES, vht.len() as u8]);
        elements.extend_from_slice(&vht);
    }
    if ours.he && theirs.he {
        let mut he = [0u8; 1 + 6 + 11 + 4];
        he[0] = EXTENSION_HE_CAPABILITIES;
        he[7] = match ours.forty {
            true => 0b110,
            false => 0,
        };
        let map = mcs_map(ours.streams);
        he[18..20].copy_from_slice(&map.to_le_bytes());
        he[20..22].copy_from_slice(&map.to_le_bytes());
        elements.extend_from_slice(&[ELEMENT_EXTENSION, he.len() as u8]);
        elements.extend_from_slice(&he);
    }
    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_streams_offer_mcs_0_to_15_and_vht_mcs_0_to_9_on_both() {
        let ours = Ours {
            ht: true,
            vht: true,
            he: false,
            streams: 2,
            forty: true,
        };
        let theirs = Phy {
            ht: true,
            vht: true,
            ..Phy::default()
        };
        let elements = offered(&ours, &theirs);
        assert_eq!(&elements[..2], &[ELEMENT_HT_CAPABILITIES, 26]);
        assert_eq!(&elements[5..7], &[0xff, 0xff]);
        assert_eq!(elements[28], ELEMENT_VHT_CAPABILITIES);
        assert_eq!(u16::from_le_bytes([elements[34], elements[35]]), 0xfffa);
        assert!(offered(&ours, &Phy::default()).is_empty());
    }
}

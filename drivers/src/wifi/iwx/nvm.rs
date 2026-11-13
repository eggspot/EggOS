//! What the adapter's NVM says once its firmware is up (D275): the init commands that let the
//! firmware read it, then `NVM_GET_INFO`'s answer (the NVM's version, which bands and standards
//! the adapter may use, its antennas, whether the firmware sets the regulatory domain itself)
//! and the channels it may use, with the MAC address from its registers.

use super::firmware::Firmware;
use super::transport::{
    LEGACY_GROUP, REGULATORY_AND_NVM_GROUP, SYSTEM_GROUP, Transport, TransportError, Wait,
};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use alloc::vec::Vec;

const INIT_EXTENDED_CONFIG: u8 = 0x03;
const INIT_NVM: u32 = 1 << 1;
const NVM_ACCESS_COMPLETE: u8 = 0x00;
const NVM_GET_INFO: u8 = 0x02;
const INIT_COMPLETE: u8 = 0x04;
const API_REGULATORY_NVM_INFO: usize = 48;
const CAPABILITY_LAR_SUPPORT: usize = 1;

const MAC_SKU_11N: u32 = 1 << 2;
const MAC_SKU_11AC: u32 = 1 << 3;
const MAC_SKU_11AX: u32 = 1 << 4;
const MAC_SKU_2_4_GHZ: u32 = 1 << 0;
const MAC_SKU_5_GHZ: u32 = 1 << 1;
const MAC_SKU_MIMO_DISABLED: u32 = 1 << 5;
const CHANNEL_VALID: u32 = 1 << 0;
const CHANNEL_ACTIVE: u32 = 1 << 3;
const CHANNEL_RADAR: u32 = 1 << 4;

const CHANNELS_V3: usize = 51;
const CHANNELS_V4: usize = 110;
const TWO_GHZ_CHANNELS: usize = 14;
const FIVE_GHZ_CHANNELS: usize = 37;

const NVM_CHANNELS: [u8; TWO_GHZ_CHANNELS + FIVE_GHZ_CHANNELS] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80,
    84, 88, 92, 96, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 149, 153, 157, 161,
    165, 169, 173, 177, 181,
];

/// A band of the adapter's radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    TwoPointFour,
    Five,
}

/// One channel the adapter may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    pub number: u8,
    pub band: Band,
    /// Whether the adapter may send probe requests on it (else it only listens).
    pub active: bool,
    /// Whether radar detection rules apply.
    pub radar: bool,
}

impl Channel {
    /// Its centre frequency in MHz.
    pub fn megahertz(&self) -> u32 {
        match (self.band, self.number) {
            (Band::TwoPointFour, 14) => 2484,
            (Band::TwoPointFour, number) => 2407 + 5 * u32::from(number),
            (Band::Five, number) => 5000 + 5 * u32::from(number),
        }
    }
}

/// What the NVM said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nvm {
    pub version: u16,
    pub address: [u8; 6],
    pub high_throughput: bool,
    pub very_high_throughput: bool,
    pub high_efficiency: bool,
    pub mimo: bool,
    pub tx_chains: u8,
    pub rx_chains: u8,
    /// Whether the firmware chooses the regulatory domain itself (location-aware regulatory).
    pub location_aware: bool,
    pub channels: Vec<Channel>,
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn half(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// Reads `NVM_GET_INFO`'s answer `bytes` (version 4 when `wide_profiles`, else 3), with the
/// adapter's `address`, for a firmware that does (or not) `location_aware` regulatory.
pub fn parse(
    bytes: &[u8],
    wide_profiles: bool,
    address: [u8; 6],
    location_aware: bool,
) -> Option<Nvm> {
    let expected = match wide_profiles {
        true => 28 + 4 * CHANNELS_V4,
        false => 24 + 2 * CHANNELS_V3 + 2,
    };
    if bytes.len() != expected {
        return None;
    }
    let mac_sku = word(bytes, 8)?;
    let profile = |index: usize| match wide_profiles {
        true => word(bytes, 28 + 4 * index),
        false => half(bytes, 24 + 2 * index).map(u32::from),
    };
    let five_allowed = mac_sku & MAC_SKU_5_GHZ != 0;
    let two_allowed = mac_sku & MAC_SKU_2_4_GHZ != 0;
    let channels = NVM_CHANNELS
        .iter()
        .enumerate()
        .filter_map(|(index, &number)| {
            let flags = profile(index)?;
            let band = match index < TWO_GHZ_CHANNELS {
                true => Band::TwoPointFour,
                false => Band::Five,
            };
            let allowed = match band {
                Band::TwoPointFour => two_allowed,
                Band::Five => five_allowed,
            };
            (allowed && flags & CHANNEL_VALID != 0).then_some(Channel {
                number,
                band,
                active: flags & CHANNEL_ACTIVE != 0,
                radar: flags & CHANNEL_RADAR != 0,
            })
        })
        .collect();
    Some(Nvm {
        version: half(bytes, 4)?,
        address,
        high_throughput: mac_sku & MAC_SKU_11N != 0,
        very_high_throughput: mac_sku & MAC_SKU_11AC != 0,
        high_efficiency: mac_sku & MAC_SKU_11AX != 0,
        mimo: mac_sku & MAC_SKU_MIMO_DISABLED == 0,
        tx_chains: word(bytes, 12)? as u8,
        rx_chains: word(bytes, 16)? as u8,
        location_aware: location_aware && word(bytes, 20)? != 0,
        channels,
    })
}

/// After alive: lets the AX210 family go on without a platform NVM, tells the firmware the
/// host will read the NVM, waits for its init to complete and reads the NVM.
pub fn initialize<R: RegisterIo, M: DmaMemory>(
    transport: &mut Transport<R, M>,
    firmware: &Firmware,
    wait: &mut dyn Wait,
) -> Result<Nvm, TransportError> {
    transport.finish_platform_nvm(wait)?;
    transport.send_command(
        SYSTEM_GROUP,
        INIT_EXTENDED_CONFIG,
        &INIT_NVM.to_le_bytes(),
        wait,
    )?;
    transport.send_command(
        REGULATORY_AND_NVM_GROUP,
        NVM_ACCESS_COMPLETE,
        &0u32.to_le_bytes(),
        wait,
    )?;
    transport.wait_notification(LEGACY_GROUP, INIT_COMPLETE, wait)?;
    let answer = transport.send_command(
        REGULATORY_AND_NVM_GROUP,
        NVM_GET_INFO,
        &0u32.to_le_bytes(),
        wait,
    )?;
    let address = transport.mac_address(wait)?;
    parse(
        &answer,
        firmware.has_api(API_REGULATORY_NVM_INFO),
        address,
        firmware.has_capability(CAPABILITY_LAR_SUPPORT),
    )
    .ok_or(TransportError::BadNvm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_channels_the_nvm_allows_are_kept_with_their_frequencies() {
        let mut bytes = vec![0u8; 28 + 4 * CHANNELS_V4];
        bytes[4..6].copy_from_slice(&0x0c0au16.to_le_bytes());
        bytes[8..12]
            .copy_from_slice(&(MAC_SKU_2_4_GHZ | MAC_SKU_5_GHZ | MAC_SKU_11AX).to_le_bytes());
        bytes[12..16].copy_from_slice(&3u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        for (index, flags) in [
            (0, CHANNEL_VALID | CHANNEL_ACTIVE),
            (13, CHANNEL_VALID),
            (14, CHANNEL_VALID | CHANNEL_RADAR),
        ] {
            bytes[28 + 4 * index..32 + 4 * index].copy_from_slice(&flags.to_le_bytes());
        }
        let address = [0, 0x11, 0x22, 0x33, 0x44, 0x55];
        let nvm = parse(&bytes, true, address, true).unwrap();
        assert_eq!(nvm.version, 0x0c0a);
        assert!(nvm.high_efficiency && !nvm.very_high_throughput && nvm.mimo && nvm.location_aware);
        assert_eq!(nvm.tx_chains, 3);
        assert_eq!(nvm.channels.len(), 3);
        assert_eq!(
            (
                nvm.channels[0].number,
                nvm.channels[0].megahertz(),
                nvm.channels[0].active
            ),
            (1, 2412, true)
        );
        assert_eq!(nvm.channels[1].megahertz(), 2484);
        assert_eq!(
            (
                nvm.channels[2].number,
                nvm.channels[2].megahertz(),
                nvm.channels[2].radar
            ),
            (36, 5180, true)
        );
        assert_eq!(parse(&bytes[1..], true, address, true), None);
        assert!(!parse(&bytes, true, address, false).unwrap().location_aware);
    }
}

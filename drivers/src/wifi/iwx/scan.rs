//! Configuring the firmware for operation and scanning (D276): the commands the reference sends
//! after the NVM (transmit antennas, Bluetooth coexistence off, the SoC's crystal latency, the
//! dedicated command queue, power saving off, the regulatory domain left to the firmware, the
//! reduced scan configuration, beacon filtering off), then a passive scan of every channel the
//! NVM allows (`SCAN_REQ_UMAC` version 14), collecting the beacons and probe responses the
//! firmware passes up until it says the scan is complete.

use super::devices::{Adapter, Family};
use super::firmware::Firmware;
use super::nvm::{Band, Nvm};
use super::transport::{LEGACY_GROUP, LONG_GROUP, SYSTEM_GROUP, Transport, TransportError, Wait};
use crate::dma::DmaMemory;
use crate::io::RegisterIo;
use alloc::vec;
use alloc::vec::Vec;

const TX_ANTENNA_CONFIG: u8 = 0x98;
const BT_CONFIG: u8 = 0x9b;
const BT_COEX_WIFI: u32 = 0x3;
const SOC_CONFIGURATION: u8 = 0x01;
const SOC_DISCRETE: u32 = 1 << 0;
const SOC_LOW_LATENCY: u32 = 1 << 1;
const DATA_PATH_GROUP: u8 = 0x5;
const DQA_ENABLE: u8 = 0x00;
const POWER_TABLE: u8 = 0x77;
const MCC_UPDATE: u8 = 0xc8;
const MCC_SOURCE_OLD_FIRMWARE: u8 = 0;
const MCC_SOURCE_GET_CURRENT: u8 = 0x10;
const MCC_WORLD: u16 = (b'Z' as u16) << 8 | b'Z' as u16;
const SCAN_CONFIG: u8 = 0x0c;
const SCAN_REQUEST: u8 = 0x0d;
const SCAN_COMPLETE: u8 = 0x0f;
const BEACON_FILTER: u8 = 0xd2;
const BEACON_FILTER_BYTES: usize = 60;
/// The code of the packets that carry received frames.
pub const RX_MPDU: u8 = 0xc1;

const CAPABILITY_DQA: usize = 12;
const CAPABILITY_LAR_MULTI_MCC: usize = 29;
const API_WIFI_MCC_UPDATE: usize = 9;
const API_REDUCED_SCAN_CONFIG: usize = 56;
const API_SCAN_EXTENDED_CHANNEL: usize = 58;

const SCAN_FLAG_PASS_ALL: u16 = 1 << 1;
const SCAN_FLAG_NOTIFY_ITERATION: u16 = 1 << 2;
const SCAN_FLAG_ADAPTIVE_DWELL: u16 = 1 << 7;
const SCAN_FLAG_FORCE_PASSIVE: u16 = 1 << 11;
const SCAN_PRIORITY_EXT_6: u32 = 6;
const DWELL_ACTIVE: u8 = 10;
const DWELL_PASSIVE: u8 = 110;
const ADAPTIVE_SOCIAL: u8 = 10;
const ADAPTIVE_2_4: u8 = 2;
const ADAPTIVE_5: u8 = 8;
const ADAPTIVE_FULL_BUDGET: u16 = 300;
const CHANNEL_ORDER: u8 = 1 << 5;
const APS_GO_FRIENDLY: u8 = 10;
const APS_SOCIAL: u8 = 2;
const PHY_BAND_5: u8 = 0;
const PHY_BAND_2_4: u8 = 1;
const MAX_SCAN_CHANNEL_ENTRIES: usize = 67;

const SCAN_REQUEST_BYTES: usize = 1976;
const PARAMETERS: usize = 8;
const GENERAL: usize = PARAMETERS;
const CHANNELS: usize = GENERAL + 36;
const PERIODIC: usize = CHANNELS + 4 + MAX_SCAN_CHANNEL_ENTRIES * 8;
const PROBE: usize = PERIODIC + 12;
const PROBE_BUFFER: usize = PROBE + 20;

const RATES_2_4: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
const EXTENDED_RATES_2_4: [u8; 4] = [0x30, 0x48, 0x60, 0x6c];
const RATES_5: [u8; 8] = [0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c];
const SUPPORTED_RATES: u8 = 1;
const EXTENDED_RATES: u8 = 50;

const MPDU_STATUS_CRC_OK: u32 = 1 << 0;
const MPDU_STATUS_OVERRUN_OK: u32 = 1 << 1;
const MPDU_PADDED: u8 = 0x20;
const DESCRIPTOR_V1_BYTES: usize = 48;
const DESCRIPTOR_V3_BYTES: usize = 64;
const MANAGEMENT_HEADER_BYTES: usize = 24;

/// A frame the firmware passed up during a scan: the 802.11 frame, its channel and its
/// signal in dBm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub frame: Vec<u8>,
    pub channel: u8,
    pub signal: i8,
}

fn valid_tx_antennas(firmware: &Firmware, nvm: &Nvm) -> u32 {
    let firmware_chains = (firmware.phy_sku >> 16) & 0xf;
    match nvm.tx_chains {
        0 => firmware_chains,
        chains => firmware_chains & u32::from(chains),
    }
}

fn valid_rx_antennas(firmware: &Firmware, nvm: &Nvm) -> u32 {
    let firmware_chains = (firmware.phy_sku >> 20) & 0xf;
    match nvm.rx_chains {
        0 => firmware_chains,
        chains => firmware_chains & u32::from(chains),
    }
}

fn pair(first: u32, second: u32) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&first.to_le_bytes());
    bytes[4..].copy_from_slice(&second.to_le_bytes());
    bytes
}

/// Sends the commands that ready the firmware for scanning and association.
pub fn configure<R: RegisterIo, M: DmaMemory>(
    transport: &mut Transport<R, M>,
    firmware: &Firmware,
    adapter: &Adapter,
    nvm: &Nvm,
    wait: &mut dyn Wait,
) -> Result<(), TransportError> {
    let tx = valid_tx_antennas(firmware, nvm);
    transport.send_command(LEGACY_GROUP, TX_ANTENNA_CONFIG, &tx.to_le_bytes(), wait)?;
    transport.send_command(LEGACY_GROUP, BT_CONFIG, &pair(BT_COEX_WIFI, 0), wait)?;
    let flags = match adapter.integrated {
        false => SOC_DISCRETE,
        true => match (adapter.low_latency_xtal, firmware.command_version(LONG_GROUP, SCAN_REQUEST)) {
            (true, Some(version)) if version >= 2 => SOC_LOW_LATENCY,
            _ => 0,
        },
    };
    transport.send_command(SYSTEM_GROUP, SOC_CONFIGURATION, &pair(flags, adapter.xtal_latency), wait)?;
    if firmware.has_capability(CAPABILITY_DQA) {
        transport.send_command(DATA_PATH_GROUP, DQA_ENABLE, &0u32.to_le_bytes(), wait)?;
    }
    transport.send_command(LEGACY_GROUP, POWER_TABLE, &[0; 4], wait)?;
    if nvm.location_aware {
        let mut mcc = [0u8; 28];
        mcc[..2].copy_from_slice(&MCC_WORLD.to_le_bytes());
        mcc[2] = match firmware.has_api(API_WIFI_MCC_UPDATE) || firmware.has_capability(CAPABILITY_LAR_MULTI_MCC) {
            true => MCC_SOURCE_GET_CURRENT,
            false => MCC_SOURCE_OLD_FIRMWARE,
        };
        transport.send_command(LEGACY_GROUP, MCC_UPDATE, &mcc, wait)?;
    }
    if !firmware.has_api(API_REDUCED_SCAN_CONFIG) {
        return Err(TransportError::BadFirmware);
    }
    let mut scan = [0u8; 12];
    if firmware.command_version(LONG_GROUP, SCAN_CONFIG).is_none_or(|version| version < 5) {
        scan[2] = 0xff;
    }
    scan[4..8].copy_from_slice(&tx.to_le_bytes());
    scan[8..12].copy_from_slice(&valid_rx_antennas(firmware, nvm).to_le_bytes());
    transport.send_command(LONG_GROUP, SCAN_CONFIG, &scan, wait)?;
    transport.send_command(LEGACY_GROUP, BEACON_FILTER, &[0; BEACON_FILTER_BYTES], wait)?;
    Ok(())
}

fn put16(request: &mut [u8], at: usize, value: u16) {
    request[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(request: &mut [u8], at: usize, value: u32) {
    request[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn probe_request(request: &mut [u8], address: [u8; 6], five: bool) {
    let mut frame = vec![0x40, 0x00, 0, 0];
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&address);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[0, 0]);
    let header = frame.len();
    let band_2_4 = frame.len();
    frame.extend_from_slice(&[SUPPORTED_RATES, RATES_2_4.len() as u8]);
    frame.extend_from_slice(&RATES_2_4);
    frame.extend_from_slice(&[EXTENDED_RATES, EXTENDED_RATES_2_4.len() as u8]);
    frame.extend_from_slice(&EXTENDED_RATES_2_4);
    let band_2_4_length = frame.len() - band_2_4;
    let band_5 = frame.len();
    if five {
        frame.extend_from_slice(&[SUPPORTED_RATES, RATES_5.len() as u8]);
        frame.extend_from_slice(&RATES_5);
    }
    let band_5_length = frame.len() - band_5;
    put16(request, PROBE, 0);
    put16(request, PROBE + 2, header as u16);
    put16(request, PROBE + 4, band_2_4 as u16);
    put16(request, PROBE + 6, band_2_4_length as u16);
    if five {
        put16(request, PROBE + 8, band_5 as u16);
        put16(request, PROBE + 10, band_5_length as u16);
    }
    put16(request, PROBE + 16, frame.len() as u16);
    request[PROBE_BUFFER..PROBE_BUFFER + frame.len()].copy_from_slice(&frame);
}

/// The `SCAN_REQ_UMAC` version 14 request for a passive scan of `nvm`'s channels, at most
/// `channel_limit` of them.
pub fn scan_request(nvm: &Nvm, channel_limit: usize, extended_channels: bool) -> Vec<u8> {
    let mut request = vec![0u8; SCAN_REQUEST_BYTES];
    put32(&mut request, 4, SCAN_PRIORITY_EXT_6);
    let flags = SCAN_FLAG_FORCE_PASSIVE | SCAN_FLAG_PASS_ALL | SCAN_FLAG_NOTIFY_ITERATION | SCAN_FLAG_ADAPTIVE_DWELL;
    put16(&mut request, GENERAL, flags);
    request[GENERAL + 4] = DWELL_ACTIVE;
    request[GENERAL + 5] = DWELL_ACTIVE;
    request[GENERAL + 6] = ADAPTIVE_2_4;
    request[GENERAL + 7] = ADAPTIVE_5;
    request[GENERAL + 8] = ADAPTIVE_SOCIAL;
    put16(&mut request, GENERAL + 10, ADAPTIVE_FULL_BUDGET);
    put32(&mut request, GENERAL + 28, SCAN_PRIORITY_EXT_6);
    request[GENERAL + 32] = DWELL_PASSIVE;
    request[GENERAL + 33] = DWELL_PASSIVE;
    request[CHANNELS] = CHANNEL_ORDER;
    let channels: Vec<_> = nvm.channels.iter().take(channel_limit.min(MAX_SCAN_CHANNEL_ENTRIES)).collect();
    request[CHANNELS + 1] = channels.len() as u8;
    request[CHANNELS + 2] = APS_GO_FRIENDLY;
    request[CHANNELS + 3] = APS_SOCIAL;
    for (index, channel) in channels.iter().enumerate() {
        let entry = CHANNELS + 4 + index * 8;
        request[entry + 4] = channel.number;
        match extended_channels {
            true => {
                request[entry + 5] = match channel.band {
                    Band::TwoPointFour => PHY_BAND_2_4,
                    Band::Five => PHY_BAND_5,
                };
                request[entry + 6] = 1;
            }
            false => request[entry + 5] = 1,
        }
    }
    request[PERIODIC + 2] = 1;
    let five = nvm.channels.iter().any(|channel| channel.band == Band::Five);
    probe_request(&mut request, nvm.address, five);
    request
}

/// The 802.11 frame in a `REPLY_RX_MPDU` packet's `payload`, with its channel and signal; `None`
/// when the firmware marked it damaged.
pub fn received_frame(payload: &[u8], ax210: bool) -> Option<Heard> {
    let descriptor = match ax210 {
        true => DESCRIPTOR_V3_BYTES,
        false => DESCRIPTOR_V1_BYTES,
    };
    let length = usize::from(u16::from_le_bytes(payload.get(..2)?.try_into().ok()?));
    let flags = *payload.get(3)?;
    let status = u32::from_le_bytes(payload.get(12..16)?.try_into().ok()?);
    if status & MPDU_STATUS_CRC_OK == 0 || status & MPDU_STATUS_OVERRUN_OK == 0 || length < MANAGEMENT_HEADER_BYTES {
        return None;
    }
    let (energy, channel) = match ax210 {
        true => (40, 42),
        false => (32, 34),
    };
    let energies = [*payload.get(energy)?, *payload.get(energy + 1)?];
    let strongest = energies.iter().filter(|&&value| value != 0).min().copied().unwrap_or(127);
    let mut frame = payload.get(descriptor..descriptor + length)?.to_vec();
    if flags & MPDU_PADDED != 0 {
        frame.drain(MANAGEMENT_HEADER_BYTES..MANAGEMENT_HEADER_BYTES + 2);
    }
    Some(Heard {
        frame,
        channel: *payload.get(channel)?,
        signal: -(strongest.min(127) as i8),
    })
}

/// Scans every channel the NVM allows and returns what was heard, until the firmware says the
/// scan is complete or the wait runs out.
pub fn scan<R: RegisterIo, M: DmaMemory>(
    transport: &mut Transport<R, M>,
    firmware: &Firmware,
    adapter: &Adapter,
    nvm: &Nvm,
    wait: &mut dyn Wait,
) -> Result<Vec<Heard>, TransportError> {
    let request = scan_request(nvm, firmware.scan_channels as usize, firmware.has_api(API_SCAN_EXTENDED_CHANNEL));
    transport.send_command(LONG_GROUP, SCAN_REQUEST, &request, wait)?;
    let ax210 = adapter.family == Family::Ax210;
    let mut frames = Vec::new();
    loop {
        let mut complete = false;
        for packet in transport.take_notifications() {
            match packet.code {
                RX_MPDU => frames.extend(received_frame(&packet.payload, ax210)),
                SCAN_COMPLETE => complete = true,
                _ => {}
            }
        }
        if complete {
            return Ok(frames);
        }
        if !wait.keep_waiting() {
            return match frames.is_empty() {
                true => Err(TransportError::TimedOut),
                false => Ok(frames),
            };
        }
        wait.pause(1000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::iwx::nvm::Channel;

    fn nvm() -> Nvm {
        Nvm {
            version: 1,
            address: [0, 0x11, 0x22, 0x33, 0x44, 0x55],
            high_throughput: true,
            very_high_throughput: false,
            high_efficiency: false,
            mimo: true,
            tx_chains: 3,
            rx_chains: 3,
            location_aware: false,
            channels: vec![
                Channel { number: 1, band: Band::TwoPointFour, active: true, radar: false },
                Channel { number: 36, band: Band::Five, active: true, radar: false },
            ],
        }
    }

    #[test]
    fn the_scan_request_names_each_channel_and_carries_a_probe_request() {
        let request = scan_request(&nvm(), 67, true);
        assert_eq!(request.len(), SCAN_REQUEST_BYTES);
        assert_eq!(request[CHANNELS + 1], 2);
        assert_eq!(&request[CHANNELS + 4 + 4..CHANNELS + 4 + 7], &[1, PHY_BAND_2_4, 1]);
        assert_eq!(&request[CHANNELS + 12 + 4..CHANNELS + 12 + 6], &[36, PHY_BAND_5]);
        assert_eq!(request[PROBE_BUFFER], 0x40);
        assert_eq!(&request[PROBE_BUFFER + 10..PROBE_BUFFER + 16], &nvm().address);
        let flags = u16::from_le_bytes([request[GENERAL], request[GENERAL + 1]]);
        assert_ne!(flags & SCAN_FLAG_FORCE_PASSIVE, 0);
        assert_eq!(scan_request(&nvm(), 1, false)[CHANNELS + 1], 1);
    }

    #[test]
    fn a_received_beacon_is_unwrapped_with_its_channel_and_signal() {
        let mut payload = vec![0u8; DESCRIPTOR_V1_BYTES];
        let frame: Vec<u8> = (0..30u8).collect();
        payload[..2].copy_from_slice(&(frame.len() as u16).to_le_bytes());
        payload[12..16].copy_from_slice(&(MPDU_STATUS_CRC_OK | MPDU_STATUS_OVERRUN_OK).to_le_bytes());
        payload[32] = 55;
        payload[33] = 0;
        payload[34] = 11;
        payload.extend_from_slice(&frame);
        let heard = received_frame(&payload, false).unwrap();
        assert_eq!((heard.channel, heard.signal), (11, -55));
        assert_eq!(heard.frame, frame);
        payload[12] = 0;
        assert_eq!(received_frame(&payload, false), None);
    }
}

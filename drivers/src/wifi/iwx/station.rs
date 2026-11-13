//! Joining a network (D277): the firmware's PHY context on the network's channel, the MAC
//! context for a client station with the network's BSSID, the binding between them, the station
//! entry for the access point, the management transmit queue and session protection, then frames
//! sent through that queue with the firmware choosing their rate. Association state changes
//! (the association id, the beacon interval) go to the MAC context afterwards.

use super::devices::{Adapter, Family};
use super::firmware::Firmware;
use super::nvm::{Band, Channel};
use super::transport::{LEGACY_GROUP, LONG_GROUP, Transport, TransportError, Wait};
use crate::dma::{DmaAllocator, DmaMemory};
use crate::io::RegisterIo;

const PHY_CONTEXT: u8 = 0x08;
const MAC_CONTEXT: u8 = 0x28;
const BINDING_CONTEXT: u8 = 0x2b;
const ADD_STATION: u8 = 0x18;
const REMOVE_STATION: u8 = 0x19;
const TX_QUEUE_CONFIG: u8 = 0x1d;
const TX_COMMAND: u8 = 0x1c;
const DATA_PATH_GROUP: u8 = 0x5;
const SCD_QUEUE_CONFIG: u8 = 0x17;
const RLC_CONFIG: u8 = 0x08;
const MAC_CONF_GROUP: u8 = 0x3;
const SESSION_PROTECTION: u8 = 0x05;

const ACTION_ADD: u32 = 1;
const ACTION_MODIFY: u32 = 2;
const ACTION_REMOVE: u32 = 3;
const SCD_QUEUE_REMOVE: u32 = 1;
const CONTEXT_INVALID: u32 = 0xffff_ffff;
const MAC_TYPE_BSS_STATION: u32 = 5;
const FILTER_ACCEPT_GROUP: u32 = 1 << 2;
const FILTER_IN_BEACON: u32 = 1 << 6;
const LMAC_2_4: u32 = 0;
const LMAC_5: u32 = 1;
const PHY_BAND_5: u8 = 0;
const PHY_BAND_2_4: u8 = 1;
const RX_CHAIN_VALID_SHIFT: u32 = 1;
const RX_CHAIN_COUNT_SHIFT: u32 = 10;
const RX_CHAIN_MIMO_COUNT_SHIFT: u32 = 12;
const CAPABILITY_CDB: usize = 40;
const CAPABILITY_ULTRA_HIGH_BAND: usize = 48;
const STATION_ID: u8 = 0;
const STATION_LINK: u8 = 0;
const STATION_FAT_MASK: u32 = 3 << 26;
const STATION_MIMO_MASK: u32 = 3 << 28;
const ADD_STATION_SUCCESS: u32 = 0x1;
const MANAGEMENT_TID: u8 = 15;
const QUEUE_ENABLE: u16 = 1 << 0;
const SESSION_CONFIG_ASSOC: u32 = 0;
const DEFAULT_SESSION_TU: u32 = 900;

/// The management queue's number.
pub const MANAGEMENT_QUEUE: u8 = 1;
/// The slots of a transmit queue.
pub const QUEUE_SLOTS: usize = 256;
const QUEUE_SIZE_CODE: u32 = 5;
const QUEUE_TFDS: usize = 0x0;
const TFD_BYTES: usize = 256;
const QUEUE_BYTE_COUNTS: usize = 0x1_0000;
const QUEUE_COMMANDS: usize = 0x1_1000;
const COMMAND_SLOT_BYTES: usize = 128;
const QUEUE_PAYLOADS: usize = 0x2_0000;
const PAYLOAD_SLOT_BYTES: usize = 2048;
/// The bytes of a transmit queue's DMA block.
pub const QUEUE_BYTES: usize = QUEUE_PAYLOADS + QUEUE_SLOTS * PAYLOAD_SLOT_BYTES;
const FIRST_TB_BYTES: usize = 20;
const TB_BYTES: usize = 10;
const COMMAND_HEADER_BYTES: usize = 4;
const TX_COMMAND_GEN2_BYTES: usize = 20;
const TX_COMMAND_GEN3_BYTES: usize = 28;
const TX_ENCRYPTION_OFF: u32 = 1 << 1;
const OFFLOAD_PAD: u32 = 1 << 13;
const OFFLOAD_HEADER_SHIFT: u32 = 8;
const OFFLOAD_HEADER_MASK: u32 = 0x1f;
const HBUS_WRITE_POINTER_QUEUE_SHIFT: u32 = 16;

/// Why joining or sending failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    Transport(TransportError),
    /// The firmware refused the station or binding, with this status.
    Refused(u32),
    /// No DMA memory for the queue.
    NoMemory,
    /// A frame larger than a queue slot holds, or shorter than its header.
    BadFrame,
    /// The firmware's PHY context command is a version this driver does not send.
    UnsupportedFirmware,
}

impl From<TransportError> for JoinError {
    fn from(error: TransportError) -> Self {
        JoinError::Transport(error)
    }
}

impl JoinError {
    /// The reason, for the console.
    pub fn description(self) -> &'static str {
        match self {
            JoinError::Transport(error) => error.description(),
            JoinError::Refused(_) => "the Wi-Fi firmware refused the network's station",
            JoinError::NoMemory => "no memory for the Wi-Fi transmit queue",
            JoinError::BadFrame => "a Wi-Fi frame was too large",
            JoinError::UnsupportedFirmware => {
                "the Wi-Fi firmware's context commands are not ones the driver sends"
            }
        }
    }
}

/// The network to join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub bssid: [u8; 6],
    pub channel: Channel,
    /// Its beacon interval, in time units.
    pub beacon_interval: u16,
}

/// A joined network's transmit queue and what the MAC context needs again.
#[derive(Debug)]
pub struct Link<M: DmaMemory> {
    queue: M,
    next: usize,
    hardware: usize,
    target: Target,
    address: [u8; 6],
    ax210: bool,
}

fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    put(bytes, at, &value.to_le_bytes());
}

fn status_of(answer: &[u8]) -> u32 {
    answer.get(..4).map_or(0, |bytes| {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    })
}

fn lmac(channel: &Channel, firmware: &Firmware) -> u32 {
    match channel.band == Band::Five && firmware.has_capability(CAPABILITY_CDB) {
        true => LMAC_5,
        false => LMAC_2_4,
    }
}

fn rx_chain_info(firmware: &Firmware) -> u32 {
    let valid = (firmware.phy_sku >> 20) & 0xf;
    (valid << RX_CHAIN_VALID_SHIFT) | (1 << RX_CHAIN_COUNT_SHIFT) | (1 << RX_CHAIN_MIMO_COUNT_SHIFT)
}

/// The PHY context command for `channel`, 20 MHz wide.
pub fn phy_context(channel: &Channel, firmware: &Firmware, action: u32) -> alloc::vec::Vec<u8> {
    let band = match channel.band {
        Band::TwoPointFour => PHY_BAND_2_4,
        Band::Five => PHY_BAND_5,
    };
    let rlc_separate = firmware.command_version(DATA_PATH_GROUP, RLC_CONFIG) == Some(2);
    let wide = firmware.has_capability(CAPABILITY_ULTRA_HIGH_BAND);
    let mut command = alloc::vec![0u8; if wide { 32 } else { 28 }];
    put32(&mut command, 4, action);
    let after_channel = match wide {
        true => {
            put32(&mut command, 8, u32::from(channel.number));
            command[12] = band;
            16
        }
        false => {
            command[8] = band;
            command[9] = channel.number;
            12
        }
    };
    put32(&mut command, after_channel, lmac(channel, firmware));
    if !rlc_separate {
        put32(&mut command, after_channel + 4, rx_chain_info(firmware));
    }
    command
}

fn mac_context(
    link_address: [u8; 6],
    target: &Target,
    action: u32,
    association: Option<u16>,
) -> alloc::vec::Vec<u8> {
    let mut command = alloc::vec![0u8; 148];
    put32(&mut command, 4, action);
    put32(&mut command, 8, MAC_TYPE_BSS_STATION);
    put(&mut command, 16, &link_address);
    put(&mut command, 24, &target.bssid);
    let mut filters = FILTER_ACCEPT_GROUP;
    if association.is_none() {
        filters |= FILTER_IN_BEACON;
    }
    put32(&mut command, 52, filters);
    let station = 100;
    put32(&mut command, station, u32::from(association.is_some()));
    put32(
        &mut command,
        station + 16,
        u32::from(target.beacon_interval),
    );
    put32(
        &mut command,
        station + 24,
        u32::from(target.beacon_interval),
    );
    put32(&mut command, station + 32, 10);
    put32(
        &mut command,
        station + 36,
        u32::from(association.unwrap_or(0) & 0x3fff),
    );
    command
}

fn binding(channel: &Channel, firmware: &Firmware, action: u32) -> [u8; 28] {
    let mut command = [0u8; 28];
    put32(&mut command, 4, action);
    put32(&mut command, 12, CONTEXT_INVALID);
    put32(&mut command, 16, CONTEXT_INVALID);
    put32(&mut command, 24, lmac(channel, firmware));
    command
}

fn add_station(target: &Target, update: bool) -> [u8; 48] {
    let mut command = [0u8; 48];
    command[0] = u8::from(update);
    if !update {
        put(&mut command, 8, &target.bssid);
    }
    command[16] = STATION_ID;
    put32(&mut command, 24, STATION_FAT_MASK | STATION_MIMO_MASK);
    command[35] = STATION_LINK;
    command
}

/// Readies the firmware to talk to `target`: its PHY, MAC and binding contexts, its station,
/// the management queue (in a DMA block `allocator` gives) and session protection while
/// joining.
pub fn join<R: RegisterIo, M: DmaMemory, A: DmaAllocator<Memory = M>>(
    transport: &mut Transport<R, M>,
    firmware: &Firmware,
    adapter: &Adapter,
    address: [u8; 6],
    target: Target,
    allocator: &mut A,
    wait: &mut dyn Wait,
) -> Result<Link<M>, JoinError> {
    if !matches!(
        firmware.command_version(LONG_GROUP, PHY_CONTEXT),
        None | Some(3) | Some(4)
    ) {
        return Err(JoinError::UnsupportedFirmware);
    }
    transport.send_command(
        LEGACY_GROUP,
        PHY_CONTEXT,
        &phy_context(&target.channel, firmware, ACTION_ADD),
        wait,
    )?;
    if firmware.command_version(DATA_PATH_GROUP, RLC_CONFIG) == Some(2) {
        let mut rlc = [0u8; 32];
        put32(&mut rlc, 4, rx_chain_info(firmware));
        transport.send_command(DATA_PATH_GROUP, RLC_CONFIG, &rlc, wait)?;
    }
    transport.send_command(
        LEGACY_GROUP,
        MAC_CONTEXT,
        &mac_context(address, &target, ACTION_ADD, None),
        wait,
    )?;
    let bound = binding(&target.channel, firmware, ACTION_ADD);
    let status = status_of(&transport.send_command(LEGACY_GROUP, BINDING_CONTEXT, &bound, wait)?);
    if status != 0 {
        return Err(JoinError::Refused(status));
    }
    let status = status_of(&transport.send_command(
        LEGACY_GROUP,
        ADD_STATION,
        &add_station(&target, false),
        wait,
    )?);
    if status & 0xff != ADD_STATION_SUCCESS {
        return Err(JoinError::Refused(status));
    }
    let queue = allocator.allocate(QUEUE_BYTES).ok_or(JoinError::NoMemory)?;
    let base = queue.dma_addr().get();
    let mut config = [0u8; 40];
    let answer = match firmware.command_version(DATA_PATH_GROUP, SCD_QUEUE_CONFIG) {
        Some(3) => {
            put32(&mut config, 4, 1 << STATION_ID);
            config[8] = MANAGEMENT_TID;
            put32(&mut config, 16, QUEUE_SIZE_CODE);
            put(
                &mut config,
                20,
                &(base + QUEUE_BYTE_COUNTS as u64).to_le_bytes(),
            );
            put(&mut config, 28, &(base + QUEUE_TFDS as u64).to_le_bytes());
            transport.send_command(DATA_PATH_GROUP, SCD_QUEUE_CONFIG, &config[..36], wait)?
        }
        _ => {
            config[0] = STATION_ID;
            config[1] = MANAGEMENT_TID;
            put(&mut config, 2, &QUEUE_ENABLE.to_le_bytes());
            put32(&mut config, 4, QUEUE_SIZE_CODE);
            put(
                &mut config,
                8,
                &(base + QUEUE_BYTE_COUNTS as u64).to_le_bytes(),
            );
            put(&mut config, 16, &(base + QUEUE_TFDS as u64).to_le_bytes());
            transport.send_command(LEGACY_GROUP, TX_QUEUE_CONFIG, &config[..24], wait)?
        }
    };
    if answer
        .get(..2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        != Some(u16::from(MANAGEMENT_QUEUE))
    {
        return Err(JoinError::Refused(status_of(&answer)));
    }
    let duration = match target.beacon_interval {
        0 => DEFAULT_SESSION_TU,
        interval => u32::from(interval) * 9,
    };
    let mut session = [0u8; 24];
    put32(&mut session, 4, ACTION_ADD);
    put32(&mut session, 8, SESSION_CONFIG_ASSOC);
    put32(&mut session, 12, duration);
    transport.send_command(MAC_CONF_GROUP, SESSION_PROTECTION, &session, wait)?;
    Ok(Link {
        queue,
        next: 0,
        hardware: 0,
        target,
        address,
        ax210: adapter.family == Family::Ax210,
    })
}

/// Tells the firmware the station is associated with association id `association`.
pub fn associated<R: RegisterIo, M: DmaMemory>(
    transport: &mut Transport<R, M>,
    link: &Link<M>,
    association: u16,
    wait: &mut dyn Wait,
) -> Result<(), JoinError> {
    let command = mac_context(link.address, &link.target, ACTION_MODIFY, Some(association));
    transport.send_command(LEGACY_GROUP, MAC_CONTEXT, &command, wait)?;
    let status = status_of(&transport.send_command(
        LEGACY_GROUP,
        ADD_STATION,
        &add_station(&link.target, true),
        wait,
    )?);
    match status & 0xff {
        ADD_STATION_SUCCESS => Ok(()),
        _ => Err(JoinError::Refused(status)),
    }
}

fn queue_off(firmware: &Firmware) -> (u8, u8, alloc::vec::Vec<u8>) {
    match firmware.command_version(DATA_PATH_GROUP, SCD_QUEUE_CONFIG) {
        Some(3) => {
            let mut config = alloc::vec![0u8; 36];
            put32(&mut config, 0, SCD_QUEUE_REMOVE);
            put32(&mut config, 4, 1 << STATION_ID);
            config[8] = MANAGEMENT_TID;
            (DATA_PATH_GROUP, SCD_QUEUE_CONFIG, config)
        }
        _ => {
            let mut config = alloc::vec![0u8; 24];
            config[0] = STATION_ID;
            config[1] = MANAGEMENT_TID;
            (LEGACY_GROUP, TX_QUEUE_CONFIG, config)
        }
    }
}

/// Undoes [`join`]: ends session protection, turns the management queue off, removes the
/// access point's station, the binding, the MAC context and the PHY context, in that order, so
/// the adapter can scan and join another network. The link's queue memory is freed after the
/// firmware let go of it.
pub fn leave<R: RegisterIo, M: DmaMemory>(
    transport: &mut Transport<R, M>,
    firmware: &Firmware,
    link: Link<M>,
    wait: &mut dyn Wait,
) -> Result<(), JoinError> {
    let mut session = [0u8; 24];
    put32(&mut session, 4, ACTION_REMOVE);
    put32(&mut session, 8, SESSION_CONFIG_ASSOC);
    transport.send_command(MAC_CONF_GROUP, SESSION_PROTECTION, &session, wait)?;
    let (group, code, config) = queue_off(firmware);
    transport.send_command(group, code, &config, wait)?;
    transport.send_command(LEGACY_GROUP, REMOVE_STATION, &[STATION_ID, 0, 0, 0], wait)?;
    let unbound = binding(&link.target.channel, firmware, ACTION_REMOVE);
    transport.send_command(LEGACY_GROUP, BINDING_CONTEXT, &unbound, wait)?;
    let mac = mac_context(link.address, &link.target, ACTION_REMOVE, None);
    transport.send_command(LEGACY_GROUP, MAC_CONTEXT, &mac, wait)?;
    transport.send_command(
        LEGACY_GROUP,
        PHY_CONTEXT,
        &phy_context(&link.target.channel, firmware, ACTION_REMOVE),
        wait,
    )?;
    drop(link);
    Ok(())
}

fn header_bytes(frame: &[u8]) -> usize {
    let control = frame[0];
    let data = control & 0x0c == 0x08;
    let quality_of_service = data && control & 0x80 != 0;
    let four_addresses = frame[1] & 0x03 == 0x03;
    24 + if four_addresses { 6 } else { 0 } + if quality_of_service { 2 } else { 0 }
}

impl<M: DmaMemory> Link<M> {
    /// Sends the 802.11 `frame` through the management queue, unencrypted, the firmware
    /// choosing its rate.
    pub fn send<R: RegisterIo>(
        &mut self,
        transport: &Transport<R, M>,
        frame: &[u8],
    ) -> Result<(), JoinError> {
        if frame.len() < 24 {
            return Err(JoinError::BadFrame);
        }
        let header = header_bytes(frame);
        let body = &frame[header..];
        if body.len() > PAYLOAD_SLOT_BYTES {
            return Err(JoinError::BadFrame);
        }
        let slot = self.next;
        let pad = (4 - header % 4) % 4;
        let offload = (((header / 2) as u32 & OFFLOAD_HEADER_MASK) << OFFLOAD_HEADER_SHIFT)
            | if pad != 0 { OFFLOAD_PAD } else { 0 };
        let mut command = [0u8; COMMAND_SLOT_BYTES];
        command[0] = TX_COMMAND;
        command[2] = slot as u8;
        command[3] = MANAGEMENT_QUEUE;
        let tx = COMMAND_HEADER_BYTES;
        let tx_bytes = match self.ax210 {
            true => {
                put(&mut command, tx, &(frame.len() as u16).to_le_bytes());
                put(
                    &mut command,
                    tx + 2,
                    &(TX_ENCRYPTION_OFF as u16).to_le_bytes(),
                );
                put32(&mut command, tx + 4, offload);
                TX_COMMAND_GEN3_BYTES
            }
            false => {
                put(&mut command, tx, &(frame.len() as u16).to_le_bytes());
                put(&mut command, tx + 2, &(offload as u16).to_le_bytes());
                put32(&mut command, tx + 4, TX_ENCRYPTION_OFF);
                TX_COMMAND_GEN2_BYTES
            }
        };
        let header_at = tx + tx_bytes;
        put(&mut command, header_at, &frame[..header]);
        let command_bytes = header_at + header + pad;
        let command_at = QUEUE_COMMANDS + slot * COMMAND_SLOT_BYTES;
        let payload_at = QUEUE_PAYLOADS + slot * PAYLOAD_SLOT_BYTES;
        let base = self.queue.dma_addr().get();
        let mut descriptor = [0u8; TFD_BYTES];
        let tbs: u16 = if body.is_empty() { 2 } else { 3 };
        put(&mut descriptor, 0, &tbs.to_le_bytes());
        put(&mut descriptor, 2, &(FIRST_TB_BYTES as u16).to_le_bytes());
        put(
            &mut descriptor,
            4,
            &(base + command_at as u64).to_le_bytes(),
        );
        put(
            &mut descriptor,
            2 + TB_BYTES,
            &((command_bytes - FIRST_TB_BYTES) as u16).to_le_bytes(),
        );
        put(
            &mut descriptor,
            4 + TB_BYTES,
            &(base + (command_at + FIRST_TB_BYTES) as u64).to_le_bytes(),
        );
        if !body.is_empty() {
            put(
                &mut descriptor,
                2 + 2 * TB_BYTES,
                &(body.len() as u16).to_le_bytes(),
            );
            put(
                &mut descriptor,
                4 + 2 * TB_BYTES,
                &(base + payload_at as u64).to_le_bytes(),
            );
        }
        let fetched = 2 + usize::from(tbs) * TB_BYTES;
        let chunks = fetched.div_ceil(64) as u16 - 1;
        let count = match self.ax210 {
            true => (frame.len() as u16) | (chunks << 14),
            false => (frame.len().div_ceil(4) as u16) | (chunks << 12),
        };
        self.write(command_at, &command[..command_bytes]);
        self.write(payload_at, body);
        self.write(QUEUE_TFDS + slot * TFD_BYTES, &descriptor);
        self.write(QUEUE_BYTE_COUNTS + slot * 2, &count.to_le_bytes());
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.next = (self.next + 1) % QUEUE_SLOTS;
        let size = match self.ax210 {
            true => 65_536,
            false => QUEUE_SLOTS,
        };
        self.hardware = (self.hardware + 1) % size;
        transport.ring_doorbell(
            (u32::from(MANAGEMENT_QUEUE) << HBUS_WRITE_POINTER_QUEUE_SHIFT) | self.hardware as u32,
        );
        Ok(())
    }

    fn write(&self, at: usize, bytes: &[u8]) {
        assert!(at + bytes.len() <= self.queue.len());
        // SAFETY: the range lies inside the queue's block (checked), which this link owns; the
        // device reads the slot only after the doorbell that follows.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.queue.as_mut_ptr().wrapping_add(at),
                bytes.len(),
            )
        };
    }

    /// The network this link joined.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// This station's own address.
    pub fn address(&self) -> [u8; 6] {
        self.address
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> Channel {
        Channel {
            number: 6,
            band: Band::TwoPointFour,
            active: true,
            radar: false,
        }
    }

    #[test]
    fn the_mac_context_names_the_station_and_the_network() {
        let target = Target {
            bssid: [2, 3, 4, 5, 6, 7],
            channel: channel(),
            beacon_interval: 100,
        };
        let command = mac_context([0, 1, 2, 3, 4, 5], &target, ACTION_ADD, None);
        assert_eq!(command.len(), 148);
        assert_eq!(status_of(&command[8..]), MAC_TYPE_BSS_STATION);
        assert_eq!(&command[16..22], &[0, 1, 2, 3, 4, 5]);
        assert_eq!(&command[24..30], &target.bssid);
        assert_ne!(status_of(&command[52..]) & FILTER_IN_BEACON, 0);
        let associated = mac_context([0; 6], &target, ACTION_MODIFY, Some(0xc005));
        assert_eq!(status_of(&associated[100..]), 1);
        assert_eq!(status_of(&associated[136..]), 5);
        assert_eq!(header_bytes(&[0x88, 0x01]), 26);
        assert_eq!(header_bytes(&[0xb0, 0x00]), 24);
    }
}

//! The DMCUB mailbox commands the AMD display driver sends and the notifications it reads
//! (D790). Every command is 64 bytes: a 4-byte header (its type, its sub-type and its payload's
//! bytes in bits 24 to 29), then the payload. AUX requests go as `DP_AUX_ACCESS` (type 70) and
//! come back in outbox 1 as `DP_AUX_REPLY` (type 1), once outbox 1 is turned on
//! (`OUTBOX1_ENABLE`, type 71); the PHY is driven by the video BIOS's transmitter control,
//! which DMCUB runs (`VBIOS`, type 128, sub-type 1, its v1.7 parameters). Layouts and values
//! are from AMD's MIT `dmub_cmd.h` and `atomfirmware.h` (`AMD-NOTICE.md`).

use super::amd_dmcub::{COMMAND_BYTES, Command};

const HEADER_BYTES: usize = 4;
const PAYLOAD_BYTES_SHIFT: u32 = 24;
const PAYLOAD_BYTES_MASK: u32 = 0x3f;
const TYPE_DP_AUX_ACCESS: u8 = 70;
const TYPE_OUTBOX1_ENABLE: u8 = 71;
const TYPE_VBIOS: u8 = 128;
const VBIOS_TRANSMITTER_CONTROL: u8 = 1;
const NOTIFICATION_DP_AUX_REPLY: u8 = 1;
const AUX_CONTROL_BYTES: usize = 12;
const AUX_REQUEST_HEADER_BYTES: usize = 8;
const ACTION_I2C_WRITE: u8 = 0x00;
const ACTION_I2C_READ: u8 = 0x10;
const ACTION_I2C_WRITE_MORE: u8 = 0x40;
const ACTION_I2C_READ_MORE: u8 = 0x50;
const ACTION_NATIVE_WRITE: u8 = 0x80;
const ACTION_NATIVE_READ: u8 = 0x90;
const TRANSMITTER_BYTES: usize = 16;
const ACTION_DISABLE: u8 = 0;
const ACTION_ENABLE: u8 = 1;
const ACTION_SET_DRIVE: u8 = 11;
const PRE_EMPHASIS_SHIFT: u8 = 3;

/// The most bytes one AUX request carries.
pub const AUX_MOST_BYTES: usize = 16;
/// The encoder mode the video BIOS drives a DisplayPort link in.
pub const MODE_DISPLAY_PORT: u8 = 0;
/// The encoder mode for HDMI.
pub const MODE_HDMI: u8 = 3;
/// The video BIOS's connector object ids.
pub const CONNECTOR_DISPLAY_PORT: u8 = 0x13;
pub const CONNECTOR_EMBEDDED_DISPLAY_PORT: u8 = 0x14;
pub const CONNECTOR_HDMI: u8 = 0x0c;

fn command(kind: u8, sub_kind: u8, payload: &[u8]) -> Command {
    let mut out = [0u8; COMMAND_BYTES];
    let header = u32::from(kind)
        | u32::from(sub_kind) << 8
        | (payload.len() as u32 & PAYLOAD_BYTES_MASK) << PAYLOAD_BYTES_SHIFT;
    out[..HEADER_BYTES].copy_from_slice(&header.to_le_bytes());
    out[HEADER_BYTES..HEADER_BYTES + payload.len()].copy_from_slice(payload);
    out
}

/// The command that has the firmware post its notifications (AUX replies, hot plugs) in outbox 1.
pub fn outbox1_enable() -> Command {
    command(TYPE_OUTBOX1_ENABLE, 0, &1u32.to_le_bytes())
}

/// What an AUX request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxAction {
    NativeRead,
    NativeWrite,
    /// An I2C read over AUX; `more` keeps the I2C transaction open after it.
    I2cRead { more: bool },
    /// An I2C write over AUX; `more` keeps the I2C transaction open after it.
    I2cWrite { more: bool },
}

impl AuxAction {
    fn code(self) -> (bool, u8) {
        match self {
            Self::NativeRead => (false, ACTION_NATIVE_READ),
            Self::NativeWrite => (false, ACTION_NATIVE_WRITE),
            Self::I2cRead { more: false } => (true, ACTION_I2C_READ),
            Self::I2cRead { more: true } => (true, ACTION_I2C_READ_MORE),
            Self::I2cWrite { more: false } => (true, ACTION_I2C_WRITE),
            Self::I2cWrite { more: true } => (true, ACTION_I2C_WRITE_MORE),
        }
    }
}

/// The AUX request on AUX channel `instance`: `action` at `address` (a DPCD address, or the
/// I2C target's 7-bit address) of `length` bytes, carrying `data` when it writes; `None` when
/// more than [`AUX_MOST_BYTES`] are asked.
pub fn aux_access(
    instance: u8,
    action: AuxAction,
    address: u32,
    length: usize,
    data: &[u8],
) -> Option<Command> {
    if length > AUX_MOST_BYTES || data.len() > AUX_MOST_BYTES {
        return None;
    }
    let (is_i2c, code) = action.code();
    let mut payload = [0u8; AUX_CONTROL_BYTES + AUX_REQUEST_HEADER_BYTES + AUX_MOST_BYTES];
    payload[0] = instance;
    let request = &mut payload[AUX_CONTROL_BYTES..];
    request[0] = u8::from(is_i2c);
    request[1] = code;
    request[2] = length as u8;
    request[4..8].copy_from_slice(&address.to_le_bytes());
    request[AUX_REQUEST_HEADER_BYTES..AUX_REQUEST_HEADER_BYTES + data.len()]
        .copy_from_slice(data);
    Some(command(TYPE_DP_AUX_ACCESS, 0, &payload))
}

/// How the sink answered an AUX request: the reply's command bits (the DisplayPort reply
/// byte's high nibble), so ACK is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxAnswer {
    Ack,
    Nack,
    Defer,
}

/// DMCUB's notification of an AUX reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxReply {
    /// The firmware's result: 0 when the transaction completed, else why not (timed out, hot
    /// plug lost, ...).
    pub result: u8,
    pub answer: AuxAnswer,
    pub length: usize,
    pub data: [u8; AUX_MOST_BYTES],
}

/// The AUX reply in `notification`, if it is one.
pub fn aux_reply(notification: &Command) -> Option<AuxReply> {
    if notification[0] != NOTIFICATION_DP_AUX_REPLY {
        return None;
    }
    let control = &notification[HEADER_BYTES..HEADER_BYTES + 8];
    let reply = &notification[HEADER_BYTES + 8..HEADER_BYTES + 8 + 4 + AUX_MOST_BYTES];
    let answer = match reply[0] {
        0 => AuxAnswer::Ack,
        bits if bits & (0x02 | 0x08 | 0x20) != 0 => AuxAnswer::Defer,
        _ => AuxAnswer::Nack,
    };
    let mut data = [0u8; AUX_MOST_BYTES];
    data.copy_from_slice(&reply[4..]);
    Some(AuxReply {
        result: control[5],
        answer,
        length: usize::from(reply[1]).min(AUX_MOST_BYTES),
        data,
    })
}

/// The PHY a DisplayPort or HDMI output drives and what the video BIOS knows it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transmitter {
    /// The PHY: 0 for UNIPHY A.
    pub phy: u8,
    /// The hot-plug pin, counted from 1 (0 for none).
    pub hot_plug: u8,
    /// The DIG front end (stream encoder) feeding it.
    pub front_end: u8,
    /// The connector's object id ([`CONNECTOR_DISPLAY_PORT`], ...).
    pub connector: u8,
    pub lanes: u8,
}

/// What the transmitter control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmitterAction {
    Disable,
    /// Turns the PHY on in encoder mode `mode` with its PLL at `link_khz` (the link's symbol
    /// clock: the DisplayPort rate in kHz, 270000 for 2.7 Gbps, or HDMI's TMDS clock).
    Enable { mode: u8, link_khz: u32 },
    /// Drives every lane at a voltage swing and pre-emphasis level, 0 to 3 each.
    SetDrive { swing: u8, pre_emphasis: u8 },
}

/// The video BIOS transmitter control command for `transmitter`.
pub fn transmitter_control(transmitter: &Transmitter, action: TransmitterAction) -> Command {
    let (code, mode_or_drive, link_khz) = match action {
        TransmitterAction::Disable => (ACTION_DISABLE, MODE_DISPLAY_PORT, 0),
        TransmitterAction::Enable { mode, link_khz } => (ACTION_ENABLE, mode, link_khz),
        TransmitterAction::SetDrive {
            swing,
            pre_emphasis,
        } => (
            ACTION_SET_DRIVE,
            swing.min(3) | pre_emphasis.min(3) << PRE_EMPHASIS_SHIFT,
            0,
        ),
    };
    let mut payload = [0u8; TRANSMITTER_BYTES];
    payload[0] = transmitter.phy;
    payload[1] = code;
    payload[2] = mode_or_drive;
    payload[3] = transmitter.lanes;
    payload[4..8].copy_from_slice(&(link_khz / 10).to_le_bytes());
    payload[8] = transmitter.hot_plug;
    payload[9] = 1u8.checked_shl(u32::from(transmitter.front_end)).unwrap_or(0);
    payload[10] = transmitter.connector;
    command(TYPE_VBIOS, VBIOS_TRANSMITTER_CONTROL, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aux_read_carries_its_instance_action_length_and_address() {
        let read = aux_access(2, AuxAction::NativeRead, 0x202, 6, &[]).unwrap();
        assert_eq!(&read[..4], &[70, 0, 0, 36]);
        assert_eq!(read[4], 2);
        assert_eq!(&read[16..20], &[0, 0x90, 6, 0]);
        assert_eq!(&read[20..24], &0x202u32.to_le_bytes());
        let edid = aux_access(0, AuxAction::I2cWrite { more: true }, 0x50, 1, &[0]).unwrap();
        assert_eq!(&edid[16..19], &[1, 0x40, 1]);
        assert!(aux_access(0, AuxAction::NativeRead, 0, 17, &[]).is_none());
    }

    #[test]
    fn an_aux_reply_gives_its_answer_and_bytes() {
        let mut notification = [0u8; COMMAND_BYTES];
        notification[0] = 1;
        notification[13] = 3;
        notification[16..19].copy_from_slice(&[0x12, 0x0a, 0x84]);
        let reply = aux_reply(&notification).unwrap();
        assert_eq!(reply.answer, AuxAnswer::Ack);
        assert_eq!(reply.length, 3);
        assert_eq!(&reply.data[..3], &[0x12, 0x0a, 0x84]);
        notification[12] = 0x02;
        assert_eq!(aux_reply(&notification).unwrap().answer, AuxAnswer::Defer);
        notification[0] = 2;
        assert!(aux_reply(&notification).is_none());
    }

    #[test]
    fn the_transmitter_control_packs_the_bios_parameters() {
        let transmitter = Transmitter {
            phy: 1,
            hot_plug: 2,
            front_end: 3,
            connector: CONNECTOR_DISPLAY_PORT,
            lanes: 4,
        };
        let enable = transmitter_control(
            &transmitter,
            TransmitterAction::Enable {
                mode: MODE_DISPLAY_PORT,
                link_khz: 540_000,
            },
        );
        assert_eq!(&enable[..4], &[128, 1, 0, 16]);
        assert_eq!(&enable[4..8], &[1, 1, 0, 4]);
        assert_eq!(&enable[8..12], &54_000u32.to_le_bytes());
        assert_eq!(&enable[12..15], &[2, 8, 0x13]);
        let drive = transmitter_control(
            &transmitter,
            TransmitterAction::SetDrive {
                swing: 2,
                pre_emphasis: 1,
            },
        );
        assert_eq!(&drive[5..7], &[11, 0x0a]);
    }
}

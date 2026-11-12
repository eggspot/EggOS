//! Intel's DisplayPort AUX channels (D445): each digital port's control register and five data
//! registers carry one AUX message to the sink and its reply back, the bytes packed most
//! significant first. On top of them: native reads of the sink's DPCD (its link capabilities)
//! and I2C over AUX to the sink's EDID at address 0x50.

use crate::io::RegisterIo;

/// The most bytes one AUX message or reply carries, its header included.
pub const MOST_BYTES: usize = 20;
/// The most data bytes one read asks for.
pub const MOST_READ: usize = 16;
/// The DPCD's receiver capabilities: its first 16 bytes.
pub const RECEIVER_CAPABILITIES: u32 = 0x000;
/// The EDID's I2C address.
pub const EDID_ADDRESS: u32 = 0x50;

const CONTROL: usize = 0x64010;
const DATA: usize = 0x64014;
const PORT_STRIDE: usize = 0x100;
const SEND_BUSY: u32 = 1 << 31;
const DONE: u32 = 1 << 30;
const TIMED_OUT: u32 = 1 << 28;
const TIME_OUT_1600_US: u32 = 3 << 26;
const RECEIVE_ERROR: u32 = 1 << 25;
const SIZE_SHIFT: u32 = 20;
const SIZE_MASK: u32 = 0x1f;
const SYNC_PULSES: u32 = 31 << 5;
const FAST_WAKE_PULSES: u32 = 17;
const NATIVE_READ: u8 = 0x9;
const I2C_WRITE: u8 = 0x0;
const I2C_READ: u8 = 0x1;
const MIDDLE_OF_TRANSACTION: u8 = 0x4;
const REPLY_NACK: u8 = 0x1;
const REPLY_DEFER: u8 = 0x2;
const I2C_REPLY_NACK: u8 = 0x1;
const I2C_REPLY_DEFER: u8 = 0x2;
const MOST_TRIES: usize = 7;

/// Why an AUX transaction failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxError {
    /// The sink did not answer in time, or nothing is connected.
    TimedOut,
    /// The reply came back damaged.
    Damaged,
    /// The sink refused the request.
    Refused,
    /// The sink asked to wait more times than allowed.
    Deferred,
}

/// An AUX message's four header bytes: the request and address, then the length less one
/// (no length byte for an address-only I2C message).
pub fn header(request: u8, address: u32, length: usize) -> [u8; 4] {
    [
        request << 4 | ((address >> 16) & 0xf) as u8,
        (address >> 8) as u8,
        address as u8,
        length.saturating_sub(1) as u8,
    ]
}

/// `bytes` packed into the data registers' words, most significant first.
pub fn pack(bytes: &[u8]) -> [u32; MOST_BYTES / 4] {
    let mut words = [0u32; MOST_BYTES / 4];
    for (index, byte) in bytes.iter().take(MOST_BYTES).enumerate() {
        words[index / 4] |= u32::from(*byte) << (24 - 8 * (index % 4));
    }
    words
}

/// The bytes `words` hold, most significant first.
pub fn unpack(words: &[u32; MOST_BYTES / 4]) -> [u8; MOST_BYTES] {
    let mut bytes = [0u8; MOST_BYTES];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (words[index / 4] >> (24 - 8 * (index % 4))) as u8;
    }
    bytes
}

/// One AUX transaction on port `port` (0 is port A): sends `message` and puts the reply in
/// `reply`; how many bytes came back. Polls while `keep_waiting` says to.
pub fn transfer<R: RegisterIo + ?Sized>(
    regs: &R,
    port: usize,
    message: &[u8],
    reply: &mut [u8; MOST_BYTES],
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<usize, AuxError> {
    let control = CONTROL + port * PORT_STRIDE;
    let data = DATA + port * PORT_STRIDE;
    for (index, word) in pack(message).iter().enumerate() {
        regs.write32(data + 4 * index, *word);
    }
    let size = (message.len().min(MOST_BYTES) as u32) << SIZE_SHIFT;
    regs.write32(
        control,
        SEND_BUSY | DONE | TIMED_OUT | RECEIVE_ERROR | TIME_OUT_1600_US | size | SYNC_PULSES | FAST_WAKE_PULSES,
    );
    let status = loop {
        let status = regs.read32(control);
        if status & SEND_BUSY == 0 {
            break status;
        }
        if !keep_waiting() {
            return Err(AuxError::TimedOut);
        }
    };
    regs.write32(control, status | DONE | TIMED_OUT | RECEIVE_ERROR);
    if status & TIMED_OUT != 0 {
        return Err(AuxError::TimedOut);
    }
    if status & RECEIVE_ERROR != 0 {
        return Err(AuxError::Damaged);
    }
    let length = ((status >> SIZE_SHIFT) & SIZE_MASK) as usize;
    let mut words = [0u32; MOST_BYTES / 4];
    for (index, word) in words.iter_mut().enumerate() {
        *word = regs.read32(data + 4 * index);
    }
    *reply = unpack(&words);
    Ok(length.min(MOST_BYTES))
}

/// Reads `out.len()` (at most [`MOST_READ`]) bytes of the sink's DPCD from `address`.
pub fn read_dpcd<R: RegisterIo + ?Sized>(
    regs: &R,
    port: usize,
    address: u32,
    out: &mut [u8],
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), AuxError> {
    let length = out.len().min(MOST_READ);
    let message = header(NATIVE_READ, address, length);
    for _ in 0..MOST_TRIES {
        let mut reply = [0u8; MOST_BYTES];
        let got = transfer(regs, port, &message, &mut reply, keep_waiting)?;
        match reply[0] >> 4 & 0x3 {
            REPLY_DEFER => continue,
            REPLY_NACK => return Err(AuxError::Refused),
            _ if got < 1 + length => return Err(AuxError::Damaged),
            _ => {
                out[..length].copy_from_slice(&reply[1..1 + length]);
                return Ok(());
            }
        }
    }
    Err(AuxError::Deferred)
}

fn i2c<R: RegisterIo + ?Sized>(
    regs: &R,
    port: usize,
    message: &[u8],
    reply: &mut [u8; MOST_BYTES],
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<usize, AuxError> {
    for _ in 0..MOST_TRIES {
        let got = transfer(regs, port, message, reply, keep_waiting)?;
        match (reply[0] >> 4 & 0x3, reply[0] >> 6 & 0x3) {
            (REPLY_DEFER, _) | (_, I2C_REPLY_DEFER) => continue,
            (REPLY_NACK, _) | (_, I2C_REPLY_NACK) => return Err(AuxError::Refused),
            _ => return Ok(got),
        }
    }
    Err(AuxError::Deferred)
}

/// Reads the sink's EDID base block over I2C on AUX: the offset 0 written, then 16-byte reads,
/// the transaction kept open until the last.
pub fn read_edid<R: RegisterIo + ?Sized>(
    regs: &R,
    port: usize,
    out: &mut [u8; crate::edid::BLOCK_BYTES],
    keep_waiting: &mut dyn FnMut() -> bool,
) -> Result<(), AuxError> {
    let mut reply = [0u8; MOST_BYTES];
    let [request, high, low, length] = header(I2C_WRITE | MIDDLE_OF_TRANSACTION, EDID_ADDRESS, 1);
    let start = [request, high, low, length, 0];
    i2c(regs, port, &start, &mut reply, keep_waiting)?;
    for chunk in 0..crate::edid::BLOCK_BYTES / MOST_READ {
        let message = header(I2C_READ | MIDDLE_OF_TRANSACTION, EDID_ADDRESS, MOST_READ);
        let got = i2c(regs, port, &message, &mut reply, keep_waiting)?;
        if got < 1 + MOST_READ {
            return Err(AuxError::Damaged);
        }
        out[chunk * MOST_READ..(chunk + 1) * MOST_READ].copy_from_slice(&reply[1..1 + MOST_READ]);
    }
    let stop = header(I2C_READ, EDID_ADDRESS, 1);
    let _ = i2c(regs, port, &stop[..3], &mut reply, keep_waiting);
    Ok(())
}

/// What a sink's receiver capabilities say: its DPCD revision, the highest link rate in
/// gigabits per second times 100 per lane (162, 270, 540, 810), and its lane count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkCapabilities {
    pub revision: u8,
    pub rate_centigbps: u32,
    pub lanes: u8,
}

/// The capabilities in DPCD bytes 0 to 2.
pub fn link_capabilities(dpcd: &[u8]) -> Option<LinkCapabilities> {
    let rate = match *dpcd.get(1)? {
        0x06 => 162,
        0x0a => 270,
        0x14 => 540,
        0x1e => 810,
        _ => return None,
    };
    Some(LinkCapabilities {
        revision: dpcd[0],
        rate_centigbps: rate,
        lanes: *dpcd.get(2)? & 0x1f,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_packs_most_significant_byte_first() {
        let message = header(NATIVE_READ, RECEIVER_CAPABILITIES, 16);
        assert_eq!(message, [0x90, 0, 0, 15]);
        let words = pack(&message);
        assert_eq!(words[0], 0x9000_000f);
        assert_eq!(&unpack(&words)[..4], &message);
    }

    #[test]
    fn an_edid_read_names_the_i2c_address() {
        assert_eq!(header(I2C_READ | MIDDLE_OF_TRANSACTION, EDID_ADDRESS, 16), [0x50, 0, 0x50, 15]);
    }

    #[test]
    fn capabilities_name_the_rate_and_lanes() {
        assert_eq!(
            link_capabilities(&[0x12, 0x14, 0x84]),
            Some(LinkCapabilities {
                revision: 0x12,
                rate_centigbps: 540,
                lanes: 4,
            })
        );
        assert_eq!(link_capabilities(&[0x12, 0x07, 4]), None);
    }
}

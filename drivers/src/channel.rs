//! The s390x channel subsystem (z/Architecture Principles of Operation, chapters 13 to 16): the
//! control blocks the I/O instructions read and write, built big-endian whatever the host, the
//! trait through which the HAL gives drivers those instructions, and channel programs of one
//! command run to their end by polling.

use crate::dma::DmaMemory;

/// A subchannel's identification word, as the I/O instructions take it in general register 1:
/// the subchannel set, a one bit, and the subchannel number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubchannelId(u32);

impl SubchannelId {
    const ONE: u32 = 1 << 16;
    const SET_SHIFT: u32 = 17;

    /// Subchannel `number` of subchannel set `set` (0 to 3).
    pub const fn new(set: u8, number: u16) -> Self {
        Self(((set as u32 & 3) << Self::SET_SHIFT) | Self::ONE | number as u32)
    }

    /// The identification word.
    pub const fn word(self) -> u32 {
        self.0
    }

    /// The identification `word` an interruption code stored, if it names a subchannel.
    pub const fn from_word(word: u32) -> Option<Self> {
        if word & Self::ONE == 0 {
            return None;
        }
        Some(Self(word))
    }

    /// The subchannel set.
    pub const fn set(self) -> u8 {
        ((self.0 >> Self::SET_SHIFT) & 3) as u8
    }

    /// The subchannel number.
    pub const fn number(self) -> u16 {
        self.0 as u16
    }
}

impl core::fmt::Display for SubchannelId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "0.{}.{:04x}", self.set(), self.number())
    }
}

/// The condition code an I/O instruction set: 0 to 3, its meaning the instruction's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionCode(u8);

impl ConditionCode {
    /// The instruction did what it was asked.
    pub const DONE: Self = Self(0);
    /// `TEST SUBCHANNEL`: no status was pending; `START`/`MODIFY SUBCHANNEL`: status is pending.
    pub const STATUS_PENDING: Self = Self(1);
    /// The subchannel is busy.
    pub const BUSY: Self = Self(2);
    /// There is no such subchannel, or no path to its device.
    pub const NOT_OPERATIONAL: Self = Self(3);

    /// The condition code `code` (its two low bits).
    pub const fn new(code: u8) -> Self {
        Self(code & 3)
    }

    /// The code, 0 to 3.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Bytes of a subchannel-information block.
pub const SCHIB_BYTES: usize = 52;
/// Bytes of an operation-request block (the 12 a format-1 request uses, padded to 32).
pub const ORB_BYTES: usize = 32;
/// Bytes of an interruption-response block.
pub const IRB_BYTES: usize = 96;
/// Bytes of a format-1 channel-command word.
pub const CCW_BYTES: usize = 8;

const PMCW_ISC_ENABLE: usize = 4;
const PMCW_FLAGS: usize = 5;
const PMCW_DEVICE_NUMBER: usize = 6;
const PMCW_TYPE: usize = 25;
const SCHIB_SCSW: usize = 28;
const ENABLED: u8 = 0x80;
const DEVICE_NUMBER_VALID: u8 = 0x01;
const ISC_SHIFT: u8 = 3;

/// The subchannel types (`PMCW.ST`).
pub mod subchannel_type {
    /// An I/O subchannel: a device on channel paths.
    pub const IO: u8 = 0;
}

/// A subchannel-information block: the path-management-control word, the subchannel-status
/// word and the model-dependent area, as `STORE SUBCHANNEL` stores them and
/// `MODIFY SUBCHANNEL` reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(4))]
pub struct Schib(pub [u8; SCHIB_BYTES]);

impl Default for Schib {
    fn default() -> Self {
        Self([0; SCHIB_BYTES])
    }
}

impl Schib {
    /// Whether the subchannel has a device number, that is whether a device is installed.
    pub fn device_number_valid(&self) -> bool {
        self.0[PMCW_FLAGS] & DEVICE_NUMBER_VALID != 0
    }

    /// The device number.
    pub fn device_number(&self) -> u16 {
        u16::from_be_bytes([self.0[PMCW_DEVICE_NUMBER], self.0[PMCW_DEVICE_NUMBER + 1]])
    }

    /// The subchannel type ([`subchannel_type`]).
    pub fn subchannel_type(&self) -> u8 {
        self.0[PMCW_TYPE] >> 5
    }

    /// Whether the subchannel is enabled for I/O.
    pub fn is_enabled(&self) -> bool {
        self.0[PMCW_FLAGS] & ENABLED != 0
    }

    /// Enables the subchannel, its interruptions in subclass `isc` (0 to 7) with `parameter` as
    /// their interruption parameter, keeping the rest of the word.
    pub fn enable(&mut self, isc: u8, parameter: u32) {
        self.0[..4].copy_from_slice(&parameter.to_be_bytes());
        self.0[PMCW_ISC_ENABLE] = (self.0[PMCW_ISC_ENABLE] & !(7 << ISC_SHIFT)) | ((isc & 7) << ISC_SHIFT);
        self.0[PMCW_FLAGS] |= ENABLED;
    }

    /// The subchannel-status word.
    pub fn status(&self) -> Scsw {
        let mut bytes = [0; SCSW_BYTES];
        bytes.copy_from_slice(&self.0[SCHIB_SCSW..SCHIB_SCSW + SCSW_BYTES]);
        Scsw(bytes)
    }
}

const SCSW_BYTES: usize = 12;

/// Status-control bits of a subchannel-status word.
pub mod status_control {
    /// Status is pending.
    pub const PENDING: u8 = 0x01;
    /// The status is the secondary status (the device's end).
    pub const SECONDARY: u8 = 0x02;
    /// The status is the primary status (the channel program's end at the subchannel).
    pub const PRIMARY: u8 = 0x04;
    /// Intermediate status.
    pub const INTERMEDIATE: u8 = 0x08;
    /// Alert status.
    pub const ALERT: u8 = 0x10;
}

/// Device-status bits.
pub mod device_status {
    pub const ATTENTION: u8 = 0x80;
    pub const STATUS_MODIFIER: u8 = 0x40;
    pub const CONTROL_UNIT_END: u8 = 0x20;
    pub const BUSY: u8 = 0x10;
    pub const CHANNEL_END: u8 = 0x08;
    pub const DEVICE_END: u8 = 0x04;
    pub const UNIT_CHECK: u8 = 0x02;
    pub const UNIT_EXCEPTION: u8 = 0x01;
}

/// Subchannel-status bits.
pub mod subchannel_status {
    pub const PROGRAM_CONTROLLED_INTERRUPTION: u8 = 0x80;
    pub const INCORRECT_LENGTH: u8 = 0x40;
    pub const PROGRAM_CHECK: u8 = 0x20;
    pub const PROTECTION_CHECK: u8 = 0x10;
    pub const CHANNEL_DATA_CHECK: u8 = 0x08;
    pub const CHANNEL_CONTROL_CHECK: u8 = 0x04;
    pub const INTERFACE_CONTROL_CHECK: u8 = 0x02;
    pub const CHAINING_CHECK: u8 = 0x01;
}

/// A subchannel-status word (command mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scsw(pub [u8; SCSW_BYTES]);

impl Scsw {
    /// The status-control bits ([`status_control`]).
    pub fn status_control(&self) -> u8 {
        self.0[3] & 0x1f
    }

    /// The device status ([`device_status`]).
    pub fn device_status(&self) -> u8 {
        self.0[8]
    }

    /// The subchannel status ([`subchannel_status`]).
    pub fn subchannel_status(&self) -> u8 {
        self.0[9]
    }

    /// The bytes the last command did not transfer.
    pub fn residual_count(&self) -> u16 {
        u16::from_be_bytes([self.0[10], self.0[11]])
    }
}

/// An operation-request block for `START SUBCHANNEL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct Orb(pub [u8; ORB_BYTES]);

const ORB_FORMAT_1: u8 = 0x80;

impl Orb {
    /// A request to run the format-1 channel program at `program` (below 2 GiB) on the paths
    /// of `path_mask`, its interruptions carrying `parameter`.
    pub fn format1(parameter: u32, program: u32, path_mask: u8) -> Self {
        let mut bytes = [0; ORB_BYTES];
        bytes[..4].copy_from_slice(&parameter.to_be_bytes());
        bytes[5] = ORB_FORMAT_1;
        bytes[6] = path_mask;
        bytes[8..12].copy_from_slice(&program.to_be_bytes());
        Self(bytes)
    }
}

/// An interruption-response block, as `TEST SUBCHANNEL` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct Irb(pub [u8; IRB_BYTES]);

impl Default for Irb {
    fn default() -> Self {
        Self([0; IRB_BYTES])
    }
}

impl Irb {
    /// The subchannel-status word.
    pub fn status(&self) -> Scsw {
        let mut bytes = [0; SCSW_BYTES];
        bytes.copy_from_slice(&self.0[..SCSW_BYTES]);
        Scsw(bytes)
    }
}

/// Flags of a channel-command word.
pub mod ccw_flags {
    /// Chain the data of the next word.
    pub const CHAIN_DATA: u8 = 0x80;
    /// Chain the next word's command.
    pub const CHAIN_COMMAND: u8 = 0x40;
    /// Suppress the incorrect-length indication.
    pub const SUPPRESS_LENGTH: u8 = 0x20;
    /// Skip the data transfer.
    pub const SKIP: u8 = 0x10;
    /// Interrupt when the word is fetched.
    pub const PROGRAM_CONTROLLED_INTERRUPTION: u8 = 0x08;
    /// The data address names a list of indirect data addresses.
    pub const INDIRECT_DATA: u8 = 0x04;
}

/// A format-1 channel-command word: command, flags, byte count and a 31-bit data address.
pub fn ccw1(command: u8, flags: u8, count: u16, data: u32) -> [u8; CCW_BYTES] {
    let mut bytes = [0; CCW_BYTES];
    bytes[0] = command;
    bytes[1] = flags;
    bytes[2..4].copy_from_slice(&count.to_be_bytes());
    bytes[4..].copy_from_slice(&(data & 0x7fff_ffff).to_be_bytes());
    bytes
}

/// The I/O instructions the HAL gives drivers (s390x's channel subsystem).
pub trait ChannelSubsystem: Sync + core::fmt::Debug {
    /// `STORE SUBCHANNEL`: stores the subchannel's information into `schib`;
    /// [`ConditionCode::NOT_OPERATIONAL`] when there is no such subchannel.
    fn store_subchannel(&self, subchannel: SubchannelId, schib: &mut Schib) -> ConditionCode;
    /// `MODIFY SUBCHANNEL`: sets the subchannel's modifiable fields from `schib`.
    fn modify_subchannel(&self, subchannel: SubchannelId, schib: &Schib) -> ConditionCode;
    /// `START SUBCHANNEL` with the request `orb`.
    ///
    /// # Safety
    /// The channel program `orb` names, and every byte its words name, stay the caller's,
    /// readable and writable at those absolute addresses, until `TEST SUBCHANNEL` stored the
    /// program's final status (or the subchannel was cleared).
    unsafe fn start_subchannel(&self, subchannel: SubchannelId, orb: &Orb) -> ConditionCode;
    /// `TEST SUBCHANNEL`: stores the subchannel's status into `irb`, clearing a pending one.
    fn test_subchannel(&self, subchannel: SubchannelId, irb: &mut Irb) -> ConditionCode;
    /// The hypervisor's virtio-ccw notification (`DIAGNOSE 0x500`, function 3): queue `queue`
    /// of the device on `subchannel` has new buffers. Returns the hypervisor's cookie for the
    /// next notification, or a negative number when it refused.
    fn notify_virtio_queue(&self, subchannel: SubchannelId, queue: u16, cookie: u64) -> i64;
}

/// Why a channel program did not end well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// There is no such subchannel or no path to its device.
    NotOperational,
    /// The subchannel stayed busy.
    Busy,
    /// No final status came within the polls allowed.
    TimedOut,
    /// The device refused the command (unit check), as for a command it does not know.
    UnitCheck,
    /// The subchannel reported these subchannel-status bits.
    SubchannelCheck(u8),
    /// The device ended with this unexpected device status.
    DeviceStatus(u8),
    /// The memory for the channel program is above the 2 GiB a format-1 word reaches, or too
    /// small.
    OutOfReach,
}

impl ChannelError {
    /// What went wrong, for a person.
    pub const fn description(self) -> &'static str {
        match self {
            ChannelError::NotOperational => "the subchannel is not operational",
            ChannelError::Busy => "the subchannel is busy",
            ChannelError::TimedOut => "the channel program did not end in time",
            ChannelError::UnitCheck => "the device refused the channel command",
            ChannelError::SubchannelCheck(_) => "the channel subsystem reported a check",
            ChannelError::DeviceStatus(_) => "the device ended with an unexpected status",
            ChannelError::OutOfReach => "the channel program's memory is out of a channel's reach",
        }
    }
}

/// The interruption subclass EggOS's subchannels use. Control register 6 leaves it disabled
/// while the channel programs are polled, so their status waits for `TEST SUBCHANNEL`.
pub const POLLED_SUBCLASS: u8 = 3;

/// The `TEST SUBCHANNEL` polls a channel program gets before it times out.
pub const MAX_POLLS: usize = 1 << 22;

/// Enables `subchannel` for I/O in [`POLLED_SUBCLASS`], with its subchannel word as the
/// interruption parameter. Refuses a subchannel without a device.
pub fn enable(channels: &dyn ChannelSubsystem, subchannel: SubchannelId) -> Result<Schib, ChannelError> {
    let mut schib = Schib::default();
    if channels.store_subchannel(subchannel, &mut schib) != ConditionCode::DONE {
        return Err(ChannelError::NotOperational);
    }
    if !schib.device_number_valid() {
        return Err(ChannelError::NotOperational);
    }
    if schib.is_enabled() {
        return Ok(schib);
    }
    schib.enable(POLLED_SUBCLASS, subchannel.word());
    match channels.modify_subchannel(subchannel, &schib) {
        ConditionCode::DONE => {}
        ConditionCode::STATUS_PENDING => {
            let mut irb = Irb::default();
            channels.test_subchannel(subchannel, &mut irb);
            if channels.modify_subchannel(subchannel, &schib) != ConditionCode::DONE {
                return Err(ChannelError::Busy);
            }
        }
        ConditionCode::BUSY => return Err(ChannelError::Busy),
        _ => return Err(ChannelError::NotOperational),
    }
    let mut check = Schib::default();
    if channels.store_subchannel(subchannel, &mut check) != ConditionCode::DONE || !check.is_enabled() {
        return Err(ChannelError::NotOperational);
    }
    Ok(check)
}

/// Where in a block of channel memory ([`ChannelBuffer`]) the program and its data are.
const CCW_OFFSET: usize = 0;
const DATA_OFFSET: usize = 256;
/// The data bytes one command of a [`ChannelBuffer`] moves at most.
pub const DATA_BYTES: usize = 3840;
/// The bytes a [`ChannelBuffer`] needs.
pub const BUFFER_BYTES: usize = DATA_OFFSET + DATA_BYTES;
const BELOW_2_GIB: u64 = 1 << 31;

/// A block of DMA memory below 2 GiB that holds one channel command and its data, so that a
/// driver runs single-command channel programs on a subchannel.
#[derive(Debug)]
pub struct ChannelBuffer<M: DmaMemory> {
    memory: M,
}

impl<M: DmaMemory> ChannelBuffer<M> {
    /// The buffer in `memory`, which must hold [`BUFFER_BYTES`] and end below 2 GiB in the
    /// channel's view; memory too small or out of reach comes back with the error.
    pub fn new(memory: M) -> Result<Self, (ChannelError, M)> {
        let end = memory.dma_addr().get().checked_add(memory.len() as u64);
        if memory.len() < BUFFER_BYTES || end.is_none_or(|end| end > BELOW_2_GIB) {
            return Err((ChannelError::OutOfReach, memory));
        }
        Ok(Self { memory })
    }

    /// The memory back, for freeing once the subchannel ran its last program with it.
    pub fn into_memory(self) -> M {
        self.memory
    }

    fn address(&self, offset: usize) -> u32 {
        (self.memory.dma_addr().get() + offset as u64) as u32
    }

    /// The data bytes, as the last command left them.
    pub fn data(&self) -> &[u8] {
        // SAFETY: `DATA_OFFSET..BUFFER_BYTES` lies inside the memory (`new` checked), which the
        // buffer owns; a channel program writes it only while `run` waits for its end.
        unsafe { core::slice::from_raw_parts(self.memory.as_mut_ptr().add(DATA_OFFSET), DATA_BYTES) }
    }

    /// The data bytes, to fill before a command.
    pub fn data_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `data`; `&mut self` makes the slice the only one.
        unsafe { core::slice::from_raw_parts_mut(self.memory.as_mut_ptr().add(DATA_OFFSET), DATA_BYTES) }
    }

    /// Runs `command` on `subchannel` over the first `count` data bytes and polls for its end:
    /// channel end and device end, with no check. A shorter transfer than `count` is no error.
    pub fn run(
        &mut self,
        channels: &dyn ChannelSubsystem,
        subchannel: SubchannelId,
        command: u8,
        count: u16,
    ) -> Result<Scsw, ChannelError> {
        let count = count.min(DATA_BYTES as u16);
        let word = ccw1(command, ccw_flags::SUPPRESS_LENGTH, count, self.address(DATA_OFFSET));
        // SAFETY: `CCW_OFFSET..CCW_OFFSET + CCW_BYTES` lies inside the memory, which the buffer
        // owns, and no channel program runs now (`&mut self`).
        unsafe {
            core::ptr::copy_nonoverlapping(
                word.as_ptr(),
                self.memory.as_mut_ptr().add(CCW_OFFSET),
                CCW_BYTES,
            )
        };
        let orb = Orb::format1(subchannel.word(), self.address(CCW_OFFSET), 0xff);
        let mut irb = Irb::default();
        // SAFETY: the program and its data are this buffer's memory, below 2 GiB (`new`),
        // which `&mut self` keeps unused until the loop below saw the program's final status.
        let started = unsafe { channels.start_subchannel(subchannel, &orb) };
        match started {
            ConditionCode::DONE => {}
            ConditionCode::STATUS_PENDING => {
                channels.test_subchannel(subchannel, &mut irb);
                return Err(ChannelError::Busy);
            }
            ConditionCode::BUSY => return Err(ChannelError::Busy),
            _ => return Err(ChannelError::NotOperational),
        }
        for _ in 0..MAX_POLLS {
            match channels.test_subchannel(subchannel, &mut irb) {
                ConditionCode::DONE => {}
                ConditionCode::STATUS_PENDING => {
                    core::hint::spin_loop();
                    continue;
                }
                _ => return Err(ChannelError::NotOperational),
            }
            let status = irb.status();
            if status.status_control() & (status_control::PRIMARY | status_control::SECONDARY) == 0
                && status.status_control() & status_control::ALERT == 0
            {
                continue;
            }
            return final_status(status);
        }
        Err(ChannelError::TimedOut)
    }
}

fn final_status(status: Scsw) -> Result<Scsw, ChannelError> {
    let checks = status.subchannel_status()
        & !(subchannel_status::INCORRECT_LENGTH | subchannel_status::PROGRAM_CONTROLLED_INTERRUPTION);
    if checks != 0 {
        return Err(ChannelError::SubchannelCheck(checks));
    }
    let device = status.device_status();
    if device & device_status::UNIT_CHECK != 0 {
        return Err(ChannelError::UnitCheck);
    }
    let ends = device_status::CHANNEL_END | device_status::DEVICE_END;
    if device & ends != ends || device & device_status::UNIT_EXCEPTION != 0 {
        return Err(ChannelError::DeviceStatus(device));
    }
    Ok(status)
}

/// `SENSE ID`: the control unit's and device's types and models.
pub const SENSE_ID: u8 = 0xe4;

/// What `SENSE ID` says of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenseId {
    pub control_unit_type: u16,
    pub control_unit_model: u8,
    pub device_type: u16,
    pub device_model: u8,
}

/// Asks the device on `subchannel` for its [`SenseId`], through `buffer`.
pub fn sense_id<M: DmaMemory>(
    channels: &dyn ChannelSubsystem,
    subchannel: SubchannelId,
    buffer: &mut ChannelBuffer<M>,
) -> Result<SenseId, ChannelError> {
    const SENSE_ID_BYTES: u16 = 7;
    buffer.data_mut()[..usize::from(SENSE_ID_BYTES)].fill(0);
    buffer.run(channels, subchannel, SENSE_ID, SENSE_ID_BYTES)?;
    let data = buffer.data();
    if data[0] != 0xff {
        return Err(ChannelError::DeviceStatus(data[0]));
    }
    Ok(SenseId {
        control_unit_type: u16::from_be_bytes([data[1], data[2]]),
        control_unit_model: data[3],
        device_type: u16::from_be_bytes([data[4], data[5]]),
        device_model: data[6],
    })
}

#[cfg(test)]
pub(crate) mod tests;

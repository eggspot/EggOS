//! DisplayPort on AMD's DCN 3.x through DMCUB (D790): AUX requests go to the firmware as
//! mailbox commands and their replies come back in outbox 1, so the driver never arbitrates
//! the AUX engines with it; on top of them the sink's DPCD, its EDID by I2C over AUX (target
//! `0x50`: the offset written, then 16-byte reads, the transaction kept open until the last),
//! and link training ([`crate::display_port::train`]) with the DIG back end's patterns and the
//! video BIOS's transmitter control for the PHY's PLL and drive.

use super::amd_dcn::DisplayEngine;
use super::amd_dmcub::{Dmcub, DmcubError, Mailbox};
use super::amd_dmub_command::{
    self, AUX_MOST_BYTES, AuxAction, AuxAnswer, Transmitter, TransmitterAction,
};
use crate::display_port::{Drive, Link};
use crate::io::RegisterIo;

const EDID_TARGET: u32 = 0x50;
const DEFER_TRIES: usize = 7;
const DEFER_PAUSE_MICROSECONDS: u32 = 400;

/// Why an AUX transaction failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxError {
    /// The mailbox did not carry the request.
    Mailbox(DmcubError),
    /// No reply came in time.
    NoReply,
    /// The firmware could not complete it (its result code: timed out, hot plug lost, ...).
    Failed(u8),
    /// The sink refused it.
    Refused,
    /// The sink kept deferring it.
    Deferred,
    /// The sink answered fewer bytes than asked.
    Short,
}

/// How the AUX channel waits: whether to keep polling, and a pause of some microseconds.
pub trait Wait {
    fn keep_waiting(&mut self) -> bool;
    fn pause(&mut self, microseconds: u32);
}

/// AUX channel `instance` through the running DMCUB `dmcub`, whose mailbox is `mailbox`, with
/// outbox 1 already turned on ([`amd_dmub_command::outbox1_enable`]).
pub struct AuxChannel<'a, R: RegisterIo> {
    pub dmcub: &'a Dmcub<R>,
    pub mailbox: &'a mut dyn Mailbox,
    pub instance: u8,
    pub wait: &'a mut dyn Wait,
}

impl<R: RegisterIo> AuxChannel<'_, R> {
    fn send(&mut self, command: &amd_dmub_command::Command) -> Result<(), DmcubError> {
        let wait = &mut *self.wait;
        self.dmcub
            .send(&mut *self.mailbox, command, &mut || wait.keep_waiting())
    }

    fn transact(
        &mut self,
        action: AuxAction,
        address: u32,
        length: usize,
        data: &[u8],
    ) -> Result<amd_dmub_command::AuxReply, AuxError> {
        let request = amd_dmub_command::aux_access(self.instance, action, address, length, data)
            .ok_or(AuxError::Short)?;
        for _ in 0..DEFER_TRIES {
            self.send(&request).map_err(AuxError::Mailbox)?;
            let reply = loop {
                if let Some(reply) = self
                    .dmcub
                    .receive(&*self.mailbox)
                    .and_then(|notification| amd_dmub_command::aux_reply(&notification))
                {
                    break reply;
                }
                if !self.wait.keep_waiting() {
                    return Err(AuxError::NoReply);
                }
                core::hint::spin_loop();
            };
            if reply.result != 0 {
                return Err(AuxError::Failed(reply.result));
            }
            match reply.answer {
                AuxAnswer::Ack => return Ok(reply),
                AuxAnswer::Nack => return Err(AuxError::Refused),
                AuxAnswer::Defer => self.wait.pause(DEFER_PAUSE_MICROSECONDS),
            }
        }
        Err(AuxError::Deferred)
    }

    /// Reads the sink's DPCD from `address` into `out`.
    pub fn read_dpcd(&mut self, address: u32, out: &mut [u8]) -> Result<(), AuxError> {
        for (index, chunk) in out.chunks_mut(AUX_MOST_BYTES).enumerate() {
            let at = address + (index * AUX_MOST_BYTES) as u32;
            let reply = self.transact(AuxAction::NativeRead, at, chunk.len(), &[])?;
            if reply.length < chunk.len() {
                return Err(AuxError::Short);
            }
            chunk.copy_from_slice(&reply.data[..chunk.len()]);
        }
        Ok(())
    }

    /// Writes `bytes` to the sink's DPCD at `address`.
    pub fn write_dpcd(&mut self, address: u32, bytes: &[u8]) -> Result<(), AuxError> {
        for (index, chunk) in bytes.chunks(AUX_MOST_BYTES).enumerate() {
            let at = address + (index * AUX_MOST_BYTES) as u32;
            self.transact(AuxAction::NativeWrite, at, chunk.len(), chunk)?;
        }
        Ok(())
    }

    /// Reads the sink's EDID base block.
    pub fn read_edid(&mut self, out: &mut [u8; crate::edid::BLOCK_BYTES]) -> Result<(), AuxError> {
        self.transact(AuxAction::I2cWrite { more: true }, EDID_TARGET, 1, &[0])?;
        let mut result = Ok(());
        for chunk in out.chunks_mut(AUX_MOST_BYTES) {
            let read = self.transact(
                AuxAction::I2cRead { more: true },
                EDID_TARGET,
                chunk.len(),
                &[],
            );
            match read {
                Ok(reply) if reply.length >= chunk.len() => {
                    chunk.copy_from_slice(&reply.data[..chunk.len()])
                }
                Ok(_) => {
                    result = Err(AuxError::Short);
                    break;
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        let _ = self.transact(AuxAction::I2cRead { more: false }, EDID_TARGET, 0, &[]);
        result
    }

    /// Has the video BIOS, through DMCUB, do `action` on `transmitter`.
    pub fn control(
        &mut self,
        transmitter: &Transmitter,
        action: TransmitterAction,
    ) -> Result<(), DmcubError> {
        self.send(&amd_dmub_command::transmitter_control(transmitter, action))
    }
}

/// The lit DisplayPort output as [`crate::display_port::train`] drives it: patterns from the
/// DIG back end of `engine`, drive levels through the video BIOS, the DPCD through `aux`.
pub struct TrainedLink<'a, 'b, R: RegisterIo, D: RegisterIo> {
    pub engine: &'a DisplayEngine<R>,
    pub aux: &'a mut AuxChannel<'b, D>,
    pub transmitter: Transmitter,
}

impl<R: RegisterIo, D: RegisterIo> Link for TrainedLink<'_, '_, R, D> {
    fn send_pattern(&mut self, pattern: u8) {
        self.engine.send_training_pattern(pattern);
    }

    fn set_drive(&mut self, drive: Drive) {
        let _ = self.aux.control(
            &self.transmitter,
            TransmitterAction::SetDrive {
                swing: drive.swing,
                pre_emphasis: drive.pre_emphasis,
            },
        );
    }

    fn pause(&mut self, microseconds: u32) {
        self.aux.wait.pause(microseconds);
    }

    fn read(&mut self, address: u32, out: &mut [u8]) -> bool {
        self.aux.read_dpcd(address, out).is_ok()
    }

    fn write(&mut self, address: u32, bytes: &[u8]) -> bool {
        self.aux.write_dpcd(address, bytes).is_ok()
    }
}

/// The link rate code `rate` (0x06, 0x0a, 0x14, 0x1e) as the PHY's symbol clock in kHz.
pub fn rate_khz(rate: u8) -> u32 {
    u32::from(rate) * 27_000
}

/// Trains the lit output's link again at rate code `rate` on `lanes` lanes: the stream
/// stopped, the PHY off, the back end set to the lanes, the PHY on at the rate, the training,
/// then the stream started again; the drive it settled on.
pub fn retrain<R: RegisterIo, D: RegisterIo>(
    engine: &mut DisplayEngine<R>,
    aux: &mut AuxChannel<'_, D>,
    transmitter: Transmitter,
    rate: u8,
    lanes: usize,
) -> Result<Drive, crate::display_port::TrainingError> {
    let transmitter = Transmitter {
        lanes: lanes as u8,
        ..transmitter
    };
    let link_khz = rate_khz(rate);
    let wait = &mut *aux.wait;
    let stopped = engine.run_stream(false, &mut || wait.keep_waiting()).is_ok();
    let drive = match stopped && power_phy(engine, aux, &transmitter, lanes, link_khz) {
        true => {
            let mut link = TrainedLink {
                engine: &*engine,
                aux: &mut *aux,
                transmitter,
            };
            crate::display_port::train(&mut link, lanes, rate)
        }
        false => Err(crate::display_port::TrainingError::NoAnswer),
    };
    if drive.is_ok() {
        engine.retrained(lanes, link_khz);
    }
    let wait = &mut *aux.wait;
    let _ = engine.run_stream(true, &mut || wait.keep_waiting());
    drive
}

fn power_phy<R: RegisterIo, D: RegisterIo>(
    engine: &DisplayEngine<R>,
    aux: &mut AuxChannel<'_, D>,
    transmitter: &Transmitter,
    lanes: usize,
    link_khz: u32,
) -> bool {
    if aux.control(transmitter, TransmitterAction::Disable).is_err() {
        return false;
    }
    engine.set_lanes(lanes);
    let enable = TransmitterAction::Enable {
        mode: amd_dmub_command::MODE_DISPLAY_PORT,
        link_khz,
    };
    aux.control(transmitter, enable).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_rate_codes_are_multiples_of_270_mbps() {
        assert_eq!(rate_khz(0x06), 162_000);
        assert_eq!(rate_khz(0x0a), 270_000);
        assert_eq!(rate_khz(0x14), 540_000);
        assert_eq!(rate_khz(0x1e), 810_000);
    }
}

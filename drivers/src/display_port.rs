//! DisplayPort link training (VESA DisplayPort 1.4, 3.5.1.2; D446): the source sends training
//! pattern 1 until every lane's clock recovers, raising each lane's voltage swing and
//! pre-emphasis as the sink asks, then pattern 2 until every lane's channel equalises, symbols
//! lock and the lanes align; then the link carries pixels. The sink's DPCD and the source's
//! port are both reached through [`Link`], so the algorithm is tested on the host against a
//! sink of its own.

/// The DPCD registers training reads and writes.
pub mod dpcd {
    pub const LINK_BW_SET: u32 = 0x100;
    pub const LANE_COUNT_SET: u32 = 0x101;
    pub const TRAINING_PATTERN_SET: u32 = 0x102;
    pub const TRAINING_LANE0_SET: u32 = 0x103;
    pub const LANE0_1_STATUS: u32 = 0x202;
    pub const ADJUST_REQUEST_LANE0_1: u32 = 0x206;
}

const PATTERN_1: u8 = 1;
const PATTERN_2: u8 = 2;
const PATTERN_OFF: u8 = 0;
const SCRAMBLING_DISABLED: u8 = 1 << 5;
const CLOCK_RECOVERY_DONE: u8 = 1;
const CHANNEL_EQUALIZED: u8 = 1 << 1;
const SYMBOL_LOCKED: u8 = 1 << 2;
const INTERLANE_ALIGNED: u8 = 1;
const MAX_LEVEL: u8 = 3;
const MAX_SWING_REACHED: u8 = 1 << 2;
const MAX_PRE_EMPHASIS_REACHED: u8 = 1 << 5;
const CLOCK_RECOVERY_TRIES: usize = 5;
const EQUALIZATION_TRIES: usize = 5;

/// A lane's drive: its voltage swing and pre-emphasis levels, 0 to 3 each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drive {
    pub swing: u8,
    pub pre_emphasis: u8,
}

/// A link: the source port's patterns and drive, and the sink's DPCD.
pub trait Link {
    /// Sends training pattern `pattern` (1 or 2), or pixels with 0.
    fn send_pattern(&mut self, pattern: u8);
    /// Drives every lane at `drive` (the levels the sink asked for, the highest of its lanes').
    fn set_drive(&mut self, drive: Drive);
    /// Waits the time the sink needs between a pattern's change and its status (100 µs for
    /// clock recovery, 400 µs for equalisation at the DPCD's default).
    fn pause(&mut self, microseconds: u32);
    /// Reads the sink's DPCD from `address`.
    fn read(&mut self, address: u32, out: &mut [u8]) -> bool;
    /// Writes the sink's DPCD at `address`.
    fn write(&mut self, address: u32, bytes: &[u8]) -> bool;
}

/// Why training failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingError {
    /// The sink did not answer its DPCD.
    NoAnswer,
    /// A lane's clock did not recover at the highest drive.
    ClockRecovery,
    /// The lanes did not equalise, lock and align.
    Equalization,
}

fn lane_status(status: &[u8; 2], lane: usize) -> u8 {
    (status[lane / 2] >> (4 * (lane % 2))) & 0xf
}

fn asked_drive(adjust: &[u8; 2], lanes: usize) -> Drive {
    (0..lanes).fold(Drive::default(), |drive, lane| {
        let request = (adjust[lane / 2] >> (4 * (lane % 2))) & 0xf;
        Drive {
            swing: drive.swing.max(request & 0x3),
            pre_emphasis: drive.pre_emphasis.max(request >> 2 & 0x3),
        }
    })
}

fn lane_set(drive: Drive) -> u8 {
    let mut set = drive.swing | drive.pre_emphasis << 3;
    if drive.swing == MAX_LEVEL {
        set |= MAX_SWING_REACHED;
    }
    if drive.pre_emphasis == MAX_LEVEL {
        set |= MAX_PRE_EMPHASIS_REACHED;
    }
    set
}

fn write_drive(link: &mut dyn Link, drive: Drive, lanes: usize) -> Result<(), TrainingError> {
    link.set_drive(drive);
    let set = [lane_set(drive); 4];
    link.write(dpcd::TRAINING_LANE0_SET, &set[..lanes])
        .then_some(())
        .ok_or(TrainingError::NoAnswer)
}

fn status(link: &mut dyn Link) -> Result<([u8; 2], u8, [u8; 2]), TrainingError> {
    let mut bytes = [0u8; 6];
    if !link.read(dpcd::LANE0_1_STATUS, &mut bytes) {
        return Err(TrainingError::NoAnswer);
    }
    Ok(([bytes[0], bytes[1]], bytes[2], [bytes[4], bytes[5]]))
}

/// Trains a link of `lanes` lanes (1, 2 or 4) at link rate code `rate` (0x06, 0x0a, 0x14 or
/// 0x1e): the drive it settled on.
pub fn train(link: &mut dyn Link, lanes: usize, rate: u8) -> Result<Drive, TrainingError> {
    let lanes = lanes.clamp(1, 4);
    let answered =
        link.write(dpcd::LINK_BW_SET, &[rate]) && link.write(dpcd::LANE_COUNT_SET, &[lanes as u8]);
    if !answered {
        return Err(TrainingError::NoAnswer);
    }
    let mut drive = Drive::default();
    link.send_pattern(PATTERN_1);
    link.write(
        dpcd::TRAINING_PATTERN_SET,
        &[PATTERN_1 | SCRAMBLING_DISABLED],
    )
    .then_some(())
    .ok_or(TrainingError::NoAnswer)?;
    write_drive(link, drive, lanes)?;
    let mut tries = 0;
    loop {
        link.pause(100);
        let (lane_states, _, adjust) = status(link)?;
        if (0..lanes).all(|lane| lane_status(&lane_states, lane) & CLOCK_RECOVERY_DONE != 0) {
            break;
        }
        let asked = asked_drive(&adjust, lanes);
        if asked == drive {
            tries += 1;
            if tries >= CLOCK_RECOVERY_TRIES || drive.swing == MAX_LEVEL {
                return Err(TrainingError::ClockRecovery);
            }
        } else {
            tries = 0;
        }
        drive = asked;
        write_drive(link, drive, lanes)?;
    }
    link.send_pattern(PATTERN_2);
    link.write(
        dpcd::TRAINING_PATTERN_SET,
        &[PATTERN_2 | SCRAMBLING_DISABLED],
    )
    .then_some(())
    .ok_or(TrainingError::NoAnswer)?;
    for _ in 0..EQUALIZATION_TRIES {
        link.pause(400);
        let (lane_states, align, adjust) = status(link)?;
        let done = (0..lanes).all(|lane| {
            let state = lane_status(&lane_states, lane);
            state & (CLOCK_RECOVERY_DONE | CHANNEL_EQUALIZED | SYMBOL_LOCKED)
                == CLOCK_RECOVERY_DONE | CHANNEL_EQUALIZED | SYMBOL_LOCKED
        });
        if done && align & INTERLANE_ALIGNED != 0 {
            link.send_pattern(PATTERN_OFF);
            link.write(dpcd::TRAINING_PATTERN_SET, &[PATTERN_OFF])
                .then_some(())
                .ok_or(TrainingError::NoAnswer)?;
            return Ok(drive);
        }
        drive = asked_drive(&adjust, lanes);
        write_drive(link, drive, lanes)?;
    }
    link.send_pattern(PATTERN_OFF);
    let _ = link.write(dpcd::TRAINING_PATTERN_SET, &[PATTERN_OFF]);
    Err(TrainingError::Equalization)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLink {
        registers: [u8; 0x300],
        needs_swing: u8,
        patterns: Vec<u8>,
        drive: Drive,
    }

    impl Link for FakeLink {
        fn send_pattern(&mut self, pattern: u8) {
            self.patterns.push(pattern);
        }

        fn set_drive(&mut self, drive: Drive) {
            self.drive = drive;
        }

        fn pause(&mut self, _microseconds: u32) {}

        fn read(&mut self, address: u32, out: &mut [u8]) -> bool {
            let pattern = self.registers[dpcd::TRAINING_PATTERN_SET as usize] & 0x3;
            let swing = self.registers[dpcd::TRAINING_LANE0_SET as usize] & 0x3;
            let lanes = self.registers[dpcd::LANE_COUNT_SET as usize] as usize;
            let mut lane = 0u8;
            if pattern >= PATTERN_1 && swing >= self.needs_swing {
                lane |= CLOCK_RECOVERY_DONE;
            }
            if pattern == PATTERN_2 && lane != 0 {
                lane |= CHANNEL_EQUALIZED | SYMBOL_LOCKED;
            }
            let both = lane | lane << 4;
            self.registers[0x202] = both;
            self.registers[0x203] = if lanes > 2 { both } else { 0 };
            self.registers[0x204] = u8::from(pattern == PATTERN_2 && lane != 0);
            let asked = self.needs_swing | self.needs_swing << 4;
            self.registers[0x206] = asked;
            self.registers[0x207] = asked;
            let at = address as usize;
            out.copy_from_slice(&self.registers[at..at + out.len()]);
            true
        }

        fn write(&mut self, address: u32, bytes: &[u8]) -> bool {
            let at = address as usize;
            self.registers[at..at + bytes.len()].copy_from_slice(bytes);
            true
        }
    }

    #[test]
    fn training_raises_the_swing_the_sink_asks_and_ends_on_pixels() {
        let mut sink = FakeLink {
            registers: [0; 0x300],
            needs_swing: 2,
            patterns: Vec::new(),
            drive: Drive::default(),
        };
        let drive = train(&mut sink, 4, 0x14).unwrap();
        assert_eq!(drive.swing, 2);
        assert_eq!(sink.drive.swing, 2);
        assert_eq!(sink.patterns, [PATTERN_1, PATTERN_2, PATTERN_OFF]);
        assert_eq!(sink.registers[dpcd::LINK_BW_SET as usize], 0x14);
        assert_eq!(sink.registers[dpcd::LANE_COUNT_SET as usize], 4);
        assert_eq!(
            sink.registers[dpcd::TRAINING_PATTERN_SET as usize],
            PATTERN_OFF
        );
    }

    #[test]
    fn a_lane_whose_clock_never_recovers_fails_training() {
        let mut sink = FakeLink {
            registers: [0; 0x300],
            needs_swing: 4,
            patterns: Vec::new(),
            drive: Drive::default(),
        };
        assert_eq!(train(&mut sink, 2, 0x0a), Err(TrainingError::ClockRecovery));
    }
}

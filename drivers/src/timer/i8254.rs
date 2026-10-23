//! The PC's Intel 8254 programmable interval timer (8254 datasheet), used only as a reference
//! clock of known frequency to measure the frequencies of the other timers at boot.
//!
//! Channel 2 is used, because its gate and output are wired to the PC's "system control
//! port B" (port 0x61) instead of an interrupt line: it can be started and polled without
//! interrupts and without disturbing channel 0, the firmware's tick.

use crate::io::RegisterIo;

/// The 8254's input clock in Hz (the PC's 14.31818 MHz crystal divided by 12).
pub const FREQUENCY_HZ: u64 = 1_193_182;

const CHANNEL2_COUNTER: usize = 2;
const MODE_COMMAND: usize = 3;
const MODE_SELECT_CHANNEL2: u8 = 0b10 << 6;
const MODE_ACCESS_LOW_THEN_HIGH_BYTE: u8 = 0b11 << 4;
const MODE_0_INTERRUPT_ON_TERMINAL_COUNT: u8 = 0b000 << 1;
const MODE_BINARY_COUNT: u8 = 0;
const MODE_CHANNEL2_ONE_SHOT: u8 = MODE_SELECT_CHANNEL2
    | MODE_ACCESS_LOW_THEN_HIGH_BYTE
    | MODE_0_INTERRUPT_ON_TERMINAL_COUNT
    | MODE_BINARY_COUNT;

const SYSTEM_CONTROL_PORT_B: usize = 0;
const CONTROL_GATE_CHANNEL2: u8 = 1 << 0;
const CONTROL_SPEAKER_ENABLE: u8 = 1 << 1;
const CONTROL_CHANNEL2_OUTPUT: u8 = 1 << 5;

/// Size of the 8254's register block (ports 0x40-0x43).
pub const REGISTER_BLOCK_SIZE: usize = 4;
/// Size of the system control port block (port 0x61).
pub const CONTROL_BLOCK_SIZE: usize = 1;

/// The 8254, with the system control port that gates and reads its channel 2.
#[derive(Debug)]
pub struct Pit<R: RegisterIo> {
    regs: R,
    control: R,
}

impl<R: RegisterIo> Pit<R> {
    pub const fn new(regs: R, control: R) -> Self {
        Self { regs, control }
    }

    /// Runs channel 2 for `count` periods of [`FREQUENCY_HZ`], calling `on_start` right after
    /// it starts counting, and returns `true` once it has counted down. Polls `timed_out`
    /// while waiting and returns `false` as soon as it says so: some machines have no working
    /// 8254 (it can be gated off or left out by firmware and hypervisors), and the wait must
    /// not hang there. The speaker stays off.
    #[must_use]
    pub fn run_channel2(
        &self,
        count: u16,
        on_start: impl FnOnce(),
        mut timed_out: impl FnMut() -> bool,
    ) -> bool {
        let gate_low = self.control.read8(SYSTEM_CONTROL_PORT_B)
            & !(CONTROL_SPEAKER_ENABLE | CONTROL_GATE_CHANNEL2);
        self.control.write8(SYSTEM_CONTROL_PORT_B, gate_low);
        self.regs.write8(MODE_COMMAND, MODE_CHANNEL2_ONE_SHOT);
        let [low, high] = count.to_le_bytes();
        self.regs.write8(CHANNEL2_COUNTER, low);
        self.regs.write8(CHANNEL2_COUNTER, high);
        self.control
            .write8(SYSTEM_CONTROL_PORT_B, gate_low | CONTROL_GATE_CHANNEL2);
        on_start();
        let mut counted_down = true;
        while !self.channel2_output_high_at_terminal_count() {
            if timed_out() {
                counted_down = false;
                break;
            }
            core::hint::spin_loop();
        }
        self.control.write8(SYSTEM_CONTROL_PORT_B, gate_low);
        counted_down
    }

    fn channel2_output_high_at_terminal_count(&self) -> bool {
        self.control.read8(SYSTEM_CONTROL_PORT_B) & CONTROL_CHANNEL2_OUTPUT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn channel2_is_loaded_with_the_gate_low_then_started_and_polled() {
        let pit = Pit::new(FakeRegs::default(), FakeRegs::default());
        let speaker_and_gate_left_on_by_firmware = 0b11;
        let output_low = 0;
        pit.control.script_reads(
            SYSTEM_CONTROL_PORT_B,
            &[
                speaker_and_gate_left_on_by_firmware,
                output_low,
                output_low,
                CONTROL_CHANNEL2_OUTPUT as u32,
            ],
        );
        let mut started = false;
        assert!(pit.run_channel2(0x1234, || started = true, || false));
        assert!(started);
        assert_eq!(
            pit.regs.writes(),
            vec![
                (MODE_COMMAND, 0b1011_0000),
                (CHANNEL2_COUNTER, 0x34),
                (CHANNEL2_COUNTER, 0x12)
            ]
        );
        assert_eq!(pit.control.writes(), vec![(0, 0), (0, 1), (0, 0)]);
        assert_eq!(pit.control.reads_of(0), 4);
    }

    #[test]
    fn a_pit_that_never_counts_down_times_out_and_is_stopped() {
        let pit = Pit::new(FakeRegs::default(), FakeRegs::default());
        let mut polls = 0;
        assert!(!pit.run_channel2(
            100,
            || {},
            || {
                polls += 1;
                polls == 3
            }
        ));
        assert_eq!(polls, 3);
        assert_eq!(pit.control.writes(), vec![(0, 0), (0, 1), (0, 0)]);
    }
}

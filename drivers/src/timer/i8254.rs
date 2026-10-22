//! The PC's Intel 8254 programmable interval timer (8254 datasheet), used only as a reference
//! clock of known frequency to measure the frequencies of the other timers at boot.
//!
//! Channel 2 is used, because its gate and output are wired to the PC's "system control
//! port B" (port 0x61) instead of an interrupt line: it can be started and polled without
//! interrupts and without disturbing channel 0, the firmware's tick.

use crate::io::RegisterIo;

/// The 8254's input clock in Hz (the PC's 14.31818 MHz crystal divided by 12).
pub const FREQUENCY_HZ: u64 = 1_193_182;

/// Channel 2 counter register (ports 0x40-0x43: counters 0-2, then the mode register).
const CHANNEL2: usize = 2;
/// Mode/command register.
const MODE: usize = 3;
/// Channel 2 (bits 7:6 = 10), low byte then high byte (5:4 = 11), mode 0 "interrupt on
/// terminal count" (3:1 = 000), binary (0 = 0).
const MODE_CHANNEL2_ONE_SHOT: u8 = 0b1011_0000;

/// Port 0x61: bit 0 gates channel 2, bit 1 connects it to the PC speaker, bit 5 reads
/// channel 2's output.
const CONTROL_GATE2: u8 = 1 << 0;
const CONTROL_SPEAKER: u8 = 1 << 1;
const CONTROL_OUT2: u8 = 1 << 5;

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
        let control = self.control.read8(0) & !(CONTROL_SPEAKER | CONTROL_GATE2);
        // Gate low while the count is loaded, so counting starts only when it goes high.
        self.control.write8(0, control);
        self.regs.write8(MODE, MODE_CHANNEL2_ONE_SHOT);
        let [low, high] = count.to_le_bytes();
        self.regs.write8(CHANNEL2, low);
        self.regs.write8(CHANNEL2, high);
        self.control.write8(0, control | CONTROL_GATE2);
        on_start();
        // In mode 0 the output goes high when the count reaches zero.
        let mut counted_down = true;
        while self.control.read8(0) & CONTROL_OUT2 == 0 {
            if timed_out() {
                counted_down = false;
                break;
            }
            core::hint::spin_loop();
        }
        self.control.write8(0, control);
        counted_down
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    #[test]
    fn channel2_is_loaded_with_the_gate_low_then_started_and_polled() {
        let pit = Pit::new(FakeRegs::default(), FakeRegs::default());
        // Speaker bit set by firmware; output low twice, then high.
        pit.control
            .script_reads(0, &[0b11, 0, 0, CONTROL_OUT2 as u32]);
        let mut started = false;
        assert!(pit.run_channel2(0x1234, || started = true, || false));
        assert!(started);
        assert_eq!(
            pit.regs.writes(),
            vec![(MODE, 0b1011_0000), (CHANNEL2, 0x34), (CHANNEL2, 0x12)]
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

//! Synopsys DesignWare I2C controllers as a polled bus master (D462): Intel's LPSS controllers
//! (PCI functions, their registers at the start of BAR 0) and AMD's and others' that ACPI
//! declares (`AMDI0010`, `80860F41`). Registers as the public chipset datasheets describe
//! them: a transfer is a run of `IC_DATA_CMD` words, the bytes to write and then the reads,
//! the first read marked for a repeated start and the last word for a stop, fed as the
//! transmit FIFO drains and never more reads in flight than the receive FIFO holds.

use super::{I2cBus, I2cError, Speed};
use crate::io::RegisterIo;

const CONTROL: usize = 0x00;
const TARGET: usize = 0x04;
const DATA_COMMAND: usize = 0x10;
const STANDARD_HIGH_COUNT: usize = 0x14;
const STANDARD_LOW_COUNT: usize = 0x18;
const FAST_HIGH_COUNT: usize = 0x1c;
const FAST_LOW_COUNT: usize = 0x20;
const INTERRUPT_MASK: usize = 0x30;
const RAW_INTERRUPTS: usize = 0x34;
const RECEIVE_THRESHOLD: usize = 0x38;
const TRANSMIT_THRESHOLD: usize = 0x3c;
const CLEAR_INTERRUPTS: usize = 0x40;
const CLEAR_ABORT: usize = 0x54;
const CLEAR_STOP: usize = 0x60;
const ENABLE: usize = 0x6c;
const STATUS: usize = 0x70;
const TRANSMIT_LEVEL: usize = 0x74;
const RECEIVE_LEVEL: usize = 0x78;
const ABORT_SOURCE: usize = 0x80;
const ENABLE_STATUS: usize = 0x9c;
const COMPONENT_PARAMETERS: usize = 0xf4;
const COMPONENT_TYPE: usize = 0xfc;
const INTEL_LPSS_RESETS: usize = 0x204;

const DESIGNWARE_TYPE: u32 = 0x4457_0140;
const MASTER: u32 = 1;
const SPEED_STANDARD: u32 = 1 << 1;
const SPEED_FAST: u32 = 2 << 1;
const RESTART_ENABLED: u32 = 1 << 5;
const TARGET_DISABLED: u32 = 1 << 6;
const TEN_BIT_TARGET: u32 = 1 << 12;
const READ: u32 = 1 << 8;
const STOP: u32 = 1 << 9;
const RESTART: u32 = 1 << 10;
const ABORTED: u32 = 1 << 6;
const TRANSMIT_EMPTY: u32 = 1 << 2;
const MASTER_ACTIVE: u32 = 1 << 5;
const ENABLED: u32 = 1;
const LPSS_OUT_OF_RESET: u32 = 0x7;
const SMALLEST_FIFO: u32 = 2;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;
const STANDARD_HIGH_NS: u64 = 4_500;
const STANDARD_LOW_NS: u64 = 5_200;
const FAST_HIGH_NS: u64 = 800;
const FAST_LOW_NS: u64 = 1_600;
const HIGH_COUNT_OVERHEAD: u64 = 8;
const LOW_COUNT_OVERHEAD: u64 = 1;
const SMALLEST_HIGH_COUNT: u64 = 6;
const SMALLEST_LOW_COUNT: u64 = 8;
const LARGEST_ADDRESS_7: u16 = 0x7f;
const LARGEST_ADDRESS_10: u16 = 0x3ff;

/// Takes an Intel LPSS controller's function out of reset through its private registers
/// (`RESETS` at 0x204), as firmware usually leaves it; for Intel PCI functions only.
pub fn release_intel_reset<R: RegisterIo>(regs: &R) {
    regs.write32(INTEL_LPSS_RESETS, LPSS_OUT_OF_RESET);
}

/// A DesignWare controller set up as the bus master.
#[derive(Debug)]
pub struct DesignWare<R: RegisterIo> {
    regs: R,
    transmit_depth: u32,
    receive_depth: u32,
}

fn cycles(clock_hz: u32, nanoseconds: u64) -> u64 {
    (u64::from(clock_hz) * nanoseconds).div_ceil(NANOSECONDS_PER_SECOND)
}

impl<R: RegisterIo> DesignWare<R> {
    /// Sets the controller behind `regs` up as the master at `speed`; the clock's high and low
    /// counts are the firmware's when it set them, or worked out from the controller's
    /// `input_clock_hz` when it left them 0. `NotThisController` when the registers do not say
    /// DesignWare.
    pub fn new(
        regs: R,
        input_clock_hz: u32,
        speed: Speed,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<Self, I2cError> {
        if regs.read32(COMPONENT_TYPE) != DESIGNWARE_TYPE {
            return Err(I2cError::NotThisController);
        }
        let parameters = regs.read32(COMPONENT_PARAMETERS);
        let mut controller = Self {
            transmit_depth: (((parameters >> 16) & 0xff) + 1).max(SMALLEST_FIFO),
            receive_depth: (((parameters >> 8) & 0xff) + 1).max(SMALLEST_FIFO),
            regs,
        };
        controller.set_enabled(false, keep_waiting)?;
        let (speed_bits, high, low, high_ns, low_ns) = match speed {
            Speed::Standard => (
                SPEED_STANDARD,
                STANDARD_HIGH_COUNT,
                STANDARD_LOW_COUNT,
                STANDARD_HIGH_NS,
                STANDARD_LOW_NS,
            ),
            Speed::Fast => (
                SPEED_FAST,
                FAST_HIGH_COUNT,
                FAST_LOW_COUNT,
                FAST_HIGH_NS,
                FAST_LOW_NS,
            ),
        };
        let regs = &controller.regs;
        regs.write32(
            CONTROL,
            MASTER | speed_bits | RESTART_ENABLED | TARGET_DISABLED,
        );
        if regs.read32(high) == 0 || regs.read32(low) == 0 {
            let high_count = cycles(input_clock_hz, high_ns)
                .saturating_sub(HIGH_COUNT_OVERHEAD)
                .max(SMALLEST_HIGH_COUNT);
            let low_count = cycles(input_clock_hz, low_ns)
                .saturating_sub(LOW_COUNT_OVERHEAD)
                .max(SMALLEST_LOW_COUNT);
            regs.write32(high, high_count.min(0xffff) as u32);
            regs.write32(low, low_count.min(0xffff) as u32);
        }
        regs.write32(INTERRUPT_MASK, 0);
        regs.write32(RECEIVE_THRESHOLD, 0);
        regs.write32(TRANSMIT_THRESHOLD, 0);
        regs.read32(CLEAR_INTERRUPTS);
        Ok(controller)
    }

    /// The registers, to hand back.
    pub fn into_registers(self) -> R {
        self.regs
    }

    fn set_enabled(
        &mut self,
        enabled: bool,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), I2cError> {
        let wanted = u32::from(enabled);
        self.regs.write32(ENABLE, wanted);
        while self.regs.read32(ENABLE_STATUS) & ENABLED != wanted {
            if !keep_waiting() {
                return Err(I2cError::TimedOut);
            }
        }
        Ok(())
    }

    fn abort(&mut self, keep_waiting: &mut dyn FnMut() -> bool) -> I2cError {
        let source = self.regs.read32(ABORT_SOURCE);
        self.regs.read32(CLEAR_ABORT);
        let _ = self.set_enabled(false, keep_waiting);
        I2cError::NotAcknowledged(source)
    }
}

impl<R: RegisterIo> I2cBus for DesignWare<R> {
    fn transfer(
        &mut self,
        address: u16,
        ten_bit: bool,
        write: &[u8],
        read: &mut [u8],
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), I2cError> {
        let largest = match ten_bit {
            true => LARGEST_ADDRESS_10,
            false => LARGEST_ADDRESS_7,
        };
        if (write.is_empty() && read.is_empty()) || address > largest {
            return Err(I2cError::Invalid);
        }
        self.set_enabled(false, keep_waiting)?;
        let wide = match ten_bit {
            true => TEN_BIT_TARGET,
            false => 0,
        };
        self.regs.write32(TARGET, u32::from(address) | wide);
        self.set_enabled(true, keep_waiting)?;
        self.regs.read32(CLEAR_INTERRUPTS);
        let total = write.len() + read.len();
        let mut sent = 0;
        let mut received = 0;
        loop {
            if self.regs.read32(RAW_INTERRUPTS) & ABORTED != 0 {
                return Err(self.abort(keep_waiting));
            }
            while received < read.len() && self.regs.read32(RECEIVE_LEVEL) > 0 {
                read[received] = self.regs.read32(DATA_COMMAND) as u8;
                received += 1;
            }
            while sent < total && self.regs.read32(TRANSMIT_LEVEL) < self.transmit_depth {
                let reads_in_flight = sent.saturating_sub(write.len()) - received;
                if sent >= write.len() && reads_in_flight >= self.receive_depth as usize {
                    break;
                }
                let mut word = match write.get(sent) {
                    Some(&byte) => u32::from(byte),
                    None if sent == write.len() && !write.is_empty() => READ | RESTART,
                    None => READ,
                };
                if sent + 1 == total {
                    word |= STOP;
                }
                self.regs.write32(DATA_COMMAND, word);
                sent += 1;
            }
            let status = self.regs.read32(STATUS);
            let idle = status & TRANSMIT_EMPTY != 0 && status & MASTER_ACTIVE == 0;
            if sent == total && received == read.len() && idle {
                if self.regs.read32(RAW_INTERRUPTS) & ABORTED != 0 {
                    return Err(self.abort(keep_waiting));
                }
                self.regs.read32(CLEAR_STOP);
                return Ok(());
            }
            if !keep_waiting() {
                let _ = self.set_enabled(false, keep_waiting);
                return Err(I2cError::TimedOut);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};

    const TARGET_ADDRESS: u16 = 0x2c;
    const NOT_ACKNOWLEDGED_7: u32 = 1;

    #[derive(Debug, Default)]
    struct Simulated {
        component_type: Cell<u32>,
        enabled: Cell<u32>,
        target: Cell<u32>,
        memory: RefCell<Vec<u8>>,
        pointer: Cell<usize>,
        pointer_set: Cell<bool>,
        received: RefCell<VecDeque<u8>>,
        aborted: Cell<bool>,
        commands: RefCell<Vec<u32>>,
        others: RefCell<HashMap<usize, u32>>,
    }

    impl Simulated {
        fn new() -> Self {
            Self {
                component_type: Cell::new(DESIGNWARE_TYPE),
                memory: RefCell::new(vec![0; 256]),
                ..Self::default()
            }
        }

        fn command(&self, word: u32) {
            self.commands.borrow_mut().push(word);
            if self.target.get() & 0x3ff != u32::from(TARGET_ADDRESS) {
                self.aborted.set(true);
                return;
            }
            let mut memory = self.memory.borrow_mut();
            match (word & READ != 0, self.pointer_set.get()) {
                (true, _) => {
                    let at = self.pointer.get();
                    self.received.borrow_mut().push_back(memory[at % 256]);
                    self.pointer.set(at + 1);
                }
                (false, false) => {
                    self.pointer.set(usize::from(word as u8));
                    self.pointer_set.set(true);
                }
                (false, true) => {
                    let at = self.pointer.get();
                    memory[at % 256] = word as u8;
                    self.pointer.set(at + 1);
                }
            }
            if word & STOP != 0 {
                self.pointer_set.set(false);
            }
        }
    }

    impl RegisterIo for &Simulated {
        fn read8(&self, offset: usize) -> u8 {
            self.read32(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.write32(offset, value.into());
        }
        fn read16(&self, offset: usize) -> u16 {
            self.read32(offset) as u16
        }
        fn write16(&self, offset: usize, value: u16) {
            self.write32(offset, value.into());
        }
        fn read32(&self, offset: usize) -> u32 {
            match offset {
                COMPONENT_TYPE => self.component_type.get(),
                COMPONENT_PARAMETERS => (3 << 16) | (3 << 8),
                ENABLE_STATUS => self.enabled.get(),
                RAW_INTERRUPTS => match self.aborted.get() {
                    true => ABORTED,
                    false => 0,
                },
                ABORT_SOURCE => NOT_ACKNOWLEDGED_7,
                CLEAR_ABORT => {
                    self.aborted.set(false);
                    0
                }
                STATUS => TRANSMIT_EMPTY,
                TRANSMIT_LEVEL => 0,
                RECEIVE_LEVEL => self.received.borrow().len() as u32,
                DATA_COMMAND => u32::from(self.received.borrow_mut().pop_front().unwrap_or(0)),
                _ => self.others.borrow().get(&offset).copied().unwrap_or(0),
            }
        }
        fn write32(&self, offset: usize, value: u32) {
            match offset {
                ENABLE => self.enabled.set(value & ENABLED),
                TARGET => self.target.set(value),
                DATA_COMMAND => self.command(value),
                _ => {
                    self.others.borrow_mut().insert(offset, value);
                }
            }
        }
    }

    fn polls(mut count: u32) -> impl FnMut() -> bool {
        move || {
            count = count.saturating_sub(1);
            count > 0
        }
    }

    fn controller(simulated: &Simulated) -> DesignWare<&Simulated> {
        DesignWare::new(simulated, 120_000_000, Speed::Fast, &mut polls(10)).unwrap()
    }

    #[test]
    fn a_read_writes_the_register_then_reads_after_a_repeated_start_and_stops() {
        let simulated = Simulated::new();
        simulated.memory.borrow_mut()[0x10..0x13].copy_from_slice(&[1, 2, 3]);
        let mut bus = controller(&simulated);
        let mut read = [0u8; 3];
        bus.transfer(TARGET_ADDRESS, false, &[0x10], &mut read, &mut polls(100))
            .unwrap();
        assert_eq!(read, [1, 2, 3]);
        assert_eq!(
            *simulated.commands.borrow(),
            vec![0x10, READ | RESTART, READ, READ | STOP]
        );
    }

    #[test]
    fn a_write_alone_ends_with_a_stop_on_its_last_byte() {
        let simulated = Simulated::new();
        let mut bus = controller(&simulated);
        bus.transfer(TARGET_ADDRESS, false, &[0x05, 0xaa, 0xbb], &mut [], &mut polls(100))
            .unwrap();
        assert_eq!(simulated.memory.borrow()[5..7], [0xaa, 0xbb]);
        assert_eq!(simulated.commands.borrow().last(), Some(&(0xbb | STOP)));
    }

    #[test]
    fn a_target_that_does_not_answer_aborts_the_transfer() {
        let simulated = Simulated::new();
        let mut bus = controller(&simulated);
        let outcome = bus.transfer(0x50, false, &[0], &mut [0; 2], &mut polls(100));
        assert_eq!(outcome, Err(I2cError::NotAcknowledged(NOT_ACKNOWLEDGED_7)));
        assert_eq!(simulated.enabled.get(), 0);
    }

    #[test]
    fn empty_transfers_and_addresses_past_seven_bits_are_refused() {
        let simulated = Simulated::new();
        let mut bus = controller(&simulated);
        assert_eq!(
            bus.transfer(TARGET_ADDRESS, false, &[], &mut [], &mut polls(10)),
            Err(I2cError::Invalid)
        );
        assert_eq!(
            bus.transfer(0x80, false, &[0], &mut [], &mut polls(10)),
            Err(I2cError::Invalid)
        );
    }

    #[test]
    fn other_controllers_are_refused() {
        let simulated = Simulated::new();
        simulated.component_type.set(0);
        let outcome = DesignWare::new(&simulated, 120_000_000, Speed::Fast, &mut polls(10));
        assert_eq!(outcome.err(), Some(I2cError::NotThisController));
    }

    #[test]
    fn the_clock_counts_are_the_firmware_s_unless_it_left_none() {
        let simulated = Simulated::new();
        controller(&simulated);
        let others = simulated.others.borrow();
        assert_eq!(others.get(&FAST_HIGH_COUNT), Some(&88));
        assert_eq!(others.get(&FAST_LOW_COUNT), Some(&191));
        drop(others);
        let kept = Simulated::new();
        kept.others.borrow_mut().insert(FAST_HIGH_COUNT, 60);
        kept.others.borrow_mut().insert(FAST_LOW_COUNT, 160);
        controller(&kept);
        assert_eq!(kept.others.borrow().get(&FAST_HIGH_COUNT), Some(&60));
        assert_eq!(
            kept.others.borrow().get(&CONTROL),
            Some(&(MASTER | SPEED_FAST | RESTART_ENABLED | TARGET_DISABLED))
        );
    }
}

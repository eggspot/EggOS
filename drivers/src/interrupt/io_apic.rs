//! x86 I/O APIC (Intel 82093AA datasheet): routes the machine's device interrupt lines, global
//! system interrupts (GSIs) from the ACPI MADT, to local APICs as vectors.

use crate::io::RegisterIo;

const REGISTER_SELECT: usize = 0x00;
const REGISTER_WINDOW: usize = 0x10;
const VERSION: u32 = 0x01;
const FIRST_REDIRECTION: u32 = 0x10;
const MAX_REDIRECTION_SHIFT: u32 = 16;

const REDIRECTION_ACTIVE_LOW: u32 = 1 << 13;
const REDIRECTION_LEVEL_TRIGGERED: u32 = 1 << 15;
const REDIRECTION_MASKED: u32 = 1 << 16;
const DESTINATION_SHIFT: u32 = 24;

/// Size of the I/O APIC's register block in bytes.
pub const REGISTER_BLOCK_SIZE: usize = 0x20;

/// How an interrupt line signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSignal {
    /// Asserted low, as PCI lines are; ISA lines are high.
    pub active_low: bool,
    /// Pending while asserted, as PCI lines are; ISA lines interrupt on an edge.
    pub level_triggered: bool,
}

/// The ISA bus's signalling: active high, edge triggered.
pub const ISA: LineSignal = LineSignal {
    active_low: false,
    level_triggered: false,
};

/// One I/O APIC.
#[derive(Debug)]
pub struct IoApic<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> IoApic<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn read(&self, register: u32) -> u32 {
        self.regs.write32(REGISTER_SELECT, register);
        self.regs.read32(REGISTER_WINDOW)
    }

    fn write(&self, register: u32, value: u32) {
        self.regs.write32(REGISTER_SELECT, register);
        self.regs.write32(REGISTER_WINDOW, value);
    }

    /// How many lines (redirection entries) this I/O APIC has.
    pub fn lines(&self) -> u32 {
        (self.read(VERSION) >> MAX_REDIRECTION_SHIFT & 0xff) + 1
    }

    /// Sends line `line` (0-based on this I/O APIC), signalling as `signal` says, to the local
    /// APIC with id `destination` as fixed interrupt `vector`, and unmasks it. The destination
    /// is written while the line is masked, so no interrupt goes to a half-written entry.
    pub fn route(&self, line: u32, signal: LineSignal, vector: u8, destination: u8) {
        assert!(line < self.lines(), "I/O APIC line {line} out of range");
        let low = FIRST_REDIRECTION + line * 2;
        let mut entry = u32::from(vector);
        if signal.active_low {
            entry |= REDIRECTION_ACTIVE_LOW;
        }
        if signal.level_triggered {
            entry |= REDIRECTION_LEVEL_TRIGGERED;
        }
        self.write(low, REDIRECTION_MASKED);
        self.write(low + 1, u32::from(destination) << DESTINATION_SHIFT);
        self.write(low, entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;

    const TWENTY_FOUR_LINES: u32 = 23 << 16 | 0x20;

    #[test]
    fn counts_its_lines_from_the_version_register() {
        let apic = IoApic::new(FakeRegs::default());
        apic.regs
            .script_reads(REGISTER_WINDOW, &[TWENTY_FOUR_LINES]);
        assert_eq!(apic.lines(), 24);
        assert_eq!(apic.regs.writes(), vec![(REGISTER_SELECT, VERSION)]);
    }

    #[test]
    fn routes_an_isa_line_masked_first_then_destination_then_vector() {
        let apic = IoApic::new(FakeRegs::default());
        apic.regs
            .script_reads(REGISTER_WINDOW, &[TWENTY_FOUR_LINES]);
        apic.route(4, ISA, 0x32, 3);
        assert_eq!(
            apic.regs.writes()[1..],
            [
                (REGISTER_SELECT, 0x18),
                (REGISTER_WINDOW, REDIRECTION_MASKED),
                (REGISTER_SELECT, 0x19),
                (REGISTER_WINDOW, 3 << 24),
                (REGISTER_SELECT, 0x18),
                (REGISTER_WINDOW, 0x32),
            ]
        );
    }

    #[test]
    fn a_level_triggered_active_low_line_sets_both_bits() {
        let apic = IoApic::new(FakeRegs::default());
        apic.regs
            .script_reads(REGISTER_WINDOW, &[TWENTY_FOUR_LINES]);
        let pci = LineSignal {
            active_low: true,
            level_triggered: true,
        };
        apic.route(16, pci, 0x40, 0);
        assert_eq!(
            apic.regs.writes().last(),
            Some(&(REGISTER_WINDOW, 0x40 | 1 << 13 | 1 << 15))
        );
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_line_beyond_the_last_is_refused() {
        let apic = IoApic::new(FakeRegs::default());
        apic.regs
            .script_reads(REGISTER_WINDOW, &[TWENTY_FOUR_LINES]);
        apic.route(24, ISA, 0x32, 0);
    }
}

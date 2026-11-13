//! The RISC-V Advanced Platform-Level Interrupt Controller (APLIC) of the Advanced Interrupt
//! Architecture, in MSI delivery mode (D532): each wired source, once given its mode, a target
//! hart and an identity, and enabled, becomes a message to that hart's IMSIC, which the hart
//! claims through its `stopei` CSR. The supervisor-level domain's message addresses are set by
//! the machine-level firmware, which also delegates the sources to it. Register places are the
//! RISC-V AIA specification's (chapter 4).

use crate::io::RegisterIo;

/// The bytes of an APLIC domain's register block (without per-hart interrupt delivery
/// control, which MSI mode does not use).
pub const REGISTER_BLOCK_SIZE: usize = 0x4000;
/// The highest source an APLIC has.
pub const MOST_SOURCES: u32 = 1023;

const DOMAIN_CONFIGURATION: usize = 0x0000;
const SOURCE_CONFIGURATIONS: usize = 0x0004;
const SET_ENABLE_NUMBER: usize = 0x1edc;
const CLEAR_ENABLE_NUMBER: usize = 0x1fdc;
const TARGETS: usize = 0x3004;
const INTERRUPTS_ENABLED: u32 = 1 << 8;
const MSI_DELIVERY: u32 = 1 << 2;
const HART_INDEX_SHIFT: u32 = 18;
const IDENTITY_MASK: u32 = 0x7ff;
const EDGE_RISING_TYPE: u32 = 1;
const EDGE_FALLING_TYPE: u32 = 2;
const LEVEL_LOW_TYPE: u32 = 8;

/// How a source's wire signals: its `sourcecfg` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    EdgeRising = 4,
    EdgeFalling = 5,
    LevelHigh = 6,
    LevelLow = 7,
}

/// The source and mode of an interrupt specifier in an APLIC's two cells (`riscv,aplic`: the
/// source, then Linux's interrupt type flags).
pub fn source_of_specifier(cells: &[u32]) -> Result<(u32, SourceMode), &'static str> {
    let source = *cells.first().ok_or("an APLIC specifier has no source")?;
    if source == 0 || source > MOST_SOURCES {
        return Err("an APLIC source past its range");
    }
    let mode = match cells.get(1).copied().unwrap_or(0) {
        EDGE_RISING_TYPE => SourceMode::EdgeRising,
        EDGE_FALLING_TYPE => SourceMode::EdgeFalling,
        LEVEL_LOW_TYPE => SourceMode::LevelLow,
        _ => SourceMode::LevelHigh,
    };
    Ok((source, mode))
}

/// One APLIC domain, through its registers in `regs`.
pub struct Aplic<R: RegisterIo> {
    regs: R,
}

impl<R: RegisterIo> Aplic<R> {
    pub const fn new(regs: R) -> Self {
        Self { regs }
    }

    fn per_source(base: usize, source: u32) -> usize {
        base + 4 * (source as usize - 1)
    }

    /// Turns the domain's interrupts on, delivered as MSIs.
    pub fn enable_msi_delivery(&self) {
        self.regs
            .write32(DOMAIN_CONFIGURATION, INTERRUPTS_ENABLED | MSI_DELIVERY);
    }

    /// Makes `source` (1 to [`MOST_SOURCES`]) signal in `mode` and arrive at hart index
    /// `hart` as identity `identity`, and enables it.
    pub fn route(&self, source: u32, mode: SourceMode, hart: u32, identity: u32) {
        self.regs
            .write32(Self::per_source(SOURCE_CONFIGURATIONS, source), mode as u32);
        self.regs.write32(
            Self::per_source(TARGETS, source),
            hart << HART_INDEX_SHIFT | identity & IDENTITY_MASK,
        );
        self.regs.write32(SET_ENABLE_NUMBER, source);
    }

    /// Has `source` arrive at hart index `hart` as identity `identity` from now on, leaving its
    /// mode and enable alone (D536).
    pub fn set_target(&self, source: u32, hart: u32, identity: u32) {
        self.regs.write32(
            Self::per_source(TARGETS, source),
            hart << HART_INDEX_SHIFT | identity & IDENTITY_MASK,
        );
    }

    /// Stops `source` from interrupting.
    pub fn disable(&self, source: u32) {
        self.regs.write32(CLEAR_ENABLE_NUMBER, source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl RegisterIo for &Registers {
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
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
    }

    #[test]
    fn a_routed_source_gets_its_mode_target_and_enable() {
        let regs = Registers::default();
        let aplic = Aplic::new(&regs);
        aplic.enable_msi_delivery();
        aplic.route(10, SourceMode::LevelHigh, 2, 10);
        assert_eq!((&regs).read32(DOMAIN_CONFIGURATION), 1 << 8 | 1 << 2);
        assert_eq!((&regs).read32(0x0004 + 4 * 9), 6);
        assert_eq!((&regs).read32(0x3004 + 4 * 9), 2 << 18 | 10);
        assert_eq!((&regs).read32(SET_ENABLE_NUMBER), 10);
        aplic.set_target(10, 3, 10);
        assert_eq!((&regs).read32(0x3004 + 4 * 9), 3 << 18 | 10);
        assert_eq!((&regs).read32(0x0004 + 4 * 9), 6);
        aplic.disable(10);
        assert_eq!((&regs).read32(CLEAR_ENABLE_NUMBER), 10);
    }

    #[test]
    fn specifiers_give_the_source_and_its_mode() {
        assert_eq!(
            source_of_specifier(&[33, 4]),
            Ok((33, SourceMode::LevelHigh))
        );
        assert_eq!(
            source_of_specifier(&[7, 1]),
            Ok((7, SourceMode::EdgeRising))
        );
        assert!(source_of_specifier(&[0, 4]).is_err());
    }
}

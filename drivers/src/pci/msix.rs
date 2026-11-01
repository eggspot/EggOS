//! MSI-X (PCI Local Bus 3.0, 6.8.2): a function's table of message-signalled interrupts in one
//! of its BARs, each entry an address, the data written there and a mask bit, and the
//! capability that says where the table is and turns MSI-X on. With MSI-X on, the function no
//! longer asserts its legacy interrupt pin.

use super::{Address, ConfigSpace, capabilities};
use crate::io::RegisterIo;

/// The MSI-X capability's id.
pub const CAPABILITY_ID: u8 = 0x11;
/// Bytes of one table entry.
pub const ENTRY_BYTES: usize = 16;

const TABLE: u16 = 4;
const MESSAGE_CONTROL_SHIFT: u32 = 16;
const TABLE_SIZE_MASK: u32 = 0x7ff;
const FUNCTION_MASK: u32 = 1 << 14;
const ENABLE: u32 = 1 << 15;
const BAR_INDICATOR_MASK: u32 = 0x7;
const MAX_BAR: u8 = 5;
const ENTRY_ADDRESS_LOW: usize = 0;
const ENTRY_ADDRESS_HIGH: usize = 4;
const ENTRY_DATA: usize = 8;
const ENTRY_VECTOR_CONTROL: usize = 12;
const VECTOR_MASKED: u32 = 1;

/// A function's MSI-X capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Msix {
    /// Where the capability starts in configuration space.
    pub offset: u8,
    /// How many entries the table has (1 to 2048).
    pub entries: u16,
    /// The BAR (0-5) that holds the table.
    pub bar: u8,
    /// The table's offset in that BAR.
    pub table_offset: u32,
}

/// The MSI-X capability of the function at `address`, if it has one whose table is in a BAR.
pub fn find<C: ConfigSpace + ?Sized>(config: &C, address: Address) -> Option<Msix> {
    let capability = capabilities(config, address).find(|found| found.id == CAPABILITY_ID)?;
    let at = u16::from(capability.offset);
    let control = config.read32(address, at) >> MESSAGE_CONTROL_SHIFT;
    let table = config.read32(address, at + TABLE);
    let bar = (table & BAR_INDICATOR_MASK) as u8;
    (bar <= MAX_BAR).then_some(Msix {
        offset: capability.offset,
        entries: ((control & TABLE_SIZE_MASK) + 1) as u16,
        bar,
        table_offset: table & !BAR_INDICATOR_MASK,
    })
}

impl Msix {
    /// Bytes of the whole table.
    pub fn table_bytes(&self) -> usize {
        usize::from(self.entries) * ENTRY_BYTES
    }

    /// Turns MSI-X on for the function at `address`, each entry's own mask bit deciding whether
    /// it signals: program the entries ([`MsixTable::set`]) before or after.
    pub fn enable<C: ConfigSpace + ?Sized>(&self, config: &C, address: Address) {
        let at = u16::from(self.offset);
        let header = config.read32(address, at);
        let control = header >> MESSAGE_CONTROL_SHIFT & !FUNCTION_MASK | ENABLE;
        config.write32(
            address,
            at,
            control << MESSAGE_CONTROL_SHIFT | header & 0xffff,
        );
    }
}

/// A function's MSI-X table, mapped.
#[derive(Debug)]
pub struct MsixTable<R: RegisterIo> {
    regs: R,
    entries: u16,
}

impl<R: RegisterIo> MsixTable<R> {
    /// The table of `entries` entries whose registers `regs` reaches from its first entry.
    pub fn new(regs: R, entries: u16) -> Self {
        Self { regs, entries }
    }

    /// Makes entry `entry` write `data` to `address`, and unmasks it; masked while it changes, so
    /// the function never sends half a message. False, and nothing written, for an entry the
    /// table does not have.
    pub fn set(&self, entry: u16, address: u64, data: u32) -> bool {
        if entry >= self.entries {
            return false;
        }
        let at = usize::from(entry) * ENTRY_BYTES;
        self.regs.write32(at + ENTRY_VECTOR_CONTROL, VECTOR_MASKED);
        self.regs.write32(at + ENTRY_ADDRESS_LOW, address as u32);
        self.regs
            .write32(at + ENTRY_ADDRESS_HIGH, (address >> 32) as u32);
        self.regs.write32(at + ENTRY_DATA, data);
        self.regs.write32(at + ENTRY_VECTOR_CONTROL, 0);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::fake::FakeRegs;
    use crate::pci::fake::FakeConfig;
    use crate::pci::{CAPABILITIES_POINTER, COMMAND, STATUS_CAPABILITY_LIST};

    fn function() -> Address {
        Address::new(0, 3, 0).unwrap()
    }

    fn with_msix(control: u32, table: u32) -> FakeConfig {
        let config = FakeConfig::default();
        config.function(function(), (0x1af4, 0x1052), (0x09, 0x80), 0);
        config.set(function(), COMMAND, u32::from(STATUS_CAPABILITY_LIST) << 16);
        config.set(function(), CAPABILITIES_POINTER, 0x98);
        config.set(function(), 0x98, 0x84 << 8 | 0x11 | control << 16);
        config.set(function(), 0x9c, table);
        config.set(function(), 0x84, 0x09);
        config
    }

    #[test]
    fn finds_qemus_virtio_table_in_bar_4() {
        let config = with_msix(1, 0x3004);
        assert_eq!(
            find(&config, function()),
            Some(Msix {
                offset: 0x98,
                entries: 2,
                bar: 4,
                table_offset: 0x3000,
            })
        );
        assert_eq!(find(&config, function()).unwrap().table_bytes(), 32);
    }

    #[test]
    fn a_function_without_msix_or_with_its_table_beyond_bar_5_has_none() {
        let config = with_msix(1, 0x3006);
        assert_eq!(find(&config, function()), None);
        let config = with_msix(1, 0x3004);
        config.set(function(), 0x98, 0x05);
        assert_eq!(find(&config, function()), None);
    }

    #[test]
    fn enabling_sets_the_enable_bit_and_clears_the_function_mask() {
        let config = with_msix(FUNCTION_MASK | 1, 0x3004);
        let msix = find(&config, function()).unwrap();
        msix.enable(&config, function());
        let header = config.get(function(), 0x98);
        assert_eq!(header >> 16, ENABLE | 1);
        assert_eq!(header & 0xffff, 0x84 << 8 | 0x11);
    }

    #[test]
    fn an_entry_is_masked_written_then_unmasked() {
        let table = MsixTable::new(FakeRegs::default(), 2);
        assert!(table.set(1, 0xfee0_1000, 0x34));
        assert_eq!(
            table.regs.writes(),
            vec![
                (16 + 12, 1),
                (16, 0xfee0_1000),
                (16 + 4, 0),
                (16 + 8, 0x34),
                (16 + 12, 0),
            ]
        );
        assert!(!table.set(2, 0xfee0_0000, 0x33));
        assert_eq!(table.regs.writes().len(), 5);
    }
}

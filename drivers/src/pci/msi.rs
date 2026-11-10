//! Message-signalled interrupts (PCI Local Bus 3.0, 6.8.1): the MSI capability, for functions
//! without MSI-X (AHCI controllers, many network adapters; D237). One vector per function.

use super::{Address, ConfigSpace, capabilities};

pub const CAPABILITY_ID: u8 = 0x05;

const MESSAGE_CONTROL_SHIFT: u32 = 16;
const ENABLE: u32 = 1 << 0;
const MULTIPLE_MESSAGE_ENABLE_MASK: u32 = 0b111 << 4;
const ADDRESS_64_BIT: u32 = 1 << 7;
const PER_VECTOR_MASKING: u32 = 1 << 8;
const ADDRESS_LOW: u16 = 4;
const ADDRESS_HIGH: u16 = 8;
const DATA_32: u16 = 8;
const DATA_64: u16 = 12;
const MASK_32: u16 = 12;
const MASK_64: u16 = 16;

/// A function's MSI capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Msi {
    pub offset: u8,
    pub is_64_bit: bool,
    pub per_vector_masking: bool,
}

/// The MSI capability of the function at `address`, if it has one.
pub fn find<C: ConfigSpace + ?Sized>(config: &C, address: Address) -> Option<Msi> {
    let capability = capabilities(config, address).find(|found| found.id == CAPABILITY_ID)?;
    let control = config.read32(address, u16::from(capability.offset)) >> MESSAGE_CONTROL_SHIFT;
    Some(Msi {
        offset: capability.offset,
        is_64_bit: control & ADDRESS_64_BIT != 0,
        per_vector_masking: control & PER_VECTOR_MASKING != 0,
    })
}

impl Msi {
    /// Programs one message, `data` written to `message_address`, and turns MSI on with one
    /// vector. A 32-bit capability takes only an address below 4 GiB: `false` otherwise.
    pub fn enable<C: ConfigSpace + ?Sized>(
        &self,
        config: &C,
        address: Address,
        message_address: u64,
        data: u16,
    ) -> bool {
        let at = u16::from(self.offset);
        if !self.is_64_bit && message_address >> 32 != 0 {
            return false;
        }
        config.write32(address, at + ADDRESS_LOW, message_address as u32);
        let data_at = if self.is_64_bit {
            config.write32(address, at + ADDRESS_HIGH, (message_address >> 32) as u32);
            DATA_64
        } else {
            DATA_32
        };
        let data_word = config.read32(address, at + data_at) & 0xffff_0000;
        config.write32(address, at + data_at, data_word | u32::from(data));
        if self.per_vector_masking {
            let mask_at = if self.is_64_bit { MASK_64 } else { MASK_32 };
            config.write32(address, at + mask_at, 0);
        }
        let header = config.read32(address, at);
        let control = (header >> MESSAGE_CONTROL_SHIFT) & !MULTIPLE_MESSAGE_ENABLE_MASK | ENABLE;
        config.write32(address, at, control << MESSAGE_CONTROL_SHIFT | header & 0xffff);
        true
    }

    /// Turns MSI off.
    pub fn disable<C: ConfigSpace + ?Sized>(&self, config: &C, address: Address) {
        let at = u16::from(self.offset);
        let header = config.read32(address, at);
        config.write32(address, at, header & !(ENABLE << MESSAGE_CONTROL_SHIFT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pci::fake::FakeConfig;
    use crate::pci::{CAPABILITIES_POINTER, STATUS_CAPABILITY_LIST};

    fn function_with_msi(control: u32) -> (FakeConfig, Address) {
        let config = FakeConfig::default();
        let address = Address::new(0, 3, 0).unwrap();
        config.function(address, (0x8086, 0x10d3), (0x02, 0x00), 0);
        config.set(address, 4, u32::from(STATUS_CAPABILITY_LIST) << 16);
        config.set(address, u16::from(CAPABILITIES_POINTER) & !3, 0x50);
        config.register(address, 0x50, control << 16 | u32::from(CAPABILITY_ID), u32::MAX);
        for offset in [0x54, 0x58, 0x5c, 0x60] {
            config.register(address, offset, 0, u32::MAX);
        }
        (config, address)
    }

    #[test]
    fn a_64_bit_capability_takes_its_address_and_data() {
        let (config, address) = function_with_msi(ADDRESS_64_BIT);
        let msi = find(&config, address).unwrap();
        assert!(msi.is_64_bit);
        assert!(msi.enable(&config, address, 0xfee0_0000, 0x41));
        assert_eq!(config.get(address, 0x54), 0xfee0_0000);
        assert_eq!(config.get(address, 0x5c) & 0xffff, 0x41);
        assert_ne!(config.get(address, 0x50) & (ENABLE << 16), 0);
    }

    #[test]
    fn a_32_bit_capability_refuses_a_high_address() {
        let (config, address) = function_with_msi(0);
        let msi = find(&config, address).unwrap();
        assert!(!msi.enable(&config, address, 1 << 32, 0x41));
    }
}

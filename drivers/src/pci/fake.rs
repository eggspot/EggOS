use super::{Address, COMMAND, ConfigSpace};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct FakeConfig {
    pub functions: RefCell<HashMap<Address, Vec<u32>>>,
    pub writable: RefCell<HashMap<(Address, u16), u32>>,
    pub writes: RefCell<Vec<(Address, u16, u32)>>,
}

impl FakeConfig {
    pub fn function(
        &self,
        address: Address,
        vendor_device: (u16, u16),
        class: (u8, u8),
        header_type: u8,
    ) {
        let mut words = vec![0u32; 1024];
        words[0] = u32::from(vendor_device.1) << 16 | u32::from(vendor_device.0);
        words[2] = u32::from(class.0) << 24 | u32::from(class.1) << 16;
        words[3] = u32::from(header_type) << 16;
        self.functions.borrow_mut().insert(address, words);
    }

    pub fn set(&self, address: Address, offset: u16, value: u32) {
        self.functions.borrow_mut().get_mut(&address).unwrap()[usize::from(offset / 4)] = value;
    }

    pub fn get(&self, address: Address, offset: u16) -> u32 {
        self.functions.borrow()[&address][usize::from(offset / 4)]
    }

    pub fn register(&self, address: Address, offset: u16, value: u32, writable: u32) {
        self.set(address, offset, value);
        self.writable
            .borrow_mut()
            .insert((address, offset), writable);
    }
}

impl ConfigSpace for FakeConfig {
    fn read32(&self, address: Address, offset: u16) -> u32 {
        self.functions
            .borrow()
            .get(&address)
            .map_or(u32::MAX, |words| words[usize::from(offset / 4)])
    }

    fn write32(&self, address: Address, offset: u16, value: u32) {
        self.writes.borrow_mut().push((address, offset, value));
        let writable = self.writable.borrow().get(&(address, offset)).copied();
        let mut functions = self.functions.borrow_mut();
        let Some(words) = functions.get_mut(&address) else {
            return;
        };
        let word = &mut words[usize::from(offset / 4)];
        *word = if offset == COMMAND {
            let status = (*word >> 16) & !(value >> 16);
            status << 16 | value & 0xffff
        } else {
            let unimplemented_bar = (0x10..0x28).contains(&offset);
            let writable = writable.unwrap_or(if unimplemented_bar { 0 } else { u32::MAX });
            value & writable | *word & !writable
        };
    }
}

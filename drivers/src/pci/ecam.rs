//! The PCI Express Enhanced Configuration Access Mechanism: every function's 4 KiB of
//! configuration space at a fixed place in one memory window, 1 MiB per bus.

use super::{Address, ConfigSpace};

/// Where `offset` of `address`'s configuration space is in an ECAM window whose first bus is
/// `start_bus`: bus, device and function select 1 MiB, 32 KiB and 4 KiB blocks.
pub const fn ecam_offset(address: Address, start_bus: u8, offset: u16) -> usize {
    (address.bus.wrapping_sub(start_bus) as usize) << 20
        | (address.device as usize) << 15
        | (address.function as usize) << 12
        | offset as usize
}

/// Bytes of an ECAM window for buses `start_bus..=end_bus`.
pub const fn ecam_length(start_bus: u8, end_bus: u8) -> u64 {
    (end_bus as u64 + 1).saturating_sub(start_bus as u64) << 20
}

/// Configuration space through a mapped ECAM window. Accesses outside it (a bus out of range,
/// an offset past 4 KiB or unaligned, a device or function number too large) read all ones and
/// write nothing, as an absent function does.
#[derive(Debug)]
pub struct Ecam {
    base: usize,
    start_bus: u8,
    end_bus: u8,
}

impl Ecam {
    /// Access through the window at virtual address `base` for buses `start_bus..=end_bus`.
    ///
    /// # Safety
    /// `base..base + ecam_length(start_bus, end_bus)` must be the ECAM window of those buses,
    /// mapped as device memory for as long as the value is used.
    pub const unsafe fn new(base: usize, start_bus: u8, end_bus: u8) -> Self {
        Self {
            base,
            start_bus,
            end_bus,
        }
    }

    fn register(&self, address: Address, offset: u16) -> Option<*mut u32> {
        let inside = (self.start_bus..=self.end_bus).contains(&address.bus)
            && address.device < 32
            && address.function < 8
            && offset < 4096
            && offset.is_multiple_of(4);
        inside.then(|| (self.base + ecam_offset(address, self.start_bus, offset)) as *mut u32)
    }
}

impl ConfigSpace for Ecam {
    fn read32(&self, address: Address, offset: u16) -> u32 {
        match self.register(address, offset) {
            // SAFETY: `register` checked the access lies inside the window, which `new`'s
            // contract keeps mapped, and is aligned.
            Some(register) => unsafe { register.read_volatile() },
            None => u32::MAX,
        }
    }

    fn write32(&self, address: Address, offset: u16, value: u32) {
        if let Some(register) = self.register(address, offset) {
            // SAFETY: as in `read32`.
            unsafe { register.write_volatile(value) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(bus: u8, device: u8, function: u8) -> Address {
        Address::new(bus, device, function).unwrap()
    }

    #[test]
    fn bus_device_function_and_offset_have_their_own_bits() {
        assert_eq!(ecam_offset(at(0, 0, 0), 0, 0), 0);
        assert_eq!(ecam_offset(at(1, 0, 0), 0, 0), 1 << 20);
        assert_eq!(ecam_offset(at(0, 31, 0), 0, 0), 31 << 15);
        assert_eq!(ecam_offset(at(0, 0, 7), 0, 0), 7 << 12);
        assert_eq!(ecam_offset(at(0, 0, 0), 0, 0xffc), 0xffc);
        assert_eq!(
            ecam_offset(at(0x12, 3, 2), 0x10, 0x40),
            2 << 20 | 3 << 15 | 2 << 12 | 0x40
        );
    }

    #[test]
    fn the_window_holds_one_mebibyte_per_bus() {
        assert_eq!(ecam_length(0, 255), 256 << 20);
        assert_eq!(ecam_length(0, 0), 1 << 20);
        assert_eq!(ecam_length(0x10, 0x1f), 16 << 20);
        assert_eq!(ecam_length(5, 4), 0);
    }

    #[test]
    fn reads_and_writes_go_to_the_window_and_never_outside_it() {
        let mut window = vec![0u32; (1 << 20) / 4];
        window[(3 << 15 | 1 << 12 | 8) / 4] = 0x0200_0001;
        let mut expected = window.clone();
        expected[0xffc / 4] = 0x1234_5678;
        {
            // SAFETY: `window` is a 1 MiB host buffer standing in for bus 4's window, alive and
            // only reached through `ecam` in this block.
            let ecam = unsafe { Ecam::new(window.as_mut_ptr() as usize, 4, 4) };
            check_window(&ecam);
            ecam.write32(at(5, 0, 0), 0, 1);
            ecam.write32(at(4, 0, 0), 0x1000, 1);
            ecam.write32(BAD_DEVICE, 0, 1);
        }
        assert_eq!(window, expected);
    }

    const BAD_DEVICE: Address = Address {
        bus: 4,
        device: 32,
        function: 0,
    };

    fn check_window(ecam: &Ecam) {
        assert_eq!(ecam.read32(at(4, 3, 1), 8), 0x0200_0001);
        assert_eq!(ecam.read16(at(4, 3, 1), 0xa), 0x0200);
        ecam.write32(at(4, 0, 0), 0xffc, 0x1234_5678);
        assert_eq!(ecam.read32(at(4, 0, 0), 0xffc), 0x1234_5678);
        assert_eq!(ecam.read32(at(5, 0, 0), 0), u32::MAX);
        assert_eq!(ecam.read32(at(3, 0, 0), 0), u32::MAX);
        assert_eq!(ecam.read32(at(4, 0, 0), 0x1000), u32::MAX);
        assert_eq!(ecam.read32(at(4, 0, 0), 2), u32::MAX);
        assert_eq!(ecam.read32(BAD_DEVICE, 0), u32::MAX);
    }
}

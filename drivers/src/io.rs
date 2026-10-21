//! Register access for drivers, independent of how the bus reaches the device.

use core::ptr::NonNull;

/// A bank of 8-bit device registers addressed by offset from the bank's base.
///
/// Implemented for memory-mapped registers here ([`Mmio`]) and for bus types
/// that only exist on some architectures (such as x86 I/O ports) in the HAL.
pub trait ByteRegisters {
    /// Read the register at `offset`.
    fn read8(&self, offset: usize) -> u8;

    /// Write `value` to the register at `offset`.
    fn write8(&self, offset: usize, value: u8);
}

/// A memory-mapped register bank.
#[derive(Debug)]
pub struct Mmio {
    base: NonNull<u8>,
}

impl Mmio {
    /// Create a register bank at `base`.
    ///
    /// # Safety
    ///
    /// `base` must be the address of a device register bank that is mapped and
    /// large enough for every offset a driver will use, and nothing else may
    /// treat that memory as ordinary RAM for as long as this value lives.
    pub const unsafe fn new(base: NonNull<u8>) -> Self {
        Self { base }
    }

    /// Read the 32-bit register at `offset`, which must be 4-byte aligned.
    pub fn read32(&self, offset: usize) -> u32 {
        debug_assert!(offset.is_multiple_of(4), "unaligned 32-bit register offset");
        // SAFETY: `Mmio::new`'s contract guarantees the register bank is mapped
        // device memory covering `offset`; volatile keeps the access exactly as
        // written, which device registers need.
        unsafe { self.base.add(offset).cast::<u32>().read_volatile() }
    }

    /// Write `value` to the 32-bit register at `offset`, which must be 4-byte
    /// aligned.
    pub fn write32(&self, offset: usize, value: u32) {
        debug_assert!(offset.is_multiple_of(4), "unaligned 32-bit register offset");
        // SAFETY: as in `read32`.
        unsafe { self.base.add(offset).cast::<u32>().write_volatile(value) }
    }
}

impl ByteRegisters for Mmio {
    fn read8(&self, offset: usize) -> u8 {
        // SAFETY: as in `read32`.
        unsafe { self.base.add(offset).read_volatile() }
    }

    fn write8(&self, offset: usize, value: u8) {
        // SAFETY: as in `read32`.
        unsafe { self.base.add(offset).write_volatile(value) }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! An in-memory register bank that records writes, for driver tests.

    use super::ByteRegisters;
    use std::cell::RefCell;

    /// Registers backed by a byte array; every write is also logged.
    pub struct FakeRegisters {
        pub values: RefCell<[u8; 8]>,
        pub writes: RefCell<Vec<(usize, u8)>>,
    }

    impl FakeRegisters {
        pub fn new(values: [u8; 8]) -> Self {
            Self {
                values: RefCell::new(values),
                writes: RefCell::new(Vec::new()),
            }
        }
    }

    impl ByteRegisters for FakeRegisters {
        fn read8(&self, offset: usize) -> u8 {
            self.values.borrow()[offset]
        }

        fn write8(&self, offset: usize, value: u8) {
            self.values.borrow_mut()[offset] = value;
            self.writes.borrow_mut().push((offset, value));
        }
    }
}

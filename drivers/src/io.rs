//! Register access for drivers.
//!
//! Drivers never touch hardware addresses directly; they go through [`RegisterIo`], so one
//! driver works over memory-mapped I/O on every architecture, over x86 port I/O, and against
//! fake registers in host unit tests. Implementations are tiny and `#[inline]`, so after
//! monomorphisation a register access is a single load or store.

/// Access to one device's registers, addressed by byte offset into its register block.
pub trait RegisterIo {
    fn read8(&self, offset: usize) -> u8;
    fn write8(&self, offset: usize, value: u8);
    fn read32(&self, offset: usize) -> u32;
    fn write32(&self, offset: usize, value: u32);
}

/// A memory-mapped register block.
#[derive(Debug)]
pub struct Mmio {
    base: usize,
    size: usize,
}

impl Mmio {
    /// Creates access to the `size`-byte register block at physical/virtual address `base`.
    ///
    /// # Safety
    /// `base..base + size` must be the register block of a device, mapped (or reachable at its
    /// physical address with the MMU off) for as long as the returned value is used, and nothing
    /// else may rely on those registers keeping their state between this value's accesses.
    pub const unsafe fn new(base: usize, size: usize) -> Self {
        Self { base, size }
    }

    /// Checked in release builds too: `RegisterIo` is a safe trait, so an out-of-bounds or
    /// unaligned offset from safe code must panic, never reach the volatile access. The
    /// compares are a few cycles next to a device access that costs hundreds.
    #[inline]
    fn address(&self, offset: usize, width: usize) -> usize {
        assert!(
            offset
                .checked_add(width)
                .is_some_and(|end| end <= self.size),
            "register offset {offset:#x} (+{width}) outside block of {:#x} bytes",
            self.size
        );
        assert!(
            offset.is_multiple_of(width),
            "unaligned register offset {offset:#x}"
        );
        self.base + offset
    }
}

impl RegisterIo for Mmio {
    #[inline]
    fn read8(&self, offset: usize) -> u8 {
        let address = self.address(offset, 1);
        // SAFETY: `Mmio::new`'s contract makes every in-bounds offset a valid device register, and
        // `address` has checked the bounds.
        unsafe { core::ptr::read_volatile(address as *const u8) }
    }

    #[inline]
    fn write8(&self, offset: usize, value: u8) {
        let address = self.address(offset, 1);
        // SAFETY: as in `read8`.
        unsafe { core::ptr::write_volatile(address as *mut u8, value) }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        let address = self.address(offset, 4);
        // SAFETY: `Mmio::new`'s contract makes every in-bounds offset a valid device register,
        // and `address` has checked bounds and 4-byte alignment.
        unsafe { core::ptr::read_volatile(address as *const u32) }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        let address = self.address(offset, 4);
        // SAFETY: as in `read32`.
        unsafe { core::ptr::write_volatile(address as *mut u32, value) }
    }
}

/// Fake registers for host unit tests: records every write, and answers reads from a script.
#[cfg(test)]
pub(crate) mod fake {
    use super::RegisterIo;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Access {
        Read(usize),
        Write(usize, u32),
    }

    #[derive(Debug, Default)]
    pub struct FakeRegs {
        /// Values returned by successive reads of an offset; when exhausted, reads return 0.
        pub reads: RefCell<HashMap<usize, VecDeque<u32>>>,
        pub log: RefCell<Vec<Access>>,
    }

    impl FakeRegs {
        pub fn script_reads(&self, offset: usize, values: &[u32]) {
            self.reads
                .borrow_mut()
                .entry(offset)
                .or_default()
                .extend(values);
        }

        pub fn writes(&self) -> Vec<(usize, u32)> {
            self.log
                .borrow()
                .iter()
                .filter_map(|a| match *a {
                    Access::Write(o, v) => Some((o, v)),
                    Access::Read(_) => None,
                })
                .collect()
        }

        pub fn reads_of(&self, offset: usize) -> usize {
            self.log
                .borrow()
                .iter()
                .filter(|a| **a == Access::Read(offset))
                .count()
        }

        fn read(&self, offset: usize) -> u32 {
            self.log.borrow_mut().push(Access::Read(offset));
            self.reads
                .borrow_mut()
                .get_mut(&offset)
                .and_then(VecDeque::pop_front)
                .unwrap_or(0)
        }

        fn write(&self, offset: usize, value: u32) {
            self.log.borrow_mut().push(Access::Write(offset, value));
        }
    }

    impl RegisterIo for FakeRegs {
        fn read8(&self, offset: usize) -> u8 {
            self.read(offset) as u8
        }
        fn write8(&self, offset: usize, value: u8) {
            self.write(offset, value.into());
        }
        fn read32(&self, offset: usize) -> u32 {
            self.read(offset)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.write(offset, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmio_reads_and_writes_memory_at_offsets() {
        let mut block = [0u32; 4];
        // SAFETY: `block` is a live, aligned 16-byte buffer standing in for a register block,
        // and is only accessed through `regs` until `regs` is dropped.
        let regs = unsafe { Mmio::new(block.as_mut_ptr() as usize, 16) };
        regs.write32(8, 0xdead_beef);
        regs.write8(4, 0x5a);
        assert_eq!(regs.read32(8), 0xdead_beef);
        assert_eq!(regs.read8(4), 0x5a);
        assert_eq!(block[2], 0xdead_beef);
    }

    #[test]
    #[should_panic(expected = "outside block")]
    fn mmio_rejects_out_of_bounds_offset() {
        let mut block = [0u32; 1];
        // SAFETY: as above; the out-of-bounds access is caught before any memory is touched.
        let regs = unsafe { Mmio::new(block.as_mut_ptr() as usize, 4) };
        regs.read32(4);
    }

    #[test]
    #[should_panic(expected = "unaligned")]
    fn mmio_rejects_unaligned_offset() {
        let mut block = [0u32; 2];
        // SAFETY: as above; the unaligned access is caught before any memory is touched.
        let regs = unsafe { Mmio::new(block.as_mut_ptr() as usize, 8) };
        regs.read32(2);
    }
}

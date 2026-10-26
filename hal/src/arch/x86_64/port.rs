//! x86 I/O port access.

use eggos_drivers::io::RegisterIo;

/// A device register block in the x86 I/O port space, for drivers written against
/// [`RegisterIo`] (e.g. the PC serial port).
#[derive(Debug)]
pub struct PortIo {
    base: u16,
    size: usize,
}

impl PortIo {
    /// Creates access to the `size` ports starting at `base`.
    ///
    /// # Safety
    /// `base..base + size` must be one device's ports, and nothing else may rely on those
    /// registers keeping their state between this value's accesses.
    pub const unsafe fn new(base: u16, size: usize) -> Self {
        Self { base, size }
    }

    #[inline]
    fn port(&self, offset: usize, width: usize) -> u16 {
        assert!(
            offset
                .checked_add(width)
                .is_some_and(|end| end <= self.size),
            "port offset {offset:#x} (+{width}) outside block of {:#x} ports",
            self.size
        );
        self.base.wrapping_add(offset as u16)
    }
}

impl RegisterIo for PortIo {
    #[inline]
    fn read8(&self, offset: usize) -> u8 {
        // SAFETY: `PortIo::new`'s contract makes every in-bounds offset one of the device's
        // ports, and `port` has checked the bounds.
        unsafe { in8(self.port(offset, 1)) }
    }

    #[inline]
    fn write8(&self, offset: usize, value: u8) {
        // SAFETY: as in `read8`.
        unsafe { out8(self.port(offset, 1), value) }
    }

    #[inline]
    fn read16(&self, offset: usize) -> u16 {
        // SAFETY: as in `read8`.
        unsafe { in16(self.port(offset, 2)) }
    }

    #[inline]
    fn write16(&self, offset: usize, value: u16) {
        // SAFETY: as in `read8`.
        unsafe { out16(self.port(offset, 2), value) }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: as in `read8`.
        unsafe { in32(self.port(offset, 4)) }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: as in `read8`.
        unsafe { out32(self.port(offset, 4), value) }
    }
}

/// # Safety
/// `port` must be a device register that is safe to read (reads can have side effects).
#[inline]
pub unsafe fn in8(port: u16) -> u8 {
    let value: u8;
    // SAFETY: `in` touches no memory or flags; the caller vouches for the port.
    unsafe {
        core::arch::asm!("in al, dx", out("al") value, in("dx") port,
            options(nomem, nostack, preserves_flags));
    }
    value
}

/// # Safety
/// `port` must be a device register that is safe to write `value` to.
#[inline]
pub unsafe fn out8(port: u16, value: u8) {
    // SAFETY: `out` touches no memory or flags; the caller vouches for the port and value.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value,
            options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// As [`in8`], for a 16-bit register.
#[inline]
pub unsafe fn in16(port: u16) -> u16 {
    let value: u16;
    // SAFETY: as in `in8`.
    unsafe {
        core::arch::asm!("in ax, dx", out("ax") value, in("dx") port,
            options(nomem, nostack, preserves_flags));
    }
    value
}

/// # Safety
/// As [`out8`], for a 16-bit register.
#[inline]
pub unsafe fn out16(port: u16, value: u16) {
    // SAFETY: as in `out8`.
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") value,
            options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// As [`in8`], for a 32-bit register.
#[inline]
pub unsafe fn in32(port: u16) -> u32 {
    let value: u32;
    // SAFETY: as in `in8`.
    unsafe {
        core::arch::asm!("in eax, dx", out("eax") value, in("dx") port,
            options(nomem, nostack, preserves_flags));
    }
    value
}

/// # Safety
/// As [`out8`], for a 32-bit register.
#[inline]
pub unsafe fn out32(port: u16, value: u32) {
    // SAFETY: as in `out8`.
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") value,
            options(nomem, nostack, preserves_flags));
    }
}

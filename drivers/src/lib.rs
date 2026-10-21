//! The EggOS driver model and the drivers written against it.
//!
//! Every driver implements [`Driver`], plus one trait per device class it
//! provides (today only [`CharDevice`]). Drivers never touch hardware directly:
//! they reach their registers through a register-access trait from [`io`], so
//! the same driver serves every architecture and bus that exposes the device
//! (for example the 16550 UART behind x86 I/O ports and behind RISC-V MMIO).
//! Adding a device therefore never touches the kernel core or the HAL's
//! architecture code beyond saying where the device lives.

#![cfg_attr(not(test), no_std)]

pub mod io;
pub mod pl011;
pub mod uart16550;

/// Why a driver could not bring its device up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverError {
    /// The device did not respond the way this driver expects.
    DeviceNotResponding,
}

/// What every driver provides, whatever its device class.
pub trait Driver {
    /// Short, stable driver name for logs and diagnostics, e.g. `"uart16550"`.
    fn name(&self) -> &'static str;

    /// Bring the device into a known working state. Called once, before any
    /// device-class method.
    fn init(&mut self) -> Result<(), DriverError>;
}

/// A device that accepts a stream of bytes, such as a serial port.
pub trait CharDevice: Driver {
    /// Write all of `bytes`, blocking until the device has accepted them.
    fn write_bytes(&self, bytes: &[u8]);
}

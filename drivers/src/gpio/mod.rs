//! GPIO pin controllers (D789): the pins a device's interrupt comes in on when its ACPI
//! resources name a `GpioInt`, as I2C HID touchpads and sensors do. AMD's controller has one
//! register a pin; Intel's has pad configuration registers and status and enable bitmaps per
//! community. Both answer one shared interrupt, which the kernel dispatches to the pins it
//! armed.

pub mod amd;
pub mod intel;

/// How a pin's interrupt is triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    pub edge: bool,
    pub active_low: bool,
    pub both_edges: bool,
}

/// A pin controller as the kernel uses it.
pub trait PinController {
    /// Arms `pin`'s interrupt as `trigger` says; false for a pin it does not have.
    fn arm(&mut self, pin: u16, trigger: Trigger) -> bool;
    /// Whether `pin`'s interrupt is pending.
    fn pending(&mut self, pin: u16) -> bool;
    /// Acknowledges `pin`'s interrupt.
    fn acknowledge(&mut self, pin: u16);
    /// Ends the controller's interrupt once every pending pin was acknowledged.
    fn end_of_interrupt(&mut self) {}
}

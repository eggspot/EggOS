//! Timer devices.

pub mod hpet;
pub mod i8254;

pub use hpet::Hpet;
pub use i8254::Pit;

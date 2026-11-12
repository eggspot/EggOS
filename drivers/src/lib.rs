//! EggOS device drivers.
//!
//! Drivers are architecture-independent: they reach hardware only through register-access
//! traits, so the same driver serves every architecture and bus, and is unit-tested on the
//! host against fake registers.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod apple;
pub mod channel;
pub mod desktop;
pub mod display;
pub mod display_port;
pub mod edid;
pub mod embedded_controller;
pub mod dma;
pub mod ethernet;
pub mod gpu;
pub use eggos_font as font;
pub mod i2c;
pub mod input;
pub mod interrupt;
pub mod io;
pub mod pci;
pub mod rtc;
pub mod serial;
pub mod storage;
pub use eggos_terminal as terminal;
pub mod timer;
pub mod usb;
pub mod virtio;
pub mod wifi;

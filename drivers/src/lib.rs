//! EggOS device drivers.
//!
//! Drivers are architecture-independent: they reach hardware only through register-access
//! traits, so the same driver serves every architecture and bus, and is unit-tested on the
//! host against fake registers.

#![cfg_attr(not(test), no_std)]

pub mod dma;
pub mod interrupt;
pub mod io;
pub mod pci;
pub mod serial;
pub mod timer;
pub mod virtio;

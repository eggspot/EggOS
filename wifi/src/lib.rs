//! Wi-Fi's frames and security, independent of any adapter (D246): IEEE 802.11 management frames
//! as scanning reads them, WPA2-Personal's key hierarchy and four-way handshake as a supplicant,
//! and CCMP's protection of data frames. Adapter drivers move the frames; this crate decides what
//! is in them.

#![cfg_attr(not(test), no_std)]

pub mod frames;
pub mod handshake;
pub mod protection;

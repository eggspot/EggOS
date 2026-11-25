//! Wi-Fi's frames and security, independent of any adapter (D246): IEEE 802.11 management frames
//! as scanning reads them, WPA2-Personal's key hierarchy and four-way handshake as a supplicant,
//! CCMP's protection of data frames, and a client station's authentication, association and
//! data frames (D278). Adapter drivers move the frames; this crate decides what
//! is in them. WPA3-Personal's SAE and its SHA-256 key hierarchy came with D285; WPA2- and
//! WPA3-Enterprise's 802.1X, with EAP-TLS and PEAP over EggOS's TLS client, with D779.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod capabilities;
pub mod client;
pub mod eap;
pub mod enterprise;
pub mod frames;
pub mod handshake;
pub mod kdf;
pub mod management;
pub mod mschapv2;
pub mod peap;
pub mod protection;
pub mod sae;
pub mod sae_h2e;
pub mod tunnel;

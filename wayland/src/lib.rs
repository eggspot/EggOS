//! The Wayland protocol as EggOS speaks it (D140), written from the public specifications
//! `wayland.xml` and `xdg-shell.xml` (MIT): the wire format's codec ([`wire`]), the interfaces,
//! requests, events, errors and enums the Wayland server `eggway` serves ([`protocol`]), the
//! copy of a client's pixels into a window ([`blit`]) and the US keyboard map sent to every
//! client ([`keymap`]). Nothing here allocates, panics on hostile bytes or makes a system call:
//! the server, its test client and the kernel's benchmark all link it.

#![cfg_attr(not(test), no_std)]

pub mod blit;
pub mod keymap;
pub mod protocol;
pub mod wire;

#[cfg(test)]
mod tests;

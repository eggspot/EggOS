//! The Wayland protocol as EggOS speaks it (D140, D172), written from the public
//! specifications `wayland.xml`, `xdg-shell.xml`, `xdg-decoration-unstable-v1.xml`,
//! `cursor-shape-v1.xml` and `viewporter.xml` (MIT): the wire format's codec ([`wire`]), the
//! interfaces, requests, events, errors and enums the Wayland server `eggway` serves
//! ([`protocol`]), the copy of a client's pixels into a window ([`blit`]), where a popup goes
//! ([`positioner`]) and the US keyboard map sent to every client ([`keymap`]). Nothing here
//! allocates, panics on hostile bytes or makes a system call: the server, its test client and
//! the kernel's benchmark all link it.

#![cfg_attr(not(test), no_std)]

pub mod blit;
pub mod keymap;
pub mod positioner;
pub mod protocol;
pub mod wire;

#[cfg(test)]
mod tests;

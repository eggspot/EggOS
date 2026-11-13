//! D-Bus as EggOS speaks it (D575), written from the D-Bus Specification (version 0.43): the
//! wire format ([`wire`]), messages and their headers ([`message`]), the SASL handshake a
//! connection opens with ([`auth`]), match rules ([`rules`]) and the message bus itself, its
//! names, routing and the `org.freedesktop.DBus` interface ([`bus`]), none of which makes a
//! system call; `session-bus` runs it on a Unix socket ([`linux`]). Single-instance apps,
//! portals and accessibility (AT-SPI) all meet on this bus.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod auth;
pub mod bus;
#[cfg(eggos_user)]
pub mod linux;
pub mod message;
pub mod rules;
pub mod wire;

//! EggOS's network protocols (D157), shared by the kernel's IPv4 stack, the commands that talk
//! to the network (`ip`, `ping`, `nslookup`), the MCP server and Settings: Ethernet II frames
//! ([`ethernet`]), ARP ([`arp`]), IPv4 ([`ipv4`]), ICMP echo ([`icmp`]), UDP ([`udp`]), a DHCP
//! client ([`dhcp`]), DNS messages ([`dns`]), one interface's stack tying them together
//! ([`interface`]) and the text the kernel reports the network in ([`status`]).
//!
//! Nothing here allocates, reads a clock or touches a device: every call takes the time in
//! milliseconds and the buffers it works in, and hands frames to send to a closure, so the
//! same code runs in the kernel, in programs without an allocator and in host tests.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod arp;
pub mod checksum;
pub mod dhcp;
pub mod dns;
pub mod ethernet;
pub mod icmp;
pub mod interface;
pub mod ipv4;
pub mod status;
pub mod udp;

#[cfg(test)]
mod tests;

pub use ethernet::MacAddress;
pub use ipv4::{Ipv4Address, Subnet};

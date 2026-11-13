//! An address of either IP version (D565), for what works over both: TCP, UDP and sockets.

use crate::ipv4::{self, Ipv4Address};
use crate::ipv6::{self, Ipv6Address};
use crate::checksum;
use core::fmt;

/// An IPv4 or IPv6 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IpAddress {
    /// IPv4.
    V4(Ipv4Address),
    /// IPv6.
    V6(Ipv6Address),
}

impl Default for IpAddress {
    fn default() -> Self {
        Self::V4(Ipv4Address::UNSPECIFIED)
    }
}

impl From<Ipv4Address> for IpAddress {
    fn from(address: Ipv4Address) -> Self {
        Self::V4(address)
    }
}

impl From<Ipv6Address> for IpAddress {
    fn from(address: Ipv6Address) -> Self {
        Self::V6(address)
    }
}

impl IpAddress {
    /// The IPv4 address, for an IPv4 address or an IPv4-mapped IPv6 one.
    pub fn ipv4(self) -> Option<Ipv4Address> {
        match self {
            Self::V4(address) => Some(address),
            Self::V6(address) => address.to_mapped_ipv4(),
        }
    }

    /// The IPv6 address, for an IPv6 address.
    pub fn ipv6(self) -> Option<Ipv6Address> {
        match self {
            Self::V4(_) => None,
            Self::V6(address) => Some(address),
        }
    }

    /// The same address in its own version: an IPv4-mapped IPv6 address becomes IPv4.
    pub fn unmapped(self) -> Self {
        match self.ipv4() {
            Some(address) => Self::V4(address),
            None => self,
        }
    }

    /// Whether it is the unspecified address of its version.
    pub fn is_unspecified(self) -> bool {
        match self {
            Self::V4(address) => address.is_unspecified(),
            Self::V6(address) => address.is_unspecified(),
        }
    }

    /// Whether it is a loopback address.
    pub fn is_loopback(self) -> bool {
        match self {
            Self::V4(address) => address.is_loopback(),
            Self::V6(address) => address.is_loopback(),
        }
    }

    /// Whether it is a multicast address.
    pub fn is_multicast(self) -> bool {
        match self {
            Self::V4(address) => address.is_multicast(),
            Self::V6(address) => address.is_multicast(),
        }
    }

    /// Whether `self` and `other` are the same version.
    pub fn same_version(self, other: Self) -> bool {
        matches!((self, other), (Self::V4(_), Self::V4(_)) | (Self::V6(_), Self::V6(_)))
    }

    /// The bytes of its header an upper layer adds to its segments' size: 20 or 40.
    pub fn header_bytes(self) -> usize {
        match self {
            Self::V4(_) => ipv4::HEADER_BYTES,
            Self::V6(_) => ipv6::HEADER_BYTES,
        }
    }
}

impl fmt::Display for IpAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(address) => address.fmt(formatter),
            Self::V6(address) => address.fmt(formatter),
        }
    }
}

/// The running checksum sum of the pseudo-header a TCP segment or UDP datagram of `length`
/// bytes of `protocol` from `source` to `destination` sums over, in whichever version they
/// are (both the same; a mixed pair sums as IPv6 with the IPv4 one mapped).
pub fn pseudo_header(source: IpAddress, destination: IpAddress, protocol: u8, length: usize) -> u32 {
    match (source, destination) {
        (IpAddress::V4(source), IpAddress::V4(destination)) => {
            let sum = checksum::add(checksum::add(0, &source.0), &destination.0);
            sum + u32::from(protocol) + length as u32
        }
        _ => ipv6::pseudo_header(as_v6(source), as_v6(destination), protocol, length),
    }
}

fn as_v6(address: IpAddress) -> Ipv6Address {
    match address {
        IpAddress::V4(address) => Ipv6Address::mapped(address),
        IpAddress::V6(address) => address,
    }
}

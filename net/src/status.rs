//! The text the kernel reports the network in, and the words every front-end uses for its
//! state (docs/ux/network-v0.3.md section 0):
//!
//! - `/proc/net/interfaces`, EggOS's own: one line per interface ([`write_interface`],
//!   [`parse_interface`]), its fields separated by single spaces, `-` for none: the name, the
//!   link (`up` or `down`), the hardware address, the MTU, the DHCP client's state
//!   ([`Dhcp`]), the address with its prefix, the router, the DNS servers separated by commas,
//!   the DHCP server, the lease's seconds left and its length (`forever` for a lease that never
//!   ends), then the counters: packets, bytes, errors and drops received, packets, bytes and
//!   drops sent.
//! - `/proc/net/route` and `/proc/net/arp` in Linux's layouts ([`write_routes`],
//!   [`write_neighbours`], [`parse_neighbour`]).

use crate::dhcp::INFINITE_LEASE_S;
use crate::ethernet::MacAddress;
use crate::interface::Counters;
use crate::ipv4::{Ipv4Address, Subnet};
use core::fmt::{self, Write};

/// The longest interface name, as Linux's `IFNAMSIZ` less its NUL.
pub const NAME_BYTES: usize = 15;
/// The loopback interface's name.
pub const LOOPBACK_NAME: &[u8] = b"lo";
/// The first Ethernet interface's name.
pub const ETHERNET_NAME: &[u8] = b"eth0";

/// An interface's name: 1 to [`NAME_BYTES`] printable ASCII bytes without spaces or colons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceName {
    bytes: [u8; NAME_BYTES],
    length: u8,
}

impl InterfaceName {
    /// The name `name`, if it is one.
    pub fn new(name: &[u8]) -> Option<Self> {
        if name.is_empty()
            || name.len() > NAME_BYTES
            || !name.iter().all(|&byte| byte.is_ascii_graphic() && byte != b':')
        {
            return None;
        }
        let mut bytes = [0u8; NAME_BYTES];
        bytes[..name.len()].copy_from_slice(name);
        Some(Self {
            bytes,
            length: name.len() as u8,
        })
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// Its text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

impl fmt::Display for InterfaceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where an interface's DHCP client is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dhcp {
    /// Asking for an address, retrying with backoff.
    Asking,
    /// Holding an address.
    Configured,
    /// No server answered; it asks again every minute.
    NoAddress,
    /// The interface has no DHCP client (`lo`).
    Off,
}

impl Dhcp {
    /// Its word in the status line, `ip` and `net_status`.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Asking => "asking",
            Self::Configured => "configured",
            Self::NoAddress => "no-address",
            Self::Off => "off",
        }
    }

    fn parse(word: &[u8]) -> Option<Self> {
        [Self::Asking, Self::Configured, Self::NoAddress, Self::Off]
            .into_iter()
            .find(|state| state.word().as_bytes() == word)
    }
}

/// The state a person sees, one vocabulary for the GUI, the CLI and MCP (no adapter at all is
/// the fifth, `none`, which has no interface to report).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The link is down.
    CableUnplugged,
    /// The link is up and the DHCP client is asking.
    GettingAddress,
    /// The link is up and no DHCP server answered.
    NoAddress,
    /// The link is up and the interface has an address.
    Connected,
}

impl State {
    /// Its word for `net_status` and `settings`.
    pub const fn word(self) -> &'static str {
        match self {
            Self::CableUnplugged => "cable-unplugged",
            Self::GettingAddress => "getting-address",
            Self::NoAddress => "no-address",
            Self::Connected => "connected",
        }
    }

    /// Its text for a person, as Settings shows it.
    pub const fn text(self) -> &'static str {
        match self {
            Self::CableUnplugged => "Cable unplugged",
            Self::GettingAddress => "Getting an address...",
            Self::NoAddress => "No address",
            Self::Connected => "Connected",
        }
    }
}

/// `net_status`'s word for a machine without a network adapter.
pub const NO_ADAPTER_WORD: &str = "none";

/// One interface's state, as `/proc/net/interfaces` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceStatus {
    /// Its name.
    pub name: InterfaceName,
    /// Whether its link is up.
    pub link_up: bool,
    /// Its hardware address (zero for `lo`).
    pub mac: MacAddress,
    /// Its MTU.
    pub mtu: u32,
    /// Its DHCP client's state.
    pub dhcp: Dhcp,
    /// Its address and network.
    pub subnet: Option<Subnet>,
    /// Its router.
    pub router: Option<Ipv4Address>,
    /// Its DNS servers.
    pub dns: [Option<Ipv4Address>; 3],
    /// The DHCP server its lease comes from.
    pub dhcp_server: Option<Ipv4Address>,
    /// The lease's seconds left ([`INFINITE_LEASE_S`]: it never ends).
    pub lease_left_s: Option<u32>,
    /// The lease's length in seconds ([`INFINITE_LEASE_S`]: it never ends).
    pub lease_s: Option<u32>,
    /// What it counted.
    pub counters: Counters,
}

impl InterfaceStatus {
    /// The state a person sees.
    pub fn state(&self) -> State {
        match (self.link_up, self.dhcp, self.subnet) {
            (false, _, _) => State::CableUnplugged,
            (true, Dhcp::Configured | Dhcp::Off, Some(_)) => State::Connected,
            (true, Dhcp::NoAddress, _) => State::NoAddress,
            _ => State::GettingAddress,
        }
    }

    /// Whether it is the loopback interface.
    pub fn is_loopback(&self) -> bool {
        self.name.as_bytes() == LOOPBACK_NAME
    }

    /// The DNS servers it has.
    pub fn dns_servers(&self) -> impl Iterator<Item = Ipv4Address> + '_ {
        self.dns.iter().flatten().copied()
    }
}

struct Field<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for Field<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(formatter),
            None => formatter.write_str("-"),
        }
    }
}

struct Seconds(Option<u32>);

impl fmt::Display for Seconds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => formatter.write_str("-"),
            Some(INFINITE_LEASE_S) => formatter.write_str("forever"),
            Some(seconds) => write!(formatter, "{seconds}"),
        }
    }
}

/// Writes `status` as one line of `/proc/net/interfaces`.
pub fn write_interface(out: &mut impl Write, status: &InterfaceStatus) -> fmt::Result {
    write!(
        out,
        "{} {} {} {} {} {} {} ",
        status.name,
        if status.link_up { "up" } else { "down" },
        status.mac,
        status.mtu,
        status.dhcp.word(),
        Field(status.subnet),
        Field(status.router),
    )?;
    let mut servers = status.dns_servers().peekable();
    if servers.peek().is_none() {
        out.write_str("-")?;
    }
    let mut first = true;
    for server in servers {
        if !first {
            out.write_str(",")?;
        }
        first = false;
        write!(out, "{server}")?;
    }
    let counters = status.counters;
    writeln!(
        out,
        " {} {} {} {} {} {} {} {} {} {}",
        Field(status.dhcp_server),
        Seconds(status.lease_left_s),
        Seconds(status.lease_s),
        counters.received_packets,
        counters.received_bytes,
        counters.received_errors,
        counters.received_dropped,
        counters.sent_packets,
        counters.sent_bytes,
        counters.sent_dropped,
    )
}

fn parse_optional<T>(word: &[u8], parse: impl Fn(&[u8]) -> Option<T>) -> Option<Option<T>> {
    match word {
        b"-" => Some(None),
        word => parse(word).map(Some),
    }
}

fn parse_number(word: &[u8]) -> Option<u64> {
    if word.is_empty() || word.len() > 20 || !word.iter().all(u8::is_ascii_digit) {
        return None;
    }
    word.iter().try_fold(0u64, |value, digit| {
        value.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
    })
}

fn parse_seconds(word: &[u8]) -> Option<Option<u32>> {
    match word {
        b"forever" => Some(Some(INFINITE_LEASE_S)),
        word => parse_optional(word, |word| u32::try_from(parse_number(word)?).ok()),
    }
}

/// The interface a line of `/proc/net/interfaces` describes; `None` for a line that is not one.
pub fn parse_interface(line: &[u8]) -> Option<InterfaceStatus> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let mut words = line.split(|&byte| byte == b' ');
    let mut next = || words.next();
    let name = InterfaceName::new(next()?)?;
    let link_up = match next()? {
        b"up" => true,
        b"down" => false,
        _ => return None,
    };
    let mac = MacAddress::parse(next()?)?;
    let mtu = u32::try_from(parse_number(next()?)?).ok()?;
    let dhcp = Dhcp::parse(next()?)?;
    let subnet = parse_optional(next()?, Subnet::parse)?;
    let router = parse_optional(next()?, Ipv4Address::parse)?;
    let mut dns = [None; 3];
    let servers = next()?;
    if servers != b"-" {
        for (slot, server) in dns.iter_mut().zip(servers.split(|&byte| byte == b',')) {
            *slot = Some(Ipv4Address::parse(server)?);
        }
    }
    let dhcp_server = parse_optional(next()?, Ipv4Address::parse)?;
    let lease_left_s = parse_seconds(next()?)?;
    let lease_s = parse_seconds(next()?)?;
    let mut counter = || parse_number(next()?);
    let counters = Counters {
        received_packets: counter()?,
        received_bytes: counter()?,
        received_errors: counter()?,
        received_dropped: counter()?,
        sent_packets: counter()?,
        sent_bytes: counter()?,
        sent_dropped: counter()?,
    };
    Some(InterfaceStatus {
        name,
        link_up,
        mac,
        mtu,
        dhcp,
        subnet,
        router,
        dns,
        dhcp_server,
        lease_left_s,
        lease_s,
        counters,
    })
}

/// The interfaces a whole `/proc/net/interfaces` describes, at most `N`, and how many.
pub fn parse_interfaces<const N: usize>(text: &[u8]) -> ([Option<InterfaceStatus>; N], usize) {
    let mut interfaces = [None; N];
    let mut count = 0;
    for line in text.split(|&byte| byte == b'\n') {
        if count == N {
            break;
        }
        if let Some(status) = parse_interface(line) {
            interfaces[count] = Some(status);
            count += 1;
        }
    }
    (interfaces, count)
}

const LINUX_LINE: usize = 127;
const ROUTE_UP: u16 = 0x0001;
const ROUTE_GATEWAY: u16 = 0x0002;

fn hex(address: Ipv4Address) -> u32 {
    u32::from_le_bytes(address.0)
}

/// Writes `/proc/net/route` in Linux's layout for `interfaces`: each one's network, and its
/// default route through its router; `lo`'s routes are not listed, as Linux keeps them in its
/// local table.
pub fn write_routes<'a>(
    out: &mut impl Write,
    interfaces: impl IntoIterator<Item = &'a InterfaceStatus>,
) -> fmt::Result {
    writeln!(
        out,
        "{:<LINUX_LINE$}",
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT"
    )?;
    let mut row = |name: &InterfaceName, destination, gateway, flags: u16, mask| {
        let mut line = Line::default();
        write!(
            line,
            "{name}\t{:08X}\t{:08X}\t{flags:04X}\t0\t0\t0\t{:08X}\t0\t0\t0",
            hex(destination),
            hex(gateway),
            hex(mask)
        )?;
        writeln!(out, "{:<LINUX_LINE$}", line.as_str())
    };
    for status in interfaces {
        let Some(subnet) = status.subnet.filter(|_| !status.is_loopback()) else {
            continue;
        };
        if let Some(router) = status.router {
            row(
                &status.name,
                Ipv4Address::UNSPECIFIED,
                router,
                ROUTE_UP | ROUTE_GATEWAY,
                Ipv4Address::UNSPECIFIED,
            )?;
        }
        row(
            &status.name,
            subnet.network(),
            Ipv4Address::UNSPECIFIED,
            ROUTE_UP,
            subnet.mask(),
        )?;
    }
    Ok(())
}

struct Line {
    bytes: [u8; LINUX_LINE],
    length: usize,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            bytes: [0; LINUX_LINE],
            length: 0,
        }
    }
}

impl Line {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.length + text.len();
        let room = self.bytes.get_mut(self.length..end).ok_or(fmt::Error)?;
        room.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

/// One neighbour, as `/proc/net/arp` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neighbour {
    /// Its address.
    pub address: Ipv4Address,
    /// Its hardware address.
    pub mac: MacAddress,
    /// The interface it was heard on.
    pub interface: InterfaceName,
}

/// Writes `/proc/net/arp` in Linux's layout: a heading, then each neighbour, complete.
pub fn write_neighbours(
    out: &mut impl Write,
    neighbours: impl IntoIterator<Item = Neighbour>,
) -> fmt::Result {
    out.write_str(
        "IP address       HW type     Flags       HW address            Mask     Device\n",
    )?;
    for neighbour in neighbours {
        let mut address = Line::default();
        write!(address, "{}", neighbour.address)?;
        writeln!(
            out,
            "{:<16} 0x{:<10x}0x{:<10x}{}     *        {}",
            address.as_str(),
            1,
            2,
            neighbour.mac,
            neighbour.interface
        )?;
    }
    Ok(())
}

/// The neighbour a line of `/proc/net/arp` lists; `None` for the heading or another line.
pub fn parse_neighbour(line: &[u8]) -> Option<Neighbour> {
    let mut words = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|word| !word.is_empty());
    let address = Ipv4Address::parse(words.next()?)?;
    let _kind = words.next()?;
    let _flags = words.next()?;
    let mac = MacAddress::parse(words.next()?)?;
    let _mask = words.next()?;
    let interface = InterfaceName::new(words.next()?)?;
    Some(Neighbour {
        address,
        mac,
        interface,
    })
}

#[cfg(test)]
mod tests;

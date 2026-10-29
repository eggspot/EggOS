//! The network (D157; TCP, D158: [`tcp`]): an IPv4 stack over the first network adapter a
//! driver lends the kernel ([`NetDevice`]), and the loopback interface `lo`, which always
//! exists.
//!
//! The protocols are the `eggos-net` crate's ([`eggos_net::interface::Interface`]); this module
//! owns the adapter and its interface (`eth0`), runs them, and connects them to the endpoints
//! programs send and receive through ([`endpoints`]). Adapters do not interrupt yet (issue
//! #97), so a kernel thread started at boot when there is an adapter polls it: every
//! [`ACTIVE_POLL_MS`] while frames moved in the last [`ACTIVE_WINDOW_MS`], every
//! [`IDLE_POLL_MS`] otherwise, sooner when a timer of the interface is due. A machine without
//! an adapter starts no thread and holds nothing but `lo`'s counters.
//!
//! Packets to `127.0.0.0/8` or to the interface's own address never reach the adapter: they
//! are handed to the endpoints at once, as Linux's loopback does. `/etc/resolv.conf` lists the
//! DNS servers DHCP gave ([`resolv_conf`]); `/proc/net/interfaces`, `/proc/net/route` and
//! `/proc/net/arp` report the interfaces ([`interfaces_text`], [`routes_text`],
//! [`neighbours_text`]).

pub mod endpoints;
pub mod resolv_conf;
pub mod tcp;

use crate::console;
use crate::sync::LeafSpinLock;
use crate::thread;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use eggos_hal::Platform;
use eggos_net::ethernet::{MAX_FRAME_BYTES, MacAddress};
use eggos_net::interface::{Counters, Datagram, Delivery, Interface, SendError};
use eggos_net::ipv4::{Ipv4Address, Subnet};
use eggos_net::status::{
    Dhcp, ETHERNET_NAME, InterfaceName, InterfaceStatus, LOOPBACK_NAME, Neighbour,
};
use eggos_net::{icmp, ipv4};

/// How often the adapter is polled while frames moved lately.
pub const ACTIVE_POLL_MS: u64 = 1;
/// How often it is polled otherwise.
pub const IDLE_POLL_MS: u64 = 20;
/// How long after frames last moved the adapter is polled every [`ACTIVE_POLL_MS`].
pub const ACTIVE_WINDOW_MS: u64 = 1000;
/// The most frames one poll takes from the adapter, so a flood cannot hold the CPU.
pub const FRAMES_PER_POLL: usize = 64;
/// `lo`'s MTU, as Linux's.
pub const LOOPBACK_MTU: u32 = 65536;

/// What a network adapter's driver lends the stack: frames in and out, polled.
pub trait NetDevice: Send {
    /// The adapter's hardware address.
    fn mac(&self) -> MacAddress;
    /// The largest IP packet its link carries.
    fn mtu(&self) -> usize;
    /// Whether its link is up.
    fn is_link_up(&self) -> bool;
    /// Copies the next frame it received into the start of `frame`: its length, or `None`
    /// when there is none.
    fn receive(&mut self, frame: &mut [u8]) -> Option<usize>;
    /// Hands `frame` to the adapter to send.
    fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str>;
}

struct Adapter {
    device: Box<dyn NetDevice>,
    interface: Interface,
    frame: [u8; MAX_FRAME_BYTES],
}

static DEVICE: LeafSpinLock<Option<Box<dyn NetDevice>>> = LeafSpinLock::new_leaf(None);

static ADAPTER: LeafSpinLock<Option<Box<Adapter>>> = LeafSpinLock::new_leaf(None);

static LAST_TRAFFIC_MS: AtomicU64 = AtomicU64::new(0);

static LOOPBACK_PACKETS: AtomicU64 = AtomicU64::new(0);

static LOOPBACK_BYTES: AtomicU64 = AtomicU64::new(0);

/// Lends the stack `device`, the first adapter; gives it back when there already is one.
pub fn install(device: Box<dyn NetDevice>) -> Result<(), Box<dyn NetDevice>> {
    if is_present() {
        return Err(device);
    }
    let mut slot = DEVICE.lock();
    match slot.is_some() {
        true => Err(device),
        false => {
            *slot = Some(device);
            Ok(())
        }
    }
}

/// Whether a driver lent the stack an adapter.
pub fn is_present() -> bool {
    DEVICE.lock().is_some() || ADAPTER.lock().is_some()
}

/// Milliseconds since boot on platform `P`'s clock.
pub fn now_ms<P: Platform>() -> u64 {
    let frequency = u128::from(P::clock_frequency().max(1));
    (u128::from(P::clock_ticks()) * 1000 / frequency) as u64
}

/// Starts the network at boot: with an adapter, makes its interface, serves
/// `/etc/resolv.conf`, starts the thread that polls it and says so on `console`. Without one,
/// does nothing.
pub fn start<P: Platform>(console: &mut impl Write) -> core::fmt::Result {
    let Some(device) = DEVICE.lock().take() else {
        return Ok(());
    };
    let mut seed = [0u8; 8];
    crate::random::fill::<P>(&mut seed);
    let mac = device.mac();
    let link_up = device.is_link_up();
    let mut interface = Interface::new(mac, device.mtu(), u64::from_ne_bytes(seed));
    interface.set_link(link_up, now_ms::<P>());
    *ADAPTER.lock() = Some(Box::new(Adapter {
        device,
        interface,
        frame: [0; MAX_FRAME_BYTES],
    }));
    resolv_conf::mount();
    let state = match link_up {
        true => "asking for an address by DHCP",
        false => "no cable",
    };
    writeln!(console, "net: eth0 {mac}: {state}")?;
    if thread::spawn::<P, _>(poll_forever::<P>).is_err() {
        writeln!(console, "net: no thread to run the network: eth0 stays idle")?;
    }
    Ok(())
}

fn poll_forever<P: Platform>() {
    loop {
        poll::<P>();
        let now = now_ms::<P>();
        let active = now.saturating_sub(LAST_TRAFFIC_MS.load(Ordering::Relaxed)) < ACTIVE_WINDOW_MS;
        let interval = if active { ACTIVE_POLL_MS } else { IDLE_POLL_MS };
        let due = next_poll_ms().saturating_sub(now).clamp(ACTIVE_POLL_MS, interval);
        thread::sleep::<P>(Duration::from_millis(due));
    }
}

fn next_poll_ms() -> u64 {
    ADAPTER
        .lock()
        .as_ref()
        .map_or(u64::MAX, |adapter| adapter.interface.next_poll_ms())
}

/// Takes the frames the adapter received (at most [`FRAMES_PER_POLL`]), runs the interface's
/// timers, hands what came for programs to their endpoints and wakes their waiters. Whether
/// anything was delivered.
pub fn poll<P: Platform>() -> bool {
    let now = now_ms::<P>();
    let mut delivered = false;
    let mut moved = false;
    {
        let mut adapter = ADAPTER.lock();
        let Some(adapter) = adapter.as_mut() else {
            return false;
        };
        let Adapter {
            device,
            interface,
            frame,
        } = &mut **adapter;
        interface.set_link(device.is_link_up(), now);
        for _ in 0..FRAMES_PER_POLL {
            let Some(length) = device.receive(frame) else {
                break;
            };
            moved = true;
            interface.receive(
                &frame[..length],
                now,
                &mut |bytes| {
                    let _ = device.transmit(bytes);
                },
                &mut |delivery| delivered |= deliver::<P>(delivery),
            );
        }
        interface.poll(now, &mut |bytes| {
            let _ = device.transmit(bytes);
        });
    }
    tcp::run::<P>();
    if moved {
        LAST_TRAFFIC_MS.store(now, Ordering::Relaxed);
    }
    if delivered {
        console::wake_input_waiters::<P>();
    }
    delivered
}

fn deliver<P: Platform>(delivery: Delivery<'_>) -> bool {
    match delivery {
        Delivery::Tcp {
            source,
            destination,
            segment,
        } => tcp::deliver::<P>(source, destination, &segment),
        other => endpoints::deliver(other),
    }
}

/// The largest TCP segment the adapter's link carries; [`tcp::MSS`] without an adapter.
pub fn tcp_mss() -> u16 {
    ADAPTER
        .lock()
        .as_ref()
        .map_or(tcp::MSS, |adapter| adapter.interface.tcp_mss())
}

/// Sends `segment`, a TCP segment from the interface's address, to `destination` through the
/// adapter, with time to live `ttl`.
pub fn send_tcp<P: Platform>(
    destination: Ipv4Address,
    segment: &[u8],
    ttl: u8,
) -> Result<(), SendError> {
    let now = now_ms::<P>();
    let mut adapter = ADAPTER.lock();
    let Some(adapter) = adapter.as_mut() else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut **adapter;
    interface.send_tcp(destination, segment, ttl, now, &mut |bytes| {
        let _ = device.transmit(bytes);
    })
}

/// Whether `address` is this machine's: in `127.0.0.0/8`, or the interface's.
pub fn is_local(address: Ipv4Address) -> bool {
    address.is_loopback() || local_address() == Some(address)
}

/// The interface's address, if it has one.
pub fn local_address() -> Option<Ipv4Address> {
    ADAPTER
        .lock()
        .as_ref()
        .and_then(|adapter| adapter.interface.address())
}

/// The broadcast address of the interface's network, if it has an address.
pub fn subnet_broadcast() -> Option<Ipv4Address> {
    ADAPTER
        .lock()
        .as_ref()
        .and_then(|adapter| adapter.interface.subnet())
        .map(Subnet::broadcast)
}

/// The DNS servers the interface's lease names.
pub fn dns_servers() -> [Option<Ipv4Address>; 3] {
    ADAPTER
        .lock()
        .as_ref()
        .and_then(|adapter| adapter.interface.lease().map(|lease| lease.dns))
        .unwrap_or([None; 3])
}

/// The address a packet to `destination` leaves from: `127.0.0.1` to the loopback network,
/// the interface's address otherwise (`0.0.0.0` without one).
pub fn source_for(destination: Ipv4Address) -> Ipv4Address {
    match destination.is_loopback() {
        true => Ipv4Address::LOOPBACK,
        false => local_address().unwrap_or(Ipv4Address::UNSPECIFIED),
    }
}

fn count_loopback(bytes: usize) {
    LOOPBACK_PACKETS.fetch_add(1, Ordering::Relaxed);
    LOOPBACK_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
}

/// Sends `data` in a UDP datagram from `source` (`0.0.0.0`: the address the route gives):
/// `source_port` to `destination`:`destination_port`, with time to live `ttl`. A datagram for
/// this machine is handed to its endpoint at once.
pub fn send_udp<P: Platform>(
    source: Ipv4Address,
    source_port: u16,
    destination: (Ipv4Address, u16),
    data: &[u8],
    ttl: u8,
) -> Result<(), SendError> {
    let (address, port) = destination;
    if is_local(address) {
        if data.len() > usize::from(u16::MAX) - 28 {
            return Err(SendError::TooLong);
        }
        let source = match source.is_unspecified() {
            true => source_for(address),
            false => source,
        };
        count_loopback(data.len() + 28);
        let datagram = Datagram {
            source,
            source_port,
            destination: address,
            destination_port: port,
            data,
        };
        if endpoints::deliver(Delivery::Udp(datagram)) {
            console::wake_input_waiters::<P>();
        }
        return Ok(());
    }
    let now = now_ms::<P>();
    let mut adapter = ADAPTER.lock();
    let Some(adapter) = adapter.as_mut() else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut **adapter;
    interface.send_udp(source_port, address, port, data, ttl, now, &mut |bytes| {
        let _ = device.transmit(bytes);
    })
}

/// Sends an echo request with `identifier`, `sequence` and `data` to `destination`, with time
/// to live `ttl`. One to this machine is answered at once: the reply goes to the ping
/// endpoint with `identifier`.
pub fn send_echo<P: Platform>(
    destination: Ipv4Address,
    identifier: u16,
    sequence: u16,
    data: &[u8],
    ttl: u8,
) -> Result<(), SendError> {
    if is_local(destination) {
        let mut reply = Vec::new();
        if reply.try_reserve_exact(icmp::HEADER_BYTES + data.len()).is_err() {
            return Err(SendError::TooLong);
        }
        reply.resize(icmp::HEADER_BYTES, 0);
        reply.extend_from_slice(data);
        icmp::write_echo_header(&mut reply, icmp::kind::ECHO_REPLY, identifier, sequence, data.len())
            .ok_or(SendError::TooLong)?;
        count_loopback(2 * (reply.len() + ipv4::HEADER_BYTES));
        let delivery = Delivery::EchoReply {
            source: destination,
            identifier,
            message: &reply,
        };
        if endpoints::deliver(delivery) {
            console::wake_input_waiters::<P>();
        }
        return Ok(());
    }
    let now = now_ms::<P>();
    let mut adapter = ADAPTER.lock();
    let Some(adapter) = adapter.as_mut() else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut **adapter;
    interface.send_echo(
        destination,
        icmp::kind::ECHO_REQUEST,
        identifier,
        sequence,
        data,
        ttl,
        now,
        &mut |bytes| {
            let _ = device.transmit(bytes);
        },
    )
}

fn loopback_status() -> InterfaceStatus {
    let packets = LOOPBACK_PACKETS.load(Ordering::Relaxed);
    let bytes = LOOPBACK_BYTES.load(Ordering::Relaxed);
    InterfaceStatus {
        name: InterfaceName::new(LOOPBACK_NAME).unwrap_or_else(|| unreachable!()),
        link_up: true,
        mac: MacAddress::ZERO,
        mtu: LOOPBACK_MTU,
        dhcp: Dhcp::Off,
        subnet: Subnet::new(Ipv4Address::LOOPBACK, 8),
        router: None,
        dns: [None; 3],
        dhcp_server: None,
        lease_left_s: None,
        lease_s: None,
        counters: Counters {
            received_packets: packets,
            received_bytes: bytes,
            sent_packets: packets,
            sent_bytes: bytes,
            ..Counters::default()
        },
    }
}

fn ethernet_status<P: Platform>() -> Option<InterfaceStatus> {
    let now = now_ms::<P>();
    let adapter = ADAPTER.lock();
    let interface = &adapter.as_ref()?.interface;
    let lease = interface.lease();
    Some(InterfaceStatus {
        name: InterfaceName::new(ETHERNET_NAME)?,
        link_up: interface.is_link_up(),
        mac: interface.mac(),
        mtu: interface.mtu() as u32,
        dhcp: interface.dhcp(),
        subnet: interface.subnet(),
        router: lease.and_then(|lease| lease.router),
        dns: lease.map_or([None; 3], |lease| lease.dns),
        dhcp_server: lease.map(|lease| lease.server),
        lease_left_s: lease.map(|lease| lease.left_s(now)),
        lease_s: lease.map(|lease| lease.lease_s),
        counters: interface.counters(),
    })
}

/// Every interface's state at this moment: `lo`, then `eth0` when there is an adapter.
pub fn interfaces<P: Platform>() -> Vec<InterfaceStatus> {
    let mut interfaces = Vec::new();
    if interfaces.try_reserve_exact(2).is_ok() {
        interfaces.push(loopback_status());
        interfaces.extend(ethernet_status::<P>());
    }
    interfaces
}

struct Text(Vec<u8>);

impl Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0
            .try_reserve(text.len())
            .map_err(|_| core::fmt::Error)?;
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// `/proc/net/interfaces`: one line per interface ([`eggos_net::status`]).
pub fn interfaces_text<P: Platform>() -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    for status in interfaces::<P>() {
        eggos_net::status::write_interface(&mut text, &status).ok()?;
    }
    Some(text.0)
}

/// `/proc/net/route` in Linux's layout.
pub fn routes_text<P: Platform>() -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    eggos_net::status::write_routes(&mut text, &interfaces::<P>()).ok()?;
    Some(text.0)
}

/// The neighbours the interface knows the hardware addresses of.
pub fn neighbours<P: Platform>() -> Vec<Neighbour> {
    let now = now_ms::<P>();
    let mut neighbours = Vec::new();
    let adapter = ADAPTER.lock();
    let (Some(adapter), Some(name)) = (adapter.as_ref(), InterfaceName::new(ETHERNET_NAME)) else {
        return neighbours;
    };
    for (address, mac) in adapter.interface.neighbours(now) {
        if neighbours.try_reserve(1).is_err() {
            break;
        }
        neighbours.push(Neighbour {
            address,
            mac,
            interface: name,
        });
    }
    neighbours
}

/// `/proc/net/arp` in Linux's layout.
pub fn neighbours_text<P: Platform>() -> Option<Vec<u8>> {
    let mut text = Text(Vec::new());
    eggos_net::status::write_neighbours(&mut text, neighbours::<P>()).ok()?;
    Some(text.0)
}

#[cfg(test)]
mod tests;

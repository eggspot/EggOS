//! The network (D157; TCP, D158: [`tcp`]): an IPv4 stack over the network adapters drivers lend
//! the kernel ([`NetDevice`]): the first wired one as `eth0` and a Wi-Fi station as `wlan0`, both
//! at once (D286), and the loopback interface `lo`, which always exists. A packet leaves through
//! the adapter whose network holds its destination, otherwise through the first adapter with a
//! router, wired before Wi-Fi as Linux's default metrics order them.
//!
//! The protocols are the `eggos-net` crate's ([`eggos_net::interface::Interface`]); this module
//! owns the adapter and its interface (`eth0`), runs them, and connects them to the endpoints
//! programs send and receive through ([`endpoints`]). A kernel thread started at boot when
//! there is an adapter runs it. An adapter that interrupts (D220) wakes the thread when frames
//! arrive, and otherwise the thread sleeps until a timer of the interface is due, or, while
//! TCP connections are open, polls as below for their timers. An adapter that cannot interrupt
//! is polled: every [`ACTIVE_POLL_MS`] while frames moved in the last [`ACTIVE_WINDOW_MS`],
//! every [`IDLE_POLL_MS`] otherwise, sooner when a timer of the interface is due. A machine
//! without an adapter starts no thread and holds nothing but `lo`'s counters.
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
use crate::devices::interrupts::DeviceEvents;
use crate::sync::LeafSpinLock;
use crate::thread;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use eggos_drivers::desktop::NetworkState;
use eggos_hal::Platform;
use eggos_hal::atomic::AtomicU64;
use eggos_net::checksum::Completion;
use eggos_net::ethernet::{MAX_FRAME_BYTES, MacAddress};

/// The longest frame an adapter hands up: a TCP segment the device joined from several
/// (receive coalescing, D566), up to the largest IP packet with its Ethernet header.
pub const JOINED_FRAME_BYTES: usize = u16::MAX as usize + MAX_FRAME_BYTES;
use eggos_net::interface::{
    Counters, Datagram, Delivery, Interface, LARGE_TCP_SEGMENT_AT, SendError,
};
use eggos_net::ip::IpAddress;
use eggos_net::ipv4::{Ipv4Address, Subnet};
use eggos_net::ipv6::{self, Ipv6Address};
use eggos_net::status::{
    Dhcp, ETHERNET_NAME, InterfaceName, InterfaceStatus, LOOPBACK_NAME, Neighbour, WIRELESS_NAME,
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
    /// Whether it completes the TCP and UDP checksums of the frames it sends, which then carry
    /// only their pseudo-header's sum (D535).
    fn completes_checksums(&self) -> bool {
        false
    }
    /// The longest frame it cuts into TCP segments itself, from a TCP segment longer than the
    /// MTU (D541); `None` when it does not (the default).
    fn segmentation_limit(&self) -> Option<usize> {
        None
    }
    /// Hands `frame`, a TCP segment longer than the MTU, to the adapter to cut into segments
    /// of `segment_size` data bytes and send.
    fn transmit_segmented(
        &mut self,
        _frame: &[u8],
        _segment_size: u16,
    ) -> Result<(), &'static str> {
        Err("the adapter does not cut segments")
    }
}

struct Adapter {
    name: &'static [u8],
    device: Box<dyn NetDevice>,
    interface: Interface,
    frame: Vec<u8>,
}

impl Adapter {
    fn new<P: Platform>(name: &'static [u8], device: Box<dyn NetDevice>) -> Box<Self> {
        let mut seed = [0u8; 8];
        crate::random::fill::<P>(&mut seed);
        let mut interface = Interface::new(device.mac(), device.mtu(), u64::from_ne_bytes(seed));
        interface.set_link(device.is_link_up(), now_ms::<P>());
        if device.completes_checksums() {
            interface.set_transmit_checksums(Completion::Partial);
        }
        interface.set_segmentation_limit(device.segmentation_limit());
        Box::new(Self {
            name,
            device,
            interface,
            frame: alloc::vec![0; JOINED_FRAME_BYTES],
        })
    }

    fn has_router(&self) -> bool {
        self.interface.is_link_up()
            && self.interface.address().is_some()
            && self
                .interface
                .lease()
                .is_some_and(|lease| lease.router.is_some())
    }

    fn holds(&self, destination: Ipv4Address) -> bool {
        self.interface.is_link_up()
            && self
                .interface
                .subnet()
                .is_some_and(|subnet| subnet.contains(destination))
    }
}

fn route(adapters: &[Box<Adapter>], destination: Ipv4Address) -> Option<usize> {
    adapters
        .iter()
        .position(|adapter| adapter.holds(destination))
        .or_else(|| adapters.iter().position(|adapter| adapter.has_router()))
        .or_else(|| {
            adapters
                .iter()
                .position(|adapter| adapter.interface.is_link_up())
        })
        .or_else(|| (!adapters.is_empty()).then_some(0))
}

fn route6(adapters: &[Box<Adapter>], destination: Ipv6Address, now: u64) -> Option<usize> {
    adapters
        .iter()
        .position(|adapter| {
            adapter.interface.is_link_up() && adapter.interface.ipv6().is_on_link(destination)
                && !destination.is_link_local()
        })
        .or_else(|| {
            adapters.iter().position(|adapter| {
                adapter.interface.is_link_up() && adapter.interface.ipv6().router(now).is_some()
            })
        })
        .or_else(|| {
            adapters
                .iter()
                .position(|adapter| adapter.interface.is_link_up())
        })
}

fn default_adapter(adapters: &[Box<Adapter>]) -> Option<&Adapter> {
    route(adapters, Ipv4Address::new(1, 1, 1, 1)).map(|index| &*adapters[index])
}

static DEVICE: LeafSpinLock<Option<Box<dyn NetDevice>>> = LeafSpinLock::new_leaf(None);

static ADAPTERS: LeafSpinLock<Vec<Box<Adapter>>> = LeafSpinLock::new_leaf(Vec::new());

static POLLING: AtomicBool = AtomicBool::new(false);

static LAST_TRAFFIC_MS: AtomicU64 = AtomicU64::new(0);

/// What the adapter raises when it interrupts (D220): it wakes the network's thread.
pub static DEVICE_EVENTS: DeviceEvents = DeviceEvents::new();

static INTERRUPTING: AtomicBool = AtomicBool::new(false);

static LOOPBACK_PACKETS: AtomicU64 = AtomicU64::new(0);

static LOOPBACK_BYTES: AtomicU64 = AtomicU64::new(0);

static INDICATED: AtomicU64 = AtomicU64::new(NO_ADAPTER_WORD);

const NO_ADAPTER_WORD: u64 = 0;
const CABLE_UNPLUGGED_WORD: u64 = 1;
const GETTING_ADDRESS_WORD: u64 = 2;
const NO_ADDRESS_WORD: u64 = 3;
const CONNECTED_WORD: u64 = 4;
const STATE_BITS: u64 = 0xff;
const ADDRESS_SHIFT: u32 = 8;

/// Lends the stack `device`, the first wired adapter; gives it back when there already is one.
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

/// Lends the stack `device`, whose interrupts raise [`DEVICE_EVENTS`]: the network's thread
/// sleeps until it interrupts or a timer is due instead of polling it.
pub fn install_interrupting(device: Box<dyn NetDevice>) -> Result<(), Box<dyn NetDevice>> {
    install(device)?;
    INTERRUPTING.store(true, Ordering::Release);
    Ok(())
}

/// Whether a driver lent the stack a wired adapter.
pub fn is_present() -> bool {
    DEVICE.lock().is_some()
        || ADAPTERS
            .lock()
            .iter()
            .any(|adapter| adapter.name == ETHERNET_NAME)
}

fn publish_indicator<P: Platform>() {
    let indicated = word_of(indicator_state(&ADAPTERS.lock()));
    if INDICATED.swap(indicated, Ordering::Relaxed) != indicated {
        crate::terminal::network_changed::<P>();
    }
}

fn start_polling<P: Platform>() -> Result<(), &'static str> {
    if POLLING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    thread::spawn::<P, _>(poll_forever::<P>)
        .map(|_| ())
        .map_err(|_| {
            POLLING.store(false, Ordering::Release);
            "no thread to run the network"
        })
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
    let mut port_key = [0u8; 32];
    crate::random::fill::<P>(&mut port_key);
    endpoints::seed_ports(port_key);
    let Some(device) = DEVICE.lock().take() else {
        return Ok(());
    };
    let mac = device.mac();
    let link_up = device.is_link_up();
    let adapter = Adapter::new::<P>(ETHERNET_NAME, device);
    let indicated = word_of(indicator_state(core::slice::from_ref(&adapter)));
    ADAPTERS.lock().insert(0, adapter);
    INDICATED.store(indicated, Ordering::Relaxed);
    crate::terminal::network_changed::<P>();
    resolv_conf::mount();
    let state = match link_up {
        true => "asking for an address by DHCP",
        false => "no cable",
    };
    writeln!(console, "net: eth0 {mac}: {state}")?;
    if start_polling::<P>().is_err() {
        writeln!(
            console,
            "net: no thread to run the network: eth0 stays idle"
        )?;
    }
    Ok(())
}

/// Lends the stack the Wi-Fi station's `device` after boot as `wlan0`, beside a wired adapter
/// if there is one (D279, D286). Starts the network's thread if [`start`] did not.
pub fn attach_wireless<P: Platform>(device: Box<dyn NetDevice>) -> Result<(), &'static str> {
    {
        let mut adapters = ADAPTERS.lock();
        if adapters.iter().any(|adapter| adapter.name == WIRELESS_NAME) {
            return Err("a Wi-Fi adapter is already in use");
        }
        if adapters.try_reserve(1).is_err() {
            return Err("no memory for the Wi-Fi adapter");
        }
        adapters.push(Adapter::new::<P>(WIRELESS_NAME, device));
    }
    publish_indicator::<P>();
    resolv_conf::mount();
    start_polling::<P>()
}

fn poll_forever<P: Platform>() {
    let interrupting = INTERRUPTING.load(Ordering::Acquire);
    loop {
        let seen = DEVICE_EVENTS.raised();
        poll::<P>();
        let now = now_ms::<P>();
        let active = now.saturating_sub(LAST_TRAFFIC_MS.load(Ordering::Relaxed)) < ACTIVE_WINDOW_MS;
        let interval = if active { ACTIVE_POLL_MS } else { IDLE_POLL_MS };
        let timers_due = next_poll_ms().saturating_sub(now);
        if interrupting {
            let due = match tcp::has_streams() {
                true => timers_due.clamp(ACTIVE_POLL_MS, interval),
                false => timers_due.max(ACTIVE_POLL_MS),
            };
            DEVICE_EVENTS.wait::<P>(seen, ticks_after::<P>(due));
            continue;
        }
        thread::sleep::<P>(Duration::from_millis(
            timers_due.clamp(ACTIVE_POLL_MS, interval),
        ));
    }
}

pub(crate) fn ticks_after<P: Platform>(milliseconds: u64) -> u64 {
    if milliseconds == u64::MAX {
        return u64::MAX;
    }
    let ticks = eggos_hal::time::mul_div_ceil(milliseconds, P::clock_frequency(), 1000);
    P::clock_ticks().saturating_add(ticks)
}

fn next_poll_ms() -> u64 {
    ADAPTERS
        .lock()
        .iter()
        .map(|adapter| adapter.interface.next_poll_ms())
        .min()
        .unwrap_or(u64::MAX)
}

/// Takes the frames the adapter received (at most [`FRAMES_PER_POLL`]), runs the interface's
/// timers, hands what came for programs to their endpoints and wakes their waiters. Whether
/// anything was delivered.
pub fn poll<P: Platform>() -> bool {
    let now = now_ms::<P>();
    let mut delivered = false;
    let mut moved = false;
    let indicated = {
        let mut adapters = ADAPTERS.lock();
        if adapters.is_empty() {
            return false;
        }
        for adapter in adapters.iter_mut() {
            let Adapter {
                device,
                interface,
                frame,
                ..
            } = &mut **adapter;
            interface.set_link(device.is_link_up(), now);
            let link_mss = interface.tcp_mss();
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
                    &mut |delivery| delivered |= deliver::<P>(delivery, link_mss),
                );
            }
            interface.poll(now, &mut |bytes| {
                let _ = device.transmit(bytes);
            });
        }
        word_of(indicator_state(&adapters))
    };
    if INDICATED.swap(indicated, Ordering::Relaxed) != indicated {
        crate::terminal::network_changed::<P>();
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

fn deliver<P: Platform>(delivery: Delivery<'_>, link_mss: u16) -> bool {
    match delivery {
        Delivery::Tcp {
            source,
            destination,
            segment,
        } => tcp::deliver::<P>(source, destination, &segment, link_mss),
        other => endpoints::deliver(other),
    }
}

fn indicator_state(adapters: &[Box<Adapter>]) -> NetworkState {
    let chosen = adapters
        .iter()
        .find(|adapter| adapter.interface.is_link_up() && adapter.interface.address().is_some())
        .or_else(|| {
            adapters
                .iter()
                .find(|adapter| adapter.interface.is_link_up())
        })
        .or(adapters.first());
    match chosen {
        Some(adapter) => interface_state(&adapter.interface),
        None => NetworkState::CableUnplugged,
    }
}

fn interface_state(interface: &Interface) -> NetworkState {
    match (
        interface.is_link_up(),
        interface.dhcp(),
        interface.address(),
    ) {
        (false, _, _) => NetworkState::CableUnplugged,
        (true, _, Some(address)) => NetworkState::Connected(address.0),
        (true, Dhcp::NoAddress, None) => NetworkState::NoAddress,
        (true, _, None) => NetworkState::GettingAddress,
    }
}

fn word_of(state: NetworkState) -> u64 {
    match state {
        NetworkState::CableUnplugged => CABLE_UNPLUGGED_WORD,
        NetworkState::GettingAddress => GETTING_ADDRESS_WORD,
        NetworkState::NoAddress => NO_ADDRESS_WORD,
        NetworkState::Connected(address) => {
            CONNECTED_WORD | (u64::from(u32::from_be_bytes(address)) << ADDRESS_SHIFT)
        }
    }
}

fn state_of(word: u64) -> Option<NetworkState> {
    match word & STATE_BITS {
        CABLE_UNPLUGGED_WORD => Some(NetworkState::CableUnplugged),
        GETTING_ADDRESS_WORD => Some(NetworkState::GettingAddress),
        NO_ADDRESS_WORD => Some(NetworkState::NoAddress),
        CONNECTED_WORD => Some(NetworkState::Connected(
            ((word >> ADDRESS_SHIFT) as u32).to_be_bytes(),
        )),
        _ => None,
    }
}

/// What the desktop's network indicator shows (docs/ux/network-v0.3.md section 4, D225): the
/// adapter's state as the network's thread last saw it, `None` without an adapter. Each change
/// asks the desktop to draw again ([`crate::terminal::network_changed`]).
pub fn indicated() -> Option<NetworkState> {
    state_of(INDICATED.load(Ordering::Relaxed))
}

/// The largest TCP segment the default adapter's link carries; [`tcp::MSS`] without an
/// adapter.
pub fn tcp_mss() -> u16 {
    default_adapter(&ADAPTERS.lock()).map_or(tcp::MSS, |adapter| adapter.interface.tcp_mss())
}

/// The largest TCP segment to `destination` its adapter's link carries (D565).
pub fn tcp_mss_for(destination: IpAddress) -> u16 {
    match destination.unmapped() {
        IpAddress::V4(_) => tcp_mss(),
        IpAddress::V6(destination) => {
            let adapters = ADAPTERS.lock();
            route6(&adapters, destination, 0)
                .map_or(tcp::MSS, |index| adapters[index].interface.tcp_mss6())
        }
    }
}

/// Sends `segment`, a TCP segment from the interface's address whose checksum is whole or
/// partial as `completion` says, to `destination` through the adapter, with time to live `ttl`
/// (a hop limit over IPv6, D565).
pub fn send_tcp<P: Platform>(
    destination: impl Into<IpAddress>,
    segment: &[u8],
    completion: Completion,
    ttl: u8,
) -> Result<(), SendError> {
    let now = now_ms::<P>();
    let destination = match destination.into().unmapped() {
        IpAddress::V4(destination) => destination,
        IpAddress::V6(destination) => {
            let mut adapters = ADAPTERS.lock();
            let index = route6(&adapters, destination, now).ok_or(SendError::NoRoute)?;
            let Adapter {
                device, interface, ..
            } = &mut *adapters[index];
            return interface.send_tcp6(destination, segment, completion, ttl, now, &mut |bytes| {
                let _ = device.transmit(bytes);
            });
        }
    };
    let mut adapters = ADAPTERS.lock();
    let Some(index) = route(&adapters, destination) else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut *adapters[index];
    interface.send_tcp(destination, segment, completion, ttl, now, &mut |bytes| {
        let _ = device.transmit(bytes);
    })
}

/// The most TCP data one segment to `destination` may hold for its adapter to cut into
/// segments itself (D541); `None` when the adapter does not cut segments.
pub fn segmentation_data_limit(destination: IpAddress) -> Option<usize> {
    let IpAddress::V4(destination) = destination.unmapped() else {
        return None;
    };
    let adapters = ADAPTERS.lock();
    let index = route(&adapters, destination)?;
    adapters[index].interface.segmentation_data_limit()
}

/// Sends `segment`, a TCP segment from the interface's address longer than the MTU, whose
/// checksum is whole or partial as `completion` says, to `destination` through an adapter that
/// cuts it into segments of `segment_size` data bytes itself (D541), with time to live `ttl`.
/// [`SendError::TooLong`] when that adapter does not cut segments.
pub fn send_large_tcp<P: Platform>(
    destination: IpAddress,
    segment: &[u8],
    completion: Completion,
    segment_size: u16,
    ttl: u8,
) -> Result<(), SendError> {
    let IpAddress::V4(destination) = destination.unmapped() else {
        return Err(SendError::TooLong);
    };
    let mut frame = Vec::new();
    let length = LARGE_TCP_SEGMENT_AT + segment.len();
    if frame.try_reserve_exact(length).is_err() {
        return Err(SendError::TooLong);
    }
    frame.resize(length, 0);
    frame[LARGE_TCP_SEGMENT_AT..].copy_from_slice(segment);
    let now = now_ms::<P>();
    let mut adapters = ADAPTERS.lock();
    let Some(index) = route(&adapters, destination) else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut *adapters[index];
    let length =
        interface.frame_large_tcp(&mut frame, destination, segment.len(), completion, ttl, now)?;
    device
        .transmit_segmented(&frame[..length], segment_size)
        .map_err(|_| SendError::TooLong)
}

/// Whether `address` is this machine's: in `127.0.0.0/8`, `::1`, or an interface's.
pub fn is_local(address: impl Into<IpAddress>) -> bool {
    match address.into().unmapped() {
        IpAddress::V4(address) => {
            address.is_loopback()
                || ADAPTERS
                    .lock()
                    .iter()
                    .any(|adapter| adapter.interface.address() == Some(address))
        }
        IpAddress::V6(address) => {
            address.is_loopback()
                || ADAPTERS.lock().iter().any(|adapter| {
                    let ipv6 = adapter.interface.ipv6();
                    ipv6.link_local == address
                        || ipv6.global.is_some_and(|(global, _, _)| global == address)
                })
        }
    }
}

/// Joins (`join`) or leaves the IPv4 multicast `group` on every adapter (D577); `false` when
/// an adapter has no room for another group.
pub fn set_group(group: Ipv4Address, join: bool) -> bool {
    let mut adapters = ADAPTERS.lock();
    let mut joined = true;
    for adapter in adapters.iter_mut() {
        match join {
            true => joined &= adapter.interface.join_group(group),
            false => adapter.interface.leave_group(group),
        }
    }
    joined
}

/// The default adapter's address, if it has one.
pub fn local_address() -> Option<Ipv4Address> {
    default_adapter(&ADAPTERS.lock()).and_then(|adapter| adapter.interface.address())
}

/// The broadcast address of the default adapter's network, if it has an address.
pub fn subnet_broadcast() -> Option<Ipv4Address> {
    default_adapter(&ADAPTERS.lock())
        .and_then(|adapter| adapter.interface.subnet())
        .map(Subnet::broadcast)
}

/// Whether `address` is the broadcast address of any interface's network.
pub fn is_subnet_broadcast(address: Ipv4Address) -> bool {
    ADAPTERS
        .lock()
        .iter()
        .filter_map(|adapter| adapter.interface.subnet())
        .any(|subnet| subnet.broadcast() == address)
}

/// The DNS servers the default adapter's lease names, else the first lease's that names any.
pub fn dns_servers() -> [Option<Ipv4Address>; 3] {
    let adapters = ADAPTERS.lock();
    let of = |adapter: &Adapter| {
        adapter
            .interface
            .lease()
            .map(|lease| lease.dns)
            .filter(|dns| dns.iter().any(Option::is_some))
    };
    default_adapter(&adapters)
        .and_then(of)
        .or_else(|| adapters.iter().find_map(|adapter| of(adapter)))
        .unwrap_or([None; 3])
}

/// The address a packet to `destination` leaves from: `127.0.0.1` to the loopback network,
/// the address of the adapter its route leaves through otherwise (`0.0.0.0` without one).
pub fn source_for(destination: Ipv4Address) -> Ipv4Address {
    if destination.is_loopback() {
        return Ipv4Address::LOOPBACK;
    }
    let adapters = ADAPTERS.lock();
    route(&adapters, destination)
        .and_then(|index| adapters[index].interface.address())
        .unwrap_or(Ipv4Address::UNSPECIFIED)
}

/// [`source_for`] for either IP version (D565): `::1` to `::1`, the link-local or global
/// address of the adapter an IPv6 route leaves through, `::` without one.
pub fn source_for_any<P: Platform>(destination: IpAddress) -> IpAddress {
    match destination.unmapped() {
        IpAddress::V4(destination) => IpAddress::V4(source_for(destination)),
        IpAddress::V6(destination) if destination.is_loopback() => IpAddress::V6(destination),
        IpAddress::V6(destination) => {
            let now = now_ms::<P>();
            let adapters = ADAPTERS.lock();
            let source = route6(&adapters, destination, now)
                .map(|index| adapters[index].interface.ipv6_source_for(destination, now))
                .unwrap_or(Ipv6Address::UNSPECIFIED);
            IpAddress::V6(source)
        }
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
    source: impl Into<IpAddress>,
    source_port: u16,
    destination: (impl Into<IpAddress>, u16),
    data: &[u8],
    ttl: u8,
) -> Result<(), SendError> {
    let source = source.into();
    let (address, port) = (destination.0.into().unmapped(), destination.1);
    if is_local(address) {
        if data.len() > usize::from(u16::MAX) - 48 {
            return Err(SendError::TooLong);
        }
        let source = match source.is_unspecified() {
            true => source_for_any::<P>(address),
            false => source.unmapped(),
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
    let address = match address {
        IpAddress::V4(address) => address,
        IpAddress::V6(address) => {
            let mut adapters = ADAPTERS.lock();
            let index = route6(&adapters, address, now).ok_or(SendError::NoRoute)?;
            let Adapter {
                device, interface, ..
            } = &mut *adapters[index];
            return interface.send_udp6(source_port, address, port, data, ttl, now, &mut |bytes| {
                let _ = device.transmit(bytes);
            });
        }
    };
    let source = source.ipv4().unwrap_or(Ipv4Address::UNSPECIFIED);
    let mut adapters = ADAPTERS.lock();
    let chosen = adapters
        .iter()
        .position(|adapter| !source.is_unspecified() && adapter.interface.address() == Some(source))
        .or_else(|| route(&adapters, address));
    let Some(index) = chosen else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut *adapters[index];
    interface.send_udp(source_port, address, port, data, ttl, now, &mut |bytes| {
        let _ = device.transmit(bytes);
    })
}

/// Sends an echo request with `identifier`, `sequence` and `data` to `destination`, with time
/// to live `ttl`. One to this machine is answered at once: the reply goes to the ping
/// endpoint with `identifier`.
pub fn send_echo<P: Platform>(
    destination: impl Into<IpAddress>,
    identifier: u16,
    sequence: u16,
    data: &[u8],
    ttl: u8,
) -> Result<(), SendError> {
    let destination = destination.into().unmapped();
    if let IpAddress::V6(destination) = destination {
        return send_echo6::<P>(destination, identifier, sequence, data, ttl);
    }
    let IpAddress::V4(destination) = destination else {
        return Err(SendError::NoRoute);
    };
    if is_local(destination) {
        let mut reply = Vec::new();
        if reply
            .try_reserve_exact(icmp::HEADER_BYTES + data.len())
            .is_err()
        {
            return Err(SendError::TooLong);
        }
        reply.resize(icmp::HEADER_BYTES, 0);
        reply.extend_from_slice(data);
        icmp::write_echo_header(
            &mut reply,
            icmp::kind::ECHO_REPLY,
            identifier,
            sequence,
            data.len(),
        )
        .ok_or(SendError::TooLong)?;
        count_loopback(2 * (reply.len() + ipv4::HEADER_BYTES));
        let delivery = Delivery::EchoReply {
            source: IpAddress::V4(destination),
            identifier,
            message: &reply,
        };
        if endpoints::deliver(delivery) {
            console::wake_input_waiters::<P>();
        }
        return Ok(());
    }
    let now = now_ms::<P>();
    let mut adapters = ADAPTERS.lock();
    let Some(index) = route(&adapters, destination) else {
        return Err(SendError::NoRoute);
    };
    let Adapter {
        device, interface, ..
    } = &mut *adapters[index];
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

fn send_echo6<P: Platform>(
    destination: Ipv6Address,
    identifier: u16,
    sequence: u16,
    data: &[u8],
    hop_limit: u8,
) -> Result<(), SendError> {
    if is_local(destination) {
        let source = source_for_any::<P>(IpAddress::V6(destination))
            .ipv6()
            .unwrap_or(Ipv6Address::LOOPBACK);
        let mut reply = Vec::new();
        if reply.try_reserve_exact(8 + data.len()).is_err() {
            return Err(SendError::TooLong);
        }
        reply.resize(8 + data.len(), 0);
        eggos_net::icmpv6::write_echo(
            &mut reply,
            destination,
            source,
            eggos_net::icmpv6::kind::ECHO_REPLY,
            identifier,
            sequence,
            data,
        )
        .ok_or(SendError::TooLong)?;
        count_loopback(2 * (reply.len() + ipv6::HEADER_BYTES));
        let delivery = Delivery::EchoReply {
            source: IpAddress::V6(destination),
            identifier,
            message: &reply,
        };
        if endpoints::deliver(delivery) {
            console::wake_input_waiters::<P>();
        }
        return Ok(());
    }
    let now = now_ms::<P>();
    let mut adapters = ADAPTERS.lock();
    let index = route6(&adapters, destination, now).ok_or(SendError::NoRoute)?;
    let Adapter {
        device, interface, ..
    } = &mut *adapters[index];
    interface.send_echo6(
        destination,
        eggos_net::icmpv6::kind::ECHO_REQUEST,
        identifier,
        sequence,
        data,
        hop_limit,
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

fn adapter_status(adapter: &Adapter, now: u64) -> Option<InterfaceStatus> {
    let interface = &adapter.interface;
    let lease = interface.lease();
    Some(InterfaceStatus {
        name: InterfaceName::new(adapter.name)?,
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

/// Every interface's state at this moment: `lo`, then `eth0` and `wlan0` when there are those
/// adapters.
pub fn interfaces<P: Platform>() -> Vec<InterfaceStatus> {
    let now = now_ms::<P>();
    let adapters = ADAPTERS.lock();
    let mut interfaces = Vec::new();
    if interfaces.try_reserve_exact(1 + adapters.len()).is_ok() {
        interfaces.push(loopback_status());
        interfaces.extend(
            adapters
                .iter()
                .filter_map(|adapter| adapter_status(adapter, now)),
        );
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
    let adapters = ADAPTERS.lock();
    for adapter in adapters.iter() {
        let Some(name) = InterfaceName::new(adapter.name) else {
            continue;
        };
        for (address, mac) in adapter.interface.neighbours(now) {
            if neighbours.try_reserve(1).is_err() {
                return neighbours;
            }
            neighbours.push(Neighbour {
                address,
                mac,
                interface: name,
            });
        }
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

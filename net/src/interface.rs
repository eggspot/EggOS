//! One Ethernet interface's IPv4 stack: it answers ARP for its address and asks for the
//! addresses it sends to, queueing a few packets while it waits; it gets its address from its
//! DHCP client; it answers pings to its address; and it hands the UDP datagrams and the echo
//! replies that come for it to the caller. It routes to its own network directly and
//! everything else through the lease's router. The caller owns the device: it passes each
//! frame received to [`Interface::receive`], calls [`Interface::poll`] by
//! [`Interface::next_poll_ms`], and sends what both hand its `transmit` closure.

use crate::arp::{self, Cache};
use crate::dhcp::{self, Client, Event, Lease, Phase};
use crate::ethernet::{self, MacAddress, ether_type};
use crate::icmp;
use crate::ipv4::{self, Ipv4Address, Subnet};
use crate::status::Dhcp;
use crate::udp;

/// How many packets wait for an ARP answer at most; a new one replaces the oldest.
pub const PENDING_PACKETS: usize = 4;
/// How long the interface waits for an ARP answer before it asks again.
pub const ARP_RETRY_MS: u64 = 1000;
/// How many times it asks before it drops the packets waiting.
pub const ARP_ATTEMPTS: u8 = 3;

const IP_AT: usize = ethernet::HEADER_BYTES;
const PAYLOAD_AT: usize = IP_AT + ipv4::HEADER_BYTES;

/// What the interface counted, as Linux's interface statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// Frames received.
    pub received_packets: u64,
    /// Their bytes.
    pub received_bytes: u64,
    /// Frames that were malformed or failed a checksum.
    pub received_errors: u64,
    /// Frames that were not for this interface or carried something it does not speak.
    pub received_dropped: u64,
    /// Frames sent.
    pub sent_packets: u64,
    /// Their bytes.
    pub sent_bytes: u64,
    /// Packets dropped unsent: no ARP answer, or no room to wait for one.
    pub sent_dropped: u64,
}

/// A UDP datagram that came for the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Datagram<'a> {
    /// The sender's address.
    pub source: Ipv4Address,
    /// The sender's port.
    pub source_port: u16,
    /// The address it was sent to: this interface's, or a broadcast address.
    pub destination: Ipv4Address,
    /// The port it was sent to.
    pub destination_port: u16,
    /// The data.
    pub data: &'a [u8],
}

/// What the interface hands up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery<'a> {
    /// A UDP datagram.
    Udp(Datagram<'a>),
    /// An answer to a ping: the whole ICMP message, as a ping socket reads it.
    EchoReply {
        /// Who answered.
        source: Ipv4Address,
        /// The identifier of the conversation (the ping socket's).
        identifier: u16,
        /// The ICMP message, header and data.
        message: &'a [u8],
    },
}

/// Why a packet was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The link is down (no cable).
    LinkDown,
    /// The interface has no address yet.
    NoAddress,
    /// No route reaches the destination: off the network with no router, or a multicast
    /// address.
    NoRoute,
    /// The packet is longer than the link's MTU; fragments are not sent.
    TooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextHop {
    Broadcast,
    Via(Ipv4Address),
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    next_hop: Ipv4Address,
    length: usize,
    asked_ms: u64,
    attempts: u8,
    frame: [u8; ethernet::MAX_FRAME_BYTES],
}

/// One Ethernet interface's stack.
#[derive(Debug, Clone)]
pub struct Interface {
    mac: MacAddress,
    mtu: usize,
    link_up: bool,
    dhcp: Client,
    arp: Cache,
    pending: [Option<Pending>; PENDING_PACKETS],
    identification: u16,
    counters: Counters,
    frame: [u8; ethernet::MAX_FRAME_BYTES],
}

impl Interface {
    /// The stack of an adapter with hardware address `mac` and MTU `mtu` (at most 1500), its
    /// link up and its DHCP client about to ask; `seed` makes its numbers unpredictable.
    pub fn new(mac: MacAddress, mtu: usize, seed: u64) -> Self {
        Self {
            mac,
            mtu: mtu.min(ethernet::MAX_PAYLOAD_BYTES),
            link_up: true,
            dhcp: Client::new(mac, seed),
            arp: Cache::new(),
            pending: [None; PENDING_PACKETS],
            identification: (seed >> 16) as u16,
            counters: Counters::default(),
            frame: [0; ethernet::MAX_FRAME_BYTES],
        }
    }

    /// The adapter's hardware address.
    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    /// The largest IPv4 packet the link carries.
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    /// Whether the link is up.
    pub fn is_link_up(&self) -> bool {
        self.link_up
    }

    /// What it counted.
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// The lease its address comes from.
    pub fn lease(&self) -> Option<&Lease> {
        self.dhcp.lease()
    }

    /// Its address and network.
    pub fn subnet(&self) -> Option<Subnet> {
        self.lease().map(|lease| lease.subnet)
    }

    /// Its address.
    pub fn address(&self) -> Option<Ipv4Address> {
        self.subnet().map(|subnet| subnet.address)
    }

    /// Where its DHCP client is, in the words the status uses.
    pub fn dhcp(&self) -> Dhcp {
        match (self.dhcp.phase(), self.lease()) {
            (_, Some(_)) => Dhcp::Configured,
            (Phase::NoAnswer, None) => Dhcp::NoAddress,
            _ => Dhcp::Asking,
        }
    }

    /// The neighbours whose hardware addresses it knows at `now_ms`.
    pub fn neighbours(&self, now_ms: u64) -> impl Iterator<Item = (Ipv4Address, MacAddress)> + '_ {
        self.arp.entries(now_ms)
    }

    /// When it next wants [`Self::poll`] called, in milliseconds.
    pub fn next_poll_ms(&self) -> u64 {
        self.pending
            .iter()
            .flatten()
            .map(|pending| pending.asked_ms.saturating_add(ARP_RETRY_MS))
            .fold(self.dhcp.next_poll_ms(), u64::min)
    }

    /// Records whether the link is up at `now_ms`; when it comes back, the DHCP client checks
    /// its lease with the network again.
    pub fn set_link(&mut self, up: bool, now_ms: u64) {
        if up && !self.link_up {
            self.dhcp.restart(now_ms);
        }
        self.link_up = up;
    }

    fn is_for_us(&self, destination: Ipv4Address) -> bool {
        destination.is_broadcast()
            || self.subnet().is_some_and(|subnet| {
                destination == subnet.address || destination == subnet.broadcast()
            })
    }

    /// Takes in `frame`, received at `now_ms`: answers what it must through `transmit` and
    /// hands what came for the caller to `deliver`. Returns what changed in the DHCP client.
    pub fn receive(
        &mut self,
        frame: &[u8],
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
        deliver: &mut dyn FnMut(Delivery<'_>),
    ) -> Event {
        self.counters.received_packets += 1;
        self.counters.received_bytes += frame.len() as u64;
        let Some((header, payload)) = ethernet::parse(frame) else {
            self.counters.received_errors += 1;
            return Event::Nothing;
        };
        if header.destination != self.mac && !header.destination.is_broadcast() {
            self.counters.received_dropped += 1;
            return Event::Nothing;
        }
        match header.ether_type {
            ether_type::ARP => {
                self.receive_arp(payload, now_ms, transmit);
                Event::Nothing
            }
            ether_type::IPV4 => self.receive_ipv4(payload, now_ms, transmit, deliver),
            _ => {
                self.counters.received_dropped += 1;
                Event::Nothing
            }
        }
    }

    fn receive_arp(&mut self, payload: &[u8], now_ms: u64, transmit: &mut dyn FnMut(&[u8])) {
        let Some(packet) = arp::parse(payload) else {
            self.counters.received_errors += 1;
            return;
        };
        let ours = self.address() == Some(packet.target_ip);
        if !packet.sender_ip.is_unspecified() {
            match ours {
                true => self.arp.update(packet.sender_ip, packet.sender_mac, now_ms),
                false => {
                    self.arp.refresh(packet.sender_ip, packet.sender_mac, now_ms);
                }
            }
        }
        if ours && packet.operation == arp::operation::REQUEST {
            let reply = arp::Packet {
                operation: arp::operation::REPLY,
                sender_mac: self.mac,
                sender_ip: packet.target_ip,
                target_mac: packet.sender_mac,
                target_ip: packet.sender_ip,
            };
            self.send_arp(&reply, packet.sender_mac, transmit);
        }
        self.send_pending_to(packet.sender_ip, now_ms, transmit);
    }

    fn receive_ipv4(
        &mut self,
        payload: &[u8],
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
        deliver: &mut dyn FnMut(Delivery<'_>),
    ) -> Event {
        let Some((ip, body)) = ipv4::parse(payload) else {
            self.counters.received_errors += 1;
            return Event::Nothing;
        };
        match ip.protocol {
            ipv4::protocol::UDP => {
                let Some((header, data)) = udp::parse(ip.source, ip.destination, body) else {
                    self.counters.received_errors += 1;
                    return Event::Nothing;
                };
                if header.destination_port == dhcp::CLIENT_PORT
                    && header.source_port == dhcp::SERVER_PORT
                {
                    return self.dhcp.receive(data, now_ms);
                }
                if !self.is_for_us(ip.destination) {
                    self.counters.received_dropped += 1;
                    return Event::Nothing;
                }
                deliver(Delivery::Udp(Datagram {
                    source: ip.source,
                    source_port: header.source_port,
                    destination: ip.destination,
                    destination_port: header.destination_port,
                    data,
                }));
            }
            ipv4::protocol::ICMP if self.is_for_us(ip.destination) => {
                let Some(echo) = icmp::parse_echo(body) else {
                    self.counters.received_dropped += 1;
                    return Event::Nothing;
                };
                match echo.kind {
                    icmp::kind::ECHO_REQUEST if self.address() == Some(ip.destination) => {
                        let _ = self.send_echo(
                            ip.source,
                            icmp::kind::ECHO_REPLY,
                            echo.identifier,
                            echo.sequence,
                            echo.data,
                            ipv4::DEFAULT_TTL,
                            now_ms,
                            transmit,
                        );
                    }
                    icmp::kind::ECHO_REPLY => deliver(Delivery::EchoReply {
                        source: ip.source,
                        identifier: echo.identifier,
                        message: body,
                    }),
                    _ => {}
                }
            }
            _ => self.counters.received_dropped += 1,
        }
        Event::Nothing
    }

    /// Acts on the time at `now_ms`: the DHCP client's messages and timers, and ARP questions
    /// asked again or given up. Returns what changed in the DHCP client.
    pub fn poll(&mut self, now_ms: u64, transmit: &mut dyn FnMut(&[u8])) -> Event {
        let mut message = [0u8; dhcp::MESSAGE_BYTES];
        let (outgoing, event) = self.dhcp.poll(now_ms, &mut message);
        if let Some(outgoing) = outgoing
            && self.link_up
        {
            let _ = self.send_udp_from(
                outgoing.source,
                dhcp::CLIENT_PORT,
                outgoing.destination,
                dhcp::SERVER_PORT,
                &message[..outgoing.length],
                ipv4::DEFAULT_TTL,
                now_ms,
                transmit,
            );
        }
        for index in 0..PENDING_PACKETS {
            let Some(pending) = &mut self.pending[index] else {
                continue;
            };
            if now_ms < pending.asked_ms.saturating_add(ARP_RETRY_MS) {
                continue;
            }
            if pending.attempts >= ARP_ATTEMPTS {
                self.pending[index] = None;
                self.counters.sent_dropped += 1;
                continue;
            }
            pending.attempts += 1;
            pending.asked_ms = now_ms;
            let next_hop = pending.next_hop;
            self.ask(next_hop, transmit);
        }
        event
    }

    /// Sends `data` in a UDP datagram from port `source_port` of the interface's address to
    /// `destination`:`destination_port`, with time to live `ttl`.
    #[allow(clippy::too_many_arguments)]
    pub fn send_udp(
        &mut self,
        source_port: u16,
        destination: Ipv4Address,
        destination_port: u16,
        data: &[u8],
        ttl: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = match (self.address(), destination.is_broadcast()) {
            (Some(address), _) => address,
            (None, true) => Ipv4Address::UNSPECIFIED,
            (None, false) => return Err(SendError::NoAddress),
        };
        self.send_udp_from(
            source,
            source_port,
            destination,
            destination_port,
            data,
            ttl,
            now_ms,
            transmit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn send_udp_from(
        &mut self,
        source: Ipv4Address,
        source_port: u16,
        destination: Ipv4Address,
        destination_port: u16,
        data: &[u8],
        ttl: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let length = udp::HEADER_BYTES + data.len();
        let next_hop = self.route(destination, length)?;
        let datagram = &mut self.frame[PAYLOAD_AT..PAYLOAD_AT + length];
        datagram[udp::HEADER_BYTES..].copy_from_slice(data);
        let header = udp::Header {
            source_port,
            destination_port,
        };
        udp::write_header(datagram, source, destination, header, data.len())
            .ok_or(SendError::TooLong)?;
        let ip = ipv4::Header {
            source,
            destination,
            protocol: ipv4::protocol::UDP,
            ttl,
        };
        self.dispatch(&ip, length, next_hop, now_ms, transmit);
        Ok(())
    }

    /// Sends an echo message of `kind` with `identifier`, `sequence` and `data` from the
    /// interface's address to `destination`, with time to live `ttl`.
    #[allow(clippy::too_many_arguments)]
    pub fn send_echo(
        &mut self,
        destination: Ipv4Address,
        kind: u8,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        ttl: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = self.address().ok_or(SendError::NoAddress)?;
        let length = icmp::HEADER_BYTES + data.len();
        let next_hop = self.route(destination, length)?;
        let message = &mut self.frame[PAYLOAD_AT..PAYLOAD_AT + length];
        message[icmp::HEADER_BYTES..].copy_from_slice(data);
        icmp::write_echo_header(message, kind, identifier, sequence, data.len())
            .ok_or(SendError::TooLong)?;
        let ip = ipv4::Header {
            source,
            destination,
            protocol: ipv4::protocol::ICMP,
            ttl,
        };
        self.dispatch(&ip, length, next_hop, now_ms, transmit);
        Ok(())
    }

    fn route(&self, destination: Ipv4Address, payload_bytes: usize) -> Result<NextHop, SendError> {
        if !self.link_up {
            return Err(SendError::LinkDown);
        }
        if ipv4::HEADER_BYTES + payload_bytes > self.mtu {
            return Err(SendError::TooLong);
        }
        if destination.is_broadcast() {
            return Ok(NextHop::Broadcast);
        }
        let lease = self.lease().ok_or(SendError::NoAddress)?;
        if destination == lease.subnet.broadcast() {
            return Ok(NextHop::Broadcast);
        }
        if destination.is_multicast() || destination.is_unspecified() {
            return Err(SendError::NoRoute);
        }
        if lease.subnet.contains(destination) {
            return Ok(NextHop::Via(destination));
        }
        lease.router.map(NextHop::Via).ok_or(SendError::NoRoute)
    }

    fn dispatch(
        &mut self,
        ip: &ipv4::Header,
        payload_bytes: usize,
        next_hop: NextHop,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) {
        self.identification = self.identification.wrapping_add(1);
        let Some(header_bytes) =
            ipv4::write_header(&mut self.frame[IP_AT..], ip, self.identification, payload_bytes)
        else {
            return;
        };
        let length = IP_AT + header_bytes + payload_bytes;
        let destination = match next_hop {
            NextHop::Broadcast => Some(MacAddress::BROADCAST),
            NextHop::Via(hop) => self.arp.lookup(hop, now_ms),
        };
        let header = ethernet::Header {
            destination: destination.unwrap_or(MacAddress::ZERO),
            source: self.mac,
            ether_type: ether_type::IPV4,
        };
        ethernet::write_header(&mut self.frame, &header);
        let length = pad(&mut self.frame, length);
        match (destination, next_hop) {
            (Some(_), _) | (None, NextHop::Broadcast) => {
                self.counters.sent_packets += 1;
                self.counters.sent_bytes += length as u64;
                transmit(&self.frame[..length]);
            }
            (None, NextHop::Via(hop)) => self.wait_for_answer(hop, length, now_ms, transmit),
        }
    }

    fn wait_for_answer(
        &mut self,
        hop: Ipv4Address,
        length: usize,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) {
        let asked = self
            .pending
            .iter()
            .any(|slot| matches!(slot, Some(pending) if pending.next_hop == hop));
        let slot = match self.pending.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                self.counters.sent_dropped += 1;
                self.pending
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, slot)| slot.as_ref().map_or(0, |pending| pending.asked_ms))
                    .map_or(0, |(index, _)| index)
            }
        };
        self.pending[slot] = Some(Pending {
            next_hop: hop,
            length,
            asked_ms: now_ms,
            attempts: 1,
            frame: self.frame,
        });
        if !asked {
            self.ask(hop, transmit);
        }
    }

    fn send_pending_to(&mut self, hop: Ipv4Address, now_ms: u64, transmit: &mut dyn FnMut(&[u8])) {
        let Some(mac) = self.arp.lookup(hop, now_ms) else {
            return;
        };
        for slot in &mut self.pending {
            let Some(pending) = slot else {
                continue;
            };
            if pending.next_hop != hop {
                continue;
            }
            pending.frame[..6].copy_from_slice(&mac.0);
            self.counters.sent_packets += 1;
            self.counters.sent_bytes += pending.length as u64;
            transmit(&pending.frame[..pending.length]);
            *slot = None;
        }
    }

    fn ask(&mut self, hop: Ipv4Address, transmit: &mut dyn FnMut(&[u8])) {
        let request = arp::Packet {
            operation: arp::operation::REQUEST,
            sender_mac: self.mac,
            sender_ip: self.address().unwrap_or(Ipv4Address::UNSPECIFIED),
            target_mac: MacAddress::ZERO,
            target_ip: hop,
        };
        self.send_arp(&request, MacAddress::BROADCAST, transmit);
    }

    fn send_arp(&mut self, packet: &arp::Packet, to: MacAddress, transmit: &mut dyn FnMut(&[u8])) {
        let mut frame = [0u8; ethernet::MIN_FRAME_BYTES];
        let header = ethernet::Header {
            destination: to,
            source: self.mac,
            ether_type: ether_type::ARP,
        };
        ethernet::write_header(&mut frame, &header);
        arp::write(&mut frame[ethernet::HEADER_BYTES..], packet);
        self.counters.sent_packets += 1;
        self.counters.sent_bytes += frame.len() as u64;
        transmit(&frame);
    }
}

fn pad(frame: &mut [u8], length: usize) -> usize {
    match length < ethernet::MIN_FRAME_BYTES {
        true => {
            frame[length..ethernet::MIN_FRAME_BYTES].fill(0);
            ethernet::MIN_FRAME_BYTES
        }
        false => length,
    }
}

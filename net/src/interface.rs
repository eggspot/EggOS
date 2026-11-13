//! One Ethernet interface's IPv4 and IPv6 stack (IPv6 since D565: neighbour discovery, the
//! link-local address and SLAAC, ICMPv6 echo, UDP and TCP; see [`crate::ndp`]): it answers ARP for its address and asks for the
//! addresses it sends to, queueing a few packets while it waits; it gets its address from its
//! DHCP client; it answers pings to its address; and it hands the UDP datagrams, the TCP
//! segments and the echo replies that come for it to the caller, and sends the caller's
//! (TCP's connections are the caller's, [`crate::tcp`]). It routes to its own network directly and
//! everything else through the lease's router. The caller owns the device: it passes each
//! frame received to [`Interface::receive`], calls [`Interface::poll`] by
//! [`Interface::next_poll_ms`], and sends what both hand its `transmit` closure.

use crate::arp::{self, Cache};
use crate::checksum::Completion;
use crate::dhcp::{self, Client, Event, Lease, Phase};
use crate::ethernet::{self, MacAddress, ether_type};
use crate::icmp;
use crate::icmpv6;
use crate::ip::IpAddress;
use crate::ipv4::{self, Ipv4Address, Subnet};
use crate::ipv6::{self, Ipv6Address};
use crate::ndp;
use crate::status::Dhcp;
use crate::tcp::{self, Segment};
use crate::udp;

/// How many packets wait for an ARP answer at most; a new one replaces the oldest.
pub const PENDING_PACKETS: usize = 4;
/// How long the interface waits for an ARP answer before it asks again.
pub const ARP_RETRY_MS: u64 = 1000;
/// How many times it asks before it drops the packets waiting.
pub const ARP_ATTEMPTS: u8 = 3;

const IP_AT: usize = ethernet::HEADER_BYTES;
const PAYLOAD_AT: usize = IP_AT + ipv4::HEADER_BYTES;
const PAYLOAD6_AT: usize = IP_AT + ipv6::HEADER_BYTES;
/// Where the TCP segment lies in a frame [`Interface::frame_large_tcp`] completes: after the
/// Ethernet and IPv4 headers.
pub const LARGE_TCP_SEGMENT_AT: usize = PAYLOAD_AT;

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
    pub source: IpAddress,
    /// The sender's port.
    pub source_port: u16,
    /// The address it was sent to: this interface's, or a broadcast or multicast address.
    pub destination: IpAddress,
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
        source: IpAddress,
        identifier: u16,
        message: &'a [u8],
    },
    /// A TCP segment to this interface's address, its checksum checked.
    Tcp {
        source: IpAddress,
        destination: IpAddress,
        segment: Segment<'a>,
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
    next_hop: IpAddress,
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
    transmit_checksums: Completion,
    segmentation_limit: Option<usize>,
    frame: [u8; ethernet::MAX_FRAME_BYTES],
    ipv6: ndp::Addresses,
    neighbours6: ndp::Cache,
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
            transmit_checksums: Completion::Full,
            segmentation_limit: None,
            frame: [0; ethernet::MAX_FRAME_BYTES],
            ipv6: ndp::Addresses::new(mac),
            neighbours6: ndp::Cache::default(),
        }
    }

    /// Its IPv6 addresses and router (D565).
    pub fn ipv6(&self) -> &ndp::Addresses {
        &self.ipv6
    }

    /// The IPv6 neighbours whose hardware addresses it knows at `now_ms`.
    pub fn neighbours6(
        &self,
        now_ms: u64,
    ) -> impl Iterator<Item = (Ipv6Address, MacAddress)> + '_ {
        self.neighbours6.entries(now_ms)
    }

    /// Says the adapter cuts TCP segments longer than the MTU into segments itself, in frames
    /// of up to `limit` bytes (D541), or that it does not (`None`, the default).
    pub fn set_segmentation_limit(&mut self, limit: Option<usize>) {
        self.segmentation_limit = limit;
    }

    /// The longest frame the adapter cuts into TCP segments, if it does.
    pub fn segmentation_limit(&self) -> Option<usize> {
        self.segmentation_limit
    }

    /// The most TCP data one segment for the adapter to cut may hold: its frame limit less the
    /// Ethernet, IPv4 and TCP headers; `None` when it does not cut segments.
    pub fn segmentation_data_limit(&self) -> Option<usize> {
        self.segmentation_limit?
            .checked_sub(PAYLOAD_AT + tcp::MAX_HEADER_BYTES)
            .filter(|&bytes| bytes > 0)
    }

    /// Completes `frame`, whose bytes from [`LARGE_TCP_SEGMENT_AT`] hold a TCP segment of
    /// `segment_bytes` bytes from the interface's address to `destination`, longer than the
    /// MTU takes, for the adapter to cut into segments itself (D541): the segment's checksum,
    /// written as `completion` says, made partial, as the adapter completes each piece's, and
    /// the IPv4 and Ethernet headers written: the frame's length.
    /// [`SendError::TooLong`] when the adapter does not cut segments or the frame is longer
    /// than it takes; [`SendError::NoRoute`] while the next hop's hardware address is not
    /// known, which TCP's retransmission covers.
    #[allow(clippy::too_many_arguments)]
    pub fn frame_large_tcp(
        &mut self,
        frame: &mut [u8],
        destination: Ipv4Address,
        segment_bytes: usize,
        completion: Completion,
        ttl: u8,
        now_ms: u64,
    ) -> Result<usize, SendError> {
        let limit = self.segmentation_limit.ok_or(SendError::TooLong)?;
        let length = PAYLOAD_AT
            .checked_add(segment_bytes)
            .ok_or(SendError::TooLong)?;
        if length > limit || length > frame.len() {
            return Err(SendError::TooLong);
        }
        let source = self.address().ok_or(SendError::NoAddress)?;
        let NextHop::Via(hop) = self.route(destination, 0)? else {
            return Err(SendError::NoRoute);
        };
        let next_hop = self.arp.lookup(hop, now_ms).ok_or(SendError::NoRoute)?;
        if completion == Completion::Full {
            let segment = &mut frame[PAYLOAD_AT..length];
            tcp::convert_checksum(segment, source, destination, Completion::Partial);
        }
        self.identification = self.identification.wrapping_add(1);
        let ip = ipv4::Header {
            source,
            destination,
            protocol: ipv4::protocol::TCP,
            ttl,
        };
        ipv4::write_header(&mut frame[IP_AT..], &ip, self.identification, segment_bytes)
            .ok_or(SendError::TooLong)?;
        let header = ethernet::Header {
            destination: next_hop,
            source: self.mac,
            ether_type: ether_type::IPV4,
        };
        ethernet::write_header(frame, &header);
        self.counters.sent_packets += 1;
        self.counters.sent_bytes += length as u64;
        Ok(length)
    }

    /// Has every TCP and UDP checksum the interface sends go out whole, or partial for an
    /// adapter that completes them itself (D535); whole until this says otherwise.
    pub fn set_transmit_checksums(&mut self, completion: Completion) {
        self.transmit_checksums = completion;
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
            .min(match self.link_up {
                true => self.ipv6.next_solicitation_ms(),
                false => u64::MAX,
            })
    }

    /// Records whether the link is up at `now_ms`; when it comes back, the DHCP client checks
    /// its lease with the network again.
    pub fn set_link(&mut self, up: bool, now_ms: u64) {
        if up && !self.link_up {
            self.dhcp.restart(now_ms);
            self.ipv6.restart();
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
        let ipv6_multicast = header.destination.0[..2] == [0x33, 0x33];
        if header.destination != self.mac && !header.destination.is_broadcast() && !ipv6_multicast
        {
            self.counters.received_dropped += 1;
            return Event::Nothing;
        }
        match header.ether_type {
            ether_type::ARP => {
                self.receive_arp(payload, now_ms, transmit);
                Event::Nothing
            }
            ether_type::IPV4 => self.receive_ipv4(payload, now_ms, transmit, deliver),
            ether_type::IPV6 => {
                self.receive_ipv6(payload, header.source, now_ms, transmit, deliver);
                Event::Nothing
            }
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
                    self.arp
                        .refresh(packet.sender_ip, packet.sender_mac, now_ms);
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
        self.send_pending_to(IpAddress::V4(packet.sender_ip), now_ms, transmit);
    }

    fn receive_ipv6(
        &mut self,
        payload: &[u8],
        sender: MacAddress,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
        deliver: &mut dyn FnMut(Delivery<'_>),
    ) {
        let Some(packet) = ipv6::parse(payload) else {
            self.counters.received_dropped += 1;
            return;
        };
        let ip = packet.header;
        if !self.ipv6.is_ours(ip.destination) {
            self.counters.received_dropped += 1;
            return;
        }
        let (source, destination) = (IpAddress::V6(ip.source), IpAddress::V6(ip.destination));
        let unicast = !ip.destination.is_multicast();
        match ip.protocol {
            ipv6::next_header::ICMPV6 => {
                let Some(message) =
                    icmpv6::parse(ip.source, ip.destination, ip.hop_limit, packet.payload)
                else {
                    self.counters.received_errors += 1;
                    return;
                };
                self.receive_icmpv6(message, &ip, sender, packet.payload, now_ms, transmit, deliver);
            }
            ipv6::next_header::UDP => {
                let Some((header, data)) = udp::parse(source, destination, packet.payload) else {
                    self.counters.received_errors += 1;
                    return;
                };
                deliver(Delivery::Udp(Datagram {
                    source,
                    source_port: header.source_port,
                    destination,
                    destination_port: header.destination_port,
                    data,
                }));
            }
            ipv6::next_header::TCP if unicast => {
                let Some(segment) = tcp::parse(source, destination, packet.payload) else {
                    self.counters.received_errors += 1;
                    return;
                };
                deliver(Delivery::Tcp {
                    source,
                    destination,
                    segment,
                });
            }
            _ => self.counters.received_dropped += 1,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn receive_icmpv6(
        &mut self,
        message: icmpv6::Message<'_>,
        ip: &ipv6::Header,
        sender: MacAddress,
        body: &[u8],
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
        deliver: &mut dyn FnMut(Delivery<'_>),
    ) {
        match message {
            icmpv6::Message::EchoRequest {
                identifier,
                sequence,
                data,
            } if !ip.destination.is_multicast() => {
                let _ = self.send_echo6(
                    ip.source,
                    icmpv6::kind::ECHO_REPLY,
                    identifier,
                    sequence,
                    data,
                    ipv6::DEFAULT_HOP_LIMIT,
                    now_ms,
                    transmit,
                );
            }
            icmpv6::Message::EchoReply { identifier, .. } => deliver(Delivery::EchoReply {
                source: IpAddress::V6(ip.source),
                identifier,
                message: body,
            }),
            icmpv6::Message::NeighbourSolicitation(neighbour) => {
                if !ip.source.is_unspecified() {
                    let link = neighbour.link.unwrap_or(sender);
                    self.neighbours6.learn(ip.source, link, now_ms);
                    self.send_pending_to(IpAddress::V6(ip.source), now_ms, transmit);
                }
                let ours = neighbour.target == self.ipv6.link_local
                    || self
                        .ipv6
                        .global
                        .is_some_and(|(global, _, _)| global == neighbour.target);
                if ours {
                    let (to, solicited) = match ip.source.is_unspecified() {
                        true => (Ipv6Address::ALL_NODES, false),
                        false => (ip.source, true),
                    };
                    let mut message = [0u8; 32];
                    if let Some(length) = icmpv6::write_neighbour_advertisement(
                        &mut message,
                        neighbour.target,
                        to,
                        neighbour.target,
                        self.mac,
                        solicited,
                    ) {
                        let _ = self.send_icmpv6(neighbour.target, to, &message[..length], now_ms, transmit);
                    }
                }
            }
            icmpv6::Message::NeighbourAdvertisement(neighbour) => {
                if let Some(link) = neighbour.link {
                    self.neighbours6.learn(neighbour.target, link, now_ms);
                    self.send_pending_to(IpAddress::V6(neighbour.target), now_ms, transmit);
                }
            }
            icmpv6::Message::RouterAdvertisement(advertisement) => {
                let link = advertisement.link.unwrap_or(sender);
                self.neighbours6.learn(ip.source, link, now_ms);
                self.ipv6
                    .take_advertisement(ip.source, &advertisement, self.mac, now_ms);
                self.send_pending_to(IpAddress::V6(ip.source), now_ms, transmit);
            }
            _ => {}
        }
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
                    source: IpAddress::V4(ip.source),
                    source_port: header.source_port,
                    destination: IpAddress::V4(ip.destination),
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
                        source: IpAddress::V4(ip.source),
                        identifier: echo.identifier,
                        message: body,
                    }),
                    _ => {}
                }
            }
            ipv4::protocol::TCP if self.address() == Some(ip.destination) => {
                let Some(segment) = tcp::parse(ip.source, ip.destination, body) else {
                    self.counters.received_errors += 1;
                    return Event::Nothing;
                };
                deliver(Delivery::Tcp {
                    source: IpAddress::V4(ip.source),
                    destination: IpAddress::V4(ip.destination),
                    segment,
                });
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
        if self.link_up && self.ipv6.solicitation_due(now_ms) {
            let source = self.ipv6.link_local;
            let mut message = [0u8; 16];
            if let Some(length) =
                icmpv6::write_router_solicitation(&mut message, source, Some(self.mac))
            {
                let _ = self.send_icmpv6(
                    source,
                    Ipv6Address::ALL_ROUTERS,
                    &message[..length],
                    now_ms,
                    transmit,
                );
            }
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
        let completion = self.transmit_checksums;
        udp::write_header_with(
            datagram,
            source,
            destination,
            header,
            data.len(),
            completion,
        )
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

    /// Sends `segment`, a TCP segment whose checksum was made for the interface's address and
    /// `destination` ([`tcp::write_header_with`]), whole or partial as `completion` says, to
    /// `destination`, with time to live `ttl`; its checksum is made whole or partial first
    /// when the interface sends them the other way.
    pub fn send_tcp(
        &mut self,
        destination: Ipv4Address,
        segment: &[u8],
        completion: Completion,
        ttl: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = self.address().ok_or(SendError::NoAddress)?;
        let next_hop = match self.route(destination, segment.len())? {
            NextHop::Broadcast => return Err(SendError::NoRoute),
            hop => hop,
        };
        let carried = &mut self.frame[PAYLOAD_AT..PAYLOAD_AT + segment.len()];
        carried.copy_from_slice(segment);
        if completion != self.transmit_checksums {
            tcp::convert_checksum(carried, source, destination, self.transmit_checksums);
        }
        let ip = ipv4::Header {
            source,
            destination,
            protocol: ipv4::protocol::TCP,
            ttl,
        };
        self.dispatch(&ip, segment.len(), next_hop, now_ms, transmit);
        Ok(())
    }

    /// The largest TCP segment size the link carries: its MTU less the IPv4 and TCP headers.
    pub fn tcp_mss(&self) -> u16 {
        self.mtu
            .saturating_sub(ipv4::HEADER_BYTES + tcp::HEADER_BYTES) as u16
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
        let Some(header_bytes) = ipv4::write_header(
            &mut self.frame[IP_AT..],
            ip,
            self.identification,
            payload_bytes,
        ) else {
            return;
        };
        let length = IP_AT + header_bytes + payload_bytes;
        let destination = match next_hop {
            NextHop::Broadcast => Some(MacAddress::BROADCAST),
            NextHop::Via(hop) => self.arp.lookup(hop, now_ms),
        };
        let next_hop = match next_hop {
            NextHop::Via(hop) => Some(IpAddress::V4(hop)),
            NextHop::Broadcast => None,
        };
        let header = ethernet::Header {
            destination: destination.unwrap_or(MacAddress::ZERO),
            source: self.mac,
            ether_type: ether_type::IPV4,
        };
        ethernet::write_header(&mut self.frame, &header);
        let length = pad(&mut self.frame, length);
        match (destination, next_hop) {
            (Some(_), _) | (None, None) => {
                self.counters.sent_packets += 1;
                self.counters.sent_bytes += length as u64;
                transmit(&self.frame[..length]);
            }
            (None, Some(hop)) => self.wait_for_answer(hop, length, now_ms, transmit),
        }
    }

    fn wait_for_answer(
        &mut self,
        hop: IpAddress,
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

    fn send_pending_to(&mut self, hop: IpAddress, now_ms: u64, transmit: &mut dyn FnMut(&[u8])) {
        let known = match hop {
            IpAddress::V4(hop) => self.arp.lookup(hop, now_ms),
            IpAddress::V6(hop) => self.neighbours6.lookup(hop, now_ms),
        };
        let Some(mac) = known else {
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

    fn ask(&mut self, hop: IpAddress, transmit: &mut dyn FnMut(&[u8])) {
        let hop = match hop {
            IpAddress::V4(hop) => hop,
            IpAddress::V6(hop) => {
                let source = self.ipv6.link_local;
                let group = hop.solicited_node();
                let mut message = [0u8; 32];
                if let Some(length) = icmpv6::write_neighbour_solicitation(
                    &mut message,
                    source,
                    group,
                    hop,
                    Some(self.mac),
                ) {
                    let mut frame = [0u8; ethernet::MIN_FRAME_BYTES + 32];
                    if let Some(total) = self.frame_icmpv6(&mut frame, source, group, &message[..length]) {
                        self.counters.sent_packets += 1;
                        self.counters.sent_bytes += total as u64;
                        transmit(&frame[..total]);
                    }
                }
                return;
            }
        };
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

impl Interface {
    fn frame_icmpv6(
        &self,
        frame: &mut [u8],
        source: Ipv6Address,
        destination: Ipv6Address,
        message: &[u8],
    ) -> Option<usize> {
        let at = ethernet::HEADER_BYTES + ipv6::HEADER_BYTES;
        frame.get_mut(at..at + message.len())?.copy_from_slice(message);
        let header = ipv6::Header {
            source,
            destination,
            protocol: ipv6::next_header::ICMPV6,
            hop_limit: 255,
        };
        let packet = ipv6::write_header(&mut frame[ethernet::HEADER_BYTES..], &header, message.len())?;
        let ethernet_header = ethernet::Header {
            destination: destination.multicast_mac(),
            source: self.mac,
            ether_type: ether_type::IPV6,
        };
        ethernet::write_header(frame, &ethernet_header);
        Some(pad(frame, ethernet::HEADER_BYTES + packet))
    }

    fn send_icmpv6(
        &mut self,
        source: Ipv6Address,
        destination: Ipv6Address,
        message: &[u8],
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let payload = self
            .frame
            .get_mut(PAYLOAD6_AT..PAYLOAD6_AT + message.len())
            .ok_or(SendError::TooLong)?;
        payload.copy_from_slice(message);
        let header = ipv6::Header {
            source,
            destination,
            protocol: ipv6::next_header::ICMPV6,
            hop_limit: 255,
        };
        self.dispatch6(&header, message.len(), now_ms, transmit)
    }

    /// Its IPv6 address to send to `destination` from (D565).
    pub fn ipv6_source_for(&self, destination: Ipv6Address, now_ms: u64) -> Ipv6Address {
        self.ipv6.source_for(destination, now_ms)
    }

    /// Sends `data` in a UDP datagram from port `source_port` of its IPv6 address to
    /// `destination`:`destination_port`, with hop limit `hop_limit` (D565).
    #[allow(clippy::too_many_arguments)]
    pub fn send_udp6(
        &mut self,
        source_port: u16,
        destination: Ipv6Address,
        destination_port: u16,
        data: &[u8],
        hop_limit: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = self.ipv6.source_for(destination, now_ms);
        let length = udp::HEADER_BYTES + data.len();
        let datagram = self
            .frame
            .get_mut(PAYLOAD6_AT..PAYLOAD6_AT + length)
            .ok_or(SendError::TooLong)?;
        datagram[udp::HEADER_BYTES..].copy_from_slice(data);
        let header = udp::Header {
            source_port,
            destination_port,
        };
        udp::write_header_with(
            datagram,
            source,
            destination,
            header,
            data.len(),
            self.transmit_checksums,
        )
        .ok_or(SendError::TooLong)?;
        let ip = ipv6::Header {
            source,
            destination,
            protocol: ipv6::next_header::UDP,
            hop_limit,
        };
        self.dispatch6(&ip, length, now_ms, transmit)
    }

    /// Sends an ICMPv6 echo message of `kind` from its IPv6 address to `destination` (D565).
    #[allow(clippy::too_many_arguments)]
    pub fn send_echo6(
        &mut self,
        destination: Ipv6Address,
        kind: u8,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        hop_limit: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = self.ipv6.source_for(destination, now_ms);
        let length = 8 + data.len();
        let message = self
            .frame
            .get_mut(PAYLOAD6_AT..PAYLOAD6_AT + length)
            .ok_or(SendError::TooLong)?;
        icmpv6::write_echo(message, source, destination, kind, identifier, sequence, data)
            .ok_or(SendError::TooLong)?;
        let ip = ipv6::Header {
            source,
            destination,
            protocol: ipv6::next_header::ICMPV6,
            hop_limit,
        };
        self.dispatch6(&ip, length, now_ms, transmit)
    }

    /// Sends `segment`, a TCP segment whose checksum was made between its IPv6 address for
    /// `destination` and `destination`, as [`Self::send_tcp`] does over IPv4 (D565).
    pub fn send_tcp6(
        &mut self,
        destination: Ipv6Address,
        segment: &[u8],
        completion: Completion,
        hop_limit: u8,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        let source = self.ipv6.source_for(destination, now_ms);
        let carried = self
            .frame
            .get_mut(PAYLOAD6_AT..PAYLOAD6_AT + segment.len())
            .ok_or(SendError::TooLong)?;
        carried.copy_from_slice(segment);
        if completion != self.transmit_checksums {
            tcp::convert_checksum(carried, source, destination, self.transmit_checksums);
        }
        let ip = ipv6::Header {
            source,
            destination,
            protocol: ipv6::next_header::TCP,
            hop_limit,
        };
        self.dispatch6(&ip, segment.len(), now_ms, transmit)
    }

    /// The largest TCP segment size the link carries over IPv6.
    pub fn tcp_mss6(&self) -> u16 {
        let mtu = self
            .ipv6
            .mtu
            .map_or(self.mtu, |mtu| (mtu as usize).min(self.mtu));
        mtu.saturating_sub(ipv6::HEADER_BYTES + tcp::HEADER_BYTES) as u16
    }

    fn dispatch6(
        &mut self,
        ip: &ipv6::Header,
        payload_bytes: usize,
        now_ms: u64,
        transmit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), SendError> {
        if !self.link_up {
            return Err(SendError::LinkDown);
        }
        if ipv6::HEADER_BYTES + payload_bytes > self.mtu {
            return Err(SendError::TooLong);
        }
        let destination = ip.destination;
        let hop = match () {
            _ if destination.is_multicast() => None,
            _ if self.ipv6.is_on_link(destination) => Some(destination),
            _ => Some(self.ipv6.router(now_ms).ok_or(SendError::NoRoute)?),
        };
        ipv6::write_header(&mut self.frame[IP_AT..], ip, payload_bytes).ok_or(SendError::TooLong)?;
        let mac = match hop {
            None => Some(destination.multicast_mac()),
            Some(hop) => self.neighbours6.lookup(hop, now_ms),
        };
        let header = ethernet::Header {
            destination: mac.unwrap_or(MacAddress::ZERO),
            source: self.mac,
            ether_type: ether_type::IPV6,
        };
        ethernet::write_header(&mut self.frame, &header);
        let length = pad(&mut self.frame, PAYLOAD6_AT + payload_bytes);
        match (mac, hop) {
            (Some(_), _) | (None, None) => {
                self.counters.sent_packets += 1;
                self.counters.sent_bytes += length as u64;
                transmit(&self.frame[..length]);
            }
            (None, Some(hop)) => self.wait_for_answer(IpAddress::V6(hop), length, now_ms, transmit),
        }
        Ok(())
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

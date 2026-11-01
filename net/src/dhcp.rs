//! A DHCP client (RFC 2131 and RFC 2132): it asks the network for an address (`DISCOVER`),
//! takes the first offer (`OFFER`, `REQUEST`, `ACK`), renews the lease halfway through it with
//! the server that gave it, asks any server once seven eighths are gone, and starts again when
//! the lease ends or a server refuses it (`NAK`). Requests are sent again after 4, 8, 16 and
//! 32 seconds (each plus up to a second chosen at random, the RFC's randomisation); after
//! [`DISCOVER_ATTEMPTS`] unanswered `DISCOVER`s the client rests for
//! [`RETRY_AFTER_NO_ANSWER_MS`] and starts again, for as long as it runs.
//!
//! The client only builds and reads messages: [`Client::poll`] says when one is due and where
//! it goes, and the interface ([`crate::interface`]) sends it over UDP from port 68 to port 67.

use crate::ethernet::MacAddress;
use crate::ipv4::{Ipv4Address, Subnet, prefix_of_mask};

/// The client's UDP port.
pub const CLIENT_PORT: u16 = 68;
/// The servers' UDP port.
pub const SERVER_PORT: u16 = 67;
/// Bytes of a message the client sends: BOOTP's minimum, which some servers insist on.
pub const MESSAGE_BYTES: usize = 300;
/// How long the client waits for the first answer before it asks again.
pub const FIRST_RETRY_MS: u64 = 4_000;
/// The longest wait between two tries.
pub const LAST_RETRY_MS: u64 = 64_000;
/// How many `DISCOVER`s go unanswered before the client rests.
pub const DISCOVER_ATTEMPTS: u8 = 4;
/// How many `REQUEST`s for an offer go unanswered before the client asks for offers again.
pub const REQUEST_ATTEMPTS: u8 = 4;
/// How long the client rests after no server answered.
pub const RETRY_AFTER_NO_ANSWER_MS: u64 = 60_000;
/// The shortest wait between two tries to renew or rebind a lease.
pub const MIN_RENEW_RETRY_MS: u64 = 60_000;
/// A lease's length that means it never ends.
pub const INFINITE_LEASE_S: u32 = u32::MAX;

const DEFAULT_LEASE_S: u32 = 86_400;
const OPTIONS_OFFSET: usize = 240;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const BOOT_REQUEST: u8 = 1;
const BOOT_REPLY: u8 = 2;
const HARDWARE_ETHERNET: u8 = 1;
const BROADCAST_FLAG: u16 = 0x8000;
const RANDOM_SPREAD_MS: u64 = 1000;

/// The message types (option 53).
pub mod message_type {
    /// A client looks for servers.
    pub const DISCOVER: u8 = 1;
    /// A server offers an address.
    pub const OFFER: u8 = 2;
    /// A client asks for an offered address, or to keep its lease.
    pub const REQUEST: u8 = 3;
    /// A server grants the request.
    pub const ACK: u8 = 5;
    /// A server refuses it.
    pub const NAK: u8 = 6;
}

mod option {
    pub const PAD: u8 = 0;
    pub const SUBNET_MASK: u8 = 1;
    pub const ROUTER: u8 = 3;
    pub const DNS: u8 = 6;
    pub const REQUESTED_ADDRESS: u8 = 50;
    pub const LEASE_TIME: u8 = 51;
    pub const MESSAGE_TYPE: u8 = 53;
    pub const SERVER_ID: u8 = 54;
    pub const PARAMETERS: u8 = 55;
    pub const RENEWAL_TIME: u8 = 58;
    pub const REBINDING_TIME: u8 = 59;
    pub const END: u8 = 255;
}

const PARAMETERS: [u8; 6] = [
    option::SUBNET_MASK,
    option::ROUTER,
    option::DNS,
    option::LEASE_TIME,
    option::RENEWAL_TIME,
    option::REBINDING_TIME,
];

/// What a client asks in a message it sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// [`message_type::DISCOVER`] or [`message_type::REQUEST`].
    pub message_type: u8,
    /// The transaction the answers carry back.
    pub transaction: u32,
    /// Seconds since the client started asking.
    pub seconds: u16,
    /// Whether servers should broadcast their answers.
    pub broadcast: bool,
    /// The address the client has and wants to keep (`ciaddr`), when renewing or rebinding.
    pub client_address: Ipv4Address,
    /// The address it asks for (option 50).
    pub requested: Option<Ipv4Address>,
    /// The server whose offer it takes (option 54).
    pub server: Option<Ipv4Address>,
}

/// Writes `request` from the adapter with hardware address `mac` to the start of `out`:
/// [`MESSAGE_BYTES`], or 0 when `out` is shorter.
pub fn write_request(out: &mut [u8], mac: MacAddress, request: &Request) -> usize {
    let Some(message) = out.get_mut(..MESSAGE_BYTES) else {
        return 0;
    };
    message.fill(0);
    message[0] = BOOT_REQUEST;
    message[1] = HARDWARE_ETHERNET;
    message[2] = 6;
    message[4..8].copy_from_slice(&request.transaction.to_be_bytes());
    message[8..10].copy_from_slice(&request.seconds.to_be_bytes());
    let flags = if request.broadcast { BROADCAST_FLAG } else { 0 };
    message[10..12].copy_from_slice(&flags.to_be_bytes());
    message[12..16].copy_from_slice(&request.client_address.0);
    message[28..34].copy_from_slice(&mac.0);
    message[236..240].copy_from_slice(&MAGIC_COOKIE);
    let mut at = OPTIONS_OFFSET;
    let mut put = |bytes: &[u8]| {
        message[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&[option::MESSAGE_TYPE, 1, request.message_type]);
    if let Some(requested) = request.requested {
        put(&[option::REQUESTED_ADDRESS, 4]);
        put(&requested.0);
    }
    if let Some(server) = request.server {
        put(&[option::SERVER_ID, 4]);
        put(&server.0);
    }
    put(&[option::PARAMETERS, PARAMETERS.len() as u8]);
    put(&PARAMETERS);
    put(&[option::END]);
    MESSAGE_BYTES
}

/// What a server's answer says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    /// [`message_type::OFFER`], [`message_type::ACK`] or [`message_type::NAK`].
    pub message_type: u8,
    /// The address offered or granted (`yiaddr`).
    pub your_address: Ipv4Address,
    /// The server (option 54).
    pub server: Option<Ipv4Address>,
    /// The network mask (option 1).
    pub subnet_mask: Option<Ipv4Address>,
    /// The first router (option 3).
    pub router: Option<Ipv4Address>,
    /// The first three DNS servers (option 6).
    pub dns: [Option<Ipv4Address>; 3],
    /// The lease's length in seconds (option 51).
    pub lease_s: Option<u32>,
    /// When to renew, in seconds (option 58).
    pub renewal_s: Option<u32>,
    /// When to rebind, in seconds (option 59).
    pub rebinding_s: Option<u32>,
}

/// The answer `message` holds for transaction `transaction` of the adapter with hardware
/// address `mac`; `None` for anything else.
pub fn parse_reply(message: &[u8], transaction: u32, mac: MacAddress) -> Option<Reply> {
    if message.len() < OPTIONS_OFFSET
        || message[0] != BOOT_REPLY
        || message[1] != HARDWARE_ETHERNET
        || message[2] != 6
        || message[4..8] != transaction.to_be_bytes()
        || message[28..34] != mac.0
        || message[236..240] != MAGIC_COOKIE
    {
        return None;
    }
    let mut reply = Reply {
        message_type: 0,
        your_address: Ipv4Address([message[16], message[17], message[18], message[19]]),
        server: None,
        subnet_mask: None,
        router: None,
        dns: [None; 3],
        lease_s: None,
        renewal_s: None,
        rebinding_s: None,
    };
    let mut at = OPTIONS_OFFSET;
    while let Some(&code) = message.get(at) {
        match code {
            option::PAD => {
                at += 1;
                continue;
            }
            option::END => break,
            _ => {}
        }
        let length = usize::from(*message.get(at + 1)?);
        let value = message.get(at + 2..at + 2 + length)?;
        read_option(&mut reply, code, value);
        at += 2 + length;
    }
    matches!(
        reply.message_type,
        message_type::OFFER | message_type::ACK | message_type::NAK
    )
    .then_some(reply)
}

fn read_option(reply: &mut Reply, code: u8, value: &[u8]) {
    let address = |at: usize| {
        value
            .get(at..at + 4)
            .map(|bytes| Ipv4Address([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    let seconds = || {
        value
            .first_chunk::<4>()
            .map(|bytes| u32::from_be_bytes(*bytes))
    };
    match code {
        option::MESSAGE_TYPE => reply.message_type = value.first().copied().unwrap_or(0),
        option::SUBNET_MASK => reply.subnet_mask = address(0),
        option::ROUTER => reply.router = address(0),
        option::DNS => {
            for (index, server) in reply.dns.iter_mut().enumerate() {
                *server = address(index * 4);
            }
        }
        option::SERVER_ID => reply.server = address(0),
        option::LEASE_TIME => reply.lease_s = seconds(),
        option::RENEWAL_TIME => reply.renewal_s = seconds(),
        option::REBINDING_TIME => reply.rebinding_s = seconds(),
        _ => {}
    }
}

/// An address the client holds, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// The address and its network.
    pub subnet: Subnet,
    /// The router off the network.
    pub router: Option<Ipv4Address>,
    /// Up to three DNS servers.
    pub dns: [Option<Ipv4Address>; 3],
    /// The server that granted it.
    pub server: Ipv4Address,
    /// Its length in seconds ([`INFINITE_LEASE_S`]: it never ends).
    pub lease_s: u32,
    /// When to renew, in seconds from when it was granted.
    pub renewal_s: u32,
    /// When to ask any server, in seconds from when it was granted.
    pub rebinding_s: u32,
    /// When it was granted, in milliseconds.
    pub granted_ms: u64,
}

impl Lease {
    fn at(&self, seconds: u32) -> u64 {
        match self.lease_s {
            INFINITE_LEASE_S => u64::MAX,
            _ => self
                .granted_ms
                .saturating_add(u64::from(seconds).saturating_mul(1000)),
        }
    }

    /// When it ends, in milliseconds; `u64::MAX` for never.
    pub fn expires_ms(&self) -> u64 {
        self.at(self.lease_s)
    }

    /// Whole seconds left at `now_ms`; [`INFINITE_LEASE_S`] for a lease that never ends.
    pub fn left_s(&self, now_ms: u64) -> u32 {
        match self.lease_s {
            INFINITE_LEASE_S => INFINITE_LEASE_S,
            _ => (self.expires_ms().saturating_sub(now_ms) / 1000) as u32,
        }
    }
}

/// Where the client is in getting and keeping an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Looking for servers (`DISCOVER`).
    Selecting,
    /// Asking for an offered address (`REQUEST`).
    Requesting,
    /// Holding a lease.
    Bound,
    /// Asking the server that granted the lease to extend it.
    Renewing,
    /// Asking any server to extend it.
    Rebinding,
    /// No server answered: resting before asking again.
    NoAnswer,
}

/// A message the client wants sent: its length at the start of the buffer [`Client::poll`]
/// filled, and the addresses it goes between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outgoing {
    /// Its bytes.
    pub length: usize,
    /// The source address: `0.0.0.0` until the client holds a lease.
    pub source: Ipv4Address,
    /// Where it goes: `255.255.255.255`, or the server when renewing.
    pub destination: Ipv4Address,
}

/// What changed in the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Nothing the interface must act on.
    Nothing,
    /// A server granted or extended a lease.
    Bound,
    /// The lease ended or a server refused it: the address is gone.
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Offer {
    address: Ipv4Address,
    server: Ipv4Address,
}

/// The client of one adapter.
#[derive(Debug, Clone)]
pub struct Client {
    mac: MacAddress,
    phase: Phase,
    transaction: u32,
    random: u64,
    attempts: u8,
    next_send_ms: u64,
    started_ms: u64,
    offer: Option<Offer>,
    lease: Option<Lease>,
}

impl Client {
    /// A client for the adapter with hardware address `mac`, about to look for servers;
    /// `seed` picks its transaction numbers and spreads its retries.
    pub fn new(mac: MacAddress, seed: u64) -> Self {
        let mut client = Self {
            mac,
            phase: Phase::Selecting,
            transaction: 0,
            random: seed | 1,
            attempts: 0,
            next_send_ms: 0,
            started_ms: 0,
            offer: None,
            lease: None,
        };
        client.new_transaction();
        client
    }

    /// Where it is.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The lease it holds.
    pub fn lease(&self) -> Option<&Lease> {
        self.lease.as_ref()
    }

    /// When it next wants [`Self::poll`] called, in milliseconds (`u64::MAX`: only when a
    /// lease time passes).
    pub fn next_poll_ms(&self) -> u64 {
        let lease = self.lease.map_or(u64::MAX, |lease| match self.phase {
            Phase::Bound => lease.at(lease.renewal_s),
            Phase::Renewing => lease.at(lease.rebinding_s),
            _ => lease.expires_ms(),
        });
        self.next_send_ms.min(lease)
    }

    fn next_random(&mut self) -> u64 {
        let mut random = self.random;
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        self.random = random;
        random
    }

    fn new_transaction(&mut self) {
        self.transaction = self.next_random() as u32;
    }

    fn retry_delay(&mut self) -> u64 {
        let shift = u32::from(self.attempts.saturating_sub(1)).min(8);
        let delay = (FIRST_RETRY_MS << shift).min(LAST_RETRY_MS);
        delay + self.next_random() % RANDOM_SPREAD_MS
    }

    fn start_selecting(&mut self, now_ms: u64) {
        self.phase = Phase::Selecting;
        self.offer = None;
        self.attempts = 0;
        self.next_send_ms = now_ms;
        self.started_ms = now_ms;
        self.new_transaction();
    }

    /// Starts again after the link came back: rebinds the lease it holds, or looks for servers.
    pub fn restart(&mut self, now_ms: u64) {
        match self.lease {
            Some(_) => {
                self.phase = Phase::Rebinding;
                self.attempts = 0;
                self.next_send_ms = now_ms;
                self.started_ms = now_ms;
                self.new_transaction();
            }
            None => self.start_selecting(now_ms),
        }
    }

    /// Acts on the time: ends a lease that ran out, moves to renewing and rebinding, and
    /// writes the message due at `now_ms`, if any, to `out` ([`MESSAGE_BYTES`] or more).
    pub fn poll(&mut self, now_ms: u64, out: &mut [u8]) -> (Option<Outgoing>, Event) {
        let mut event = Event::Nothing;
        if let Some(lease) = self.lease {
            if now_ms >= lease.expires_ms() {
                self.lease = None;
                self.start_selecting(now_ms);
                event = Event::Lost;
            } else if self.phase == Phase::Bound && now_ms >= lease.at(lease.renewal_s) {
                self.phase = Phase::Renewing;
                self.attempts = 0;
                self.next_send_ms = now_ms;
                self.started_ms = now_ms;
                self.new_transaction();
            } else if self.phase == Phase::Renewing && now_ms >= lease.at(lease.rebinding_s) {
                self.phase = Phase::Rebinding;
                self.next_send_ms = now_ms;
                self.new_transaction();
            }
        }
        if now_ms < self.next_send_ms {
            return (None, event);
        }
        let outgoing = match self.phase {
            Phase::Bound => {
                self.next_send_ms = u64::MAX;
                None
            }
            Phase::NoAnswer => {
                self.start_selecting(now_ms);
                self.send_discover(now_ms, out)
            }
            Phase::Selecting if self.attempts >= DISCOVER_ATTEMPTS => {
                self.phase = Phase::NoAnswer;
                self.next_send_ms = now_ms.saturating_add(RETRY_AFTER_NO_ANSWER_MS);
                None
            }
            Phase::Selecting => self.send_discover(now_ms, out),
            Phase::Requesting if self.attempts >= REQUEST_ATTEMPTS => {
                self.start_selecting(now_ms);
                self.send_discover(now_ms, out)
            }
            Phase::Requesting => self.send_request_for_offer(now_ms, out),
            Phase::Renewing | Phase::Rebinding => self.send_extension(now_ms, out),
        };
        (outgoing, event)
    }

    fn seconds(&self, now_ms: u64) -> u16 {
        u16::try_from(now_ms.saturating_sub(self.started_ms) / 1000).unwrap_or(u16::MAX)
    }

    fn send(
        &mut self,
        out: &mut [u8],
        request: Request,
        source: Ipv4Address,
        destination: Ipv4Address,
    ) -> Option<Outgoing> {
        let length = write_request(out, self.mac, &request);
        (length > 0).then_some(Outgoing {
            length,
            source,
            destination,
        })
    }

    fn send_discover(&mut self, now_ms: u64, out: &mut [u8]) -> Option<Outgoing> {
        self.attempts = self.attempts.saturating_add(1);
        self.next_send_ms = now_ms.saturating_add(self.retry_delay());
        let request = Request {
            message_type: message_type::DISCOVER,
            transaction: self.transaction,
            seconds: self.seconds(now_ms),
            broadcast: true,
            client_address: Ipv4Address::UNSPECIFIED,
            requested: None,
            server: None,
        };
        self.send(
            out,
            request,
            Ipv4Address::UNSPECIFIED,
            Ipv4Address::BROADCAST,
        )
    }

    fn send_request_for_offer(&mut self, now_ms: u64, out: &mut [u8]) -> Option<Outgoing> {
        let offer = self.offer?;
        self.attempts = self.attempts.saturating_add(1);
        self.next_send_ms = now_ms.saturating_add(self.retry_delay());
        let request = Request {
            message_type: message_type::REQUEST,
            transaction: self.transaction,
            seconds: self.seconds(now_ms),
            broadcast: true,
            client_address: Ipv4Address::UNSPECIFIED,
            requested: Some(offer.address),
            server: Some(offer.server),
        };
        self.send(
            out,
            request,
            Ipv4Address::UNSPECIFIED,
            Ipv4Address::BROADCAST,
        )
    }

    fn send_extension(&mut self, now_ms: u64, out: &mut [u8]) -> Option<Outgoing> {
        let lease = self.lease?;
        let renewing = self.phase == Phase::Renewing;
        let until = match renewing {
            true => lease.at(lease.rebinding_s),
            false => lease.expires_ms(),
        };
        let wait = (until.saturating_sub(now_ms) / 2).max(MIN_RENEW_RETRY_MS);
        self.next_send_ms = now_ms.saturating_add(wait);
        let request = Request {
            message_type: message_type::REQUEST,
            transaction: self.transaction,
            seconds: self.seconds(now_ms),
            broadcast: !renewing,
            client_address: lease.subnet.address,
            requested: None,
            server: None,
        };
        let destination = match renewing {
            true => lease.server,
            false => Ipv4Address::BROADCAST,
        };
        self.send(out, request, lease.subnet.address, destination)
    }

    /// Reads `message`, a datagram that came to port 68, at `now_ms`.
    pub fn receive(&mut self, message: &[u8], now_ms: u64) -> Event {
        let Some(reply) = parse_reply(message, self.transaction, self.mac) else {
            return Event::Nothing;
        };
        let asking = matches!(
            self.phase,
            Phase::Requesting | Phase::Renewing | Phase::Rebinding
        );
        match reply.message_type {
            message_type::OFFER if self.phase == Phase::Selecting => {
                let (Some(server), false) = (reply.server, reply.your_address.is_unspecified())
                else {
                    return Event::Nothing;
                };
                self.offer = Some(Offer {
                    address: reply.your_address,
                    server,
                });
                self.phase = Phase::Requesting;
                self.attempts = 0;
                self.next_send_ms = now_ms;
                Event::Nothing
            }
            message_type::ACK if asking => match self.bind(&reply, now_ms) {
                true => Event::Bound,
                false => Event::Nothing,
            },
            message_type::NAK if asking => {
                self.lease = None;
                self.start_selecting(now_ms);
                Event::Lost
            }
            _ => Event::Nothing,
        }
    }

    fn bind(&mut self, reply: &Reply, now_ms: u64) -> bool {
        let address = match reply.your_address.is_unspecified() {
            false => reply.your_address,
            true => match self.lease {
                Some(lease) => lease.subnet.address,
                None => return false,
            },
        };
        let Some(server) = reply
            .server
            .or(self.offer.map(|offer| offer.server))
            .or(self.lease.map(|lease| lease.server))
        else {
            return false;
        };
        let prefix = reply
            .subnet_mask
            .and_then(prefix_of_mask)
            .unwrap_or_else(|| classful_prefix(address));
        let Some(subnet) = Subnet::new(address, prefix) else {
            return false;
        };
        let lease_s = reply.lease_s.unwrap_or(DEFAULT_LEASE_S);
        let renewal_default = lease_s / 2;
        let rebinding_default = (u64::from(lease_s) * 7 / 8) as u32;
        let rebinding_s = reply
            .rebinding_s
            .filter(|&seconds| seconds <= lease_s)
            .unwrap_or(rebinding_default);
        let renewal_s = reply
            .renewal_s
            .filter(|&seconds| seconds <= rebinding_s)
            .unwrap_or(renewal_default.min(rebinding_s));
        self.lease = Some(Lease {
            subnet,
            router: reply.router,
            dns: reply.dns,
            server,
            lease_s,
            renewal_s,
            rebinding_s,
            granted_ms: now_ms,
        });
        self.phase = Phase::Bound;
        self.offer = None;
        self.attempts = 0;
        self.next_send_ms = u64::MAX;
        true
    }
}

fn classful_prefix(address: Ipv4Address) -> u8 {
    match address.0[0] {
        0..=127 => 8,
        128..=191 => 16,
        _ => 24,
    }
}

/// Test helpers: a server's answer to a client's message, as QEMU's user networking sends it.
pub mod server {
    use super::*;

    /// What a stand-in server offers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Offered {
        /// The client's address.
        pub address: Ipv4Address,
        /// Its network mask.
        pub mask: Ipv4Address,
        /// The router.
        pub router: Ipv4Address,
        /// The DNS server.
        pub dns: Ipv4Address,
        /// The server's own address.
        pub server: Ipv4Address,
        /// The lease in seconds.
        pub lease_s: u32,
    }

    /// Writes the answer of type `answer` to the client message `request` to the start of
    /// `out`: its length, or `None` when `request` is not a client's message or `out` is
    /// shorter than [`MESSAGE_BYTES`].
    pub fn write_answer(
        out: &mut [u8],
        request: &[u8],
        answer: u8,
        offered: &Offered,
    ) -> Option<usize> {
        if request.len() < OPTIONS_OFFSET || request[0] != BOOT_REQUEST {
            return None;
        }
        let message = out.get_mut(..MESSAGE_BYTES)?;
        message.fill(0);
        message[..OPTIONS_OFFSET].copy_from_slice(&request[..OPTIONS_OFFSET]);
        message[0] = BOOT_REPLY;
        message[12..16].fill(0);
        message[16..20].copy_from_slice(&offered.address.0);
        message[20..24].copy_from_slice(&offered.server.0);
        let mut at = OPTIONS_OFFSET;
        let mut put = |bytes: &[u8]| {
            message[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(&[option::MESSAGE_TYPE, 1, answer]);
        put(&[option::SERVER_ID, 4]);
        put(&offered.server.0);
        put(&[option::SUBNET_MASK, 4]);
        put(&offered.mask.0);
        put(&[option::ROUTER, 4]);
        put(&offered.router.0);
        put(&[option::DNS, 4]);
        put(&offered.dns.0);
        put(&[option::LEASE_TIME, 4]);
        put(&offered.lease_s.to_be_bytes());
        put(&[option::END]);
        Some(MESSAGE_BYTES)
    }

    /// The message type of the client message `request`; 0 when it has none.
    pub fn request_type(request: &[u8]) -> u8 {
        let mut at = OPTIONS_OFFSET;
        while let Some(&code) = request.get(at) {
            match code {
                option::PAD => at += 1,
                option::END => return 0,
                option::MESSAGE_TYPE => return request.get(at + 2).copied().unwrap_or(0),
                _ => at += 2 + usize::from(request.get(at + 1).copied().unwrap_or(0)),
            }
        }
        0
    }
}

#[cfg(test)]
mod tests;

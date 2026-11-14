//! The network server's mDNS advertisement (D577): `eggos._mcp._tcp.local`, a `_mcp._tcp`
//! service at `eggos.local` (the name its certificate holds) on the server's port, with `TXT`
//! strings saying the endpoint's path and the certificate's SHA-256 fingerprint, so an agent
//! on the link finds the server and the certificate to pin without the person typing either.
//! Announced twice a second apart when the server starts or its address changes, and answered
//! to the questions on the link.

use super::super::{call, now_ms};
use super::addresses;
use eggos_crypto::sha2::Sha256;
use eggos_net::ipv4::Ipv4Address;
use eggos_net::mdns::{self, ANNOUNCED, Service};
use eggos_runtime::errno::Errno;
use eggos_runtime::syscall::number;

const AF_INET: usize = 2;
const SOCK_DGRAM: usize = 2;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const SOCK_NONBLOCK: usize = 0o4000;
const SOL_SOCKET: usize = 1;
const SO_REUSEADDR: usize = 2;
const SOL_IP: usize = 0;
const IP_ADD_MEMBERSHIP: usize = 35;
const IP_MULTICAST_TTL: usize = 33;
const MDNS_TTL: i32 = 255;
const ANNOUNCEMENTS: u8 = 2;
const ANNOUNCEMENT_GAP_MS: u64 = 1000;
const ADDRESS_CHECK_MS: u64 = 30_000;
const SOCKET_ADDRESS_BYTES: usize = 16;
const INSTANCE: &[u8] = b"eggos";
const HOST: &[u8] = b"eggos";
const PATH_TEXT: &[u8] = b"path=/mcp";
const FINGERPRINT_PREFIX: &[u8] = b"fp=sha256:";
const FINGERPRINT_TEXT_BYTES: usize = 10 + 64;

/// The advertisement's socket and state.
pub struct Advertiser {
    socket: usize,
    port: u16,
    address: Option<Ipv4Address>,
    fingerprint: [u8; FINGERPRINT_TEXT_BYTES],
    announced: u8,
    next_announcement_ms: u64,
    next_address_check_ms: u64,
}

fn socket_address(address: Ipv4Address, port: u16) -> [u8; SOCKET_ADDRESS_BYTES] {
    let mut bytes = [0u8; SOCKET_ADDRESS_BYTES];
    bytes[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
    bytes[2..4].copy_from_slice(&port.to_be_bytes());
    bytes[4..8].copy_from_slice(&address.0);
    bytes
}

fn set_option(socket: usize, level: usize, name: usize, value: &[u8]) -> Result<usize, Errno> {
    call(
        number::SETSOCKOPT,
        [socket, level, name, value.as_ptr() as usize, value.len(), 0],
    )
}

impl Advertiser {
    /// Starts advertising the server on `port` whose certificate is `certificate`; `None` when
    /// the mDNS port cannot be had (another responder holds it, or no network).
    pub fn start(port: u16, certificate: &[u8]) -> Option<Self> {
        let socket = call(
            number::SOCKET,
            [AF_INET, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, 0, 0, 0],
        )
        .ok()?;
        let _ = set_option(socket, SOL_SOCKET, SO_REUSEADDR, &1i32.to_ne_bytes());
        let _ = set_option(socket, SOL_IP, IP_MULTICAST_TTL, &MDNS_TTL.to_ne_bytes());
        let bound = socket_address(Ipv4Address::UNSPECIFIED, mdns::PORT);
        let mut membership = [0u8; 8];
        membership[..4].copy_from_slice(&mdns::GROUP.0);
        let ready = call(
            number::BIND,
            [socket, bound.as_ptr() as usize, bound.len(), 0, 0, 0],
        )
        .and_then(|_| set_option(socket, SOL_IP, IP_ADD_MEMBERSHIP, &membership));
        if ready.is_err() {
            super::super::close(socket);
            return None;
        }
        let mut hash = Sha256::new();
        hash.update(certificate);
        let digest = hash.finish();
        let mut fingerprint = [0u8; FINGERPRINT_TEXT_BYTES];
        fingerprint[..FINGERPRINT_PREFIX.len()].copy_from_slice(FINGERPRINT_PREFIX);
        for (index, byte) in digest.iter().enumerate() {
            let at = FINGERPRINT_PREFIX.len() + 2 * index;
            fingerprint[at] = b"0123456789abcdef"[usize::from(byte >> 4)];
            fingerprint[at + 1] = b"0123456789abcdef"[usize::from(byte & 15)];
        }
        Some(Self {
            socket,
            port,
            address: None,
            fingerprint,
            announced: 0,
            next_announcement_ms: 0,
            next_address_check_ms: 0,
        })
    }

    /// The socket to wait on.
    pub fn socket(&self) -> usize {
        self.socket
    }

    /// How long until [`Advertiser::tick`] has something to do, in milliseconds.
    pub fn timeout_ms(&self) -> u64 {
        let now = now_ms();
        let next = match self.announced < ANNOUNCEMENTS && self.address.is_some() {
            true => self.next_announcement_ms.min(self.next_address_check_ms),
            false => self.next_address_check_ms,
        };
        next.saturating_sub(now)
    }

    fn send(&self, message: &[u8], to: Ipv4Address, port: u16) {
        let address = socket_address(to, port);
        let _ = call(
            number::SENDTO,
            [
                self.socket,
                message.as_ptr() as usize,
                message.len(),
                0,
                address.as_ptr() as usize,
                address.len(),
            ],
        );
    }

    fn with_service<R>(&self, then: impl FnOnce(&Service<'_>) -> R) -> Option<R> {
        let text: [&[u8]; 2] = [PATH_TEXT, &self.fingerprint];
        let service = Service {
            instance: INSTANCE,
            service_type: [b"_mcp", b"_tcp"],
            host: HOST,
            address: self.address?,
            port: self.port,
            text: &text,
        };
        Some(then(&service))
    }

    /// Checks the address now and then, and sends the announcements that are due.
    pub fn tick(&mut self) {
        let now = now_ms();
        if now >= self.next_address_check_ms {
            self.next_address_check_ms = now + ADDRESS_CHECK_MS;
            let (found, count) = addresses();
            let address = (count > 0).then(|| Ipv4Address(found[0]));
            if address != self.address {
                self.address = address;
                self.announced = 0;
                self.next_announcement_ms = now;
            }
        }
        if self.announced >= ANNOUNCEMENTS || now < self.next_announcement_ms {
            return;
        }
        let mut message = [0u8; mdns::MAX_MESSAGE_BYTES];
        let length = self
            .with_service(|service| mdns::write_response(service, ANNOUNCED, 0, false, &mut message))
            .flatten();
        if let Some(length) = length {
            self.send(&message[..length], mdns::GROUP, mdns::PORT);
            self.announced += 1;
            self.next_announcement_ms = now + ANNOUNCEMENT_GAP_MS;
        }
    }

    /// Answers the questions waiting on the socket.
    pub fn readable(&mut self) {
        let mut query = [0u8; mdns::MAX_MESSAGE_BYTES];
        loop {
            let mut from = [0u8; SOCKET_ADDRESS_BYTES];
            let mut from_length = SOCKET_ADDRESS_BYTES as u32;
            let Ok(length) = call(
                number::RECVFROM,
                [
                    self.socket,
                    query.as_mut_ptr() as usize,
                    query.len(),
                    0,
                    from.as_mut_ptr() as usize,
                    (&raw mut from_length) as usize,
                ],
            ) else {
                return;
            };
            let source = Ipv4Address([from[4], from[5], from[6], from[7]]);
            let source_port = u16::from_be_bytes([from[2], from[3]]);
            let query = &query[..length];
            let mut answer = [0u8; mdns::MAX_MESSAGE_BYTES];
            let written = self.with_service(|service| {
                let (asked, _) = mdns::questions(service, query)?;
                let legacy = source_port != mdns::PORT;
                let id = match legacy {
                    true => u16::from_be_bytes([query[0], query[1]]),
                    false => 0,
                };
                let length = mdns::write_response(service, asked, id, false, &mut answer)?;
                Some((length, legacy || asked.unicast))
            });
            let Some(Some((length, unicast))) = written else {
                continue;
            };
            let reply = &answer[..length];
            match unicast {
                true => self.send(reply, source, source_port),
                false => self.send(reply, mdns::GROUP, mdns::PORT),
            }
        }
    }
}

//! The endpoints programs send and receive through (D157): a UDP endpoint is bound to a port,
//! a ping endpoint (ICMP echo, Linux's "ping socket") to an identifier, which the kernel picks
//! when the program does not. The stack hands each datagram that comes for the machine to the
//! endpoint bound to its port and address, connected to its sender if connected at all
//! ([`deliver`]), and each echo reply to the ping endpoint with its identifier; the endpoint
//! queues them until the program takes them, up to its receive limit. Datagrams that find no
//! room, or no memory the reserve lets programs have (D155), are dropped and counted, as UDP
//! allows.
//!
//! Endpoints are found through a table of weak references, so an endpoint that is no longer
//! used frees its port with its last reference.

use crate::sync::LeafSpinLock;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use eggos_net::interface::{Datagram as Arrived, Delivery};
use eggos_net::ipv4::Ipv4Address;

/// The ports and identifiers the kernel picks from, as Linux's `ip_local_port_range`.
pub const EPHEMERAL_PORTS: core::ops::RangeInclusive<u16> = 32768..=60999;
/// The bytes an endpoint queues before it drops datagrams, as Linux's `rmem_default`.
pub const DEFAULT_RECEIVE_LIMIT: usize = 212_992;
/// The least receive limit a program can set, as Linux's `SOCK_MIN_RCVBUF` rounds it.
pub const MIN_RECEIVE_LIMIT: usize = 2304;
/// The most bytes one datagram's data holds over UDP and IPv4.
pub const MAX_DATAGRAM_BYTES: usize = 65_507;

/// What an endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// UDP datagrams.
    Udp,
    /// ICMP echo requests out, echo replies in.
    Ping,
}

/// A datagram an endpoint queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// Who sent it.
    pub source: Ipv4Address,
    /// From which port (0 for an echo reply).
    pub source_port: u16,
    /// Its data; an echo reply's whole ICMP message.
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
struct State {
    local: Option<(Ipv4Address, u16)>,
    remote: Option<(Ipv4Address, u16)>,
    queue: VecDeque<Datagram>,
    queued_bytes: usize,
    receive_limit: usize,
    read_closed: bool,
    write_closed: bool,
    dropped: u64,
}

/// One endpoint.
#[derive(Debug)]
pub struct Endpoint {
    protocol: Protocol,
    inode: u64,
    state: LeafSpinLock<State>,
}

/// Why an endpoint could not be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// It is bound already.
    AlreadyBound,
    /// Another endpoint has the port.
    InUse,
    /// Every port the kernel picks from is taken.
    NoFreePort,
    /// No memory to record the binding.
    NoMemory,
}

static BOUND: LeafSpinLock<Option<Box<Vec<Weak<Endpoint>>>>> = LeafSpinLock::new_leaf(None);

static NEXT_PORT: AtomicU64 = AtomicU64::new(0);

static LAST_INODE: AtomicU64 = AtomicU64::new(0);

const FIRST_INODE: u64 = 1 << 32;

impl Endpoint {
    /// A new endpoint of `protocol`, not bound.
    pub fn new(protocol: Protocol) -> Arc<Self> {
        Arc::new(Self {
            protocol,
            inode: FIRST_INODE + LAST_INODE.fetch_add(1, Ordering::Relaxed),
            state: LeafSpinLock::new_leaf(State {
                local: None,
                remote: None,
                queue: VecDeque::new(),
                queued_bytes: 0,
                receive_limit: DEFAULT_RECEIVE_LIMIT,
                read_closed: false,
                write_closed: false,
                dropped: 0,
            }),
        })
    }

    /// What it speaks.
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Its number, as `fstat` reports it (`st_ino`): from 2^32 up, apart from `AF_UNIX`
    /// sockets' numbers on the same device.
    pub fn inode(&self) -> u64 {
        self.inode
    }

    /// The address and port it is bound to.
    pub fn local(&self) -> Option<(Ipv4Address, u16)> {
        self.state.lock().local
    }

    /// The address and port it is connected to.
    pub fn remote(&self) -> Option<(Ipv4Address, u16)> {
        self.state.lock().remote
    }

    /// Binds it to `address` and `port`, or a port the kernel picks for port 0.
    pub fn bind(self: &Arc<Self>, address: Ipv4Address, port: u16) -> Result<u16, BindError> {
        if self.state.lock().local.is_some() {
            return Err(BindError::AlreadyBound);
        }
        let mut table = BOUND.lock();
        if table.is_none() {
            let mut empty = Vec::new();
            empty.try_reserve(8).map_err(|_| BindError::NoMemory)?;
            *table = Some(Box::new(empty));
        }
        let Some(bound) = table.as_deref_mut() else {
            return Err(BindError::NoMemory);
        };
        bound.retain(|endpoint| endpoint.strong_count() > 0);
        let taken = |port: u16| {
            bound.iter().filter_map(Weak::upgrade).any(|endpoint| {
                endpoint.protocol == self.protocol
                    && endpoint.local().is_some_and(|(other, taken)| {
                        taken == port
                            && (other.is_unspecified()
                                || address.is_unspecified()
                                || other == address)
                    })
            })
        };
        let port = match port {
            0 => {
                let first = *EPHEMERAL_PORTS.start();
                let count = u64::from(*EPHEMERAL_PORTS.end() - first) + 1;
                let start = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
                (0..count)
                    .map(|offset| first + ((start + offset) % count) as u16)
                    .find(|&port| !taken(port))
                    .ok_or(BindError::NoFreePort)?
            }
            port if taken(port) => return Err(BindError::InUse),
            port => port,
        };
        bound.try_reserve(1).map_err(|_| BindError::NoMemory)?;
        let mut state = self.state.lock();
        if state.local.is_some() {
            return Err(BindError::AlreadyBound);
        }
        state.local = Some((address, port));
        drop(state);
        bound.push(Arc::downgrade(self));
        Ok(port)
    }

    /// Connects it to `remote`, or disconnects it for `None`.
    pub fn connect(&self, remote: Option<(Ipv4Address, u16)>) {
        self.state.lock().remote = remote;
    }

    /// Sets how many bytes it queues before it drops datagrams.
    pub fn set_receive_limit(&self, bytes: usize) {
        self.state.lock().receive_limit = bytes.max(MIN_RECEIVE_LIMIT);
    }

    /// How many bytes it queues before it drops datagrams.
    pub fn receive_limit(&self) -> usize {
        self.state.lock().receive_limit
    }

    /// How many datagrams it dropped for want of room.
    pub fn dropped(&self) -> u64 {
        self.state.lock().dropped
    }

    /// Stops receiving (`read`) and sending (`write`), for good.
    pub fn shut_down(&self, read: bool, write: bool) {
        let mut state = self.state.lock();
        state.read_closed |= read;
        state.write_closed |= write;
    }

    /// Whether it no longer receives.
    pub fn is_read_closed(&self) -> bool {
        self.state.lock().read_closed
    }

    /// Whether it no longer sends.
    pub fn is_write_closed(&self) -> bool {
        self.state.lock().write_closed
    }

    /// Whether a datagram waits.
    pub fn has_datagrams(&self) -> bool {
        !self.state.lock().queue.is_empty()
    }

    /// The length of the datagram that waits first.
    pub fn next_length(&self) -> Option<usize> {
        self.state
            .lock()
            .queue
            .front()
            .map(|datagram| datagram.bytes.len())
    }

    /// The datagram that waits first, taken, or a copy of it when `peek`; `None` when none
    /// waits or the copy finds no memory.
    pub fn take(&self, peek: bool) -> Option<Datagram> {
        let mut state = self.state.lock();
        if peek {
            let first = state.queue.front()?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(first.bytes.len()).ok()?;
            bytes.extend_from_slice(&first.bytes);
            return Some(Datagram {
                source: first.source,
                source_port: first.source_port,
                bytes,
            });
        }
        let datagram = state.queue.pop_front()?;
        state.queued_bytes -= datagram.bytes.len();
        Some(datagram)
    }

    fn accepts(&self, protocol: Protocol, from: (Ipv4Address, u16), to: (Ipv4Address, u16)) -> bool {
        if self.protocol != protocol {
            return false;
        }
        let state = self.state.lock();
        let Some((address, port)) = state.local.filter(|_| !state.read_closed) else {
            return false;
        };
        let ping = protocol == Protocol::Ping;
        let bound = port == to.1
            && (ping || address.is_unspecified() || address == to.0 || to.0.is_broadcast());
        let connected = state.remote.is_none_or(|(remote, remote_port)| {
            remote == from.0 && (ping || remote_port == from.1)
        });
        bound && connected
    }

    fn push(&self, source: Ipv4Address, source_port: u16, data: &[u8]) -> bool {
        let mut state = self.state.lock();
        let fits = state.queued_bytes + data.len() <= state.receive_limit;
        if !fits || !crate::heap::may_grow_for_user(data.len()) || state.queue.try_reserve(1).is_err() {
            state.dropped += 1;
            return false;
        }
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(data.len()).is_err() {
            state.dropped += 1;
            return false;
        }
        bytes.extend_from_slice(data);
        state.queued_bytes += data.len();
        state.queue.push_back(Datagram {
            source,
            source_port,
            bytes,
        });
        true
    }
}

/// Hands `delivery` to the endpoint it is for: whether one queued it.
pub fn deliver(delivery: Delivery<'_>) -> bool {
    let (protocol, from, to, data) = match delivery {
        Delivery::Udp(Arrived {
            source,
            source_port,
            destination,
            destination_port,
            data,
        }) => (
            Protocol::Udp,
            (source, source_port),
            (destination, destination_port),
            data,
        ),
        Delivery::EchoReply {
            source,
            identifier,
            message,
        } => (
            Protocol::Ping,
            (source, 0),
            (Ipv4Address::UNSPECIFIED, identifier),
            message,
        ),
    };
    let endpoint = BOUND
        .lock()
        .as_deref()
        .into_iter()
        .flatten()
        .filter_map(Weak::upgrade)
        .find(|endpoint| endpoint.accepts(protocol, from, to));
    endpoint.is_some_and(|endpoint| endpoint.push(from.0, from.1, data))
}

#[cfg(test)]
mod tests;

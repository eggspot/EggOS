//! `AF_NETLINK` sockets of `NETLINK_ROUTE` (D559): what iproute2's `ip link`, `ip addr` and
//! `ip route`, musl's and glibc's `getifaddrs` and `if_nameindex` ask the kernel. A request is
//! answered at once into the socket's queue of messages, which `recvmsg`, `recvfrom` and `read`
//! take one datagram at a time: a dump of the links (`RTM_GETLINK`), the IPv4 addresses
//! (`RTM_GETADDR`) or the routes (`RTM_GETROUTE`), each ended by `NLMSG_DONE`, or an
//! `NLMSG_ERROR` (with error 0 as an acknowledgment when asked). Changes (`RTM_NEWADDR`,
//! `RTM_NEWROUTE`, ...) are refused with `EPERM`: EggOS's addresses come from DHCP or Settings.
//! Multicast groups are accepted at `bind` and send nothing.

use super::super::Errno;
use crate::sync::SpinLock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use eggos_hal::Platform;
use eggos_net::status::InterfaceStatus;

pub const AF_NETLINK: u32 = 16;
pub const NETLINK_ROUTE: u32 = 0;

const HEADER: usize = 16;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_MULTI: u16 = 0x2;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_DUMP: u16 = 0x300;

const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;

const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const ARPHRD_ETHER: u16 = 1;
const ARPHRD_LOOPBACK: u16 = 772;
const IFF_UP: u32 = 0x1;
const IFF_BROADCAST: u32 = 0x2;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_MULTICAST: u32 = 0x1000;
const IFF_LOWER_UP: u32 = 0x1_0000;

const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_OPERSTATE: u16 = 16;
const IF_OPER_DOWN: u8 = 2;
const IF_OPER_UP: u8 = 6;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;
const RT_SCOPE_UNIVERSE: u8 = 0;
const RT_SCOPE_LINK: u8 = 253;
const RT_SCOPE_HOST: u8 = 254;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PREFSRC: u16 = 7;
const RTA_TABLE: u16 = 15;
const RT_TABLE_MAIN: u8 = 254;
const RTPROT_KERNEL: u8 = 2;
const RTPROT_DHCP: u8 = 16;
const RTN_UNICAST: u8 = 1;

/// The most messages a socket keeps unread before it drops a reply (`ENOBUFS` at the next read).
const QUEUE_LIMIT: usize = 64;

/// A netlink socket: its port id and the datagrams waiting to be read.
#[derive(Debug)]
pub struct NetlinkSocket {
    port: u32,
    queue: SpinLock<VecDeque<Vec<u8>>>,
}

impl NetlinkSocket {
    /// A new socket, bound to the caller's process id as its port.
    pub fn new(port: u32) -> Arc<Self> {
        Arc::new(Self {
            port,
            queue: SpinLock::new(VecDeque::new()),
        })
    }

    pub fn port(&self) -> u32 {
        self.port
    }

    /// Whether a datagram waits.
    pub fn is_readable(&self) -> bool {
        !self.queue.lock().is_empty()
    }

    /// Takes the next datagram into `buffer`: its length, cut to the buffer (`MSG_TRUNC` says the
    /// whole length), or `None` when none waits.
    pub fn take(&self, buffer: &mut [u8]) -> Option<(usize, usize)> {
        let datagram = self.queue.lock().pop_front()?;
        let copied = datagram.len().min(buffer.len());
        buffer[..copied].copy_from_slice(&datagram[..copied]);
        Some((copied, datagram.len()))
    }

    /// Handles the requests in `bytes`, a sender's datagram: each answered into the queue.
    pub fn send<P: Platform>(&self, bytes: &[u8]) -> Result<usize, Errno> {
        let mut at = 0;
        while at + HEADER <= bytes.len() {
            let length = u32_at(bytes, at) as usize;
            if length < HEADER || at + length > bytes.len() {
                return Err(Errno::EINVAL);
            }
            let message = &bytes[at..at + length];
            self.answer::<P>(message);
            at += length.next_multiple_of(4);
        }
        Ok(bytes.len())
    }

    fn answer<P: Platform>(&self, message: &[u8]) {
        let kind = u16_at(message, 4);
        let flags = u16_at(message, 6);
        let sequence = u32_at(message, 8);
        if flags & NLM_F_REQUEST == 0 {
            return;
        }
        let interfaces = crate::net::interfaces::<P>();
        let mut reply = Reply::new(sequence, self.port);
        match kind {
            RTM_GETLINK => {
                for (index, interface) in interfaces.iter().enumerate() {
                    reply.link(index as u32 + 1, interface);
                }
                reply.done();
            }
            RTM_GETADDR => {
                for (index, interface) in interfaces.iter().enumerate() {
                    reply.address(index as u32 + 1, interface);
                }
                reply.done();
            }
            RTM_GETROUTE => {
                for (index, interface) in interfaces.iter().enumerate() {
                    reply.routes(index as u32 + 1, interface);
                }
                reply.done();
            }
            _ if flags & NLM_F_DUMP != 0 => reply.done(),
            _ => {
                reply.error(message, Errno::EPERM.0 as i32);
                self.queue_reply(reply);
                return;
            }
        }
        if flags & NLM_F_ACK != 0 && flags & NLM_F_DUMP != NLM_F_DUMP {
            reply.error(message, 0);
        }
        self.queue_reply(reply);
    }

    fn queue_reply(&self, reply: Reply) {
        let mut queue = self.queue.lock();
        if queue.len() < QUEUE_LIMIT && queue.try_reserve(1).is_ok() {
            queue.push_back(reply.bytes);
        }
    }
}

/// A reply being built: messages one after the other in one datagram, as Linux sends a dump.
struct Reply {
    bytes: Vec<u8>,
    sequence: u32,
    port: u32,
}

impl Reply {
    fn new(sequence: u32, port: u32) -> Self {
        Self {
            bytes: Vec::new(),
            sequence,
            port,
        }
    }

    fn begin(&mut self, kind: u16, flags: u16) -> usize {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(&[0; HEADER]);
        self.bytes[start + 4..start + 6].copy_from_slice(&kind.to_ne_bytes());
        self.bytes[start + 6..start + 8].copy_from_slice(&flags.to_ne_bytes());
        self.bytes[start + 8..start + 12].copy_from_slice(&self.sequence.to_ne_bytes());
        self.bytes[start + 12..start + 16].copy_from_slice(&self.port.to_ne_bytes());
        start
    }

    fn end(&mut self, start: usize) {
        let length = (self.bytes.len() - start) as u32;
        self.bytes[start..start + 4].copy_from_slice(&length.to_ne_bytes());
        while self.bytes.len() % 4 != 0 {
            self.bytes.push(0);
        }
    }

    fn attribute(&mut self, kind: u16, value: &[u8]) {
        let length = (4 + value.len()) as u16;
        self.bytes.extend_from_slice(&length.to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(value);
        while self.bytes.len() % 4 != 0 {
            self.bytes.push(0);
        }
    }

    fn link(&mut self, index: u32, interface: &InterfaceStatus) {
        let start = self.begin(RTM_NEWLINK, NLM_F_MULTI);
        let loopback = interface.name.as_bytes() == b"lo";
        let up = interface.link_up || loopback;
        let mut flags = IFF_UP | IFF_MULTICAST;
        flags |= if loopback { IFF_LOOPBACK } else { IFF_BROADCAST };
        if up {
            flags |= IFF_RUNNING | IFF_LOWER_UP;
        }
        self.bytes.push(AF_UNSPEC);
        self.bytes.push(0);
        let kind = if loopback { ARPHRD_LOOPBACK } else { ARPHRD_ETHER };
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(&(index as i32).to_ne_bytes());
        self.bytes.extend_from_slice(&flags.to_ne_bytes());
        self.bytes.extend_from_slice(&0u32.to_ne_bytes());
        let mut name = Vec::from(interface.name.as_bytes());
        name.push(0);
        self.attribute(IFLA_IFNAME, &name);
        self.attribute(IFLA_MTU, &interface.mtu.to_ne_bytes());
        self.attribute(IFLA_ADDRESS, &interface.mac.0);
        if !loopback {
            self.attribute(IFLA_BROADCAST, &[0xff; 6]);
        }
        self.attribute(IFLA_OPERSTATE, &[if up { IF_OPER_UP } else { IF_OPER_DOWN }]);
        self.end(start);
    }

    fn address(&mut self, index: u32, interface: &InterfaceStatus) {
        let Some(subnet) = interface.subnet else {
            return;
        };
        let start = self.begin(RTM_NEWADDR, NLM_F_MULTI);
        let loopback = interface.name.as_bytes() == b"lo";
        self.bytes.push(AF_INET);
        self.bytes.push(subnet.prefix);
        self.bytes.push(0);
        self.bytes.push(if loopback { RT_SCOPE_HOST } else { RT_SCOPE_UNIVERSE });
        self.bytes.extend_from_slice(&index.to_ne_bytes());
        self.attribute(IFA_ADDRESS, &subnet.address.0);
        self.attribute(IFA_LOCAL, &subnet.address.0);
        if !loopback && subnet.prefix < 31 {
            let host_bits = u32::MAX.checked_shr(u32::from(subnet.prefix)).unwrap_or(0);
            let broadcast = (u32::from_be_bytes(subnet.address.0) | host_bits).to_be_bytes();
            self.attribute(IFA_BROADCAST, &broadcast);
        }
        let mut name = Vec::from(interface.name.as_bytes());
        name.push(0);
        self.attribute(IFA_LABEL, &name);
        self.end(start);
    }

    fn routes(&mut self, index: u32, interface: &InterfaceStatus) {
        let Some(subnet) = interface.subnet else {
            return;
        };
        if let Some(router) = interface.router {
            self.route(index, None, 0, Some(router), subnet.address, RTPROT_DHCP, RT_SCOPE_UNIVERSE);
        }
        let mask = u32::MAX.checked_shl(32 - u32::from(subnet.prefix)).unwrap_or(0);
        let network = (u32::from_be_bytes(subnet.address.0) & mask).to_be_bytes();
        self.route(
            index,
            Some(network),
            subnet.prefix,
            None,
            subnet.address,
            RTPROT_KERNEL,
            RT_SCOPE_LINK,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn route(
        &mut self,
        index: u32,
        destination: Option<[u8; 4]>,
        prefix: u8,
        gateway: Option<eggos_net::ipv4::Ipv4Address>,
        source: eggos_net::ipv4::Ipv4Address,
        protocol: u8,
        scope: u8,
    ) {
        let start = self.begin(RTM_NEWROUTE, NLM_F_MULTI);
        self.bytes.extend_from_slice(&[
            AF_INET,
            prefix,
            0,
            0,
            RT_TABLE_MAIN,
            protocol,
            scope,
            RTN_UNICAST,
        ]);
        self.bytes.extend_from_slice(&0u32.to_ne_bytes());
        self.attribute(RTA_TABLE, &u32::from(RT_TABLE_MAIN).to_ne_bytes());
        if let Some(destination) = destination {
            self.attribute(RTA_DST, &destination);
        }
        if let Some(gateway) = gateway {
            self.attribute(RTA_GATEWAY, &gateway.0);
        }
        self.attribute(RTA_PREFSRC, &source.0);
        self.attribute(RTA_OIF, &index.to_ne_bytes());
        self.end(start);
    }

    fn done(&mut self) {
        let start = self.begin(NLMSG_DONE, NLM_F_MULTI);
        self.bytes.extend_from_slice(&0i32.to_ne_bytes());
        self.end(start);
    }

    fn error(&mut self, request: &[u8], error: i32) {
        let start = self.begin(NLMSG_ERROR, 0);
        self.bytes.extend_from_slice(&(-error).to_ne_bytes());
        self.bytes.extend_from_slice(&request[..HEADER]);
        self.end(start);
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_ne_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// `struct sockaddr_nl` for `getsockname`: family, padding, port id, groups.
pub fn address_of(socket: &NetlinkSocket) -> [u8; 12] {
    let mut address = [0u8; 12];
    address[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
    address[4..8].copy_from_slice(&socket.port().to_ne_bytes());
    address
}

/// The kernel's `struct sockaddr_nl`: port 0.
pub fn address_of_kernel() -> [u8; 12] {
    let mut address = [0u8; 12];
    address[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
    address
}

//! `AF_INET` datagram sockets (D157): UDP sockets, and ping sockets (`SOCK_DGRAM` with
//! `IPPROTO_ICMP`, Linux's unprivileged ICMP echo), over the kernel's network
//! ([`crate::net`]). `socket`, `bind`, `connect` (`AF_UNSPEC` disconnects), `getsockname`,
//! `getpeername`, `sendto`, `sendmsg`, `recvfrom`, `recvmsg`, `shutdown`, `setsockopt` and
//! `getsockopt`, and `read`, `write`, `poll`, `ioctl(FIONREAD)` and `fstat` on them, as Linux
//! answers them, with `struct sockaddr_in` (a family, a port and an address in network order,
//! eight zero bytes).
//!
//! A socket that sends or connects before it binds is bound to a port the kernel picks. A
//! datagram is sent whole or not at all, at once: the stack does not fragment, so data longer
//! than the link's MTU allows fails with `EMSGSIZE`. A ping socket sends the echo requests the
//! program writes (an ICMP header of type 8 and code 0, then data), with the socket's
//! identifier and the checksum filled in, and reads the echo replies with that identifier. A
//! receive waits until a datagram comes, the socket's timeout passes or a signal comes.

use super::{
    MSGHDR_SIZE, SOL_SOCKET, TIMEVAL_SIZE, deadline, finish, interrupted, message, option,
    read_header, read_int, timeout_nanoseconds, timeval,
};
use crate::linux::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use crate::linux::signals;
use crate::linux::terminal::{Waited, wait_for_input};
use crate::linux::thread::LinuxThread;
use crate::linux::{Errno, Iovecs, MAX_IOVECS, ends_in_user_space, iovec, number};
use crate::net::endpoints::{self, BindError, Endpoint, Protocol};
use crate::process::Process;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;
use eggos_net::icmp;
use eggos_net::interface::SendError;
use eggos_net::ipv4::{self, Ipv4Address};

/// The IPv4 address family (`AF_INET`).
pub const AF_INET: u32 = 2;
/// Datagram sockets (`SOCK_DGRAM`).
pub const SOCK_DGRAM: u32 = 2;
/// ICMP, for ping sockets (`IPPROTO_ICMP`).
pub const IPPROTO_ICMP: u32 = 1;
/// UDP (`IPPROTO_UDP`).
pub const IPPROTO_UDP: u32 = 17;
/// The size of a `struct sockaddr_in`.
pub const SOCKADDR_IN_SIZE: usize = 16;

const AF_UNSPEC: u16 = 0;
const SOL_IP: u32 = 0;
const IP_TTL: u32 = 2;
const SO_BROADCAST: u32 = 6;
const SO_REUSEPORT: u32 = 15;
const MSG_TRUNC: u32 = 0x20;
const MSG_ERRQUEUE: u32 = 0x2000;
const PRIVILEGED_PORTS: u16 = 1024;

/// An `AF_INET` socket's options.
#[derive(Debug, Default)]
pub struct Options {
    /// `SO_RCVTIMEO` in nanoseconds; 0 for none.
    pub receive_timeout: AtomicU64,
    /// `SO_SNDTIMEO` in nanoseconds; 0 for none.
    pub send_timeout: AtomicU64,
    broadcast: AtomicBool,
    reuse_address: AtomicBool,
    reuse_port: AtomicBool,
    ttl: AtomicU32,
    send_buffer: AtomicU32,
}

/// An `AF_INET` socket: its endpoint and its options.
#[derive(Debug)]
pub struct InetSocket {
    endpoint: Arc<Endpoint>,
    options: Options,
}

impl InetSocket {
    /// A new socket of `protocol`, not bound.
    pub fn new(protocol: Protocol) -> Arc<Self> {
        Arc::new(Self {
            endpoint: Endpoint::new(protocol),
            options: Options::default(),
        })
    }

    /// Its endpoint.
    pub fn endpoint(&self) -> &Arc<Endpoint> {
        &self.endpoint
    }

    /// Its options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Its number, as `fstat` reports it (`st_ino`).
    pub fn inode(&self) -> u64 {
        self.endpoint.inode()
    }

    fn ttl(&self) -> u8 {
        match self.options.ttl.load(Ordering::Relaxed) {
            0 => ipv4::DEFAULT_TTL,
            ttl => ttl as u8,
        }
    }

    /// The `poll` events of `requested` it has: writable always, readable with a datagram
    /// waiting or reading shut down, hung up when both directions are.
    pub fn events(&self, requested: u16) -> u16 {
        use crate::linux::poll::event;
        let read_closed = self.endpoint.is_read_closed();
        let mut events = requested & (event::OUT | event::WRNORM);
        if self.endpoint.has_datagrams() || read_closed {
            events |= requested & (event::IN | event::RDNORM);
        }
        if read_closed && self.endpoint.is_write_closed() {
            events |= event::HUP;
        }
        events
    }
}

impl PartialEq for InetSocket {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other)
    }
}

impl Eq for InetSocket {}

/// The `AF_INET` socket open at `fd`, if that is what it is.
pub fn socket_at(thread: &LinuxThread, fd: u64) -> Option<(Arc<OpenFile>, Arc<InetSocket>)> {
    let file = thread.files().file(fd).ok()?;
    let socket = match file.kind() {
        FileKind::InetSocket(socket) => Arc::clone(socket),
        _ => return None,
    };
    Some((file, socket))
}

/// `socket(AF_INET, type, protocol)`: a new datagram socket's descriptor, UDP for protocol 0
/// or `IPPROTO_UDP`, ping for `IPPROTO_ICMP`. As Linux: `EINVAL` for unknown type bits,
/// `EPROTONOSUPPORT` for another protocol; `ESOCKTNOSUPPORT` for streams and raw sockets,
/// which EggOS does not have yet; `EMFILE` without a free descriptor.
pub fn socket<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    kind: u64,
    protocol: u64,
) -> Result<u64, Errno> {
    let kind = kind as u32;
    let flags = kind & super::SOCKET_FLAGS;
    let base = kind & !super::SOCKET_FLAGS;
    if base & !super::SOCK_TYPE_MASK != 0 || base >= super::SOCK_MAX {
        return Err(Errno::EINVAL);
    }
    if base != SOCK_DGRAM {
        return Err(Errno::ESOCKTNOSUPPORT);
    }
    let protocol = match protocol as u32 {
        0 | IPPROTO_UDP => Protocol::Udp,
        IPPROTO_ICMP => Protocol::Ping,
        _ => return Err(Errno::EPROTONOSUPPORT),
    };
    let file = Arc::new(OpenFile::new(
        FileKind::InetSocket(InetSocket::new(protocol)),
        open_flag::RDWR | flags & open_flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let opened = thread
        .files()
        .lock()
        .open(file, flags & open_flag::CLOEXEC != 0, limit);
    opened.map(u64::from)
}

/// The address a `struct sockaddr_in` of `bytes` names; `None` for `AF_UNSPEC`. As Linux:
/// `EINVAL` when shorter than the structure, `EAFNOSUPPORT` for another family.
pub fn parse_address(bytes: &[u8]) -> Result<Option<(Ipv4Address, u16)>, Errno> {
    let family = match bytes.first_chunk::<2>() {
        Some(family) => u16::from_ne_bytes(*family),
        None => return Err(Errno::EINVAL),
    };
    if family == AF_UNSPEC {
        return Ok(None);
    }
    if bytes.len() < SOCKADDR_IN_SIZE {
        return Err(Errno::EINVAL);
    }
    if u32::from(family) != AF_INET {
        return Err(Errno::EAFNOSUPPORT);
    }
    let port = u16::from_be_bytes([bytes[2], bytes[3]]);
    let address = Ipv4Address([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Ok(Some((address, port)))
}

/// `address`:`port` as a `struct sockaddr_in`.
pub fn encode_address(address: Ipv4Address, port: u16) -> [u8; SOCKADDR_IN_SIZE] {
    let mut bytes = [0u8; SOCKADDR_IN_SIZE];
    bytes[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
    bytes[2..4].copy_from_slice(&port.to_be_bytes());
    bytes[4..8].copy_from_slice(&address.0);
    bytes
}

fn read_address<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
) -> Result<Option<(Ipv4Address, u16)>, Errno> {
    let length = (length as u32 as usize).min(SOCKADDR_IN_SIZE);
    let mut bytes = [0u8; SOCKADDR_IN_SIZE];
    process
        .read_user_memory(address, &mut bytes[..length])
        .map_err(|_| Errno::EFAULT)?;
    parse_address(&bytes[..length])
}

fn write_address<P: Platform>(
    process: &Process<P>,
    address: u64,
    length_at: u64,
    value: (Ipv4Address, u16),
) -> Result<(), Errno> {
    if address == 0 {
        return Ok(());
    }
    let room = read_int(process, length_at)?;
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let bytes = encode_address(value.0, value.1);
    let written = SOCKADDR_IN_SIZE.min(room as usize);
    process
        .write_user_memory(address, &bytes[..written])
        .map_err(|_| Errno::EFAULT)?;
    process
        .write_user_memory(length_at, &(SOCKADDR_IN_SIZE as u32).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)
}

fn bind_error(error: BindError) -> Errno {
    match error {
        BindError::AlreadyBound => Errno::EINVAL,
        BindError::InUse => Errno::EADDRINUSE,
        BindError::NoFreePort => Errno::EAGAIN,
        BindError::NoMemory => Errno::ENOMEM,
    }
}

fn send_error(error: SendError) -> Errno {
    match error {
        SendError::LinkDown => Errno::ENETDOWN,
        SendError::NoAddress | SendError::NoRoute => Errno::ENETUNREACH,
        SendError::TooLong => Errno::EMSGSIZE,
    }
}

fn is_privileged<P: Platform>(process: &Process<P>) -> bool {
    process
        .credentials_and_limits()
        .lock()
        .credentials
        .is_privileged()
}

fn ensure_bound(socket: &InetSocket) -> Result<(Ipv4Address, u16), Errno> {
    if let Some(local) = socket.endpoint.local() {
        return Ok(local);
    }
    match socket.endpoint.bind(Ipv4Address::UNSPECIFIED, 0) {
        Ok(port) => Ok((Ipv4Address::UNSPECIFIED, port)),
        Err(BindError::AlreadyBound) => socket.endpoint.local().ok_or(Errno::EINVAL),
        Err(error) => Err(bind_error(error)),
    }
}

/// Every socket call but `socket` and `socketpair` on the `AF_INET` socket `socket` open as
/// `file`, by `number`: sets the call's result; the calls that wait deliver the signals due
/// after them.
pub fn call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
    file: Arc<OpenFile>,
    socket: Arc<InetSocket>,
) -> ControlFlow<()> {
    let [_, second, third, fourth, fifth, sixth] = context.syscall_arguments();
    let result = match number {
        number::BIND => bind(process, &socket, second, third),
        number::CONNECT => connect(process, &socket, second, third),
        number::GETSOCKNAME => name(process, &socket, second, third, false),
        number::GETPEERNAME => name(process, &socket, second, third, true),
        number::SHUTDOWN => shutdown::<P>(&socket, second),
        number::SETSOCKOPT => setsockopt(process, &socket, second, third, fourth, fifth),
        number::GETSOCKOPT => getsockopt(process, &socket, second, third, fourth, fifth),
        number::LISTEN | number::ACCEPT | number::ACCEPT4 => Err(Errno::EOPNOTSUPP),
        number::SENDTO => {
            let prepared = match ends_in_user_space::<P>(second, third) {
                true => Ok(Iovecs::one(second, third)),
                false => Err(Errno::EFAULT),
            };
            let destination = match fifth {
                0 => Ok(None),
                address => read_address(process, address, sixth),
            };
            let result = prepared.and_then(|buffers| {
                send(process, &socket, &buffers, destination?, fourth as u32)
            });
            return finish_send(process, thread, context, result, fourth as u32);
        }
        number::SENDMSG => {
            let result = read_header(process, third).and_then(|header| {
                if header.buffer_count > MAX_IOVECS {
                    return Err(Errno::EMSGSIZE);
                }
                let buffers = Iovecs::read(process, header.buffers, header.buffer_count)?;
                let destination = match (header.name, header.name_length) {
                    (0, _) | (_, 0) => None,
                    (address, length) => read_address(process, address, u64::from(length))?,
                };
                send(process, &socket, &buffers, destination, fourth as u32)
            });
            return finish_send(process, thread, context, result, fourth as u32);
        }
        number::RECVFROM => {
            if !ends_in_user_space::<P>(second, third) {
                return finish(process, thread, context, Err(Errno::EFAULT));
            }
            let receive = Receive {
                file,
                socket,
                buffers: Iovecs::one(second, third),
                flags: fourth as u32,
            };
            return recvfrom(process, thread, context, receive, fifth, sixth);
        }
        number::RECVMSG => return recvmsg(process, thread, context, file, socket, third, fourth),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn bind<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    address: u64,
    length: u64,
) -> Result<u64, Errno> {
    if (length as u32 as usize) < SOCKADDR_IN_SIZE {
        return Err(Errno::EINVAL);
    }
    let (address, port) = read_address(process, address, length)?.ok_or(Errno::EAFNOSUPPORT)?;
    let local = address.is_unspecified() || crate::net::is_local(address) || address.is_broadcast();
    if !local {
        return Err(Errno::EADDRNOTAVAIL);
    }
    let ping = socket.endpoint.protocol() == Protocol::Ping;
    if !ping && port != 0 && port < PRIVILEGED_PORTS && !is_privileged(process) {
        return Err(Errno::EACCES);
    }
    socket
        .endpoint
        .bind(address, port)
        .map(|_| 0)
        .map_err(bind_error)
}

fn connect<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    address: u64,
    length: u64,
) -> Result<u64, Errno> {
    match read_address(process, address, length)? {
        None => socket.endpoint.connect(None),
        Some((address, port)) => {
            ensure_bound(socket)?;
            let address = match address.is_unspecified() {
                true => Ipv4Address::LOOPBACK,
                false => address,
            };
            socket.endpoint.connect(Some((address, port)));
        }
    }
    Ok(0)
}

fn name<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    address: u64,
    length_at: u64,
    peer: bool,
) -> Result<u64, Errno> {
    let value = match peer {
        true => socket.endpoint.remote().ok_or(Errno::ENOTCONN)?,
        false => {
            let (address, port) = socket
                .endpoint
                .local()
                .unwrap_or((Ipv4Address::UNSPECIFIED, 0));
            match (address.is_unspecified(), socket.endpoint.remote()) {
                (true, Some((remote, _))) => (crate::net::source_for(remote), port),
                _ => (address, port),
            }
        }
    };
    if address == 0 {
        return Err(Errno::EFAULT);
    }
    write_address(process, address, length_at, value)?;
    Ok(0)
}

fn shutdown<P: Platform>(socket: &InetSocket, how: u64) -> Result<u64, Errno> {
    let (read, write) = match how as u32 {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(Errno::EINVAL),
    };
    if socket.endpoint.remote().is_none() {
        return Err(Errno::ENOTCONN);
    }
    socket.endpoint.shut_down(read, write);
    crate::console::wake_input_waiters::<P>();
    Ok(0)
}

fn setsockopt<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    level: u64,
    name: u64,
    value: u64,
    length: u64,
) -> Result<u64, Errno> {
    let length = length as u32 as usize;
    let options = &socket.options;
    match (level as u32, name as u32) {
        (
            SOL_SOCKET,
            option::RCVTIMEO_OLD | option::RCVTIMEO_NEW | option::SNDTIMEO_OLD | option::SNDTIMEO_NEW,
        ) => {
            if length < TIMEVAL_SIZE {
                return Err(Errno::EINVAL);
            }
            let mut bytes = [0u8; TIMEVAL_SIZE];
            process
                .read_user_memory(value, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let timeout = timeout_nanoseconds(bytes)?;
            let kept = match name as u32 {
                option::RCVTIMEO_OLD | option::RCVTIMEO_NEW => &options.receive_timeout,
                _ => &options.send_timeout,
            };
            kept.store(timeout, Ordering::Relaxed);
            Ok(0)
        }
        (
            SOL_SOCKET,
            option::SNDBUF | option::RCVBUF | option::REUSEADDR | SO_REUSEPORT | SO_BROADCAST,
        )
        | (SOL_IP, IP_TTL) => {
            if length < 4 {
                return Err(Errno::EINVAL);
            }
            let number = read_int(process, value)?;
            match (level as u32, name as u32) {
                (SOL_IP, _) => {
                    let ttl = match number {
                        -1 => 0,
                        1..=255 => number as u32,
                        _ => return Err(Errno::EINVAL),
                    };
                    options.ttl.store(ttl, Ordering::Relaxed);
                }
                (_, option::SNDBUF) => options
                    .send_buffer
                    .store(doubled(number), Ordering::Relaxed),
                (_, option::RCVBUF) => socket
                    .endpoint
                    .set_receive_limit(doubled(number) as usize),
                (_, option::REUSEADDR) => options
                    .reuse_address
                    .store(number != 0, Ordering::Relaxed),
                (_, SO_REUSEPORT) => options.reuse_port.store(number != 0, Ordering::Relaxed),
                _ => options.broadcast.store(number != 0, Ordering::Relaxed),
            }
            Ok(0)
        }
        _ => Err(Errno::ENOPROTOOPT),
    }
}

fn doubled(number: i32) -> u32 {
    (number.max(0) as u32).saturating_mul(2).max(4608)
}

fn getsockopt<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    level: u64,
    name: u64,
    value: u64,
    length_at: u64,
) -> Result<u64, Errno> {
    let room = read_int(process, length_at)?;
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let options = &socket.options;
    let int = |value: u32| {
        let mut bytes = [0u8; TIMEVAL_SIZE];
        bytes[..4].copy_from_slice(&value.to_ne_bytes());
        (bytes, 4)
    };
    let flag = |value: &AtomicBool| int(u32::from(value.load(Ordering::Relaxed)));
    let protocol = match socket.endpoint.protocol() {
        Protocol::Udp => IPPROTO_UDP,
        Protocol::Ping => IPPROTO_ICMP,
    };
    let (bytes, size) = match (level as u32, name as u32) {
        (SOL_SOCKET, option::TYPE) => int(SOCK_DGRAM),
        (SOL_SOCKET, option::DOMAIN) => int(AF_INET),
        (SOL_SOCKET, option::PROTOCOL) => int(protocol),
        (SOL_SOCKET, option::ERROR | option::ACCEPTCONN) => int(0),
        (SOL_SOCKET, option::SNDBUF) => int(match options.send_buffer.load(Ordering::Relaxed) {
            0 => endpoints::DEFAULT_RECEIVE_LIMIT as u32,
            bytes => bytes,
        }),
        (SOL_SOCKET, option::RCVBUF) => int(socket.endpoint.receive_limit() as u32),
        (SOL_SOCKET, option::REUSEADDR) => flag(&options.reuse_address),
        (SOL_SOCKET, SO_REUSEPORT) => flag(&options.reuse_port),
        (SOL_SOCKET, SO_BROADCAST) => flag(&options.broadcast),
        (SOL_SOCKET, option::RCVTIMEO_OLD | option::RCVTIMEO_NEW) => (
            timeval(options.receive_timeout.load(Ordering::Relaxed)),
            TIMEVAL_SIZE,
        ),
        (SOL_SOCKET, option::SNDTIMEO_OLD | option::SNDTIMEO_NEW) => (
            timeval(options.send_timeout.load(Ordering::Relaxed)),
            TIMEVAL_SIZE,
        ),
        (SOL_IP, IP_TTL) => int(u32::from(socket.ttl())),
        _ => return Err(Errno::ENOPROTOOPT),
    };
    let written = size.min(room as usize);
    process
        .write_user_memory(value, &bytes[..written])
        .map_err(|_| Errno::EFAULT)?;
    process
        .write_user_memory(length_at, &(written as u32).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

/// `ioctl` on an `AF_INET` socket: `FIONREAD` (`SIOCINQ`) the length of the datagram that
/// waits first, as an `int`; `None` for another request.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    request: u64,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    if request as u32 != crate::linux::terminal::request::FIONREAD {
        return None;
    }
    let length = socket.endpoint.next_length().unwrap_or(0);
    let length = i32::try_from(length).unwrap_or(i32::MAX);
    Some(
        process
            .write_user_memory(argument, &length.to_ne_bytes())
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT),
    )
}

fn gather<P: Platform>(process: &Process<P>, buffers: &Iovecs) -> Result<Vec<u8>, Errno> {
    let total = buffers.total();
    if total > endpoints::MAX_DATAGRAM_BYTES as u64 {
        return Err(Errno::EMSGSIZE);
    }
    let total = total as usize;
    let mut bytes = Vec::new();
    if !crate::heap::may_grow_for_user(total) || bytes.try_reserve_exact(total).is_err() {
        return Err(Errno::ENOBUFS);
    }
    bytes.resize(total, 0);
    let mut at = 0;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length as usize;
        if length == 0 {
            continue;
        }
        process
            .read_user_memory(base, &mut bytes[at..at + length])
            .map_err(|_| Errno::EFAULT)?;
        at += length;
    }
    Ok(bytes)
}

fn is_broadcast(address: Ipv4Address) -> bool {
    address.is_broadcast()
        || crate::net::subnet_broadcast().is_some_and(|broadcast| broadcast == address)
}

fn send<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    buffers: &Iovecs,
    destination: Option<(Ipv4Address, u16)>,
    flags: u32,
) -> Result<u64, Errno> {
    if flags & message::OOB != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    if socket.endpoint.is_write_closed() {
        return Err(Errno::EPIPE);
    }
    let destination = match destination.or(socket.endpoint.remote()) {
        Some(destination) => destination,
        None => return Err(Errno::EDESTADDRREQ),
    };
    let (address, port) = destination;
    let address = match address.is_unspecified() {
        true => Ipv4Address::LOOPBACK,
        false => address,
    };
    if is_broadcast(address) && !socket.options.broadcast.load(Ordering::Relaxed) {
        return Err(Errno::EACCES);
    }
    let data = gather(process, buffers)?;
    let (local, local_port) = ensure_bound(socket)?;
    let ttl = socket.ttl();
    let sent = match socket.endpoint.protocol() {
        Protocol::Udp => {
            crate::net::send_udp::<P>(local, local_port, (address, port), &data, ttl)
        }
        Protocol::Ping => {
            let Some((header, payload)) = data.split_first_chunk::<{ icmp::HEADER_BYTES }>()
            else {
                return Err(Errno::EINVAL);
            };
            if header[0] != icmp::kind::ECHO_REQUEST || header[1] != 0 {
                return Err(Errno::EINVAL);
            }
            let sequence = u16::from_be_bytes([header[6], header[7]]);
            crate::net::send_echo::<P>(address, local_port, sequence, payload, ttl)
        }
    };
    sent.map_err(send_error)?;
    Ok(data.len() as u64)
}

fn finish_send<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    result: Result<u64, Errno>,
    flags: u32,
) -> ControlFlow<()> {
    if result == Err(Errno::EPIPE) && flags & message::NOSIGNAL == 0 {
        signals::send_from_kernel_to_self(process, thread, crate::linux::signal::SIGPIPE);
    }
    finish(process, thread, context, result)
}

/// `write` or `writev` on an `AF_INET` socket: a send to its peer with no flags.
pub fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    socket: &Arc<InetSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let result = send(process, socket, &buffers, None, 0);
    finish_send(process, thread, context, result, 0)
}

struct Receive {
    file: Arc<OpenFile>,
    socket: Arc<InetSocket>,
    buffers: Iovecs,
    flags: u32,
}

struct Received {
    copied: u64,
    length: u64,
    source: (Ipv4Address, u16),
}

fn recvfrom<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    receive: Receive,
    address: u64,
    length_at: u64,
) -> ControlFlow<()> {
    let result = match receive_datagram(process, thread, context, &receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    let result = result.and_then(|received| {
        write_address(process, address, length_at, received.source)?;
        Ok(match receive.flags & MSG_TRUNC {
            0 => received.copied,
            _ => received.length,
        })
    });
    finish(process, thread, context, result)
}

/// `read` or `readv` on an `AF_INET` socket: a receive with no flags.
pub fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let receive = Receive {
        file: Arc::clone(file),
        socket: Arc::clone(socket),
        buffers,
        flags: 0,
    };
    let result = match receive_datagram(process, thread, context, &receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    finish(process, thread, context, result.map(|received| received.copied))
}

fn recvmsg<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: Arc<OpenFile>,
    socket: Arc<InetSocket>,
    header_at: u64,
    flags: u64,
) -> ControlFlow<()> {
    let prepared = read_header(process, header_at).and_then(|header| {
        if header.buffer_count > MAX_IOVECS {
            return Err(Errno::EMSGSIZE);
        }
        let buffers = Iovecs::read(process, header.buffers, header.buffer_count)?;
        Ok((header, buffers))
    });
    let (header, buffers) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let receive = Receive {
        file,
        socket,
        buffers,
        flags: flags as u32,
    };
    let result = match receive_datagram(process, thread, context, &receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    let result = result.and_then(|received| {
        let mut written = [0u8; MSGHDR_SIZE];
        process
            .read_user_memory(header_at, &mut written)
            .map_err(|_| Errno::EFAULT)?;
        if header.name != 0 {
            let bytes = encode_address(received.source.0, received.source.1);
            let copied = SOCKADDR_IN_SIZE.min(header.name_length as usize);
            process
                .write_user_memory(header.name, &bytes[..copied])
                .map_err(|_| Errno::EFAULT)?;
            written[8..12].copy_from_slice(&(SOCKADDR_IN_SIZE as u32).to_ne_bytes());
        }
        let truncated = received.copied < received.length;
        let message_flags = if truncated { MSG_TRUNC } else { 0 };
        written[40..48].copy_from_slice(&0u64.to_ne_bytes());
        written[48..52].copy_from_slice(&message_flags.to_ne_bytes());
        process
            .write_user_memory(header_at, &written)
            .map_err(|_| Errno::EFAULT)?;
        Ok(match receive.flags & MSG_TRUNC {
            0 => received.copied,
            _ => received.length,
        })
    });
    finish(process, thread, context, result)
}

fn receive_datagram<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    receive: &Receive,
) -> ControlFlow<(), Option<Result<Received, Errno>>> {
    let timeout = receive
        .socket
        .options
        .receive_timeout
        .load(Ordering::Relaxed);
    let interrupted = interrupted(context, timeout);
    let deadline = deadline::<P>(timeout);
    loop {
        match receive_or_wait(process, thread, receive, deadline)? {
            Some(result) => return ControlFlow::Continue(Some(result)),
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(None);
                }
            }
        }
    }
}

fn receive_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    receive: &Receive,
    deadline: u64,
) -> ControlFlow<(), Option<Result<Received, Errno>>> {
    if receive.flags & message::OOB != 0 {
        return ControlFlow::Continue(Some(Err(Errno::EOPNOTSUPP)));
    }
    if receive.flags & MSG_ERRQUEUE != 0 {
        return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
    }
    let endpoint = &receive.socket.endpoint;
    let peek = receive.flags & message::PEEK != 0;
    let nonblocking = receive.file.is_nonblocking() || receive.flags & message::DONTWAIT != 0;
    loop {
        if let Some(datagram) = endpoint.take(peek) {
            return ControlFlow::Continue(Some(copy_out(process, &receive.buffers, &datagram)));
        }
        if endpoint.is_read_closed() {
            return ControlFlow::Continue(Some(Ok(Received {
                copied: 0,
                length: 0,
                source: endpoint.remote().unwrap_or((Ipv4Address::UNSPECIFIED, 0)),
            })));
        }
        if nonblocking {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let ready = || endpoint.has_datagrams() || endpoint.is_read_closed();
        match wait_for_input(process, thread, deadline, ready)? {
            Waited::Ready => continue,
            Waited::TimedOut => return ControlFlow::Continue(Some(Err(Errno::EAGAIN))),
            Waited::Signalled => return ControlFlow::Continue(None),
        }
    }
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    datagram: &endpoints::Datagram,
) -> Result<Received, Errno> {
    let bytes = &datagram.bytes;
    let mut copied = 0usize;
    for entry in buffers.entries() {
        if copied == bytes.len() {
            break;
        }
        let (base, length) = iovec(entry);
        let count = (length as usize).min(bytes.len() - copied);
        if count == 0 {
            continue;
        }
        process
            .write_user_memory(base, &bytes[copied..copied + count])
            .map_err(|_| Errno::EFAULT)?;
        copied += count;
    }
    Ok(Received {
        copied: copied as u64,
        length: bytes.len() as u64,
        source: (datagram.source, datagram.source_port),
    })
}

#[cfg(test)]
mod tests;

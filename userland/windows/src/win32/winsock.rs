//! wsock32, Winsock 1.1 (D178), on EggOS's IPv4 sockets (D157, D158): a socket is a Linux
//! descriptor with a file handle's value, the calls are the Linux ones with Winsock's error
//! codes in the thread's last error, and the functions programs import by ordinal are found by
//! Winsock 1.1's ordinal numbers. A name resolves from a dotted address, `/etc/hosts`, the
//! machine's own name (its interfaces' addresses) or a DNS query to the servers
//! `/etc/resolv.conf` names; `hostent`, `servent` and `inet_ntoa`'s text are one buffer per
//! process that the next call reuses. Windows' message-based notification (`WSAAsyncSelect`)
//! is not there yet.

use super::file::{fd_of, handle_of};
use super::{SpinLock, fail, thread_environment};
use crate::library::Export;
use crate::linux::{self, AF_INET, MSG_NOSIGNAL, O_NONBLOCK};
use crate::sockets::{self, code};
use core::sync::atomic::{AtomicU32, Ordering};
use eggos_net::dns::{self, ResponseError};
use eggos_net::{Ipv4Address, status};

const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
const INADDR_NONE: u32 = u32::MAX;
const AF_INET_WINDOWS: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const FIONBIO: u32 = 0x8004_667e;
const FIONREAD: u32 = 0x4004_667f;
const MSG_OOB_PEEK: i32 = 0x3;
const MSG_WAITALL: i32 = 0x8;
const LINUX_MSG_WAITALL: usize = 0x100;
const EINPROGRESS: u16 = 115;
const SOCKADDR_IN_SIZE: usize = 16;
const HIGHEST_VERSION: u16 = 0x0202;
const WSADATA_SIZE: usize = 408;
const WSADATA_DESCRIPTION: usize = 16;
const WSADATA_STATUS: usize = 273;
const DNS_TIMEOUT_NANOSECONDS: u64 = 2_000_000_000;
const DNS_ATTEMPTS: usize = 2;
const MAX_INTERFACES: usize = 4;
pub(super) const MAX_ADDRESSES: usize = dns::MAX_ADDRESSES;
const ADDRESS_BYTES: i16 = 4;

static STARTED: AtomicU32 = AtomicU32::new(0);

#[allow(dead_code, reason = "the program reads it")]
#[repr(C)]
struct HostEntry {
    name: usize,
    aliases: usize,
    address_type: i16,
    length: i16,
    addresses: usize,
}

#[allow(dead_code, reason = "the program reads it")]
#[repr(C)]
struct ServiceEntry {
    name: usize,
    aliases: usize,
    protocol: usize,
    port: i16,
}

struct Answers {
    host: HostEntry,
    host_aliases: [usize; 1],
    host_addresses: [usize; MAX_ADDRESSES + 1],
    address_bytes: [[u8; 4]; MAX_ADDRESSES],
    host_name: [u8; 256],
    service: ServiceEntry,
    service_aliases: [usize; 1],
    service_name: [u8; 32],
    service_protocol: [u8; 8],
    dotted: [u8; 16],
}

static ANSWERS: SpinLock<Answers> = SpinLock::new(Answers {
    host: HostEntry {
        name: 0,
        aliases: 0,
        address_type: 0,
        length: 0,
        addresses: 0,
    },
    host_aliases: [0],
    host_addresses: [0; MAX_ADDRESSES + 1],
    address_bytes: [[0; 4]; MAX_ADDRESSES],
    host_name: [0; 256],
    service: ServiceEntry {
        name: 0,
        aliases: 0,
        protocol: 0,
        port: 0,
    },
    service_aliases: [0],
    service_name: [0; 32],
    service_protocol: [0; 8],
    dotted: [0; 16],
});

fn started() -> bool {
    STARTED.load(Ordering::Acquire) != 0
}

fn socket_fd(socket: usize) -> Result<usize, u32> {
    if !started() {
        return Err(code::WSANOTINITIALISED);
    }
    fd_of(socket).ok_or(code::WSAENOTSOCK)
}

fn count_or_error(result: Result<usize, eggos_runtime::errno::Errno>) -> i32 {
    match result {
        Ok(count) => count.min(i32::MAX as usize) as i32,
        Err(errno) => fail(sockets::error_for(errno.0), SOCKET_ERROR),
    }
}

/// A socket address's bytes for `address` and `port`, as Linux and Windows lay out
/// `sockaddr_in`.
pub(super) fn socket_address(address: [u8; 4], port: u16) -> [u8; SOCKADDR_IN_SIZE] {
    let mut bytes = [0u8; SOCKADDR_IN_SIZE];
    bytes[0..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    bytes[2..4].copy_from_slice(&port.to_be_bytes());
    bytes[4..8].copy_from_slice(&address);
    bytes
}

fn query(server: Ipv4Address, name: &[u8]) -> Result<dns::Answers, u32> {
    let fd = linux::socket(AF_INET, SOCK_DGRAM as usize, 0)
        .map_err(|errno| sockets::error_for(errno.0))?;
    let answer = exchange(fd, server, name);
    let _ = linux::close(fd);
    answer
}

fn exchange(fd: usize, server: Ipv4Address, name: &[u8]) -> Result<dns::Answers, u32> {
    linux::connect(fd, &socket_address(server.0, dns::PORT))
        .map_err(|errno| sockets::error_for(errno.0))?;
    let id = (linux::clock_nanoseconds(linux::CLOCK_MONOTONIC) as u16) ^ 0x5eed;
    let mut message = [0u8; dns::MAX_MESSAGE_BYTES];
    let length = dns::write_query(&mut message, id, name, dns::record_type::A)
        .map_err(|_| code::WSAHOST_NOT_FOUND)?;
    for _ in 0..DNS_ATTEMPTS {
        linux::send(fd, &message[..length], MSG_NOSIGNAL)
            .map_err(|errno| sockets::error_for(errno.0))?;
        let deadline = linux::clock_nanoseconds(linux::CLOCK_MONOTONIC) + DNS_TIMEOUT_NANOSECONDS;
        loop {
            let now = linux::clock_nanoseconds(linux::CLOCK_MONOTONIC);
            if now >= deadline {
                break;
            }
            if !matches!(
                linux::wait_readable([fd as i32, -1], Some(deadline - now)),
                Ok(1..)
            ) {
                continue;
            }
            let mut answer = [0u8; dns::MAX_MESSAGE_BYTES];
            let Ok(received) = linux::receive(fd, &mut answer, 0) else {
                break;
            };
            match dns::parse_response(&answer[..received], id) {
                Ok(answers) if !answers.addresses().is_empty() => return Ok(answers),
                Ok(_) | Err(ResponseError::NameNotFound) => return Err(code::WSAHOST_NOT_FOUND),
                Err(ResponseError::NotOurs) => continue,
                Err(_) => return Err(code::WSATRY_AGAIN),
            }
        }
    }
    Err(code::WSATRY_AGAIN)
}

fn machine_addresses(out: &mut [[u8; 4]; MAX_ADDRESSES]) -> usize {
    let mut text = [0u8; 2048];
    let length = linux::read_file(b"/proc/net/interfaces\0", &mut text).unwrap_or(0);
    let (interfaces, count) = status::parse_interfaces::<MAX_INTERFACES>(&text[..length]);
    let mut found = 0;
    for interface in interfaces[..count].iter().flatten() {
        if let Some(subnet) = interface.subnet
            && !interface.is_loopback()
            && found < out.len()
        {
            out[found] = subnet.address.0;
            found += 1;
        }
    }
    if found == 0 {
        out[0] = Ipv4Address::LOOPBACK.0;
        found = 1;
    }
    found
}

/// The IPv4 addresses `name` resolves to into `out`: a dotted address, a name in `/etc/hosts`,
/// the machine's own name or a DNS answer; how many, or the Winsock error.
pub(super) fn resolve(name: &[u8], out: &mut [[u8; 4]; MAX_ADDRESSES]) -> Result<usize, u32> {
    if let Some(address) = Ipv4Address::parse(name) {
        out[0] = address.0;
        return Ok(1);
    }
    let mut hosts = [0u8; 4096];
    let length = linux::read_file(b"/etc/hosts\0", &mut hosts).unwrap_or(0);
    if let Some(address) = sockets::host_address(&hosts[..length], name) {
        out[0] = address;
        return Ok(1);
    }
    let mut host = [0u8; super::system::HOST_NAME_CAPACITY];
    let host_length = super::system::host_name(&mut host);
    if host_length > 0 && host[..host_length].eq_ignore_ascii_case(name) {
        return Ok(machine_addresses(out));
    }
    let mut resolv_conf = [0u8; 1024];
    let length = linux::read_file(b"/etc/resolv.conf\0", &mut resolv_conf).unwrap_or(0);
    let mut failure = code::WSAHOST_NOT_FOUND;
    for server in dns::nameservers(&resolv_conf[..length])
        .into_iter()
        .flatten()
    {
        match query(server, name) {
            Ok(answers) => {
                let addresses = answers.addresses();
                for (slot, address) in out.iter_mut().zip(addresses) {
                    *slot = address.0;
                }
                return Ok(addresses.len().min(MAX_ADDRESSES));
            }
            Err(code::WSAHOST_NOT_FOUND) => return Err(code::WSAHOST_NOT_FOUND),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

unsafe extern "efiapi" fn wsa_startup(version: u16, data: *mut u8) -> i32 {
    if data.is_null() {
        return code::WSAEFAULT as i32;
    }
    let (major, minor) = (version & 0xff, version >> 8);
    if major == 0 {
        return code::WSAVERNOTSUPPORTED as i32;
    }
    let granted = match (major, minor) {
        (1, _) | (2, 0..=2) => version,
        _ => HIGHEST_VERSION,
    };
    let mut out = [0u8; WSADATA_SIZE];
    out[0..2].copy_from_slice(&granted.to_le_bytes());
    out[2..4].copy_from_slice(&HIGHEST_VERSION.to_le_bytes());
    let description = b"EggOS IPv4 sockets";
    out[WSADATA_DESCRIPTION..WSADATA_DESCRIPTION + description.len()].copy_from_slice(description);
    let running = b"Running";
    out[WSADATA_STATUS..WSADATA_STATUS + running.len()].copy_from_slice(running);
    // SAFETY: `WSAStartup`'s contract: a writable `WSADATA`.
    unsafe { data.copy_from_nonoverlapping(out.as_ptr(), WSADATA_SIZE) };
    STARTED.fetch_add(1, Ordering::AcqRel);
    0
}

extern "efiapi" fn wsa_cleanup() -> i32 {
    let ended = STARTED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        count.checked_sub(1)
    });
    match ended {
        Ok(_) => 0,
        Err(_) => fail(code::WSANOTINITIALISED, SOCKET_ERROR),
    }
}

extern "efiapi" fn wsa_get_last_error() -> i32 {
    thread_environment::get_last_error() as i32
}

extern "efiapi" fn wsa_set_last_error(error: i32) {
    thread_environment::set_last_error(error as u32);
}

extern "efiapi" fn socket(family: i32, kind: i32, protocol: i32) -> usize {
    if !started() {
        return fail(code::WSANOTINITIALISED, INVALID_SOCKET);
    }
    if family != AF_INET_WINDOWS {
        return fail(code::WSAEAFNOSUPPORT, INVALID_SOCKET);
    }
    if kind != SOCK_STREAM && kind != SOCK_DGRAM {
        return fail(code::WSAESOCKTNOSUPPORT, INVALID_SOCKET);
    }
    match linux::socket(AF_INET, kind as usize, protocol.max(0) as usize) {
        Ok(fd) => handle_of(fd),
        Err(errno) => fail(sockets::error_for(errno.0), INVALID_SOCKET),
    }
}

extern "efiapi" fn close_socket(socket: usize) -> i32 {
    match socket_fd(socket) {
        Ok(fd) => count_or_error(linux::close(fd)).min(0),
        Err(error) => fail(error, SOCKET_ERROR),
    }
}

unsafe extern "efiapi" fn connect(socket: usize, address: *const u8, length: i32) -> i32 {
    let fd = match socket_fd(socket) {
        Ok(fd) => fd,
        Err(error) => return fail(error, SOCKET_ERROR),
    };
    if address.is_null() || (length as usize) < SOCKADDR_IN_SIZE {
        return fail(code::WSAEFAULT, SOCKET_ERROR);
    }
    let mut bytes = [0u8; SOCKADDR_IN_SIZE];
    // SAFETY: `connect`'s contract: `length` readable bytes, at least a `sockaddr_in`.
    unsafe {
        bytes
            .as_mut_ptr()
            .copy_from_nonoverlapping(address, SOCKADDR_IN_SIZE)
    };
    if u16::from_le_bytes([bytes[0], bytes[1]]) != AF_INET as u16 {
        return fail(code::WSAEAFNOSUPPORT, SOCKET_ERROR);
    }
    match linux::connect(fd, &bytes) {
        Ok(_) => 0,
        Err(errno) if errno.0 == EINPROGRESS => fail(code::WSAEWOULDBLOCK, SOCKET_ERROR),
        Err(errno) => fail(sockets::error_for(errno.0), SOCKET_ERROR),
    }
}

unsafe extern "efiapi" fn send(socket: usize, buffer: *const u8, length: i32, flags: i32) -> i32 {
    let fd = match socket_fd(socket) {
        Ok(fd) => fd,
        Err(error) => return fail(error, SOCKET_ERROR),
    };
    if length < 0 || (length > 0 && buffer.is_null()) {
        return fail(code::WSAEFAULT, SOCKET_ERROR);
    }
    let bytes = match length {
        0 => &[][..],
        // SAFETY: `send`'s contract: `length` readable bytes.
        _ => unsafe { core::slice::from_raw_parts(buffer, length as usize) },
    };
    count_or_error(linux::send(
        fd,
        bytes,
        (flags & 0x5) as usize | MSG_NOSIGNAL,
    ))
}

unsafe extern "efiapi" fn recv(socket: usize, buffer: *mut u8, length: i32, flags: i32) -> i32 {
    let fd = match socket_fd(socket) {
        Ok(fd) => fd,
        Err(error) => return fail(error, SOCKET_ERROR),
    };
    if length < 0 || (length > 0 && buffer.is_null()) {
        return fail(code::WSAEFAULT, SOCKET_ERROR);
    }
    let bytes = match length {
        0 => &mut [][..],
        // SAFETY: `recv`'s contract: `length` writable bytes.
        _ => unsafe { core::slice::from_raw_parts_mut(buffer, length as usize) },
    };
    let mut linux_flags = (flags & MSG_OOB_PEEK) as usize;
    if flags & MSG_WAITALL != 0 {
        linux_flags |= LINUX_MSG_WAITALL;
    }
    count_or_error(linux::receive(fd, bytes, linux_flags))
}

extern "efiapi" fn shutdown(socket: usize, how: i32) -> i32 {
    match socket_fd(socket) {
        Ok(_) if !(0..=2).contains(&how) => fail(code::WSAEINVAL, SOCKET_ERROR),
        Ok(fd) => count_or_error(linux::shutdown(fd, how as usize)).min(0),
        Err(error) => fail(error, SOCKET_ERROR),
    }
}

unsafe extern "efiapi" fn ioctl_socket(socket: usize, command: i32, argument: *mut u32) -> i32 {
    let fd = match socket_fd(socket) {
        Ok(fd) => fd,
        Err(error) => return fail(error, SOCKET_ERROR),
    };
    if argument.is_null() {
        return fail(code::WSAEFAULT, SOCKET_ERROR);
    }
    match command as u32 {
        FIONBIO => {
            // SAFETY: `ioctlsocket`'s contract: a readable `u_long`.
            let blocking = unsafe { argument.read() } == 0;
            let changed = linux::control(fd, linux::F_GETFL, 0).and_then(|flags| {
                let flags = match blocking {
                    true => flags & !O_NONBLOCK,
                    false => flags | O_NONBLOCK,
                };
                linux::control(fd, linux::F_SETFL, flags)
            });
            count_or_error(changed).min(0)
        }
        FIONREAD => match linux::bytes_to_read(fd) {
            Ok(count) => {
                // SAFETY: as above: a writable `u_long`.
                unsafe { argument.write(count.min(u32::MAX as usize) as u32) };
                0
            }
            Err(errno) => fail(sockets::error_for(errno.0), SOCKET_ERROR),
        },
        _ => fail(code::WSAEINVAL, SOCKET_ERROR),
    }
}

extern "efiapi" fn htonl(value: u32) -> u32 {
    value.to_be()
}

extern "efiapi" fn htons(value: u16) -> u16 {
    value.to_be()
}

extern "efiapi" fn ntohs(value: u16) -> u16 {
    u16::from_be(value)
}

unsafe extern "efiapi" fn inet_addr(text: *const u8) -> u32 {
    if text.is_null() {
        return INADDR_NONE;
    }
    // SAFETY: `inet_addr`'s contract: a NUL-terminated string.
    let text = unsafe { core::ffi::CStr::from_ptr(text.cast()) }.to_bytes();
    sockets::parse_address(text).map_or(INADDR_NONE, u32::from_ne_bytes)
}

extern "efiapi" fn inet_ntoa(address: u32) -> *const u8 {
    ANSWERS.with(|answers| {
        let length = sockets::write_dotted(address.to_ne_bytes(), &mut answers.dotted);
        answers.dotted[length.min(15)] = 0;
        answers.dotted.as_ptr()
    })
}

fn host_entry(name: &[u8], addresses: &[[u8; 4]]) -> *const HostEntry {
    ANSWERS.with(|answers| {
        let length = name.len().min(answers.host_name.len() - 1);
        answers.host_name[..length].copy_from_slice(&name[..length]);
        answers.host_name[length] = 0;
        let count = addresses.len().min(MAX_ADDRESSES);
        answers.address_bytes[..count].copy_from_slice(&addresses[..count]);
        answers.host_addresses = [0; MAX_ADDRESSES + 1];
        for (slot, bytes) in answers
            .host_addresses
            .iter_mut()
            .zip(&answers.address_bytes[..count])
        {
            *slot = bytes.as_ptr() as usize;
        }
        answers.host_aliases[0] = 0;
        answers.host = HostEntry {
            name: answers.host_name.as_ptr() as usize,
            aliases: answers.host_aliases.as_ptr() as usize,
            address_type: AF_INET_WINDOWS as i16,
            length: ADDRESS_BYTES,
            addresses: answers.host_addresses.as_ptr() as usize,
        };
        &raw const answers.host
    })
}

unsafe extern "efiapi" fn gethostbyname(name: *const u8) -> *const HostEntry {
    if !started() {
        return fail(code::WSANOTINITIALISED, core::ptr::null());
    }
    let mut host = [0u8; super::system::HOST_NAME_CAPACITY];
    let name = match name.is_null() {
        true => {
            let length = super::system::host_name(&mut host);
            &host[..length]
        }
        // SAFETY: `gethostbyname`'s contract: a NUL-terminated name.
        false => unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes(),
    };
    let mut addresses = [[0u8; 4]; MAX_ADDRESSES];
    match resolve(name, &mut addresses) {
        Ok(count) => host_entry(name, &addresses[..count]),
        Err(error) => fail(error, core::ptr::null()),
    }
}

unsafe extern "efiapi" fn gethostbyaddr(
    address: *const u8,
    length: i32,
    kind: i32,
) -> *const HostEntry {
    if !started() {
        return fail(code::WSANOTINITIALISED, core::ptr::null());
    }
    if kind != AF_INET_WINDOWS {
        return fail(code::WSAEAFNOSUPPORT, core::ptr::null());
    }
    if address.is_null() || length < i32::from(ADDRESS_BYTES) {
        return fail(code::WSAEFAULT, core::ptr::null());
    }
    let mut bytes = [0u8; 4];
    // SAFETY: `gethostbyaddr`'s contract: `length` readable bytes, at least four.
    unsafe { bytes.as_mut_ptr().copy_from_nonoverlapping(address, 4) };
    let mut hosts = [0u8; 4096];
    let read = linux::read_file(b"/etc/hosts\0", &mut hosts).unwrap_or(0);
    match sockets::host_name(&hosts[..read], bytes) {
        Some(name) => host_entry(name, &[bytes]),
        None => fail(code::WSANO_DATA, core::ptr::null()),
    }
}

fn service_entry(service: sockets::Service) -> *const ServiceEntry {
    ANSWERS.with(|answers| {
        let name = service.name.len().min(answers.service_name.len() - 1);
        answers.service_name[..name].copy_from_slice(&service.name[..name]);
        answers.service_name[name] = 0;
        let protocol = service
            .protocol
            .len()
            .min(answers.service_protocol.len() - 1);
        answers.service_protocol[..protocol].copy_from_slice(&service.protocol[..protocol]);
        answers.service_protocol[protocol] = 0;
        answers.service_aliases[0] = 0;
        answers.service = ServiceEntry {
            name: answers.service_name.as_ptr() as usize,
            aliases: answers.service_aliases.as_ptr() as usize,
            protocol: answers.service_protocol.as_ptr() as usize,
            port: service.port.to_be() as i16,
        };
        &raw const answers.service
    })
}

unsafe fn protocol_of<'a>(protocol: *const u8) -> Option<&'a [u8]> {
    match protocol.is_null() {
        true => None,
        // SAFETY: the callers' contract: a NUL-terminated protocol name, or null.
        false => Some(unsafe { core::ffi::CStr::from_ptr(protocol.cast()) }.to_bytes()),
    }
}

unsafe extern "efiapi" fn getservbyname(
    name: *const u8,
    protocol: *const u8,
) -> *const ServiceEntry {
    if !started() {
        return fail(code::WSANOTINITIALISED, core::ptr::null());
    }
    if name.is_null() {
        return fail(code::WSAEFAULT, core::ptr::null());
    }
    // SAFETY: `getservbyname`'s contract: a NUL-terminated name.
    let name = unsafe { core::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    // SAFETY: as above: a NUL-terminated protocol, or null.
    let protocol = unsafe { protocol_of(protocol) };
    match sockets::service_by_name(name, protocol) {
        Some(service) => service_entry(service),
        None => fail(code::WSANO_DATA, core::ptr::null()),
    }
}

unsafe extern "efiapi" fn getservbyport(port: i32, protocol: *const u8) -> *const ServiceEntry {
    if !started() {
        return fail(code::WSANOTINITIALISED, core::ptr::null());
    }
    // SAFETY: `getservbyport`'s contract: a NUL-terminated protocol, or null.
    let protocol = unsafe { protocol_of(protocol) };
    match sockets::service_by_port(u16::from_be(port as u16), protocol) {
        Some(service) => service_entry(service),
        None => fail(code::WSANO_DATA, core::ptr::null()),
    }
}

unsafe extern "efiapi" fn gethostname(name: *mut u8, length: i32) -> i32 {
    if !started() {
        return fail(code::WSANOTINITIALISED, SOCKET_ERROR);
    }
    let mut host = [0u8; super::system::HOST_NAME_CAPACITY];
    let host_length = super::system::host_name(&mut host);
    if name.is_null() || length < 0 || host_length >= length as usize {
        return fail(code::WSAEFAULT, SOCKET_ERROR);
    }
    // SAFETY: `gethostname`'s contract: `length` writable bytes, more than the name takes.
    let out = unsafe { core::slice::from_raw_parts_mut(name, host_length + 1) };
    out[..host_length].copy_from_slice(&host[..host_length]);
    out[host_length] = 0;
    0
}

extern "efiapi" fn wsa_async_select(
    socket: usize,
    _window: usize,
    _message: u32,
    _events: i32,
) -> i32 {
    match socket_fd(socket) {
        Ok(_) => fail(code::WSAEOPNOTSUPP, SOCKET_ERROR),
        Err(error) => fail(error, SOCKET_ERROR),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"closesocket" => close_socket,
    b"connect" => connect,
    b"htonl" => htonl,
    b"htons" => htons,
    b"inet_addr" => inet_addr,
    b"inet_ntoa" => inet_ntoa,
    b"ioctlsocket" => ioctl_socket,
    b"ntohs" => ntohs,
    b"recv" => recv,
    b"send" => send,
    b"shutdown" => shutdown,
    b"socket" => socket,
    b"gethostbyaddr" => gethostbyaddr,
    b"gethostbyname" => gethostbyname,
    b"getservbyname" => getservbyname,
    b"getservbyport" => getservbyport,
    b"gethostname" => gethostname,
    b"WSAAsyncSelect" => wsa_async_select,
    b"WSAGetLastError" => wsa_get_last_error,
    b"WSASetLastError" => wsa_set_last_error,
    b"WSAStartup" => wsa_startup,
    b"WSACleanup" => wsa_cleanup,
];

/// The function Winsock 1.1's ordinal `ordinal` names, if the layer has it.
pub(super) fn by_ordinal(ordinal: u16) -> Option<*const ()> {
    let name: &[u8] = match ordinal {
        3 => b"closesocket",
        4 => b"connect",
        8 => b"htonl",
        9 => b"htons",
        10 => b"inet_addr",
        11 => b"inet_ntoa",
        12 => b"ioctlsocket",
        15 => b"ntohs",
        16 => b"recv",
        19 => b"send",
        22 => b"shutdown",
        23 => b"socket",
        51 => b"gethostbyaddr",
        52 => b"gethostbyname",
        55 => b"getservbyname",
        56 => b"getservbyport",
        57 => b"gethostname",
        101 => b"WSAAsyncSelect",
        111 => b"WSAGetLastError",
        112 => b"WSASetLastError",
        115 => b"WSAStartup",
        116 => b"WSACleanup",
        _ => return None,
    };
    crate::library::find(&[EXPORTS], name)
}

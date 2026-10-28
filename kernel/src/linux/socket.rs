//! The socket calls (D138): `socket`, `socketpair`, `bind`, `listen`, `accept`, `accept4`,
//! `connect`, `sendto`, `sendmsg`, `recvfrom`, `recvmsg`, `getsockname`, `getpeername`,
//! `shutdown`, `setsockopt` and `getsockopt`, and `read`, `write` and their vector forms on a
//! socket, for `AF_UNIX` stream sockets ([`super::unix_socket`]).
//!
//! The calls read their arguments as Linux's ABI lays them out on the three architectures:
//! `struct sockaddr_un` (a family and up to 108 bytes of path), `struct msghdr` and
//! `struct cmsghdr` with 64-bit lengths, `struct ucred`, and `struct timeval` for the
//! timeouts. Errors are Linux's, in Linux's order where it has one. The calls that wait
//! (`accept`, `connect` on a full backlog, and those that send and receive) wait until their
//! socket changes, their timeout passes or a signal comes, and restart after a handler with
//! `SA_RESTART` unless the socket has a timeout. A send to a peer that is gone fails with
//! `EPIPE` and sends `SIGPIPE`, unless `MSG_NOSIGNAL`.

use super::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use super::file_changes::{UMASK, now, parent_and_name, refused};
use super::files::{starting_points, walk_error};
use super::poll::deadline_in;
use super::signals::{self, Interrupted};
use super::terminal::{Waited, wait_for_input};
use super::thread::LinuxThread;
use super::unix_socket::{
    self, Address, BindingKey, Connection, NotConnected, OutgoingRights, PeerCredentials, Side,
    Taken, UnixSocket,
};
use super::{
    Errno, Iovecs, MAX_IOVECS, MAX_RW_COUNT, credentials, ends_in_user_space, iovec, number,
};
use crate::console;
use crate::process::Process;
use crate::vfs::{self, Node, NodeKind, Refused};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::Ordering;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// The only address family: local sockets (`AF_UNIX`, `PF_UNIX`).
pub const AF_UNIX: u32 = 1;

/// The only socket type: a stream (`SOCK_STREAM`).
pub const SOCK_STREAM: u32 = 1;

const SOCK_TYPE_MASK: u32 = 0xf;

const SOCK_MAX: u32 = 11;

const SOCKET_FLAGS: u32 = open_flag::CLOEXEC | open_flag::NONBLOCK;

/// The flags of `send` and `recv` (`MSG_*`), the generic values, which x86_64 shares.
pub mod message {
    /// Out-of-band data: not supported (`EOPNOTSUPP`).
    pub const OOB: u32 = 0x1;
    /// Read without taking.
    pub const PEEK: u32 = 0x2;
    /// The control buffer was too small (returned).
    pub const CTRUNC: u32 = 0x8;
    /// Do not wait.
    pub const DONTWAIT: u32 = 0x40;
    /// Wait until the whole buffer is read.
    pub const WAITALL: u32 = 0x100;
    /// Do not send `SIGPIPE`.
    pub const NOSIGNAL: u32 = 0x4000;
    /// Received descriptors are closed by `execve`.
    pub const CMSG_CLOEXEC: u32 = 0x4000_0000;
}

/// The socket level of `setsockopt`, `getsockopt` and control messages (`SOL_SOCKET`).
pub const SOL_SOCKET: u32 = 1;

/// A control message of descriptors (`SCM_RIGHTS`).
pub const SCM_RIGHTS: u32 = 1;

/// The socket options answered (`SO_*`), the generic values, which x86_64 shares.
pub mod option {
    pub const REUSEADDR: u32 = 2;
    pub const TYPE: u32 = 3;
    pub const ERROR: u32 = 4;
    pub const SNDBUF: u32 = 7;
    pub const RCVBUF: u32 = 8;
    pub const PASSCRED: u32 = 16;
    pub const PEERCRED: u32 = 17;
    pub const RCVTIMEO_OLD: u32 = 20;
    pub const SNDTIMEO_OLD: u32 = 21;
    pub const ACCEPTCONN: u32 = 30;
    pub const PROTOCOL: u32 = 38;
    pub const DOMAIN: u32 = 39;
    pub const RCVTIMEO_NEW: u32 = 66;
    pub const SNDTIMEO_NEW: u32 = 67;
}

/// The size of a `struct sockaddr_un`: the family, then 108 bytes of path.
pub const SOCKADDR_UN_SIZE: usize = 110;

const FAMILY_SIZE: usize = 2;

const MSGHDR_SIZE: usize = 56;

const CMSGHDR_SIZE: usize = 16;

const MAX_CONTROL: u64 = 20480;

const TIMEVAL_SIZE: usize = 16;

const UCRED_SIZE: usize = 12;

const CHUNK: usize = 4096;

const TIOCOUTQ: u32 = 0x5411;

const NANOSECONDS_PER_MICROSECOND: u64 = 1000;

const MICROSECONDS_PER_SECOND: u64 = 1_000_000;

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
)))]
compile_error!("check Linux's socket constants and struct msghdr for this architecture");

fn wake<P: Platform>() {
    console::wake_input_waiters::<P>();
}

fn credentials_of<P: Platform>(process: &Process<P>) -> PeerCredentials {
    use credentials::Kind::{Group, User};
    PeerCredentials {
        pid: process.id().get(),
        uid: credentials::get_effective(process, User) as u32,
        gid: credentials::get_effective(process, Group) as u32,
    }
}

fn socket_of(thread: &LinuxThread, fd: u64) -> Result<(Arc<OpenFile>, Arc<UnixSocket>), Errno> {
    let file = thread.files().file(fd)?;
    let socket = match file.kind() {
        FileKind::Socket(socket) => Arc::clone(socket),
        _ => return Err(Errno::ENOTSOCK),
    };
    Ok((file, socket))
}

/// Every socket call, by `number`, with the program's registers: one out-of-line entry, so the
/// system-call path is as it was. Sets the call's result; the calls that wait deliver the
/// signals due after them.
#[inline(never)]
pub fn call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [first, second, third, fourth, fifth, sixth] = context.syscall_arguments();
    let result = match number {
        number::SOCKET => socket::<P>(process, thread, first, second, third),
        number::SOCKETPAIR => socketpair::<P>(process, thread, first, second, third, fourth),
        number::BIND => bind::<P>(process, thread, first, second, third),
        number::LISTEN => listen(process, thread, first, second),
        number::GETSOCKNAME => name(process, thread, first, second, third, false),
        number::GETPEERNAME => name(process, thread, first, second, third, true),
        number::SHUTDOWN => shutdown(thread, first, second),
        number::SETSOCKOPT => setsockopt(process, thread, first, second, third, fourth, fifth),
        number::GETSOCKOPT => getsockopt(process, thread, first, second, third, fourth, fifth),
        number::ACCEPT => return accept::<P>(process, thread, context, [first, second, third, 0]),
        number::ACCEPT4 => {
            return accept::<P>(process, thread, context, [first, second, third, fourth]);
        }
        number::CONNECT => return connect::<P>(process, thread, context, first, second, third),
        number::SENDTO => {
            return sendto::<P>(
                process,
                thread,
                context,
                [first, second, third, fourth, fifth, sixth],
            );
        }
        number::SENDMSG => return sendmsg::<P>(process, thread, context, first, second, third),
        number::RECVFROM => {
            return recvfrom::<P>(
                process,
                thread,
                context,
                [first, second, third, fourth, fifth, sixth],
            );
        }
        number::RECVMSG => return recvmsg::<P>(process, thread, context, first, second, third),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn finish<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    result: Result<u64, Errno>,
) -> ControlFlow<()> {
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::take_in_sent(process, thread);
    signals::deliver(process, thread, context)
}

fn interrupted(context: &UserContext, timeout: u64) -> Interrupted {
    match timeout {
        0 => Interrupted::RestartsIfAsked {
            number: context.syscall_number(),
            first_argument: context.syscall_arguments()[0],
        },
        _ => Interrupted::Fails,
    }
}

fn deadline<P: Platform>(timeout: u64) -> u64 {
    match timeout {
        0 => u64::MAX,
        _ => deadline_in::<P>(timeout).unwrap_or(u64::MAX),
    }
}

fn checked_type(domain: u64, kind: u64, protocol: u64) -> Result<u32, Errno> {
    let kind = kind as u32;
    let flags = kind & SOCKET_FLAGS;
    let base = kind & !SOCKET_FLAGS;
    if base & !SOCK_TYPE_MASK != 0 || base >= SOCK_MAX {
        return Err(Errno::EINVAL);
    }
    if domain as u32 != AF_UNIX {
        return Err(Errno::EAFNOSUPPORT);
    }
    if !matches!(protocol as u32, 0 | AF_UNIX) {
        return Err(Errno::EPROTONOSUPPORT);
    }
    if base != SOCK_STREAM {
        return Err(Errno::ESOCKTNOSUPPORT);
    }
    Ok(flags)
}

fn open_socket(socket: Arc<UnixSocket>, flags: u32) -> Arc<OpenFile> {
    Arc::new(OpenFile::new(
        FileKind::Socket(socket),
        open_flag::RDWR | flags & open_flag::NONBLOCK,
    ))
}

/// `socket(domain, type, protocol)`: a new open `AF_UNIX` stream socket's descriptor.
/// `EINVAL` for unknown type bits, `EAFNOSUPPORT` for another family, `EPROTONOSUPPORT` for a
/// protocol, `ESOCKTNOSUPPORT` for another type, `EMFILE` without a free descriptor.
pub fn socket<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    domain: u64,
    kind: u64,
    protocol: u64,
) -> Result<u64, Errno> {
    let flags = checked_type(domain, kind, protocol)?;
    let file = open_socket(UnixSocket::new(wake::<P>), flags);
    let limit = open_file_limit(process);
    let opened =
        thread
            .files()
            .lock()
            .open(Arc::clone(&file), flags & open_flag::CLOEXEC != 0, limit);
    drop(file);
    opened.map(u64::from)
}

/// `socketpair(domain, type, protocol, sv)`: two connected sockets, their descriptors written
/// to `sv` as two `int`s. Errors as [`socket`], then `EFAULT` for `sv` (nothing left open).
pub fn socketpair<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    domain: u64,
    kind: u64,
    protocol: u64,
    pair: u64,
) -> Result<u64, Errno> {
    let flags = checked_type(domain, kind, protocol)?;
    let (first, second) = UnixSocket::pair(credentials_of(process), wake::<P>);
    let close_on_exec = flags & open_flag::CLOEXEC != 0;
    let limit = open_file_limit(process);
    let files = thread.files();
    let (first, second) = (open_socket(first, flags), open_socket(second, flags));
    let first_fd = files.lock().open(Arc::clone(&first), close_on_exec, limit);
    let second_fd = match first_fd {
        Ok(_) => files.lock().open(Arc::clone(&second), close_on_exec, limit),
        Err(error) => Err(error),
    };
    let (first_fd, second_fd) = match (first_fd, second_fd) {
        (Ok(first_fd), Ok(second_fd)) => (first_fd, second_fd),
        (first_fd, Err(error)) => {
            let closed = first_fd.ok().and_then(|fd| files.lock().remove(fd));
            drop(closed);
            drop((first, second));
            return Err(error);
        }
        (Err(error), _) => return Err(error),
    };
    drop((first, second));
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&first_fd.to_ne_bytes());
    bytes[4..].copy_from_slice(&second_fd.to_ne_bytes());
    if process.write_user_memory(pair, &bytes).is_err() {
        let closed = {
            let mut table = files.lock();
            (table.remove(first_fd), table.remove(second_fd))
        };
        drop(closed);
        return Err(Errno::EFAULT);
    }
    Ok(0)
}

/// Parses a `struct sockaddr_un` of `length` bytes, as Linux does: `EINVAL` below the family
/// or above the structure, or for another family. Just the family is [`Address::Unnamed`]; a
/// path ends at its first NUL or at `length`, so a length that counts the NUL is the same
/// path; a first byte of NUL starts an abstract name, every byte after it to `length`.
pub fn parse_address(bytes: &[u8]) -> Result<Address, Errno> {
    if bytes.len() < FAMILY_SIZE || bytes.len() > SOCKADDR_UN_SIZE {
        return Err(Errno::EINVAL);
    }
    if u16::from_ne_bytes([bytes[0], bytes[1]]) as u32 != AF_UNIX {
        return Err(Errno::EINVAL);
    }
    let path = &bytes[FAMILY_SIZE..];
    Ok(match path.first() {
        None => Address::Unnamed,
        Some(0) => Address::Abstract(Arc::from(&path[1..])),
        Some(_) => {
            let end = path
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(path.len());
            Address::Path(Arc::from(&path[..end]))
        }
    })
}

/// `address` as a `struct sockaddr_un`, and its length as Linux reports it: the family for
/// none, then the path and its NUL, or a NUL and the abstract name.
pub fn encode_address(address: &Address) -> ([u8; SOCKADDR_UN_SIZE], usize) {
    let mut bytes = [0u8; SOCKADDR_UN_SIZE];
    bytes[..FAMILY_SIZE].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    let length = match address {
        Address::Unnamed => FAMILY_SIZE,
        Address::Path(path) => {
            let length = path.len().min(SOCKADDR_UN_SIZE - FAMILY_SIZE - 1);
            bytes[FAMILY_SIZE..FAMILY_SIZE + length].copy_from_slice(&path[..length]);
            FAMILY_SIZE + length + 1
        }
        Address::Abstract(name) => {
            let length = name.len().min(SOCKADDR_UN_SIZE - FAMILY_SIZE - 1);
            bytes[FAMILY_SIZE + 1..FAMILY_SIZE + 1 + length].copy_from_slice(&name[..length]);
            FAMILY_SIZE + 1 + length
        }
    };
    (bytes, length)
}

fn read_address<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
) -> Result<Address, Errno> {
    let length = length as u32 as usize;
    if !(FAMILY_SIZE..=SOCKADDR_UN_SIZE).contains(&length) {
        return Err(Errno::EINVAL);
    }
    let mut bytes = [0u8; SOCKADDR_UN_SIZE];
    process
        .read_user_memory(address, &mut bytes[..length])
        .map_err(|_| Errno::EFAULT)?;
    parse_address(&bytes[..length])
}

fn write_address<P: Platform>(
    process: &Process<P>,
    address: u64,
    length_at: u64,
    value: &Address,
) -> Result<(), Errno> {
    if address == 0 {
        return Ok(());
    }
    let mut length = [0u8; 4];
    process
        .read_user_memory(length_at, &mut length)
        .map_err(|_| Errno::EFAULT)?;
    let room = i32::from_ne_bytes(length);
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let (bytes, full) = encode_address(value);
    let written = full.min(room as usize);
    process
        .write_user_memory(address, &bytes[..written])
        .map_err(|_| Errno::EFAULT)?;
    process
        .write_user_memory(length_at, &(full as u32).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)
}

/// `bind(fd, address, length)`: binds the socket to a path, whose socket file it makes, to an
/// abstract name, or, with just the family, to a new abstract name of five hexadecimal
/// digits. As Linux: `ENOTSOCK`, the address's errors, `EINVAL` if it is bound already; for a
/// path, the walk's errors, `EADDRINUSE` if the name exists, `EROFS` on a file system that
/// cannot be changed and `EPERM` on one that cannot hold a socket file (EggFS, FAT);
/// `EADDRINUSE` for an abstract name a live socket has.
pub fn bind<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    address: u64,
    length: u64,
) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    let address = read_address(process, address, length)?;
    socket.check_unbound()?;
    let (address, key) = match address {
        Address::Unnamed => {
            let name = unix_socket::register_autobound(&socket)?;
            (
                Address::Abstract(Arc::clone(&name)),
                BindingKey::Abstract(name),
            )
        }
        Address::Abstract(name) => {
            let key = BindingKey::Abstract(Arc::clone(&name));
            unix_socket::register(key.clone(), &socket)?;
            (Address::Abstract(name), key)
        }
        Address::Path(path) => {
            let node = make_socket_file::<P>(thread, &path)?;
            let key = BindingKey::Node(node);
            unix_socket::register(key.clone(), &socket)?;
            (Address::Path(path), key)
        }
    };
    if let Err(error) = socket.set_bound(address) {
        unix_socket::unregister_key(&key, &socket);
        return Err(error);
    }
    Ok(0)
}

fn make_socket_file<P: Platform>(thread: &LinuxThread, path: &[u8]) -> Result<Node, Errno> {
    let (root, start) = starting_points(thread, super::stat::at::FDCWD as u64, path)?;
    let (directory, name) =
        parent_and_name(root, start, path, false).map_err(|error| match error {
            Errno::EBUSY => Errno::EADDRINUSE,
            error => error,
        })?;
    if directory.lookup(name).is_ok() {
        return Err(Errno::EADDRINUSE);
    }
    let file_system = directory.file_system();
    if !file_system.is_writable() {
        return Err(Errno::EROFS);
    }
    let id = file_system
        .create(
            directory.id(),
            name,
            NodeKind::Socket,
            0o777 & !UMASK,
            now::<P>(),
        )
        .map_err(|error| match error {
            Refused::Exists => Errno::EADDRINUSE,
            Refused::InvalidArgument => Errno::EPERM,
            error => refused(error),
        })?;
    vfs::resolve(root, directory, name)
        .map_err(walk_error)
        .and_then(|node| match node.id() == id {
            true => Ok(node),
            false => Err(Errno::ENOENT),
        })
}

/// `listen(fd, backlog)`: makes a bound socket listen, the backlog read as Linux reads it, an
/// unsigned number at most 4096 (so -1 is 4096). `EINVAL` unless it is bound and not
/// connected.
pub fn listen<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    backlog: u64,
) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    socket.listen(backlog_limit(backlog), credentials_of(process))?;
    Ok(0)
}

/// `listen`'s backlog: an `int` read as unsigned, at most [`unix_socket::MAX_BACKLOG`].
pub fn backlog_limit(backlog: u64) -> u32 {
    (backlog as u32).min(unix_socket::MAX_BACKLOG)
}

fn name<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    address: u64,
    length_at: u64,
    peer: bool,
) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    let value = match peer {
        true => socket.peer_address().ok_or(Errno::ENOTCONN)?,
        false => socket.local_address(),
    };
    if address == 0 {
        return Err(Errno::EFAULT);
    }
    write_address(process, address, length_at, &value)?;
    Ok(0)
}

fn shutdown(thread: &LinuxThread, fd: u64, how: u64) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    let (read, write) = match how as u32 {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(Errno::EINVAL),
    };
    let (connection, side) = socket.connected().map_err(|_| Errno::ENOTCONN)?;
    connection.shutdown(side, read, write);
    drop(connection);
    socket.changed();
    Ok(0)
}

fn read_int<P: Platform>(process: &Process<P>, address: u64) -> Result<i32, Errno> {
    let mut bytes = [0u8; 4];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(i32::from_ne_bytes(bytes))
}

/// A `struct timeval` as nanoseconds (0 for none): `EDOM` for microseconds out of range, 0
/// for a negative time, as Linux takes them.
pub fn timeout_nanoseconds(bytes: [u8; TIMEVAL_SIZE]) -> Result<u64, Errno> {
    let seconds = i64::from_ne_bytes(bytes[..8].try_into().unwrap());
    let microseconds = i64::from_ne_bytes(bytes[8..].try_into().unwrap());
    if !(0..MICROSECONDS_PER_SECOND as i64).contains(&microseconds) {
        return Err(Errno::EDOM);
    }
    if seconds < 0 {
        return Ok(0);
    }
    Ok((seconds as u64)
        .saturating_mul(MICROSECONDS_PER_SECOND)
        .saturating_add(microseconds as u64)
        .saturating_mul(NANOSECONDS_PER_MICROSECOND))
}

fn timeval(nanoseconds: u64) -> [u8; TIMEVAL_SIZE] {
    let microseconds = nanoseconds / NANOSECONDS_PER_MICROSECOND;
    let mut bytes = [0u8; TIMEVAL_SIZE];
    bytes[..8].copy_from_slice(&(microseconds / MICROSECONDS_PER_SECOND).to_ne_bytes());
    bytes[8..].copy_from_slice(&(microseconds % MICROSECONDS_PER_SECOND).to_ne_bytes());
    bytes
}

/// `setsockopt(fd, level, name, value, length)`: `SO_SNDBUF` and `SO_RCVBUF` kept and
/// reported doubled, `SO_PASSCRED` and `SO_REUSEADDR` kept, `SO_RCVTIMEO` and `SO_SNDTIMEO`
/// acted on. As Linux: `EOPNOTSUPP` for another level, `EINVAL` for a value too short,
/// `EFAULT` for one that is not the program's, `ENOPROTOOPT` for another option.
pub fn setsockopt<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    level: u64,
    name: u64,
    value: u64,
    length: u64,
) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    if level as u32 != SOL_SOCKET {
        return Err(Errno::EOPNOTSUPP);
    }
    let length = length as u32 as usize;
    let options = socket.options();
    let name = name as u32;
    match name {
        option::RCVTIMEO_OLD
        | option::RCVTIMEO_NEW
        | option::SNDTIMEO_OLD
        | option::SNDTIMEO_NEW => {
            if length < TIMEVAL_SIZE {
                return Err(Errno::EINVAL);
            }
            let mut bytes = [0u8; TIMEVAL_SIZE];
            process
                .read_user_memory(value, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let timeout = timeout_nanoseconds(bytes)?;
            let kept = match name {
                option::RCVTIMEO_OLD | option::RCVTIMEO_NEW => &options.receive_timeout,
                _ => &options.send_timeout,
            };
            kept.store(timeout, Ordering::Relaxed);
            return Ok(0);
        }
        option::SNDBUF | option::RCVBUF | option::PASSCRED | option::REUSEADDR => {}
        _ => return Err(Errno::ENOPROTOOPT),
    }
    if length < 4 {
        return Err(Errno::EINVAL);
    }
    let number = read_int(process, value)?;
    match name {
        option::SNDBUF | option::RCVBUF => {
            let doubled = (number.max(0) as u32).saturating_mul(2).max(4608);
            let kept = match name {
                option::SNDBUF => &options.send_buffer,
                _ => &options.receive_buffer,
            };
            kept.store(doubled, Ordering::Relaxed);
        }
        option::PASSCRED => options
            .pass_credentials
            .store(number != 0, Ordering::Relaxed),
        _ => options.reuse_address.store(number != 0, Ordering::Relaxed),
    }
    Ok(0)
}

/// `getsockopt(fd, level, name, value, length)`: writes at most the `int` at `length` bytes
/// of the option and how many it wrote back there. `SO_ERROR` reports a pending reset once.
/// As Linux: `EOPNOTSUPP` for another level, `ENOPROTOOPT` for another option, `EINVAL` for a
/// negative length, `EFAULT` for memory that is not the program's.
pub fn getsockopt<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    fd: u64,
    level: u64,
    name: u64,
    value: u64,
    length_at: u64,
) -> Result<u64, Errno> {
    let (_file, socket) = socket_of(thread, fd)?;
    if level as u32 != SOL_SOCKET {
        return Err(Errno::EOPNOTSUPP);
    }
    let room = read_int(process, length_at)?;
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let options = socket.options();
    let int = |value: u32| {
        let mut bytes = [0u8; TIMEVAL_SIZE];
        bytes[..4].copy_from_slice(&value.to_ne_bytes());
        (bytes, 4)
    };
    let (bytes, size) = match name as u32 {
        option::TYPE => int(SOCK_STREAM),
        option::DOMAIN => int(AF_UNIX),
        option::PROTOCOL => int(0),
        option::ERROR => {
            let reset = match socket.connected() {
                Ok((connection, side)) => connection.take_reset(side),
                Err(_) => false,
            };
            int(match reset {
                true => u32::from(Errno::ECONNRESET.0),
                false => 0,
            })
        }
        option::ACCEPTCONN => int(u32::from(socket.is_listening())),
        option::SNDBUF => int(options.send_buffer.load(Ordering::Relaxed)),
        option::RCVBUF => int(options.receive_buffer.load(Ordering::Relaxed)),
        option::PASSCRED => int(u32::from(options.pass_credentials.load(Ordering::Relaxed))),
        option::REUSEADDR => int(u32::from(options.reuse_address.load(Ordering::Relaxed))),
        option::PEERCRED => {
            let credentials = socket.peer_credentials();
            let mut bytes = [0u8; TIMEVAL_SIZE];
            bytes[..4].copy_from_slice(&credentials.pid.to_ne_bytes());
            bytes[4..8].copy_from_slice(&credentials.uid.to_ne_bytes());
            bytes[8..12].copy_from_slice(&credentials.gid.to_ne_bytes());
            (bytes, UCRED_SIZE)
        }
        option::RCVTIMEO_OLD | option::RCVTIMEO_NEW => (
            timeval(options.receive_timeout.load(Ordering::Relaxed)),
            TIMEVAL_SIZE,
        ),
        option::SNDTIMEO_OLD | option::SNDTIMEO_NEW => (
            timeval(options.send_timeout.load(Ordering::Relaxed)),
            TIMEVAL_SIZE,
        ),
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

/// `ioctl` on a socket: `FIONREAD` (`SIOCINQ`) the bytes queued to read, `TIOCOUTQ`
/// (`SIOCOUTQ`) the bytes sent and not yet read, as `int`s; `None` for another request.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    socket: &UnixSocket,
    request: u64,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    let request = request as u32;
    if request != super::terminal::request::FIONREAD && request != TIOCOUTQ {
        return None;
    }
    let count = match socket.connected() {
        Ok((connection, side)) if request == TIOCOUTQ => connection.unread_sent(side),
        Ok((connection, side)) => connection.available(side),
        Err(NotConnected::Listening) => return Some(Err(Errno::EINVAL)),
        Err(NotConnected::Open) => 0,
    };
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    Some(
        process
            .write_user_memory(argument, &count.to_ne_bytes())
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT),
    )
}

fn accept<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    [fd, address, length_at, flags]: [u64; 4],
) -> ControlFlow<()> {
    let prepared = match flags as u32 & !SOCKET_FLAGS {
        0 => socket_of(thread, fd),
        _ => Err(Errno::EINVAL),
    };
    let (file, socket) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let flags = flags as u32;
    let timeout = socket.options().receive_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    let result = loop {
        match socket.accept() {
            Err(error) => break Err(error),
            Ok(Some(connection)) => {
                socket.changed();
                break install_accepted::<P>(
                    process, thread, &socket, connection, address, length_at, flags,
                );
            }
            Ok(None) if file.is_nonblocking() => break Err(Errno::EAGAIN),
            Ok(None) => match wait_for_input(process, thread, deadline, || socket.can_accept())? {
                Waited::Ready => {}
                Waited::TimedOut => break Err(Errno::EAGAIN),
                Waited::Signalled => {
                    if signals::deliver_interrupting(process, thread, context, interrupted)? {
                        return ControlFlow::Continue(());
                    }
                }
            },
        }
    };
    finish(process, thread, context, result)
}

fn install_accepted<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    listener: &UnixSocket,
    connection: Arc<Connection>,
    address: u64,
    length_at: u64,
    flags: u32,
) -> Result<u64, Errno> {
    let peer = connection.address(Side::First);
    let accepted = UnixSocket::accepted(connection, wake::<P>);
    let file = open_socket(Arc::clone(&accepted), flags);
    let limit = open_file_limit(process);
    let opened =
        thread
            .files()
            .lock()
            .open(Arc::clone(&file), flags & open_flag::CLOEXEC != 0, limit);
    drop(file);
    let fd = match opened {
        Ok(fd) => fd,
        Err(error) => {
            if let Some(connection) = accepted.detach()
                && !listener.put_back(Arc::clone(&connection))
            {
                unix_socket::release_rights(connection.abort(Side::Second));
            }
            return Err(error);
        }
    };
    drop(accepted);
    if let Err(error) = write_address(process, address, length_at, &peer) {
        let closed = thread.files().lock().remove(fd);
        drop(closed);
        return Err(error);
    }
    Ok(u64::from(fd))
}

fn find_target(thread: &LinuxThread, address: &Address) -> Result<Arc<UnixSocket>, Errno> {
    let key = match address {
        Address::Unnamed => return Err(Errno::EINVAL),
        Address::Abstract(name) => BindingKey::Abstract(Arc::clone(name)),
        Address::Path(path) => {
            let (root, start) = starting_points(thread, super::stat::at::FDCWD as u64, path)?;
            let node = vfs::resolve(root, start, path).map_err(walk_error)?;
            if node.kind() != NodeKind::Socket {
                return Err(Errno::ECONNREFUSED);
            }
            BindingKey::Node(node)
        }
    };
    unix_socket::find(&key).ok_or(Errno::ECONNREFUSED)
}

fn connect<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    fd: u64,
    address: u64,
    length: u64,
) -> ControlFlow<()> {
    let prepared = socket_of(thread, fd).and_then(|(file, socket)| {
        let target = read_address(process, address, length)?;
        let target = find_target(thread, &target)?;
        let own = socket.begin_connect()?;
        Ok((file, socket, own, target))
    });
    let (file, socket, own, target) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let connection = Connection::new(own, credentials_of(&**process));
    let outcome = queue_connection(
        process,
        thread,
        context,
        &file,
        &socket,
        &target,
        &connection,
    );
    match outcome {
        ControlFlow::Continue(Some(Ok(result))) => finish(process, thread, context, Ok(result)),
        ControlFlow::Continue(Some(Err(error))) => {
            socket.abandon_connect();
            finish(process, thread, context, Err(error))
        }
        ControlFlow::Continue(None) => {
            socket.abandon_connect();
            ControlFlow::Continue(())
        }
        ControlFlow::Break(()) => {
            socket.abandon_connect();
            ControlFlow::Break(())
        }
    }
}

fn queue_connection<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &OpenFile,
    socket: &UnixSocket,
    target: &UnixSocket,
    connection: &Arc<Connection>,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let timeout = socket.options().send_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    loop {
        match target.queue(connection) {
            Err(error) => return ControlFlow::Continue(Some(Err(error))),
            Ok(true) => {
                target.changed();
                let result = match socket.finish_connect(connection) {
                    Ok(()) => Ok(0),
                    Err(error) => {
                        unix_socket::release_rights(connection.abort(Side::First));
                        target.changed();
                        Err(error)
                    }
                };
                return ControlFlow::Continue(Some(result));
            }
            Ok(false) if file.is_nonblocking() => {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
            Ok(false) => match wait_for_input(process, thread, deadline, || target.can_queue())? {
                Waited::Ready => {}
                Waited::TimedOut => return ControlFlow::Continue(Some(Err(Errno::EAGAIN))),
                Waited::Signalled => {
                    if signals::deliver_interrupting(process, thread, context, interrupted)? {
                        return ControlFlow::Continue(None);
                    }
                }
            },
        }
    }
}

struct Send {
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
    buffers: Iovecs,
    rights: Option<OutgoingRights>,
    flags: u32,
}

fn sendto<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    [fd, buffer, length, flags, address, address_length]: [u64; 6],
) -> ControlFlow<()> {
    let prepared = socket_of(thread, fd).and_then(|(file, socket)| {
        if !ends_in_user_space::<P>(buffer, length) {
            return Err(Errno::EFAULT);
        }
        check_destination(&socket, address != 0 && address_length as u32 != 0)?;
        Ok(Send {
            file,
            socket,
            buffers: Iovecs::one(buffer, length),
            rights: None,
            flags: flags as u32,
        })
    });
    match prepared {
        Ok(send) => send_message(process, thread, context, send),
        Err(error) => finish(process, thread, context, Err(error)),
    }
}

fn check_destination(socket: &UnixSocket, named: bool) -> Result<(), Errno> {
    if !named {
        return Ok(());
    }
    match socket.connected() {
        Ok(_) => Err(Errno::EISCONN),
        Err(_) => Err(Errno::EOPNOTSUPP),
    }
}

#[derive(Debug, Clone, Copy)]
struct MessageHeader {
    name: u64,
    name_length: u32,
    buffers: u64,
    buffer_count: u64,
    control: u64,
    control_length: u64,
}

fn word(bytes: &[u8; MSGHDR_SIZE], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn read_header<P: Platform>(process: &Process<P>, address: u64) -> Result<MessageHeader, Errno> {
    let mut bytes = [0u8; MSGHDR_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(MessageHeader {
        name: word(&bytes, 0),
        name_length: word(&bytes, 8) as u32,
        buffers: word(&bytes, 16),
        buffer_count: word(&bytes, 24),
        control: word(&bytes, 32),
        control_length: word(&bytes, 40),
    })
}

fn sendmsg<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    fd: u64,
    header: u64,
    flags: u64,
) -> ControlFlow<()> {
    let prepared = socket_of(thread, fd).and_then(|(file, socket)| {
        let header = read_header(process, header)?;
        if header.buffer_count > MAX_IOVECS {
            return Err(Errno::EMSGSIZE);
        }
        let buffers = Iovecs::read(process, header.buffers, header.buffer_count)?;
        check_destination(&socket, header.name_length != 0)?;
        let rights = read_rights(process, thread, header.control, header.control_length)?;
        Ok(Send {
            file,
            socket,
            buffers,
            rights,
            flags: flags as u32,
        })
    });
    match prepared {
        Ok(send) => send_message(process, thread, context, send),
        Err(error) => finish(process, thread, context, Err(error)),
    }
}

fn read_rights<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    control: u64,
    length: u64,
) -> Result<Option<OutgoingRights>, Errno> {
    if length < CMSGHDR_SIZE as u64 {
        return Ok(None);
    }
    if length > MAX_CONTROL {
        return Err(Errno::ENOBUFS);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(|_| Errno::ENOBUFS)?;
    bytes.resize(length as usize, 0);
    process
        .read_user_memory(control, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let descriptors = parse_rights(&bytes)?;
    if descriptors.is_empty() {
        return Ok(None);
    }
    let mut files = Vec::new();
    files
        .try_reserve_exact(descriptors.len())
        .map_err(|_| Errno::ENOMEM)?;
    for fd in descriptors {
        files.push(thread.files().file(u64::from(fd as u32))?);
    }
    Ok(Some(OutgoingRights::new(files)))
}

/// The descriptor numbers in the `SCM_RIGHTS` messages of a control buffer, as Linux reads
/// them: `EINVAL` for a message whose length is short or overruns the buffer, for another
/// socket-level message (`SCM_CREDENTIALS`), or for more than
/// [`unix_socket::MAX_RIGHTS_PER_MESSAGE`] descriptors in all. Messages of other levels are
/// skipped.
pub fn parse_rights(bytes: &[u8]) -> Result<Vec<i32>, Errno> {
    let mut descriptors = Vec::new();
    let mut offset = 0usize;
    while bytes.len().saturating_sub(offset) >= CMSGHDR_SIZE {
        let header = &bytes[offset..offset + CMSGHDR_SIZE];
        let length = u64::from_ne_bytes(header[..8].try_into().unwrap());
        let level = u32::from_ne_bytes(header[8..12].try_into().unwrap());
        let kind = u32::from_ne_bytes(header[12..16].try_into().unwrap());
        if length < CMSGHDR_SIZE as u64 || length > (bytes.len() - offset) as u64 {
            return Err(Errno::EINVAL);
        }
        let length = length as usize;
        if level == SOL_SOCKET {
            if kind != SCM_RIGHTS {
                return Err(Errno::EINVAL);
            }
            let data = &bytes[offset + CMSGHDR_SIZE..offset + length];
            let count = data.len() / 4;
            if descriptors.len() + count > unix_socket::MAX_RIGHTS_PER_MESSAGE {
                return Err(Errno::EINVAL);
            }
            descriptors.try_reserve(count).map_err(|_| Errno::ENOMEM)?;
            descriptors.extend(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|fd| i32::from_ne_bytes(*fd)),
            );
        }
        offset += length.next_multiple_of(8);
    }
    Ok(descriptors)
}

/// `write` or `writev` on a socket: a send with no flags.
pub(super) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<UnixSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let send = Send {
        file: Arc::clone(file),
        socket: Arc::clone(socket),
        buffers,
        rights: None,
        flags: 0,
    };
    send_message(process, thread, context, send)
}

fn send_message<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    mut send: Send,
) -> ControlFlow<()> {
    let timeout = send.socket.options().send_timeout.load(Ordering::Relaxed);
    let interrupted = interrupted(context, timeout);
    let deadline = deadline::<P>(timeout);
    let result = loop {
        match send_or_wait(process, thread, &mut send, deadline)? {
            Some(result) => break result,
            None => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    unix_socket::release_rights(
        send.rights
            .take()
            .map(|rights| rights.files)
            .unwrap_or_default(),
    );
    if result == Err(Errno::EPIPE) && send.flags & message::NOSIGNAL == 0 {
        signals::send_from_kernel_to_self(process, thread, super::signal::SIGPIPE);
    }
    finish(process, thread, context, result)
}

fn send_or_wait<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    send: &mut Send,
    deadline: u64,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    if send.flags & message::OOB != 0 {
        return ControlFlow::Continue(Some(Err(Errno::EOPNOTSUPP)));
    }
    let (connection, side) = match send.socket.connected() {
        Ok(connected) => connected,
        Err(_) => return ControlFlow::Continue(Some(Err(Errno::ENOTCONN))),
    };
    if connection.write_shut(side) {
        return ControlFlow::Continue(Some(Err(Errno::EPIPE)));
    }
    let total = send.buffers.total().min(MAX_RW_COUNT);
    if total == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    let nonblocking = send.file.is_nonblocking() || send.flags & message::DONTWAIT != 0;
    let mut chunk = [0u8; CHUNK];
    let mut written = 0;
    let partial = |written: u64, error: Errno| match written {
        0 => Err(error),
        _ => Ok(written),
    };
    for entry in send.buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(total - written);
        let mut done = 0;
        while done < length {
            let size = (length - done).min(CHUNK as u64) as usize;
            if process
                .read_user_memory(base + done, &mut chunk[..size])
                .is_err()
            {
                return ControlFlow::Continue(Some(partial(written, Errno::EFAULT)));
            }
            let mut sent = 0;
            while sent < size {
                let pushed =
                    match connection.send_passing(side, &chunk[sent..size], &mut send.rights) {
                        Ok(pushed) => pushed,
                        Err(error) => return ControlFlow::Continue(Some(partial(written, error))),
                    };
                if pushed > 0 {
                    send.socket.changed();
                    sent += pushed;
                    written += pushed as u64;
                    continue;
                }
                if nonblocking {
                    return ControlFlow::Continue(Some(partial(written, Errno::EAGAIN)));
                }
                match wait_for_input(process, thread, deadline, || connection.can_send(side))? {
                    Waited::Ready => {}
                    Waited::TimedOut => {
                        return ControlFlow::Continue(Some(partial(written, Errno::EAGAIN)));
                    }
                    Waited::Signalled => {
                        return ControlFlow::Continue(match written {
                            0 => None,
                            _ => Some(Ok(written)),
                        });
                    }
                }
            }
            done += size as u64;
        }
    }
    ControlFlow::Continue(Some(Ok(written)))
}

struct Receive {
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
    buffers: Iovecs,
    flags: u32,
    rights: Option<Vec<Arc<OpenFile>>>,
}

fn recvfrom<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    [fd, buffer, length, flags, address, length_at]: [u64; 6],
) -> ControlFlow<()> {
    let prepared = socket_of(thread, fd).and_then(|(file, socket)| {
        if !ends_in_user_space::<P>(buffer, length) {
            return Err(Errno::EFAULT);
        }
        Ok(Receive {
            file,
            socket,
            buffers: Iovecs::one(buffer, length),
            flags: flags as u32,
            rights: None,
        })
    });
    let mut receive = match prepared {
        Ok(receive) => receive,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let result = match receive_message(process, thread, context, &mut receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    unix_socket::release_rights(receive.rights.take().unwrap_or_default());
    let result = result.and_then(|count| {
        if let Some(peer) = receive.socket.peer_address() {
            write_address(process, address, length_at, &peer)?;
        }
        Ok(count)
    });
    finish(process, thread, context, result)
}

/// `read` or `readv` on a socket: a receive with no flags, whose descriptors are closed.
pub(super) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<UnixSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let mut receive = Receive {
        file: Arc::clone(file),
        socket: Arc::clone(socket),
        buffers,
        flags: 0,
        rights: None,
    };
    let result = match receive_message(process, thread, context, &mut receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    unix_socket::release_rights(receive.rights.take().unwrap_or_default());
    finish(process, thread, context, result)
}

fn recvmsg<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    fd: u64,
    header_at: u64,
    flags: u64,
) -> ControlFlow<()> {
    let prepared = socket_of(thread, fd).and_then(|(file, socket)| {
        let header = read_header(process, header_at)?;
        if header.buffer_count > MAX_IOVECS {
            return Err(Errno::EMSGSIZE);
        }
        let buffers = Iovecs::read(process, header.buffers, header.buffer_count)?;
        Ok((
            header,
            Receive {
                file,
                socket,
                buffers,
                flags: flags as u32,
                rights: None,
            },
        ))
    });
    let (header, mut receive) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let result = match receive_message(process, thread, context, &mut receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    let result = result.and_then(|count| {
        let rights = receive.rights.take().unwrap_or_default();
        let close_on_exec = receive.flags & message::CMSG_CLOEXEC != 0;
        complete_header(
            process,
            thread,
            &receive.socket,
            header_at,
            &header,
            rights,
            close_on_exec,
        )?;
        Ok(count)
    });
    unix_socket::release_rights(receive.rights.take().unwrap_or_default());
    finish(process, thread, context, result)
}

/// What [`install_rights`] did with received descriptors.
#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    /// The descriptors opened, in order.
    pub descriptors: Vec<u32>,
    /// How many bytes of the control buffer the message takes (`msg_controllen`).
    pub used: u64,
    /// Whether some did not fit (`MSG_CTRUNC`).
    pub truncated: bool,
}

/// The `SCM_RIGHTS` message for `descriptors`.
pub fn rights_message(descriptors: &[u32]) -> Vec<u8> {
    let length = CMSGHDR_SIZE + 4 * descriptors.len();
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(&(length as u64).to_ne_bytes());
    bytes.extend_from_slice(&SOL_SOCKET.to_ne_bytes());
    bytes.extend_from_slice(&SCM_RIGHTS.to_ne_bytes());
    for descriptor in descriptors {
        bytes.extend_from_slice(&descriptor.to_ne_bytes());
    }
    bytes
}

/// Opens descriptors for as many of `rights` as a control buffer of `room` bytes holds and the
/// table allows, as Linux does, and closes the rest.
pub fn install_rights<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    rights: Vec<Arc<OpenFile>>,
    room: u64,
    close_on_exec: bool,
) -> Installed {
    let fits = match room.checked_sub(CMSGHDR_SIZE as u64) {
        Some(left) => (left / 4).min(rights.len() as u64) as usize,
        None => 0,
    };
    let mut truncated = fits < rights.len();
    let limit = open_file_limit(process);
    let mut descriptors = Vec::new();
    let mut left = Vec::new();
    let mut stopped = false;
    for (index, file) in rights.into_iter().enumerate() {
        if stopped || index >= fits {
            left.push(file);
            continue;
        }
        let opened = thread
            .files()
            .lock()
            .open(Arc::clone(&file), close_on_exec, limit);
        match opened {
            Ok(fd) => descriptors.push(fd),
            Err(_) => {
                stopped = true;
                truncated = true;
                left.push(file);
            }
        }
    }
    unix_socket::release_rights(left);
    let used = match descriptors.is_empty() {
        true => 0,
        false => ((CMSGHDR_SIZE + 4 * descriptors.len()).next_multiple_of(8) as u64).min(room),
    };
    Installed {
        descriptors,
        used,
        truncated,
    }
}

fn complete_header<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    socket: &UnixSocket,
    header_at: u64,
    header: &MessageHeader,
    rights: Vec<Arc<OpenFile>>,
    close_on_exec: bool,
) -> Result<(), Errno> {
    let had_rights = !rights.is_empty();
    let installed = match had_rights {
        true => install_rights(
            process,
            thread,
            rights,
            header.control_length,
            close_on_exec,
        ),
        false => Installed {
            descriptors: Vec::new(),
            used: 0,
            truncated: false,
        },
    };
    let undo = |installed: &Installed| {
        let mut closed = Vec::new();
        {
            let mut table = thread.files().lock();
            for &fd in &installed.descriptors {
                closed.push(table.remove(fd));
            }
        }
        drop(closed);
    };
    if !installed.descriptors.is_empty() {
        let message = rights_message(&installed.descriptors);
        if process.write_user_memory(header.control, &message).is_err() {
            undo(&installed);
            return Err(Errno::EFAULT);
        }
    }
    let mut written = [0u8; MSGHDR_SIZE];
    if process.read_user_memory(header_at, &mut written).is_err() {
        undo(&installed);
        return Err(Errno::EFAULT);
    }
    let mut name_length = header.name_length;
    if header.name != 0 {
        let peer = socket.peer_address().unwrap_or_default();
        let (bytes, full) = encode_address(&peer);
        let copied = full.min(header.name_length as usize);
        if process
            .write_user_memory(header.name, &bytes[..copied])
            .is_err()
        {
            undo(&installed);
            return Err(Errno::EFAULT);
        }
        name_length = full as u32;
    }
    let flags = match installed.truncated {
        true => message::CTRUNC,
        false => 0,
    };
    written[8..12].copy_from_slice(&name_length.to_ne_bytes());
    written[40..48].copy_from_slice(&installed.used.to_ne_bytes());
    written[48..52].copy_from_slice(&flags.to_ne_bytes());
    if process.write_user_memory(header_at, &written).is_err() {
        undo(&installed);
        return Err(Errno::EFAULT);
    }
    Ok(())
}

fn receive_message<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    receive: &mut Receive,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let timeout = receive
        .socket
        .options()
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
    receive: &mut Receive,
    deadline: u64,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    if receive.flags & message::OOB != 0 {
        return ControlFlow::Continue(Some(Err(Errno::EOPNOTSUPP)));
    }
    let (connection, side) = match receive.socket.connected() {
        Ok(connected) => connected,
        Err(NotConnected::Listening) => return ControlFlow::Continue(Some(Err(Errno::EINVAL))),
        Err(NotConnected::Open) => return ControlFlow::Continue(Some(Err(Errno::ENOTCONN))),
    };
    let count = receive.buffers.total().min(MAX_RW_COUNT);
    if count == 0 {
        return ControlFlow::Continue(Some(Ok(0)));
    }
    let peek = receive.flags & message::PEEK != 0;
    let wait_all = receive.flags & message::WAITALL != 0 && !peek;
    let nonblocking = receive.file.is_nonblocking() || receive.flags & message::DONTWAIT != 0;
    let mut chunk = [0u8; CHUNK];
    let mut copied = 0u64;
    for entry in receive.buffers.entries() {
        let (base, length) = iovec(entry);
        let length = length.min(count - copied);
        let mut done = 0;
        while done < length {
            let wanted = (length - done).min(CHUNK as u64) as usize;
            let first = copied == 0;
            let taken = match peek {
                true => connection.peek(side, copied as usize, &mut chunk[..wanted], first),
                false => connection.receive(side, &mut chunk[..wanted], first),
            };
            let taken = match taken {
                Taken::Bytes { count, rights } => {
                    if let Some(rights) = rights {
                        receive.rights = Some(rights);
                    }
                    if !peek {
                        receive.socket.changed();
                    }
                    count
                }
                Taken::Boundary | Taken::Ended => return ControlFlow::Continue(Some(Ok(copied))),
                Taken::Reset => return ControlFlow::Continue(Some(Err(Errno::ECONNRESET))),
                Taken::Empty if copied > 0 && !wait_all => {
                    return ControlFlow::Continue(Some(Ok(copied)));
                }
                Taken::Empty => {
                    if nonblocking {
                        return ControlFlow::Continue(Some(match copied {
                            0 => Err(Errno::EAGAIN),
                            _ => Ok(copied),
                        }));
                    }
                    match wait_for_input(process, thread, deadline, || {
                        connection.can_receive(side)
                    })? {
                        Waited::Ready => continue,
                        Waited::TimedOut => {
                            return ControlFlow::Continue(Some(match copied {
                                0 => Err(Errno::EAGAIN),
                                _ => Ok(copied),
                            }));
                        }
                        Waited::Signalled => {
                            return ControlFlow::Continue(match copied {
                                0 => None,
                                _ => Some(Ok(copied)),
                            });
                        }
                    }
                }
            };
            if process
                .write_user_memory(base + done, &chunk[..taken])
                .is_err()
            {
                return ControlFlow::Continue(Some(match copied {
                    0 => Err(Errno::EFAULT),
                    _ => Ok(copied),
                }));
            }
            done += taken as u64;
            copied += taken as u64;
        }
        if copied == count {
            break;
        }
    }
    ControlFlow::Continue(Some(Ok(copied)))
}

#[cfg(test)]
mod tests;

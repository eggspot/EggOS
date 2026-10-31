//! `AF_UNIX` datagram sockets (`SOCK_DGRAM`, D188): `socket`, `socketpair`, `bind` (a path or
//! an abstract name, through the stream sockets' registry), `connect` (`AF_UNSPEC`
//! disconnects), `sendto`, `sendmsg`, `recvfrom`, `recvmsg`, `getsockname`, `getpeername`,
//! `shutdown`, `setsockopt`, `getsockopt`, `ioctl(FIONREAD)` and `poll`, and `read`, `write`
//! and their vector forms, as Linux answers them for datagram sockets.
//!
//! A datagram is sent whole, at once, to one socket, which queues it with its sender's address
//! until it is received whole; a receive that gives fewer bytes drops the rest. A socket
//! connected to another takes datagrams from that one only (`EPERM` for others). A sender
//! waits while the receiver holds [`MAX_QUEUED`] datagrams or its receive buffer's worth of
//! bytes, unless it is non-blocking (`EAGAIN`).
//!
//! `/dev/socket/logdw`, Android's log socket, is the kernel's own: what is sent there goes to
//! [`crate::linux::android_log`] as from the sending process.

use super::inet::finish_send;
use super::{
    MSGHDR_SIZE, deadline, encode_address, finish, interrupted, message, read_address, read_header,
    write_address,
};
use crate::linux::android_log;
use crate::linux::descriptors::{FileKind, OpenFile};
use crate::linux::files::{starting_points, walk_error};
use crate::linux::poll::{self, event};
use crate::linux::signals;
use crate::linux::terminal::Waited;
use crate::linux::thread::LinuxThread;
use crate::linux::unix_socket::{self, Address, BindingKey, UnixSocket};
use crate::linux::{Errno, Iovecs, MAX_IOVECS, ends_in_user_space, iovec, number};
use crate::process::Process;
use crate::sync::LeafSpinLock;
use crate::vfs::{self, NodeKind, devfs};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::Ordering;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// Datagram sockets (`SOCK_DGRAM`).
pub const SOCK_DGRAM: u32 = 2;

/// The most datagrams queued towards one socket.
pub const MAX_QUEUED: usize = 256;

const MSG_TRUNC: u32 = 0x20;

#[derive(Debug)]
struct Datagram {
    sender: Address,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
enum Target {
    Socket(Weak<UnixSocket>),
    Log,
}

#[derive(Debug, Clone)]
struct Peer {
    target: Target,
    address: Address,
}

#[derive(Debug)]
enum Destination {
    Socket(Arc<UnixSocket>),
    Log,
}

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<Datagram>,
    queued_bytes: usize,
    peer: Option<Peer>,
    read_shut: bool,
    write_shut: bool,
}

enum Refused {
    Full(Datagram),
    Failed(Errno),
}

/// A datagram socket's own side: the datagrams queued towards it, the socket it is connected
/// to, and what was shut down. Its lock is a leaf lock, never held with another socket's.
pub struct Datagrams {
    state: LeafSpinLock<State>,
}

impl core::fmt::Debug for Datagrams {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Datagrams").finish_non_exhaustive()
    }
}

impl Default for Datagrams {
    fn default() -> Self {
        Self::new()
    }
}

impl Datagrams {
    /// An unconnected side with nothing queued.
    pub fn new() -> Self {
        Self {
            state: LeafSpinLock::new_leaf(State::default()),
        }
    }

    /// The length of the next datagram, 0 without one (`FIONREAD`).
    pub fn next_length(&self) -> usize {
        self.state
            .lock()
            .queue
            .front()
            .map_or(0, |datagram| datagram.bytes.len())
    }

    fn has_datagrams(&self) -> bool {
        !self.state.lock().queue.is_empty()
    }

    fn is_read_shut(&self) -> bool {
        self.state.lock().read_shut
    }

    fn is_write_shut(&self) -> bool {
        self.state.lock().write_shut
    }

    fn has_room(&self, size: usize, limit: usize) -> bool {
        let state = self.state.lock();
        Self::room_in(&state, size, limit)
    }

    fn room_in(state: &State, size: usize, limit: usize) -> bool {
        state.queue.is_empty()
            || state.queue.len() < MAX_QUEUED && state.queued_bytes + size <= limit
    }

    fn connect(&self, peer: Option<Peer>) {
        let previous = core::mem::replace(&mut self.state.lock().peer, peer);
        drop(previous);
    }

    fn peer_address(&self) -> Option<Address> {
        self.state
            .lock()
            .peer
            .as_ref()
            .map(|peer| peer.address.clone())
    }

    fn destination(&self) -> Result<Destination, Errno> {
        let target = self
            .state
            .lock()
            .peer
            .as_ref()
            .map(|peer| peer.target.clone());
        match target {
            None => Err(Errno::ENOTCONN),
            Some(Target::Log) => Ok(Destination::Log),
            Some(Target::Socket(socket)) => socket
                .upgrade()
                .map(Destination::Socket)
                .ok_or(Errno::ECONNREFUSED),
        }
    }

    fn push(
        &self,
        sender: *const UnixSocket,
        datagram: Datagram,
        limit: usize,
    ) -> Result<(), Refused> {
        let mut state = self.state.lock();
        if let Some(Peer {
            target: Target::Socket(peer),
            ..
        }) = &state.peer
            && peer.strong_count() > 0
            && !core::ptr::eq(Weak::as_ptr(peer), sender)
        {
            return Err(Refused::Failed(Errno::EPERM));
        }
        if state.read_shut {
            return Err(Refused::Failed(Errno::EPIPE));
        }
        if !Self::room_in(&state, datagram.bytes.len(), limit) {
            return Err(Refused::Full(datagram));
        }
        if state.queue.try_reserve(1).is_err() {
            return Err(Refused::Failed(Errno::ENOBUFS));
        }
        state.queued_bytes += datagram.bytes.len();
        state.queue.push_back(datagram);
        Ok(())
    }

    fn take(&self, peek: bool) -> Result<Option<Datagram>, Errno> {
        let mut state = self.state.lock();
        if !peek {
            let datagram = state.queue.pop_front();
            if let Some(datagram) = &datagram {
                state.queued_bytes -= datagram.bytes.len();
            }
            return Ok(datagram);
        }
        let Some(front) = state.queue.front() else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(front.bytes.len())
            .map_err(|_| Errno::ENOMEM)?;
        bytes.extend_from_slice(&front.bytes);
        Ok(Some(Datagram {
            sender: front.sender.clone(),
            bytes,
        }))
    }

    fn shutdown(&self, read: bool, write: bool) {
        let mut state = self.state.lock();
        state.read_shut |= read;
        state.write_shut |= write;
    }

    /// The `poll` events among `requested`: readable while a datagram waits or reading is shut,
    /// writable unless writing is shut or its peer's queue is full, hung up once both are shut.
    pub fn events(&self, requested: u16) -> u16 {
        let (readable, read_shut, write_shut, peer) = {
            let state = self.state.lock();
            (
                !state.queue.is_empty(),
                state.read_shut,
                state.write_shut,
                state.peer.as_ref().map(|peer| peer.target.clone()),
            )
        };
        let writable = !write_shut
            && match peer {
                Some(Target::Socket(peer)) => peer.upgrade().is_none_or(|peer| {
                    peer.datagrams()
                        .is_none_or(|datagrams| datagrams.has_room(1, receive_limit(&peer)))
                }),
                Some(Target::Log) | None => true,
            };
        let mut events = 0;
        if readable || read_shut {
            events |= requested & (event::IN | event::RDNORM);
        }
        if writable {
            events |= requested & (event::OUT | event::WRNORM);
        }
        if read_shut && write_shut {
            events |= event::HUP;
        }
        events
    }
}

fn receive_limit(socket: &UnixSocket) -> usize {
    socket.options().receive_buffer.load(Ordering::Relaxed) as usize
}

fn send_limit(socket: &UnixSocket) -> usize {
    socket.options().send_buffer.load(Ordering::Relaxed) as usize
}

/// Two datagram sockets connected to each other (`socketpair`).
pub fn pair() -> (Arc<UnixSocket>, Arc<UnixSocket>) {
    let first = UnixSocket::new_datagram();
    let second = UnixSocket::new_datagram();
    for (from, to) in [(&first, &second), (&second, &first)] {
        if let Some(datagrams) = from.datagrams() {
            datagrams.connect(Some(Peer {
                target: Target::Socket(Arc::downgrade(to)),
                address: Address::Unnamed,
            }));
        }
    }
    (first, second)
}

/// The datagram socket open at `fd`, if that is what it is.
pub fn socket_at(thread: &LinuxThread, fd: u64) -> Option<(Arc<OpenFile>, Arc<UnixSocket>)> {
    let file = thread.files().file(fd).ok()?;
    let socket = match file.kind() {
        FileKind::Socket(socket) if socket.is_datagram() => Arc::clone(socket),
        _ => return None,
    };
    Some((file, socket))
}

/// Every socket call on a datagram socket, by `number`, with the program's registers.
pub fn call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    context: &mut UserContext,
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
) -> ControlFlow<()> {
    let [first, second, third, fourth, fifth, sixth] = context.syscall_arguments();
    let result = match number {
        number::BIND => super::bind::<P>(process, thread, first, second, third),
        number::CONNECT => connect(process, thread, &socket, second, third),
        number::GETSOCKNAME => super::name::<P>(process, thread, first, second, third, false),
        number::GETPEERNAME => peer_name(process, &socket, second, third),
        number::SHUTDOWN => shutdown(&socket, second),
        number::SETSOCKOPT => {
            super::setsockopt(process, thread, first, second, third, fourth, fifth)
        }
        number::GETSOCKOPT => {
            super::getsockopt(process, thread, first, second, third, fourth, fifth)
        }
        number::LISTEN | number::ACCEPT | number::ACCEPT4 => Err(Errno::EOPNOTSUPP),
        number::SENDTO => {
            let send = match ends_in_user_space::<P>(second, third) {
                true => destination_at(process, fifth, sixth).map(|destination| Send {
                    file,
                    socket,
                    buffers: Iovecs::one(second, third),
                    destination,
                    flags: fourth as u32,
                }),
                false => Err(Errno::EFAULT),
            };
            return send_datagram(process, thread, context, send);
        }
        number::SENDMSG => {
            let send = read_header(process, second).and_then(|header| {
                if header.buffer_count > MAX_IOVECS {
                    return Err(Errno::EMSGSIZE);
                }
                if header.control_length != 0 {
                    return Err(Errno::EOPNOTSUPP);
                }
                let buffers = Iovecs::read(process, header.buffers, header.buffer_count)?;
                let destination = match header.name_length {
                    0 => None,
                    length => destination_at(process, header.name, u64::from(length))?,
                };
                Ok(Send {
                    file,
                    socket,
                    buffers,
                    destination,
                    flags: third as u32,
                })
            });
            return send_datagram(process, thread, context, send);
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
            return recvfrom(process, thread, context, &receive, fifth, sixth);
        }
        number::RECVMSG => return recvmsg(process, thread, context, file, socket, second, third),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn destination_at<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
) -> Result<Option<Address>, Errno> {
    match address {
        0 => Ok(None),
        address => read_address(process, address, length).map(Some),
    }
}

fn find_destination(
    caller: u32,
    thread: &LinuxThread,
    address: &Address,
) -> Result<Destination, Errno> {
    let key = match address {
        Address::Unnamed => return Err(Errno::EINVAL),
        Address::Abstract(name) => BindingKey::Abstract(Arc::clone(name)),
        Address::Path(path) => {
            let (root, start) =
                starting_points(thread, crate::linux::stat::at::FDCWD as u64, path)?;
            let node = vfs::resolve_as(root, start, path, caller, true).map_err(walk_error)?;
            if devfs::is_log_writer(node) {
                return Ok(Destination::Log);
            }
            if node.kind() != NodeKind::Socket {
                return Err(Errno::ECONNREFUSED);
            }
            BindingKey::Node(node)
        }
    };
    let socket = unix_socket::find(&key).ok_or(Errno::ECONNREFUSED)?;
    match socket.is_datagram() {
        true => Ok(Destination::Socket(socket)),
        false => Err(Errno::EPROTOTYPE),
    }
}

fn is_unspecified<P: Platform>(
    process: &Process<P>,
    address: u64,
    length: u64,
) -> Result<bool, Errno> {
    if (length as u32 as usize) < 2 {
        return Ok(false);
    }
    let mut family = [0u8; 2];
    process
        .read_user_memory(address, &mut family)
        .map_err(|_| Errno::EFAULT)?;
    Ok(u16::from_ne_bytes(family) == 0)
}

fn connect<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    socket: &UnixSocket,
    address: u64,
    length: u64,
) -> Result<u64, Errno> {
    let datagrams = socket.datagrams().ok_or(Errno::ENOTSOCK)?;
    if is_unspecified(process, address, length)? {
        datagrams.connect(None);
        socket.changed();
        return Ok(0);
    }
    let address = read_address(process, address, length)?;
    let target = match find_destination(process.id().get(), thread, &address)? {
        Destination::Log => Target::Log,
        Destination::Socket(peer) => Target::Socket(Arc::downgrade(&peer)),
    };
    datagrams.connect(Some(Peer { target, address }));
    socket.changed();
    Ok(0)
}

fn peer_name<P: Platform>(
    process: &Process<P>,
    socket: &UnixSocket,
    address: u64,
    length_at: u64,
) -> Result<u64, Errno> {
    let peer = socket
        .datagrams()
        .and_then(Datagrams::peer_address)
        .ok_or(Errno::ENOTCONN)?;
    if address == 0 {
        return Err(Errno::EFAULT);
    }
    write_address(process, address, length_at, &peer)?;
    Ok(0)
}

fn shutdown(socket: &UnixSocket, how: u64) -> Result<u64, Errno> {
    let (read, write) = match how as u32 {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(Errno::EINVAL),
    };
    let datagrams = socket.datagrams().ok_or(Errno::ENOTSOCK)?;
    datagrams.shutdown(read, write);
    socket.changed();
    Ok(0)
}

struct Send {
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
    buffers: Iovecs,
    destination: Option<Address>,
    flags: u32,
}

fn gather<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    limit: usize,
) -> Result<Vec<u8>, Errno> {
    let total = buffers.total();
    if total > limit as u64 {
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

fn prepare<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    send: &Send,
) -> Result<(Destination, Vec<u8>), Errno> {
    if send.flags & message::OOB != 0 {
        return Err(Errno::EOPNOTSUPP);
    }
    let datagrams = send.socket.datagrams().ok_or(Errno::ENOTSOCK)?;
    if datagrams.is_write_shut() {
        return Err(Errno::EPIPE);
    }
    let destination = match &send.destination {
        Some(address) => find_destination(process.id().get(), thread, address)?,
        None => datagrams.destination()?,
    };
    let bytes = gather(process, &send.buffers, send_limit(&send.socket))?;
    Ok((destination, bytes))
}

fn send_datagram<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    send: Result<Send, Errno>,
) -> ControlFlow<()> {
    let send = match send {
        Ok(send) => send,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let (destination, bytes) = match prepare(process, thread, &send) {
        Ok(prepared) => prepared,
        Err(error) => return finish_send(process, thread, context, Err(error), send.flags),
    };
    let sent = bytes.len() as u64;
    let result = match destination {
        Destination::Log => {
            android_log::write(&bytes, process.id().get());
            Ok(sent)
        }
        Destination::Socket(target) => {
            let datagram = Datagram {
                sender: send.socket.local_address(),
                bytes,
            };
            match deliver(process, thread, context, &send, &target, datagram)? {
                Some(result) => result.map(|()| sent),
                None => return ControlFlow::Continue(()),
            }
        }
    };
    finish_send(process, thread, context, result, send.flags)
}

fn deliver<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    send: &Send,
    target: &Arc<UnixSocket>,
    mut datagram: Datagram,
) -> ControlFlow<(), Option<Result<(), Errno>>> {
    let Some(receiving) = target.datagrams() else {
        return ControlFlow::Continue(Some(Err(Errno::EPROTOTYPE)));
    };
    let timeout = send.socket.options().send_timeout.load(Ordering::Relaxed);
    let interrupted = interrupted(context, timeout);
    let deadline = deadline::<P>(timeout);
    let nonblocking = send.file.is_nonblocking() || send.flags & message::DONTWAIT != 0;
    let limit = receive_limit(target);
    let sender = Arc::as_ptr(&send.socket);
    loop {
        match receiving.push(sender, datagram, limit) {
            Ok(()) => {
                target.changed();
                return ControlFlow::Continue(Some(Ok(())));
            }
            Err(Refused::Failed(error)) => return ControlFlow::Continue(Some(Err(error))),
            Err(Refused::Full(back)) => datagram = back,
        }
        if nonblocking {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let size = datagram.bytes.len();
        let lists = [target.wake_list()];
        match poll::wait_for(process, thread, &lists, deadline, || {
            receiving.has_room(size, limit)
        })? {
            Waited::Ready => {}
            Waited::TimedOut => return ControlFlow::Continue(Some(Err(Errno::EAGAIN))),
            Waited::Signalled => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(None);
                }
            }
        }
    }
}

/// `write` or `writev` on a datagram socket: one datagram to its peer, with no flags.
pub(in crate::linux) fn write<P: Platform>(
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
        destination: None,
        flags: 0,
    };
    send_datagram(process, thread, context, Ok(send))
}

struct Receive {
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
    buffers: Iovecs,
    flags: u32,
}

struct Received {
    copied: u64,
    length: u64,
    sender: Address,
}

fn receive<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    receive: &Receive,
) -> ControlFlow<(), Option<Result<Received, Errno>>> {
    let Some(datagrams) = receive.socket.datagrams() else {
        return ControlFlow::Continue(Some(Err(Errno::ENOTSOCK)));
    };
    if receive.flags & message::OOB != 0 {
        return ControlFlow::Continue(Some(Err(Errno::EOPNOTSUPP)));
    }
    let timeout = receive
        .socket
        .options()
        .receive_timeout
        .load(Ordering::Relaxed);
    let interrupted = interrupted(context, timeout);
    let deadline = deadline::<P>(timeout);
    let peek = receive.flags & message::PEEK != 0;
    let nonblocking = receive.file.is_nonblocking() || receive.flags & message::DONTWAIT != 0;
    loop {
        match datagrams.take(peek) {
            Err(error) => return ControlFlow::Continue(Some(Err(error))),
            Ok(Some(datagram)) => {
                if !peek {
                    receive.socket.changed();
                }
                return ControlFlow::Continue(Some(copy_out(process, &receive.buffers, datagram)));
            }
            Ok(None) => {}
        }
        if datagrams.is_read_shut() {
            return ControlFlow::Continue(Some(Ok(Received {
                copied: 0,
                length: 0,
                sender: Address::Unnamed,
            })));
        }
        if nonblocking {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let lists = [receive.socket.wake_list()];
        match poll::wait_for(process, thread, &lists, deadline, || {
            datagrams.has_datagrams() || datagrams.is_read_shut()
        })? {
            Waited::Ready => {}
            Waited::TimedOut => return ControlFlow::Continue(Some(Err(Errno::EAGAIN))),
            Waited::Signalled => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(None);
                }
            }
        }
    }
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    datagram: Datagram,
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
        sender: datagram.sender,
    })
}

fn returned(received: &Received, flags: u32) -> u64 {
    match flags & MSG_TRUNC {
        0 => received.copied,
        _ => received.length,
    }
}

fn recvfrom<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    receiving: &Receive,
    address: u64,
    length_at: u64,
) -> ControlFlow<()> {
    let Some(result) = receive(process, thread, context, receiving)? else {
        return ControlFlow::Continue(());
    };
    let result = result.and_then(|received| {
        write_address(process, address, length_at, &received.sender)?;
        Ok(returned(&received, receiving.flags))
    });
    finish(process, thread, context, result)
}

/// `read` or `readv` on a datagram socket: one datagram, with no flags.
pub(in crate::linux) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<UnixSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let receiving = Receive {
        file: Arc::clone(file),
        socket: Arc::clone(socket),
        buffers,
        flags: 0,
    };
    let Some(result) = receive(process, thread, context, &receiving)? else {
        return ControlFlow::Continue(());
    };
    finish(
        process,
        thread,
        context,
        result.map(|received| received.copied),
    )
}

fn recvmsg<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: Arc<OpenFile>,
    socket: Arc<UnixSocket>,
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
    let receiving = Receive {
        file,
        socket,
        buffers,
        flags: flags as u32,
    };
    let Some(result) = receive(process, thread, context, &receiving)? else {
        return ControlFlow::Continue(());
    };
    let result = result.and_then(|received| {
        let mut written = [0u8; MSGHDR_SIZE];
        process
            .read_user_memory(header_at, &mut written)
            .map_err(|_| Errno::EFAULT)?;
        if header.name != 0 {
            let (bytes, full) = encode_address(&received.sender);
            let copied = full.min(header.name_length as usize);
            process
                .write_user_memory(header.name, &bytes[..copied])
                .map_err(|_| Errno::EFAULT)?;
            written[8..12].copy_from_slice(&(full as u32).to_ne_bytes());
        }
        let message_flags = match received.copied < received.length {
            true => MSG_TRUNC,
            false => 0,
        };
        written[40..48].copy_from_slice(&0u64.to_ne_bytes());
        written[48..52].copy_from_slice(&message_flags.to_ne_bytes());
        process
            .write_user_memory(header_at, &written)
            .map_err(|_| Errno::EFAULT)?;
        Ok(returned(&received, receiving.flags))
    });
    finish(process, thread, context, result)
}

#[cfg(test)]
mod tests;

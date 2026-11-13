//! `AF_INET` stream sockets: TCP (D158), over the kernel's connections ([`crate::net::tcp`]).
//! `bind`, `listen`, `accept`, `accept4`, `connect`, `getsockname`, `getpeername`, `sendto`,
//! `sendmsg`, `recvfrom`, `recvmsg` (with `MSG_PEEK`, `MSG_DONTWAIT`, `MSG_WAITALL`),
//! `shutdown`, `setsockopt` and `getsockopt` (`SO_ERROR` reports a failed connection once,
//! `TCP_NODELAY` is always on; `SO_KEEPALIVE`, `TCP_KEEPIDLE`, `TCP_KEEPINTVL`, `TCP_KEEPCNT`
//! and `SO_OOBINLINE` work, D564), urgent data with `MSG_OOB` and `POLLPRI`, and `read`,
//! `write`, `poll`, `ioctl(FIONREAD)` and `fstat` on them, as Linux answers them.
//!
//! A socket that listens or connects before it binds is bound to a port the kernel picks. A
//! blocking `connect` waits for the handshake (`ECONNREFUSED` when the peer resets it,
//! `ETIMEDOUT` when it never answers), a non-blocking one returns `EINPROGRESS` and `poll`
//! reports the socket writable when it is done. A send queues what fits and waits for room
//! for the rest; a receive waits for data, the peer's FIN (a read of 0) or the end of the
//! connection. Closing a socket with received bytes it never read resets the connection, as
//! Linux does.

use super::inet::{
    self, IPPROTO_TCP, InetSocket, ensure_bound, finish_send, read_address, write_address,
};
use super::{MSGHDR_SIZE, SOL_SOCKET, deadline, finish, interrupted, message, option, read_header};
use crate::linux::descriptors::{FileKind, OpenFile, open_file_limit, open_flag};
use crate::linux::signals;
use crate::linux::terminal::{Waited, wait_for_input};
use crate::linux::thread::LinuxThread;
use crate::linux::{Errno, Iovecs, MAX_IOVECS, ends_in_user_space, iovec, number};
use crate::net::tcp::{self, Listener, Phase, Socket, Stream};
use crate::process::Process;
use alloc::sync::Arc;
use core::ops::ControlFlow;
use core::sync::atomic::Ordering;
use eggos_hal::Platform;
use eggos_hal::user::UserContext;
use eggos_net::tcp::{Error, State};

const TCP_NODELAY: u32 = 1;
const TCP_KEEPIDLE: u32 = 4;
const TCP_KEEPINTVL: u32 = 5;
const TCP_KEEPCNT: u32 = 6;
const SO_KEEPALIVE: u32 = 9;
const SO_OOBINLINE: u32 = 10;
const SO_LINGER: u32 = 13;
const MAX_KEEPALIVE_SECONDS: u32 = 32_767;
const MAX_KEEPALIVE_PROBES: u32 = 127;
const CHUNK: usize = 4096;

fn connection_error(error: Error) -> Errno {
    match error {
        Error::Refused => Errno::ECONNREFUSED,
        Error::Reset => Errno::ECONNRESET,
        Error::TimedOut => Errno::ETIMEDOUT,
    }
}

fn tcp_socket(socket: &InetSocket) -> Result<&Socket, Errno> {
    socket.stream().ok_or(Errno::ENOTSOCK)
}

fn connected(socket: &InetSocket) -> Result<Arc<Stream>, Errno> {
    match tcp_socket(socket)?.phase() {
        Phase::Connected(stream) => Ok(stream),
        _ => Err(Errno::ENOTCONN),
    }
}

/// Every socket call but `socket` and `socketpair` on the stream socket `socket` open as
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
        number::BIND => inet::bind(process, &socket, second, third),
        number::LISTEN => listen(&socket, second),
        number::GETSOCKNAME => name(process, &socket, second, third, false),
        number::GETPEERNAME => name(process, &socket, second, third, true),
        number::SHUTDOWN => shutdown::<P>(&socket, second),
        number::SETSOCKOPT => setsockopt(process, &socket, second, third, fourth, fifth),
        number::GETSOCKOPT => getsockopt(process, &socket, second, third, fourth, fifth),
        number::ACCEPT => {
            return accept(process, thread, context, &file, &socket, [second, third, 0]);
        }
        number::ACCEPT4 => {
            return accept(
                process,
                thread,
                context,
                &file,
                &socket,
                [second, third, fourth],
            );
        }
        number::CONNECT => return connect(process, thread, context, &file, &socket, second, third),
        number::SENDTO => {
            if !ends_in_user_space::<P>(second, third) {
                return finish(process, thread, context, Err(Errno::EFAULT));
            }
            let buffers = Iovecs::one(second, third);
            return send(
                process,
                thread,
                context,
                &file,
                &socket,
                buffers,
                fourth as u32,
            );
        }
        number::SENDMSG => {
            let prepared = read_header(process, second).and_then(|header| {
                if header.buffer_count > MAX_IOVECS {
                    return Err(Errno::EMSGSIZE);
                }
                Iovecs::read(process, header.buffers, header.buffer_count)
            });
            return match prepared {
                Ok(buffers) => send(
                    process,
                    thread,
                    context,
                    &file,
                    &socket,
                    buffers,
                    third as u32,
                ),
                Err(error) => finish(process, thread, context, Err(error)),
            };
        }
        number::RECVFROM => {
            if !ends_in_user_space::<P>(second, third) {
                return finish(process, thread, context, Err(Errno::EFAULT));
            }
            let receive = Receive {
                buffers: Iovecs::one(second, third),
                flags: fourth as u32,
            };
            let result = match receive_bytes(process, thread, context, &file, &socket, &receive)? {
                Some(result) => result,
                None => return ControlFlow::Continue(()),
            };
            let result = result.and_then(|count| {
                if fifth != 0 {
                    let peer = connected(&socket)?.remote();
                    write_address(process, socket.family(), fifth, sixth, peer)?;
                }
                Ok(count)
            });
            return finish(process, thread, context, result);
        }
        number::RECVMSG => return recvmsg(process, thread, context, &file, &socket, second, third),
        _ => Err(Errno::ENOSYS),
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

fn listen(socket: &InetSocket, backlog: u64) -> Result<u64, Errno> {
    let backlog = super::backlog_limit(backlog) as usize;
    let tcp_socket = tcp_socket(socket)?;
    match tcp_socket.phase() {
        Phase::Listening(listener) => {
            listener.set_backlog(backlog);
            Ok(0)
        }
        Phase::Connected(_) => Err(Errno::EINVAL),
        Phase::Idle => {
            let local = ensure_bound(socket)?;
            let listener = Listener::listen(local, backlog).ok_or(Errno::ENOBUFS)?;
            match tcp_socket.leave_idle(Phase::Listening(listener)) {
                true => Ok(0),
                false => Err(Errno::EINVAL),
            }
        }
    }
}

fn name<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    address: u64,
    length_at: u64,
    peer: bool,
) -> Result<u64, Errno> {
    if address == 0 {
        return Err(Errno::EFAULT);
    }
    let phase = tcp_socket(socket)?.phase();
    let value = match (peer, phase) {
        (true, Phase::Connected(stream)) => stream.remote(),
        (true, _) => return Err(Errno::ENOTCONN),
        (false, Phase::Connected(stream)) => stream.local(),
        (false, Phase::Listening(listener)) => listener.local(),
        (false, Phase::Idle) => socket
            .endpoint()
            .local()
            .unwrap_or((socket.family().unspecified(), 0)),
    };
    write_address(process, socket.family(), address, length_at, value)?;
    Ok(0)
}

fn shutdown<P: Platform>(socket: &InetSocket, how: u64) -> Result<u64, Errno> {
    let (read, write) = match how as u32 {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(Errno::EINVAL),
    };
    let stream = connected(socket)?;
    if read {
        stream.shut_down_receiving();
    }
    if write {
        stream.shut_down_sending();
    }
    tcp::run::<P>();
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
    match (level as u32, name as u32) {
        (SOL_SOCKET, SO_LINGER) => {
            if (length as u32) < 8 {
                return Err(Errno::EINVAL);
            }
            let mut bytes = [0u8; 8];
            process
                .read_user_memory(value, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let on = i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != 0;
            let seconds = i32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]).max(0) as u32;
            tcp_socket(socket)?.set_linger(on.then_some(seconds));
            Ok(0)
        }
        (SOL_SOCKET, SO_KEEPALIVE) | (SOL_SOCKET, SO_OOBINLINE) => {
            let on = read_option_int(process, value, length)? != 0;
            let tcp_socket = tcp_socket(socket)?;
            match name as u32 {
                SO_KEEPALIVE => tcp_socket.set_keep_alive::<P>(on),
                _ => tcp_socket.set_urgent_inline(on),
            }
            Ok(0)
        }
        (IPPROTO_TCP, TCP_KEEPIDLE) | (IPPROTO_TCP, TCP_KEEPINTVL) | (IPPROTO_TCP, TCP_KEEPCNT) => {
            let wanted = read_option_int(process, value, length)?;
            let limit = match name as u32 {
                TCP_KEEPCNT => MAX_KEEPALIVE_PROBES,
                _ => MAX_KEEPALIVE_SECONDS,
            };
            if wanted < 1 || wanted as u32 > limit {
                return Err(Errno::EINVAL);
            }
            let stream = connected(socket)?;
            let mut settings = stream.keep_alive_settings();
            match name as u32 {
                TCP_KEEPIDLE => settings.idle_ms = u64::from(wanted as u32) * 1000,
                TCP_KEEPINTVL => settings.interval_ms = u64::from(wanted as u32) * 1000,
                _ => settings.probes = wanted as u8,
            }
            stream.set_keep_alive_settings::<P>(settings);
            Ok(0)
        }
        (IPPROTO_TCP, _) => {
            if (length as u32) < 4 {
                return Err(Errno::EINVAL);
            }
            Ok(0)
        }
        _ => inet::setsockopt(process, socket, level, name, value, length),
    }
}

fn read_option_int<P: Platform>(process: &Process<P>, value: u64, length: u64) -> Result<i32, Errno> {
    if (length as u32) < 4 {
        return Err(Errno::EINVAL);
    }
    let mut bytes = [0u8; 4];
    process
        .read_user_memory(value, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    Ok(i32::from_ne_bytes(bytes))
}

fn getsockopt<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    level: u64,
    name: u64,
    value: u64,
    length_at: u64,
) -> Result<u64, Errno> {
    if (level as u32, name as u32) == (SOL_SOCKET, SO_LINGER) {
        let linger = tcp_socket(socket)?.linger();
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&i32::from(linger.is_some()).to_ne_bytes());
        bytes[4..].copy_from_slice(&(linger.unwrap_or(0) as i32).to_ne_bytes());
        return give(process, &bytes, value, length_at);
    }
    if (level as u32, name as u32) == (IPPROTO_TCP, TCP_INFO) {
        let bytes = tcp_info(&tcp_socket(socket)?.phase());
        return give(process, &bytes, value, length_at);
    }
    let phase = tcp_socket(socket)?.phase();
    let answer = match (level as u32, name as u32) {
        (SOL_SOCKET, option::TYPE) => super::SOCK_STREAM,
        (SOL_SOCKET, option::PROTOCOL) => IPPROTO_TCP,
        (SOL_SOCKET, option::ACCEPTCONN) => u32::from(matches!(phase, Phase::Listening(_))),
        (SOL_SOCKET, option::ERROR) => match phase {
            Phase::Connected(stream) => stream
                .take_error()
                .map_or(0, |error| connection_error(error).0 as u32),
            _ => 0,
        },
        (SOL_SOCKET, SO_KEEPALIVE) => match &phase {
            Phase::Connected(stream) => u32::from(stream.keeps_alive()),
            _ => 0,
        },
        (IPPROTO_TCP, TCP_KEEPIDLE) | (IPPROTO_TCP, TCP_KEEPINTVL) | (IPPROTO_TCP, TCP_KEEPCNT) => {
            let settings = match &phase {
                Phase::Connected(stream) => stream.keep_alive_settings(),
                _ => eggos_net::tcp::KeepAlive::LINUX,
            };
            match name as u32 {
                TCP_KEEPIDLE => (settings.idle_ms / 1000) as u32,
                TCP_KEEPINTVL => (settings.interval_ms / 1000) as u32,
                _ => u32::from(settings.probes),
            }
        }
        (IPPROTO_TCP, TCP_NODELAY) => 1,
        _ => return inet::getsockopt(process, socket, level, name, value, length_at),
    };
    let room = super::read_int(process, length_at)?;
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let bytes = answer.to_ne_bytes();
    let written = bytes.len().min(room as usize);
    process
        .write_user_memory(value, &bytes[..written])
        .map_err(|_| Errno::EFAULT)?;
    process
        .write_user_memory(length_at, &(written as u32).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

const TCP_INFO: u32 = 11;
/// `struct tcp_info` up to `tcpi_total_retrans`, as old programs and `ss` read it.
const TCP_INFO_BYTES: usize = 104;

/// `TCP_INFO`'s answer (D559): Linux's state number, the segment size both ways, and the bytes
/// waiting to be read and acknowledged; the counters EggOS's TCP does not keep read 0.
fn tcp_info(phase: &Phase) -> [u8; TCP_INFO_BYTES] {
    let mut info = [0u8; TCP_INFO_BYTES];
    let state = match phase {
        Phase::Idle => 7,
        Phase::Listening(_) => 10,
        Phase::Connected(stream) => match stream.status().state {
            State::Established => 1,
            State::SynSent => 2,
            State::SynReceived => 3,
            State::FinWait1 => 4,
            State::FinWait2 => 5,
            State::TimeWait => 6,
            State::Closed => 7,
            State::CloseWait => 8,
            State::LastAck => 9,
            State::Closing => 11,
        },
    };
    info[0] = state;
    let mss = u32::from(crate::net::tcp_mss());
    info[16..20].copy_from_slice(&mss.to_ne_bytes());
    info[20..24].copy_from_slice(&mss.to_ne_bytes());
    info
}

fn give<P: Platform>(process: &Process<P>, bytes: &[u8], value: u64, length_at: u64) -> Result<u64, Errno> {
    let room = super::read_int(process, length_at)?;
    if room < 0 {
        return Err(Errno::EINVAL);
    }
    let written = bytes.len().min(room as usize);
    process
        .write_user_memory(value, &bytes[..written])
        .map_err(|_| Errno::EFAULT)?;
    process
        .write_user_memory(length_at, &(written as u32).to_ne_bytes())
        .map_err(|_| Errno::EFAULT)?;
    Ok(0)
}

/// The `poll` events of `requested` a stream socket has, as Linux reports them: a listener
/// is readable with a connection to accept; a connection is readable with data, at the
/// peer's FIN or when it ended, writable with room to send once connected, and reports
/// `POLLERR` for an error not yet told and `POLLHUP` once both directions are over.
pub fn events(socket: &Socket, requested: u16) -> u16 {
    use crate::linux::poll::event;
    let readable = requested & (event::IN | event::RDNORM);
    let writable = requested & (event::OUT | event::WRNORM);
    match socket.phase() {
        Phase::Idle => writable | event::HUP,
        Phase::Listening(listener) => match listener.has_ready() {
            true => readable,
            false => 0,
        },
        Phase::Connected(stream) => {
            let status = stream.status();
            let mut events = 0;
            if status.readable > 0 || status.read_ended {
                events |= readable;
            }
            let connecting = matches!(status.state, State::SynSent | State::SynReceived);
            if !connecting && (status.send_room > 0 || status.state == State::Closed) {
                events |= writable;
            }
            if status.error.is_some() {
                events |= event::ERR;
            }
            if status.urgent {
                events |= requested & event::PRI;
            }
            if status.read_ended && !matches!(status.state, State::Established | State::CloseWait) {
                events |= event::HUP;
            }
            if status.read_ended {
                events |= requested & event::RDHUP;
            }
            events
        }
    }
}

/// `ioctl` on a stream socket: `FIONREAD` the bytes waiting to be read, as an `int`; `None`
/// for another request.
pub fn ioctl<P: Platform>(
    process: &Process<P>,
    socket: &Socket,
    request: u64,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    if request as u32 != crate::linux::terminal::request::FIONREAD {
        return None;
    }
    let readable = match socket.phase() {
        Phase::Connected(stream) => stream.status().readable,
        _ => 0,
    };
    let readable = i32::try_from(readable).unwrap_or(i32::MAX);
    Some(
        process
            .write_user_memory(argument, &readable.to_ne_bytes())
            .map(|()| 0)
            .map_err(|_| Errno::EFAULT),
    )
}

fn accept<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    [address, length_at, flags]: [u64; 3],
) -> ControlFlow<()> {
    let flags = flags as u32;
    let listener = match (
        flags & !super::SOCKET_FLAGS,
        tcp_socket(socket).map(Socket::phase),
    ) {
        (0, Ok(Phase::Listening(listener))) => listener,
        (0, Ok(_)) => return finish(process, thread, context, Err(Errno::EINVAL)),
        (0, Err(error)) => return finish(process, thread, context, Err(error)),
        _ => return finish(process, thread, context, Err(Errno::EINVAL)),
    };
    let timeout = socket.options().receive_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    let result = loop {
        tcp::run::<P>();
        if let Some(stream) = listener.take_ready() {
            if let Ok(listening) = tcp_socket(socket) {
                listening.hand_options_to::<P>(&stream);
            }
            let family = socket.family();
            break install_accepted(process, thread, stream, family, address, length_at, flags);
        }
        if file.is_nonblocking() {
            break Err(Errno::EAGAIN);
        }
        match wait_for_input(process, thread, deadline, || listener.has_ready())? {
            Waited::Ready => {}
            Waited::TimedOut => break Err(Errno::EAGAIN),
            Waited::Signalled => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    finish(process, thread, context, result)
}

fn install_accepted<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    stream: Arc<Stream>,
    family: inet::Family,
    address: u64,
    length_at: u64,
    flags: u32,
) -> Result<u64, Errno> {
    let peer = stream.remote();
    let accepted = InetSocket::accepted(stream, tcp::run::<P>, family);
    let file = Arc::new(OpenFile::new(
        FileKind::InetSocket(accepted),
        open_flag::RDWR | flags & open_flag::NONBLOCK,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & open_flag::CLOEXEC != 0, limit)?;
    if address != 0
        && let Err(error) = write_address(process, family, address, length_at, peer)
    {
        let closed = thread.files().lock().remove(fd);
        drop(closed);
        return Err(error);
    }
    Ok(u64::from(fd))
}

#[allow(clippy::too_many_arguments)]
fn connect<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    address: u64,
    length: u64,
) -> ControlFlow<()> {
    let started = start_connection::<P>(process, socket, address, length);
    let (stream, fresh) = match started {
        Ok(started) => started,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    let timeout = socket.options().send_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    let result = loop {
        let status = stream.status();
        let connecting = matches!(status.state, State::SynSent | State::SynReceived);
        if !connecting {
            break match stream.take_error() {
                Some(error) => Err(connection_error(error)),
                None if fresh => Ok(0),
                None => Err(Errno::EISCONN),
            };
        }
        if file.is_nonblocking() {
            break Err(match fresh {
                true => Errno::EINPROGRESS,
                false => Errno::EALREADY,
            });
        }
        let ready = || {
            let state = stream.status().state;
            state != State::SynSent && state != State::SynReceived
        };
        match wait_for_input(process, thread, deadline, ready)? {
            Waited::Ready => {}
            Waited::TimedOut => break Err(Errno::EINPROGRESS),
            Waited::Signalled => {
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(());
                }
            }
        }
    };
    finish(process, thread, context, result)
}

fn start_connection<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    address: u64,
    length: u64,
) -> Result<(Arc<Stream>, bool), Errno> {
    let tcp_socket = tcp_socket(socket)?;
    match tcp_socket.phase() {
        Phase::Connected(stream) => return Ok((stream, false)),
        Phase::Listening(_) => return Err(Errno::EINVAL),
        Phase::Idle => {}
    }
    let (address, port) = read_address(process, socket.family(), address, length)?
        .ok_or(Errno::EAFNOSUPPORT)?;
    if !socket.takes(address) {
        return Err(Errno::EAFNOSUPPORT);
    }
    let remote = match address.is_unspecified() {
        true => (socket.family().loopback(), port),
        false => (address.unmapped(), port),
    };
    let broadcast = remote.0.ipv4().is_some_and(|address| address.is_broadcast());
    if broadcast || remote.0.is_multicast() || port == 0 {
        return Err(Errno::ENETUNREACH);
    }
    let (bound, local_port) = ensure_bound(socket)?;
    let source = match bound.is_unspecified() {
        true => crate::net::source_for_any::<P>(remote.0),
        false => bound.unmapped(),
    };
    if source.is_unspecified() {
        return Err(Errno::ENETUNREACH);
    }
    let stream = Stream::connect::<P>((source, local_port), remote).ok_or(Errno::ENOBUFS)?;
    if !tcp_socket.leave_idle(Phase::Connected(Arc::clone(&stream))) {
        return Err(Errno::EALREADY);
    }
    tcp_socket.hand_options_to::<P>(&stream);
    tcp::run::<P>();
    Ok((stream, true))
}

fn send<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    buffers: Iovecs,
    flags: u32,
) -> ControlFlow<()> {
    let result = send_bytes(process, thread, context, file, socket, &buffers, flags)?;
    if flags & message::OOB != 0
        && matches!(result, Some(Ok(sent)) if sent > 0)
        && let Ok(stream) = connected(socket)
    {
        stream.mark_urgent();
        tcp::run::<P>();
    }
    match result {
        Some(result) => finish_send(process, thread, context, result, flags),
        None => ControlFlow::Continue(()),
    }
}

/// `write` or `writev` on a stream socket: a send with no flags.
pub(in crate::linux) fn write<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    socket: &Arc<InetSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let file = match thread.files().file(context.syscall_arguments()[0]) {
        Ok(file) => file,
        Err(error) => return finish(process, thread, context, Err(error)),
    };
    send(process, thread, context, &file, socket, buffers, 0)
}

fn sendable(stream: &Stream) -> Result<usize, Errno> {
    let status = stream.status();
    if let Some(error) = status.error {
        let _ = stream.take_error();
        return Err(connection_error(error));
    }
    match status.state {
        State::SynSent | State::SynReceived => Ok(0),
        State::Established | State::CloseWait => Ok(status.send_room),
        _ => Err(Errno::EPIPE),
    }
}

fn copy_in<P: Platform>(
    process: &Process<P>,
    buffers: &Iovecs,
    offset: u64,
    out: &mut [u8],
) -> Result<(), Errno> {
    let mut skip = offset;
    let mut filled = 0;
    for entry in buffers.entries() {
        if filled == out.len() {
            break;
        }
        let (base, length) = iovec(entry);
        if skip >= length {
            skip -= length;
            continue;
        }
        let count = ((length - skip) as usize).min(out.len() - filled);
        process
            .read_user_memory(base + skip, &mut out[filled..filled + count])
            .map_err(|_| Errno::EFAULT)?;
        filled += count;
        skip = 0;
    }
    Ok(())
}

fn send_bytes<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    buffers: &Iovecs,
    flags: u32,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    let stream = match connected(socket) {
        Ok(stream) => stream,
        Err(_) => return ControlFlow::Continue(Some(Err(Errno::EPIPE))),
    };
    let nonblocking = file.is_nonblocking() || flags & message::DONTWAIT != 0;
    let total = buffers.total();
    let timeout = socket.options().send_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    let mut written = 0u64;
    let mut chunk = [0u8; CHUNK];
    loop {
        while written < total {
            let room = match sendable(&stream) {
                Ok(room) => room,
                Err(error) if written == 0 => return ControlFlow::Continue(Some(Err(error))),
                Err(_) => break,
            };
            let count = (room as u64).min(total - written).min(CHUNK as u64) as usize;
            if count == 0 {
                break;
            }
            if let Err(error) = copy_in(process, buffers, written, &mut chunk[..count]) {
                return ControlFlow::Continue(Some(match written {
                    0 => Err(error),
                    _ => Ok(written),
                }));
            }
            let taken = stream.write(&chunk[..count]);
            written += taken as u64;
            if taken < count {
                break;
            }
        }
        tcp::run::<P>();
        if written == total || (written > 0 && nonblocking) {
            return ControlFlow::Continue(Some(Ok(written)));
        }
        if let Err(error) = sendable(&stream) {
            return ControlFlow::Continue(Some(match written {
                0 => Err(error),
                _ => Ok(written),
            }));
        }
        if nonblocking {
            return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
        }
        let ready = || {
            let status = stream.status();
            let open = matches!(
                status.state,
                State::SynSent | State::SynReceived | State::Established | State::CloseWait
            );
            status.send_room > 0 || status.error.is_some() || !open
        };
        match wait_for_input(process, thread, deadline, ready)? {
            Waited::Ready => {}
            Waited::TimedOut => {
                return ControlFlow::Continue(Some(match written {
                    0 => Err(Errno::EAGAIN),
                    _ => Ok(written),
                }));
            }
            Waited::Signalled => {
                if written > 0 {
                    return ControlFlow::Continue(Some(Ok(written)));
                }
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(None);
                }
            }
        }
    }
}

struct Receive {
    buffers: Iovecs,
    flags: u32,
}

fn copy_out<P: Platform>(
    process: &Process<P>,
    stream: &Stream,
    buffers: &Iovecs,
    offset: u64,
    peek: bool,
) -> Result<u64, Errno> {
    let mut chunk = [0u8; CHUNK];
    let mut copied = 0u64;
    let mut skip_buffers = offset;
    for entry in buffers.entries() {
        let (base, length) = iovec(entry);
        if skip_buffers >= length {
            skip_buffers -= length;
            continue;
        }
        let mut at = skip_buffers;
        skip_buffers = 0;
        while at < length {
            let room = ((length - at) as usize).min(CHUNK);
            let skip = match peek {
                true => (offset + copied) as usize,
                false => 0,
            };
            let count = stream.read(&mut chunk[..room], skip, peek);
            if count == 0 {
                return Ok(copied);
            }
            process
                .write_user_memory(base + at, &chunk[..count])
                .map_err(|_| Errno::EFAULT)?;
            at += count as u64;
            copied += count as u64;
        }
    }
    Ok(copied)
}

fn receive_bytes<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    receive: &Receive,
) -> ControlFlow<(), Option<Result<u64, Errno>>> {
    if receive.flags & message::OOB != 0 {
        return ControlFlow::Continue(Some(receive_urgent(process, socket, receive)));
    }
    let stream = match connected(socket) {
        Ok(stream) => stream,
        Err(error) => return ControlFlow::Continue(Some(Err(error))),
    };
    let peek = receive.flags & message::PEEK != 0;
    let wait_all = receive.flags & message::WAITALL != 0 && !peek;
    let nonblocking = file.is_nonblocking() || receive.flags & message::DONTWAIT != 0;
    let total = receive.buffers.total();
    let timeout = socket.options().receive_timeout.load(Ordering::Relaxed);
    let deadline = deadline::<P>(timeout);
    let interrupted = interrupted(context, timeout);
    let mut received = 0u64;
    loop {
        if received < total {
            match copy_out(process, &stream, &receive.buffers, received, peek) {
                Ok(count) => received += count,
                Err(error) => return ControlFlow::Continue(Some(Err(error))),
            }
            if !peek {
                tcp::run::<P>();
            }
        }
        let status = stream.status();
        let done = received == total || (received > 0 && !wait_all);
        if done || (received > 0 && (status.read_ended || nonblocking)) {
            return ControlFlow::Continue(Some(Ok(received)));
        }
        if received == 0 {
            if let Some(error) = stream.take_error() {
                return ControlFlow::Continue(Some(Err(connection_error(error))));
            }
            if status.read_ended || total == 0 {
                return ControlFlow::Continue(Some(Ok(0)));
            }
            if nonblocking {
                return ControlFlow::Continue(Some(Err(Errno::EAGAIN)));
            }
        }
        let ready = || {
            let status = stream.status();
            status.readable > 0 || status.read_ended || status.error.is_some()
        };
        match wait_for_input(process, thread, deadline, ready)? {
            Waited::Ready => {}
            Waited::TimedOut => {
                return ControlFlow::Continue(Some(match received {
                    0 => Err(Errno::EAGAIN),
                    _ => Ok(received),
                }));
            }
            Waited::Signalled => {
                if received > 0 {
                    return ControlFlow::Continue(Some(Ok(received)));
                }
                if signals::deliver_interrupting(process, thread, context, interrupted)? {
                    return ControlFlow::Continue(None);
                }
            }
        }
    }
}

/// `read` or `readv` on a stream socket: a receive with no flags.
/// `recv(MSG_OOB)` (D564): the urgent byte the peer sent, apart from the stream, as Linux:
/// `EINVAL` when there is none or urgent bytes stay in the stream, `EAGAIN` while it is
/// announced but has not come yet.
fn receive_urgent<P: Platform>(
    process: &Process<P>,
    socket: &InetSocket,
    receive: &Receive,
) -> Result<u64, Errno> {
    let stream = connected(socket)?;
    let peek = receive.flags & message::PEEK != 0;
    let Some(byte) = stream.take_urgent_byte(peek) else {
        return match stream.status().urgent {
            true => Err(Errno::EAGAIN),
            false => Err(Errno::EINVAL),
        };
    };
    let Some((base, _)) = receive
        .buffers
        .entries()
        .map(iovec)
        .find(|&(_, length)| length > 0)
    else {
        return Ok(0);
    };
    process
        .write_user_memory(base, &[byte])
        .map_err(|_| Errno::EFAULT)?;
    Ok(1)
}

pub(in crate::linux) fn read<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
    buffers: Iovecs,
) -> ControlFlow<()> {
    let receive = Receive { buffers, flags: 0 };
    let result = match receive_bytes(process, thread, context, file, socket, &receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    finish(process, thread, context, result)
}

fn recvmsg<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    file: &Arc<OpenFile>,
    socket: &Arc<InetSocket>,
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
        buffers,
        flags: flags as u32,
    };
    let result = match receive_bytes(process, thread, context, file, socket, &receive)? {
        Some(result) => result,
        None => return ControlFlow::Continue(()),
    };
    let result = result.and_then(|count| {
        let mut written = [0u8; MSGHDR_SIZE];
        process
            .read_user_memory(header_at, &mut written)
            .map_err(|_| Errno::EFAULT)?;
        if header.name != 0 {
            let peer = connected(socket)?.remote();
            let (bytes, size) = inet::encode_address(peer.0, peer.1, socket.family());
            let copied = size.min(header.name_length as usize);
            process
                .write_user_memory(header.name, &bytes[..copied])
                .map_err(|_| Errno::EFAULT)?;
            written[8..12].copy_from_slice(&(size as u32).to_ne_bytes());
        }
        written[40..48].copy_from_slice(&0u64.to_ne_bytes());
        written[48..52].copy_from_slice(&0u32.to_ne_bytes());
        process
            .write_user_memory(header_at, &written)
            .map_err(|_| Errno::EFAULT)?;
        Ok(count)
    });
    finish(process, thread, context, result)
}

#[cfg(test)]
mod tests;

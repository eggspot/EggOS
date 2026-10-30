//! `AF_UNIX` stream sockets (D138): the objects behind `socket`, `socketpair`, `bind`,
//! `listen`, `accept`, `connect` and the calls that move bytes and descriptors between
//! processes; [`super::socket`] reads their arguments from the program. A datagram socket
//! (D188) is an open socket with a datagram side ([`Datagrams`],
//! [`super::socket::unix_datagram`]), bound and found through the same registry.
//!
//! A [`UnixSocket`] is open, listening or connected. A connected pair shares one
//! [`Connection`]: a queue of bytes towards each side, growing on demand up to
//! [`QUEUE_CAPACITY`], with the descriptors sent by `SCM_RIGHTS` kept beside the bytes they
//! came with, so a read stops before the next group and hands a group over with its first
//! byte. A bound socket is found by its address in a registry that exists only once something
//! was bound (D18).
//!
//! Locks are leaf locks taken one socket state at a time, then at most that socket's
//! connection; the registry's lock is taken alone. Descriptors leave a queue through
//! [`release_rights`], outside every lock and without recursion. A socket may be passed only
//! when it carries no sockets itself, so passed sockets never form a cycle, and at most
//! [`MAX_IN_FLIGHT`] descriptors are in flight at once.
//!
//! Each socket has a wake list ([`WakeList`]) for its waiters (blocked calls, `poll`,
//! `select`, `epoll`); a connection keeps its two sides' lists and wakes both after each
//! change to it ([`Connection::changed`]), a listener's own list is woken when connections
//! come and go. While nobody waits, a change costs the connection's lock and two relaxed loads.

use super::Errno;
use super::descriptors::{FileKind, OpenFile};
use super::poll::{WakeList, event};
use super::socket::unix_datagram::Datagrams;
use crate::sync::LeafSpinLock;
use crate::vfs::Node;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The most bytes queued towards one side of a connection.
pub const QUEUE_CAPACITY: usize = 256 * 1024;

/// Every connection's queues together hold at most `1 / QUEUE_MEMORY_SHARE` of memory: an
/// eighth (D155). A queue that cannot grow past it, or past the reserve programs leave the
/// kernel, makes its writer wait while it holds bytes, and is `ENOMEM` when it is empty.
pub const QUEUE_MEMORY_SHARE: u64 = 8;

static QUEUE_MEMORY: AtomicU64 = AtomicU64::new(0);

/// The room a side's send queue needs for `poll` to report it writable, as pipes need
/// `PIPE_BUF`.
pub const WRITE_ROOM: usize = 4096;

/// The most descriptors one message carries (`SCM_MAX_FD`).
pub const MAX_RIGHTS_PER_MESSAGE: usize = 253;

/// The most descriptors queued towards one side of a connection.
pub const MAX_QUEUED_RIGHTS: usize = 1024;

/// The most descriptors in flight in every connection together.
pub const MAX_IN_FLIGHT: u32 = 4096;

/// The longest backlog `listen` accepts (`SOMAXCONN`).
pub const MAX_BACKLOG: u32 = 4096;

/// The buffer size a new socket reports (`SO_SNDBUF`, `SO_RCVBUF`).
pub const DEFAULT_BUFFER: u32 = 212_992;

const AUTOBIND_NAMES: u32 = 1 << 20;

static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

static PASSING: LeafSpinLock<()> = LeafSpinLock::new_leaf(());

static BOUND_SOCKETS: LeafSpinLock<Option<Box<BoundSockets>>> = LeafSpinLock::new_leaf(None);

static RELEASING: LeafSpinLock<Releasing> = LeafSpinLock::new_leaf(Releasing {
    files: None,
    draining: false,
});

static LAST_INODE: AtomicU64 = AtomicU64::new(0);

/// A socket's address: none, a path in the file system, or an abstract name (without its
/// leading NUL).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Address {
    /// Not bound.
    #[default]
    Unnamed,
    /// A socket file's path, as it was bound.
    Path(Arc<[u8]>),
    /// An abstract name.
    Abstract(Arc<[u8]>),
}

/// The process and ids `SO_PEERCRED` reports (`struct ucred`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

impl PeerCredentials {
    /// What a socket with no peer reports, as Linux does.
    pub const NONE: Self = Self {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
}

/// One side of a connection: the one that connected (or a pair's first), or the one accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    First,
    Second,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

/// Descriptors a message carries, of which `sockets` name sockets.
#[derive(Debug)]
pub struct OutgoingRights {
    pub files: Vec<Arc<OpenFile>>,
    pub sockets: u32,
}

impl OutgoingRights {
    /// `files`, counting the sockets among them.
    pub fn new(files: Vec<Arc<OpenFile>>) -> Self {
        let sockets = files
            .iter()
            .filter(|file| matches!(file.kind(), FileKind::Socket(_)))
            .count() as u32;
        Self { files, sockets }
    }
}

#[derive(Debug)]
struct InFlightRights {
    position: u64,
    files: Vec<Arc<OpenFile>>,
    sockets: u32,
}

#[derive(Debug, Default)]
struct Direction {
    bytes: VecDeque<u8>,
    taken: u64,
    rights: VecDeque<InFlightRights>,
    queued_rights: usize,
}

impl Direction {
    fn room(&self) -> usize {
        QUEUE_CAPACITY - self.bytes.len()
    }

    fn end(&self) -> u64 {
        self.taken + self.bytes.len() as u64
    }

    fn readable_from(&self, cursor: u64, first: bool) -> usize {
        let stop = self
            .rights
            .iter()
            .map(|rights| rights.position)
            .find(|&position| match first {
                true => position > cursor,
                false => position >= cursor,
            })
            .unwrap_or(self.end())
            .min(self.end());
        stop.saturating_sub(cursor) as usize
    }

    fn copy_out(&self, start: usize, out: &mut [u8]) {
        let (front, back) = self.bytes.as_slices();
        let mut done = 0;
        if start < front.len() {
            done = (front.len() - start).min(out.len());
            out[..done].copy_from_slice(&front[start..start + done]);
        }
        if done < out.len() {
            let from = start + done - front.len();
            let rest = out.len() - done;
            out[done..].copy_from_slice(&back[from..from + rest]);
        }
    }

    fn take_rights_at_front(&mut self) -> Option<InFlightRights> {
        if self.rights.front()?.position != self.taken {
            return None;
        }
        let rights = self.rights.pop_front()?;
        self.queued_rights -= rights.files.len();
        Some(rights)
    }

    fn clear(&mut self) -> Vec<InFlightRights> {
        self.let_go_of_bytes();
        self.queued_rights = 0;
        self.rights.drain(..).collect()
    }

    fn growth_for(&self, count: usize) -> usize {
        let held = self.bytes.capacity();
        let needed = self.bytes.len() + count;
        if needed <= held {
            return 0;
        }
        (held * 2).clamp(needed, QUEUE_CAPACITY.max(needed)) - held
    }

    fn may_grow(growth: usize, every_queue_limit: u64) -> bool {
        growth == 0
            || (QUEUE_MEMORY
                .load(Ordering::Relaxed)
                .saturating_add(growth as u64)
                <= every_queue_limit
                && crate::heap::may_grow_for_user(growth))
    }

    fn can_take(&self, room: usize, every_queue_limit: u64) -> bool {
        self.room() >= room
            && (self.bytes.is_empty() || Self::may_grow(self.growth_for(1), every_queue_limit))
    }

    fn make_room(&mut self, count: usize, every_queue_limit: u64) -> Result<(), Errno> {
        let growth = self.growth_for(count);
        if growth == 0 {
            return Ok(());
        }
        if !Self::may_grow(growth, every_queue_limit) {
            return Err(Errno::ENOMEM);
        }
        let held = self.bytes.capacity();
        self.bytes
            .try_reserve_exact(held + growth - self.bytes.len())
            .map_err(|_| Errno::ENOMEM)?;
        QUEUE_MEMORY.fetch_add((self.bytes.capacity() - held) as u64, Ordering::Relaxed);
        Ok(())
    }

    fn shrink_when_empty(&mut self) {
        let held = self.bytes.capacity();
        if !self.bytes.is_empty() || held <= WRITE_ROOM {
            return;
        }
        self.bytes.shrink_to(WRITE_ROOM);
        QUEUE_MEMORY.fetch_sub((held - self.bytes.capacity()) as u64, Ordering::Relaxed);
    }

    fn let_go_of_bytes(&mut self) {
        QUEUE_MEMORY.fetch_sub(self.bytes.capacity() as u64, Ordering::Relaxed);
        self.bytes = VecDeque::new();
    }
}

impl Drop for Direction {
    fn drop(&mut self) {
        self.let_go_of_bytes();
    }
}

#[derive(Debug, Default)]
struct ConnectionState {
    towards: [Direction; 2],
    closed: [bool; 2],
    read_shut: [bool; 2],
    write_shut: [bool; 2],
    reset: [bool; 2],
    addresses: [Address; 2],
    credentials: [Option<PeerCredentials>; 2],
    sockets_carried: u32,
    watchers: [Option<Arc<WakeList>>; 2],
}

impl ConnectionState {
    fn receive_shut(&self, side: Side) -> bool {
        let other = side.other().index();
        self.closed[other] || self.write_shut[other] || self.read_shut[side.index()]
    }

    fn send_shut(&self, side: Side) -> bool {
        let other = side.other().index();
        self.closed[other] || self.read_shut[other] || self.write_shut[side.index()]
    }

    fn let_go(&mut self, rights: Vec<InFlightRights>) -> Vec<Arc<OpenFile>> {
        let mut files = Vec::new();
        for group in rights {
            self.sockets_carried -= group.sockets;
            IN_FLIGHT.fetch_sub(group.files.len() as u32, Ordering::Relaxed);
            if files.is_empty() {
                files = group.files;
            } else {
                files.extend(group.files);
            }
        }
        files
    }
}

/// What a read from a connection took ([`Connection::receive`], [`Connection::peek`]).
#[derive(Debug)]
pub enum Taken {
    /// `count` bytes, and the descriptors that came with the first of them.
    Bytes {
        count: usize,
        rights: Option<Vec<Arc<OpenFile>>>,
    },
    /// The next bytes carry descriptors: a read that took some stops before them.
    Boundary,
    /// Nothing queued yet.
    Empty,
    /// Nothing queued, and nothing more will come.
    Ended,
    /// The peer closed with bytes it had not read (`ECONNRESET`, reported once).
    Reset,
}

/// A connected pair's shared state. See the module documentation.
#[derive(Debug)]
pub struct Connection {
    state: LeafSpinLock<ConnectionState>,
}

impl Connection {
    /// A connection whose first side has `address` and `credentials`; its second gets its own
    /// once queued ([`UnixSocket::queue`]) or at once for a pair.
    pub fn new(address: Address, credentials: PeerCredentials) -> Arc<Self> {
        let mut state = ConnectionState::default();
        state.addresses[0] = address;
        state.credentials[0] = Some(credentials);
        Arc::new(Self {
            state: LeafSpinLock::new_leaf(state),
        })
    }

    fn set(&self, side: Side, address: Address, credentials: PeerCredentials) {
        let mut state = self.state.lock();
        state.addresses[side.index()] = address;
        state.credentials[side.index()] = Some(credentials);
    }

    fn attach(&self, side: Side, list: &Arc<WakeList>) {
        self.state.lock().watchers[side.index()] = Some(Arc::clone(list));
    }

    /// Wakes the waiters of both sides' sockets: call it after every change, with no lock
    /// held.
    pub fn changed(&self) {
        let watched = {
            let state = self.state.lock();
            state.watchers.each_ref().map(|list| {
                list.as_ref()
                    .filter(|list| list.is_watched())
                    .map(Arc::clone)
            })
        };
        for list in watched.iter().flatten() {
            list.wake();
        }
    }

    /// The address of `side`'s socket: its own name, as `getsockname` reports.
    pub fn address(&self, side: Side) -> Address {
        self.state.lock().addresses[side.index()].clone()
    }

    /// The credentials of `side`'s process when it connected, listened or made the pair.
    pub fn credentials(&self, side: Side) -> PeerCredentials {
        self.state.lock().credentials[side.index()].unwrap_or(PeerCredentials::NONE)
    }

    /// Queues up to all of `bytes` from `from` towards its peer, with `rights` before the
    /// first of them if given (taken then): how many, 0 when the queue is full, or when it
    /// cannot grow (every queue at [`QUEUE_MEMORY_SHARE`] of memory, or the free frames at the
    /// reserve) while it holds bytes its peer will read. `EPIPE` once the peer is gone or either
    /// side shut the way down, `ETOOMANYREFS` past the limits on queued descriptors, `ENOMEM`
    /// when an empty queue cannot grow.
    pub fn send(
        &self,
        from: Side,
        bytes: &[u8],
        rights: &mut Option<OutgoingRights>,
    ) -> Result<usize, Errno> {
        let to = from.other().index();
        let mut state = self.state.lock();
        if state.send_shut(from) {
            return Err(Errno::EPIPE);
        }
        let count = bytes.len().min(state.towards[to].room());
        if count == 0 {
            return Ok(0);
        }
        let direction = &mut state.towards[to];
        let limit = crate::memory::share_of_memory(QUEUE_MEMORY_SHARE);
        let count = match direction.make_room(count, limit) {
            Ok(()) => count,
            Err(error) => match direction.make_room(1, limit) {
                Ok(()) => count.min(direction.bytes.capacity() - direction.bytes.len()),
                Err(_) if !direction.bytes.is_empty() => return Ok(0),
                Err(_) => return Err(error),
            },
        };
        if let Some(outgoing) = rights.as_ref() {
            let files = outgoing.files.len();
            if direction.queued_rights + files > MAX_QUEUED_RIGHTS {
                return Err(Errno::ETOOMANYREFS);
            }
            direction.rights.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
            IN_FLIGHT
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_flight| {
                    in_flight
                        .checked_add(files as u32)
                        .filter(|&total| total <= MAX_IN_FLIGHT)
                })
                .map_err(|_| Errno::ETOOMANYREFS)?;
            let Some(outgoing) = rights.take() else {
                unreachable!("checked above")
            };
            let position = direction.end();
            direction.queued_rights += files;
            direction.rights.push_back(InFlightRights {
                position,
                files: outgoing.files,
                sockets: outgoing.sockets,
            });
            state.sockets_carried += outgoing.sockets;
        }
        state.towards[to].bytes.extend(&bytes[..count]);
        Ok(count)
    }

    /// [`Self::send`], first checking under the lock that serialises passing sockets that no
    /// socket in `rights` would make a cycle: one that carries sockets itself, one whose
    /// connection is this one, or a listener with connections waiting (`ETOOMANYREFS`).
    pub fn send_passing(
        &self,
        from: Side,
        bytes: &[u8],
        rights: &mut Option<OutgoingRights>,
    ) -> Result<usize, Errno> {
        if rights.as_ref().is_none_or(|rights| rights.sockets == 0) {
            return self.send(from, bytes, rights);
        }
        let _passing = PASSING.lock();
        let passable = rights.as_ref().is_none_or(|rights| {
            rights.files.iter().all(|file| match file.kind() {
                FileKind::Socket(socket) => socket.passable_into(self),
                _ => true,
            })
        });
        if !passable {
            return Err(Errno::ETOOMANYREFS);
        }
        self.send(from, bytes, rights)
    }

    /// Takes up to `out.len()` bytes queued towards `side` into `out`, stopping before
    /// descriptors that came with later bytes. The `first` read of a call also takes the
    /// descriptors that came with its first byte, and reports a pending reset.
    pub fn receive(&self, side: Side, out: &mut [u8], first: bool) -> Taken {
        let mut state = self.state.lock();
        let index = side.index();
        if state.towards[index].bytes.is_empty() {
            return Self::nothing(&mut state, side, first);
        }
        let direction = &mut state.towards[index];
        let available = direction.readable_from(direction.taken, first);
        if available == 0 {
            return Taken::Boundary;
        }
        let group = match first {
            true => direction.take_rights_at_front(),
            false => None,
        };
        let count = available.min(out.len());
        direction.copy_out(0, &mut out[..count]);
        direction.bytes.drain(..count);
        direction.taken += count as u64;
        direction.shrink_when_empty();
        let rights = group.map(|group| state.let_go(alloc::vec![group]));
        Taken::Bytes { count, rights }
    }

    /// As [`Self::receive`] without taking anything: from `offset` bytes past the first queued,
    /// never with descriptors (`MSG_PEEK`).
    pub fn peek(&self, side: Side, offset: usize, out: &mut [u8], first: bool) -> Taken {
        let mut state = self.state.lock();
        let index = side.index();
        if state.towards[index].bytes.len() <= offset {
            return Self::nothing(&mut state, side, first);
        }
        let direction = &state.towards[index];
        let available = direction.readable_from(direction.taken + offset as u64, first);
        if available == 0 {
            return Taken::Boundary;
        }
        let count = available.min(out.len());
        direction.copy_out(offset, &mut out[..count]);
        Taken::Bytes {
            count,
            rights: None,
        }
    }

    fn nothing(state: &mut ConnectionState, side: Side, first: bool) -> Taken {
        let index = side.index();
        if first && state.reset[index] {
            state.reset[index] = false;
            return Taken::Reset;
        }
        match state.receive_shut(side) {
            true => Taken::Ended,
            false => Taken::Empty,
        }
    }

    /// Whether a read on `side` would not wait: bytes queued, the end, or a reset.
    pub fn can_receive(&self, side: Side) -> bool {
        let state = self.state.lock();
        !state.towards[side.index()].bytes.is_empty()
            || state.receive_shut(side)
            || state.reset[side.index()]
    }

    /// Whether a write on `side` would not wait: room queued towards the peer, or an error.
    pub fn can_send(&self, side: Side) -> bool {
        let state = self.state.lock();
        let limit = crate::memory::share_of_memory(QUEUE_MEMORY_SHARE);
        state.send_shut(side) || state.towards[side.other().index()].can_take(1, limit)
    }

    /// The bytes queued towards `side` (`FIONREAD`).
    pub fn available(&self, side: Side) -> usize {
        self.state.lock().towards[side.index()].bytes.len()
    }

    /// The bytes `side` sent that its peer has not read (`SIOCOUTQ`).
    pub fn unread_sent(&self, side: Side) -> usize {
        self.state.lock().towards[side.other().index()].bytes.len()
    }

    /// Takes `side`'s pending error (`SO_ERROR`): whether it was a reset.
    pub fn take_reset(&self, side: Side) -> bool {
        core::mem::take(&mut self.state.lock().reset[side.index()])
    }

    /// Whether `side` shut down writing (`SHUT_WR`): a send of nothing is `EPIPE` then, as
    /// any other send.
    pub fn write_shut(&self, side: Side) -> bool {
        self.state.lock().write_shut[side.index()]
    }

    /// `shutdown` from `side`: no more reading, no more writing, or both.
    pub fn shutdown(&self, side: Side, read: bool, write: bool) {
        let mut state = self.state.lock();
        state.read_shut[side.index()] |= read;
        state.write_shut[side.index()] |= write;
    }

    /// `side` closes: its peer reads what is queued, then the end, after a reset if `side` left
    /// bytes unread. The descriptors queued towards `side` are returned for
    /// [`release_rights`].
    #[must_use]
    pub fn close(&self, side: Side) -> Vec<Arc<OpenFile>> {
        self.close_resetting(side, false)
    }

    /// [`Self::close`], always resetting the peer: a connection a listener never accepted, or
    /// one a `connect` that lost a race gives up.
    #[must_use]
    pub fn abort(&self, side: Side) -> Vec<Arc<OpenFile>> {
        self.close_resetting(side, true)
    }

    fn close_resetting(&self, side: Side, always: bool) -> Vec<Arc<OpenFile>> {
        let mut state = self.state.lock();
        let index = side.index();
        state.closed[index] = true;
        if always || !state.towards[index].bytes.is_empty() {
            state.reset[side.other().index()] = true;
        }
        let rights = state.towards[index].clear();
        state.let_go(rights)
    }

    /// The `poll` events of `side` among `requested`.
    pub fn events(&self, side: Side, requested: u16) -> u16 {
        let state = self.state.lock();
        let index = side.index();
        let receive_shut = state.receive_shut(side);
        let mut events = 0;
        if !state.towards[index].bytes.is_empty() || receive_shut {
            events |= requested & (event::IN | event::RDNORM);
        }
        if receive_shut {
            events |= requested & event::RDHUP;
        }
        if receive_shut && state.send_shut(side) {
            events |= event::HUP;
        }
        if state.reset[index] {
            events |= event::ERR;
        }
        let limit = crate::memory::share_of_memory(QUEUE_MEMORY_SHARE);
        if state.towards[side.other().index()].can_take(WRITE_ROOM, limit) {
            events |= requested & (event::OUT | event::WRNORM);
        }
        events
    }

    fn carries_sockets(&self) -> bool {
        self.state.lock().sockets_carried > 0
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        let mut rights = state.towards[0].clear();
        rights.extend(state.towards[1].clear());
        let files = state.let_go(rights);
        release_rights(files);
    }
}

#[derive(Debug)]
struct Listening {
    address: Address,
    backlog: VecDeque<Arc<Connection>>,
    limit: u32,
    credentials: PeerCredentials,
}

#[derive(Debug)]
enum SocketState {
    Open {
        address: Address,
    },
    Connecting {
        address: Address,
    },
    Listening(Listening),
    Connected {
        connection: Arc<Connection>,
        side: Side,
    },
}

/// A socket's options (`setsockopt`): kept and reported, the timeouts acted on.
#[derive(Debug)]
pub struct Options {
    pub send_buffer: AtomicU32,
    pub receive_buffer: AtomicU32,
    pub pass_credentials: AtomicBool,
    pub reuse_address: AtomicBool,
    pub receive_timeout: AtomicU64,
    pub send_timeout: AtomicU64,
}

impl Options {
    fn new() -> Self {
        Self {
            send_buffer: AtomicU32::new(DEFAULT_BUFFER),
            receive_buffer: AtomicU32::new(DEFAULT_BUFFER),
            pass_credentials: AtomicBool::new(false),
            reuse_address: AtomicBool::new(false),
            receive_timeout: AtomicU64::new(0),
            send_timeout: AtomicU64::new(0),
        }
    }
}

/// What a socket is: see the module documentation.
pub struct UnixSocket {
    state: LeafSpinLock<SocketState>,
    options: Options,
    inode: u64,
    wake_list: Arc<WakeList>,
    datagrams: Option<Box<Datagrams>>,
}

impl core::fmt::Debug for UnixSocket {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("UnixSocket")
            .field(&self.inode)
            .finish()
    }
}

impl PartialEq for UnixSocket {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other)
    }
}

impl Eq for UnixSocket {}

/// Why [`UnixSocket::begin_connect`] or [`UnixSocket::connected`] found no connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotConnected {
    /// It listens.
    Listening,
    /// It is open, bound or not.
    Open,
}

impl UnixSocket {
    fn with(state: SocketState) -> Arc<Self> {
        Self::built(state, None)
    }

    fn built(state: SocketState, datagrams: Option<Box<Datagrams>>) -> Arc<Self> {
        let wake_list = WakeList::shared();
        if let SocketState::Connected { connection, side } = &state {
            connection.attach(*side, &wake_list);
        }
        Arc::new(Self {
            state: LeafSpinLock::new_leaf(state),
            options: Options::new(),
            inode: LAST_INODE.fetch_add(1, Ordering::Relaxed) + 1,
            wake_list,
            datagrams,
        })
    }

    /// A new open socket.
    pub fn new() -> Arc<Self> {
        Self::with(SocketState::Open {
            address: Address::Unnamed,
        })
    }

    /// A new open datagram socket (`SOCK_DGRAM`, D188).
    pub fn new_datagram() -> Arc<Self> {
        Self::built(
            SocketState::Open {
                address: Address::Unnamed,
            },
            Some(Box::new(Datagrams::new())),
        )
    }

    /// Its datagram side, if it is a datagram socket.
    pub fn datagrams(&self) -> Option<&Datagrams> {
        self.datagrams.as_deref()
    }

    /// Whether it is a datagram socket.
    pub fn is_datagram(&self) -> bool {
        self.datagrams.is_some()
    }

    /// The second side of `connection`, accepted.
    pub fn accepted(connection: Arc<Connection>) -> Arc<Self> {
        Self::with(SocketState::Connected {
            connection,
            side: Side::Second,
        })
    }

    /// A connected pair, both made by the process `credentials` names (`socketpair`).
    pub fn pair(credentials: PeerCredentials) -> (Arc<Self>, Arc<Self>) {
        let connection = Connection::new(Address::Unnamed, credentials);
        connection.set(Side::Second, Address::Unnamed, credentials);
        let first = Self::with(SocketState::Connected {
            connection: Arc::clone(&connection),
            side: Side::First,
        });
        (first, Self::accepted(connection))
    }

    /// Its number, as `fstat` reports it (`st_ino`).
    pub fn inode(&self) -> u64 {
        self.inode
    }

    /// Its options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// The threads and `epoll` instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.wake_list
    }

    /// Wakes the threads that wait for it and, if it is connected, for its peer: call it
    /// after every change, with no lock held.
    pub fn changed(&self) {
        self.wake_list.wake();
        if let Ok((connection, _)) = self.connected() {
            connection.changed();
        }
    }

    /// Its connection and side, if it is connected.
    pub fn connected(&self) -> Result<(Arc<Connection>, Side), NotConnected> {
        match &*self.state.lock() {
            SocketState::Connected { connection, side } => Ok((Arc::clone(connection), *side)),
            SocketState::Listening(_) => Err(NotConnected::Listening),
            SocketState::Open { .. } | SocketState::Connecting { .. } => Err(NotConnected::Open),
        }
    }

    /// Whether it listens.
    pub fn is_listening(&self) -> bool {
        matches!(&*self.state.lock(), SocketState::Listening(_))
    }

    /// Its own address (`getsockname`).
    pub fn local_address(&self) -> Address {
        let connection = match &*self.state.lock() {
            SocketState::Open { address } | SocketState::Connecting { address } => {
                return address.clone();
            }
            SocketState::Listening(listening) => return listening.address.clone(),
            SocketState::Connected { connection, side } => (Arc::clone(connection), *side),
        };
        connection.0.address(connection.1)
    }

    /// Its peer's address (`getpeername`), if it is connected.
    pub fn peer_address(&self) -> Option<Address> {
        let (connection, side) = self.connected().ok()?;
        Some(connection.address(side.other()))
    }

    /// Its peer's credentials (`SO_PEERCRED`): [`PeerCredentials::NONE`] without a peer.
    pub fn peer_credentials(&self) -> PeerCredentials {
        match self.connected() {
            Ok((connection, side)) => connection.credentials(side.other()),
            Err(_) => PeerCredentials::NONE,
        }
    }

    /// Whether it is open and not bound, as `bind` needs (`EINVAL` otherwise).
    pub fn check_unbound(&self) -> Result<(), Errno> {
        match &*self.state.lock() {
            SocketState::Open {
                address: Address::Unnamed,
            } => Ok(()),
            _ => Err(Errno::EINVAL),
        }
    }

    /// Records the address it was bound to: `EINVAL` if it was bound meanwhile.
    pub fn set_bound(&self, bound: Address) -> Result<(), Errno> {
        match &mut *self.state.lock() {
            SocketState::Open { address } if *address == Address::Unnamed => {
                *address = bound;
                Ok(())
            }
            _ => Err(Errno::EINVAL),
        }
    }

    /// `listen` with a backlog of `limit` connections, by the process `credentials` names:
    /// `EINVAL` unless it is bound and not connected. Listening again sets the backlog.
    pub fn listen(&self, limit: u32, credentials: PeerCredentials) -> Result<(), Errno> {
        let mut state = self.state.lock();
        match &mut *state {
            SocketState::Listening(listening) => {
                listening.limit = limit;
                Ok(())
            }
            SocketState::Open { address } if *address != Address::Unnamed => {
                let address = core::mem::take(address);
                *state = SocketState::Listening(Listening {
                    address,
                    backlog: VecDeque::new(),
                    limit,
                    credentials,
                });
                Ok(())
            }
            _ => Err(Errno::EINVAL),
        }
    }

    /// The next connection waiting to be accepted, `None` if none waits; `EINVAL` if it does
    /// not listen.
    pub fn accept(&self) -> Result<Option<Arc<Connection>>, Errno> {
        match &mut *self.state.lock() {
            SocketState::Listening(listening) => Ok(listening.backlog.pop_front()),
            _ => Err(Errno::EINVAL),
        }
    }

    /// Puts `connection` back at the head of its backlog, after `accept` found no descriptor
    /// for it; `false` if it no longer listens.
    pub fn put_back(&self, connection: Arc<Connection>) -> bool {
        match &mut *self.state.lock() {
            SocketState::Listening(listening) => {
                if listening.backlog.try_reserve(1).is_err() {
                    return false;
                }
                listening.backlog.push_front(connection);
                true
            }
            _ => false,
        }
    }

    /// Whether `accept` would not wait.
    pub fn can_accept(&self) -> bool {
        match &*self.state.lock() {
            SocketState::Listening(listening) => !listening.backlog.is_empty(),
            _ => true,
        }
    }

    /// Marks it connecting and returns its own address, before a `connect` queues its
    /// connection: a connecting socket can neither listen, be bound nor be passed, so it cannot
    /// end up inside its own connection. `EISCONN` if it is connected, `EALREADY` if another
    /// call connects it, `EINVAL` if it listens. The caller then finishes
    /// ([`Self::finish_connect`]) or gives up ([`Self::abandon_connect`]).
    pub fn begin_connect(&self) -> Result<Address, Errno> {
        let mut state = self.state.lock();
        match &mut *state {
            SocketState::Open { address } => {
                let address = core::mem::take(address);
                *state = SocketState::Connecting {
                    address: address.clone(),
                };
                Ok(address)
            }
            SocketState::Connecting { .. } => Err(Errno::EALREADY),
            SocketState::Connected { .. } => Err(Errno::EISCONN),
            SocketState::Listening(_) => Err(Errno::EINVAL),
        }
    }

    /// Makes a connecting socket open again: its `connect` failed.
    pub fn abandon_connect(&self) {
        let mut state = self.state.lock();
        if let SocketState::Connecting { address } = &mut *state {
            let address = core::mem::take(address);
            *state = SocketState::Open { address };
        }
    }

    /// Queues `connection` for `accept`, giving its second side this listener's address and
    /// credentials: `true` if it is queued, `false` if the backlog is full, `ECONNREFUSED` if
    /// the socket does not listen.
    pub fn queue(&self, connection: &Arc<Connection>) -> Result<bool, Errno> {
        let mut state = self.state.lock();
        let SocketState::Listening(listening) = &mut *state else {
            return Err(Errno::ECONNREFUSED);
        };
        if listening.backlog.len() > listening.limit as usize {
            return Ok(false);
        }
        listening
            .backlog
            .try_reserve(1)
            .map_err(|_| Errno::ENOMEM)?;
        connection.set(
            Side::Second,
            listening.address.clone(),
            listening.credentials,
        );
        listening.backlog.push_back(Arc::clone(connection));
        Ok(true)
    }

    /// Whether [`Self::queue`] would not wait: room in the backlog, or no listener.
    pub fn can_queue(&self) -> bool {
        match &*self.state.lock() {
            SocketState::Listening(listening) => {
                listening.backlog.len() <= listening.limit as usize
            }
            _ => true,
        }
    }

    /// Makes a connecting socket the first side of `connection` once that is queued: its
    /// address goes with the connection. `EINVAL` if it is not connecting; the caller then
    /// closes the connection.
    pub fn finish_connect(&self, connection: &Arc<Connection>) -> Result<(), Errno> {
        let mut state = self.state.lock();
        match &mut *state {
            SocketState::Connecting { address } => {
                let address = core::mem::take(address);
                let credentials = connection.credentials(Side::First);
                connection.set(Side::First, address, credentials);
                connection.attach(Side::First, &self.wake_list);
                *state = SocketState::Connected {
                    connection: Arc::clone(connection),
                    side: Side::First,
                };
                Ok(())
            }
            SocketState::Connected { .. } => Err(Errno::EISCONN),
            SocketState::Open { .. } | SocketState::Listening(_) => Err(Errno::EINVAL),
        }
    }

    /// Gives up its connection without closing it, back to an open socket: an accepted
    /// socket that found no descriptor.
    pub fn detach(&self) -> Option<Arc<Connection>> {
        let mut state = self.state.lock();
        let previous = core::mem::replace(
            &mut *state,
            SocketState::Open {
                address: Address::Unnamed,
            },
        );
        match previous {
            SocketState::Connected { connection, .. } => Some(connection),
            other => {
                *state = other;
                None
            }
        }
    }

    /// The `poll` events among `requested`: a listener is readable while connections wait; an
    /// open socket is writable and hung up, as on Linux.
    pub fn events(&self, requested: u16) -> u16 {
        if let Some(datagrams) = &self.datagrams {
            return datagrams.events(requested);
        }
        match &*self.state.lock() {
            SocketState::Open { .. } | SocketState::Connecting { .. } => {
                requested & (event::OUT | event::WRNORM) | event::HUP
            }
            SocketState::Listening(listening) => match listening.backlog.is_empty() {
                true => 0,
                false => requested & (event::IN | event::RDNORM),
            },
            SocketState::Connected { connection, side } => connection.events(*side, requested),
        }
    }

    fn passable_into(&self, receiving: &Connection) -> bool {
        match &*self.state.lock() {
            SocketState::Open { .. } => true,
            SocketState::Connecting { .. } => false,
            SocketState::Listening(listening) => listening.backlog.is_empty(),
            SocketState::Connected { connection, .. } => {
                !core::ptr::eq(&**connection, receiving) && !connection.carries_sockets()
            }
        }
    }

    fn listing(&self) -> (bool, bool) {
        match &*self.state.lock() {
            SocketState::Listening(_) => (true, false),
            SocketState::Connected { .. } => (false, true),
            SocketState::Open { .. } | SocketState::Connecting { .. } => (false, false),
        }
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        let state = core::mem::replace(
            self.state.get_mut(),
            SocketState::Open {
                address: Address::Unnamed,
            },
        );
        match state {
            SocketState::Connected { connection, side } => {
                release_rights(connection.close(side));
                connection.changed();
            }
            SocketState::Listening(listening) => {
                for connection in listening.backlog {
                    release_rights(connection.abort(Side::Second));
                    connection.changed();
                }
            }
            SocketState::Open { .. } | SocketState::Connecting { .. } => {}
        }
        unregister(&*self);
        self.wake_list.wake();
    }
}

/// What a bound socket is found by: its socket file's node, or its abstract name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingKey {
    Node(Node),
    Abstract(Arc<[u8]>),
}

#[derive(Debug, Default)]
struct BoundSockets {
    entries: Vec<(BindingKey, Weak<UnixSocket>)>,
    next_autobind: u32,
}

/// Records that `socket` is bound to `key`: `EADDRINUSE` if a live socket has it.
pub fn register(key: BindingKey, socket: &Arc<UnixSocket>) -> Result<(), Errno> {
    let mut bound = BOUND_SOCKETS.lock();
    let sockets = bound.get_or_insert_with(Box::default);
    sockets
        .entries
        .retain(|(_, socket)| socket.strong_count() > 0);
    if sockets.entries.iter().any(|(bound, _)| *bound == key) {
        return Err(Errno::EADDRINUSE);
    }
    sockets.entries.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
    sockets.entries.push((key, Arc::downgrade(socket)));
    Ok(())
}

/// Binds `socket` to an abstract name of five hexadecimal digits no live socket has
/// (autobind): the name. `EADDRINUSE` if every one is taken.
pub fn register_autobound(socket: &Arc<UnixSocket>) -> Result<Arc<[u8]>, Errno> {
    let mut bound = BOUND_SOCKETS.lock();
    let sockets = bound.get_or_insert_with(Box::default);
    sockets
        .entries
        .retain(|(_, socket)| socket.strong_count() > 0);
    for _ in 0..AUTOBIND_NAMES {
        let number = sockets.next_autobind % AUTOBIND_NAMES;
        sockets.next_autobind = (number + 1) % AUTOBIND_NAMES;
        let name = autobind_name(number);
        let key = BindingKey::Abstract(Arc::clone(&name));
        if sockets.entries.iter().any(|(bound, _)| *bound == key) {
            continue;
        }
        sockets.entries.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
        sockets.entries.push((key, Arc::downgrade(socket)));
        return Ok(name);
    }
    Err(Errno::EADDRINUSE)
}

fn autobind_name(number: u32) -> Arc<[u8]> {
    let digits = *b"0123456789abcdef";
    let name: [u8; 5] =
        core::array::from_fn(|index| digits[(number >> (4 * (4 - index)) & 0xf) as usize]);
    Arc::from(&name[..])
}

/// The live socket bound to `key`, if any. The registry's lock is released before the caller
/// locks the socket.
pub fn find(key: &BindingKey) -> Option<Arc<UnixSocket>> {
    let bound = BOUND_SOCKETS.lock();
    bound
        .as_ref()?
        .entries
        .iter()
        .find(|(bound, _)| bound == key)
        .and_then(|(_, socket)| socket.upgrade())
}

/// Forgets every binding of `socket`, and the registry once it holds none.
pub fn unregister(socket: *const UnixSocket) {
    let mut bound = BOUND_SOCKETS.lock();
    let Some(sockets) = bound.as_mut() else {
        return;
    };
    sockets.entries.retain(|(_, entry)| {
        entry.strong_count() > 0 && !core::ptr::eq(Weak::as_ptr(entry), socket)
    });
    if sockets.entries.is_empty() {
        *bound = None;
    }
}

/// Forgets `key`'s binding if `socket` has it, as a `bind` that lost a race undoes it.
pub fn unregister_key(key: &BindingKey, socket: &UnixSocket) {
    let mut bound = BOUND_SOCKETS.lock();
    let Some(sockets) = bound.as_mut() else {
        return;
    };
    sockets
        .entries
        .retain(|(bound, entry)| !(bound == key && core::ptr::eq(Weak::as_ptr(entry), socket)));
    if sockets.entries.is_empty() {
        *bound = None;
    }
}

/// Whether anything is bound: the registry exists only then.
pub fn has_bindings() -> bool {
    BOUND_SOCKETS.lock().is_some()
}

/// How many descriptors are in flight in every connection together.
pub fn in_flight() -> u32 {
    IN_FLIGHT.load(Ordering::Relaxed)
}

struct Releasing {
    #[expect(
        clippy::box_collection,
        reason = "a null box keeps the static all zeros, in .bss"
    )]
    files: Option<Box<Vec<Arc<OpenFile>>>>,
    draining: bool,
}

/// Drops `files`, descriptors that left a queue, outside every lock and without recursing:
/// a socket among them that carries descriptors itself hands them here, and the first caller
/// drops them one by one until none is left, so the kernel stack stays as deep as one drop
/// however long a chain of passed sockets is.
pub fn release_rights(files: Vec<Arc<OpenFile>>) {
    if files.is_empty() {
        return;
    }
    {
        let mut releasing = RELEASING.lock();
        if releasing.draining {
            let list = releasing.files.get_or_insert_with(Box::default);
            if list.try_reserve(files.len()).is_ok() {
                list.extend(files);
                return;
            }
            drop(releasing);
            drop(files);
            return;
        }
        releasing.draining = true;
    }
    drop(files);
    loop {
        let next = {
            let mut releasing = RELEASING.lock();
            match releasing.files.as_mut().and_then(|list| list.pop()) {
                Some(file) => Some(file),
                None => {
                    releasing.draining = false;
                    releasing.files = None;
                    None
                }
            }
        };
        match next {
            Some(file) => drop(file),
            None => return,
        }
    }
}

struct Text(Vec<u8>);

impl core::fmt::Write for Text {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0
            .try_reserve(text.len())
            .map_err(|_| core::fmt::Error)?;
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// `/proc/net/unix`'s text: Linux's header, then a line for each bound socket, its kernel
/// address shown as zeros as Linux shows it to programs.
pub fn proc_net_unix() -> Option<Vec<u8>> {
    use core::fmt::Write;
    let sockets: Vec<(BindingKey, Arc<UnixSocket>)> = {
        let bound = BOUND_SOCKETS.lock();
        let mut sockets = Vec::new();
        if let Some(bound) = bound.as_ref() {
            sockets.try_reserve(bound.entries.len()).ok()?;
            for (key, socket) in &bound.entries {
                if let Some(socket) = socket.upgrade() {
                    sockets.push((key.clone(), socket));
                }
            }
        }
        sockets
    };
    let mut text = Text(Vec::new());
    let written = (|| {
        writeln!(
            text,
            "Num       RefCount Protocol Flags    Type St Inode Path"
        )?;
        for (key, socket) in &sockets {
            let (listening, connected) = socket.listing();
            let flags = if listening { 0x0001_0000 } else { 0 };
            let state = if connected { 3 } else { 1 };
            write!(
                text,
                "{:016x}: {:08X} {:08X} {:08X} {:04X} {:02X} {:5} ",
                0,
                Arc::strong_count(socket),
                0,
                flags,
                if socket.is_datagram() { 2 } else { 1 },
                state,
                socket.inode()
            )?;
            match key {
                BindingKey::Node(_) => {
                    let address = socket.local_address();
                    if let Address::Path(path) = address {
                        text.0
                            .try_reserve(path.len())
                            .map_err(|_| core::fmt::Error)?;
                        text.0.extend_from_slice(&path);
                    }
                }
                BindingKey::Abstract(name) => {
                    text.0
                        .try_reserve(name.len() + 1)
                        .map_err(|_| core::fmt::Error)?;
                    text.0.push(b'@');
                    text.0.extend_from_slice(name);
                }
            }
            writeln!(text)?;
        }
        Ok::<(), core::fmt::Error>(())
    })();
    drop(sockets);
    written.ok()?;
    Some(text.0)
}

#[cfg(test)]
static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn tests_lock() -> std::sync::MutexGuard<'static, ()> {
    TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;

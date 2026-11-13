//! TCP for programs (D158): the connections the kernel runs, the listeners that take new ones,
//! and the sockets programs hold them through. The protocol is the `eggos-net` crate's
//! ([`eggos_net::tcp::Connection`]); this module keeps each connection's bytes
//! ([`Stream`]), finds the connection or the listener each segment is for ([`deliver`]),
//! answers segments no one owns with a reset, and runs every connection's timers and output
//! ([`run`]): after each socket call, and from the network's poll thread when there is an
//! adapter.
//!
//! Segments to this machine (`127.0.0.0/8` or the interface's own address) never reach the
//! adapter: they wait in the outbox, which [`run`] delivers until the connections have
//! nothing more to say. Connections stay in the table until they are over (`TIME-WAIT`
//! included), whether or not a program still holds them; the memory their queues take is the
//! programs', so it grows only while the kernel's reserve allows it (D155).

use super::{is_local, now_ms};
use crate::sync::{LeafSpinLock, SpinLock};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use eggos_hal::Platform;
use eggos_net::checksum::{self, Completion};
use eggos_net::interface::SendError;
use eggos_net::ipv4::{self, Ipv4Address};
use eggos_net::tcp::{
    self, Buffers, Connection, Error, Extensions, KeepAlive, Segment, State, flags,
};

/// The segment size connections ask for and send at most: an Ethernet link's.
pub const MSS: u16 = 1460;
/// The bytes a connection queues for sending before a program's send waits (D564: a scaled
/// window lets more be in flight).
pub const SEND_LIMIT: usize = 256 * 1024;
/// The bytes a connection keeps received before its window closes (D564), out-of-order data
/// held for it included.
pub const RECEIVE_LIMIT: usize = 1024 * 1024;
/// The window scale shift connections offer: the smallest that lets a window reach
/// [`RECEIVE_LIMIT`].
pub const RECEIVE_WINDOW_SCALE: u8 = 5;
/// The most separate out-of-order runs a connection holds; a segment that would make more is
/// dropped and sent again.
pub const HELD_RUNS: usize = 32;

const _: () = assert!(tcp::MAX_WINDOW << RECEIVE_WINDOW_SCALE >= RECEIVE_LIMIT);
/// The most segments one connection sends in one [`run`].
pub const SEGMENTS_PER_RUN: usize = 64;
/// The most rounds of loopback delivery one [`run`] makes.
pub const LOOPBACK_ROUNDS: usize = 16;
/// The most segments the outbox holds (the loopback's and the resets that answer segments no
/// connection owns); more are dropped, as a link drops them.
pub const OUTBOX_SEGMENTS: usize = 1024;
/// The room a segment takes while it is written.
pub const SEGMENT_BYTES: usize = tcp::MAX_HEADER_BYTES + MSS as usize;
/// The room a segment for an adapter that cuts segments itself takes at most (D541): the
/// largest an IPv4 packet carries.
pub const LARGE_SEGMENT_BYTES: usize = u16::MAX as usize - ipv4::HEADER_BYTES;

static SEGMENT_SCRATCH: LeafSpinLock<Vec<u8>> = LeafSpinLock::new_leaf(Vec::new());

#[derive(Debug, Default)]
struct Queues {
    send: VecDeque<u8>,
    received: VecDeque<u8>,
    held: Vec<(u32, Vec<u8>)>,
    held_bytes: usize,
    last_held: Option<u32>,
}

impl Buffers for Queues {
    fn send_len(&self) -> usize {
        self.send.len()
    }

    fn copy_unacknowledged(&self, offset: usize, out: &mut [u8]) -> usize {
        copy_from(&self.send, offset, out)
    }

    fn acknowledged(&mut self, count: usize) {
        self.send.drain(..count.min(self.send.len()));
        if self.send.is_empty() {
            self.send.shrink_to_fit();
        }
    }

    fn receive_room(&self) -> usize {
        RECEIVE_LIMIT.saturating_sub(self.received.len() + self.held_bytes)
    }

    fn received(&mut self, data: &[u8]) -> usize {
        let count = data.len().min(self.receive_room());
        if count == 0
            || !crate::heap::may_grow_for_user(count)
            || self.received.try_reserve(count).is_err()
        {
            return 0;
        }
        self.received.extend(&data[..count]);
        count
    }

    fn hold_out_of_order(&mut self, sequence: u32, data: &[u8]) -> bool {
        if data.is_empty()
            || self.held_bytes + data.len() > RECEIVE_LIMIT.saturating_sub(self.received.len())
            || !crate::heap::may_grow_for_user(data.len())
        {
            return false;
        }
        let end = sequence.wrapping_add(data.len() as u32);
        let covered = self.held.iter().any(|(start, bytes)| {
            let held_end = start.wrapping_add(bytes.len() as u32);
            tcp::not_after(*start, sequence) && tcp::not_after(end, held_end)
        });
        if covered {
            self.last_held = Some(sequence);
            return true;
        }
        if self.held.len() >= HELD_RUNS || self.held.try_reserve(1).is_err() {
            return false;
        }
        let mut copy = Vec::new();
        if copy.try_reserve_exact(data.len()).is_err() {
            return false;
        }
        copy.extend_from_slice(data);
        let at = self
            .held
            .iter()
            .position(|(start, _)| tcp::before(sequence, *start))
            .unwrap_or(self.held.len());
        self.held.insert(at, (sequence, copy));
        self.held_bytes += data.len();
        self.last_held = Some(sequence);
        true
    }

    fn take_held(&mut self, next: u32) -> u32 {
        let mut next = next;
        let mut moved = 0u32;
        while let Some(start) = self.held.first().map(|&(start, _)| start) {
            if tcp::before(next, start) {
                break;
            }
            let (_, bytes) = self.held.remove(0);
            self.held_bytes -= bytes.len();
            let end = start.wrapping_add(bytes.len() as u32);
            if tcp::not_after(end, next) {
                continue;
            }
            let skip = next.wrapping_sub(start) as usize;
            let taken = self.received(&bytes[skip..]);
            next = next.wrapping_add(taken as u32);
            moved = moved.wrapping_add(taken as u32);
            if skip + taken < bytes.len() {
                break;
            }
        }
        if self.held.is_empty() {
            self.held = Vec::new();
            self.last_held = None;
        }
        moved
    }

    fn held_ranges(&self, out: &mut [(u32, u32)]) -> usize {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (start, bytes) in &self.held {
            let end = start.wrapping_add(bytes.len() as u32);
            match runs.last_mut() {
                Some(last) if tcp::not_after(*start, last.1) => {
                    if tcp::before(last.1, end) {
                        last.1 = end;
                    }
                }
                _ => {
                    if runs.try_reserve(1).is_err() {
                        break;
                    }
                    runs.push((*start, end));
                }
            }
        }
        let recent = self.last_held.and_then(|sequence| {
            runs.iter().position(|&(start, end)| {
                tcp::not_after(start, sequence) && tcp::before(sequence, end)
            })
        });
        let mut written = 0;
        if let Some(index) = recent
            && let Some(slot) = out.first_mut()
        {
            *slot = runs[index];
            written = 1;
        }
        for (index, &run) in runs.iter().enumerate() {
            if Some(index) == recent {
                continue;
            }
            let Some(slot) = out.get_mut(written) else {
                break;
            };
            *slot = run;
            written += 1;
        }
        written
    }
}


impl Queues {
    fn drop_received(&mut self) {
        self.received.clear();
        self.held = Vec::new();
        self.held_bytes = 0;
        self.last_held = None;
    }
}

fn copy_from(queue: &VecDeque<u8>, offset: usize, out: &mut [u8]) -> usize {
    let (first, second) = queue.as_slices();
    let mut copied = 0;
    let mut skip = offset;
    for part in [first, second] {
        if skip >= part.len() {
            skip -= part.len();
            continue;
        }
        let available = &part[skip..];
        skip = 0;
        let count = available.len().min(out.len() - copied);
        out[copied..copied + count].copy_from_slice(&available[..count]);
        copied += count;
        if copied == out.len() {
            break;
        }
    }
    copied
}

#[derive(Debug)]
struct StreamState {
    connection: Connection,
    queues: Queues,
    listener: Option<Weak<Listener>>,
    read_closed: bool,
    released: bool,
    error_reported: bool,
    segmentation_decided: bool,
    keep_alive: KeepAlive,
}

/// One connection with its bytes.
#[derive(Debug)]
pub struct Stream {
    state: LeafSpinLock<StreamState>,
}

/// What a program sees of a stream at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStatus {
    /// The connection's state.
    pub state: State,
    /// Why it ended abnormally, until a program was told.
    pub error: Option<Error>,
    /// Bytes waiting to be read.
    pub readable: usize,
    /// Whether reading is over: the peer closed, the connection ended or reading was shut down.
    pub read_ended: bool,
    /// Bytes a send can queue now.
    pub send_room: usize,
    /// Whether urgent data waits (`POLLPRI`, D564).
    pub urgent: bool,
}

impl Stream {
    fn new(mut connection: Connection, listener: Option<Weak<Listener>>) -> Arc<Self> {
        connection.set_checksum(Completion::Partial);
        connection.set_extensions(Extensions {
            window_scale: Some(RECEIVE_WINDOW_SCALE),
            selective_acknowledgments: true,
        });
        Arc::new(Self {
            state: LeafSpinLock::new_leaf(StreamState {
                connection,
                queues: Queues::default(),
                listener,
                read_closed: false,
                released: false,
                error_reported: false,
                segmentation_decided: false,
                keep_alive: KeepAlive::LINUX,
            }),
        })
    }

    /// Starts a connection from `local` to `remote` and puts it in the table: its SYN goes
    /// at the next [`run`]. `None` when the reserve leaves programs no memory for it.
    pub fn connect<P: Platform>(
        local: (Ipv4Address, u16),
        remote: (Ipv4Address, u16),
    ) -> Option<Arc<Self>> {
        if !crate::heap::may_grow_for_user(core::mem::size_of::<Self>()) {
            return None;
        }
        let mss = match is_local(remote.0) {
            true => MSS,
            false => crate::net::tcp_mss().min(MSS),
        };
        let connection = Connection::connect(local, remote, initial_sequence::<P>(), mss);
        let stream = Self::new(connection, None);
        register::<P>(Arc::clone(&stream)).then_some(stream)
    }

    /// The connection's own address and port.
    pub fn local(&self) -> (Ipv4Address, u16) {
        self.state.lock().connection.local()
    }

    /// The peer's address and port.
    pub fn remote(&self) -> (Ipv4Address, u16) {
        self.state.lock().connection.remote()
    }

    /// What a program sees of it now.
    pub fn status(&self) -> StreamStatus {
        let state = self.state.lock();
        let connection = &state.connection;
        let room = match connection.may_send() {
            true => SEND_LIMIT.saturating_sub(state.queues.send.len()),
            false => 0,
        };
        StreamStatus {
            state: connection.state(),
            error: connection.error().filter(|_| !state.error_reported),
            readable: state.queues.received.len(),
            read_ended: state.read_closed
                || connection.fin_received()
                || connection.state() == State::Closed,
            send_room: room,
            urgent: connection.has_urgent(),
        }
    }

    /// Turns keep-alive probes on or off (`SO_KEEPALIVE`, D564) with the times last set.
    pub fn set_keep_alive<P: Platform>(&self, on: bool) {
        let mut state = self.state.lock();
        let settings = state.keep_alive;
        state
            .connection
            .set_keep_alive(on.then_some(settings), now_ms::<P>());
    }

    /// Whether keep-alive probes are on.
    pub fn keeps_alive(&self) -> bool {
        self.state.lock().connection.keep_alive().is_some()
    }

    /// The keep-alive times (`TCP_KEEPIDLE`, `TCP_KEEPINTVL`, `TCP_KEEPCNT`).
    pub fn keep_alive_settings(&self) -> KeepAlive {
        self.state.lock().keep_alive
    }

    /// Changes the keep-alive times; probes that are on follow them from now.
    pub fn set_keep_alive_settings<P: Platform>(&self, settings: KeepAlive) {
        let mut state = self.state.lock();
        state.keep_alive = settings;
        if state.connection.keep_alive().is_some() {
            state
                .connection
                .set_keep_alive(Some(settings), now_ms::<P>());
        }
    }

    /// Marks what is queued to send as urgent (`send(MSG_OOB)`).
    pub fn mark_urgent(&self) {
        let mut state = self.state.lock();
        let StreamState {
            connection, queues, ..
        } = &mut *state;
        connection.mark_urgent(queues);
    }

    /// The urgent byte received apart from the stream, taken unless `peek` (`recv(MSG_OOB)`).
    pub fn take_urgent_byte(&self, peek: bool) -> Option<u8> {
        self.state.lock().connection.take_urgent_byte(peek)
    }

    /// Keeps urgent bytes in the stream (`SO_OOBINLINE`).
    pub fn set_urgent_inline(&self, inline: bool) {
        self.state.lock().connection.set_urgent_inline(inline);
    }

    /// The error the connection ended with, once: a program is told only the first time.
    pub fn take_error(&self) -> Option<Error> {
        let mut state = self.state.lock();
        let error = state.connection.error().filter(|_| !state.error_reported);
        if error.is_some() {
            state.error_reported = true;
        }
        error
    }

    /// Queues the start of `data` to send: how many bytes it took (0 when the queue is full,
    /// the connection can no longer send, or the reserve leaves programs no memory).
    pub fn write(&self, data: &[u8]) -> usize {
        let mut state = self.state.lock();
        if !state.connection.may_send() {
            return 0;
        }
        let count = data
            .len()
            .min(SEND_LIMIT.saturating_sub(state.queues.send.len()));
        if count == 0
            || !crate::heap::may_grow_for_user(count)
            || state.queues.send.try_reserve(count).is_err()
        {
            return 0;
        }
        state.queues.send.extend(&data[..count]);
        count
    }

    /// Copies received bytes from `skip` on into `out`, taking them unless `peek`: how many.
    pub fn read(&self, out: &mut [u8], skip: usize, peek: bool) -> usize {
        let mut state = self.state.lock();
        let copied = copy_from(&state.queues.received, skip, out);
        if !peek {
            state.queues.received.drain(..copied);
            if state.queues.received.is_empty() {
                state.queues.received.shrink_to_fit();
            }
        }
        copied
    }

    /// Stops sending: a FIN follows what is queued.
    pub fn shut_down_sending(&self) {
        self.state.lock().connection.close();
    }

    /// Stops receiving: reads end at once, and what comes later is dropped.
    pub fn shut_down_receiving(&self) {
        let mut state = self.state.lock();
        state.read_closed = true;
        state.queues.drop_received();
    }

    fn release(&self) {
        let mut state = self.state.lock();
        state.released = true;
        match state.queues.received.is_empty() {
            true => state.connection.close(),
            false => {
                state.queues.drop_received();
                state.connection.abort();
            }
        }
    }

    fn abort(&self) {
        self.state.lock().connection.abort();
    }
}

#[derive(Debug)]
struct ListenerState {
    backlog: usize,
    pending: usize,
    ready: VecDeque<Arc<Stream>>,
    closed: bool,
}

/// A listening socket's side: the connections being made to its port, and those made and
/// waiting for `accept`.
#[derive(Debug)]
pub struct Listener {
    local: (Ipv4Address, u16),
    state: LeafSpinLock<ListenerState>,
}

impl Listener {
    /// Listens on `local` (address `0.0.0.0` for any), with at most `backlog` connections
    /// waiting at once (being made or made). `None` when the reserve leaves programs no
    /// memory for it.
    pub fn listen(local: (Ipv4Address, u16), backlog: usize) -> Option<Arc<Self>> {
        if !crate::heap::may_grow_for_user(core::mem::size_of::<Self>()) {
            return None;
        }
        let listener = Arc::new(Self {
            local,
            state: LeafSpinLock::new_leaf(ListenerState {
                backlog: backlog.max(1),
                pending: 0,
                ready: VecDeque::new(),
                closed: false,
            }),
        });
        let mut table = TABLE.lock();
        let table = table_of(&mut table)?;
        table.listeners.try_reserve(1).ok()?;
        table
            .listeners
            .retain(|listener| listener.strong_count() > 0);
        table.listeners.push(Arc::downgrade(&listener));
        Some(listener)
    }

    /// Where it listens.
    pub fn local(&self) -> (Ipv4Address, u16) {
        self.local
    }

    /// Sets how many connections may wait at once.
    pub fn set_backlog(&self, backlog: usize) {
        self.state.lock().backlog = backlog.max(1);
    }

    /// Whether a connection waits for `accept`.
    pub fn has_ready(&self) -> bool {
        !self.state.lock().ready.is_empty()
    }

    /// The first connection waiting for `accept`, taken.
    pub fn take_ready(&self) -> Option<Arc<Stream>> {
        self.state.lock().ready.pop_front()
    }

    fn takes(&self, address: Ipv4Address, port: u16) -> bool {
        port == self.local.1 && (self.local.0.is_unspecified() || self.local.0 == address)
    }

    fn has_room(&self) -> bool {
        let state = self.state.lock();
        !state.closed && state.pending + state.ready.len() < state.backlog
    }

    fn close(self: &Arc<Self>) {
        let ready = {
            let mut state = self.state.lock();
            state.closed = true;
            core::mem::take(&mut state.ready)
        };
        for stream in ready {
            stream.abort();
        }
        let mut index = 0;
        while let Some(stream) = stream_at(index) {
            index += 1;
            let mut state = stream.state.lock();
            let ours = state
                .listener
                .as_ref()
                .is_some_and(|listener| core::ptr::eq(listener.as_ptr(), Arc::as_ptr(self)));
            if ours {
                state.listener = None;
                state.released = true;
                state.connection.abort();
            }
        }
    }
}

/// What an `AF_INET` stream socket is: new or bound, listening, or a connection.
#[derive(Debug, Clone)]
pub enum Phase {
    /// Neither connected nor listening.
    Idle,
    /// Listening.
    Listening(Arc<Listener>),
    /// Connecting or connected.
    Connected(Arc<Stream>),
}

/// The TCP side of an `AF_INET` stream socket: what it is, and the function that runs the
/// connections when it lets go of them.
#[derive(Debug)]
pub struct Socket {
    phase: LeafSpinLock<Phase>,
    run: fn(),
    /// `SO_LINGER` (D559): `None` when off, else the seconds `close` lingers; 0 resets the
    /// connection at close instead of closing it.
    linger: LeafSpinLock<Option<u32>>,
    /// `SO_KEEPALIVE` and `SO_OOBINLINE` as set (D564): given to the connection it makes, or
    /// to the ones a listener accepts.
    keep_alive: core::sync::atomic::AtomicBool,
    urgent_inline: core::sync::atomic::AtomicBool,
}

impl Socket {
    /// A new socket, neither connected nor listening; `run` is [`run`] for the platform.
    pub fn new(run: fn()) -> Self {
        Self {
            phase: LeafSpinLock::new_leaf(Phase::Idle),
            linger: LeafSpinLock::new_leaf(None),
            keep_alive: core::sync::atomic::AtomicBool::new(false),
            urgent_inline: core::sync::atomic::AtomicBool::new(false),
            run,
        }
    }

    /// A socket for `stream`, a connection a listener made.
    pub fn accepted(stream: Arc<Stream>, run: fn()) -> Self {
        Self {
            phase: LeafSpinLock::new_leaf(Phase::Connected(stream)),
            linger: LeafSpinLock::new_leaf(None),
            keep_alive: core::sync::atomic::AtomicBool::new(false),
            urgent_inline: core::sync::atomic::AtomicBool::new(false),
            run,
        }
    }

    /// What it is now.
    pub fn phase(&self) -> Phase {
        self.phase.lock().clone()
    }

    /// Makes it `phase` if it is still idle: whether it was.
    pub fn leave_idle(&self, phase: Phase) -> bool {
        let mut current = self.phase.lock();
        match *current {
            Phase::Idle => {
                *current = phase;
                true
            }
            _ => false,
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let phase = core::mem::replace(&mut *self.phase.lock(), Phase::Idle);
        let reset = *self.linger.lock() == Some(0);
        match phase {
            Phase::Idle => return,
            Phase::Listening(listener) => listener.close(),
            Phase::Connected(stream) if reset => {
                stream.abort();
                stream.release();
            }
            Phase::Connected(stream) => stream.release(),
        }
        (self.run)();
    }
}

impl Socket {
    /// Sets `SO_LINGER` (D559).
    pub fn set_linger(&self, linger: Option<u32>) {
        *self.linger.lock() = linger;
    }

    /// `SO_LINGER` as set.
    pub fn linger(&self) -> Option<u32> {
        *self.linger.lock()
    }

    /// Sets `SO_KEEPALIVE` (D564), for the connection now and the ones to come.
    pub fn set_keep_alive<P: Platform>(&self, on: bool) {
        self.keep_alive
            .store(on, core::sync::atomic::Ordering::Relaxed);
        if let Phase::Connected(stream) = self.phase() {
            stream.set_keep_alive::<P>(on);
        }
    }

    /// Sets `SO_OOBINLINE` (D564), for the connection now and the ones to come.
    pub fn set_urgent_inline(&self, inline: bool) {
        self.urgent_inline
            .store(inline, core::sync::atomic::Ordering::Relaxed);
        if let Phase::Connected(stream) = self.phase() {
            stream.set_urgent_inline(inline);
        }
    }

    /// Gives `stream`, a connection it made or accepted, its `SO_KEEPALIVE` and
    /// `SO_OOBINLINE`.
    pub fn hand_options_to<P: Platform>(&self, stream: &Stream) {
        if self.keep_alive.load(core::sync::atomic::Ordering::Relaxed) {
            stream.set_keep_alive::<P>(true);
        }
        if self.urgent_inline.load(core::sync::atomic::Ordering::Relaxed) {
            stream.set_urgent_inline(true);
        }
    }
}

#[derive(Debug)]
struct Outgoing {
    source: Ipv4Address,
    destination: Ipv4Address,
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct Table {
    streams: Vec<Arc<Stream>>,
    listeners: Vec<Weak<Listener>>,
    outbox: VecDeque<Outgoing>,
}

static TABLE: LeafSpinLock<Option<Box<Table>>> = LeafSpinLock::new_leaf(None);

fn table_of(slot: &mut Option<Box<Table>>) -> Option<&mut Table> {
    if slot.is_none() {
        *slot = Some(Box::new(Table::default()));
    }
    slot.as_deref_mut()
}

fn register<P: Platform>(stream: Arc<Stream>) -> bool {
    {
        let mut table = TABLE.lock();
        let Some(table) = table_of(&mut table) else {
            return false;
        };
        if table.streams.try_reserve(1).is_err() {
            return false;
        }
        table.streams.push(stream);
    }
    crate::net::DEVICE_EVENTS.raise::<P>();
    true
}

fn stream_at(index: usize) -> Option<Arc<Stream>> {
    TABLE
        .lock()
        .as_ref()
        .and_then(|table| table.streams.get(index).cloned())
}

/// Whether the kernel runs any connection.
pub fn has_streams() -> bool {
    TABLE
        .lock()
        .as_ref()
        .is_some_and(|table| !table.streams.is_empty() || !table.outbox.is_empty())
}

/// How many connections the kernel runs, `TIME-WAIT` included.
pub fn stream_count() -> usize {
    TABLE.lock().as_ref().map_or(0, |table| table.streams.len())
}

fn initial_sequence<P: Platform>() -> u32 {
    let mut bytes = [0u8; 4];
    crate::random::fill::<P>(&mut bytes);
    u32::from_ne_bytes(bytes)
}

fn find_stream(local: (Ipv4Address, u16), remote: (Ipv4Address, u16)) -> Option<Arc<Stream>> {
    let table = TABLE.lock();
    table.as_ref()?.streams.iter().find_map(|stream| {
        let state = stream.state.lock();
        let connection = &state.connection;
        (connection.local() == local && connection.remote() == remote).then(|| Arc::clone(stream))
    })
}

fn find_listener(address: Ipv4Address, port: u16) -> Option<Arc<Listener>> {
    let table = TABLE.lock();
    table
        .as_ref()?
        .listeners
        .iter()
        .filter_map(Weak::upgrade)
        .find(|listener| listener.takes(address, port))
}

fn queue_outgoing(source: Ipv4Address, destination: Ipv4Address, bytes: &[u8]) {
    let mut copy = Vec::new();
    if copy.try_reserve_exact(bytes.len()).is_err() {
        return;
    }
    copy.extend_from_slice(bytes);
    let mut table = TABLE.lock();
    let Some(table) = table_of(&mut table) else {
        return;
    };
    if table.outbox.len() < OUTBOX_SEGMENTS && table.outbox.try_reserve(1).is_ok() {
        table.outbox.push_back(Outgoing {
            source,
            destination,
            bytes: copy,
        });
    }
}

fn take_outgoing() -> Option<Outgoing> {
    TABLE.lock().as_mut()?.outbox.pop_front()
}

/// Takes in `segment`, which came from `source` to `destination`, this machine, over a link
/// that carries segments of at most `link_mss` bytes: hands it to its connection, or makes a
/// connection for a listener's SYN, or answers it with a reset. Whether a connection took it.
/// It takes no lock of the adapter, whose receive path calls it.
pub fn deliver<P: Platform>(
    source: Ipv4Address,
    destination: Ipv4Address,
    segment: &Segment<'_>,
    link_mss: u16,
) -> bool {
    let now = now_ms::<P>();
    let local = (destination, segment.header.destination_port);
    let remote = (source, segment.header.source_port);
    if let Some(stream) = find_stream(local, remote) {
        let established = {
            let mut state = stream.state.lock();
            let StreamState {
                connection,
                queues,
                read_closed,
                released,
                listener,
                ..
            } = &mut *state;
            let was_connecting = connection.is_connecting();
            connection.receive(segment, now, queues);
            if *read_closed || *released {
                queues.drop_received();
            }
            let made = was_connecting && !connection.is_connecting();
            match (made, connection.state()) {
                (true, State::Closed) => None,
                (true, _) => listener.take(),
                (false, _) => None,
            }
        };
        if let Some(listener) = established.and_then(|listener| listener.upgrade()) {
            let mut state = listener.state.lock();
            state.pending = state.pending.saturating_sub(1);
            match state.closed || state.ready.try_reserve(1).is_err() {
                true => {
                    drop(state);
                    stream.abort();
                }
                false => state.ready.push_back(stream),
            }
        }
        return true;
    }
    let syn = segment.has(flags::SYN) && !segment.has(flags::ACK) && !segment.has(flags::RST);
    if syn && let Some(listener) = find_listener(destination, local.1) {
        return accept_syn::<P>(&listener, local, remote, segment, link_mss);
    }
    let mut reset = [0u8; tcp::HEADER_BYTES];
    if let Some(length) = tcp::write_reset_for(&mut reset, source, destination, segment) {
        queue_outgoing(destination, source, &reset[..length]);
    }
    false
}

fn accept_syn<P: Platform>(
    listener: &Arc<Listener>,
    local: (Ipv4Address, u16),
    remote: (Ipv4Address, u16),
    segment: &Segment<'_>,
    link_mss: u16,
) -> bool {
    if !listener.has_room() || !crate::heap::may_grow_for_user(core::mem::size_of::<Stream>()) {
        return false;
    }
    let mss = link_mss.min(MSS);
    let Some(connection) = Connection::accept(local, remote, segment, initial_sequence::<P>(), mss)
    else {
        return false;
    };
    let stream = Stream::new(connection, Some(Arc::downgrade(listener)));
    if !register::<P>(stream) {
        return false;
    }
    listener.state.lock().pending += 1;
    true
}

fn send_segment<P: Platform>(
    source: Ipv4Address,
    destination: Ipv4Address,
    bytes: &mut [u8],
    segment_size: u16,
) -> bool {
    if is_local(destination) {
        checksum::complete(bytes, tcp::CHECKSUM_AT);
        queue_outgoing(source, destination, bytes);
        return true;
    }
    let ttl = ipv4::DEFAULT_TTL;
    if bytes.len() > tcp::MAX_HEADER_BYTES + usize::from(segment_size) {
        let sent =
            super::send_large_tcp::<P>(destination, bytes, Completion::Partial, segment_size, ttl);
        return sent != Err(SendError::TooLong);
    }
    let _ = super::send_tcp::<P>(destination, bytes, Completion::Partial, ttl);
    true
}

fn sweep() -> bool {
    let finished = {
        let mut table = TABLE.lock();
        let Some(table) = table.as_deref_mut() else {
            return false;
        };
        let mut finished = Vec::new();
        let mut index = 0;
        while index < table.streams.len() {
            let done = table.streams[index].state.lock().connection.is_finished();
            if done && finished.try_reserve(1).is_ok() {
                finished.push(table.streams.swap_remove(index));
            } else {
                index += 1;
            }
        }
        finished
    };
    let swept = !finished.is_empty();
    for stream in finished {
        let listener = stream.state.lock().listener.take();
        if let Some(listener) = listener.and_then(|listener| listener.upgrade()) {
            let mut state = listener.state.lock();
            state.pending = state.pending.saturating_sub(1);
        }
    }
    swept
}

static RUNNING: SpinLock<()> = SpinLock::new(());

/// Runs every connection at this moment: its timers, then what it has to send, delivering
/// what goes to this machine at once (and sending the resets [`deliver`] queued), until the
/// connections have nothing more to say; then drops the connections that are over and, when
/// anything changed, wakes the programs that wait on sockets. One CPU runs them at a time, so
/// segments to this machine arrive in the order they were sent.
pub fn run<P: Platform>() {
    if !has_streams() {
        return;
    }
    let running = RUNNING.lock();
    let changed = run_once::<P>();
    drop(running);
    if changed {
        crate::console::wake_input_waiters::<P>();
    }
}

fn take_segment_scratch() -> Option<Vec<u8>> {
    let mut scratch = core::mem::take(&mut *SEGMENT_SCRATCH.lock());
    for wanted in [LARGE_SEGMENT_BYTES, SEGMENT_BYTES] {
        if scratch.len() >= wanted {
            return Some(scratch);
        }
        if scratch.try_reserve_exact(wanted - scratch.len()).is_ok() {
            scratch.resize(wanted, 0);
            return Some(scratch);
        }
    }
    None
}

fn give_back_segment_scratch(scratch: Vec<u8>) {
    let mut kept = SEGMENT_SCRATCH.lock();
    if kept.len() < scratch.len() {
        *kept = scratch;
    }
}

fn decide_segmentation(stream: &Stream) {
    let remote = {
        let state = stream.state.lock();
        match state.segmentation_decided {
            true => return,
            false => state.connection.remote().0,
        }
    };
    let limit = match is_local(remote) {
        true => None,
        false => super::segmentation_data_limit(remote),
    };
    let mut state = stream.state.lock();
    state.connection.set_segment_limit(limit);
    state.segmentation_decided = true;
}

fn run_once<P: Platform>() -> bool {
    let now = now_ms::<P>();
    let Some(mut segment) = take_segment_scratch() else {
        return false;
    };
    let mut changed = false;
    for _ in 0..LOOPBACK_ROUNDS {
        let mut index = 0;
        while let Some(stream) = stream_at(index) {
            index += 1;
            changed |= stream.state.lock().connection.poll(now);
            decide_segmentation(&stream);
            for _ in 0..SEGMENTS_PER_RUN {
                let written = {
                    let mut state = stream.state.lock();
                    let StreamState {
                        connection, queues, ..
                    } = &mut *state;
                    let room = segment
                        .len()
                        .min(tcp::MAX_HEADER_BYTES + connection.segment_data_bytes());
                    connection
                        .output(now, queues, &mut segment[..room])
                        .map(|length| {
                            let ends = (connection.local().0, connection.remote().0);
                            (length, ends, connection.send_mss())
                        })
                };
                let Some((length, (source, destination), segment_size)) = written else {
                    break;
                };
                let bytes = &mut segment[..length];
                if !send_segment::<P>(source, destination, bytes, segment_size) {
                    let mut state = stream.state.lock();
                    state.connection.set_segment_limit(None);
                }
            }
        }
        let mut delivered = false;
        for _ in 0..OUTBOX_SEGMENTS {
            let Some(outgoing) = take_outgoing() else {
                break;
            };
            if !is_local(outgoing.destination) {
                let (destination, bytes) = (outgoing.destination, &outgoing.bytes);
                let _ =
                    super::send_tcp::<P>(destination, bytes, Completion::Full, ipv4::DEFAULT_TTL);
                continue;
            }
            if let Some(arrived) =
                tcp::parse(outgoing.source, outgoing.destination, &outgoing.bytes)
            {
                deliver::<P>(outgoing.source, outgoing.destination, &arrived, MSS);
                delivered = true;
            }
        }
        changed |= delivered;
        if !delivered {
            break;
        }
    }
    give_back_segment_scratch(segment);
    changed | sweep()
}

#[cfg(test)]
mod tests;

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
use eggos_net::ipv4::{self, Ipv4Address};
use eggos_net::tcp::{self, Buffers, Connection, Error, Segment, State, flags};

/// The segment size connections ask for and send at most: an Ethernet link's.
pub const MSS: u16 = 1460;
/// The bytes a connection queues for sending before a program's send waits, as Linux's
/// default `tcp_wmem`'s ceiling for a socket without `SO_SNDBUF`, kept to what one window holds.
pub const SEND_LIMIT: usize = 64 * 1024;
/// The bytes a connection keeps received before its window closes: the largest window a
/// header carries without window scaling.
pub const RECEIVE_LIMIT: usize = tcp::MAX_WINDOW;
/// The most segments one connection sends in one [`run`].
pub const SEGMENTS_PER_RUN: usize = 64;
/// The most rounds of loopback delivery one [`run`] makes.
pub const LOOPBACK_ROUNDS: usize = 16;
/// The most segments the outbox holds (the loopback's and the resets that answer segments no
/// connection owns); more are dropped, as a link drops them.
pub const OUTBOX_SEGMENTS: usize = 1024;
/// The room a segment takes while it is written.
pub const SEGMENT_BYTES: usize = tcp::MAX_HEADER_BYTES + MSS as usize;

#[derive(Debug, Default)]
struct Queues {
    send: VecDeque<u8>,
    received: VecDeque<u8>,
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
        RECEIVE_LIMIT.saturating_sub(self.received.len())
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
}

impl Stream {
    fn new(connection: Connection, listener: Option<Weak<Listener>>) -> Arc<Self> {
        Arc::new(Self {
            state: LeafSpinLock::new_leaf(StreamState {
                connection,
                queues: Queues::default(),
                listener,
                read_closed: false,
                released: false,
                error_reported: false,
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
        register(Arc::clone(&stream)).then_some(stream)
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
        }
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
        state.queues.received.clear();
    }

    fn release(&self) {
        let mut state = self.state.lock();
        state.released = true;
        match state.queues.received.is_empty() {
            true => state.connection.close(),
            false => {
                state.queues.received.clear();
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
}

impl Socket {
    /// A new socket, neither connected nor listening; `run` is [`run`] for the platform.
    pub fn new(run: fn()) -> Self {
        Self {
            phase: LeafSpinLock::new_leaf(Phase::Idle),
            run,
        }
    }

    /// A socket for `stream`, a connection a listener made.
    pub fn accepted(stream: Arc<Stream>, run: fn()) -> Self {
        Self {
            phase: LeafSpinLock::new_leaf(Phase::Connected(stream)),
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
        match phase {
            Phase::Idle => return,
            Phase::Listening(listener) => listener.close(),
            Phase::Connected(stream) => stream.release(),
        }
        (self.run)();
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

fn register(stream: Arc<Stream>) -> bool {
    let mut table = TABLE.lock();
    let Some(table) = table_of(&mut table) else {
        return false;
    };
    if table.streams.try_reserve(1).is_err() {
        return false;
    }
    table.streams.push(stream);
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
                queues.received.clear();
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
    if !register(stream) {
        return false;
    }
    listener.state.lock().pending += 1;
    true
}

fn send_segment<P: Platform>(source: Ipv4Address, destination: Ipv4Address, bytes: &[u8]) {
    match is_local(destination) {
        true => queue_outgoing(source, destination, bytes),
        false => {
            let _ = super::send_tcp::<P>(destination, bytes, ipv4::DEFAULT_TTL);
        }
    }
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

fn run_once<P: Platform>() -> bool {
    let now = now_ms::<P>();
    let mut segment = [0u8; SEGMENT_BYTES];
    let mut changed = false;
    for _ in 0..LOOPBACK_ROUNDS {
        let mut index = 0;
        while let Some(stream) = stream_at(index) {
            index += 1;
            changed |= stream.state.lock().connection.poll(now);
            for _ in 0..SEGMENTS_PER_RUN {
                let written = {
                    let mut state = stream.state.lock();
                    let StreamState {
                        connection, queues, ..
                    } = &mut *state;
                    connection
                        .output(now, queues, &mut segment)
                        .map(|length| (length, connection.local().0, connection.remote().0))
                };
                let Some((length, source, destination)) = written else {
                    break;
                };
                send_segment::<P>(source, destination, &segment[..length]);
            }
        }
        let mut delivered = false;
        for _ in 0..OUTBOX_SEGMENTS {
            let Some(outgoing) = take_outgoing() else {
                break;
            };
            if !is_local(outgoing.destination) {
                let _ =
                    super::send_tcp::<P>(outgoing.destination, &outgoing.bytes, ipv4::DEFAULT_TTL);
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
    changed | sweep()
}

#[cfg(test)]
mod tests;

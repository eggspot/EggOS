//! TCP (RFC 9293): the segment format with its checksum over the IPv4 pseudo-header, and one
//! connection's state machine ([`Connection`]): the three-way handshake both ways, the maximum
//! segment size option, sending within the peer's window, retransmission on a timer whose
//! timeout follows the measured round trip (RFC 6298, with Karn's rule and exponential
//! backoff), fast retransmission after three duplicate acknowledgments (RFC 5681), probing a
//! closed window, the receiver's window and its updates, orderly close by FIN both ways,
//! `TIME-WAIT`, and resets: sent for an abort or a segment no connection owns, honoured when
//! in sequence, and answered with a challenge acknowledgment otherwise (RFC 5961).
//!
//! A connection holds no data and no clock: its caller keeps the bytes waiting to be sent and
//! the bytes received ([`Buffers`]), passes each segment that comes for it to
//! [`Connection::receive`], calls [`Connection::poll`] by [`Connection::next_poll_ms`], and
//! sends each segment [`Connection::output`] writes. Segments that arrive out of order are
//! dropped and acknowledged with the next byte expected, so the sender resends them; there is
//! no window scaling, so a window is at most [`MAX_WINDOW`] bytes.

use crate::checksum;
use crate::ipv4::{Ipv4Address, protocol};

/// Bytes of a header without options.
pub const HEADER_BYTES: usize = 20;
/// Bytes of the maximum segment size option.
pub const MSS_OPTION_BYTES: usize = 4;
/// The largest header the connection writes: with the maximum segment size option.
pub const MAX_HEADER_BYTES: usize = HEADER_BYTES + MSS_OPTION_BYTES;
/// The segment size assumed when the peer names none (RFC 9293's default).
pub const DEFAULT_MSS: u16 = 536;
/// The largest window a header carries without window scaling.
pub const MAX_WINDOW: usize = 65_535;
/// The retransmission timeout before a round trip is measured (RFC 6298).
pub const INITIAL_RTO_MS: u64 = 1000;
/// The shortest retransmission timeout, as Linux's.
pub const MIN_RTO_MS: u64 = 200;
/// The longest retransmission timeout, as Linux's.
pub const MAX_RTO_MS: u64 = 120_000;
/// How many times a SYN is sent again before the connection gives up, as Linux's
/// `tcp_syn_retries`.
pub const SYN_RETRIES: u8 = 6;
/// How many times data is sent again before the connection gives up, as Linux's
/// `tcp_retries2`.
pub const DATA_RETRIES: u8 = 15;
/// How long a connection stays in `TIME-WAIT`, as Linux's.
pub const TIME_WAIT_MS: u64 = 60_000;
/// How long a connection waits in `FIN-WAIT-2` for the peer's FIN, as Linux's `tcp_fin_timeout`.
pub const FIN_WAIT_2_MS: u64 = 60_000;
/// How many duplicate acknowledgments start a fast retransmission.
pub const DUPLICATE_ACKS: u8 = 3;

/// The control bits of a segment.
pub mod flags {
    /// No more data from the sender.
    pub const FIN: u8 = 0x01;
    /// Synchronize sequence numbers.
    pub const SYN: u8 = 0x02;
    /// Reset the connection.
    pub const RST: u8 = 0x04;
    /// Push.
    pub const PSH: u8 = 0x08;
    /// The acknowledgment field is significant.
    pub const ACK: u8 = 0x10;
}

const OPTION_END: u8 = 0;
const OPTION_NOP: u8 = 1;
const OPTION_MSS: u8 = 2;

/// A segment's header fields the stack reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    /// The sender's port.
    pub source_port: u16,
    /// The receiver's port.
    pub destination_port: u16,
    /// The sequence number of the first byte (or of the SYN or FIN).
    pub sequence: u32,
    /// The next sequence number the sender expects, when [`flags::ACK`] is set.
    pub acknowledgment: u32,
    /// The control bits ([`flags`]).
    pub flags: u8,
    /// How many bytes the sender takes beyond the acknowledgment.
    pub window: u16,
    /// The maximum segment size option, on a SYN.
    pub mss: Option<u16>,
}

/// A segment that came for the stack: its header and data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment<'a> {
    /// The header.
    pub header: Header,
    /// The data.
    pub data: &'a [u8],
}

impl Segment<'_> {
    /// Whether it carries `flag` ([`flags`]).
    pub fn has(&self, flag: u8) -> bool {
        self.header.flags & flag != 0
    }

    /// How much sequence space it takes: its data, plus one for a SYN and one for a FIN.
    pub fn length(&self) -> u32 {
        self.data.len() as u32 + u32::from(self.has(flags::SYN)) + u32::from(self.has(flags::FIN))
    }
}

fn pseudo_header(source: Ipv4Address, destination: Ipv4Address, length: usize) -> u32 {
    let sum = checksum::add(checksum::add(0, &source.0), &destination.0);
    sum + u32::from(protocol::TCP) + length as u32
}

/// The segment `bytes` carries between `source` and `destination`: `None` when it is shorter
/// than its header or its checksum is wrong.
pub fn parse(source: Ipv4Address, destination: Ipv4Address, bytes: &[u8]) -> Option<Segment<'_>> {
    let fixed = bytes.first_chunk::<HEADER_BYTES>()?;
    let header_bytes = usize::from(fixed[12] >> 4) * 4;
    if header_bytes < HEADER_BYTES || header_bytes > bytes.len() {
        return None;
    }
    if checksum::finish(checksum::add(
        pseudo_header(source, destination, bytes.len()),
        bytes,
    )) != 0
    {
        return None;
    }
    let word =
        |at: usize| u32::from_be_bytes([fixed[at], fixed[at + 1], fixed[at + 2], fixed[at + 3]]);
    let half = |at: usize| u16::from_be_bytes([fixed[at], fixed[at + 1]]);
    Some(Segment {
        header: Header {
            source_port: half(0),
            destination_port: half(2),
            sequence: word(4),
            acknowledgment: word(8),
            flags: fixed[13] & 0x3f,
            window: half(14),
            mss: mss_option(&bytes[HEADER_BYTES..header_bytes]),
        },
        data: &bytes[header_bytes..],
    })
}

fn mss_option(mut options: &[u8]) -> Option<u16> {
    while let Some((&kind, rest)) = options.split_first() {
        match kind {
            OPTION_END => return None,
            OPTION_NOP => options = rest,
            _ => {
                let length = usize::from(*rest.first()?);
                if length < 2 || length > options.len() {
                    return None;
                }
                if kind == OPTION_MSS && length == 4 {
                    return Some(u16::from_be_bytes([options[2], options[3]]));
                }
                options = &options[length..];
            }
        }
    }
    None
}

/// Writes `header` for a segment from `source` to `destination` whose `data_bytes` of data are
/// already at `out[header_bytes(header)..]`, with its checksum: the segment's length, or `None`
/// when `out` is shorter.
pub fn write_header(
    out: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    header: &Header,
    data_bytes: usize,
) -> Option<usize> {
    let header_bytes = header_bytes(header);
    let length = header_bytes.checked_add(data_bytes)?;
    let segment = out.get_mut(..length)?;
    segment[0..2].copy_from_slice(&header.source_port.to_be_bytes());
    segment[2..4].copy_from_slice(&header.destination_port.to_be_bytes());
    segment[4..8].copy_from_slice(&header.sequence.to_be_bytes());
    segment[8..12].copy_from_slice(&header.acknowledgment.to_be_bytes());
    segment[12] = (header_bytes as u8 / 4) << 4;
    segment[13] = header.flags;
    segment[14..16].copy_from_slice(&header.window.to_be_bytes());
    segment[16..20].fill(0);
    if let Some(mss) = header.mss {
        segment[20] = OPTION_MSS;
        segment[21] = MSS_OPTION_BYTES as u8;
        segment[22..24].copy_from_slice(&mss.to_be_bytes());
    }
    let sum = checksum::finish(checksum::add(
        pseudo_header(source, destination, length),
        segment,
    ));
    segment[16..18].copy_from_slice(&sum.to_be_bytes());
    Some(length)
}

/// How many bytes `header` takes written: [`HEADER_BYTES`], plus the option when it has one.
pub fn header_bytes(header: &Header) -> usize {
    match header.mss {
        Some(_) => MAX_HEADER_BYTES,
        None => HEADER_BYTES,
    }
}

/// Writes the reset that answers `segment`, which came from `source` to `destination` and no
/// connection owns (RFC 9293 3.10.7.1), to the start of `out`: its length, or `None` when a
/// reset is never sent for it (it is a reset itself).
pub fn write_reset_for(
    out: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    segment: &Segment<'_>,
) -> Option<usize> {
    if segment.has(flags::RST) {
        return None;
    }
    let header = match segment.has(flags::ACK) {
        true => Header {
            source_port: segment.header.destination_port,
            destination_port: segment.header.source_port,
            sequence: segment.header.acknowledgment,
            flags: flags::RST,
            ..Header::default()
        },
        false => Header {
            source_port: segment.header.destination_port,
            destination_port: segment.header.source_port,
            acknowledgment: segment.header.sequence.wrapping_add(segment.length()),
            flags: flags::RST | flags::ACK,
            ..Header::default()
        },
    };
    write_header(out, destination, source, &header, 0)
}

/// Whether sequence number `a` comes before `b`, modulo 2^32.
pub fn before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// Whether `a` comes before `b` or is `b`.
pub fn not_after(a: u32, b: u32) -> bool {
    !before(b, a)
}

/// Where a connection is (RFC 9293's states; `LISTEN` is its caller's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// It sent a SYN and waits for the answer.
    SynSent,
    /// It answered a SYN and waits for the acknowledgment.
    SynReceived,
    /// Data flows both ways.
    Established,
    /// Its side closed; its FIN waits for its acknowledgment.
    FinWait1,
    /// Its FIN was acknowledged; it waits for the peer's.
    FinWait2,
    /// The peer closed; its side may still send.
    CloseWait,
    /// Both sides closed at once; its FIN waits for its acknowledgment.
    Closing,
    /// The peer closed first, then its side; its FIN waits for its acknowledgment.
    LastAck,
    /// Both sides closed; it waits so that late segments die.
    TimeWait,
    /// It is over.
    Closed,
}

/// Why a connection ended abnormally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The peer answered the SYN with a reset: no one listens there (`ECONNREFUSED`).
    Refused,
    /// The peer reset the connection (`ECONNRESET`).
    Reset,
    /// The peer stopped answering (`ETIMEDOUT`).
    TimedOut,
}

/// The bytes a connection's caller keeps for it.
pub trait Buffers {
    /// How many bytes wait to be sent or acknowledged, the oldest unacknowledged first.
    fn send_len(&self) -> usize;
    /// Copies the bytes from `offset` on of those into `out`: how many.
    fn copy_unacknowledged(&self, offset: usize, out: &mut [u8]) -> usize;
    /// Drops the `count` oldest of them: the peer has them.
    fn acknowledged(&mut self, count: usize);
    /// How many more bytes it takes in.
    fn receive_room(&self) -> usize;
    /// Takes in `data`, which came in order: how many bytes it took.
    fn received(&mut self, data: &[u8]) -> usize;
}

/// One TCP connection.
#[derive(Debug, Clone)]
pub struct Connection {
    local: (Ipv4Address, u16),
    remote: (Ipv4Address, u16),
    state: State,
    error: Option<Error>,
    initial_sequence: u32,
    send_unacknowledged: u32,
    send_next: u32,
    send_highest: u32,
    send_window: u32,
    window_update_sequence: u32,
    window_update_acknowledgment: u32,
    send_mss: u16,
    receive_mss: u16,
    receive_next: u32,
    advertised_edge: u32,
    syn_due: bool,
    ack_due: bool,
    reset_due: bool,
    fin_sent: bool,
    fin_received: bool,
    probe_due: bool,
    duplicate_acks: u8,
    retransmit_at: Option<u64>,
    retries: u8,
    rto_ms: u64,
    smoothed_rtt_ms: Option<u64>,
    rtt_variance_ms: u64,
    timed: Option<(u32, u64)>,
    deadline: Option<u64>,
}

/// What a received segment did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Received {
    /// Bytes the caller's buffers took.
    pub data: usize,
    /// Bytes of its own the peer acknowledged.
    pub acknowledged: usize,
    /// Whether the state changed.
    pub state_changed: bool,
}

impl Connection {
    /// A connection from `local` to `remote` that sends its SYN at the next
    /// [`Self::output`], with initial sequence number `initial_sequence` and a segment size of
    /// at most `mss` for what it receives.
    pub fn connect(
        local: (Ipv4Address, u16),
        remote: (Ipv4Address, u16),
        initial_sequence: u32,
        mss: u16,
    ) -> Self {
        let mut connection = Self::new(local, remote, initial_sequence, mss, State::SynSent);
        connection.syn_due = true;
        connection
    }

    /// The connection a listener on `local` makes for `syn`, a segment with SYN and without
    /// ACK from `remote`: it answers with its SYN at the next [`Self::output`]. `None` for
    /// another segment.
    pub fn accept(
        local: (Ipv4Address, u16),
        remote: (Ipv4Address, u16),
        syn: &Segment<'_>,
        initial_sequence: u32,
        mss: u16,
    ) -> Option<Self> {
        if !syn.has(flags::SYN) || syn.has(flags::ACK) || syn.has(flags::RST) {
            return None;
        }
        let mut connection = Self::new(local, remote, initial_sequence, mss, State::SynReceived);
        connection.receive_next = syn.header.sequence.wrapping_add(1);
        connection.advertised_edge = connection.receive_next;
        connection.send_mss = syn.header.mss.unwrap_or(DEFAULT_MSS).min(mss).max(1);
        connection.send_window = u32::from(syn.header.window);
        connection.window_update_sequence = syn.header.sequence;
        connection.syn_due = true;
        Some(connection)
    }

    fn new(
        local: (Ipv4Address, u16),
        remote: (Ipv4Address, u16),
        initial_sequence: u32,
        mss: u16,
        state: State,
    ) -> Self {
        Self {
            local,
            remote,
            state,
            error: None,
            initial_sequence,
            send_unacknowledged: initial_sequence,
            send_next: initial_sequence,
            send_highest: initial_sequence,
            send_window: 0,
            window_update_sequence: 0,
            window_update_acknowledgment: 0,
            send_mss: DEFAULT_MSS.min(mss).max(1),
            receive_mss: mss.max(1),
            receive_next: 0,
            advertised_edge: 0,
            syn_due: false,
            ack_due: false,
            reset_due: false,
            fin_sent: false,
            fin_received: false,
            probe_due: false,
            duplicate_acks: 0,
            retransmit_at: None,
            retries: 0,
            rto_ms: INITIAL_RTO_MS,
            smoothed_rtt_ms: None,
            rtt_variance_ms: 0,
            timed: None,
            deadline: None,
        }
    }

    /// Its own address and port.
    pub fn local(&self) -> (Ipv4Address, u16) {
        self.local
    }

    /// The peer's address and port.
    pub fn remote(&self) -> (Ipv4Address, u16) {
        self.remote
    }

    /// Where it is.
    pub fn state(&self) -> State {
        self.state
    }

    /// Why it ended, if it ended abnormally.
    pub fn error(&self) -> Option<Error> {
        self.error
    }

    /// Whether the handshake is still going on.
    pub fn is_connecting(&self) -> bool {
        matches!(self.state, State::SynSent | State::SynReceived)
    }

    /// Whether the peer closed its side: everything it sent has been taken in.
    pub fn fin_received(&self) -> bool {
        self.fin_received
    }

    /// Whether its side may still queue data to send.
    pub fn may_send(&self) -> bool {
        matches!(self.state, State::Established | State::CloseWait)
    }

    /// Whether it is over and has nothing left to send.
    pub fn is_finished(&self) -> bool {
        self.state == State::Closed && !self.reset_due
    }

    /// The largest segment it sends.
    pub fn send_mss(&self) -> u16 {
        self.send_mss
    }

    /// The current retransmission timeout.
    pub fn rto_ms(&self) -> u64 {
        self.rto_ms
    }

    /// Closes its side: a FIN follows the data still queued. Does nothing once it closed.
    pub fn close(&mut self) {
        self.state = match self.state {
            State::SynSent => State::Closed,
            State::SynReceived | State::Established => State::FinWait1,
            State::CloseWait => State::LastAck,
            other => other,
        };
        if self.state == State::Closed {
            self.retransmit_at = None;
        }
    }

    /// Ends it at once: a reset goes to the peer (unless the handshake never started there)
    /// and it is closed.
    pub fn abort(&mut self) {
        self.reset_due = !matches!(self.state, State::SynSent | State::Closed | State::TimeWait);
        self.enter_closed(None);
    }

    fn enter_closed(&mut self, error: Option<Error>) {
        self.state = State::Closed;
        self.error = self.error.or(error);
        self.retransmit_at = None;
        self.deadline = None;
        self.syn_due = false;
        self.ack_due = false;
        self.probe_due = false;
    }

    fn enter_time_wait(&mut self, now_ms: u64) {
        self.state = State::TimeWait;
        self.retransmit_at = None;
        self.deadline = Some(now_ms.saturating_add(TIME_WAIT_MS));
    }

    /// When it next wants [`Self::poll`] called, in milliseconds; `u64::MAX` for never.
    pub fn next_poll_ms(&self) -> u64 {
        self.retransmit_at
            .unwrap_or(u64::MAX)
            .min(self.deadline.unwrap_or(u64::MAX))
    }

    /// Acts on the time at `now_ms`: sends again what was not acknowledged in time, gives up
    /// after too many tries, and ends `TIME-WAIT` and `FIN-WAIT-2`. Whether the state changed.
    pub fn poll(&mut self, now_ms: u64) -> bool {
        if let Some(deadline) = self.deadline
            && now_ms >= deadline
            && matches!(self.state, State::TimeWait | State::FinWait2)
        {
            self.enter_closed(None);
            return true;
        }
        let Some(due) = self.retransmit_at else {
            return false;
        };
        if now_ms < due {
            return false;
        }
        let limit = match self.state {
            State::SynSent | State::SynReceived => SYN_RETRIES,
            _ => DATA_RETRIES,
        };
        if self.retries >= limit {
            self.enter_closed(Some(Error::TimedOut));
            return true;
        }
        self.retries += 1;
        self.rto_ms = (self.rto_ms * 2).min(MAX_RTO_MS);
        self.retransmit_at = None;
        self.timed = None;
        self.duplicate_acks = 0;
        match self.state {
            State::SynSent | State::SynReceived => self.syn_due = true,
            _ => self.go_back(),
        }
        if self.send_window == 0 {
            self.probe_due = true;
        }
        false
    }

    fn go_back(&mut self) {
        self.send_next = self.send_unacknowledged;
        self.fin_sent = false;
    }

    fn window(&self, buffers: &(impl Buffers + ?Sized)) -> u32 {
        buffers.receive_room().min(MAX_WINDOW) as u32
    }

    /// Takes in `segment`, which came for it at `now_ms`, putting its data in `buffers`.
    pub fn receive(
        &mut self,
        segment: &Segment<'_>,
        now_ms: u64,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> Received {
        let before_state = self.state;
        let mut received = match self.state {
            State::Closed => Received::default(),
            State::SynSent => self.receive_in_syn_sent(segment, now_ms, buffers),
            _ => self.receive_synchronized(segment, now_ms, buffers),
        };
        received.state_changed = self.state != before_state;
        received
    }

    fn receive_in_syn_sent(
        &mut self,
        segment: &Segment<'_>,
        now_ms: u64,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> Received {
        let header = &segment.header;
        let acceptable_ack = segment.has(flags::ACK)
            && before(self.initial_sequence, header.acknowledgment)
            && not_after(header.acknowledgment, self.send_next);
        if segment.has(flags::ACK) && !acceptable_ack {
            if !segment.has(flags::RST) {
                self.reset_due = true;
            }
            return Received::default();
        }
        if segment.has(flags::RST) {
            if acceptable_ack {
                self.enter_closed(Some(Error::Refused));
            }
            return Received::default();
        }
        if !segment.has(flags::SYN) {
            return Received::default();
        }
        self.receive_next = header.sequence.wrapping_add(1);
        self.advertised_edge = self.receive_next;
        self.send_mss = header
            .mss
            .unwrap_or(DEFAULT_MSS)
            .min(self.receive_mss)
            .max(1);
        self.window_update_sequence = header.sequence;
        self.window_update_acknowledgment = header.acknowledgment;
        self.send_window = u32::from(header.window);
        self.ack_due = true;
        self.syn_due = false;
        if !acceptable_ack {
            self.state = State::SynReceived;
            self.syn_due = true;
            return Received::default();
        }
        self.measure_rtt(header.acknowledgment, now_ms);
        self.send_unacknowledged = header.acknowledgment;
        self.retransmit_at = None;
        self.retries = 0;
        self.state = State::Established;
        let data = self.take_data(segment, header.sequence.wrapping_add(1), buffers);
        self.take_fin(segment, header.sequence.wrapping_add(1), now_ms);
        Received {
            data,
            ..Received::default()
        }
    }

    fn acceptable(&self, segment: &Segment<'_>, window: u32) -> bool {
        let start = segment.header.sequence;
        let length = segment.length();
        let edge = self.receive_next.wrapping_add(window);
        match (length, window) {
            (0, 0) => start == self.receive_next,
            (0, _) => not_after(self.receive_next, start) && before(start, edge),
            (_, 0) => false,
            (_, _) => {
                let last = start.wrapping_add(length - 1);
                (not_after(self.receive_next, start) && before(start, edge))
                    || (not_after(self.receive_next, last) && before(last, edge))
            }
        }
    }

    fn receive_synchronized(
        &mut self,
        segment: &Segment<'_>,
        now_ms: u64,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> Received {
        let window = self.window(&*buffers).max(1);
        if !self.acceptable(segment, window) {
            if !segment.has(flags::RST) {
                self.ack_due = true;
            }
            return Received::default();
        }
        let header = &segment.header;
        if segment.has(flags::RST) {
            match header.sequence == self.receive_next {
                true => self.enter_closed(Some(Error::Reset)),
                false => self.ack_due = true,
            }
            return Received::default();
        }
        if segment.has(flags::SYN) {
            if self.state == State::SynReceived
                && header.sequence.wrapping_add(1) == self.receive_next
            {
                self.syn_due = true;
            } else {
                self.ack_due = true;
            }
            return Received::default();
        }
        if !segment.has(flags::ACK) {
            return Received::default();
        }
        let mut received = Received::default();
        if self.state == State::SynReceived {
            let acks_syn = before(self.send_unacknowledged, header.acknowledgment)
                && not_after(header.acknowledgment, self.send_next);
            if !acks_syn {
                self.reset_due = true;
                return received;
            }
            self.state = State::Established;
            self.syn_due = false;
            self.send_unacknowledged = self.initial_sequence.wrapping_add(1);
            self.window_update_sequence = header.sequence;
            self.window_update_acknowledgment = header.acknowledgment;
            self.send_window = u32::from(header.window);
            self.retries = 0;
            self.retransmit_at = None;
            self.measure_rtt(header.acknowledgment, now_ms);
        }
        if before(self.send_next, header.acknowledgment) && !self.after_retransmit(header) {
            self.ack_due = true;
            return received;
        }
        received.acknowledged = self.take_acknowledgment(segment, now_ms, buffers);
        if self.state == State::Closed {
            return received;
        }
        received.data = self.take_data(segment, header.sequence, buffers);
        self.take_fin(segment, header.sequence, now_ms);
        received
    }

    fn after_retransmit(&self, header: &Header) -> bool {
        not_after(header.acknowledgment, self.send_highest)
    }

    fn take_acknowledgment(
        &mut self,
        segment: &Segment<'_>,
        now_ms: u64,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> usize {
        let header = &segment.header;
        let ack = header.acknowledgment;
        if before(ack, self.send_unacknowledged) {
            return 0;
        }
        if before(self.send_next, ack) {
            self.send_next = ack;
        }
        let newly = ack.wrapping_sub(self.send_unacknowledged) as usize;
        let queued = buffers.send_len();
        let data = newly.min(queued);
        let fin_acknowledged = newly > queued;
        if fin_acknowledged {
            self.fin_sent = true;
        }
        if newly > 0 || self.send_window == 0 {
            self.retries = 0;
        }
        if newly > 0 {
            buffers.acknowledged(data);
            self.send_unacknowledged = ack;
            self.measure_rtt(ack, now_ms);
            self.duplicate_acks = 0;
            self.retransmit_at = match self.send_unacknowledged == self.send_next {
                true => None,
                false => Some(now_ms.saturating_add(self.rto_ms)),
            };
        } else if segment.data.is_empty()
            && !segment.has(flags::FIN)
            && u32::from(header.window) == self.send_window
            && self.send_unacknowledged != self.send_next
        {
            self.duplicate_acks = self.duplicate_acks.saturating_add(1);
            if self.duplicate_acks == DUPLICATE_ACKS {
                self.timed = None;
                self.go_back();
            }
        }
        let newer = before(self.window_update_sequence, header.sequence)
            || (self.window_update_sequence == header.sequence
                && not_after(self.window_update_acknowledgment, ack));
        if newer {
            let opened = self.send_window == 0 && header.window > 0;
            self.send_window = u32::from(header.window);
            self.window_update_sequence = header.sequence;
            self.window_update_acknowledgment = ack;
            if opened {
                self.probe_due = false;
                self.go_back();
            }
            if self.send_window == 0 && self.retransmit_at.is_none() && buffers.send_len() > 0 {
                self.retransmit_at = Some(now_ms.saturating_add(self.rto_ms));
            }
        }
        if fin_acknowledged {
            match self.state {
                State::FinWait1 => {
                    self.state = State::FinWait2;
                    self.deadline = Some(now_ms.saturating_add(FIN_WAIT_2_MS));
                }
                State::Closing => self.enter_time_wait(now_ms),
                State::LastAck => self.enter_closed(None),
                _ => {}
            }
        }
        data
    }

    fn measure_rtt(&mut self, ack: u32, now_ms: u64) {
        let Some((sequence, sent_ms)) = self.timed else {
            return;
        };
        if !before(sequence, ack) {
            return;
        }
        self.timed = None;
        let sample = now_ms.saturating_sub(sent_ms);
        match self.smoothed_rtt_ms {
            None => {
                self.smoothed_rtt_ms = Some(sample);
                self.rtt_variance_ms = sample / 2;
            }
            Some(smoothed) => {
                let error = smoothed.abs_diff(sample);
                self.rtt_variance_ms = (3 * self.rtt_variance_ms + error) / 4;
                self.smoothed_rtt_ms = Some((7 * smoothed + sample) / 8);
            }
        }
        let smoothed = self.smoothed_rtt_ms.unwrap_or(sample);
        self.rto_ms = (smoothed + (4 * self.rtt_variance_ms).max(1)).clamp(MIN_RTO_MS, MAX_RTO_MS);
    }

    fn take_data(
        &mut self,
        segment: &Segment<'_>,
        first: u32,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> usize {
        if segment.data.is_empty() {
            return 0;
        }
        if !matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        ) {
            return 0;
        }
        self.ack_due = true;
        if before(self.receive_next, first) {
            return 0;
        }
        let skip = self.receive_next.wrapping_sub(first) as usize;
        let Some(fresh) = segment.data.get(skip..) else {
            return 0;
        };
        let taken = buffers.received(fresh);
        self.receive_next = self.receive_next.wrapping_add(taken as u32);
        taken
    }

    fn take_fin(&mut self, segment: &Segment<'_>, first: u32, now_ms: u64) {
        if !segment.has(flags::FIN) || self.state == State::Closed {
            return;
        }
        let fin = first.wrapping_add(segment.data.len() as u32);
        if fin != self.receive_next {
            return;
        }
        self.receive_next = self.receive_next.wrapping_add(1);
        self.fin_received = true;
        self.ack_due = true;
        match self.state {
            State::SynReceived | State::Established => self.state = State::CloseWait,
            State::FinWait1 => self.state = State::Closing,
            State::FinWait2 => self.enter_time_wait(now_ms),
            State::TimeWait => self.enter_time_wait(now_ms),
            _ => {}
        }
    }

    fn base_header(&self, flags: u8, window: u32) -> Header {
        Header {
            source_port: self.local.1,
            destination_port: self.remote.1,
            sequence: self.send_next,
            acknowledgment: self.receive_next,
            flags: flags | flags::ACK,
            window: window as u16,
            mss: None,
        }
    }

    fn wants_window_update(&self, window: u32) -> bool {
        let edge = self.receive_next.wrapping_add(window);
        before(self.advertised_edge, edge)
            && edge.wrapping_sub(self.advertised_edge) >= u32::from(self.receive_mss).min(window)
            && matches!(
                self.state,
                State::Established | State::FinWait1 | State::FinWait2
            )
    }

    /// Writes the next segment it has to send at `now_ms` to the start of `out` (at least
    /// [`MAX_HEADER_BYTES`] plus the segment size long): its length, or `None` when it has
    /// nothing to send. Call it again until it has nothing.
    pub fn output(
        &mut self,
        now_ms: u64,
        buffers: &(impl Buffers + ?Sized),
        out: &mut [u8],
    ) -> Option<usize> {
        if self.reset_due {
            self.reset_due = false;
            let header = Header {
                source_port: self.local.1,
                destination_port: self.remote.1,
                sequence: self.send_next,
                acknowledgment: self.receive_next,
                flags: flags::RST | flags::ACK,
                window: 0,
                mss: None,
            };
            return write_header(out, self.local.0, self.remote.0, &header, 0);
        }
        let window = self.window(buffers);
        match self.state {
            State::Closed => None,
            State::SynSent | State::SynReceived => self.output_syn(now_ms, window, out),
            _ => self.output_synchronized(now_ms, buffers, window, out),
        }
    }

    fn output_syn(&mut self, now_ms: u64, window: u32, out: &mut [u8]) -> Option<usize> {
        if !self.syn_due && !self.ack_due {
            return None;
        }
        self.syn_due = false;
        self.ack_due = false;
        self.send_next = self.initial_sequence;
        let mut header = self.base_header(flags::SYN, window);
        header.mss = Some(self.receive_mss);
        if self.state == State::SynSent {
            header.flags = flags::SYN;
            header.acknowledgment = 0;
        }
        let length = write_header(out, self.local.0, self.remote.0, &header, 0)?;
        self.sent(1, now_ms);
        self.advertised_edge = self.receive_next.wrapping_add(window);
        Some(length)
    }

    fn sent(&mut self, sequence_length: u32, now_ms: u64) {
        if sequence_length == 0 {
            return;
        }
        if self.timed.is_none() && self.retries == 0 {
            self.timed = Some((self.send_next, now_ms));
        }
        self.send_next = self.send_next.wrapping_add(sequence_length);
        if before(self.send_highest, self.send_next) {
            self.send_highest = self.send_next;
        }
        if self.retransmit_at.is_none() {
            self.retransmit_at = Some(now_ms.saturating_add(self.rto_ms));
        }
    }

    fn output_synchronized(
        &mut self,
        now_ms: u64,
        buffers: &(impl Buffers + ?Sized),
        window: u32,
        out: &mut [u8],
    ) -> Option<usize> {
        if self.state == State::TimeWait {
            return self.output_ack(window, out);
        }
        let queued = buffers.send_len();
        let offset = self.send_next.wrapping_sub(self.send_unacknowledged) as usize;
        let unsent = queued.saturating_sub(offset.min(queued));
        let in_flight = self.send_next.wrapping_sub(self.send_unacknowledged);
        let mut usable = self.send_window.saturating_sub(in_flight) as usize;
        let probing = self.probe_due && usable == 0 && unsent > 0;
        if probing {
            usable = 1;
        }
        let room = out.len().saturating_sub(HEADER_BYTES);
        let count = unsent.min(usable).min(usize::from(self.send_mss)).min(room);
        let closing = matches!(
            self.state,
            State::FinWait1 | State::Closing | State::LastAck
        );
        let fin = closing && !self.fin_sent && offset.min(queued) + count == queued;
        if count == 0 && !fin {
            if unsent > 0 && in_flight == 0 && self.retransmit_at.is_none() {
                self.retransmit_at = Some(now_ms.saturating_add(self.rto_ms));
            }
            if self.wants_window_update(window) {
                self.ack_due = true;
            }
            return self.output_ack(window, out);
        }
        let copied =
            buffers.copy_unacknowledged(offset, &mut out[HEADER_BYTES..HEADER_BYTES + count]);
        let mut flags_set = match copied {
            0 => 0,
            _ => flags::PSH,
        };
        if fin {
            flags_set |= flags::FIN;
        }
        let header = self.base_header(flags_set, window);
        let length = write_header(out, self.local.0, self.remote.0, &header, copied)?;
        if fin {
            self.fin_sent = true;
        }
        if probing {
            self.probe_due = false;
        }
        self.ack_due = false;
        self.advertised_edge = self.receive_next.wrapping_add(window);
        self.sent(copied as u32 + u32::from(fin), now_ms);
        Some(length)
    }

    fn output_ack(&mut self, window: u32, out: &mut [u8]) -> Option<usize> {
        if !self.ack_due {
            return None;
        }
        self.ack_due = false;
        self.advertised_edge = self.receive_next.wrapping_add(window);
        let header = self.base_header(0, window);
        write_header(out, self.local.0, self.remote.0, &header, 0)
    }
}

#[cfg(test)]
mod tests;

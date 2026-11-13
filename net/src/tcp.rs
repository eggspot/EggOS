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
//! sends each segment [`Connection::output`] writes. With [`Connection::set_extensions`]
//! (D564) it also scales its window (RFC 7323), keeps the segments that arrive out of order in
//! its caller's buffers and reports them with selective acknowledgments, resending only the
//! holes the peer's selective acknowledgments show (RFC 2018, RFC 6675's idea of a scoreboard),
//! probes an idle connection (keep-alive, RFC 9293 3.8.4) and sends and takes urgent data
//! (RFC 9293 3.8.5, the urgent pointer pointing after the urgent byte as BSD and Linux do).

use crate::checksum::{self, Completion};
use crate::ipv4::{Ipv4Address, protocol};

/// Bytes of a header without options.
pub const HEADER_BYTES: usize = 20;
/// Bytes of the maximum segment size option.
pub const MSS_OPTION_BYTES: usize = 4;
/// Where the checksum lies in a segment.
pub const CHECKSUM_AT: usize = 16;
/// The largest header there is: 40 bytes of options (D564).
pub const MAX_HEADER_BYTES: usize = HEADER_BYTES + 40;
/// The most selective acknowledgment blocks a segment carries.
pub const MAX_SACK_BLOCKS: usize = 3;
/// The largest window scale shift (RFC 7323 2.3).
pub const MAX_WINDOW_SCALE: u8 = 14;
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
    /// The urgent pointer is significant.
    pub const URG: u8 = 0x20;
}

const OPTION_END: u8 = 0;
const OPTION_NOP: u8 = 1;
const OPTION_MSS: u8 = 2;
const OPTION_WINDOW_SCALE: u8 = 3;
const OPTION_SACK_PERMITTED: u8 = 4;
const OPTION_SACK: u8 = 5;
const URGENT_AT: usize = 18;

/// Selective acknowledgment blocks: ranges of sequence numbers received beyond the
/// acknowledgment, each from its first number to the one after its last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SackBlocks {
    /// The blocks, the first [`Self::count`] of them used.
    pub blocks: [(u32, u32); MAX_SACK_BLOCKS],
    /// How many are used.
    pub count: u8,
}

impl SackBlocks {
    /// The blocks used.
    pub fn used(&self) -> &[(u32, u32)] {
        &self.blocks[..usize::from(self.count)]
    }

    fn push(&mut self, block: (u32, u32)) {
        if usize::from(self.count) < MAX_SACK_BLOCKS {
            self.blocks[usize::from(self.count)] = block;
            self.count += 1;
        }
    }
}

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
    /// The window scale option's shift, on a SYN (D564).
    pub window_scale: Option<u8>,
    /// The selective-acknowledgment-permitted option, on a SYN.
    pub sack_permitted: bool,
    /// The selective acknowledgment option's blocks.
    pub sack: SackBlocks,
    /// The urgent pointer, when [`flags::URG`] is set: how far after the sequence number the
    /// urgent data ends.
    pub urgent: u16,
}

/// The options of a segment the stack reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Options {
    mss: Option<u16>,
    window_scale: Option<u8>,
    sack_permitted: bool,
    sack: SackBlocks,
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
    let options = parse_options(&bytes[HEADER_BYTES..header_bytes]);
    Some(Segment {
        header: Header {
            source_port: half(0),
            destination_port: half(2),
            sequence: word(4),
            acknowledgment: word(8),
            flags: fixed[13] & 0x3f,
            window: half(14),
            mss: options.mss,
            window_scale: options.window_scale,
            sack_permitted: options.sack_permitted,
            sack: options.sack,
            urgent: half(URGENT_AT),
        },
        data: &bytes[header_bytes..],
    })
}

fn parse_options(mut options: &[u8]) -> Options {
    let mut found = Options::default();
    while let Some((&kind, rest)) = options.split_first() {
        match kind {
            OPTION_END => break,
            OPTION_NOP => options = rest,
            _ => {
                let Some(&length) = rest.first() else {
                    break;
                };
                let length = usize::from(length);
                if length < 2 || length > options.len() {
                    break;
                }
                let body = &options[2..length];
                match (kind, body.len()) {
                    (OPTION_MSS, 2) => found.mss = Some(u16::from_be_bytes([body[0], body[1]])),
                    (OPTION_WINDOW_SCALE, 1) => {
                        found.window_scale = Some(body[0].min(MAX_WINDOW_SCALE));
                    }
                    (OPTION_SACK_PERMITTED, 0) => found.sack_permitted = true,
                    (OPTION_SACK, bytes) if bytes % 8 == 0 => {
                        for block in body.chunks_exact(8) {
                            let left = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
                            let right = u32::from_be_bytes([block[4], block[5], block[6], block[7]]);
                            found.sack.push((left, right));
                        }
                    }
                    _ => {}
                }
                options = &options[length..];
            }
        }
    }
    found
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
    write_header_with(
        out,
        source,
        destination,
        header,
        data_bytes,
        Completion::Full,
    )
}

/// [`write_header`], with the checksum written whole or left `completion` says.
pub fn write_header_with(
    out: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    header: &Header,
    data_bytes: usize,
    completion: Completion,
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
    segment[16..18].fill(0);
    let urgent = match header.flags & flags::URG {
        0 => 0,
        _ => header.urgent,
    };
    segment[URGENT_AT..URGENT_AT + 2].copy_from_slice(&urgent.to_be_bytes());
    let mut at = HEADER_BYTES;
    if let Some(mss) = header.mss {
        segment[at..at + 4].copy_from_slice(&[OPTION_MSS, MSS_OPTION_BYTES as u8, 0, 0]);
        segment[at + 2..at + 4].copy_from_slice(&mss.to_be_bytes());
        at += 4;
    }
    if let Some(shift) = header.window_scale {
        segment[at..at + 4].copy_from_slice(&[OPTION_NOP, OPTION_WINDOW_SCALE, 3, shift]);
        at += 4;
    }
    if header.sack_permitted {
        segment[at..at + 4].copy_from_slice(&[OPTION_NOP, OPTION_NOP, OPTION_SACK_PERMITTED, 2]);
        at += 4;
    }
    if header.sack.count > 0 {
        let length = 2 + 8 * usize::from(header.sack.count);
        segment[at..at + 4].copy_from_slice(&[OPTION_NOP, OPTION_NOP, OPTION_SACK, length as u8]);
        at += 4;
        for &(left, right) in header.sack.used() {
            segment[at..at + 4].copy_from_slice(&left.to_be_bytes());
            segment[at + 4..at + 8].copy_from_slice(&right.to_be_bytes());
            at += 8;
        }
    }
    let pseudo = pseudo_header(source, destination, length);
    let sum = match completion {
        Completion::Full => checksum::finish(checksum::add(pseudo, segment)),
        Completion::Partial => checksum::partial(pseudo),
    };
    segment[CHECKSUM_AT..CHECKSUM_AT + 2].copy_from_slice(&sum.to_be_bytes());
    Some(length)
}

/// Makes the partial checksum of `segment`, from `source` to `destination`, whole, or a whole
/// one partial, as `completion` asks (D535).
pub fn convert_checksum(
    segment: &mut [u8],
    source: Ipv4Address,
    destination: Ipv4Address,
    completion: Completion,
) {
    if segment.len() < HEADER_BYTES {
        return;
    }
    match completion {
        Completion::Full => checksum::complete(segment, CHECKSUM_AT),
        Completion::Partial => {
            let sum = checksum::partial(pseudo_header(source, destination, segment.len()));
            segment[CHECKSUM_AT..CHECKSUM_AT + 2].copy_from_slice(&sum.to_be_bytes());
        }
    }
}

/// How many bytes `header` takes written: [`HEADER_BYTES`], plus its options, each padded to
/// four bytes.
pub fn header_bytes(header: &Header) -> usize {
    let sack = match header.sack.count {
        0 => 0,
        count => 4 + 8 * usize::from(count),
    };
    HEADER_BYTES
        + 4 * usize::from(header.mss.is_some())
        + 4 * usize::from(header.window_scale.is_some())
        + 4 * usize::from(header.sack_permitted)
        + sack
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
    /// Keeps `data`, which came out of order and starts at sequence number `sequence`, until
    /// what comes before it arrives (D564): whether it kept it. Never by default: such data is
    /// dropped and the sender sends it again.
    fn hold_out_of_order(&mut self, _sequence: u32, _data: &[u8]) -> bool {
        false
    }
    /// Takes in the data it holds that now follows on from `next`, the next sequence number
    /// in order, dropping what lies before it: how many bytes `next` moves on.
    fn take_held(&mut self, _next: u32) -> u32 {
        0
    }
    /// The ranges of sequence numbers it holds, the most recently received first, at most
    /// as many as `out` has room for: how many it wrote.
    fn held_ranges(&self, _out: &mut [(u32, u32)]) -> usize {
        0
    }
}

/// What [`Connection::set_extensions`] turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extensions {
    /// The window scale shift to offer for the receive window, `None` for no scaling.
    pub window_scale: Option<u8>,
    /// Whether to offer selective acknowledgments.
    pub selective_acknowledgments: bool,
}

/// When an idle connection probes its peer (keep-alive, D564): Linux's `TCP_KEEPIDLE`,
/// `TCP_KEEPINTVL` and `TCP_KEEPCNT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAlive {
    /// How long the connection is idle before the first probe.
    pub idle_ms: u64,
    /// How long between probes.
    pub interval_ms: u64,
    /// How many unanswered probes end it.
    pub probes: u8,
}

impl KeepAlive {
    /// Linux's defaults: two hours idle, 75 seconds between probes, nine probes.
    pub const LINUX: Self = Self {
        idle_ms: 7_200_000,
        interval_ms: 75_000,
        probes: 9,
    };
}

const SCOREBOARD_BLOCKS: usize = 4;

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
    checksum: Completion,
    segment_limit: usize,
    offered: Extensions,
    peer_window_scale: Option<u8>,
    peer_sack: bool,
    send_scale: u8,
    receive_scale: u8,
    sack_permitted: bool,
    scoreboard: [(u32, u32); SCOREBOARD_BLOCKS],
    scoreboard_count: u8,
    recovery_point: Option<u32>,
    hole_due: Option<u32>,
    keepalive: Option<KeepAlive>,
    last_heard_ms: u64,
    keepalive_probes: u8,
    keepalive_due: bool,
    send_urgent_end: Option<u32>,
    receive_urgent: Option<u32>,
    urgent_byte: Option<u8>,
    urgent_inline: bool,
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
    /// Whether the peer marked new urgent data (D564): its owner gets `SIGURG`.
    pub urgent: bool,
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
        connection.peer_window_scale = syn.header.window_scale;
        connection.peer_sack = syn.header.sack_permitted;
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
            checksum: Completion::Full,
            segment_limit: 0,
            offered: Extensions::default(),
            peer_window_scale: None,
            peer_sack: false,
            send_scale: 0,
            receive_scale: 0,
            sack_permitted: false,
            scoreboard: [(0, 0); SCOREBOARD_BLOCKS],
            scoreboard_count: 0,
            recovery_point: None,
            hole_due: None,
            keepalive: None,
            last_heard_ms: 0,
            keepalive_probes: 0,
            keepalive_due: false,
            send_urgent_end: None,
            receive_urgent: None,
            urgent_byte: None,
            urgent_inline: false,
        }
    }

    /// Offers window scaling and selective acknowledgments (D564): before the SYN goes, on a
    /// connection that connects, and before the answer to the peer's SYN goes, on one a
    /// listener accepted (they are used only when the peer's SYN offered them too).
    pub fn set_extensions(&mut self, extensions: Extensions) {
        self.offered = Extensions {
            window_scale: extensions.window_scale.map(|shift| shift.min(MAX_WINDOW_SCALE)),
            ..extensions
        };
        if self.state == State::SynReceived {
            self.negotiate();
        }
    }

    fn negotiate(&mut self) {
        match (self.offered.window_scale, self.peer_window_scale) {
            (Some(ours), Some(theirs)) => {
                self.receive_scale = ours;
                self.send_scale = theirs;
            }
            _ => {
                self.receive_scale = 0;
                self.send_scale = 0;
            }
        }
        self.sack_permitted = self.offered.selective_acknowledgments && self.peer_sack;
    }

    /// Whether the two sides scale their windows: the shifts each applies, ours then the
    /// peer's.
    pub fn window_scales(&self) -> (u8, u8) {
        (self.receive_scale, self.send_scale)
    }

    /// Whether the two sides use selective acknowledgments.
    pub fn uses_selective_acknowledgments(&self) -> bool {
        self.sack_permitted
    }

    /// Probes the peer once the connection was idle `keep_alive` says long, and ends it when
    /// the probes go unanswered (`SO_KEEPALIVE`, D564); `None` stops probing. `now_ms` starts
    /// the idle time.
    pub fn set_keep_alive(&mut self, keep_alive: Option<KeepAlive>, now_ms: u64) {
        self.keepalive = keep_alive;
        self.last_heard_ms = now_ms;
        self.keepalive_probes = 0;
        self.keepalive_due = false;
    }

    /// The keep-alive settings, if it probes.
    pub fn keep_alive(&self) -> Option<KeepAlive> {
        self.keepalive
    }

    /// Marks everything `buffers` hold to send as urgent (`send(MSG_OOB)`, D564): the
    /// segments that carry it say so until the peer acknowledged its last byte.
    pub fn mark_urgent(&mut self, buffers: &(impl Buffers + ?Sized)) {
        if buffers.send_len() > 0 {
            self.send_urgent_end = Some(
                self.send_unacknowledged
                    .wrapping_add(buffers.send_len() as u32),
            );
        }
    }

    /// Keeps urgent bytes in the stream (`SO_OOBINLINE`) instead of apart.
    pub fn set_urgent_inline(&mut self, inline: bool) {
        self.urgent_inline = inline;
    }

    /// The urgent byte the peer sent, kept apart from the stream, taken unless `peek`
    /// (`recv(MSG_OOB)`).
    pub fn take_urgent_byte(&mut self, peek: bool) -> Option<u8> {
        match peek {
            true => self.urgent_byte,
            false => self.urgent_byte.take(),
        }
    }

    /// Whether urgent data is announced and its byte not yet read (`POLLPRI`).
    pub fn has_urgent(&self) -> bool {
        self.urgent_byte.is_some() || self.receive_urgent.is_some()
    }

    /// Has [`Connection::output`] write its segments' checksums whole or leave them partial
    /// (D535); whole until this says otherwise.
    pub fn set_checksum(&mut self, completion: Completion) {
        self.checksum = completion;
    }

    /// How [`Connection::output`] writes its segments' checksums.
    pub fn checksum(&self) -> Completion {
        self.checksum
    }

    /// Lets [`Connection::output`] put up to `data_bytes` of data in one segment, in whole
    /// multiples of the peer's maximum segment size, for an adapter that cuts it into segments
    /// of that size itself (D541); `None` keeps every segment to the maximum segment size, as
    /// at the start.
    pub fn set_segment_limit(&mut self, data_bytes: Option<usize>) {
        self.segment_limit = data_bytes.unwrap_or(0);
    }

    /// The most data one segment holds: the peer's maximum segment size, or as many whole
    /// multiples of it as [`Connection::set_segment_limit`] allows.
    pub fn segment_data_bytes(&self) -> usize {
        let mss = usize::from(self.send_mss);
        mss.max(self.segment_limit / mss * mss)
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
            .min(self.keepalive_at().unwrap_or(u64::MAX))
    }

    fn keepalive_at(&self) -> Option<u64> {
        let keepalive = self.keepalive?;
        if !matches!(self.state, State::Established | State::CloseWait)
            || self.send_unacknowledged != self.send_next
        {
            return None;
        }
        let waited = keepalive
            .idle_ms
            .saturating_add(u64::from(self.keepalive_probes).saturating_mul(keepalive.interval_ms));
        Some(self.last_heard_ms.saturating_add(waited))
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
        if let (Some(due), Some(keepalive)) = (self.keepalive_at(), self.keepalive)
            && now_ms >= due
        {
            if self.keepalive_probes >= keepalive.probes {
                self.enter_closed(Some(Error::TimedOut));
                return true;
            }
            self.keepalive_probes += 1;
            self.keepalive_due = true;
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
        self.scoreboard_count = 0;
        self.recovery_point = None;
        self.hole_due = None;
    }

    fn window(&self, buffers: &(impl Buffers + ?Sized)) -> u32 {
        let largest = MAX_WINDOW << self.receive_scale;
        let room = buffers.receive_room().min(largest) as u32;
        room >> self.receive_scale << self.receive_scale
    }

    fn peer_window(&self, header: &Header) -> u32 {
        u32::from(header.window) << self.send_scale
    }

    fn sacked(&self) -> &[(u32, u32)] {
        &self.scoreboard[..usize::from(self.scoreboard_count)]
    }

    fn note_sack(&mut self, blocks: &SackBlocks) {
        for &(left, right) in blocks.used() {
            if !before(left, right)
                || !before(self.send_unacknowledged, right)
                || before(self.send_highest, right)
            {
                continue;
            }
            let left = match before(left, self.send_unacknowledged) {
                true => self.send_unacknowledged,
                false => left,
            };
            let mut merged = (left, right);
            let mut kept = [(0u32, 0u32); SCOREBOARD_BLOCKS];
            let mut count = 0;
            for &(start, end) in self.sacked() {
                let overlaps = not_after(start, merged.1) && not_after(merged.0, end);
                if overlaps {
                    merged.0 = if before(start, merged.0) { start } else { merged.0 };
                    merged.1 = if before(merged.1, end) { end } else { merged.1 };
                } else if count < SCOREBOARD_BLOCKS - 1 {
                    kept[count] = (start, end);
                    count += 1;
                }
            }
            kept[count] = merged;
            count += 1;
            kept[..count].sort_unstable_by(|a, b| {
                if before(a.0, b.0) {
                    core::cmp::Ordering::Less
                } else if a.0 == b.0 {
                    core::cmp::Ordering::Equal
                } else {
                    core::cmp::Ordering::Greater
                }
            });
            self.scoreboard = kept;
            self.scoreboard_count = count as u8;
        }
    }

    fn prune_sacked(&mut self) {
        let floor = self.send_unacknowledged;
        let mut kept = [(0u32, 0u32); SCOREBOARD_BLOCKS];
        let mut count = 0;
        for &(start, end) in self.sacked() {
            if before(floor, end) {
                kept[count] = (if before(start, floor) { floor } else { start }, end);
                count += 1;
            }
        }
        self.scoreboard = kept;
        self.scoreboard_count = count as u8;
    }

    fn next_hole(&self, from: u32) -> Option<(u32, u32)> {
        let mut start = from;
        for &(left, right) in self.sacked() {
            if before(start, left) {
                return Some((start, left));
            }
            if before(start, right) {
                start = right;
            }
        }
        None
    }

    /// Takes in `segment`, which came for it at `now_ms`, putting its data in `buffers`.
    pub fn receive(
        &mut self,
        segment: &Segment<'_>,
        now_ms: u64,
        buffers: &mut (impl Buffers + ?Sized),
    ) -> Received {
        let before_state = self.state;
        self.last_heard_ms = now_ms;
        self.keepalive_probes = 0;
        self.keepalive_due = false;
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
        self.peer_window_scale = header.window_scale;
        self.peer_sack = header.sack_permitted;
        self.negotiate();
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
            self.send_window = self.peer_window(header);
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
        received.urgent = self.take_urgent_pointer(segment);
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
        if self.sack_permitted {
            self.note_sack(&header.sack);
        }
        if newly > 0 {
            buffers.acknowledged(data);
            self.send_unacknowledged = ack;
            self.measure_rtt(ack, now_ms);
            self.duplicate_acks = 0;
            self.prune_sacked();
            if let Some(point) = self.recovery_point {
                match before(ack, point) {
                    true => self.hole_due = self.next_hole(ack).map(|(start, _)| start),
                    false => {
                        self.recovery_point = None;
                        self.hole_due = None;
                    }
                }
            }
            if let Some(end) = self.send_urgent_end
                && not_after(end, ack)
            {
                self.send_urgent_end = None;
            }
            self.retransmit_at = match self.send_unacknowledged == self.send_next {
                true => None,
                false => Some(now_ms.saturating_add(self.rto_ms)),
            };
        } else if segment.data.is_empty()
            && !segment.has(flags::FIN)
            && self.peer_window(header) == self.send_window
            && self.send_unacknowledged != self.send_next
        {
            self.duplicate_acks = self.duplicate_acks.saturating_add(1);
            if self.duplicate_acks == DUPLICATE_ACKS {
                self.timed = None;
                match self.sack_permitted && self.scoreboard_count > 0 {
                    true => {
                        self.recovery_point = Some(self.send_highest);
                        self.hole_due = Some(self.send_unacknowledged);
                    }
                    false => self.go_back(),
                }
            } else if self.duplicate_acks > DUPLICATE_ACKS && self.recovery_point.is_some() {
                self.hole_due = self.next_hole(self.send_unacknowledged).map(|(start, _)| start);
            }
        }
        let newer = before(self.window_update_sequence, header.sequence)
            || (self.window_update_sequence == header.sequence
                && not_after(self.window_update_acknowledgment, ack));
        if newer {
            let opened = self.send_window == 0 && header.window > 0;
            self.send_window = self.peer_window(header);
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

    fn take_urgent_pointer(&mut self, segment: &Segment<'_>) -> bool {
        if !segment.has(flags::URG) || segment.header.urgent == 0 {
            return false;
        }
        let mark = segment
            .header
            .sequence
            .wrapping_add(u32::from(segment.header.urgent))
            .wrapping_sub(1);
        if before(mark, self.receive_next) {
            return false;
        }
        let fresh = self.receive_urgent.is_none_or(|known| before(known, mark));
        if fresh {
            self.receive_urgent = Some(mark);
        }
        fresh
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
            buffers.hold_out_of_order(first, segment.data);
            return 0;
        }
        let skip = self.receive_next.wrapping_sub(first) as usize;
        let Some(fresh) = segment.data.get(skip..) else {
            return 0;
        };
        let taken = self.take_in_order(fresh, buffers);
        let held = buffers.take_held(self.receive_next);
        self.receive_next = self.receive_next.wrapping_add(held);
        taken + held as usize
    }

    fn take_in_order(&mut self, fresh: &[u8], buffers: &mut (impl Buffers + ?Sized)) -> usize {
        let urgent_at = self
            .receive_urgent
            .filter(|_| !self.urgent_inline)
            .map(|mark| mark.wrapping_sub(self.receive_next) as usize)
            .filter(|&at| at < fresh.len());
        let Some(at) = urgent_at else {
            let taken = buffers.received(fresh);
            self.receive_next = self.receive_next.wrapping_add(taken as u32);
            return taken;
        };
        let before_mark = buffers.received(&fresh[..at]);
        self.receive_next = self.receive_next.wrapping_add(before_mark as u32);
        if before_mark < at {
            return before_mark;
        }
        self.urgent_byte = Some(fresh[at]);
        self.receive_urgent = None;
        self.receive_next = self.receive_next.wrapping_add(1);
        let after_mark = buffers.received(&fresh[at + 1..]);
        self.receive_next = self.receive_next.wrapping_add(after_mark as u32);
        before_mark + after_mark
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
            window: (window >> self.receive_scale).min(MAX_WINDOW as u32) as u16,
            ..Header::default()
        }
    }

    fn with_sack(&self, mut header: Header, buffers: &(impl Buffers + ?Sized)) -> Header {
        if self.sack_permitted {
            let mut ranges = [(0u32, 0u32); MAX_SACK_BLOCKS];
            let count = buffers.held_ranges(&mut ranges).min(MAX_SACK_BLOCKS);
            for &range in &ranges[..count] {
                header.sack.push(range);
            }
        }
        header
    }

    fn with_urgent(&self, mut header: Header) -> Header {
        if let Some(end) = self.send_urgent_end
            && before(header.sequence, end)
        {
            header.flags |= flags::URG;
            header.urgent = end.wrapping_sub(header.sequence).min(u32::from(u16::MAX)) as u16;
        }
        header
    }

    fn write_header(&self, out: &mut [u8], header: &Header, data_bytes: usize) -> Option<usize> {
        let (source, destination) = (self.local.0, self.remote.0);
        write_header_with(out, source, destination, header, data_bytes, self.checksum)
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
                ..Header::default()
            };
            return self.write_header(out, &header, 0);
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
        header.window = window.min(MAX_WINDOW as u32) as u16;
        header.mss = Some(self.receive_mss);
        let answering = self.state == State::SynReceived;
        if !answering || self.peer_window_scale.is_some() {
            header.window_scale = self.offered.window_scale;
        }
        header.sack_permitted =
            self.offered.selective_acknowledgments && (!answering || self.peer_sack);
        if self.state == State::SynSent {
            header.flags = flags::SYN;
            header.acknowledgment = 0;
        }
        let length = self.write_header(out, &header, 0)?;
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
            return self.output_ack(window, buffers, out);
        }
        if self.keepalive_due {
            self.keepalive_due = false;
            let mut header = self.base_header(0, window);
            header.sequence = self.send_unacknowledged.wrapping_sub(1);
            return self.write_header(out, &header, 0);
        }
        if let Some(start) = self.hole_due.take()
            && let Some(length) = self.retransmit_hole(start, now_ms, buffers, window, out)
        {
            return Some(length);
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
        let largest_header = match self.sack_permitted {
            true => HEADER_BYTES + 4 + 8 * MAX_SACK_BLOCKS,
            false => HEADER_BYTES,
        };
        let room = out.len().saturating_sub(largest_header);
        let count = unsent.min(usable).min(self.segment_data_bytes()).min(room);
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
            return self.output_ack(window, buffers, out);
        }
        let mut flags_set = match count {
            0 => 0,
            _ => flags::PSH,
        };
        if fin {
            flags_set |= flags::FIN;
        }
        let header = self.with_urgent(self.with_sack(self.base_header(flags_set, window), buffers));
        let header_length = header_bytes(&header);
        let count = count.min(out.len().saturating_sub(header_length));
        let copied = buffers.copy_unacknowledged(
            offset,
            &mut out[header_length..header_length + count],
        );
        let length = self.write_header(out, &header, copied)?;
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

    fn output_ack(
        &mut self,
        window: u32,
        buffers: &(impl Buffers + ?Sized),
        out: &mut [u8],
    ) -> Option<usize> {
        if !self.ack_due {
            return None;
        }
        self.ack_due = false;
        self.advertised_edge = self.receive_next.wrapping_add(window);
        let header = self.with_sack(self.base_header(0, window), buffers);
        self.write_header(out, &header, 0)
    }

    fn retransmit_hole(
        &mut self,
        start: u32,
        now_ms: u64,
        buffers: &(impl Buffers + ?Sized),
        window: u32,
        out: &mut [u8],
    ) -> Option<usize> {
        let end = self
            .next_hole(start)
            .map_or(self.send_highest, |(_, end)| end);
        let offset = start.wrapping_sub(self.send_unacknowledged) as usize;
        let hole = end.wrapping_sub(start) as usize;
        let queued = buffers.send_len();
        if offset >= queued || hole == 0 {
            return None;
        }
        let mut header = self.with_sack(self.base_header(0, window), buffers);
        header.sequence = start;
        let header = self.with_urgent(header);
        let header_length = header_bytes(&header);
        let count = hole
            .min(queued - offset)
            .min(usize::from(self.send_mss))
            .min(out.len().saturating_sub(header_length));
        let copied = buffers.copy_unacknowledged(
            offset,
            &mut out[header_length..header_length + count],
        );
        let length = self.write_header(out, &header, copied)?;
        self.ack_due = false;
        self.advertised_edge = self.receive_next.wrapping_add(window);
        if self.retransmit_at.is_none() {
            self.retransmit_at = Some(now_ms.saturating_add(self.rto_ms));
        }
        Some(length)
    }
}

#[cfg(test)]
mod tests;

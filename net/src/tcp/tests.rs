use super::*;

const CLIENT: (Ipv4Address, u16) = (Ipv4Address::new(10, 0, 2, 15), 40000);
const SERVER: (Ipv4Address, u16) = (Ipv4Address::new(10, 0, 2, 2), 80);
const MSS: u16 = 1460;
const SEGMENT_ROOM: usize = MAX_HEADER_BYTES + MSS as usize;

#[derive(Debug, Default)]
struct Queues {
    send: Vec<u8>,
    received: Vec<u8>,
    room: usize,
}

impl Queues {
    fn with_room(room: usize) -> Self {
        Self {
            room,
            ..Self::default()
        }
    }
}

impl Buffers for Queues {
    fn send_len(&self) -> usize {
        self.send.len()
    }

    fn copy_unacknowledged(&self, offset: usize, out: &mut [u8]) -> usize {
        let available = &self.send[offset.min(self.send.len())..];
        let count = available.len().min(out.len());
        out[..count].copy_from_slice(&available[..count]);
        count
    }

    fn acknowledged(&mut self, count: usize) {
        self.send.drain(..count);
    }

    fn receive_room(&self) -> usize {
        self.room - self.received.len()
    }

    fn received(&mut self, data: &[u8]) -> usize {
        let count = data.len().min(self.receive_room());
        self.received.extend_from_slice(&data[..count]);
        count
    }
}

struct End {
    connection: Connection,
    queues: Queues,
}

impl End {
    fn output(&mut self, now_ms: u64) -> Vec<Vec<u8>> {
        let mut segments = Vec::new();
        let mut out = [0u8; SEGMENT_ROOM];
        while let Some(length) = self.connection.output(now_ms, &self.queues, &mut out) {
            segments.push(out[..length].to_vec());
            assert!(segments.len() < 1000, "output never ends");
        }
        segments
    }

    fn take(&mut self, bytes: &[u8], from: Ipv4Address, now_ms: u64) -> Received {
        let to = self.connection.local().0;
        let segment = parse(from, to, bytes).expect("a well-formed segment");
        self.connection.receive(&segment, now_ms, &mut self.queues)
    }
}

fn pair(room: usize) -> (End, End, Vec<u8>) {
    let mut client = End {
        connection: Connection::connect(CLIENT, SERVER, 1000, MSS),
        queues: Queues::with_room(room),
    };
    let syn = client.output(0);
    assert_eq!(syn.len(), 1);
    let segment = parse(CLIENT.0, SERVER.0, &syn[0]).unwrap();
    assert_eq!(segment.header.flags, flags::SYN);
    assert_eq!(segment.header.mss, Some(MSS));
    let server = End {
        connection: Connection::accept(SERVER, CLIENT, &segment, 5000, MSS).unwrap(),
        queues: Queues::with_room(room),
    };
    (client, server, syn[0].clone())
}

fn exchange(
    client: &mut End,
    server: &mut End,
    now_ms: u64,
    mut lose: impl FnMut(bool, &[u8]) -> bool,
) {
    for _ in 0..1000 {
        let from_client = client.output(now_ms);
        let from_server = server.output(now_ms);
        if from_client.is_empty() && from_server.is_empty() {
            return;
        }
        for segment in from_client {
            if !lose(true, &segment) {
                server.take(&segment, CLIENT.0, now_ms);
            }
        }
        for segment in from_server {
            if !lose(false, &segment) {
                client.take(&segment, SERVER.0, now_ms);
            }
        }
    }
    panic!("the exchange never settles");
}

fn connected(room: usize) -> (End, End) {
    let (mut client, mut server, _) = pair(room);
    exchange(&mut client, &mut server, 0, |_, _| false);
    assert_eq!(client.connection.state(), State::Established);
    assert_eq!(server.connection.state(), State::Established);
    (client, server)
}

#[test]
fn a_segment_round_trips_with_its_option_and_a_damaged_one_is_refused() {
    let header = Header {
        source_port: 40000,
        destination_port: 80,
        sequence: 0xdead_beef,
        acknowledgment: 7,
        flags: flags::SYN | flags::ACK,
        window: 65535,
        mss: Some(1460),
    };
    let mut bytes = [0u8; MAX_HEADER_BYTES + 3];
    bytes[MAX_HEADER_BYTES..].copy_from_slice(b"abc");
    assert_eq!(
        write_header(&mut bytes, CLIENT.0, SERVER.0, &header, 3),
        Some(MAX_HEADER_BYTES + 3)
    );
    let segment = parse(CLIENT.0, SERVER.0, &bytes).unwrap();
    assert_eq!(segment.header, header);
    assert_eq!(segment.data, b"abc");
    assert_eq!(segment.length(), 4);
    assert_eq!(parse(CLIENT.0, Ipv4Address::new(10, 0, 2, 3), &bytes), None);
    let mut damaged = bytes;
    damaged[MAX_HEADER_BYTES] ^= 1;
    assert_eq!(parse(CLIENT.0, SERVER.0, &damaged), None);
    let mut short = bytes;
    short[12] = 0x10;
    assert_eq!(parse(CLIENT.0, SERVER.0, &short[..HEADER_BYTES]), None);
    assert_eq!(
        mss_option(&[OPTION_NOP, OPTION_NOP, OPTION_MSS, 4, 0x05, 0xb4]),
        Some(1460)
    );
    assert_eq!(mss_option(&[OPTION_MSS, 9, 0, 0]), None);
    assert_eq!(mss_option(&[8, 0]), None);
    assert!(before(u32::MAX, 0));
    assert!(not_after(5, 5));
    assert!(!before(5, 5));
}

#[test]
fn a_reset_answers_a_segment_no_connection_owns_but_never_a_reset() {
    let (_, _, syn) = pair(4096);
    let segment = parse(CLIENT.0, SERVER.0, &syn).unwrap();
    let mut out = [0u8; HEADER_BYTES];
    let length = write_reset_for(&mut out, CLIENT.0, SERVER.0, &segment).unwrap();
    let reset = parse(SERVER.0, CLIENT.0, &out[..length]).unwrap();
    assert_eq!(reset.header.flags, flags::RST | flags::ACK);
    assert_eq!(reset.header.acknowledgment, 1001);
    assert_eq!(reset.header.destination_port, CLIENT.1);
    let mut client = End {
        connection: Connection::connect(CLIENT, SERVER, 1000, MSS),
        queues: Queues::with_room(4096),
    };
    client.output(0);
    client.take(&out[..length], SERVER.0, 5);
    assert_eq!(client.connection.state(), State::Closed);
    assert_eq!(client.connection.error(), Some(Error::Refused));
    assert!(write_reset_for(&mut [0u8; HEADER_BYTES], SERVER.0, CLIENT.0, &reset).is_none());
}

#[test]
fn data_flows_both_ways_and_an_orderly_close_ends_in_time_wait() {
    let (mut client, mut server) = connected(8192);
    assert_eq!(client.connection.send_mss(), MSS);
    client
        .queues
        .send
        .extend_from_slice(b"GET / HTTP/1.1\r\n\r\n");
    exchange(&mut client, &mut server, 10, |_, _| false);
    assert_eq!(server.queues.received, b"GET / HTTP/1.1\r\n\r\n");
    assert!(client.queues.send.is_empty());
    let body: Vec<u8> = (0..5000u32).map(|n| n as u8).collect();
    server.queues.send.extend_from_slice(&body);
    server.connection.close();
    assert_eq!(server.connection.state(), State::FinWait1);
    exchange(&mut client, &mut server, 20, |_, _| false);
    assert_eq!(client.queues.received, body);
    assert!(client.connection.fin_received());
    assert_eq!(client.connection.state(), State::CloseWait);
    assert_eq!(server.connection.state(), State::FinWait2);
    client.connection.close();
    exchange(&mut client, &mut server, 30, |_, _| false);
    assert_eq!(client.connection.state(), State::Closed);
    assert!(client.connection.is_finished());
    assert_eq!(client.connection.error(), None);
    assert_eq!(server.connection.state(), State::TimeWait);
    assert_eq!(server.connection.next_poll_ms(), 30 + TIME_WAIT_MS);
    assert!(!server.connection.poll(30 + TIME_WAIT_MS - 1));
    assert!(server.connection.poll(30 + TIME_WAIT_MS));
    assert_eq!(server.connection.state(), State::Closed);
}

#[test]
fn both_sides_closing_at_once_pass_through_closing() {
    let (mut client, mut server) = connected(4096);
    client.connection.close();
    server.connection.close();
    let from_client = client.output(5);
    let from_server = server.output(5);
    for segment in &from_client {
        server.take(segment, CLIENT.0, 5);
    }
    for segment in &from_server {
        client.take(segment, SERVER.0, 5);
    }
    assert_eq!(client.connection.state(), State::Closing);
    exchange(&mut client, &mut server, 6, |_, _| false);
    assert_eq!(client.connection.state(), State::TimeWait);
    assert_eq!(server.connection.state(), State::TimeWait);
}

#[test]
fn a_lost_segment_is_sent_again_after_the_timeout_with_backoff() {
    let (mut client, mut server) = connected(8192);
    client.queues.send.extend_from_slice(b"hello");
    let mut lost = 0;
    exchange(&mut client, &mut server, 0, |from_client, segment| {
        let data = parse(CLIENT.0, SERVER.0, segment).is_some_and(|s| !s.data.is_empty());
        let dropped = from_client && data && lost == 0;
        lost += usize::from(dropped);
        dropped
    });
    assert!(server.queues.received.is_empty());
    let rto = client.connection.rto_ms();
    let due = client.connection.next_poll_ms();
    assert!(!client.connection.poll(due - 1));
    assert!(!client.connection.poll(due));
    assert_eq!(client.connection.rto_ms(), (rto * 2).min(MAX_RTO_MS));
    exchange(&mut client, &mut server, due, |_, _| false);
    assert_eq!(server.queues.received, b"hello");
    assert_eq!(client.connection.next_poll_ms(), u64::MAX);
}

#[test]
fn a_peer_that_stops_answering_times_the_connection_out() {
    let (mut client, _server) = connected(8192);
    client.queues.send.extend_from_slice(b"anyone?");
    let mut now = 0;
    client.output(now);
    for _ in 0..DATA_RETRIES {
        now = client.connection.next_poll_ms();
        assert!(!client.connection.poll(now));
        assert_eq!(client.output(now).len(), 1);
    }
    let now = client.connection.next_poll_ms();
    assert!(client.connection.poll(now));
    assert_eq!(client.connection.state(), State::Closed);
    assert_eq!(client.connection.error(), Some(Error::TimedOut));
    assert!(client.output(now).is_empty());
}

#[test]
fn an_unanswered_syn_is_sent_again_then_given_up() {
    let mut client = End {
        connection: Connection::connect(CLIENT, SERVER, 1000, MSS),
        queues: Queues::with_room(4096),
    };
    assert_eq!(client.output(0).len(), 1);
    for _ in 0..SYN_RETRIES {
        let now = client.connection.next_poll_ms();
        client.connection.poll(now);
        let again = client.output(now);
        assert_eq!(again.len(), 1);
        let segment = parse(CLIENT.0, SERVER.0, &again[0]).unwrap();
        assert_eq!(segment.header.flags, flags::SYN);
        assert_eq!(segment.header.sequence, 1000);
    }
    let now = client.connection.next_poll_ms();
    assert!(client.connection.poll(now));
    assert_eq!(client.connection.error(), Some(Error::TimedOut));
}

#[test]
fn a_full_receiver_closes_its_window_and_the_sender_probes_until_it_opens() {
    let (mut client, mut server) = connected(2000);
    let data: Vec<u8> = (0..6000u32).map(|n| (n % 251) as u8).collect();
    client.queues.send.extend_from_slice(&data);
    exchange(&mut client, &mut server, 0, |_, _| false);
    assert_eq!(server.queues.received.len(), 2000);
    assert_eq!(server.queues.receive_room(), 0);
    let mut now = 0;
    for _ in 0..3 {
        now = client.connection.next_poll_ms();
        assert!(now < u64::MAX, "a probe is due");
        client.connection.poll(now);
        exchange(&mut client, &mut server, now, |_, _| false);
        assert_eq!(server.queues.received.len(), 2000);
    }
    assert_eq!(client.connection.state(), State::Established);
    let mut taken = core::mem::take(&mut server.queues.received);
    while taken.len() < data.len() {
        exchange(&mut client, &mut server, now, |_, _| false);
        taken.append(&mut server.queues.received);
        now += 1;
        assert!(now < 100_000);
    }
    assert_eq!(taken, data);
}

#[test]
fn a_reset_in_sequence_ends_the_connection_and_one_out_of_sequence_is_challenged() {
    let (mut client, mut server) = connected(4096);
    let mut reset = server.connection.clone();
    reset.abort();
    let mut out = [0u8; SEGMENT_ROOM];
    let length = reset.output(0, &server.queues, &mut out).unwrap();
    let mut shifted = out;
    let segment = parse(SERVER.0, CLIENT.0, &out[..length]).unwrap();
    let mut header = segment.header;
    header.sequence = header.sequence.wrapping_add(10);
    write_header(&mut shifted, SERVER.0, CLIENT.0, &header, 0).unwrap();
    client.take(&shifted[..length], SERVER.0, 1);
    assert_eq!(client.connection.state(), State::Established);
    let challenge = client.output(1);
    assert_eq!(challenge.len(), 1);
    assert_eq!(
        parse(CLIENT.0, SERVER.0, &challenge[0])
            .unwrap()
            .header
            .flags,
        flags::ACK
    );
    client.take(&out[..length], SERVER.0, 2);
    assert_eq!(client.connection.state(), State::Closed);
    assert_eq!(client.connection.error(), Some(Error::Reset));
    server.connection.abort();
    assert!(!server.output(3).is_empty());
    assert!(server.connection.is_finished());
}

#[test]
fn a_segment_out_of_order_is_dropped_and_three_duplicate_acks_resend_at_once() {
    let (mut client, mut server) = connected(65_535);
    let data: Vec<u8> = (0..5 * usize::from(MSS)).map(|n| n as u8).collect();
    client.queues.send.extend_from_slice(&data);
    let segments = client.output(0);
    assert_eq!(segments.len(), 5);
    let mut acks = Vec::new();
    for segment in &segments[1..] {
        server.take(segment, CLIENT.0, 1);
        acks.extend(server.output(1));
    }
    assert!(server.queues.received.is_empty());
    assert_eq!(acks.len(), 4);
    for ack in &acks[..3] {
        client.take(ack, SERVER.0, 2);
    }
    let resent = client.output(2);
    assert!(!resent.is_empty());
    let first = parse(CLIENT.0, SERVER.0, &resent[0]).unwrap();
    assert_eq!(first.header.sequence, 1001);
    for segment in &resent {
        server.take(segment, CLIENT.0, 3);
    }
    exchange(&mut client, &mut server, 3, |_, _| false);
    assert_eq!(server.queues.received, data);
}

#[test]
fn a_syn_with_an_ack_or_a_reset_makes_no_connection() {
    let (_, _, syn) = pair(4096);
    let mut segment = parse(CLIENT.0, SERVER.0, &syn).unwrap();
    segment.header.flags |= flags::ACK;
    assert!(Connection::accept(SERVER, CLIENT, &segment, 1, MSS).is_none());
    segment.header.flags = flags::SYN | flags::RST;
    assert!(Connection::accept(SERVER, CLIENT, &segment, 1, MSS).is_none());
    segment.header.flags = flags::SYN;
    segment.header.mss = None;
    let accepted = Connection::accept(SERVER, CLIENT, &segment, 1, MSS).unwrap();
    assert_eq!(accepted.send_mss(), DEFAULT_MSS);
    assert!(accepted.is_connecting());
}

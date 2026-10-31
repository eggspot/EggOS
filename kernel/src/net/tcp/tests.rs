use super::*;
use crate::tests::TestPlatform as P;

const LOOPBACK: Ipv4Address = Ipv4Address::LOOPBACK;

fn read_all(stream: &Stream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut out = [0u8; 512];
    loop {
        let count = stream.read(&mut out, 0, false);
        if count == 0 {
            return bytes;
        }
        bytes.extend_from_slice(&out[..count]);
    }
}

#[test]
fn a_loopback_connection_carries_bytes_both_ways_then_closes_in_order() {
    let listener = Listener::listen((Ipv4Address::UNSPECIFIED, 47_001), 4).unwrap();
    let client = Stream::connect::<P>((LOOPBACK, 47_002), (LOOPBACK, 47_001)).unwrap();
    run::<P>();
    assert_eq!(client.status().state, State::Established);
    let server = listener.take_ready().unwrap();
    assert!(!listener.has_ready());
    assert_eq!(server.remote(), (LOOPBACK, 47_002));
    assert_eq!(server.local(), (LOOPBACK, 47_001));
    assert_eq!(client.write(b"ping"), 4);
    run::<P>();
    let mut out = [0u8; 16];
    assert_eq!(server.read(&mut out, 0, true), 4);
    assert_eq!(server.status().readable, 4);
    assert_eq!(server.read(&mut out, 0, false), 4);
    assert_eq!(&out[..4], b"ping");
    let big: Vec<u8> = (0..20_000u32).map(|n| (n % 253) as u8).collect();
    let mut sent = 0;
    while sent < big.len() {
        sent += server.write(&big[sent..]);
        run::<P>();
    }
    assert_eq!(read_all(&client), big);
    let client_socket = Socket::accepted(client, run::<P>);
    let client = match client_socket.phase() {
        Phase::Connected(stream) => stream,
        other => panic!("{other:?}"),
    };
    drop(client_socket);
    assert!(server.status().read_ended, "the client's FIN came");
    assert_eq!(server.status().state, State::CloseWait);
    server.shut_down_sending();
    run::<P>();
    assert_eq!(server.status().state, State::Closed);
    assert_eq!(client.status().state, State::TimeWait);
}

#[test]
fn a_connection_to_a_port_no_one_listens_on_is_refused_once() {
    let client = Stream::connect::<P>((LOOPBACK, 47_011), (LOOPBACK, 47_010)).unwrap();
    run::<P>();
    let status = client.status();
    assert_eq!(status.state, State::Closed);
    assert_eq!(status.error, Some(Error::Refused));
    assert_eq!(client.take_error(), Some(Error::Refused));
    assert_eq!(client.take_error(), None);
    assert_eq!(client.write(b"x"), 0);
}

#[test]
fn a_full_backlog_drops_syns_and_a_closed_listener_resets_what_waits() {
    let listener = Listener::listen((LOOPBACK, 47_021), 1).unwrap();
    let first = Stream::connect::<P>((LOOPBACK, 47_022), (LOOPBACK, 47_021)).unwrap();
    let second = Stream::connect::<P>((LOOPBACK, 47_023), (LOOPBACK, 47_021)).unwrap();
    run::<P>();
    assert_eq!(first.status().state, State::Established);
    assert_eq!(second.status().state, State::SynSent, "its SYN was dropped");
    let socket = Socket::new(run::<P>);
    assert!(socket.leave_idle(Phase::Listening(listener)));
    assert!(!socket.leave_idle(Phase::Idle));
    drop(socket);
    let status = first.status();
    assert_eq!(status.state, State::Closed);
    assert_eq!(status.error, Some(Error::Reset));
}

#[test]
fn a_syn_from_the_network_makes_a_connection_without_the_adapter_lock_its_receive_path_holds() {
    let listener = Listener::listen((Ipv4Address::UNSPECIFIED, 47_041), 4).unwrap();
    let syn = Segment {
        header: tcp::Header {
            source_port: 47_042,
            destination_port: 47_041,
            sequence: 1_000,
            flags: flags::SYN,
            window: 65_535,
            mss: Some(1_460),
            ..tcp::Header::default()
        },
        data: &[],
    };
    let remote = Ipv4Address::new(10, 0, 2, 2);
    let local = Ipv4Address::new(10, 0, 2, 15);
    let held = crate::net::ADAPTER.lock();
    assert!(deliver::<P>(remote, local, &syn, 1_460));
    drop(held);
    assert_eq!(listener.state.lock().pending, 1);
    listener.close();
}

#[test]
fn closing_with_unread_bytes_resets_the_connection() {
    let listener = Listener::listen((LOOPBACK, 47_031), 4).unwrap();
    let client = Stream::connect::<P>((LOOPBACK, 47_032), (LOOPBACK, 47_031)).unwrap();
    run::<P>();
    let server = listener.take_ready().unwrap();
    client.write(b"never read");
    run::<P>();
    assert_eq!(server.status().readable, 10);
    drop(Socket::accepted(server, run::<P>));
    assert_eq!(client.status().error, Some(Error::Reset));
}

#[test]
fn a_queue_copies_across_its_two_slices_from_any_offset() {
    let mut queue = VecDeque::with_capacity(8);
    queue.extend(b"xxxxxx");
    queue.drain(..5);
    queue.extend(b"abcdef");
    let mut out = [0u8; 4];
    assert_eq!(copy_from(&queue, 2, &mut out), 4);
    assert_eq!(&out, b"bcde");
    assert_eq!(copy_from(&queue, 6, &mut out), 1);
    assert_eq!(copy_from(&queue, 9, &mut out), 0);
}

use super::*;
use crate::linux::descriptors::open_flag;
use alloc::vec;

const ME: PeerCredentials = PeerCredentials {
    pid: 7,
    uid: 1000,
    gid: 100,
};

fn pair() -> (Arc<UnixSocket>, Arc<UnixSocket>) {
    UnixSocket::pair(ME)
}

fn connection_of(socket: &UnixSocket) -> (Arc<Connection>, Side) {
    socket.connected().unwrap()
}

fn console() -> Arc<OpenFile> {
    Arc::new(OpenFile::new(FileKind::Console, open_flag::RDWR))
}

fn socket_file(socket: Arc<UnixSocket>) -> Arc<OpenFile> {
    Arc::new(OpenFile::new(FileKind::Socket(socket), open_flag::RDWR))
}

fn rights(files: Vec<Arc<OpenFile>>) -> Option<OutgoingRights> {
    Some(OutgoingRights::new(files))
}

fn send(socket: &UnixSocket, bytes: &[u8]) -> Result<usize, Errno> {
    let (connection, side) = connection_of(socket);
    connection.send(side, bytes, &mut None)
}

fn receive_all(socket: &UnixSocket) -> Vec<u8> {
    let (connection, side) = connection_of(socket);
    let mut out = Vec::new();
    let mut buffer = [0u8; 1000];
    let mut first = true;
    loop {
        match connection.receive(side, &mut buffer, first) {
            Taken::Bytes { count, .. } => out.extend_from_slice(&buffer[..count]),
            _ => return out,
        }
        first = false;
    }
}

#[test]
fn bytes_come_out_in_order_and_the_queue_stops_at_its_capacity() {
    let _serial = tests_lock();
    let (first, second) = pair();
    let big: Vec<u8> = (0..QUEUE_CAPACITY + 100).map(|index| index as u8).collect();
    assert_eq!(send(&first, &big), Ok(QUEUE_CAPACITY));
    assert_eq!(send(&first, b"more"), Ok(0));
    let (connection, side) = connection_of(&first);
    assert!(!connection.can_send(side));
    assert_eq!(connection.unread_sent(side), QUEUE_CAPACITY);
    let (back, back_side) = connection_of(&second);
    assert_eq!(back.available(back_side), QUEUE_CAPACITY);
    let mut part = vec![0u8; 1000];
    assert!(matches!(
        back.receive(back_side, &mut part, true),
        Taken::Bytes {
            count: 1000,
            rights: None
        }
    ));
    assert_eq!(part, big[..1000]);
    assert_eq!(send(&first, &big[QUEUE_CAPACITY..]), Ok(100));
    let rest = receive_all(&second);
    assert_eq!(rest, big[1000..]);
}

#[test]
fn a_peek_leaves_the_bytes_and_a_read_stops_before_the_next_descriptors() {
    let _serial = tests_lock();
    let (first, second) = pair();
    let (connection, side) = connection_of(&first);
    let passed = console();
    connection.send(side, b"ab", &mut None).unwrap();
    let mut with = rights(vec![Arc::clone(&passed)]);
    assert_eq!(connection.send(side, b"cd", &mut with), Ok(2));
    assert!(with.is_none());
    assert_eq!(in_flight(), 1);
    let (back, back_side) = connection_of(&second);
    let mut buffer = [0u8; 8];
    assert!(matches!(
        back.peek(back_side, 0, &mut buffer, true),
        Taken::Bytes {
            count: 2,
            rights: None
        }
    ));
    assert!(matches!(
        back.peek(back_side, 2, &mut buffer, false),
        Taken::Boundary
    ));
    assert!(matches!(
        back.peek(back_side, 2, &mut buffer, true),
        Taken::Bytes {
            count: 2,
            rights: None
        }
    ));
    assert!(matches!(
        back.receive(back_side, &mut buffer, true),
        Taken::Bytes {
            count: 2,
            rights: None
        }
    ));
    assert_eq!(&buffer[..2], b"ab");
    assert!(matches!(
        back.receive(back_side, &mut buffer, false),
        Taken::Boundary
    ));
    match back.receive(back_side, &mut buffer, true) {
        Taken::Bytes {
            count: 2,
            rights: Some(files),
        } => {
            assert_eq!(&buffer[..2], b"cd");
            assert_eq!(files.len(), 1);
            assert!(Arc::ptr_eq(&files[0], &passed));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(in_flight(), 0);
    assert!(matches!(
        back.receive(back_side, &mut buffer, true),
        Taken::Empty
    ));
}

#[test]
fn closing_ends_the_peer_s_reads_resets_it_after_unread_bytes_and_breaks_its_writes() {
    let _serial = tests_lock();
    let (first, second) = pair();
    send(&first, b"left").unwrap();
    drop(first);
    let (back, side) = connection_of(&second);
    let mut buffer = [0u8; 8];
    assert!(matches!(
        back.receive(side, &mut buffer, true),
        Taken::Bytes { count: 4, .. }
    ));
    assert!(matches!(
        back.receive(side, &mut buffer, true),
        Taken::Ended
    ));
    assert_eq!(send(&second, b"x"), Err(Errno::EPIPE));
    let events = back.events(side, u16::MAX);
    let expected = event::IN | event::RDNORM | event::RDHUP | event::HUP | event::OUT;
    assert_eq!(events & expected, expected);
    assert_eq!(events & event::ERR, 0);

    let (first, second) = pair();
    send(&first, b"unread").unwrap();
    drop(second);
    let (connection, side) = connection_of(&first);
    assert_ne!(connection.events(side, 0) & event::ERR, 0);
    assert!(matches!(
        connection.receive(side, &mut buffer, false),
        Taken::Ended
    ));
    assert!(matches!(
        connection.receive(side, &mut buffer, true),
        Taken::Reset
    ));
    assert!(matches!(
        connection.receive(side, &mut buffer, true),
        Taken::Ended
    ));
    assert!(!connection.take_reset(side));
}

#[test]
fn shutting_down_ends_reads_on_one_side_and_writes_on_the_other() {
    let _serial = tests_lock();
    let (first, second) = pair();
    let (connection, side) = connection_of(&first);
    connection.shutdown(side, false, true);
    assert_eq!(send(&first, b"x"), Err(Errno::EPIPE));
    let (back, back_side) = connection_of(&second);
    let mut buffer = [0u8; 4];
    assert!(matches!(
        back.receive(back_side, &mut buffer, true),
        Taken::Ended
    ));
    let events = back.events(back_side, u16::MAX);
    assert_eq!(
        events & (event::IN | event::RDHUP),
        event::IN | event::RDHUP
    );
    assert_eq!(events & event::HUP, 0);
    assert_eq!(send(&second, b"still"), Ok(5));
    back.shutdown(back_side, true, false);
    assert_eq!(send(&first, b"x"), Err(Errno::EPIPE));
    assert_eq!(send(&second, b"y"), Ok(1));
}

#[test]
fn poll_events_follow_the_state() {
    let _serial = tests_lock();
    let all = u16::MAX;
    let open = UnixSocket::new();
    assert_eq!(open.events(all), event::OUT | event::WRNORM | event::HUP);
    let (first, second) = pair();
    assert_eq!(first.events(all), event::OUT | event::WRNORM);
    send(&first, b"x").unwrap();
    assert_eq!(second.events(event::IN), event::IN);
    assert_eq!(second.events(event::OUT), event::OUT);
}

#[test]
fn a_listener_queues_connections_up_to_its_backlog_and_hands_them_out_in_order() {
    let _serial = tests_lock();
    let listener = UnixSocket::new();
    assert_eq!(listener.listen(1, ME), Err(Errno::EINVAL));
    listener
        .set_bound(Address::Abstract(Arc::from(&b"queue"[..])))
        .unwrap();
    assert_eq!(listener.set_bound(Address::Unnamed), Err(Errno::EINVAL));
    listener.listen(0, ME).unwrap();
    assert_eq!(listener.events(u16::MAX), 0);
    let connecting = || Connection::new(Address::Unnamed, PeerCredentials::NONE);
    let one = connecting();
    assert_eq!(listener.queue(&one), Ok(true));
    let two = connecting();
    assert_eq!(listener.queue(&two), Ok(false));
    assert!(!listener.can_queue());
    assert_eq!(listener.events(event::IN), event::IN);
    listener.listen(1, ME).unwrap();
    assert_eq!(listener.queue(&two), Ok(true));
    let first = listener.accept().unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &one));
    assert_eq!(
        first.address(Side::Second),
        Address::Abstract(Arc::from(&b"queue"[..]))
    );
    assert_eq!(first.credentials(Side::Second), ME);
    assert!(Arc::ptr_eq(&listener.accept().unwrap().unwrap(), &two));
    assert!(listener.accept().unwrap().is_none());
    assert!(UnixSocket::new().accept().is_err());
    assert_eq!(
        UnixSocket::new().queue(&connecting()),
        Err(Errno::ECONNREFUSED)
    );
}

#[test]
fn connections_left_in_a_closed_listener_s_backlog_are_reset() {
    let _serial = tests_lock();
    let listener = UnixSocket::new();
    listener
        .set_bound(Address::Abstract(Arc::from(&b"closing"[..])))
        .unwrap();
    listener.listen(4, ME).unwrap();
    let client = UnixSocket::new();
    let connection = Connection::new(client.begin_connect().unwrap(), ME);
    assert_eq!(listener.queue(&connection), Ok(true));
    client.finish_connect(&connection).unwrap();
    drop(listener);
    let mut buffer = [0u8; 4];
    assert!(matches!(
        connection.receive(Side::First, &mut buffer, true),
        Taken::Reset
    ));
    assert_eq!(send(&client, b"x"), Err(Errno::EPIPE));
}

#[test]
fn a_connecting_socket_can_neither_listen_nor_be_passed_and_a_given_up_connection_resets() {
    let _serial = tests_lock();
    let listener = UnixSocket::new();
    listener
        .set_bound(Address::Abstract(Arc::from(&b"race"[..])))
        .unwrap();
    listener.listen(4, ME).unwrap();
    let racer = UnixSocket::new();
    racer
        .set_bound(Address::Abstract(Arc::from(&b"racer"[..])))
        .unwrap();
    let own = racer.begin_connect().unwrap();
    assert_eq!(racer.begin_connect(), Err(Errno::EALREADY));
    assert_eq!(racer.listen(1, ME), Err(Errno::EINVAL));
    assert_eq!(racer.check_unbound(), Err(Errno::EINVAL));
    let (carrier, _carrier_peer) = pair();
    let (carrying, carrying_side) = connection_of(&carrier);
    let mut passed = rights(vec![socket_file(Arc::clone(&racer))]);
    assert_eq!(
        carrying.send_passing(carrying_side, b"x", &mut passed),
        Err(Errno::ETOOMANYREFS)
    );
    drop(passed);
    let connection = Connection::new(own, ME);
    assert_eq!(listener.queue(&connection), Ok(true));
    racer.abandon_connect();
    assert_eq!(racer.finish_connect(&connection), Err(Errno::EINVAL));
    release_rights(connection.abort(Side::First));
    assert_eq!(racer.listen(1, ME), Ok(()));
    let accepted = UnixSocket::accepted(listener.accept().unwrap().unwrap());
    let (back, side) = connection_of(&accepted);
    let mut buffer = [0u8; 4];
    assert!(matches!(
        back.receive(side, &mut buffer, true),
        Taken::Reset
    ));
    assert_eq!(send(&accepted, b"x"), Err(Errno::EPIPE));
    let other = UnixSocket::new();
    let connection = Connection::new(other.begin_connect().unwrap(), ME);
    other.finish_connect(&connection).unwrap();
    assert_eq!(other.begin_connect(), Err(Errno::EISCONN));
    assert_eq!(other.finish_connect(&connection), Err(Errno::EISCONN));
}

#[test]
fn two_sockets_connecting_to_each_other_at_once_both_get_refused() {
    let _serial = tests_lock();
    let x = UnixSocket::new();
    let y = UnixSocket::new();
    let names = [&b"cross-x"[..], &b"cross-y"[..]];
    for (socket, name) in [(&x, names[0]), (&y, names[1])] {
        let name: Arc<[u8]> = Arc::from(name);
        register(BindingKey::Abstract(Arc::clone(&name)), socket).unwrap();
        socket.set_bound(Address::Abstract(name)).unwrap();
    }
    let attempt = |socket: Arc<UnixSocket>, target: &'static [u8]| {
        std::thread::spawn(move || {
            for _ in 0..1000 {
                let own = socket.begin_connect().unwrap();
                let target = find(&BindingKey::Abstract(Arc::from(target))).unwrap();
                let connection = Connection::new(own, ME);
                assert_eq!(target.queue(&connection), Err(Errno::ECONNREFUSED));
                assert!(target.can_queue());
                socket.abandon_connect();
            }
        })
    };
    let first = attempt(Arc::clone(&x), b"cross-y");
    let second = attempt(Arc::clone(&y), b"cross-x");
    first.join().unwrap();
    second.join().unwrap();
}

#[test]
fn the_registry_exists_only_while_something_is_bound_and_refuses_a_taken_name() {
    let _serial = tests_lock();
    assert!(!has_bindings());
    let socket = UnixSocket::new();
    let key = BindingKey::Abstract(Arc::from(&b"taken"[..]));
    register(key.clone(), &socket).unwrap();
    assert!(has_bindings());
    let other = UnixSocket::new();
    assert_eq!(register(key.clone(), &other), Err(Errno::EADDRINUSE));
    assert!(Arc::ptr_eq(&find(&key).unwrap(), &socket));
    let first = register_autobound(&other).unwrap();
    let second = register_autobound(&other).unwrap();
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(u8::is_ascii_hexdigit));
    assert_ne!(first, second);
    unregister_key(&key, &other);
    assert!(find(&key).is_some());
    drop(socket);
    assert!(find(&key).is_none());
    register(key.clone(), &other).unwrap();
    drop(other);
    assert!(!has_bindings());
}

#[test]
fn proc_net_unix_lists_bound_sockets_in_linux_s_format() {
    let _serial = tests_lock();
    let empty = proc_net_unix().unwrap();
    assert_eq!(
        empty,
        b"Num       RefCount Protocol Flags    Type St Inode Path\n"
    );
    let socket = UnixSocket::new();
    let name: Arc<[u8]> = Arc::from(&b"listed"[..]);
    register(BindingKey::Abstract(Arc::clone(&name)), &socket).unwrap();
    socket.set_bound(Address::Abstract(name)).unwrap();
    socket.listen(5, ME).unwrap();
    let text = alloc::string::String::from_utf8(proc_net_unix().unwrap()).unwrap();
    let line = text.lines().nth(1).unwrap();
    assert!(line.starts_with("0000000000000000: 00000002 00000000 00010000 0001 01 "));
    assert!(line.ends_with(" @listed"));
    assert!(line.contains(&alloc::format!("{:5}", socket.inode())));
}

#[test]
fn passing_a_socket_that_would_make_a_cycle_is_refused_and_every_socket_is_freed() {
    let _serial = tests_lock();
    let (a, a_peer) = pair();
    let (connection_a, side_a) = connection_of(&a);
    let a_file = socket_file(Arc::clone(&a));
    let mut own = rights(vec![Arc::clone(&a_file)]);
    assert_eq!(
        connection_a.send_passing(side_a, b"x", &mut own),
        Err(Errno::ETOOMANYREFS)
    );
    let peer_file = socket_file(Arc::clone(&a_peer));
    let mut peer = rights(vec![peer_file]);
    assert_eq!(
        connection_a.send_passing(side_a, b"x", &mut peer),
        Err(Errno::ETOOMANYREFS)
    );

    let (c, c_peer) = pair();
    let (connection_c, side_c) = connection_of(&c);
    let mut a_into_c = rights(vec![Arc::clone(&a_file)]);
    assert_eq!(
        connection_c.send_passing(side_c, b"x", &mut a_into_c),
        Ok(1)
    );
    let c_file = socket_file(Arc::clone(&c));
    let mut c_into_a = rights(vec![Arc::clone(&c_file)]);
    assert_eq!(
        connection_a.send_passing(side_a, b"x", &mut c_into_a),
        Err(Errno::ETOOMANYREFS)
    );

    let listener = UnixSocket::new();
    listener
        .set_bound(Address::Abstract(Arc::from(&b"passed"[..])))
        .unwrap();
    listener.listen(4, ME).unwrap();
    let waiting = Connection::new(Address::Unnamed, ME);
    listener.queue(&waiting).unwrap();
    let mut busy = rights(vec![socket_file(Arc::clone(&listener))]);
    let (e, _e_peer) = pair();
    let (connection_e, side_e) = connection_of(&e);
    assert_eq!(
        connection_e.send_passing(side_e, b"x", &mut busy),
        Err(Errno::ETOOMANYREFS)
    );
    drop(busy);

    let sockets = [&a, &a_peer, &c, &c_peer, &listener].map(Arc::downgrade);
    drop((own, peer, a_into_c, c_into_a));
    drop((a_file, c_file, connection_a, connection_c, waiting));
    drop((a, a_peer, c, c_peer, listener));
    assert!(sockets.iter().all(|socket| socket.strong_count() == 0));
    assert_eq!(in_flight(), 0);
}

#[test]
fn a_long_chain_of_passed_sockets_is_freed_without_recursing() {
    let _serial = tests_lock();
    const CHAIN: usize = MAX_IN_FLIGHT as usize;
    let pairs: Vec<(Arc<UnixSocket>, Arc<UnixSocket>)> = (0..CHAIN).map(|_| pair()).collect();
    let watched: Vec<Weak<UnixSocket>> = pairs
        .iter()
        .map(|(first, _)| Arc::downgrade(first))
        .collect();
    let mut files: Vec<Option<Arc<OpenFile>>> = pairs
        .iter()
        .map(|(first, _)| Some(socket_file(Arc::clone(first))))
        .collect();
    for index in 0..CHAIN - 1 {
        let (connection, side) = connection_of(&pairs[index].1);
        let next = files[index + 1].take().unwrap();
        let mut carried = rights(vec![next]);
        assert_eq!(connection.send_passing(side, b"x", &mut carried), Ok(1));
    }
    assert_eq!(in_flight(), (CHAIN - 1) as u32);
    let head = files[0].take().unwrap();
    let seconds: Vec<Arc<UnixSocket>> =
        pairs.iter().map(|(_, second)| Arc::clone(second)).collect();
    drop(pairs);
    drop(seconds);
    assert!(watched[CHAIN - 1].strong_count() > 0);
    drop(head);
    assert!(watched.iter().all(|socket| socket.strong_count() == 0));
    assert_eq!(in_flight(), 0);
}

#[test]
fn descriptors_in_flight_are_capped_in_all_and_per_direction() {
    let _serial = tests_lock();
    let pairs: Vec<_> = (0..5).map(|_| pair()).collect();
    let many = |count: usize| rights((0..count).map(|_| console()).collect());
    let (connection, side) = connection_of(&pairs[0].0);
    assert_eq!(
        connection.send(side, b"x", &mut many(MAX_QUEUED_RIGHTS + 1)),
        Err(Errno::ETOOMANYREFS)
    );
    for (first, _) in &pairs[..4] {
        let (connection, side) = connection_of(first);
        assert_eq!(
            connection.send(side, b"x", &mut many(MAX_QUEUED_RIGHTS)),
            Ok(1)
        );
    }
    assert_eq!(in_flight(), MAX_IN_FLIGHT);
    let (last, last_side) = connection_of(&pairs[4].0);
    assert_eq!(
        last.send(last_side, b"x", &mut many(1)),
        Err(Errno::ETOOMANYREFS)
    );
    let (back, back_side) = connection_of(&pairs[0].1);
    let mut buffer = [0u8; 1];
    let taken = back.receive(back_side, &mut buffer, true);
    assert!(
        matches!(&taken, Taken::Bytes { rights: Some(files), .. } if files.len() == MAX_QUEUED_RIGHTS)
    );
    drop(taken);
    assert_eq!(last.send(last_side, b"x", &mut many(1)), Ok(1));
    drop(pairs);
    drop((connection, last, back));
    assert_eq!(in_flight(), 0);
}

#[test]
fn a_queue_grows_only_within_every_queue_s_limit_and_shrinks_once_read() {
    let mut direction = Direction::default();
    assert_eq!(direction.make_room(10, 0), Err(Errno::ENOMEM));
    assert_eq!(
        direction.bytes.capacity(),
        0,
        "a refused growth takes nothing"
    );
    direction.make_room(3 * WRITE_ROOM, u64::MAX).unwrap();
    let grown = direction.bytes.capacity();
    assert!(grown >= 3 * WRITE_ROOM);
    direction.bytes.extend(&vec![7u8; 3 * WRITE_ROOM]);
    let spare = grown - direction.bytes.len();
    assert!(direction.can_take(1, u64::MAX));
    if spare == 0 {
        assert!(
            !direction.can_take(1, 0),
            "full, and every queue at its cap"
        );
    }
    assert_eq!(
        direction.make_room(spare, 0),
        Ok(()),
        "room the buffer has needs no growth"
    );
    direction.bytes.drain(..WRITE_ROOM);
    direction.shrink_when_empty();
    assert_eq!(direction.bytes.capacity(), grown, "not empty yet");
    direction.bytes.drain(..);
    direction.shrink_when_empty();
    assert!(direction.bytes.capacity() < grown);
    assert!(direction.bytes.capacity() >= WRITE_ROOM.min(grown));
    let rights = direction.clear();
    assert!(rights.is_empty());
    assert_eq!(direction.bytes.capacity(), 0);
}

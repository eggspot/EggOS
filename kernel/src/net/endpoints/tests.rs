use super::*;

fn udp(source: Ipv4Address, source_port: u16, port: u16, data: &[u8]) -> Delivery<'_> {
    Delivery::Udp(Arrived {
        source,
        source_port,
        destination: Ipv4Address::new(10, 0, 2, 15),
        destination_port: port,
        data,
    })
}

const HOST: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

#[test]
fn the_kernel_picks_free_ports_and_a_taken_port_is_refused() {
    let first = Endpoint::new(Protocol::Udp);
    let second = Endpoint::new(Protocol::Udp);
    let one = first.bind(Ipv4Address::UNSPECIFIED, 0).unwrap();
    let two = second.bind(Ipv4Address::UNSPECIFIED, 0).unwrap();
    assert!(EPHEMERAL_PORTS.contains(&one) && EPHEMERAL_PORTS.contains(&two));
    assert_ne!(one, two);
    assert_eq!(
        first.bind(Ipv4Address::UNSPECIFIED, 0),
        Err(BindError::AlreadyBound)
    );
    let third = Endpoint::new(Protocol::Udp);
    assert_eq!(
        third.bind(Ipv4Address::LOOPBACK, one),
        Err(BindError::InUse)
    );
    let ping = Endpoint::new(Protocol::Ping);
    assert_eq!(
        ping.bind(Ipv4Address::UNSPECIFIED, one),
        Ok(one),
        "ping identifiers are apart"
    );
    let specific = Endpoint::new(Protocol::Udp);
    assert_eq!(specific.bind(Ipv4Address::LOOPBACK, 5101), Ok(5101));
    let other_address = Endpoint::new(Protocol::Udp);
    assert_eq!(
        other_address.bind(Ipv4Address::new(10, 0, 2, 15), 5101),
        Ok(5101)
    );
    drop(specific);
    let again = Endpoint::new(Protocol::Udp);
    assert_eq!(
        again.bind(Ipv4Address::LOOPBACK, 5101),
        Ok(5101),
        "a dropped endpoint frees its port"
    );
}

#[test]
fn datagrams_reach_the_endpoint_of_their_port_and_a_connected_one_hears_only_its_peer() {
    let endpoint = Endpoint::new(Protocol::Udp);
    endpoint.bind(Ipv4Address::UNSPECIFIED, 5102).unwrap();
    assert!(deliver(udp(HOST, 53, 5102, b"one")));
    assert!(!deliver(udp(HOST, 53, 5103, b"nobody")));
    assert_eq!(endpoint.next_length(), Some(3));
    let peeked = endpoint.take(true).unwrap();
    assert_eq!(endpoint.take(false), Some(peeked.clone()));
    assert_eq!(
        peeked,
        Datagram {
            source: HOST,
            source_port: 53,
            bytes: b"one".to_vec()
        }
    );
    assert_eq!(endpoint.take(false), None);
    endpoint.connect(Some((HOST, 53)));
    assert!(!deliver(udp(HOST, 54, 5102, b"other port")));
    assert!(!deliver(udp(
        Ipv4Address::new(10, 0, 2, 3),
        53,
        5102,
        b"other host"
    )));
    assert!(deliver(udp(HOST, 53, 5102, b"peer")));
    endpoint.shut_down(true, false);
    assert!(!deliver(udp(HOST, 53, 5102, b"after shutdown")));
    assert!(endpoint.is_read_closed() && !endpoint.is_write_closed());
}

#[test]
fn a_full_queue_drops_and_counts_what_does_not_fit() {
    let endpoint = Endpoint::new(Protocol::Udp);
    endpoint.bind(Ipv4Address::LOOPBACK, 5104).unwrap();
    endpoint.set_receive_limit(0);
    assert_eq!(endpoint.receive_limit(), MIN_RECEIVE_LIMIT);
    let data = [7u8; 1000];
    let to_loopback = |data| {
        deliver(Delivery::Udp(Arrived {
            source: Ipv4Address::LOOPBACK,
            source_port: 9,
            destination: Ipv4Address::LOOPBACK,
            destination_port: 5104,
            data,
        }))
    };
    assert!(to_loopback(&data[..]));
    assert!(to_loopback(&data[..]));
    assert!(!to_loopback(&data[..]));
    assert_eq!(endpoint.dropped(), 1);
    endpoint.take(false).unwrap();
    assert!(to_loopback(&data[..]));
}

#[test]
fn echo_replies_go_to_the_ping_endpoint_with_their_identifier() {
    let ping = Endpoint::new(Protocol::Ping);
    ping.bind(Ipv4Address::UNSPECIFIED, 5105).unwrap();
    let reply = Delivery::EchoReply {
        source: HOST,
        identifier: 5105,
        message: b"\0\0\0\0\x13\xf1\0\x01data",
    };
    assert!(deliver(reply));
    assert!(!deliver(Delivery::EchoReply {
        source: HOST,
        identifier: 5106,
        message: b"stray",
    }));
    let datagram = ping.take(false).unwrap();
    assert_eq!((datagram.source, datagram.source_port), (HOST, 0));
    assert_eq!(&datagram.bytes[8..], b"data");
}

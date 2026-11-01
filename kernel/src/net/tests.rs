use super::endpoints::{Endpoint, Protocol};
use super::*;
use crate::tests::TestPlatform;

#[test]
fn a_datagram_to_the_loopback_network_reaches_its_endpoint_at_once() {
    let receiver = Endpoint::new(Protocol::Udp);
    receiver.bind(Ipv4Address::LOOPBACK, 5201).unwrap();
    let sender = Endpoint::new(Protocol::Udp);
    let port = sender.bind(Ipv4Address::UNSPECIFIED, 0).unwrap();
    send_udp::<TestPlatform>(
        Ipv4Address::UNSPECIFIED,
        port,
        (Ipv4Address::LOOPBACK, 5201),
        b"hello",
        ipv4::DEFAULT_TTL,
    )
    .unwrap();
    let datagram = receiver.take(false).unwrap();
    assert_eq!(
        (datagram.source, datagram.source_port, &datagram.bytes[..]),
        (Ipv4Address::LOOPBACK, port, &b"hello"[..])
    );
    assert_eq!(
        send_udp::<TestPlatform>(
            Ipv4Address::UNSPECIFIED,
            port,
            (Ipv4Address::new(10, 0, 2, 2), 7),
            b"x",
            64
        ),
        Err(SendError::NoRoute),
        "without an adapter only this machine is reachable"
    );
}

#[test]
fn a_ping_to_the_loopback_network_is_answered_at_once() {
    let ping = Endpoint::new(Protocol::Ping);
    let identifier = ping.bind(Ipv4Address::UNSPECIFIED, 5202).unwrap();
    send_echo::<TestPlatform>(Ipv4Address::LOOPBACK, identifier, 9, b"abc", 64).unwrap();
    let reply = ping.take(false).unwrap();
    assert_eq!(reply.source, Ipv4Address::LOOPBACK);
    let echo = icmp::parse_echo(&reply.bytes).unwrap();
    assert_eq!(
        (echo.kind, echo.identifier, echo.sequence, echo.data),
        (icmp::kind::ECHO_REPLY, identifier, 9, &b"abc"[..])
    );
}

#[test]
fn without_an_adapter_only_lo_is_reported() {
    let interfaces = interfaces::<TestPlatform>();
    assert_eq!(interfaces.len(), 1);
    assert!(interfaces[0].is_loopback());
    let text = interfaces_text::<TestPlatform>().unwrap();
    assert!(text.starts_with(b"lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - "));
    let routes = routes_text::<TestPlatform>().unwrap();
    assert_eq!(
        routes
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        1
    );
    assert_eq!(
        neighbours_text::<TestPlatform>().unwrap(),
        b"IP address       HW type     Flags       HW address            Mask     Device\n"
    );
    assert!(!is_present());
    assert!(is_local(Ipv4Address::new(127, 0, 0, 53)));
    assert!(!is_local(Ipv4Address::new(10, 0, 2, 15)));
}

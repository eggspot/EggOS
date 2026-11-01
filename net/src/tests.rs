use crate::dhcp::server::{Offered, request_type, write_answer};
use crate::dhcp::{self, Event, message_type};
use crate::ethernet::{self, MacAddress, ether_type};
use crate::interface::{Datagram, Delivery, Interface, SendError};
use crate::ipv4::{self, Ipv4Address};
use crate::status::Dhcp;
use crate::{arp, icmp, tcp, udp};

const OURS: MacAddress = MacAddress([0x52, 0x54, 0, 0x12, 0x34, 0x56]);
const ROUTER_MAC: MacAddress = MacAddress([0x52, 0x55, 0x0a, 0, 2, 2]);
const DNS_MAC: MacAddress = MacAddress([0x52, 0x55, 0x0a, 0, 2, 3]);

fn address(last: u8) -> Ipv4Address {
    Ipv4Address::new(10, 0, 2, last)
}

fn offered() -> Offered {
    Offered {
        address: address(15),
        mask: Ipv4Address::new(255, 255, 255, 0),
        router: address(2),
        dns: address(3),
        server: address(2),
        lease_s: 86_400,
    }
}

fn ip_frame(from: MacAddress, to: MacAddress, header: ipv4::Header, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; ethernet::HEADER_BYTES + ipv4::HEADER_BYTES + payload.len()];
    ethernet::write_header(
        &mut frame,
        &ethernet::Header {
            destination: to,
            source: from,
            ether_type: ether_type::IPV4,
        },
    );
    ipv4::write_header(
        &mut frame[ethernet::HEADER_BYTES..],
        &header,
        1,
        payload.len(),
    )
    .unwrap();
    frame[ethernet::HEADER_BYTES + ipv4::HEADER_BYTES..].copy_from_slice(payload);
    frame
}

fn udp_frame(
    from: (MacAddress, Ipv4Address, u16),
    to: (MacAddress, Ipv4Address, u16),
    data: &[u8],
) -> Vec<u8> {
    let mut datagram = vec![0u8; udp::HEADER_BYTES + data.len()];
    datagram[udp::HEADER_BYTES..].copy_from_slice(data);
    let header = udp::Header {
        source_port: from.2,
        destination_port: to.2,
    };
    udp::write_header(&mut datagram, from.1, to.1, header, data.len()).unwrap();
    let ip = ipv4::Header {
        source: from.1,
        destination: to.1,
        protocol: ipv4::protocol::UDP,
        ttl: 64,
    };
    ip_frame(from.0, to.0, ip, &datagram)
}

fn arp_frame(
    operation: u16,
    sender: (MacAddress, Ipv4Address),
    target: (MacAddress, Ipv4Address),
    to: MacAddress,
) -> Vec<u8> {
    let mut frame = vec![0u8; ethernet::MIN_FRAME_BYTES];
    ethernet::write_header(
        &mut frame,
        &ethernet::Header {
            destination: to,
            source: sender.0,
            ether_type: ether_type::ARP,
        },
    );
    arp::write(
        &mut frame[ethernet::HEADER_BYTES..],
        &arp::Packet {
            operation,
            sender_mac: sender.0,
            sender_ip: sender.1,
            target_mac: target.0,
            target_ip: target.1,
        },
    );
    frame
}

struct Sent(Vec<Vec<u8>>);

impl Sent {
    fn transmit(&mut self) -> impl FnMut(&[u8]) + '_ {
        move |frame: &[u8]| self.0.push(frame.to_vec())
    }
}

fn nothing(_: Delivery<'_>) {
    panic!("nothing is delivered here");
}

fn udp_of(frame: &[u8]) -> (ethernet::Header, ipv4::Header, udp::Header, Vec<u8>) {
    let (ethernet, payload) = ethernet::parse(frame).unwrap();
    let (ip, body) = ipv4::parse(payload).unwrap();
    let (header, data) = udp::parse(ip.source, ip.destination, body).unwrap();
    (ethernet, ip, header, data.to_vec())
}

fn configured() -> Interface {
    let mut interface = Interface::new(OURS, 1500, 99);
    assert_eq!(interface.dhcp(), Dhcp::Asking);
    let mut sent = Sent(Vec::new());
    assert_eq!(interface.poll(0, &mut sent.transmit()), Event::Nothing);
    let (ethernet, ip, header, discover) = udp_of(&sent.0[0]);
    assert_eq!(ethernet.destination, MacAddress::BROADCAST);
    assert_eq!(
        (ip.source, ip.destination),
        (Ipv4Address::UNSPECIFIED, Ipv4Address::BROADCAST)
    );
    assert_eq!((header.source_port, header.destination_port), (68, 67));
    assert_eq!(request_type(&discover), message_type::DISCOVER);
    let answer = |request: &[u8], kind: u8| {
        let mut message = [0u8; dhcp::MESSAGE_BYTES];
        let length = write_answer(&mut message, request, kind, &offered()).unwrap();
        udp_frame(
            (ROUTER_MAC, address(2), 67),
            (MacAddress::BROADCAST, Ipv4Address::BROADCAST, 68),
            &message[..length],
        )
    };
    let offer = answer(&discover, message_type::OFFER);
    assert_eq!(
        interface.receive(&offer, 10, &mut sent.transmit(), &mut nothing),
        Event::Nothing
    );
    interface.poll(10, &mut sent.transmit());
    let (_, _, _, request) = udp_of(&sent.0[1]);
    assert_eq!(request_type(&request), message_type::REQUEST);
    let ack = answer(&request, message_type::ACK);
    assert_eq!(
        interface.receive(&ack, 20, &mut sent.transmit(), &mut nothing),
        Event::Bound
    );
    assert_eq!(interface.address(), Some(address(15)));
    assert_eq!(interface.dhcp(), Dhcp::Configured);
    interface
}

#[test]
fn dhcp_configures_the_interface_from_broadcast_answers() {
    let interface = configured();
    let lease = interface.lease().unwrap();
    assert_eq!(lease.router, Some(address(2)));
    assert_eq!(interface.counters().sent_packets, 2);
    assert_eq!(interface.counters().received_packets, 2);
}

#[test]
fn arp_requests_for_our_address_are_answered_and_the_asker_remembered() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    let request = arp_frame(
        arp::operation::REQUEST,
        (ROUTER_MAC, address(2)),
        (MacAddress::ZERO, address(15)),
        MacAddress::BROADCAST,
    );
    interface.receive(&request, 30, &mut sent.transmit(), &mut nothing);
    assert_eq!(sent.0.len(), 1);
    let (ethernet, payload) = ethernet::parse(&sent.0[0]).unwrap();
    assert_eq!(ethernet.destination, ROUTER_MAC);
    let reply = arp::parse(payload).unwrap();
    assert_eq!(reply.operation, arp::operation::REPLY);
    assert_eq!((reply.sender_mac, reply.sender_ip), (OURS, address(15)));
    assert_eq!(
        interface.neighbours(30).collect::<Vec<_>>(),
        [(address(2), ROUTER_MAC)]
    );
    let for_someone_else = arp_frame(
        arp::operation::REQUEST,
        (ROUTER_MAC, address(2)),
        (MacAddress::ZERO, address(99)),
        MacAddress::BROADCAST,
    );
    interface.receive(&for_someone_else, 31, &mut sent.transmit(), &mut nothing);
    assert_eq!(sent.0.len(), 1);
}

#[test]
fn a_ping_to_our_address_is_answered_and_echo_replies_are_delivered() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    let mut echo = [0u8; icmp::HEADER_BYTES + 4];
    echo[icmp::HEADER_BYTES..].copy_from_slice(b"egg!");
    icmp::write_echo_header(&mut echo, icmp::kind::ECHO_REQUEST, 0x77, 3, 4).unwrap();
    let ping = ip_frame(
        ROUTER_MAC,
        OURS,
        ipv4::Header {
            source: address(2),
            destination: address(15),
            protocol: ipv4::protocol::ICMP,
            ttl: 64,
        },
        &echo,
    );
    interface.receive(&ping, 40, &mut sent.transmit(), &mut nothing);
    assert_eq!(
        sent.0.len(),
        1,
        "the reply waits for the asker's hardware address"
    );
    let (ethernet, _) = ethernet::parse(&sent.0[0]).unwrap();
    assert_eq!(ethernet.ether_type, ether_type::ARP);
    let answer = arp_frame(
        arp::operation::REPLY,
        (ROUTER_MAC, address(2)),
        (OURS, address(15)),
        OURS,
    );
    interface.receive(&answer, 41, &mut sent.transmit(), &mut nothing);
    assert_eq!(sent.0.len(), 2);
    let (ethernet, payload_two) = ethernet::parse(&sent.0[1]).unwrap();
    assert_eq!(ethernet.destination, ROUTER_MAC);
    let (ip, body) = ipv4::parse(payload_two).unwrap();
    assert_eq!((ip.source, ip.destination), (address(15), address(2)));
    let reply = icmp::parse_echo(body).unwrap();
    assert_eq!(
        (reply.kind, reply.identifier, reply.sequence, reply.data),
        (icmp::kind::ECHO_REPLY, 0x77, 3, &b"egg!"[..])
    );
    let mut replied = echo;
    icmp::write_echo_header(&mut replied, icmp::kind::ECHO_REPLY, 0x77, 3, 4).unwrap();
    let pong = ip_frame(
        ROUTER_MAC,
        OURS,
        ipv4::Header {
            source: address(2),
            destination: address(15),
            protocol: ipv4::protocol::ICMP,
            ttl: 63,
        },
        &replied,
    );
    let mut delivered = Vec::new();
    interface.receive(&pong, 42, &mut sent.transmit(), &mut |delivery| {
        if let Delivery::EchoReply {
            source,
            identifier,
            message,
        } = delivery
        {
            delivered.push((source, identifier, message.to_vec()));
        }
    });
    assert_eq!(delivered, [(address(2), 0x77, replied.to_vec())]);
}

#[test]
fn udp_goes_on_the_network_directly_and_off_it_through_the_router_after_arp() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    interface
        .send_udp(
            40_000,
            address(3),
            53,
            b"query",
            64,
            50,
            &mut sent.transmit(),
        )
        .unwrap();
    assert_eq!(sent.0.len(), 1);
    let (_, payload) = ethernet::parse(&sent.0[0]).unwrap();
    assert_eq!(arp::parse(payload).unwrap().target_ip, address(3));
    let answer = arp_frame(
        arp::operation::REPLY,
        (DNS_MAC, address(3)),
        (OURS, address(15)),
        OURS,
    );
    interface.receive(&answer, 51, &mut sent.transmit(), &mut nothing);
    assert_eq!(sent.0.len(), 2);
    let (ethernet, ip, header, data) = udp_of(&sent.0[1]);
    assert_eq!(ethernet.destination, DNS_MAC);
    assert_eq!((ip.source, ip.destination), (address(15), address(3)));
    assert_eq!((header.source_port, header.destination_port), (40_000, 53));
    assert_eq!(data, b"query");
    let router_answer = arp_frame(
        arp::operation::REPLY,
        (ROUTER_MAC, address(2)),
        (OURS, address(15)),
        OURS,
    );
    interface
        .send_udp(
            40_001,
            Ipv4Address::new(192, 0, 2, 1),
            7,
            b"far",
            64,
            52,
            &mut sent.transmit(),
        )
        .unwrap();
    assert_eq!(
        arp::parse(ethernet::parse(&sent.0[2]).unwrap().1)
            .unwrap()
            .target_ip,
        address(2)
    );
    interface.receive(&router_answer, 53, &mut sent.transmit(), &mut nothing);
    let (ethernet, ip, _, _) = udp_of(&sent.0[3]);
    assert_eq!(ethernet.destination, ROUTER_MAC);
    assert_eq!(ip.destination, Ipv4Address::new(192, 0, 2, 1));
    interface
        .send_udp(
            40_002,
            Ipv4Address::new(192, 0, 2, 2),
            7,
            b"again",
            64,
            54,
            &mut sent.transmit(),
        )
        .unwrap();
    assert_eq!(
        udp_of(&sent.0[4]).0.destination,
        ROUTER_MAC,
        "the router is known now"
    );
}

#[test]
fn datagrams_for_us_are_delivered_and_frames_for_others_are_dropped() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    let datagram = udp_frame(
        (DNS_MAC, address(3), 53),
        (OURS, address(15), 40_000),
        b"answer",
    );
    let mut delivered = Vec::new();
    interface.receive(&datagram, 60, &mut sent.transmit(), &mut |delivery| {
        if let Delivery::Udp(Datagram {
            source,
            source_port,
            destination_port,
            data,
            ..
        }) = delivery
        {
            delivered.push((source, source_port, destination_port, data.to_vec()));
        }
    });
    assert_eq!(delivered, [(address(3), 53, 40_000, b"answer".to_vec())]);
    let elsewhere = udp_frame(
        (DNS_MAC, address(3), 53),
        (MacAddress([2, 0, 0, 0, 0, 9]), address(9), 40_000),
        b"not ours",
    );
    let dropped = interface.counters().received_dropped;
    interface.receive(&elsewhere, 61, &mut sent.transmit(), &mut |_| {
        panic!("not ours")
    });
    assert_eq!(interface.counters().received_dropped, dropped + 1);
    let mut damaged = datagram.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    let errors = interface.counters().received_errors;
    interface.receive(&damaged, 62, &mut sent.transmit(), &mut |_| {
        panic!("damaged")
    });
    assert_eq!(interface.counters().received_errors, errors + 1);
}

#[test]
fn unanswered_arp_is_asked_three_times_then_the_packet_is_dropped() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    interface
        .send_udp(1, address(9), 9, b"x", 64, 100, &mut sent.transmit())
        .unwrap();
    assert_eq!(interface.next_poll_ms(), 1100);
    interface.poll(1099, &mut sent.transmit());
    assert_eq!(sent.0.len(), 1);
    interface.poll(1100, &mut sent.transmit());
    interface.poll(2100, &mut sent.transmit());
    assert_eq!(sent.0.len(), 3);
    interface.poll(3100, &mut sent.transmit());
    assert_eq!(sent.0.len(), 3);
    assert_eq!(interface.counters().sent_dropped, 1);
}

#[test]
fn sending_without_an_address_a_link_or_room_fails_with_the_reason() {
    let mut interface = Interface::new(OURS, 1500, 1);
    let mut sent = Sent(Vec::new());
    assert_eq!(
        interface.send_udp(1, address(2), 9, b"x", 64, 0, &mut sent.transmit()),
        Err(SendError::NoAddress)
    );
    assert_eq!(
        interface.send_udp(
            1,
            Ipv4Address::BROADCAST,
            9,
            b"x",
            64,
            0,
            &mut sent.transmit()
        ),
        Ok(())
    );
    let (_, ip, _, _) = udp_of(&sent.0[0]);
    assert_eq!(ip.source, Ipv4Address::UNSPECIFIED);
    let mut interface = configured();
    assert_eq!(
        interface.send_udp(1, address(2), 9, &[0; 1473], 64, 0, &mut sent.transmit()),
        Err(SendError::TooLong)
    );
    assert_eq!(
        interface.send_udp(
            1,
            Ipv4Address::new(224, 0, 0, 251),
            9,
            b"x",
            64,
            0,
            &mut sent.transmit()
        ),
        Err(SendError::NoRoute)
    );
    interface.set_link(false, 10);
    assert_eq!(
        interface.send_udp(1, address(2), 9, b"x", 64, 10, &mut sent.transmit()),
        Err(SendError::LinkDown)
    );
    interface.set_link(true, 20);
    let before = sent.0.len();
    interface.poll(20, &mut sent.transmit());
    let (_, ip, _, request) = udp_of(&sent.0[before]);
    assert_eq!(
        ip.destination,
        Ipv4Address::BROADCAST,
        "a link back rebinds"
    );
    assert_eq!(request_type(&request), message_type::REQUEST);
}

#[test]
fn tcp_segments_to_our_address_are_delivered_and_ours_go_out_through_arp() {
    let mut interface = configured();
    let mut sent = Sent(Vec::new());
    assert_eq!(interface.tcp_mss(), 1460);
    let header = tcp::Header {
        source_port: 80,
        destination_port: 40_000,
        sequence: 9,
        flags: tcp::flags::SYN,
        window: 1024,
        ..tcp::Header::default()
    };
    let mut segment = [0u8; tcp::HEADER_BYTES + 2];
    segment[tcp::HEADER_BYTES..].copy_from_slice(b"hi");
    tcp::write_header(&mut segment, address(2), address(15), &header, 2).unwrap();
    let ip = |destination| ipv4::Header {
        source: address(2),
        destination,
        protocol: ipv4::protocol::TCP,
        ttl: 64,
    };
    let frame = ip_frame(ROUTER_MAC, OURS, ip(address(15)), &segment);
    let mut delivered = Vec::new();
    interface.receive(&frame, 70, &mut sent.transmit(), &mut |delivery| {
        if let Delivery::Tcp {
            source,
            destination,
            segment,
        } = delivery
        {
            delivered.push((source, destination, segment.header, segment.data.to_vec()));
        }
    });
    assert_eq!(
        delivered,
        [(address(2), address(15), header, b"hi".to_vec())]
    );
    let broadcast = ip_frame(ROUTER_MAC, OURS, ip(Ipv4Address::BROADCAST), &segment);
    interface.receive(&broadcast, 71, &mut sent.transmit(), &mut |_| {
        panic!("not unicast")
    });
    let mut damaged = frame.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    let errors = interface.counters().received_errors;
    interface.receive(&damaged, 72, &mut sent.transmit(), &mut |_| {
        panic!("damaged")
    });
    assert_eq!(interface.counters().received_errors, errors + 1);
    let mut reply = [0u8; tcp::HEADER_BYTES];
    let reply_header = tcp::Header {
        source_port: 40_000,
        destination_port: 80,
        flags: tcp::flags::RST,
        ..tcp::Header::default()
    };
    tcp::write_header(&mut reply, address(15), address(2), &reply_header, 0).unwrap();
    let router_answer = arp_frame(
        arp::operation::REPLY,
        (ROUTER_MAC, address(2)),
        (OURS, address(15)),
        OURS,
    );
    interface.receive(&router_answer, 73, &mut sent.transmit(), &mut nothing);
    interface
        .send_tcp(address(2), &reply, 64, 73, &mut sent.transmit())
        .unwrap();
    let (ethernet, payload) = ethernet::parse(sent.0.last().unwrap()).unwrap();
    assert_eq!(ethernet.destination, ROUTER_MAC);
    let (ip, body) = ipv4::parse(payload).unwrap();
    assert_eq!(ip.protocol, ipv4::protocol::TCP);
    assert_eq!(
        tcp::parse(ip.source, ip.destination, body).unwrap().header,
        reply_header
    );
    assert_eq!(
        interface.send_tcp(Ipv4Address::BROADCAST, &reply, 64, 74, &mut sent.transmit()),
        Err(SendError::NoRoute)
    );
}

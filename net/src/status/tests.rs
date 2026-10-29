use super::*;

fn connected() -> InterfaceStatus {
    InterfaceStatus {
        name: InterfaceName::new(ETHERNET_NAME).unwrap(),
        link_up: true,
        mac: MacAddress([0x52, 0x54, 0, 0x12, 0x34, 0x56]),
        mtu: 1500,
        dhcp: Dhcp::Configured,
        subnet: Subnet::parse(b"10.0.2.15/24"),
        router: Some(Ipv4Address::new(10, 0, 2, 2)),
        dns: [Some(Ipv4Address::new(10, 0, 2, 3)), None, None],
        dhcp_server: Some(Ipv4Address::new(10, 0, 2, 2)),
        lease_left_s: Some(86_342),
        lease_s: Some(86_400),
        counters: Counters {
            received_packets: 2905,
            received_bytes: 3_600_000,
            received_errors: 0,
            received_dropped: 4,
            sent_packets: 812,
            sent_bytes: 1_300_000,
            sent_dropped: 0,
        },
    }
}

fn line(status: &InterfaceStatus) -> String {
    let mut text = String::new();
    write_interface(&mut text, status).unwrap();
    text
}

#[test]
fn an_interface_line_has_its_fields_in_order_and_parses_back() {
    let status = connected();
    assert_eq!(
        line(&status),
        "eth0 up 52:54:00:12:34:56 1500 configured 10.0.2.15/24 10.0.2.2 10.0.2.3 10.0.2.2 \
         86342 86400 2905 3600000 0 4 812 1300000 0\n"
    );
    assert_eq!(parse_interface(line(&status).as_bytes()), Some(status));
    assert_eq!(status.state(), State::Connected);
}

#[test]
fn missing_values_are_dashes_a_lease_that_never_ends_is_forever_and_lo_is_static() {
    let lo = InterfaceStatus {
        name: InterfaceName::new(LOOPBACK_NAME).unwrap(),
        mac: MacAddress::ZERO,
        mtu: 65536,
        dhcp: Dhcp::Off,
        subnet: Subnet::parse(b"127.0.0.1/8"),
        router: None,
        dns: [None; 3],
        dhcp_server: None,
        lease_left_s: None,
        lease_s: None,
        ..connected()
    };
    assert_eq!(
        line(&lo),
        "lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - 2905 3600000 0 4 812 1300000 0\n"
    );
    assert_eq!(parse_interface(line(&lo).as_bytes()), Some(lo));
    assert!(lo.is_loopback());
    assert_eq!(lo.state(), State::Connected);
    let forever = InterfaceStatus {
        lease_left_s: Some(INFINITE_LEASE_S),
        lease_s: Some(INFINITE_LEASE_S),
        dns: [
            Some(Ipv4Address::new(10, 0, 2, 3)),
            Some(Ipv4Address::new(10, 0, 2, 4)),
            Some(Ipv4Address::new(10, 0, 2, 5)),
        ],
        ..connected()
    };
    let text = line(&forever);
    assert!(text.contains(" 10.0.2.3,10.0.2.4,10.0.2.5 10.0.2.2 forever forever "), "{text}");
    assert_eq!(parse_interface(text.as_bytes()), Some(forever));
}

#[test]
fn the_state_a_person_sees_follows_the_link_then_dhcp() {
    let down = InterfaceStatus {
        link_up: false,
        ..connected()
    };
    assert_eq!(down.state(), State::CableUnplugged);
    let asking = InterfaceStatus {
        dhcp: Dhcp::Asking,
        subnet: None,
        ..connected()
    };
    assert_eq!(asking.state(), State::GettingAddress);
    let no_address = InterfaceStatus {
        dhcp: Dhcp::NoAddress,
        subnet: None,
        ..connected()
    };
    assert_eq!(no_address.state(), State::NoAddress);
    assert_eq!(State::GettingAddress.text(), "Getting an address...");
    assert_eq!(State::CableUnplugged.word(), "cable-unplugged");
}

#[test]
fn malformed_lines_are_refused_and_a_file_parses_line_by_line() {
    let good = line(&connected());
    for bad in [
        good.replacen("up", "sideways", 1),
        good.replacen("1500", "15x0", 1),
        good.replacen("configured", "maybe", 1),
        good.replacen(" 0\n", "\n", 1),
        String::from("eth0"),
        String::new(),
    ] {
        assert_eq!(parse_interface(bad.as_bytes()), None, "{bad}");
    }
    let file = format!("{good}{}", line(&InterfaceStatus {
        name: InterfaceName::new(b"eth1").unwrap(),
        ..connected()
    }));
    let (interfaces, count) = parse_interfaces::<4>(file.as_bytes());
    assert_eq!(count, 2);
    assert_eq!(interfaces[1].unwrap().name.as_bytes(), b"eth1");
    assert_eq!(parse_interfaces::<1>(file.as_bytes()).1, 1);
    assert_eq!(InterfaceName::new(b"has space"), None);
    assert_eq!(InterfaceName::new(b"a-name-too-long-1"), None);
}

#[test]
fn routes_are_linux_s_hex_layout_padded_to_127_columns() {
    let lo = InterfaceStatus {
        name: InterfaceName::new(LOOPBACK_NAME).unwrap(),
        subnet: Subnet::parse(b"127.0.0.1/8"),
        router: None,
        ..connected()
    };
    let mut text = String::new();
    write_routes(&mut text, [&connected(), &lo]).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().all(|line| line.len() == 127), "{text}");
    assert!(lines[0].starts_with("Iface\tDestination\tGateway \tFlags"));
    assert!(lines[1].starts_with("eth0\t00000000\t0202000A\t0003\t0\t0\t0\t00000000\t0\t0\t0"));
    assert!(lines[2].starts_with("eth0\t0002000A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0"));
}

#[test]
fn neighbours_are_linux_s_arp_table_and_parse_back() {
    let neighbour = Neighbour {
        address: Ipv4Address::new(10, 0, 2, 2),
        mac: MacAddress([0x52, 0x55, 0x0a, 0, 2, 2]),
        interface: InterfaceName::new(ETHERNET_NAME).unwrap(),
    };
    let mut text = String::new();
    write_neighbours(&mut text, [neighbour]).unwrap();
    assert_eq!(
        text,
        "IP address       HW type     Flags       HW address            Mask     Device\n\
         10.0.2.2         0x1         0x2         52:55:0a:00:02:02     *        eth0\n"
    );
    let mut lines = text.lines();
    assert_eq!(parse_neighbour(lines.next().unwrap().as_bytes()), None);
    assert_eq!(parse_neighbour(lines.next().unwrap().as_bytes()), Some(neighbour));
}

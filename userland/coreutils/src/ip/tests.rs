use super::*;
use crate::fake::Fake;

const LOOPBACK: &str = "lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - 12 1008 0 0 12 1008 0\n";
const CONNECTED: &str = "eth0 up 52:54:00:12:34:56 1500 configured 10.0.2.15/24 10.0.2.2 10.0.2.3 \
                         10.0.2.2 86342 86400 2905 3600000 0 4 812 1300000 0\n";
const UNPLUGGED: &str = "eth0 down 52:54:00:12:34:56 1500 configured 10.0.2.15/24 10.0.2.2 \
                         10.0.2.3 10.0.2.2 86342 86400 0 0 0 0 0 0 0\n";
const ARP: &str = "IP address       HW type     Flags       HW address            Mask     Device\n\
                   10.0.2.2         0x1         0x2         52:55:0a:00:02:02     *        eth0\n";

fn run(interfaces: &str, arguments: &[&CStr]) -> (i32, String, String) {
    let fake = Fake::default()
        .with_file("/proc/net/interfaces", 0o444, interfaces.as_bytes(), 0)
        .with_file("/proc/net/arp", 0o444, ARP.as_bytes(), 0);
    let mut output = Output::new(fake, b"ip");
    let status = ip(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0);
    (status, fake.output(), fake.errors())
}

#[test]
fn addresses_are_shown_in_linux_s_shape_with_the_dhcp_state() {
    let both = format!("{LOOPBACK}{CONNECTED}");
    let expected = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 state UNKNOWN\n    \
                    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00\n    \
                    inet 127.0.0.1/8 scope host lo\n\
                    2: eth0: <BROADCAST,UP,LOWER_UP> mtu 1500 state UP dhcp configured\n    \
                    link/ether 52:54:00:12:34:56 brd ff:ff:ff:ff:ff:ff\n    \
                    inet 10.0.2.15/24 brd 10.0.2.255 scope global dynamic eth0\n       \
                    valid_lft 86342sec preferred_lft 86342sec\n";
    assert_eq!(run(&both, &[]), (0, expected.to_owned(), String::new()));
    assert_eq!(run(&both, &[c"addr", c"show"]).1, expected);
    assert_eq!(run(&both, &[c"a"]).1, expected);
    let eth0 = run(&both, &[c"addr", c"show", c"dev", c"eth0"]).1;
    assert!(eth0.starts_with("2: eth0:") && !eth0.contains("lo:"), "{eth0}");
}

#[test]
fn an_unplugged_cable_shows_no_carrier_and_no_address() {
    let shown = run(&format!("{LOOPBACK}{UNPLUGGED}"), &[c"addr"]).1;
    assert!(
        shown.contains("2: eth0: <NO-CARRIER,BROADCAST,UP> mtu 1500 state DOWN dhcp configured\n"),
        "{shown}"
    );
    assert!(!shown.contains("inet 10.0.2.15"), "{shown}");
    assert_eq!(run(&format!("{LOOPBACK}{UNPLUGGED}"), &[c"route"]).1, "");
}

#[test]
fn links_with_statistics_show_the_counters() {
    let shown = run(&format!("{LOOPBACK}{CONNECTED}"), &[c"-s", c"link"]).1;
    assert!(shown.contains("    RX:  bytes packets errors dropped\n"), "{shown}");
    assert!(shown.contains("   3600000    2905      0       4\n"), "{shown}");
    assert!(shown.contains("   1300000     812      0       0\n"), "{shown}");
    assert!(!shown.contains("inet "), "{shown}");
}

#[test]
fn routes_and_neighbours_are_shown_as_linux_s_ip_shows_them() {
    let both = format!("{LOOPBACK}{CONNECTED}");
    assert_eq!(
        run(&both, &[c"route"]).1,
        "default via 10.0.2.2 dev eth0 proto dhcp src 10.0.2.15\n\
         10.0.2.0/24 dev eth0 proto kernel scope link src 10.0.2.15\n"
    );
    assert_eq!(
        run(&both, &[c"neigh"]).1,
        "10.0.2.2 dev eth0 lladdr 52:55:0a:00:02:02 REACHABLE\n"
    );
}

#[test]
fn json_is_the_net_status_tool_s_answer() {
    let shown = run(&format!("{LOOPBACK}{CONNECTED}"), &[c"-j", c"addr"]).1;
    assert!(shown.starts_with("{\"state\":\"connected\",\"adapters\":[{\"name\":\"lo\""), "{shown}");
    assert!(
        shown.contains(
            "{\"name\":\"eth0\",\"state\":\"connected\",\"link\":\"up\",\"dhcp\":\"configured\",\
             \"mac\":\"52:54:00:12:34:56\",\"mtu\":1500,\"address\":\"10.0.2.15\",\"prefix\":24,\
             \"mask\":\"255.255.255.0\",\"router\":\"10.0.2.2\",\"dns\":[\"10.0.2.3\"],\
             \"dhcp_server\":\"10.0.2.2\",\"lease_left_s\":86342,\"lease_s\":86400,\
             \"lease_ends\":true,\"rx_bytes\":3600000,\"rx_packets\":2905,\"rx_errors\":0,\
             \"rx_dropped\":4,\"tx_bytes\":1300000,\"tx_packets\":812,\"tx_dropped\":0}"
        ),
        "{shown}"
    );
    assert!(shown.ends_with("]}\n"));
    let alone = run(LOOPBACK, &[c"-j", c"addr"]).1;
    assert!(alone.starts_with("{\"state\":\"none\",\"adapters\":[{\"name\":\"lo\""), "{alone}");
    assert!(alone.contains("\"router\":null,\"dns\":[]"), "{alone}");
}

#[test]
fn unknown_objects_devices_and_options_are_refused() {
    let both = format!("{LOOPBACK}{CONNECTED}");
    let (status, _, errors) = run(&both, &[c"tunnel"]);
    assert_eq!((status, errors.as_str()), (1, "ip: object \"tunnel\" is unknown, try \"ip help\"\n"));
    let (status, _, errors) = run(&both, &[c"addr", c"show", c"dev", c"wlan0"]);
    assert_eq!((status, errors.as_str()), (1, "ip: device \"wlan0\" does not exist\n"));
    assert_eq!(run(&both, &[c"-x"]).0, 1);
    let (status, _, errors) = run("", &[]);
    assert_eq!((status, errors.as_str()), (0, ""));
}

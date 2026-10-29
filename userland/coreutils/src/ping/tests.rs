use super::*;
use crate::fake::Fake;
use eggos_net::Ipv4Address;
use eggos_runtime::errno::Errno;

const HOST: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const DNS: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);

fn network() -> Fake {
    let mut fake =
        Fake::default().with_file("/etc/resolv.conf", 0o644, b"nameserver 10.0.2.3\n", 0);
    fake.network.reachable.push(HOST);
    fake.network.dns_servers.push((DNS, 53));
    fake.network.names.push((b"eggos.test".to_vec(), HOST));
    fake.network.round_trip_us = 512;
    fake
}

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
    let mut output = Output::new(fake, b"ping");
    let status = ping(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0);
    assert!(fake.network.sockets.iter().all(|socket| !socket.open));
    (status, fake.output(), fake.errors(), fake)
}

#[test]
fn a_reachable_host_answers_each_request_and_the_summary_counts_them() {
    let (status, out, errors, fake) = run(network(), &[c"-c", c"2", c"10.0.2.2"]);
    assert_eq!(
        (status, out.as_str(), errors.as_str()),
        (
            0,
            "PING 10.0.2.2 (10.0.2.2) 56(84) bytes of data.\n\
             64 bytes from 10.0.2.2: icmp_seq=1 time=0.512 ms\n\
             64 bytes from 10.0.2.2: icmp_seq=2 time=0.512 ms\n\
             \n\
             --- 10.0.2.2 ping statistics ---\n\
             2 packets transmitted, 2 received, 0% packet loss, time 1001ms\n\
             rtt min/avg/max = 0.512/0.512/0.512 ms\n",
            ""
        )
    );
    let sent: Vec<&Vec<u8>> = fake
        .network
        .sent
        .iter()
        .map(|(_, _, _, bytes)| bytes)
        .collect();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].len(), 64);
    assert_eq!(&sent[1][..8], &[8, 0, 0, 0, 0, 0, 0, 2]);
    assert_eq!(fake.slept.len(), 1, "no wait after the last request");
}

#[test]
fn a_name_is_resolved_through_resolv_conf_s_server() {
    let (status, out, _, _) = run(network(), &[c"-c", c"1", c"-s", c"8", c"eggos.test"]);
    assert_eq!(status, 0);
    assert!(
        out.starts_with("PING eggos.test (10.0.2.2) 8(36) bytes of data.\n16 bytes from 10.0.2.2"),
        "{out}"
    );
    let (status, _, errors, _) = run(network(), &[c"nowhere.test"]);
    assert_eq!(
        (status, errors.as_str()),
        (2, "ping: nowhere.test: no such name\n")
    );
}

#[test]
fn a_silent_host_loses_every_request_and_fails() {
    let (status, out, _, _) = run(network(), &[c"-c", c"1", c"10.0.2.9"]);
    assert_eq!(
        (status, out.as_str()),
        (
            1,
            "PING 10.0.2.9 (10.0.2.9) 56(84) bytes of data.\n\
             no answer from 10.0.2.9: icmp_seq=1\n\
             \n\
             --- 10.0.2.9 ping statistics ---\n\
             1 packets transmitted, 0 received, 100% packet loss, time 1000ms\n"
        )
    );
}

#[test]
fn bad_options_and_send_errors_are_reported() {
    assert_eq!(
        run(network(), &[c"-c", c"0", c"10.0.2.2"]).2,
        "ping: invalid count: '0'\n"
    );
    assert_eq!(
        run(network(), &[c"-i", c"0.1", c"10.0.2.2"]).2,
        "ping: invalid interval (at least 0.2 seconds): '0.1'\n"
    );
    assert_eq!(run(network(), &[]).0, 2);
    let mut unreachable = network();
    unreachable.network.send_error = Some(Errno(101));
    let (status, out, errors, _) = run(unreachable, &[c"-c", c"1", c"192.0.2.1"]);
    assert_eq!(status, 1);
    assert!(
        errors.starts_with("ping: sending to 192.0.2.1: "),
        "{errors}"
    );
    assert!(
        out.contains("1 packets transmitted, 0 received, +1 errors, 100% packet loss"),
        "{out}"
    );
    assert_eq!(parse_seconds_ms(b"0.25"), Some(250));
    assert_eq!(parse_seconds_ms(b"2"), Some(2000));
    assert_eq!(parse_seconds_ms(b"1."), None);
    assert_eq!(parse_seconds_ms(b"0.0001"), None);
}

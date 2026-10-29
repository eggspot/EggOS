use super::*;
use crate::fake::Fake;

fn network() -> Fake {
    let mut fake = Fake::default().with_file("/etc/resolv.conf", 0o644, b"nameserver 10.0.2.3\n", 0);
    fake.network.dns_servers.push((Ipv4Address::new(10, 0, 2, 3), 53));
    fake.network.dns_servers.push((Ipv4Address::new(10, 0, 2, 2), 5353));
    fake.network.names.push((b"eggos.test".to_vec(), Ipv4Address::new(10, 0, 2, 2)));
    fake.network.names.push((b"eggos.test".to_vec(), Ipv4Address::new(192, 0, 2, 7)));
    fake
}

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String) {
    let mut output = Output::new(fake, b"nslookup");
    let status = nslookup(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0);
    (status, fake.output(), fake.errors())
}

#[test]
fn a_name_s_addresses_are_shown_as_bind_s_nslookup_shows_them() {
    assert_eq!(
        run(network(), &[c"eggos.test"]),
        (
            0,
            "Server:\t\t10.0.2.3\nAddress:\t10.0.2.3#53\n\n\
             Non-authoritative answer:\n\
             Name:\teggos.test\nAddress: 10.0.2.2\n\
             Name:\teggos.test\nAddress: 192.0.2.7\n"
                .to_owned(),
            String::new()
        )
    );
}

#[test]
fn another_server_and_port_can_be_named() {
    let (status, out, _) = run(network(), &[c"-port=5353", c"eggos.test.", c"10.0.2.2"]);
    assert_eq!(status, 0);
    assert!(out.starts_with("Server:\t\t10.0.2.2\nAddress:\t10.0.2.2#5353\n\n"), "{out}");
    assert!(out.contains("Address: 192.0.2.7\n"), "{out}");
}

#[test]
fn a_missing_name_a_silent_server_and_bad_operands_fail() {
    let (status, out, _) = run(network(), &[c"nowhere.test"]);
    assert_eq!(status, 1);
    assert!(out.ends_with("** server can't find nowhere.test: NXDOMAIN\n"), "{out}");
    let (status, out, _) = run(network(), &[c"-timeout=1", c"eggos.test", c"10.0.2.9"]);
    assert_eq!(status, 1);
    assert!(out.ends_with(";; connection timed out; no servers could be reached\n"), "{out}");
    let (status, _, errors) = run(Fake::default(), &[c"eggos.test"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "nslookup: no DNS server: /etc/resolv.conf names none\n")
    );
    assert_eq!(run(network(), &[]).0, 1);
    assert_eq!(run(network(), &[c"-port=0", c"eggos.test"]).0, 1);
    let (status, _, errors) = run(network(), &[c"10.0.2.2"]);
    assert_eq!(status, 1);
    assert!(errors.contains("not supported yet"), "{errors}");
}

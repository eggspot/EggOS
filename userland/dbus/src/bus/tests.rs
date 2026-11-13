use super::*;

const ID: [u8; 32] = *b"00112233445566778899aabbccddeeff";
const ROOT: Credentials = Credentials { uid: 0, pid: 42 };

fn call(serial: u32, destination: &[u8], member: &[u8], signature: &[u8], body: &Writer) -> Vec<u8> {
    let interface: &[u8] = match destination {
        BUS_NAME => BUS_NAME,
        _ => b"org.example.App",
    };
    let fields = Fields {
        path: Some(b"/"),
        interface: Some(interface),
        member: Some(member),
        destination: Some(destination),
        signature,
        ..Fields::default()
    };
    message::build(Endian::Little, kind::METHOD_CALL, 0, serial, &fields, body.bytes())
}

fn strings(values: &[&[u8]]) -> Writer {
    let mut body = Writer::new(Endian::Little);
    for value in values {
        body.string(value);
    }
    body
}

fn joined(bus: &mut Bus) -> ConnectionId {
    let id = bus.connect(ROOT, true);
    bus.receive(id, &call(1, BUS_NAME, b"Hello", b"", &strings(&[])), Vec::new());
    id
}

fn members(deliveries: &[Delivery], to: ConnectionId) -> Vec<Vec<u8>> {
    deliveries
        .iter()
        .filter(|delivery| delivery.to == to)
        .map(|delivery| {
            let message = Message::parse(&delivery.bytes).unwrap();
            message
                .fields
                .member
                .or(message.fields.error_name)
                .unwrap_or(b"(return)")
                .to_vec()
        })
        .collect()
}

#[test]
fn hello_names_the_connection_and_only_hello_may_come_first() {
    let mut bus = Bus::new(ID);
    let first = joined(&mut bus);
    let deliveries = bus.take_deliveries();
    let reply = Message::parse(&deliveries[0].bytes).unwrap();
    assert_eq!(reply.kind, kind::METHOD_RETURN);
    assert_eq!(reply.string_argument(0), Some((b's', &b":1.1"[..])));
    assert_eq!(members(&deliveries, first), [b"(return)".to_vec(), b"NameAcquired".to_vec()]);
    let rude = bus.connect(ROOT, false);
    bus.receive(rude, &call(1, BUS_NAME, b"ListNames", b"", &strings(&[])), Vec::new());
    assert_eq!(bus.take_closing(), [rude]);
}

#[test]
fn a_name_is_owned_queued_replaced_and_passed_on_when_its_owner_leaves() {
    let mut bus = Bus::new(ID);
    let watcher = joined(&mut bus);
    bus.receive(
        watcher,
        &call(2, BUS_NAME, b"AddMatch", b"s", &strings(&[b"type='signal',member='NameOwnerChanged',arg0='org.example.App'"])),
        Vec::new(),
    );
    let first = joined(&mut bus);
    let second = joined(&mut bus);
    bus.take_deliveries();
    let mut body = strings(&[b"org.example.App"]);
    body.u32(request::ALLOW_REPLACEMENT);
    bus.receive(first, &call(2, BUS_NAME, b"RequestName", b"su", &body), Vec::new());
    let deliveries = bus.take_deliveries();
    assert_eq!(members(&deliveries, watcher), [b"NameOwnerChanged".to_vec()]);
    let mut body = strings(&[b"org.example.App"]);
    body.u32(0);
    bus.receive(second, &call(2, BUS_NAME, b"RequestName", b"su", &body), Vec::new());
    let reply = bus.take_deliveries();
    let answer = Message::parse(&reply[0].bytes).unwrap();
    assert_eq!(answer.body_reader().u32(), Ok(request::IN_QUEUE));
    bus.disconnect(first);
    let deliveries = bus.take_deliveries();
    assert_eq!(members(&deliveries, second), [b"NameAcquired".to_vec()]);
    assert_eq!(members(&deliveries, watcher), [b"NameOwnerChanged".to_vec()]);
    assert_eq!(bus.owner(b"org.example.App"), Some(second));
}

#[test]
fn calls_reach_the_owner_with_the_sender_set_and_only_awaited_replies_return() {
    let mut bus = Bus::new(ID);
    let app = joined(&mut bus);
    let caller = joined(&mut bus);
    let mut body = strings(&[b"org.example.App"]);
    body.u32(0);
    bus.receive(app, &call(2, BUS_NAME, b"RequestName", b"su", &body), Vec::new());
    bus.take_deliveries();
    bus.receive(caller, &call(7, b"org.example.App", b"Activate", b"", &strings(&[])), Vec::new());
    let deliveries = bus.take_deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].to, app);
    let routed = Message::parse(&deliveries[0].bytes).unwrap();
    assert_eq!(routed.fields.sender, Some(&b":1.2"[..]));
    let reply = |serial: u32| {
        let fields = Fields {
            reply_serial: Some(serial),
            destination: Some(b":1.2"),
            ..Fields::default()
        };
        message::build(Endian::Little, kind::METHOD_RETURN, 0, 3, &fields, &[])
    };
    bus.receive(app, &reply(8), Vec::new());
    assert!(bus.take_deliveries().is_empty());
    bus.receive(app, &reply(7), Vec::new());
    assert_eq!(bus.take_deliveries()[0].to, caller);
    bus.receive(app, &reply(7), Vec::new());
    assert!(bus.take_deliveries().is_empty());
}

#[test]
fn a_call_to_nobody_gets_service_unknown_and_a_departed_callee_no_reply() {
    let mut bus = Bus::new(ID);
    let caller = joined(&mut bus);
    let callee = joined(&mut bus);
    bus.take_deliveries();
    bus.receive(caller, &call(4, b"org.example.Missing", b"Do", b"", &strings(&[])), Vec::new());
    assert_eq!(members(&bus.take_deliveries(), caller), [error::SERVICE_UNKNOWN.to_vec()]);
    bus.receive(caller, &call(5, b":1.2", b"Do", b"", &strings(&[])), Vec::new());
    bus.take_deliveries();
    bus.disconnect(callee);
    assert_eq!(members(&bus.take_deliveries(), caller), [error::NO_REPLY.to_vec()]);
}

#[test]
fn descriptors_go_only_to_connections_that_take_them() {
    let mut bus = Bus::new(ID);
    let caller = joined(&mut bus);
    let plain = bus.connect(ROOT, false);
    bus.receive(plain, &call(1, BUS_NAME, b"Hello", b"", &strings(&[])), Vec::new());
    bus.take_deliveries();
    let fields = Fields {
        path: Some(b"/"),
        member: Some(b"Take"),
        destination: Some(b":1.2"),
        signature: b"h",
        unix_fds: 1,
        ..Fields::default()
    };
    let mut body = Writer::new(Endian::Little);
    body.u32(0);
    let bytes = message::build(Endian::Little, kind::METHOD_CALL, 0, 9, &fields, body.bytes());
    bus.receive(caller, &bytes, Vec::from([17]));
    assert_eq!(members(&bus.take_deliveries(), caller), [error::FAILED.to_vec()]);
    assert_eq!(bus.take_unused_descriptors(), [17]);
}

#[test]
fn well_known_names_follow_the_rules() {
    assert!(valid_well_known_name(b"org.freedesktop.portal.Desktop"));
    assert!(valid_well_known_name(b"org.a-b.c_d"));
    assert!(!valid_well_known_name(b"single"));
    assert!(!valid_well_known_name(b":1.4"));
    assert!(!valid_well_known_name(b"org..app"));
    assert!(!valid_well_known_name(b"org.9app"));
}

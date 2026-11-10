use crate::fake::{Fake, FakePage};
use crate::fetch::fetch;
use crate::output::Output;
use core::ffi::CStr;
use eggos_download_list::{Record, State};
use eggos_net::Ipv4Address;

const HOST: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const RECORD: &str = "/run/eggos/downloads/77";

fn machine() -> Fake {
    let mut fake = Fake::default()
        .with_directory("/home", 0)
        .with_directory("/home/root", 0)
        .with_directory("/run", 0);
    fake.now = 1_793_282_650;
    fake.network.reachable.push(HOST);
    for (target, response) in [
        (
            "/hello.txt",
            &b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, EggOS!"[..],
        ),
        (
            "/missing",
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        ),
        (
            "/stream",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nHello\r\n0\r\n\r\n",
        ),
    ] {
        fake.network.pages.push(FakePage {
            address: HOST,
            port: 80,
            target: target.as_bytes().to_vec(),
            response: response.to_vec(),
        });
    }
    fake
}

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, Fake) {
    let mut output = Output::new(fake, b"fetch");
    let status = fetch(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0, "every file is closed");
    let errors = fake.errors();
    (status, errors, fake)
}

fn record_of(fake: &Fake) -> Vec<u8> {
    assert!(
        !fake
            .nodes
            .keys()
            .any(|path| path.starts_with(b"/run/eggos/downloads/.")),
        "no temporary record is left"
    );
    fake.contents(RECORD)
}

#[test]
fn a_saved_download_is_recorded_done_with_what_arrived() {
    let (status, _, fake) = run(
        machine(),
        &[c"-q", c"http://10.0.2.2/hello.txt", c"/home/root/hello.txt"],
    );
    assert_eq!(status, 0);
    assert!(fake.termination_caught);
    let text = record_of(&fake);
    let record = Record::parse(&text).unwrap();
    assert_eq!(
        record,
        Record {
            url: b"http://10.0.2.2/hello.txt",
            path: b"/home/root/hello.txt",
            received: 13,
            total: Some(13),
            state: State::Done,
            reason: b"",
            started: 1_793_282_650,
            ended: Some(1_793_282_650),
        }
    );
}

#[test]
fn a_failed_download_is_recorded_with_its_reason_in_a_few_words() {
    let (status, errors, fake) = run(
        machine(),
        &[c"http://10.0.2.2/missing", c"/home/root/missing"],
    );
    assert_eq!(
        (status, errors.as_str()),
        (1, "fetch: 10.0.2.2:80/missing: 404 Not Found\n")
    );
    let text = record_of(&fake);
    let record = Record::parse(&text).unwrap();
    assert_eq!(record.state, State::Failed);
    assert_eq!(record.reason, b"404 Not Found");
    assert_eq!(record.total, None);
}

#[test]
fn a_terminated_download_removes_its_partial_file_and_is_recorded_cancelled() {
    let mut fake = machine();
    fake.termination_requested = true;
    let (status, errors, fake) = run(fake, &[c"http://10.0.2.2/stream", c"/home/root/stream"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "fetch: cancelled: the partial file was removed\n")
    );
    assert!(!fake.nodes.contains_key(&b"/home/root/stream"[..]));
    let text = record_of(&fake);
    let record = Record::parse(&text).unwrap();
    assert_eq!(record.state, State::Cancelled);
    assert_eq!(record.received, 0);
}

#[test]
fn a_relative_file_is_recorded_by_its_whole_path_and_standard_output_is_not_recorded() {
    let mut fake = machine();
    fake.working_directory = "/home/root".to_owned();
    let (status, _, fake) = run(fake, &[c"-q", c"http://10.0.2.2/hello.txt", c"hello.txt"]);
    assert_eq!(status, 0);
    let text = record_of(&fake);
    assert_eq!(Record::parse(&text).unwrap().path, b"/home/root/hello.txt");
    let (status, _, fake) = run(machine(), &[c"http://10.0.2.2/hello.txt"]);
    assert_eq!(status, 0);
    assert!(!fake.nodes.contains_key(RECORD.as_bytes()));
    assert!(!fake.termination_caught);
}

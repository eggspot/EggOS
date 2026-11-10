use crate::fake::{Fake, FakePage, FakeTerminal, FakeWindow};
use crate::system::FileSystemUsage;
use crate::utilities::{Utility, run};
use core::ffi::CStr;
use eggos_commands::COMMANDS;
use eggos_download_list::{Record, State};
use eggos_net::Ipv4Address;
use std::ffi::CString;

const EXAMPLE_HOST: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
const EXAMPLE_DNS_SERVER: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);
const EXAMPLE_ROUND_TRIP_US: u64 = 512;
const EXAMPLE_INTERFACES: &str = concat!(
    "lo up 00:00:00:00:00:00 65536 off 127.0.0.1/8 - - - - - 12 1008 0 0 12 1008 0\n",
    "eth0 up 52:54:00:12:34:56 1500 configured 10.0.2.15/24 10.0.2.2 10.0.2.3 10.0.2.2 86342 ",
    "86400 2905 3600000 0 4 812 1300000 0\n",
);
const EXAMPLE_PAGE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\nHello, EggOS!\n";

fn with_network(fake: Fake) -> Fake {
    let mut fake = fake
        .with_directory("/etc", 0)
        .with_file("/etc/resolv.conf", 0o644, b"nameserver 10.0.2.3\n", 0)
        .with_directory("/proc/net", 0)
        .with_file(
            "/proc/net/interfaces",
            0o444,
            EXAMPLE_INTERFACES.as_bytes(),
            0,
        );
    fake.network.reachable.push(EXAMPLE_HOST);
    fake.network.dns_servers.push((EXAMPLE_DNS_SERVER, 53));
    fake.network
        .names
        .push((b"eggos.test".to_vec(), EXAMPLE_HOST));
    fake.network.round_trip_us = EXAMPLE_ROUND_TRIP_US;
    fake.network.pages.push(FakePage {
        address: EXAMPLE_HOST,
        port: 80,
        target: b"/hello.txt".to_vec(),
        response: EXAMPLE_PAGE.to_vec(),
    });
    fake
}

fn machine() -> Fake {
    let fake = Fake::default()
        .with_directory("/home", 0)
        .with_directory("/home/root", 0)
        .with_file("/home/root/notes.txt", 0o644, b"eggs\nmilk\nbread\n", 0)
        .with_directory("/proc", 0)
        .with_file(
            "/proc/ps",
            0o444,
            b"1 0 1 1 R 120 init\n7 1 7 1 R 35 eggsh\n",
            0,
        )
        .with_file(
            "/proc/meminfo",
            0o444,
            b"MemTotal: 131072 kB\nMemFree: 100000 kB\nMemAvailable: 100000 kB\n",
            0,
        )
        .with_file("/proc/uptime", 0o444, b"3725.42 0.00\n", 0)
        .with_directory("/proc/driver", 0)
        .with_file(
            "/proc/driver/devices",
            0o444,
            crate::device::tests::EXAMPLE,
            0,
        )
        .with_file(
            "/proc/driver/rtc",
            0o444,
            b"rtc_time\t: 14:05:09\nrtc_date\t: 2026-10-29\n24hr\t\t: yes\n",
            0,
        )
        .with_file(
            "/proc/mounts",
            0o444,
            b"rootfs / rootfs ro 0 0\neggfs /home eggfs rw 0 0\n",
            0,
        )
        .with_link("/proc/self", "7");
    let mut fake = with_network(crate::download::tests::with_records(fake));
    fake.now = 1_793_282_709;
    fake.terminal = Some(FakeTerminal::default());
    fake.file_systems = vec![(
        b"/home".to_vec(),
        FileSystemUsage {
            block_size: 4096,
            blocks: 262_144,
            free: 196_608,
            available: 196_608,
            read_only: false,
        },
    )];
    fake.windows = vec![FakeWindow {
        id: 129,
        state: 0,
        rect: [100, 80, 640, 480],
        client: 67,
        title: b"Notes".to_vec(),
    }];
    fake
}

#[test]
fn every_core_utility_s_example_writes_what_the_registry_says() {
    let mut ran = 0;
    for command in &COMMANDS {
        if Utility::named(command.words[0].as_bytes()).is_none() {
            continue;
        }
        for example in command.examples {
            let words: Vec<CString> = example
                .line
                .split(' ')
                .map(|word| CString::new(word).unwrap())
                .collect();
            let arguments: Vec<&CStr> = words.iter().map(CString::as_c_str).collect();
            let mut fake = machine();
            let status = run(&mut fake, &arguments);
            assert_eq!(
                (status, fake.output()),
                (example.status, example.output.to_owned()),
                "{}: {}",
                example.line,
                fake.errors()
            );
            ran += 1;
        }
    }
    assert!(ran >= 25, "{ran}");
}

#[test]
fn every_utility_has_a_definition_in_the_registry() {
    for utility in Utility::ALL {
        assert!(
            COMMANDS
                .iter()
                .any(|command| command.words[0].as_bytes() == utility.name()),
            "{}",
            String::from_utf8_lossy(utility.name())
        );
    }
}

#[test]
fn the_examples_that_change_files_change_them() {
    let mut fake = machine();
    assert_eq!(
        run(
            &mut fake,
            &[c"cp", c"/home/root/notes.txt", c"/home/root/copy.txt"]
        ),
        0
    );
    assert_eq!(fake.contents("/home/root/copy.txt"), b"eggs\nmilk\nbread\n");
    assert_eq!(run(&mut fake, &[c"mkdir", c"/home/root/plans"]), 0);
    assert_eq!(run(&mut fake, &[c"ls", c"/home/root"]), 0);
    assert_eq!(fake.output(), "copy.txt\nnotes.txt\nplans\n");
    let link = [c"ln", c"-s", c"notes.txt", c"/home/root/latest"];
    assert_eq!(run(&mut fake, &link), 0);
    assert_eq!(run(&mut fake, &[c"cat", c"/home/root/latest"]), 0);
    assert!(fake.output().ends_with("plans\neggs\nmilk\nbread\n"));
    assert_eq!(
        fake.link_target("/home/root/latest"),
        Some(b"notes.txt".to_vec())
    );
}

#[test]
fn the_fetch_example_saves_the_page_and_records_the_download() {
    let mut fake = machine();
    let line = [
        c"fetch",
        c"-o",
        c"/home/root/hello.txt",
        c"http://eggos.test/hello.txt",
    ];
    assert_eq!(run(&mut fake, &line), 0);
    assert_eq!(fake.contents("/home/root/hello.txt"), b"Hello, EggOS!\n");
    let text = fake.contents("/run/eggos/downloads/77");
    assert_eq!(
        Record::parse(&text).unwrap(),
        Record {
            url: b"http://eggos.test/hello.txt",
            path: b"/home/root/hello.txt",
            received: 14,
            total: Some(14),
            state: State::Done,
            reason: b"",
            started: 1_793_282_709,
            ended: Some(1_793_282_709),
        }
    );
}

use crate::fake::{Fake, FakeWindow};
use crate::system::FileSystemUsage;
use crate::utilities::{Utility, run};
use core::ffi::CStr;
use eggos_commands::COMMANDS;
use std::ffi::CString;

fn machine() -> Fake {
    let mut fake = Fake::default()
        .with_directory("/home", 0)
        .with_directory("/home/root", 0)
        .with_file("/home/root/notes.txt", 0o644, b"eggs\nmilk\nbread\n", 0)
        .with_directory("/proc", 0)
        .with_file("/proc/ps", 0o444, b"1 0 1 1 R init\n7 1 7 1 R eggsh\n", 0)
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
            "/proc/mounts",
            0o444,
            b"rootfs / rootfs ro 0 0\neggfs /home eggfs rw 0 0\n",
            0,
        )
        .with_link("/proc/self", "7");
    fake.now = 1_793_282_709;
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

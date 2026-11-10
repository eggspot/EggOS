use super::*;
use crate::fake::Fake;

pub(crate) const RECORDS: [(&str, &[u8]); 4] = [
    (
        "/run/eggos/downloads/61",
        b"url=https://example.com/eggos-0.3.0-x86_64.elf\n\
path=/home/root/Downloads/eggos-0.3.0-x86_64.elf\nreceived=3565159\ntotal=12582912\n\
state=downloading\nstarted=1793282650\n",
    ),
    (
        "/run/eggos/downloads/58",
        b"url=http://10.0.2.2:8000/notes.txt\npath=/home/root/Downloads/notes.txt\n\
received=512\ntotal=512\nstate=done\nstarted=1793282600\nended=1793282601\n",
    ),
    (
        "/run/eggos/downloads/55",
        b"url=https://bad.example.net/a.zip\npath=/home/root/Downloads/a.zip\nreceived=0\n\
state=failed\nreason=certificate not trusted\nstarted=1793282590\nended=1793282591\n",
    ),
    (
        "/run/eggos/downloads/50",
        b"url=http://example.org/big.iso\npath=/home/root/Downloads/big.iso\nreceived=1048576\n\
total=734003200\nstate=cancelled\nstarted=1793282500\nended=1793282530\n",
    ),
];

pub(crate) fn with_records(fake: Fake) -> Fake {
    let mut fake = fake
        .with_directory("/run", 0)
        .with_directory("/run/eggos", 0)
        .with_directory("/run/eggos/downloads", 0);
    for (path, text) in RECORDS {
        fake = fake.with_file(path, 0o644, text, 0);
    }
    fake
}

fn machine(fetch_running: bool) -> Fake {
    let ps: &[u8] = match fetch_running {
        true => b"1 0 1 1 R 120 init\n61 7 61 7 R 3 fetch\n",
        false => b"1 0 1 1 R 120 init\n",
    };
    let mut fake = with_records(
        Fake::default()
            .with_directory("/proc", 0)
            .with_file("/proc/ps", 0o444, ps, 0),
    );
    fake.running = vec![61];
    fake
}

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
    let mut output = Output::new(fake, b"download");
    let status = download(&mut output, arguments);
    let status = output.finish(status);
    let fake = output.system;
    assert_eq!(fake.open_count(), 0, "every file is closed");
    (status, fake.output(), fake.errors(), fake)
}

#[test]
fn the_list_has_the_window_s_rows_newest_first() {
    let (status, out, _, _) = run(machine(true), &[c"list"]);
    assert_eq!(status, 0);
    assert_eq!(
        out,
        concat!(
            "ID    NAME                    SIZE                STATE                             FROM\n",
            "61    eggos-0.3.0-x86_64.elf  3.4 MB of 12 MB     Downloading, 28 %                 example.com\n",
            "58    notes.txt               512 B               Done                              10.0.2.2:8000\n",
            "55    a.zip                   0 B                 Failed: certificate not trusted   bad.example.net\n",
            "50    big.iso                 1.0 MB of 700 MB    Cancelled                         example.org\n",
        )
    );
    let (_, out, _, _) = run(machine(false), &[c"list"]);
    assert!(out.contains("Failed: fetch ended without finishing example.com\n"));
}

#[test]
fn the_json_list_has_the_whole_record_and_the_percent() {
    let (status, out, _, _) = run(machine(true), &[c"list", c"--json"]);
    assert_eq!(status, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with(
        "{\"id\":61,\"name\":\"eggos-0.3.0-x86_64.elf\",\
         \"path\":\"/home/root/Downloads/eggos-0.3.0-x86_64.elf\",\
         \"url\":\"https://example.com/eggos-0.3.0-x86_64.elf\",\"size_text\":\"3.4 MB of 12 MB\",\
         \"state\":\"downloading\",\"state_text\":\"Downloading, 28 %\",\"reason\":null,\
         \"percent\":28,\"received_bytes\":3565159,\"total_bytes\":12582912,"
    ));
    assert!(lines[0].ends_with("\"ended_at\":null}"));
    assert!(lines[2].contains("\"reason\":\"certificate not trusted\",\"percent\":null,"));
    assert!(lines[2].contains("\"total_bytes\":null,"));
}

#[test]
fn an_empty_or_missing_folder_lists_nothing_since_boot() {
    let (status, out, _, _) = run(Fake::default(), &[c"list"]);
    assert_eq!((status, out.as_str()), (0, "No downloads since boot.\n"));
    let (status, out, _, _) = run(Fake::default(), &[c"list", c"--json"]);
    assert_eq!((status, out.as_str()), (0, ""));
}

#[test]
fn cancel_sends_term_to_a_running_fetch_and_says_why_it_cannot() {
    let (status, out, errors, fake) = run(machine(true), &[c"cancel", c"61"]);
    assert_eq!((status, out.as_str(), errors.as_str()), (0, "", ""));
    assert_eq!(fake.signals_sent, [(61, 15)]);
    let (status, _, errors, fake) = run(machine(true), &[c"cancel", c"58"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "download: 58: has already ended\n")
    );
    assert!(fake.signals_sent.is_empty());
    let (status, _, errors, _) = run(machine(false), &[c"cancel", c"61"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "download: 61: has already ended\n")
    );
    let (status, _, errors, _) = run(machine(true), &[c"cancel", c"42"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "download: 42: no such download\n")
    );
    let mut refusing = machine(true);
    refusing.running.clear();
    let (status, _, errors, _) = run(refusing, &[c"cancel", c"61"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "download: 61: has already ended\n")
    );
}

#[test]
fn clear_removes_the_records_that_are_over_and_never_the_files() {
    let (status, out, _, fake) = run(
        machine(true).with_file("/home/root/notes.txt", 0o644, b"x", 0),
        &[c"clear"],
    );
    assert_eq!((status, out.as_str()), (0, "Cleared 3 downloads.\n"));
    assert!(fake.nodes.contains_key(&b"/run/eggos/downloads/61"[..]));
    assert!(!fake.nodes.contains_key(&b"/run/eggos/downloads/58"[..]));
    assert!(fake.nodes.contains_key(&b"/home/root/notes.txt"[..]));
    let (status, out, errors, fake) = run(machine(true), &[c"clear", c"61", c"58"]);
    assert_eq!(
        (status, out.as_str(), errors.as_str()),
        (
            1,
            "Cleared 1 download.\n",
            "download: 61: still downloading\n"
        )
    );
    assert!(fake.nodes.contains_key(&b"/run/eggos/downloads/61"[..]));
    let (status, out, _, _) = run(Fake::default(), &[c"clear"]);
    assert_eq!((status, out.as_str()), (0, "Nothing to clear.\n"));
}

#[test]
fn cancel_signals_only_a_process_that_runs_fetch() {
    let mut reused = machine(true).with_file(
        "/proc/ps",
        0o444,
        b"1 0 1 1 R 120 init\n61 7 61 7 R 3 eggsh\n",
        0,
    );
    reused.running = vec![61];
    let (status, _, errors, fake) = run(reused, &[c"cancel", c"61"]);
    assert_eq!(
        (status, errors.as_str()),
        (1, "download: 61: has already ended\n")
    );
    assert!(fake.signals_sent.is_empty());
    for (command_line, signalled) in [
        (&b"/bin/eggsh\0"[..], false),
        (
            b"fetch\0-q\0https://example.com/eggos-0.3.0-x86_64.elf\0",
            true,
        ),
    ] {
        let mut unlisted = with_records(
            Fake::default()
                .with_directory("/proc", 0)
                .with_directory("/proc/61", 0)
                .with_file("/proc/61/cmdline", 0o444, command_line, 0),
        );
        unlisted.running = vec![61];
        let (status, _, _, fake) = run(unlisted, &[c"cancel", c"61"]);
        assert_eq!(
            (status, fake.signals_sent.is_empty()),
            (if signalled { 0 } else { 1 }, !signalled),
            "{command_line:?}"
        );
    }
}

#[test]
fn a_cut_process_list_leaves_a_download_as_its_record_says() {
    let mut ps = b"1 0 1 1 R 120 init\n".repeat(PROCESSES_BYTES / 19 + 1);
    ps.truncate(PROCESSES_BYTES);
    let fake = with_records(
        Fake::default()
            .with_directory("/proc", 0)
            .with_file("/proc/ps", 0o444, &ps, 0),
    );
    let (status, out, _, _) = run(fake, &[c"list"]);
    assert_eq!(status, 0);
    assert!(out.contains("Downloading, 28 %"), "{out}");
}

#[test]
fn a_full_list_keeps_the_newest_downloads() {
    let mut fake = Fake::default()
        .with_directory("/run", 0)
        .with_directory("/run/eggos", 0)
        .with_directory("/run/eggos/downloads", 0);
    let record = |started: u64| {
        format!(
            "url=http://10.0.2.2/f\npath=/home/root/f\nreceived=1\nstate=done\nstarted={started}\n\
             ended={started}\n"
        )
    };
    for id in 100..100 + MAX_DOWNLOADS as u32 {
        let path = format!("/run/eggos/downloads/{id}");
        fake = fake.with_file(&path, 0o644, record(1_000 + u64::from(id)).as_bytes(), 0);
    }
    fake = fake.with_file(
        "/run/eggos/downloads/999",
        0o644,
        record(9_000).as_bytes(),
        0,
    );
    let (status, out, _, _) = run(fake, &[c"list", c"--json"]);
    assert_eq!(status, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), MAX_DOWNLOADS);
    assert!(lines[0].starts_with("{\"id\":999,"), "{}", lines[0]);
    assert!(!out.contains("{\"id\":100,"), "the oldest is dropped");
    assert!(lines[MAX_DOWNLOADS - 1].starts_with("{\"id\":101,"));
}

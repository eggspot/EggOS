use super::*;

const RUNNING: &[u8] = b"url=https://example.com/eggos-0.3.0-x86_64.elf\n\
path=/home/root/Downloads/eggos-0.3.0-x86_64.elf\n\
received=3565158\n\
total=12582912\n\
state=downloading\n\
started=1793282650\n";
const DONE: &[u8] = b"url=http://10.0.2.2:8000/notes.txt\npath=/home/root/Downloads/notes.txt\n\
received=512\ntotal=512\nstate=done\nstarted=1793282600\nended=1793282601\n";
const FAILED: &[u8] = b"url=https://bad.example.net/a.zip\npath=/home/root/Downloads/a.zip\n\
received=0\nstate=failed\nreason=certificate not trusted\nstarted=1793282590\nended=1793282591\n";
const CANCELLED: &[u8] = b"url=http://example.org/big.iso\npath=/home/root/Downloads/big.iso\n\
received=1048576\ntotal=734003200\nstate=cancelled\nstarted=1793282500\nended=1793282530\n";

fn words(write: impl FnOnce(&mut dyn FnMut(&[u8]))) -> String {
    let mut text = Vec::new();
    write(&mut |bytes: &[u8]| text.extend_from_slice(bytes));
    String::from_utf8(text).unwrap()
}

fn row(text: &[u8]) -> [String; 4] {
    let record = Record::parse(text).unwrap();
    [
        String::from_utf8(record.name().to_vec()).unwrap(),
        words(|out| record.write_size(out)),
        words(|out| record.write_state(out)),
        String::from_utf8(record.host().to_vec()).unwrap(),
    ]
}

#[test]
fn a_record_is_read_and_written_back_byte_for_byte() {
    for text in [RUNNING, DONE, FAILED, CANCELLED] {
        let record = Record::parse(text).unwrap();
        let mut out = [0u8; MAX_RECORD_BYTES];
        let length = record.write(&mut out).unwrap();
        assert_eq!(&out[..length], text);
    }
    let record = Record::parse(RUNNING).unwrap();
    assert_eq!(record.total, Some(12_582_912));
    assert_eq!(record.ended, None);
    assert_eq!(record.state, State::Downloading);
    assert_eq!(record.write(&mut [0u8; 40]), None);
}

#[test]
fn a_record_missing_a_required_line_or_with_a_bad_number_is_none() {
    assert_eq!(
        Record::parse(b"url=x\npath=/a\nstate=done\nstarted=1\n"),
        None
    );
    assert_eq!(
        Record::parse(b"url=x\npath=/a\nreceived=1x\nstate=done\nstarted=1\n"),
        None
    );
    assert_eq!(
        Record::parse(b"url=x\npath=/a\nreceived=1\nstate=paused\nstarted=1\n"),
        None
    );
}

#[test]
fn control_bytes_in_a_text_are_written_as_question_marks() {
    let record = Record {
        url: b"http://h/a\nstate=done",
        path: b"/home/root/a",
        received: 0,
        total: None,
        state: State::Downloading,
        reason: b"",
        started: 5,
        ended: None,
    };
    let mut out = [0u8; 256];
    let length = record.write(&mut out).unwrap();
    let parsed = Record::parse(&out[..length]).unwrap();
    assert_eq!(parsed.url, b"http://h/a?state=done");
    assert_eq!(parsed.state, State::Downloading);
}

#[test]
fn the_columns_read_as_the_window_writes_them() {
    assert_eq!(
        row(RUNNING),
        [
            "eggos-0.3.0-x86_64.elf",
            "3.4 MB of 12 MB",
            "Downloading, 28 %",
            "example.com"
        ]
    );
    assert_eq!(row(DONE), ["notes.txt", "512 B", "Done", "10.0.2.2:8000"]);
    assert_eq!(
        row(FAILED),
        [
            "a.zip",
            "0 B",
            "Failed: certificate not trusted",
            "bad.example.net"
        ]
    );
    assert_eq!(
        row(CANCELLED),
        ["big.iso", "1.0 MB of 700 MB", "Cancelled", "example.org"]
    );
    let running = Record::parse(RUNNING).unwrap();
    assert_eq!(running.permille(), Some(283));
    assert_eq!(running.percent(), Some(28));
    assert_eq!(Record::parse(FAILED).unwrap().percent(), None);
    let grown = Record {
        received: 6_291_456,
        ..running
    };
    assert_eq!(words(|out| grown.write_size(out)), "6.0 MB of 12 MB");
    assert_eq!(words(|out| grown.write_state(out)), "Downloading, 50 %");
    assert_eq!(grown.permille(), Some(500));
    let without_size = Record {
        total: None,
        ..running
    };
    assert_eq!(words(|out| without_size.write_state(out)), "Downloading");
    assert_eq!(words(|out| without_size.write_short_state(out)), "3.4 MB");
    assert_eq!(words(|out| running.write_short_state(out)), "28 %");
}

#[test]
fn the_detail_line_has_the_whole_path_or_reason() {
    let detail = |text| {
        let record = Record::parse(text).unwrap();
        words(|out| record.write_detail(out))
    };
    assert_eq!(
        detail(RUNNING),
        "Saving to /home/root/Downloads/eggos-0.3.0-x86_64.elf"
    );
    assert_eq!(detail(DONE), "Saved to /home/root/Downloads/notes.txt");
    assert_eq!(detail(FAILED), "Failed: certificate not trusted");
    assert_eq!(
        detail(CANCELLED),
        "Cancelled: the partial file was removed."
    );
}

#[test]
fn a_downloading_record_whose_fetch_is_gone_reads_as_failed() {
    let running = Record::parse(RUNNING).unwrap();
    assert_eq!(running.judged(Some(true)), running);
    assert_eq!(running.judged(None), running);
    let gone = running.judged(Some(false));
    assert_eq!(gone.state, State::Failed);
    assert_eq!(
        words(|out| gone.write_state(out)),
        "Failed: fetch ended without finishing"
    );
    let done = Record::parse(DONE).unwrap();
    assert_eq!(done.judged(Some(false)), done);
}

#[test]
fn the_process_list_says_which_fetch_still_runs() {
    let ps = b"1 0 1 1 R 10 init\n61 7 61 7 R 3 fetch\n70 7 70 7 Z 1 fetch\n";
    assert!(process_running(ps, 61));
    assert!(!process_running(ps, 70));
    assert!(!process_running(ps, 6));
    assert!(!process_running(ps, 58));
}

#[test]
fn ids_are_plain_decimal_file_names() {
    assert_eq!(id_of(b"61"), Some(61));
    assert_eq!(id_of(b"061"), None);
    assert_eq!(id_of(b".61.new"), None);
    assert_eq!(id_of(b""), None);
    assert_eq!(id_of(b"99999999999"), None);
}

#[test]
fn the_list_is_newest_first_then_by_the_larger_id() {
    let mut order = [
        (1_793_282_600, 58),
        (1_793_282_650, 61),
        (1_793_282_650, 70),
    ];
    order.sort_by(|first, second| newest_first(*first, *second));
    assert_eq!(
        order,
        [
            (1_793_282_650, 70),
            (1_793_282_650, 61),
            (1_793_282_600, 58)
        ]
    );
}

#[test]
fn a_reason_is_what_follows_the_place_on_one_line() {
    assert_eq!(
        reason_of(b"bad.example.net:443: certificate not trusted\n"),
        b"certificate not trusted"
    );
    assert_eq!(
        reason_of(b"the URL is longer than 2048 bytes"),
        b"the URL is longer than 2048 bytes"
    );
    assert_eq!(reason_of(&[b'x'; 400]).len(), MAX_REASON_BYTES);
}

#[test]
fn cancelling_signals_only_a_running_download() {
    let running = Record::parse(RUNNING).unwrap();
    let mut sent = Vec::new();
    assert_eq!(
        cancel(Some(&running), 61, |pid, signal| {
            sent.push((pid, signal));
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(sent, [(61, TERMINATE)]);
    assert_eq!(
        cancel(Some(&running), 61, |_, _| Err(ESRCH)),
        Err(CancelError::AlreadyEnded)
    );
    assert_eq!(
        cancel(Some(&running), 61, |_, _| Err(EPERM)),
        Err(CancelError::NotAllowed)
    );
    let done = Record::parse(DONE).unwrap();
    assert_eq!(
        cancel(Some(&done), 58, |_, _| panic!(
            "signalled an ended download"
        )),
        Err(CancelError::AlreadyEnded)
    );
    assert_eq!(
        cancel(None, 42, |_, _| Ok(())),
        Err(CancelError::NoSuchDownload)
    );
    assert_eq!(CancelError::NotAllowed.code(), "permission_denied");
}

#[test]
fn only_a_download_that_is_over_is_cleared() {
    assert_eq!(clear_refusal(Some(&Record::parse(DONE).unwrap())), None);
    assert_eq!(
        clear_refusal(Some(&Record::parse(RUNNING).unwrap())),
        Some(ClearError::StillDownloading)
    );
    assert_eq!(clear_refusal(None), Some(ClearError::NoSuchDownload));
    assert_eq!(words(|out| write_cleared(3, out)), "Cleared 3 downloads.");
    assert_eq!(words(|out| write_cleared(1, out)), "Cleared 1 download.");
    assert_eq!(words(|out| write_cleared(0, out)), "Nothing to clear.");
}

#[test]
fn a_host_keeps_its_port_and_drops_the_user() {
    assert_eq!(host_of(b"http://10.0.2.2:8000/notes.txt"), b"10.0.2.2:8000");
    assert_eq!(host_of(b"https://me@example.com?x=1"), b"example.com");
    assert_eq!(host_of(b"https://example.com"), b"example.com");
}

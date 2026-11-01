use super::*;
use core::ffi::CStr;
use eggos_mcp::audit::LOG_PATH;

const NOW: u64 = 1_793_438_042;

pub(crate) struct Log {
    text: String,
    read: usize,
    pub(crate) failing: Option<Errno>,
}

impl Log {
    pub(crate) fn of(text: String) -> Self {
        Self {
            text,
            read: 0,
            failing: None,
        }
    }
}

impl LogFiles for Log {
    fn open_log(&mut self, path: &CStr) -> Result<usize, Errno> {
        if path.to_bytes() != LOG_PATH.as_bytes() {
            return Err(Errno::ENOENT);
        }
        self.failing.map_or(Ok(3), Err)
    }

    fn read_log(&mut self, _: usize, out: &mut [u8]) -> Result<usize, Errno> {
        let rest = &self.text.as_bytes()[self.read..];
        let count = rest.len().min(out.len());
        out[..count].copy_from_slice(&rest[..count]);
        self.read += count;
        Ok(count)
    }

    fn close_log(&mut self, _: usize) {}
}

pub(crate) fn agentlog_test_log() -> String {
    [
        "0\t0\t-\t-\tserver-start\t\t0.2.0",
        "1\t0\tclaude-code\t-\tinitialize\t{\"requested\":\"2025-11-25\"}\t",
        "2\t1\tclaude-code\tcat\tstarted\t{\"path\":\"/home/a\"}\t/home/a",
        "3\t1\tclaude-code\tcat\tok\t{\"path\":\"/home/a\"}\texit status 0, 5 bytes",
        "4\t2\tclaude-code\tcat\trefused\t{\"path\":\"/home/../etc\"}\tRefused: outside",
        "5\t3\tclaude-code\tps\tstarted\t{}\t",
        "6\t0\t-\t-\tserver-restart\t\t0.2.0",
        "not a record",
        "7\t1\tunknown (restarted)\tls\tstarted\t{}\t/home",
    ]
    .join("\n")
        + "\n"
}

fn read(query: &ActivityQuery) -> ActivityRead {
    read_activity(&mut Log::of(agentlog_test_log()), query, NOW)
}

fn rows(read: &ActivityRead) -> Vec<String> {
    read.records.iter().map(record_row).collect()
}

#[test]
fn every_call_is_a_row_newest_first() {
    let read = read(&ActivityQuery::default());
    assert_eq!(read.matching, 7);
    assert_eq!(read.error, None);
    assert_eq!(
        rows(&read),
        [
            "01-01 00:00:07 \u{b7} local \u{b7} ls \u{b7} interrupted",
            "01-01 00:00:06 \u{b7} local \u{b7} - \u{b7} server-restart",
            "01-01 00:00:05 \u{b7} local \u{b7} ps \u{b7} interrupted",
            "01-01 00:00:04 \u{b7} local \u{b7} cat \u{b7} refused",
            "01-01 00:00:03 \u{b7} local \u{b7} cat \u{b7} ok",
            "01-01 00:00:01 \u{b7} local \u{b7} - \u{b7} initialize",
            "01-01 00:00:00 \u{b7} local \u{b7} - \u{b7} server-start",
        ]
    );
    assert_eq!(
        detail_line(&read.records[0]),
        "local \u{b7} /home \u{b7} {}"
    );
    assert_eq!(
        detail_line(&read.records[4]),
        "local \u{b7} exit status 0, 5 bytes \u{b7} {\"path\":\"/home/a\"}"
    );
    assert_eq!(detail_line(&read.records[1]), "local \u{b7} 0.2.0");
}

#[test]
fn the_filters_narrow_the_rows() {
    let tool = ActivityQuery {
        tool: Some(String::from("cat")),
        ..ActivityQuery::default()
    };
    assert_eq!(read(&tool).records.len(), 2);
    let refused = ActivityQuery {
        outcome: Some(Outcome::Refused),
        ..ActivityQuery::default()
    };
    assert_eq!(
        rows(&read(&refused)),
        ["01-01 00:00:04 \u{b7} local \u{b7} cat \u{b7} refused"]
    );
    let phone = ActivityQuery {
        agent: AgentId::new(b"phone"),
        ..ActivityQuery::default()
    };
    let none = read(&phone);
    assert_eq!((none.records.len(), none.matching), (0, 0));
    let last_hour = ActivityQuery {
        within: Some(3600),
        ..ActivityQuery::default()
    };
    assert!(read(&last_hour).records.is_empty());
}

#[test]
fn only_the_newest_records_are_kept_and_all_are_counted() {
    let mut text = String::new();
    for call in 1..=40u64 {
        text.push_str(&format!("{call}\t{call}\tphone\tc\ttee\tok\t{{}}\t\n"));
    }
    let read = read_activity(&mut Log::of(text), &ActivityQuery::default(), NOW);
    assert_eq!(read.matching, 40);
    assert_eq!(read.records.len(), SHOWN_RECORDS);
    assert_eq!(read.records[0].time, 40);
    assert_eq!(read.records[SHOWN_RECORDS - 1].time, 9);
}

#[test]
fn a_log_that_cannot_be_read_gives_its_error_and_no_rows() {
    let mut log = Log::of(agentlog_test_log());
    log.failing = Some(Errno::EACCES);
    let read = read_activity(&mut log, &ActivityQuery::default(), NOW);
    assert_eq!(read.error, Some(Errno::EACCES));
    assert!(read.records.is_empty());
}

#[test]
fn a_long_id_is_shortened_and_hidden_characters_are_escaped() {
    let record = ActivityRecord {
        time: NOW,
        agent: String::from("a-very-long-agent"),
        tool: String::from("tee"),
        result: "ok",
        ..ActivityRecord::default()
    };
    assert_eq!(
        record_row(&record),
        "10-31 09:14:02 \u{b7} a-very-lo... \u{b7} tee \u{b7} ok"
    );
    let text = String::from("1\t1\tphone\tc\tt\u{202e}ee\tok\t{}\t\n");
    let read = read_activity(&mut Log::of(text), &ActivityQuery::default(), NOW);
    assert_eq!(read.records[0].tool, "t\\u{202e}ee");
}

//! The log read as calls, the one way `agentlog` and Settings' Activity view read it (D193):
//! its files oldest first ([`each_line`]), and one row per call ([`CallRows`]): a call's row
//! has its last record and says how the call ended, and a call still open when a later one
//! starts, or when the log ends, is [`INTERRUPTED`].

use super::{Filter, GENERATIONS, Outcome, Record, parse_line};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

/// The log and its older generations, oldest first: the order they are read in.
pub const FILES_OLDEST_FIRST: [&CStr; GENERATIONS + 1] = [
    c"/var/log/agent-audit.log.7",
    c"/var/log/agent-audit.log.6",
    c"/var/log/agent-audit.log.5",
    c"/var/log/agent-audit.log.4",
    c"/var/log/agent-audit.log.3",
    c"/var/log/agent-audit.log.2",
    c"/var/log/agent-audit.log.1",
    c"/var/log/agent-audit.log",
];
/// The longest line read; a longer one is no record and is skipped.
pub const LINE_BYTES: usize = 4096;
/// The result of a call still open when a later one started or the log ended.
pub const INTERRUPTED: &str = "interrupted";

const CHUNK_BYTES: usize = 4096;

/// The file operations reading the log needs.
pub trait LogFiles {
    /// Opens the file at `path` for reading; `ENOENT` when there is none.
    fn open_log(&mut self, path: &CStr) -> Result<usize, Errno>;
    /// Reads from `descriptor` into `out`; 0 at the file's end.
    fn read_log(&mut self, descriptor: usize, out: &mut [u8]) -> Result<usize, Errno>;
    /// Closes `descriptor`.
    fn close_log(&mut self, descriptor: usize);
}

/// A log file that could not be opened or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogError {
    /// The file.
    pub path: &'static CStr,
    /// Why.
    pub errno: Errno,
}

/// Gives `line` each line of [`FILES_OLDEST_FIRST`], without its newline, and `files` back to
/// use meanwhile. A missing file is skipped, and so are a line longer than [`LINE_BYTES`] and a
/// file's last bytes after its last newline (a record being written).
pub fn each_line<F: LogFiles>(
    files: &mut F,
    line: &mut dyn FnMut(&mut F, &[u8]),
) -> Result<(), LogError> {
    let mut chunk = [0u8; CHUNK_BYTES];
    let mut held = [0u8; LINE_BYTES];
    for path in FILES_OLDEST_FIRST {
        let descriptor = match files.open_log(path) {
            Ok(descriptor) => descriptor,
            Err(Errno::ENOENT) => continue,
            Err(errno) => return Err(LogError { path, errno }),
        };
        let mut length = 0;
        let mut too_long = false;
        loop {
            let count = match files.read_log(descriptor, &mut chunk) {
                Ok(0) => break,
                Ok(count) => count,
                Err(errno) => {
                    files.close_log(descriptor);
                    return Err(LogError { path, errno });
                }
            };
            for &byte in chunk.get(..count).unwrap_or_default() {
                if byte != b'\n' {
                    match held.get_mut(length) {
                        Some(slot) => {
                            *slot = byte;
                            length += 1;
                        }
                        None => too_long = true,
                    }
                    continue;
                }
                if !too_long {
                    line(files, &held[..length]);
                }
                length = 0;
                too_long = false;
            }
        }
        files.close_log(descriptor);
    }
    Ok(())
}

/// One row of the log read as calls: a call, or a record that is no call's (the server's and
/// the sessions').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallRow<'a> {
    /// The call's last record.
    pub record: Record<'a>,
    /// Whether the call was still open when a later one started or the log ended.
    pub interrupted: bool,
}

impl CallRow<'_> {
    /// How the call ended: [`INTERRUPTED`], or its last record's outcome's word.
    pub fn result(&self) -> &'static str {
        match self.interrupted {
            true => INTERRUPTED,
            false => self.record.outcome.word(),
        }
    }
}

/// Turns the log's lines, oldest first, into [`CallRow`]s: it holds the record of the call
/// still open (`asked` or `started`) until the call ends.
#[derive(Debug, Clone)]
pub struct CallRows {
    open: [u8; LINE_BYTES],
    length: usize,
}

impl Default for CallRows {
    fn default() -> Self {
        Self::new()
    }
}

fn shows(filter: &Filter<'_>, outcome: Outcome) -> bool {
    filter.outcome.is_none_or(|wanted| wanted == outcome)
}

impl CallRows {
    /// No call open yet.
    pub const fn new() -> Self {
        Self {
            open: [0; LINE_BYTES],
            length: 0,
        }
    }

    fn open(&self) -> Option<Record<'_>> {
        match self.length {
            0 => None,
            length => parse_line(self.open.get(..length).unwrap_or_default()),
        }
    }

    /// Takes the log's next line and gives `row` the rows it ends that `filter` shows, a row
    /// by how its call ended; returns how many. A line that is no record, or whose agent, tool
    /// or time `filter` does not match, is skipped.
    pub fn take(
        &mut self,
        line: &[u8],
        filter: &Filter<'_>,
        row: &mut dyn FnMut(&CallRow<'_>),
    ) -> usize {
        let Some(record) = parse_line(line) else {
            return 0;
        };
        let by_record = Filter {
            outcome: None,
            ..*filter
        };
        if !by_record.matches(&record) {
            return 0;
        }
        let mut rows = 0;
        let continues = self.open().is_some_and(|open| {
            open.call == record.call
                && (record.outcome.ends_a_call()
                    || (open.outcome == Outcome::Asked && record.outcome == Outcome::Started))
        });
        if !continues
            && let Some(open) = self.open()
            && shows(filter, open.outcome)
        {
            row(&CallRow {
                record: open,
                interrupted: true,
            });
            rows += 1;
        }
        if matches!(record.outcome, Outcome::Asked | Outcome::Started) {
            self.length = match self.open.get_mut(..line.len()) {
                Some(place) => {
                    place.copy_from_slice(line);
                    line.len()
                }
                None => 0,
            };
            return rows;
        }
        self.length = 0;
        if !shows(filter, record.outcome) {
            return rows;
        }
        row(&CallRow {
            record,
            interrupted: false,
        });
        rows + 1
    }

    /// The log has ended: gives `row` the call still open, when `filter` shows it, as
    /// interrupted; returns how many rows that is.
    pub fn finish(&mut self, filter: &Filter<'_>, row: &mut dyn FnMut(&CallRow<'_>)) -> usize {
        let rows = match self.open() {
            Some(open) if shows(filter, open.outcome) => {
                row(&CallRow {
                    record: open,
                    interrupted: true,
                });
                1
            }
            _ => 0,
        };
        self.length = 0;
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{LOG_PATH, ROTATED_PATHS};

    const LOG: &str = "0\t0\t-\t-\tserver-start\t\t0.2.0\n\
        2\t1\tclaude-code\tcat\tstarted\t{}\t/home/a\n\
        3\t1\tclaude-code\tcat\tok\t{}\texit status 0\n\
        4\t2\tphone\tc\ttee\tasked\t{}\t\n\
        5\t2\tphone\tc\ttee\tstarted\t{}\t\n\
        6\t2\tphone\tc\ttee\terror\t{}\texit status 1\n\
        7\t3\tclaude-code\tps\tstarted\t{}\t\n\
        8\t0\t-\t-\tserver-restart\t\t0.2.0\n\
        9\t1\tclaude-code\tls\tstarted\t{}\t/home\n\
        not a record\n\
        10\t1\tclaude-code\tls\tok\t{}\t\n";

    struct Files {
        log: &'static [u8],
        read: usize,
        chunk: usize,
        failing: Option<Errno>,
        opened: usize,
        closed: usize,
    }

    impl Files {
        fn new(log: &'static str) -> Self {
            Self {
                log: log.as_bytes(),
                read: 0,
                chunk: 7,
                failing: None,
                opened: 0,
                closed: 0,
            }
        }
    }

    impl LogFiles for Files {
        fn open_log(&mut self, path: &CStr) -> Result<usize, Errno> {
            if let Some(errno) = self.failing {
                return Err(errno);
            }
            if path.to_bytes() != LOG_PATH.as_bytes() {
                return Err(Errno::ENOENT);
            }
            self.opened += 1;
            Ok(3)
        }

        fn read_log(&mut self, _: usize, out: &mut [u8]) -> Result<usize, Errno> {
            let rest = &self.log[self.read..];
            let count = rest.len().min(out.len()).min(self.chunk);
            out[..count].copy_from_slice(&rest[..count]);
            self.read += count;
            Ok(count)
        }

        fn close_log(&mut self, _: usize) {
            self.closed += 1;
        }
    }

    fn rows(log: &'static str, filter: Filter<'_>) -> Vec<String> {
        let mut files = Files::new(log);
        let mut calls = CallRows::new();
        let mut shown = Vec::new();
        let mut take = |call: &CallRow<'_>| {
            shown.push(format!(
                "{} {} {}",
                call.record.time,
                String::from_utf8_lossy(call.record.tool),
                call.result()
            ));
        };
        each_line(&mut files, &mut |_, line| {
            calls.take(line, &filter, &mut take);
        })
        .unwrap();
        calls.finish(&filter, &mut take);
        assert_eq!(files.opened, files.closed);
        shown
    }

    #[test]
    fn each_call_is_one_row_and_open_calls_are_interrupted() {
        assert_eq!(
            rows(LOG, Filter::default()),
            [
                "0 - server-start",
                "3 cat ok",
                "6 tee error",
                "7 ps interrupted",
                "8 - server-restart",
                "10 ls ok",
            ]
        );
    }

    #[test]
    fn a_row_matches_an_outcome_by_how_its_call_ended() {
        let filter = Filter {
            outcome: Some(Outcome::Started),
            ..Filter::default()
        };
        assert_eq!(rows(LOG, filter), ["7 ps interrupted"]);
        let filter = Filter {
            outcome: Some(Outcome::Ok),
            ..Filter::default()
        };
        assert_eq!(rows(LOG, filter), ["3 cat ok", "10 ls ok"]);
    }

    #[test]
    fn agent_tool_and_time_skip_records_before_calls_are_paired() {
        let filter = Filter {
            agent: Some(b"phone"),
            ..Filter::default()
        };
        assert_eq!(rows(LOG, filter), ["6 tee error"]);
        let filter = Filter {
            tool: Some(b"ls"),
            since: Some(9),
            ..Filter::default()
        };
        assert_eq!(rows(LOG, filter), ["10 ls ok"]);
    }

    #[test]
    fn a_call_open_at_the_end_is_interrupted_and_a_cut_last_line_is_skipped() {
        let log = "1\t1\tc\tls\tstarted\t{}\t\n2\t1\tc\tls\tok\t{}\t";
        assert_eq!(rows(log, Filter::default()), ["1 ls interrupted"]);
    }

    #[test]
    fn a_file_that_cannot_be_read_names_itself() {
        let mut files = Files::new(LOG);
        files.failing = Some(Errno::EACCES);
        let error = each_line(&mut files, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.path, FILES_OLDEST_FIRST[0]);
        assert_eq!(error.errno, Errno::EACCES);
    }

    #[test]
    fn the_files_are_the_log_and_its_generations_oldest_first() {
        assert_eq!(FILES_OLDEST_FIRST.len(), ROTATED_PATHS.len() + 1);
        for (file, path) in FILES_OLDEST_FIRST
            .iter()
            .zip(ROTATED_PATHS.iter().rev().chain([&LOG_PATH]))
        {
            assert_eq!(file.to_bytes(), path.as_bytes());
        }
    }
}

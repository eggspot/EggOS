use crate::agent::duration_seconds;
use crate::options::wants_json;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{commands, errors};
use eggos_files::clock::deletion_date;
use eggos_mcp::audit::calls::{self, CallRow, CallRows, LINE_BYTES, LogFiles};
use eggos_mcp::audit::{self, Filter, Outcome, Record};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: agentlog [-n <count>] [--agent <id>] [--tool <name>] \
    [--outcome <result>] [--since <time>] [--raw | --json]";
const USAGE_STATUS: i32 = 2;
const HEADER: &[u8] = b"TIME  AGENT  CLIENT  TOOL  RESULT  ARGUMENTS  DETAIL\n";
const JSON_RECORD_BYTES: usize = 4 * 7 * audit::FIELD_BYTES + 512;
const FILTER_OPTIONS: [(&[u8], FilterOption); 4] = [
    (b"--agent", FilterOption::Agent),
    (b"--tool", FilterOption::Tool),
    (b"--outcome", FilterOption::Outcome),
    (b"--since", FilterOption::Since),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Rows,
    Raw,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOption {
    Agent,
    Tool,
    Outcome,
    Since,
}

impl FilterOption {
    fn takes(self) -> &'static [u8] {
        match self {
            FilterOption::Agent => b"--agent takes an agent's id",
            FilterOption::Tool => b"--tool takes a tool's name",
            FilterOption::Outcome => {
                b"--outcome takes server-start, server-restart, initialize, disconnected, \
                  asked, started, ok, error or refused"
            }
            FilterOption::Since => {
                b"--since takes a time ago (30s, 15m, 2h or 7d) or a date and time in UTC \
                  (2026-10-31 09:00)"
            }
        }
    }
}

pub fn agentlog<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut mode = Mode::Rows;
    let mut last = None;
    let mut filter = Filter::default();
    let mut index = 0;
    if wants_json(&commands::AGENTLOG, arguments) {
        output.use_json();
    }
    while let Some(argument) = arguments.get(index) {
        if let Some((option, attached)) = filter_option(argument.to_bytes()) {
            let value = match attached {
                Some(value) => Some(value),
                None => {
                    index += 1;
                    arguments.get(index).map(|value| value.to_bytes())
                }
            };
            if !value.is_some_and(|value| narrow(output, &mut filter, option, value)) {
                output.coded(&errors::USAGE, &[option.takes()]);
                return USAGE_STATUS;
            }
            index += 1;
            continue;
        }
        match argument.to_bytes() {
            b"--raw" => mode = Mode::Raw,
            b"--json" => mode = Mode::Json,
            b"-n" => {
                index += 1;
                let Some(count) = arguments
                    .get(index)
                    .and_then(|count| number(count.to_bytes()))
                else {
                    output.coded(&errors::USAGE, &[USAGE]);
                    return USAGE_STATUS;
                };
                last = Some(count);
            }
            bytes if bytes.starts_with(b"-n") => match number(&bytes[2..]) {
                Some(count) => last = Some(count),
                None => {
                    output.coded(&errors::USAGE, &[USAGE]);
                    return USAGE_STATUS;
                }
            },
            bytes if bytes.starts_with(b"-") => {
                output.invalid_option(bytes);
                output.coded(&errors::USAGE, &[USAGE]);
                return USAGE_STATUS;
            }
            _ => {
                output.coded(&errors::USAGE, &[USAGE]);
                return USAGE_STATUS;
            }
        }
        index += 1;
    }
    let total = match each_row(output, mode, &filter, &mut |_, _| {}) {
        Ok(total) => total,
        Err(status) => return status,
    };
    let skip = last.map_or(0, |last| total.saturating_sub(last));
    if mode == Mode::Rows && total > skip {
        output.write(HEADER);
    }
    let mut seen = 0;
    match each_row(output, mode, &filter, &mut |output, row| {
        if seen >= skip {
            output.write(row);
        }
        seen += 1;
    }) {
        Ok(_) => 0,
        Err(status) => status,
    }
}

fn number(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(bytes).ok()?.parse().ok()
}

fn filter_option(argument: &[u8]) -> Option<(FilterOption, Option<&[u8]>)> {
    FILTER_OPTIONS.iter().find_map(|&(name, option)| {
        match argument.strip_prefix(name)? {
            [] => Some((option, None)),
            [b'=', value @ ..] => Some((option, Some(value))),
            _ => None,
        }
    })
}

fn narrow<'f, S: System>(
    output: &mut Output<S>,
    filter: &mut Filter<'f>,
    option: FilterOption,
    value: &'f [u8],
) -> bool {
    if value.is_empty() {
        return false;
    }
    match option {
        FilterOption::Agent => filter.agent = Some(value),
        FilterOption::Tool => filter.tool = Some(value),
        FilterOption::Outcome => match Outcome::from_word(value) {
            Some(outcome) => filter.outcome = Some(outcome),
            None => return false,
        },
        FilterOption::Since => match since(output.system.now(), value) {
            Some(seconds) => filter.since = Some(seconds),
            None => return false,
        },
    }
    true
}

fn since(now: u64, text: &[u8]) -> Option<u64> {
    if let Some(ago) = duration_seconds(text) {
        return Some(now.saturating_sub(ago));
    }
    let text = core::str::from_utf8(text).ok()?;
    let moment = eggos_calendar::parse_setting(text, i64::try_from(now).ok()?).ok()?;
    u64::try_from(moment).ok()
}

impl<S: System> LogFiles for Output<S> {
    fn open_log(&mut self, path: &CStr) -> Result<usize, Errno> {
        self.system.open(path)
    }

    fn read_log(&mut self, descriptor: usize, out: &mut [u8]) -> Result<usize, Errno> {
        self.system.read(descriptor, out)
    }

    fn close_log(&mut self, descriptor: usize) {
        self.system.close(descriptor);
    }
}

fn each_row<S: System>(
    output: &mut Output<S>,
    mode: Mode,
    filter: &Filter<'_>,
    row: &mut dyn FnMut(&mut Output<S>, &[u8]),
) -> Result<usize, i32> {
    let mut rows = 0;
    let mut calls = CallRows::new();
    let mut text = [0u8; LINE_BYTES * 2];
    let read = calls::each_line(output, &mut |output, line| {
        let view = View {
            mode,
            filter,
            calls: &mut calls,
            text: &mut text,
        };
        rows += take_line(output, view, line, row);
    });
    if let Err(error) = read {
        output.file_error(error.path, error.errno);
        return Err(FAILURE);
    }
    rows += calls.finish(filter, &mut |call| {
        let shown = format_row(call, &mut text);
        row(output, &text[..shown]);
    });
    Ok(rows)
}

struct View<'v, 'f> {
    mode: Mode,
    filter: &'v Filter<'f>,
    calls: &'v mut CallRows,
    text: &'v mut [u8],
}

fn take_line<S: System>(
    output: &mut Output<S>,
    view: View<'_, '_>,
    line: &[u8],
    row: &mut dyn FnMut(&mut Output<S>, &[u8]),
) -> usize {
    let View {
        mode,
        filter,
        calls,
        text,
    } = view;
    if mode == Mode::Rows {
        return calls.take(line, filter, &mut |call| {
            let shown = format_row(call, text);
            row(output, &text[..shown]);
        });
    }
    let Some(record) = audit::parse_line(line) else {
        return 0;
    };
    if !filter.matches(&record) {
        return 0;
    }
    if mode == Mode::Json {
        let mut record_text = [0u8; JSON_RECORD_BYTES];
        let shown = format_json(&record, &mut record_text);
        row(output, &record_text[..shown]);
        return 1;
    }
    let shown = format_raw(line, text);
    row(output, &text[..shown]);
    1
}

fn format_json(record: &Record<'_>, text: &mut [u8]) -> usize {
    let mut out = JsonWriter::new(text);
    out.begin_object();
    out.key("called_at");
    match eggos_calendar::rfc3339(i64::try_from(record.time).unwrap_or(i64::MAX)) {
        Some(stamp) => out.string_lossy(&stamp),
        None => out.null(),
    }
    out.key("call");
    out.u64(record.call);
    for (key, value) in [
        ("agent", record.agent),
        ("client", record.client),
        ("tool", record.tool),
        ("result", record.outcome.word().as_bytes()),
        ("arguments", record.arguments),
        ("detail", record.detail),
    ] {
        out.key(key);
        out.string_lossy(value);
    }
    out.end_object();
    out.end_line();
    match out.overflowed() {
        true => 0,
        false => out.len(),
    }
}

fn format_raw(line: &[u8], text: &mut [u8]) -> usize {
    let mut length = 0;
    for (index, field) in line.split(|&byte| byte == b'\t').enumerate() {
        if index > 0 {
            length += put(text, length, b"\t");
        }
        let end = text.len().min(length + audit::FIELD_BYTES);
        length += audit::display_into(field, &mut text[length..end]);
    }
    length + put(text, length, b"\n")
}

fn put(text: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    match text.get_mut(at..at + bytes.len()) {
        Some(place) => {
            place.copy_from_slice(bytes);
            bytes.len()
        }
        None => 0,
    }
}

fn format_row(call: &CallRow<'_>, text: &mut [u8]) -> usize {
    let record = &call.record;
    let stamp = deletion_date(record.time);
    let mut length = put(text, 0, stamp.as_str().as_bytes());
    let result = call.result().as_bytes();
    for field in [
        record.agent,
        record.client,
        record.tool,
        result,
        record.arguments,
        record.detail,
    ] {
        length += put(text, length, b"  ");
        let end = text.len().min(length + audit::FIELD_BYTES);
        length += audit::display_into(field, &mut text[length..end]);
    }
    length + put(text, length, b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    const LOG: &str = "/var/log/agent-audit.log";

    fn run(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
        let mut output = Output::new(fake, b"agentlog");
        let status = agentlog(&mut output, arguments);
        output.finish(status)
    }

    fn log() -> String {
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

    #[test]
    fn json_writes_one_record_per_line_of_the_log() {
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"--json", c"-n", c"2"]), 0);
        let output = fake.output();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines,
            [
                "{\"called_at\":\"1970-01-01T00:00:06Z\",\"call\":0,\"agent\":\"local\",\
                 \"client\":\"-\",\"tool\":\"-\",\"result\":\"server-restart\",\
                 \"arguments\":\"\",\"detail\":\"0.2.0\"}",
                "{\"called_at\":\"1970-01-01T00:00:07Z\",\"call\":1,\"agent\":\"local\",\
                 \"client\":\"unknown (restarted)\",\"tool\":\"ls\",\"result\":\"started\",\
                 \"arguments\":\"{}\",\"detail\":\"/home\"}",
            ]
        );
        assert_eq!(run(&mut fake, &[c"--json", c"--bad"]), 2);
        assert!(
            fake.errors().contains("\"code\":\"usage\""),
            "{}",
            fake.errors()
        );
    }

    #[test]
    fn calls_are_one_row_each_and_unfinished_ones_are_interrupted() {
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines[0],
            "TIME  AGENT  CLIENT  TOOL  RESULT  ARGUMENTS  DETAIL"
        );
        assert_eq!(lines.len(), 1 + 7);
        assert!(lines[1].contains("server-start"));
        assert!(lines[2].contains("  local  claude-code  -  initialize"));
        assert!(lines[3].contains("cat  ok  {\"path\":\"/home/a\"}  exit status 0, 5 bytes"));
        assert!(lines[4].contains("cat  refused"));
        assert!(lines[5].contains("ps  interrupted"));
        assert!(lines[6].contains("server-restart"));
        assert!(lines[7].contains("unknown (restarted)  ls  interrupted"));
        assert!(lines[3].starts_with("1970-01-01T00:00:03"));
    }

    #[test]
    fn the_last_rows_and_the_raw_records_are_shown_on_request() {
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"-n", c"2"]), 0);
        let output = fake.output();
        assert_eq!(output.lines().count(), 3);
        assert!(output.lines().nth(1).unwrap().contains("server-restart"));
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"--raw"]), 0);
        assert_eq!(fake.output().lines().count(), 8);
        assert!(fake.output().starts_with("0\t0\t-\t-\tserver-start"));
    }

    #[test]
    fn the_rotated_log_comes_first_and_a_missing_log_shows_nothing() {
        let mut fake = Fake::default()
            .with_file(LOG, 0o640, b"9\t0\t-\t-\tserver-start\t\tnew\n", 0)
            .with_file(
                "/var/log/agent-audit.log.1",
                0o640,
                b"1\t0\t-\t-\tserver-start\t\told\n",
                0,
            );
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        assert!(output.find("old").unwrap() < output.find("new").unwrap());
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &[]), 0);
        assert_eq!(fake.output(), "");
    }

    #[test]
    fn every_generation_is_read_oldest_first() {
        let mut fake =
            Fake::default().with_file(LOG, 0o640, b"9\t0\t-\t-\tserver-start\t\tg0\n", 0);
        for generation in 1..=7 {
            let record = format!("{generation}\t0\t-\t-\tserver-start\t\tg{generation}\n");
            fake = fake.with_file(&format!("{LOG}.{generation}"), 0o640, record.as_bytes(), 0);
        }
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        let order: Vec<usize> = (0..=7)
            .rev()
            .map(|generation| output.find(&format!("g{generation}")).unwrap())
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{output}");
    }

    #[test]
    fn a_question_and_its_answer_are_one_row() {
        let log = [
            "1\t1\tc\ttrash\tasked\t{\"path\":\"/home/root/a\"}\teggos-elicit-1",
            "2\t1\tc\ttrash\trefused\t{\"path\":\"/home/root/a\"}\tthe user declined",
            "3\t2\tc\ttrash\tasked\t{\"path\":\"/home/root/b\"}\teggos-elicit-2",
            "4\t2\tc\ttrash\tstarted\t{\"path\":\"/home/root/b\"}\tapproved by the user",
            "5\t2\tc\ttrash\tok\t{\"path\":\"/home/root/b\"}\texit status 0, 0 bytes",
            "6\t3\tc\tapp_uninstall\tasked\t{\"name\":\"x\"}\teggos-elicit-3",
        ]
        .join("\n")
            + "\n";
        let mut fake = Fake::default().with_file(LOG, 0o640, log.as_bytes(), 0);
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1 + 3, "{output}");
        assert!(lines[1].contains("trash  refused") && lines[1].contains("declined"));
        assert!(lines[2].contains("trash  ok"));
        assert!(lines[3].contains("app_uninstall  interrupted"));
    }

    #[test]
    fn hidden_and_control_characters_are_shown_escaped() {
        let mut bytes = "1\t1\tevil\u{1b}[2J\u{202e}\tcat\tok\t{\"path\":\"/home/\u{202e}x\"}\t"
            .as_bytes()
            .to_vec();
        bytes.extend_from_slice(b"\xff\n");
        let mut fake = Fake::default().with_file(LOG, 0o640, &bytes, 0);
        assert_eq!(run(&mut fake, &[]), 0);
        let output = fake.output();
        assert!(output.contains("evil\\e[2J\\u{202e}"));
        assert!(output.contains("/home/\\u{202e}x"));
        assert!(output.contains('\u{fffd}'));
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains('\u{202e}'));
    }

    #[test]
    fn a_hand_edited_line_full_of_control_bytes_is_shown_cut_in_both_modes() {
        let mut line = b"1\t1\tc\tcat\tok\t".to_vec();
        line.extend(std::iter::repeat_n(1u8, 4000));
        line.extend_from_slice(b"\tafter the long field\n");
        assert!(line.len() < LINE_BYTES);
        for arguments in [&[c"--raw"][..], &[]] {
            let mut fake = Fake::default().with_file(LOG, 0o640, &line, 0);
            assert_eq!(run(&mut fake, arguments), 0);
            let output = fake.output();
            assert!(output.contains("...(cut)"), "{arguments:?}");
            assert!(output.contains("after the long field"), "{arguments:?}");
            assert!(output.lines().all(|row| row.len() < 2 * LINE_BYTES));
        }
    }

    fn agents_log() -> String {
        [
            "1699990000\t1\tlocal\tclaude-code\tcat\tok\t{\"path\":\"/home/a\"}\tlong ago",
            "1699999500\t2\tphone\tphone-app\tcat\tstarted\t{\"path\":\"/home/b\"}\t",
            "1699999501\t2\tphone\tphone-app\tcat\tok\t{\"path\":\"/home/b\"}\tphone read",
            "1699999600\t3\tphone\tphone-app\ttee\trefused\t{\"path\":\"/home/c\"}\ttaken back",
            "1699999700\t4\tlocal\tclaude-code\tps\tstarted\t{}\t",
        ]
        .join("\n")
            + "\n"
    }

    fn rows(arguments: &[&CStr]) -> Vec<String> {
        let mut fake = Fake::default().with_file(LOG, 0o640, agents_log().as_bytes(), 0);
        assert_eq!(run(&mut fake, arguments), 0, "{}", fake.errors());
        fake.output().lines().map(str::to_owned).collect()
    }

    #[test]
    fn rows_name_the_agent_and_filters_narrow_them() {
        let all = rows(&[]);
        assert_eq!(all.len(), 1 + 4, "{all:?}");
        assert!(all[2].contains("  phone  phone-app  cat  ok"), "{all:?}");
        let phone = rows(&[c"--agent", c"phone"]);
        assert_eq!(phone.len(), 1 + 2, "{phone:?}");
        assert!(phone[1..].iter().all(|row| row.contains("  phone  ")));
        assert_eq!(rows(&[c"--agent=local", c"--tool", c"cat"]).len(), 1 + 1);
        let refused = rows(&[c"--outcome", c"refused"]);
        assert_eq!(refused.len(), 1 + 1, "{refused:?}");
        assert!(refused[1].contains("tee  refused") && refused[1].contains("taken back"));
        let interrupted = rows(&[c"--outcome=started"]);
        assert_eq!(interrupted.len(), 1 + 1, "{interrupted:?}");
        assert!(interrupted[1].contains("ps  interrupted"));
        assert!(rows(&[c"--agent", c"tablet"]).is_empty());
    }

    #[test]
    fn since_takes_a_time_ago_or_a_date_and_time() {
        let recent = rows(&[c"--since", c"1h"]);
        assert_eq!(recent.len(), 1 + 3, "{recent:?}");
        assert!(recent.iter().all(|row| !row.contains("long ago")));
        assert_eq!(rows(&[c"--since=2023-11-14 22:00"]), recent);
        assert_eq!(rows(&[c"--since", c"2023-11-14T22:00:00Z"]), recent);
        assert_eq!(rows(&[c"--since", c"7d"]).len(), 1 + 4);
    }

    #[test]
    fn filters_apply_to_raw_and_json_records() {
        let raw = rows(&[c"--raw", c"--agent", c"phone"]);
        assert_eq!(raw.len(), 3, "{raw:?}");
        assert!(raw.iter().all(|line| line.contains("\tphone\t")));
        let json = rows(&[c"--json", c"--outcome", c"ok", c"--tool", c"cat"]);
        assert_eq!(json.len(), 2, "{json:?}");
        assert!(json[1].contains("\"agent\":\"phone\",\"client\":\"phone-app\""));
        assert_eq!(rows(&[c"--agent", c"phone", c"-n", c"1"]).len(), 1 + 1);
    }

    #[test]
    fn a_filter_without_a_good_value_is_a_usage_error() {
        for (arguments, wanted) in [
            (&[c"--agent"][..], "--agent takes"),
            (&[c"--agent="][..], "--agent takes"),
            (&[c"--outcome", c"maybe"][..], "--outcome takes"),
            (&[c"--since", c"yesterday"][..], "--since takes"),
            (&[c"--since", c"0s"][..], "--since takes"),
        ] {
            let mut fake = Fake::default();
            assert_eq!(run(&mut fake, arguments), USAGE_STATUS, "{arguments:?}");
            assert!(fake.errors().contains(wanted), "{}", fake.errors());
        }
        let mut fake = Fake::default();
        assert_eq!(run(&mut fake, &[c"--agents", c"phone"]), USAGE_STATUS);
        assert!(fake.errors().contains("unrecognized option '--agents'"));
    }

    #[test]
    fn bad_options_are_usage_errors() {
        for arguments in [&[c"-x"][..], &[c"-n"], &[c"-n", c"many"], &[c"file"]] {
            let mut fake = Fake::default();
            assert_eq!(run(&mut fake, arguments), USAGE_STATUS);
            assert!(fake.errors().contains("usage: agentlog"));
        }
        let mut fake = Fake::default().with_file(LOG, 0o640, log().as_bytes(), 0);
        assert_eq!(run(&mut fake, &[c"-n1"]), 0);
        assert_eq!(fake.output().lines().count(), 2);
    }
}

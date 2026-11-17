//! `screen-reader` (D603, docs/ux/accessibility-v0.4.md section 1.6): turns the screen reader on
//! or off through `settings set`, says how it is, prints its log, reads a window as the reader
//! would, and says a line. The words of elements come from `eggos_element_tree::reader`, as the
//! window server's do.

use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::wm::{USAGE_STATUS, number, open_device};
use core::ffi::CStr;
use core::fmt::Write;
use eggos_commands::{commands, errors};
use eggos_element_tree::reader::{self, UtteranceRecord};
use eggos_element_tree::{MAX_TREE_BYTES, elements};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const SETTINGS: &CStr = c"/bin/settings";
const READER_KEY: &CStr = c"accessibility.screen_reader";
/// `/dev/window`'s `READ_READER_LOG` and `SAY` (D603).
pub const READ_READER_LOG: u32 = 0xc018_7743;
pub const SAY: u32 = 0x4018_7744;
/// The utterances the window server's log keeps.
pub const READER_LOG_ENTRIES: usize = 256;
mod reader_log_reply {
    pub const WRITTEN: usize = 0;
    pub const LOGGED: usize = 8;
    pub const ON: usize = 12;
    pub const FOCUSED: usize = 16;
    pub const CAPTIONS_AND_RATE: usize = 20;
}
const LOG_BYTES: usize =
    READER_LOG_ENTRIES * (reader::UTTERANCE_HEADER_BYTES + reader::MAX_UTTERANCE_BYTES);
const DEFAULT_COUNT: usize = 50;
const MOST_COUNT: usize = 256;
const RECORD_BYTES: usize = 640;
const FOLLOW_POLL_MS: u64 = 200;

struct Line {
    bytes: [u8; RECORD_BYTES],
    length: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; RECORD_BYTES],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let end = self.length + text.len();
        let place = self.bytes.get_mut(self.length..end).ok_or(core::fmt::Error)?;
        place.copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

/// What `READ_READER_LOG` answered: the bytes written, the last serial, whether the reader is
/// on, whether it shows captions and its words a minute.
struct LogRead {
    written: usize,
    logged: u32,
    on: bool,
    captions: bool,
    rate: u32,
}

fn read_log<S: System>(
    output: &mut Output<S>,
    device: usize,
    since: u32,
    into: &mut [u8],
) -> Result<LogRead, Errno> {
    let mut argument = [0u8; 24];
    argument[0..4].copy_from_slice(&since.to_ne_bytes());
    argument[8..16].copy_from_slice(&(into.as_mut_ptr() as u64).to_ne_bytes());
    argument[16..24].copy_from_slice(&(into.len() as u64).to_ne_bytes());
    output
        .system
        .device_control(device, READ_READER_LOG, &mut argument)?;
    let word = |offset: usize| {
        u32::from_ne_bytes(argument[offset..offset + 4].try_into().unwrap_or([0; 4]))
    };
    Ok(LogRead {
        written: word(reader_log_reply::WRITTEN) as usize,
        logged: word(reader_log_reply::LOGGED),
        on: word(reader_log_reply::ON) != 0,
        captions: word(reader_log_reply::CAPTIONS_AND_RATE) >> 16 != 0,
        rate: word(reader_log_reply::CAPTIONS_AND_RATE) & 0xffff,
    })
}

fn time_of_day(at_ms: u64, out: &mut Line) {
    let seconds = at_ms / 1000;
    let _ = write!(
        out,
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        at_ms % 1000
    );
}

fn write_record<S: System>(output: &mut Output<S>, record: &UtteranceRecord<'_>) {
    let mut time = Line::new();
    time_of_day(record.at_ms, &mut time);
    if output.is_json() {
        let mut buffer = [0u8; RECORD_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        out.key("serial");
        out.u64(u64::from(record.serial));
        out.key("time");
        out.string(core::str::from_utf8(time.as_bytes()).unwrap_or(""));
        out.key("at_ms");
        out.u64(record.at_ms);
        out.key("source");
        out.string(record.source.name());
        out.key("priority");
        out.string(record.priority.name());
        out.key("text");
        out.string(record.text);
        out.end_object();
        out.end_line();
        output.write(out.as_bytes());
        return;
    }
    let mut line = Line::new();
    let _ = write!(
        line,
        "{} {:<9} {:<9} {}\n",
        core::str::from_utf8(time.as_bytes()).unwrap_or(""),
        record.source.name(),
        record.priority.name(),
        record.text
    );
    output.write(line.as_bytes());
}

/// Writes the records in `bytes`, the last `count` of them; returns the last serial written.
fn write_records<S: System>(output: &mut Output<S>, bytes: &[u8], count: usize) -> u32 {
    let mut total = 0;
    let mut rest = bytes;
    while let Some((_, after)) = UtteranceRecord::read(rest) {
        total += 1;
        rest = after;
    }
    let mut last = 0;
    let mut rest = bytes;
    let mut index = 0;
    while let Some((record, after)) = UtteranceRecord::read(rest) {
        if index + count >= total {
            write_record(output, &record);
        }
        last = record.serial;
        index += 1;
        rest = after;
    }
    last
}

fn turn<S: System>(output: &mut Output<S>, on: bool) -> i32 {
    let value = match on {
        true => c"true",
        false => c"false",
    };
    match output
        .system
        .start_program(SETTINGS, &[SETTINGS, c"set", READER_KEY, value])
    {
        Ok(()) => {
            output.write(match on {
                true => b"Screen reader on.\n",
                false => b"Screen reader off.\n",
            });
            0
        }
        Err(errno) => {
            output.coded(&errors::FAILED, &[b"settings: ", errno.message()]);
            FAILURE
        }
    }
}

fn status<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let parsed = parse_defined(output, arguments, &commands::SCREEN_READER_STATUS, |_, _| Ok(()));
    if parsed.is_err() {
        return USAGE_STATUS;
    }
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut records = [0u8; LOG_BYTES];
    let read = read_log(output, device, 0, &mut records);
    output.system.close(device);
    let Ok(read) = read else {
        let words: [&[u8]; 1] = [b"screen-reader: the window server did not answer"];
        output.coded(&errors::FAILED, &words);
        return FAILURE;
    };
    let mut last = "";
    let mut rest = &records[..read.written];
    while let Some((record, after)) = UtteranceRecord::read(rest) {
        last = record.text;
        rest = after;
    }
    let mut line = Line::new();
    if output.is_json() {
        let mut buffer = [0u8; RECORD_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        out.key("on");
        out.bool(read.on);
        out.key("captions");
        out.bool(read.captions);
        out.key("rate");
        out.u64(u64::from(read.rate));
        out.key("logged");
        out.u64(u64::from(read.logged));
        out.key("last");
        out.string(last);
        out.end_object();
        out.end_line();
        output.write(out.as_bytes());
        return 0;
    }
    let _ = match read.on {
        true => write!(
            line,
            "Screen reader on; captions {}; {} words per minute; {} utterances logged; last: \
             {last}\n",
            match read.captions {
                true => "on",
                false => "off",
            },
            read.rate,
            read.logged
        ),
        false => write!(line, "Screen reader off; {} utterances logged.\n", read.logged),
    };
    output.write(line.as_bytes());
    0
}

fn log<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut count = DEFAULT_COUNT;
    let mut since = 0u32;
    let mut follow = false;
    let parsed = parse_defined(output, arguments, &commands::SCREEN_READER_LOG, |output, given| {
        let value = given.value.unwrap_or_default();
        match (given.option.short, given.option.long) {
            (Some(b'n'), _) => {
                let wanted = number(value).filter(|&n| (1..=MOST_COUNT as i64).contains(&n));
                let Some(n) = wanted else {
                    output.coded(&errors::INVALID_VALUE, &[b"-n: 1 to 256, not '", value, b"'"]);
                    return Err(crate::options::Refused::Reported);
                };
                count = n as usize;
            }
            (_, Some("since")) => {
                let Some(n) = number(value).and_then(|n| u32::try_from(n).ok()) else {
                    let parts: [&[u8]; 3] = [b"--since: a serial, not '", value, b"'"];
                    output.coded(&errors::INVALID_VALUE, &parts);
                    return Err(crate::options::Refused::Reported);
                };
                since = n;
            }
            (_, Some("follow")) => follow = true,
            _ => {}
        }
        Ok(())
    });
    if parsed.is_err() {
        return USAGE_STATUS;
    }
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut records = [0u8; LOG_BYTES];
    loop {
        let read = read_log(output, device, since, &mut records);
        let Ok(read) = read else {
            output.system.close(device);
            let words: [&[u8]; 1] = [b"screen-reader: the window server did not answer"];
            output.coded(&errors::FAILED, &words);
            return FAILURE;
        };
        let last = write_records(output, &records[..read.written], count);
        if !follow {
            break;
        }
        since = since.max(last);
        count = MOST_COUNT;
        let _ = output
            .system
            .sleep(core::time::Duration::from_millis(FOLLOW_POLL_MS));
    }
    output.system.close(device);
    0
}

fn read<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut window = None;
    let parsed = parse_defined(output, arguments, &commands::SCREEN_READER_READ, |output, given| {
        if given.option.long == Some("window") {
            let value = given.value.unwrap_or_default();
            match number(value).and_then(|n| u32::try_from(n).ok()) {
                Some(id) => window = Some(id),
                None => {
                    output.coded(&errors::USAGE, &[b"invalid window id '", value, b"'"]);
                    return Err(crate::options::Refused::Reported);
                }
            }
        }
        Ok(())
    });
    if parsed.is_err() {
        return USAGE_STATUS;
    }
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let window = match window {
        Some(window) => window,
        None => {
            let mut argument = [0u8; 24];
            let focused = output
                .system
                .device_control(device, READ_READER_LOG, &mut argument)
                .map(|()| {
                    u32::from_ne_bytes(
                        argument[reader_log_reply::FOCUSED..reader_log_reply::FOCUSED + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    )
                });
            match focused {
                Ok(id) if id != 0 => id,
                _ => {
                    output.system.close(device);
                    output.coded(&errors::NO_SUCH_WINDOW, &[b"no window has the focus"]);
                    return FAILURE;
                }
            }
        }
    };
    let mut tree = [0u8; MAX_TREE_BYTES];
    let answered = output.system.window_tree(device, window, &mut tree);
    output.system.close(device);
    let answered = match answered {
        Ok(answered) if !answered.pixels_only => answered,
        _ => {
            let words: [&[u8]; 1] = [b"pixels only: the app does not describe this window"];
            output.coded(&errors::PIXELS_ONLY, &words);
            return FAILURE;
        }
    };
    let tree = &tree[..answered.length];
    for (index, element) in elements(tree).flatten().enumerate() {
        if element.role != eggos_element_tree::Role::Window && !reader::is_read(&element) {
            continue;
        }
        let (position, parent) = reader::place_of(elements(tree), index);
        let mut line = Line::new();
        let _ = reader::utterance(&element, parent, position, &mut line);
        let text = core::str::from_utf8(line.as_bytes()).unwrap_or("");
        if output.is_json() {
            let mut buffer = [0u8; RECORD_BYTES];
            let mut out = JsonWriter::new(&mut buffer);
            out.begin_object();
            out.key("element");
            out.u64(u64::from(element.id));
            out.key("text");
            out.string(text);
            out.end_object();
            out.end_line();
            output.write(out.as_bytes());
        } else {
            output.write(text.as_bytes());
            output.write(b"\n");
        }
    }
    0
}

fn say<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut queue = false;
    let Ok(operands) = parse_defined(output, arguments, &commands::SCREEN_READER_SAY, |_, given| {
        if given.option.long == Some("queue") {
            queue = true;
        }
        Ok(())
    }) else {
        return USAGE_STATUS;
    };
    let [text] = operands.all() else {
        output.coded(&errors::USAGE, &[b"usage: screen-reader say [--queue] [--] <text>"]);
        return USAGE_STATUS;
    };
    let text = text.to_bytes();
    let fits = !text.is_empty() && text.len() <= reader::MAX_UTTERANCE_BYTES;
    if !fits || core::str::from_utf8(text).is_err() {
        output.coded(&errors::INVALID_VALUE, &[b"the text is 1 to 255 bytes of UTF-8"]);
        return USAGE_STATUS;
    }
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut argument = [0u8; 24];
    argument[0..8].copy_from_slice(&(text.as_ptr() as u64).to_ne_bytes());
    argument[8..12].copy_from_slice(&(text.len() as u32).to_ne_bytes());
    argument[12..16].copy_from_slice(&u32::from(queue).to_ne_bytes());
    let said = output.system.device_control(device, SAY, &mut argument);
    output.system.close(device);
    match said {
        Ok(()) => 0,
        Err(Errno::EOPNOTSUPP) => {
            output.coded(&errors::SCREEN_READER_OFF, &[b"the screen reader is off"]);
            FAILURE
        }
        Err(errno) => {
            output.coded(&errors::FAILED, &[b"screen-reader: ", errno.message()]);
            FAILURE
        }
    }
}

/// `screen-reader on|off|status|log|read|say`.
pub fn screen_reader<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((word, rest)) = arguments.split_first() else {
        return status(output, arguments);
    };
    match word.to_bytes() {
        b"on" => turn(output, true),
        b"off" => turn(output, false),
        b"status" => status(output, rest),
        b"log" => log(output, rest),
        b"read" => read(output, rest),
        b"say" => say(output, rest),
        _ => {
            output.coded(
                &errors::USAGE,
                &[b"usage: screen-reader on|off|status|log|read|say"],
            );
            USAGE_STATUS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_drivers::desktop::protocol;

    #[test]
    fn the_requests_match_the_window_server_s() {
        assert_eq!(READ_READER_LOG, protocol::request::READ_READER_LOG);
        assert_eq!(SAY, protocol::request::SAY);
        assert_eq!(READER_LOG_ENTRIES, eggos_drivers::desktop::READER_LOG_ENTRIES);
        assert_eq!(reader_log_reply::FOCUSED, protocol::reader_log_reply::FOCUSED);
    }

    #[test]
    fn a_time_of_day_has_milliseconds() {
        let mut line = Line::new();
        time_of_day(50_712_345, &mut line);
        assert_eq!(line.as_bytes(), b"14:05:12.345");
    }
}

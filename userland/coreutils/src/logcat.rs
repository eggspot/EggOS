use crate::output::{FAILURE, Output, error_for};
use crate::system::{Duration, System};
use core::ffi::CStr;
use eggos_android_log::{self as log, Buffer, Entry, LOG_PATH, priority};
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

const MOST_FILTERS: usize = 16;
const FOLLOW_INTERVAL: Duration = Duration {
    seconds: 0,
    nanoseconds: 250_000_000,
};
const TAG_COLUMN: usize = 8;
const SECONDS_PER_DAY: u32 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Brief,
    ThreadTime,
}

#[derive(Debug, Clone, Copy)]
struct Filter<'a> {
    tag: &'a [u8],
    lowest: u8,
}

struct Request<'a> {
    dump: bool,
    clear: bool,
    format: Format,
    default_lowest: u8,
    filters: [Filter<'a>; MOST_FILTERS],
    count: usize,
}

impl<'a> Request<'a> {
    fn read<S: System>(output: &mut Output<S>, arguments: &[&'a CStr]) -> Result<Self, i32> {
        let mut request = Request {
            dump: false,
            clear: false,
            format: Format::ThreadTime,
            default_lowest: priority::VERBOSE,
            filters: [Filter {
                tag: &[],
                lowest: priority::VERBOSE,
            }; MOST_FILTERS],
            count: 0,
        };
        let mut silent = false;
        let mut every_tag = None;
        let mut options_end = false;
        let mut index = 0;
        while let Some(argument) = arguments.get(index) {
            index += 1;
            let bytes = argument.to_bytes();
            if !options_end && bytes == b"--" {
                options_end = true;
                continue;
            }
            if !options_end && bytes.len() > 1 && bytes[0] == b'-' {
                let mut letters = bytes[1..].iter();
                while let Some(&letter) = letters.next() {
                    match letter {
                        b'd' => request.dump = true,
                        b'c' => request.clear = true,
                        b's' => silent = true,
                        b'v' => {
                            let rest = letters.as_slice();
                            let value = match rest.is_empty() {
                                false => rest,
                                true => {
                                    let Some(value) = arguments.get(index) else {
                                        output.coded(
                                            &errors::USAGE,
                                            &[b"option requires an argument -- 'v'"],
                                        );
                                        return Err(FAILURE);
                                    };
                                    index += 1;
                                    value.to_bytes()
                                }
                            };
                            request.format = Self::format(output, value)?;
                            break;
                        }
                        _ => return Err(output.invalid_option(&[b'-', letter])),
                    }
                }
                continue;
            }
            let filter = Self::filter(output, bytes)?;
            if filter.tag == b"*" {
                every_tag = Some(filter.lowest);
                continue;
            }
            let Some(slot) = request.filters.get_mut(request.count) else {
                output.coded(&errors::USAGE, &[b"at most 16 filters"]);
                return Err(FAILURE);
            };
            *slot = filter;
            request.count += 1;
        }
        request.default_lowest = every_tag.unwrap_or(match silent {
            true => priority::SILENT,
            false => priority::VERBOSE,
        });
        Ok(request)
    }

    fn format<S: System>(output: &mut Output<S>, value: &[u8]) -> Result<Format, i32> {
        match value {
            b"brief" => Ok(Format::Brief),
            b"threadtime" => Ok(Format::ThreadTime),
            _ => {
                output.coded(
                    &errors::INVALID_VALUE,
                    &[b"unknown format '", value, b"': brief or threadtime"],
                );
                Err(FAILURE)
            }
        }
    }

    fn filter<S: System>(output: &mut Output<S>, spec: &'a [u8]) -> Result<Filter<'a>, i32> {
        let (tag, lowest) = match spec.iter().rposition(|&byte| byte == b':') {
            None => (spec, Some(priority::VERBOSE)),
            Some(colon) => match &spec[colon + 1..] {
                [letter] => (&spec[..colon], log::priority_of_letter(*letter)),
                _ => (&spec[..colon], None),
            },
        };
        match (tag.is_empty(), lowest) {
            (false, Some(lowest)) => Ok(Filter { tag, lowest }),
            _ => {
                output.coded(
                    &errors::USAGE,
                    &[
                        b"'",
                        spec,
                        b"' is not a filter: tag or tag:priority, a priority one of V D I W E F S",
                    ],
                );
                Err(FAILURE)
            }
        }
    }

    fn lowest_for(&self, tag: &[u8]) -> u8 {
        self.filters[..self.count]
            .iter()
            .rev()
            .find(|filter| filter.tag == tag)
            .map_or(self.default_lowest, |filter| filter.lowest)
    }

    fn shows(&self, entry: &Entry<'_>) -> bool {
        let lowest = self.lowest_for(entry.tag);
        matches!(entry.buffer, Buffer::Main | Buffer::System | Buffer::Crash)
            && lowest < priority::SILENT
            && entry.priority >= lowest
    }
}

pub fn logcat<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let request = match Request::read(output, arguments) {
        Ok(request) => request,
        Err(status) => return status,
    };
    let descriptor = match output.system.open(LOG_PATH) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(LOG_PATH, errno);
            return FAILURE;
        }
    };
    let status = match request.clear {
        true => clear(output, descriptor),
        false => show(output, descriptor, &request),
    };
    output.system.close(descriptor);
    status
}

fn clear<S: System>(output: &mut Output<S>, descriptor: usize) -> i32 {
    match output
        .system
        .device_control(descriptor, log::CLEAR_REQUEST, &mut [])
    {
        Ok(()) => 0,
        Err(Errno::EPERM) => {
            output.coded(
                &errors::PERMISSION_DENIED,
                &[b"clearing the log needs the root user"],
            );
            FAILURE
        }
        Err(errno) => {
            output.coded(
                error_for(errno),
                &[b"cannot clear the log: ", errno.message()],
            );
            FAILURE
        }
    }
}

fn show<S: System>(output: &mut Output<S>, descriptor: usize, request: &Request<'_>) -> i32 {
    let mut snapshot = [0u8; log::SNAPSHOT_SIZE];
    let mut last = [0u8; log::MAX_RECORD];
    let mut last_length = 0;
    loop {
        if let Err(errno) =
            output
                .system
                .device_control(descriptor, log::SNAPSHOT_REQUEST, &mut snapshot)
        {
            output.file_error(LOG_PATH, errno);
            return FAILURE;
        }
        let length = u32::from_le_bytes([snapshot[0], snapshot[1], snapshot[2], snapshot[3]]);
        let records = &snapshot[4..4 + (length as usize).min(log::CAPACITY)];
        let (fresh, newest) = after(records, &last[..last_length]);
        for entry in log::records(fresh) {
            if request.shows(&entry) {
                print(output, &entry, request.format);
            }
        }
        if let Some(newest) = newest.filter(|newest| newest.len() <= last.len()) {
            last[..newest.len()].copy_from_slice(newest);
            last_length = newest.len();
        }
        if request.dump {
            return 0;
        }
        output.flush();
        if output.system.sleep(FOLLOW_INTERVAL).is_err() {
            return 0;
        }
    }
}

fn after<'r>(records: &'r [u8], last: &[u8]) -> (&'r [u8], Option<&'r [u8]>) {
    let mut at = 0;
    let mut start = 0;
    let mut newest = None;
    while let Some(size) = log::record_size_at(&records[at..])
        .filter(|&size| size >= log::RECORD_HEADER_SIZE && size <= records.len() - at)
    {
        let record = &records[at..at + size];
        if !last.is_empty() && record == last {
            start = at + size;
        }
        newest = Some(record);
        at += size;
    }
    (&records[start..at], newest)
}

fn print<S: System>(output: &mut Output<S>, entry: &Entry<'_>, format: Format) {
    let message = entry.message.strip_suffix(b"\n").unwrap_or(entry.message);
    for line in message.split(|&byte| byte == b'\n') {
        match format {
            Format::Brief => {
                output.write(&[log::priority_letter(entry.priority), b'/']);
                write_tag(output, entry.tag);
                output.write(b"(");
                output.write_number(u64::from(entry.pid), 5);
                output.write(b"): ");
            }
            Format::ThreadTime => {
                write_time(output, entry.seconds, entry.nanoseconds);
                output.write(b" ");
                output.write_number(u64::from(entry.pid), 5);
                output.write(b" ");
                output.write_number(u64::from(entry.tid), 5);
                output.write(&[b' ', log::priority_letter(entry.priority), b' ']);
                write_tag(output, entry.tag);
                output.write(b": ");
            }
        }
        output.write(line);
        output.write(b"\n");
    }
}

fn write_tag<S: System>(output: &mut Output<S>, tag: &[u8]) {
    output.write(tag);
    for _ in tag.len()..TAG_COLUMN {
        output.write(b" ");
    }
}

fn write_padded<S: System>(output: &mut Output<S>, value: u32, digits: u32) {
    for place in (0..digits).rev() {
        let digit = value / 10u32.pow(place) % 10;
        output.write(&[b'0' + digit as u8]);
    }
}

fn write_time<S: System>(output: &mut Output<S>, seconds: u32, nanoseconds: u32) {
    let days = seconds / SECONDS_PER_DAY;
    let (_, month, day) = eggos_calendar::civil_from_days(i64::from(days));
    let of_day = seconds % SECONDS_PER_DAY;
    write_padded(output, month, 2);
    output.write(b"-");
    write_padded(output, day, 2);
    output.write(b" ");
    write_padded(output, of_day / 3600, 2);
    output.write(b":");
    write_padded(output, of_day / 60 % 60, 2);
    output.write(b":");
    write_padded(output, of_day % 60, 2);
    output.write(b".");
    write_padded(output, nanoseconds / 1_000_000 % 1000, 3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn entry(tag: &'static [u8], priority: u8, message: &'static [u8]) -> Entry<'static> {
        Entry {
            buffer: Buffer::Main,
            priority,
            pid: 42,
            tid: 43,
            seconds: 1_761_826_496,
            nanoseconds: 789_000_000,
            tag,
            message,
        }
    }

    fn system(entries: &[Entry<'_>]) -> Fake {
        let mut records = Vec::new();
        for entry in entries {
            let mut record = vec![0; log::record_size(entry)];
            log::encode_record(entry, &mut record).unwrap();
            records.extend_from_slice(&record);
        }
        Fake::default().with_file("/dev/android_log", 0o444, &records, 0)
    }

    fn logcat_output(entries: &[Entry<'_>], arguments: &[&CStr]) -> (i32, String, String) {
        let mut output = Output::new(system(entries), b"logcat");
        let status = logcat(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system.output(), output.system.errors())
    }

    fn sample() -> [Entry<'static>; 3] {
        [
            entry(b"eggos-test", priority::INFO, b"hello from android-console"),
            entry(b"chatty", priority::DEBUG, b"first\nsecond\n"),
            entry(b"eggos-test", priority::VERBOSE, b"details"),
        ]
    }

    #[test]
    fn brief_lines_have_the_priority_tag_and_process() {
        let (status, output, errors) = logcat_output(&sample(), &[c"-d", c"-v", c"brief"]);
        assert_eq!((status, errors.as_str()), (0, ""));
        assert_eq!(
            output,
            "I/eggos-test(   42): hello from android-console\n\
             D/chatty  (   42): first\n\
             D/chatty  (   42): second\n\
             V/eggos-test(   42): details\n"
        );
    }

    #[test]
    fn threadtime_is_the_default_with_the_date_time_and_both_ids() {
        let (_, output, _) = logcat_output(&sample()[..1], &[c"-d"]);
        assert_eq!(
            output,
            "10-30 12:14:56.789    42    43 I eggos-test: hello from android-console\n"
        );
    }

    #[test]
    fn filters_pick_tags_and_lowest_priorities() {
        let (_, output, _) = logcat_output(&sample(), &[c"-d", c"-vbrief", c"-s", c"eggos-test:I"]);
        assert_eq!(output, "I/eggos-test(   42): hello from android-console\n");
        let (_, output, _) = logcat_output(&sample(), &[c"-d", c"-v", c"brief", c"*:D"]);
        assert_eq!(output.lines().count(), 3);
        let (_, output, _) = logcat_output(&sample(), &[c"-ds", c"-v", c"brief", c"--", c"chatty"]);
        assert!(output.starts_with("D/chatty  (   42): first\n"), "{output}");
        assert_eq!(output.lines().count(), 2);
    }

    #[test]
    fn bad_options_formats_and_filters_are_refused() {
        for arguments in [
            &[c"-x"][..],
            &[c"-v", c"long"],
            &[c"-v"],
            &[c"tag:Q"],
            &[c":I"],
        ] {
            let (status, _, errors) = logcat_output(&sample(), arguments);
            assert_eq!(status, FAILURE, "{arguments:?}");
            assert!(!errors.is_empty(), "{arguments:?}");
        }
    }

    #[test]
    fn clear_empties_the_log() {
        let mut output = Output::new(system(&sample()), b"logcat");
        assert_eq!(logcat(&mut output, &[c"-c"]), 0);
        assert_eq!(logcat(&mut output, &[c"-d"]), 0);
        output.flush();
        assert_eq!(output.system.output(), "");
    }

    #[test]
    fn only_records_after_the_last_one_shown_are_new() {
        let mut records = Vec::new();
        let mut boundaries = Vec::new();
        for entry in sample() {
            let mut record = vec![0; log::record_size(&entry)];
            log::encode_record(&entry, &mut record).unwrap();
            records.extend_from_slice(&record);
            boundaries.push(records.len());
        }
        let first = &records[..boundaries[0]];
        let (fresh, newest) = after(&records, first);
        assert_eq!(fresh, &records[boundaries[0]..]);
        assert_eq!(newest, Some(&records[boundaries[1]..]));
        let (fresh, _) = after(&records, b"gone");
        assert_eq!(fresh, &records[..]);
    }
}

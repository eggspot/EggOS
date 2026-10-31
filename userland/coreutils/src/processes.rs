use crate::lines::each_line;
use crate::options::{parse_count, parse_defined};
use crate::output::{FAILURE, Output, decimal};
use crate::page::Page;
use crate::system::System;
use core::ffi::CStr;
use core::ops::ControlFlow;
use eggos_commands::{CommandDefinition, commands, errors};
use eggos_mcp::json::JsonWriter;

const PROCESSES: &CStr = c"/proc/ps";

const MEMORY: &CStr = c"/proc/meminfo";

const UPTIME: &CStr = c"/proc/uptime";

const NAME_COLUMN: usize = 6;

const SESSION_COLUMN: usize = 3;

const CPU_COLUMN: usize = 5;

const PROCESS_RECORD_BYTES: usize = 1024;

const SMALL_RECORD_BYTES: usize = 256;

const PROCESS_NUMBERS: [&str; 4] = ["pid", "parent_pid", "group_id", "session_id"];

fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&byte| byte == b' ' || byte == b'\n' || byte == b'\t')
        .filter(|field| !field.is_empty())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Chosen {
    bytes: bool,
    mebibytes: bool,
    json: bool,
}

fn no_operands<S: System>(
    output: &mut Output<S>,
    arguments: &[&CStr],
    definition: &'static CommandDefinition,
) -> Option<Chosen> {
    let mut chosen = Chosen::default();
    let operands = parse_defined(output, arguments, definition, |_, given| {
        match (given.option.short, given.option.long) {
            (Some(b'b'), _) => chosen.bytes = true,
            (Some(b'm'), _) => chosen.mebibytes = true,
            (_, Some("json")) => chosen.json = true,
            _ => {}
        }
        Ok(())
    })
    .ok()?;
    if let Some(extra) = operands.all().first() {
        output.coded(
            &errors::USAGE,
            &[b"extra operand '", extra.to_bytes(), b"'"],
        );
        return None;
    }
    Some(chosen)
}

pub fn ps<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut json = false;
    let mut page = Page::default();
    let Ok(operands) = parse_defined(output, arguments, &commands::PS, |output, given| {
        if !page.take(output, given)? {
            json |= given.is_long("json");
        }
        Ok(())
    }) else {
        return FAILURE;
    };
    if let Some(extra) = operands.all().first() {
        output.coded(
            &errors::USAGE,
            &[b"extra operand '", extra.to_bytes(), b"'"],
        );
        return FAILURE;
    }
    let Ok(after) = page.after_number(output) else {
        return FAILURE;
    };
    if !json {
        output.write(b"  PID  PPID  PGID S CMD\n");
    }
    let mut shown = 0u64;
    let mut last = None;
    let mut more = false;
    let status = each_line(output, PROCESSES, |output, line| {
        let pid = fields(line).next().and_then(parse_count);
        if after.is_some_and(|after| pid.is_none_or(|pid| pid <= after)) {
            return ControlFlow::Continue(());
        }
        if page.limit.is_some_and(|limit| shown == limit) {
            more = true;
            return ControlFlow::Break(());
        }
        shown += 1;
        last = pid;
        match json {
            true => write_process(output, line),
            false => write_process_text(output, line),
        }
        ControlFlow::Continue(())
    });
    if let (true, Some(pid)) = (more, last) {
        let mut digits = [0u8; 20];
        output.more(decimal(pid, &mut digits));
    }
    status
}

fn write_process_text<S: System>(output: &mut Output<S>, line: &[u8]) {
    for (index, field) in fields(line).enumerate() {
        if index == SESSION_COLUMN || index == CPU_COLUMN {
            continue;
        }
        if index < SESSION_COLUMN {
            for _ in field.len()..5 {
                output.write(b" ");
            }
        }
        output.write(field);
        output.write(if index < NAME_COLUMN { b" " } else { b"" });
    }
    output.write(b"\n");
}

fn write_process<S: System>(output: &mut Output<S>, line: &[u8]) {
    let mut buffer = [0u8; PROCESS_RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    let mut parts = fields(line);
    out.begin_object();
    for key in PROCESS_NUMBERS {
        out.key(key);
        match parts.next().and_then(parse_count) {
            Some(number) => out.u64(number),
            None => out.null(),
        }
    }
    out.key("state");
    out.string_lossy(parts.next().unwrap_or_default());
    out.key("cpu_ms");
    match parts.next().and_then(parse_count) {
        Some(number) => out.u64(number),
        None => out.null(),
    }
    out.key("name");
    out.begin_string();
    for (index, part) in parts.enumerate() {
        if index > 0 {
            out.string_piece(" ");
        }
        out.string_piece_lossy(part);
    }
    out.end_string();
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

pub fn free<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some(chosen) = no_operands(output, arguments, &commands::FREE) else {
        return FAILURE;
    };
    let mut values = [None::<u64>; 4];
    let status = each_line(output, MEMORY, |_, line| {
        let mut parts = fields(line);
        let index = match parts.next() {
            Some(b"MemTotal:") => 0,
            Some(b"MemFree:") => 1,
            Some(b"MemAvailable:") => 2,
            Some(b"KernelReserve:") => 3,
            _ => return ControlFlow::Continue(()),
        };
        values[index] = parts.next().and_then(parse_count);
        ControlFlow::Continue(())
    });
    if status != 0 {
        return status;
    }
    let [Some(total), Some(free), Some(available), reserve] = values else {
        output.error(&[b"/proc/meminfo: no MemTotal, MemFree or MemAvailable"]);
        return FAILURE;
    };
    if chosen.json {
        write_memory(
            output,
            [total, total.saturating_sub(free), free, available],
            reserve,
        );
        return 0;
    }
    let unit = |kilobytes: u64| match (chosen.bytes, chosen.mebibytes) {
        (true, _) => kilobytes * 1024,
        (_, true) => kilobytes / 1024,
        _ => kilobytes,
    };
    output.write(
        b"               total        used        free      shared  buff/cache   available\n",
    );
    output.write(b"Mem:   ");
    for value in [total, total.saturating_sub(free), free, 0, 0, available] {
        output.write_number(unit(value), 12);
    }
    output.write(b"\n");
    if let Some(reserve) = reserve {
        output.write(b"Reserve:");
        output.write_number(unit(reserve), 11);
        output.write(b"\n");
    }
    0
}

pub fn uptime<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some(chosen) = no_operands(output, arguments, &commands::UPTIME) else {
        return FAILURE;
    };
    let mut seconds = None;
    let status = each_line(output, UPTIME, |_, line| {
        seconds = fields(line)
            .next()
            .and_then(|field| field.split(|&byte| byte == b'.').next())
            .and_then(parse_count);
        ControlFlow::Break(())
    });
    if status != 0 {
        return status;
    }
    let Some(seconds) = seconds else {
        output.error(&[b"/proc/uptime: no time since boot"]);
        return FAILURE;
    };
    if chosen.json {
        let mut buffer = [0u8; SMALL_RECORD_BYTES];
        let mut out = JsonWriter::new(&mut buffer);
        out.begin_object();
        out.key("uptime_ms");
        out.u64(seconds.saturating_mul(1000));
        out.end_object();
        out.end_line();
        output.write(out.as_bytes());
        return 0;
    }
    output.write(b"up ");
    write_duration(output, seconds);
    output.write(b"\n");
    0
}

const MEMORY_KEYS: [&str; 4] = ["total_bytes", "used_bytes", "free_bytes", "available_bytes"];

fn write_memory<S: System>(output: &mut Output<S>, kilobytes: [u64; 4], reserve: Option<u64>) {
    let mut buffer = [0u8; SMALL_RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    for (key, value) in MEMORY_KEYS.into_iter().zip(kilobytes) {
        out.key(key);
        out.u64(value.saturating_mul(1024));
    }
    out.key("reserve_bytes");
    match reserve {
        Some(reserve) => out.u64(reserve.saturating_mul(1024)),
        None => out.null(),
    }
    out.end_object();
    out.end_line();
    output.write(out.as_bytes());
}

fn write_duration<S: System>(output: &mut Output<S>, seconds: u64) {
    let parts = [
        (seconds / 86_400, &b"day"[..]),
        (seconds / 3600 % 24, b"hour"),
        (seconds / 60 % 60, b"minute"),
    ];
    let mut first = true;
    for (count, unit) in parts {
        if count == 0 {
            continue;
        }
        if !first {
            output.write(b", ");
        }
        output.write_number(count, 0);
        output.write(b" ");
        output.write(unit);
        if count != 1 {
            output.write(b"s");
        }
        first = false;
    }
    if first {
        output.write_number(seconds, 0);
        output.write(if seconds == 1 {
            b" second"
        } else {
            b" seconds"
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn proc() -> Fake {
        Fake::default()
            .with_directory("/proc", 0)
            .with_file("/proc/ps", 0o444, b"1 0 1 1 R 120 init\n7 1 7 1 R 35 eggsh\n12 7 12 1 Z 0 sleep\n", 0)
            .with_file(
                "/proc/meminfo",
                0o444,
                b"MemTotal:         131072 kB\nMemFree:          100000 kB\nMemAvailable:     100000 kB\n",
                0,
            )
            .with_file("/proc/uptime", 0o444, b"3725.42 0.00\n", 0)
    }

    fn run(
        utility: fn(&mut Output<Fake>, &[&CStr]) -> i32,
        fake: Fake,
        arguments: &[&CStr],
    ) -> (i32, String, String) {
        let mut output = Output::new(fake, b"t");
        let status = utility(&mut output, arguments);
        let status = output.finish(status);
        assert_eq!(output.system.open_count(), 0);
        (status, output.system.output(), output.system.errors())
    }

    #[test]
    fn ps_lists_each_process_with_its_parent_group_state_and_name() {
        let (status, output, _) = run(ps, proc(), &[]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "  PID  PPID  PGID S CMD\n    1     0     1 R init\n    7     1     7 R eggsh\n   12     7    12 Z sleep\n"
        );
        let (status, _, errors) = run(ps, Fake::default(), &[]);
        assert_eq!(status, 1);
        assert_eq!(errors, "t: /proc/ps: No such file or directory\n");
    }

    #[test]
    fn ps_json_writes_a_record_per_process() {
        let (status, output, _) = run(ps, proc(), &[c"--json"]);
        assert_eq!(status, 0);
        assert_eq!(
            output.lines().next(),
            Some(
                "{\"pid\":1,\"parent_pid\":0,\"group_id\":1,\"session_id\":1,\
                 \"state\":\"R\",\"cpu_ms\":120,\"name\":\"init\"}"
            )
        );
        assert_eq!(output.lines().count(), 3);
        let (status, _, errors) = run(ps, proc(), &[c"--json", c"x"]);
        assert_eq!(status, 1);
        assert!(errors.contains("\"code\":\"usage\""), "{errors}");
        let (status, _, errors) = run(ps, Fake::default(), &[c"--json"]);
        assert_eq!(status, 1);
        assert!(errors.contains("\"code\":\"no_such_file\""), "{errors}");
    }

    #[test]
    fn ps_pages_by_process_id() {
        let (status, output, errors) = run(ps, proc(), &[c"--limit", c"2"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "  PID  PPID  PGID S CMD\n    1     0     1 R init\n    7     1     7 R eggsh\n"
        );
        assert_eq!(errors, "more: --after 7\n");
        let (status, output, errors) = run(ps, proc(), &[c"--after", c"7", c"--limit", c"2"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "  PID  PPID  PGID S CMD\n   12     7    12 Z sleep\n"
        );
        assert_eq!(errors, "");
        let (_, output, errors) = run(ps, proc(), &[c"--json", c"--limit", c"1"]);
        assert_eq!(output.lines().count(), 1);
        assert!(errors.contains("\"next_cursor\":\"1\""), "{errors}");
        let (_, output, _) = run(ps, proc(), &[c"--after", c"12"]);
        assert_eq!(output, "  PID  PPID  PGID S CMD\n");
        let (status, _, errors) = run(ps, proc(), &[c"--after", c"x7"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "t: invalid cursor 'x7'\n");
    }

    #[test]
    fn free_shows_memory_in_kibibytes_or_as_asked() {
        let (status, output, _) = run(free, proc(), &[]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "               total        used        free      shared  buff/cache   available\n\
             Mem:         131072       31072      100000           0           0      100000\n"
        );
        let (_, output, _) = run(free, proc(), &[c"-m"]);
        assert!(output.ends_with(
            "Mem:            128          30          97           0           0          97\n"
        ));
        let (status, _, errors) = run(free, proc(), &[c"x"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "t: extra operand 'x'\n");
    }

    #[test]
    fn free_shows_the_reserve_kept_for_the_kernel_when_there_is_one() {
        let fake = Fake::default().with_directory("/proc", 0).with_file(
            "/proc/meminfo",
            0o444,
            b"MemTotal:         131072 kB\nMemFree:          100000 kB\nMemAvailable:      95904 kB\n\
              KernelReserve:      4096 kB\n",
            0,
        );
        let (status, output, _) = run(free, fake, &[c"-m"]);
        assert_eq!(status, 0);
        assert_eq!(
            output,
            "               total        used        free      shared  buff/cache   available\n\
             Mem:            128          30          97           0           0          93\n\
             Reserve:          4\n"
        );
    }

    #[test]
    fn uptime_says_how_long_since_boot() {
        assert_eq!(run(uptime, proc(), &[]).1, "up 1 hour, 2 minutes\n");
        let short = Fake::default().with_file("/proc/uptime", 0o444, b"1.50 0.00\n", 0);
        assert_eq!(run(uptime, short, &[]).1, "up 1 second\n");
        let days = Fake::default().with_file("/proc/uptime", 0o444, b"90061.00 0.00\n", 0);
        assert_eq!(run(uptime, days, &[]).1, "up 1 day, 1 hour, 1 minute\n");
    }
}

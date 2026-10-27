use crate::lines::each_line;
use crate::options::{Item, parse, parse_count};
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use core::ops::ControlFlow;

const PROCESSES: &CStr = c"/proc/ps";

const MEMORY: &CStr = c"/proc/meminfo";

const UPTIME: &CStr = c"/proc/uptime";

const NAME_COLUMN: usize = 5;

fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&byte| byte == b' ' || byte == b'\n' || byte == b'\t')
        .filter(|field| !field.is_empty())
}

fn no_operands<S: System>(
    output: &mut Output<S>,
    arguments: &[&CStr],
    flags: &[u8],
) -> Option<[bool; 4]> {
    let mut seen = [false; 4];
    let operands = parse(output, arguments, flags, b"", |_, item| {
        if let Item::Flag(letter) = item
            && let Some(index) = flags.iter().position(|&flag| flag == letter)
        {
            seen[index] = true;
        }
        Ok(())
    })
    .ok()?;
    if let Some(extra) = operands.all().first() {
        output.error(&[b"extra operand '", extra.to_bytes(), b"'"]);
        return None;
    }
    Some(seen)
}

pub fn ps<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    if no_operands(output, arguments, b"Aef").is_none() {
        return FAILURE;
    }
    output.write(b"  PID  PPID  PGID S CMD\n");
    each_line(output, PROCESSES, |output, line| {
        for (index, field) in fields(line).enumerate() {
            if index == 3 {
                continue;
            }
            if index < NAME_COLUMN - 1 {
                for _ in field.len()..5 {
                    output.write(b" ");
                }
            }
            output.write(field);
            output.write(if index < NAME_COLUMN { b" " } else { b"" });
        }
        output.write(b"\n");
        ControlFlow::Continue(())
    })
}

pub fn free<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some([bytes, _, mebibytes, _]) = no_operands(output, arguments, b"bkm") else {
        return FAILURE;
    };
    let mut values = [None::<u64>; 3];
    let status = each_line(output, MEMORY, |_, line| {
        let mut parts = fields(line);
        let index = match parts.next() {
            Some(b"MemTotal:") => 0,
            Some(b"MemFree:") => 1,
            Some(b"MemAvailable:") => 2,
            _ => return ControlFlow::Continue(()),
        };
        values[index] = parts.next().and_then(parse_count);
        ControlFlow::Continue(())
    });
    if status != 0 {
        return status;
    }
    let [Some(total), Some(free), Some(available)] = values else {
        output.error(&[b"/proc/meminfo: no MemTotal, MemFree or MemAvailable"]);
        return FAILURE;
    };
    let unit = |kilobytes: u64| match (bytes, mebibytes) {
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
    0
}

pub fn uptime<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    if no_operands(output, arguments, b"p").is_none() {
        return FAILURE;
    }
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
    output.write(b"up ");
    write_duration(output, seconds);
    output.write(b"\n");
    0
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
            .with_file("/proc/ps", 0o444, b"1 0 1 1 R init\n7 1 7 1 R eggsh\n12 7 12 1 Z sleep\n", 0)
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
    fn uptime_says_how_long_since_boot() {
        assert_eq!(run(uptime, proc(), &[]).1, "up 1 hour, 2 minutes\n");
        let short = Fake::default().with_file("/proc/uptime", 0o444, b"1.50 0.00\n", 0);
        assert_eq!(run(uptime, short, &[]).1, "up 1 second\n");
        let days = Fake::default().with_file("/proc/uptime", 0o444, b"90061.00 0.00\n", 0);
        assert_eq!(run(uptime, days, &[]).1, "up 1 day, 1 hour, 1 minute\n");
    }
}

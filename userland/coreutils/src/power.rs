use crate::options::parse_defined;
use crate::output::{FAILURE, Output};
use crate::system::{Duration, System};
use core::ffi::CStr;
use eggos_commands::{CommandDefinition, commands, errors};
use eggos_runtime::errno::Errno;

pub const POWER_OFF: u32 = 0x4321_fedc;
pub const RESTART: u32 = 0x0123_4567;
pub const HALT: u32 = 0xcdef_0123;

const SIGRTMIN: u32 = 34;

const INIT_HALTS: u32 = SIGRTMIN + 3;
const INIT_POWERS_OFF: u32 = SIGRTMIN + 4;
const INIT_RESTARTS: u32 = SIGRTMIN + 5;

const INIT_ANSWER_WAIT: Duration = Duration {
    seconds: 2,
    nanoseconds: 0,
};

fn init_signal(command: u32) -> u32 {
    match command {
        HALT => INIT_HALTS,
        RESTART => INIT_RESTARTS,
        _ => INIT_POWERS_OFF,
    }
}

fn going_down_now(command: u32) -> &'static [u8] {
    match command {
        HALT => b"init did not answer; halting now",
        RESTART => b"init did not answer; restarting now",
        _ => b"init did not answer; powering off now",
    }
}

fn parse<S: System>(
    output: &mut Output<S>,
    arguments: &[&CStr],
    definition: &'static CommandDefinition,
) -> Option<bool> {
    let mut force = false;
    let parsed = parse_defined(output, arguments, definition, |_, given| {
        force |= given.option.short == Some(b'f');
        Ok(())
    });
    let operands = parsed.ok()?;
    match operands.all().first() {
        Some(extra) => {
            output.coded(
                &errors::USAGE,
                &[b"extra operand '", extra.to_bytes(), b"'"],
            );
            None
        }
        None => Some(force),
    }
}

pub fn sync<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    if parse(output, arguments, &commands::SYNC).is_none() {
        return FAILURE;
    }
    output.system.sync();
    0
}

pub fn power<S: System>(output: &mut Output<S>, arguments: &[&CStr], command: u32) -> i32 {
    let definition = match command {
        HALT => &commands::HALT,
        RESTART => &commands::REBOOT,
        _ => &commands::POWEROFF,
    };
    let Some(force) = parse(output, arguments, definition) else {
        return FAILURE;
    };
    output.flush();
    if !force {
        match output.system.signal_init(init_signal(command)) {
            Ok(()) => {
                let _ = output.system.sleep(INIT_ANSWER_WAIT);
                output.error(&[going_down_now(command)]);
                output.flush();
            }
            Err(Errno::EPERM) => {
                output.error(&[Errno::EPERM.message()]);
                return FAILURE;
            }
            Err(_) => {}
        }
    }
    output.system.sync();
    match output.system.reboot(command) {
        Ok(()) => 0,
        Err(errno) => {
            output.error(&[errno.message()]);
            FAILURE
        }
    }
}

/// Where a waiting `shutdown` keeps its process id, for `shutdown -c` (D572).
const PENDING_FILE: &CStr = c"/run/shutdown.pid";
const SIGTERM: u32 = 15;
const MESSAGE_CAPACITY: usize = 1024;
const MINUTE: u64 = 60;

/// The seconds until the time a `shutdown` operand names, at `now` (seconds since the epoch):
/// `now`, `+minutes` or `hh:mm` (UTC, tomorrow when it has passed today).
pub fn delay_of(time: &[u8], now: u64) -> Option<u64> {
    let number = |digits: &[u8]| -> Option<u64> {
        (!digits.is_empty() && digits.iter().all(u8::is_ascii_digit)).then(|| {
            digits
                .iter()
                .fold(0u64, |value, &digit| value * 10 + u64::from(digit - b'0'))
        })
    };
    match time {
        b"now" => Some(0),
        [b'+', minutes @ ..] => number(minutes).map(|minutes| minutes * MINUTE),
        _ => {
            let colon = time.iter().position(|&byte| byte == b':')?;
            let hours = number(&time[..colon]).filter(|&hours| hours < 24)?;
            let minutes = number(&time[colon + 1..]).filter(|&minutes| minutes < 60)?;
            let wanted = hours * 3600 + minutes * MINUTE;
            let today = now % 86_400;
            Some(match wanted >= today {
                true => wanted - today,
                false => wanted + 86_400 - today,
            })
        }
    }
}

fn warning(command: u32, seconds: u64, message: &[u8], out: &mut [u8; MESSAGE_CAPACITY]) -> usize {
    let what: &[u8] = match command {
        HALT => b"halt",
        RESTART => b"reboot",
        _ => b"power-off",
    };
    let mut length = 0;
    let mut push = |bytes: &[u8], length: &mut usize| {
        let count = bytes.len().min(MESSAGE_CAPACITY - *length);
        out[*length..*length + count].copy_from_slice(&bytes[..count]);
        *length += count;
    };
    push(b"The system will ", &mut length);
    push(what, &mut length);
    match seconds {
        0 => push(b" now!", &mut length),
        _ => {
            let mut digits = [0u8; 20];
            push(b" in ", &mut length);
            push(decimal(seconds.div_ceil(MINUTE), &mut digits), &mut length);
            push(b" minute(s).", &mut length);
        }
    }
    if !message.is_empty() {
        push(b"\n", &mut length);
        push(message, &mut length);
    }
    length
}

fn decimal(mut value: u64, out: &mut [u8; 20]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &out[at..];
        }
    }
}

/// `shutdown [-P|-r|-H] [-c] [-k] [time] [message]`.
pub fn shutdown<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut command = POWER_OFF;
    let (mut cancel, mut warn_only) = (false, false);
    let parsed = parse_defined(output, arguments, &commands::SHUTDOWN, |_, given| {
        match given.option.short {
            Some(b'r') => command = RESTART,
            Some(b'H') => command = HALT,
            Some(b'P') => command = POWER_OFF,
            Some(b'c') => cancel = true,
            Some(b'k') => warn_only = true,
            _ => {}
        }
        Ok(())
    });
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let operands = operands.all();
    let mut message = [0u8; MESSAGE_CAPACITY];
    let (time, rest): (&[u8], &[&CStr]) = match operands.split_first() {
        Some((first, rest)) if delay_of(first.to_bytes(), 0).is_some() => (first.to_bytes(), rest),
        _ => (b"+1", operands),
    };
    let mut length = 0;
    for (index, word) in rest.iter().enumerate() {
        let bytes = word.to_bytes();
        let separator = usize::from(index > 0);
        if length + separator + bytes.len() > MESSAGE_CAPACITY {
            break;
        }
        if separator == 1 {
            message[length] = b' ';
        }
        length += separator;
        message[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    }
    let message = &message[..length];
    if cancel {
        return cancel_pending(output, message);
    }
    let now = output.system.now();
    let Some(mut left) = delay_of(time, now) else {
        output.coded(&errors::USAGE, &[b"invalid time '", time, b"'"]);
        return FAILURE;
    };
    let pid = output.system.process_id();
    keep_pending(output, pid);
    if output.system.catch_termination().is_err() {
        output.error(&[b"cannot wait: termination signals cannot be caught"]);
    }
    loop {
        let mut text = [0u8; MESSAGE_CAPACITY];
        let text_length = warning(command, left, message, &mut text);
        broadcast(output, &text[..text_length]);
        if left == 0 {
            break;
        }
        let step = match left > 10 * MINUTE {
            true => left - 10 * MINUTE,
            false => left.min(MINUTE),
        };
        let _ = output.system.sleep(Duration {
            seconds: step,
            nanoseconds: 0,
        });
        if output.system.termination_requested() {
            let _ = output.system.remove(PENDING_FILE, false);
            return 0;
        }
        left -= step;
    }
    let _ = output.system.remove(PENDING_FILE, false);
    if warn_only {
        return 0;
    }
    power(output, &[], command)
}

fn keep_pending<S: System>(output: &mut Output<S>, pid: u32) {
    let mut digits = [0u8; 20];
    let text = decimal(u64::from(pid), &mut digits);
    if let Ok(descriptor) = output.system.create(PENDING_FILE, 0o644, true) {
        let _ = output.system.write(descriptor, text);
        output.system.close(descriptor);
    }
}

fn cancel_pending<S: System>(output: &mut Output<S>, message: &[u8]) -> i32 {
    let mut digits = [0u8; 20];
    let length = match output.system.open(PENDING_FILE) {
        Ok(descriptor) => {
            let read = output.system.read(descriptor, &mut digits).unwrap_or(0);
            output.system.close(descriptor);
            read
        }
        Err(_) => 0,
    };
    let pid = digits[..length]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .fold(0i32, |value, &digit| value * 10 + i32::from(digit - b'0'));
    if pid <= 0 {
        output.error(&[b"no shutdown is waiting"]);
        return FAILURE;
    }
    if let Err(errno) = output.system.send_signal(pid, SIGTERM) {
        output.error(&[errno.message()]);
        return FAILURE;
    }
    let _ = output.system.remove(PENDING_FILE, false);
    let mut text = [0u8; MESSAGE_CAPACITY];
    let mut length = 0;
    for part in [&b"The shutdown was cancelled."[..], b"\n", message] {
        let count = part.len().min(MESSAGE_CAPACITY - length);
        text[length..length + count].copy_from_slice(&part[..count]);
        length += count;
    }
    broadcast(output, &text[..length]);
    0
}

/// Writes `message` on the console and every pseudo-terminal, after a line saying who sent it
/// (`wall`, D572).
fn broadcast<S: System>(output: &mut Output<S>, message: &[u8]) {
    let mut terminals: [[u8; 24]; 64] = [[0; 24]; 64];
    let mut count = 0;
    let mut add = |path: &[u8], count: &mut usize| {
        if *count < terminals.len() && path.len() < 24 {
            terminals[*count][..path.len()].copy_from_slice(path);
            *count += 1;
        }
    };
    add(b"/dev/console", &mut count);
    if let Ok(descriptor) = output.system.open(c"/dev/pts") {
        let mut chunk = [0u8; 2048];
        while let Ok(length) = output.system.directory_entries(descriptor, &mut chunk) {
            if length == 0 {
                break;
            }
            for name in crate::ls::record_names(&chunk[..length]) {
                if name.iter().all(u8::is_ascii_digit) && !name.is_empty() {
                    let mut path = [0u8; 24];
                    let prefix = b"/dev/pts/";
                    path[..prefix.len()].copy_from_slice(prefix);
                    let end = (prefix.len() + name.len()).min(23);
                    path[prefix.len()..end].copy_from_slice(&name[..end - prefix.len()]);
                    add(&path[..end], &mut count);
                }
            }
        }
        output.system.close(descriptor);
    }
    for terminal in &terminals[..count] {
        let length = terminal.iter().position(|&byte| byte == 0).unwrap_or(terminal.len());
        let mut path = [0u8; 25];
        path[..length].copy_from_slice(&terminal[..length]);
        let Ok(path) = CStr::from_bytes_until_nul(&path) else {
            continue;
        };
        let Ok(descriptor) = output.system.append(path, 0o620) else {
            continue;
        };
        for part in [&b"\r\nBroadcast message from root:\r\n\r\n"[..], message, b"\r\n"] {
            let _ = output.system.write(descriptor, part);
        }
        output.system.close(descriptor);
    }
}

/// `wall [message ...]`: the message (standard input without operands) on every terminal.
pub fn wall<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let parsed = parse_defined(output, arguments, &commands::WALL, |_, _| Ok(()));
    let Ok(operands) = parsed else {
        return FAILURE;
    };
    let mut message = [0u8; MESSAGE_CAPACITY];
    let mut length = 0;
    if operands.all().is_empty() {
        while length < MESSAGE_CAPACITY {
            match output.system.read(crate::system::STANDARD_INPUT, &mut message[length..]) {
                Ok(0) | Err(_) => break,
                Ok(read) => length += read,
            }
        }
    } else {
        for (index, word) in operands.all().iter().enumerate() {
            let bytes = word.to_bytes();
            let separator = usize::from(index > 0);
            if length + separator + bytes.len() > MESSAGE_CAPACITY {
                break;
            }
            if separator == 1 {
                message[length] = b' ';
            }
            length += separator;
            message[length..length + bytes.len()].copy_from_slice(bytes);
            length += bytes.len();
        }
    }
    let text = message[..length].strip_suffix(b"\n").unwrap_or(&message[..length]);
    broadcast(output, text);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run_with(fake: Fake, arguments: &[&CStr], command: u32) -> (i32, Fake) {
        let mut output = Output::new(fake, b"poweroff");
        let status = power(&mut output, arguments, command);
        let status = output.finish(status);
        (status, output.system)
    }

    fn run(arguments: &[&CStr], command: u32, allowed: bool) -> (i32, Fake) {
        let mut fake = Fake::default();
        if allowed {
            fake.reboot_error = None;
        }
        run_with(fake, arguments, command)
    }

    #[test]
    fn each_name_asks_init_with_systemd_s_signal_for_it() {
        for (command, signal) in [(POWER_OFF, 38), (RESTART, 39), (HALT, 37)] {
            let mut fake = Fake::default();
            fake.reboot_error = None;
            fake.init_signal_error = Some(Errno::ESRCH);
            let (status, fake) = run_with(fake, &[], command);
            assert_eq!(fake.init_signals, vec![signal]);
            assert_eq!((status, &fake.rebooted), (0, &vec![command]));
            assert!(fake.slept.is_empty(), "no init: nothing to wait for");
            assert_eq!(fake.errors(), "");
        }
    }

    #[test]
    fn force_goes_straight_to_the_kernel() {
        for arguments in [&[c"-f"][..], &[c"--force"][..], &[c"-ff"][..]] {
            let (status, fake) = run(arguments, POWER_OFF, true);
            assert_eq!((status, fake.rebooted), (0, vec![POWER_OFF]));
            assert!(fake.init_signals.is_empty());
        }
    }

    #[test]
    fn an_init_that_does_not_answer_in_time_is_gone_around() {
        let (status, fake) = run(&[], RESTART, true);
        assert_eq!(fake.init_signals, vec![INIT_RESTARTS]);
        assert_eq!(fake.slept, vec![INIT_ANSWER_WAIT]);
        assert_eq!(
            fake.errors(),
            "poweroff: init did not answer; restarting now\n"
        );
        assert_eq!((status, fake.rebooted, fake.synced), (0, vec![RESTART], 1));
    }

    #[test]
    fn sync_puts_changes_on_the_disk_and_takes_no_operands() {
        let mut output = Output::new(Fake::default(), b"sync");
        let status = sync(&mut output, &[]);
        assert_eq!((output.finish(status), output.system.synced), (0, 1));
        let mut output = Output::new(Fake::default(), b"sync");
        let status = sync(&mut output, &[c"file"]);
        assert_eq!(output.finish(status), 1);
        assert_eq!(output.system.synced, 0);
        assert_eq!(output.system.errors(), "sync: extra operand 'file'\n");
    }

    #[test]
    fn a_refusal_is_reported() {
        let mut fake = Fake::default();
        fake.init_signal_error = Some(Errno::EPERM);
        let (status, fake) = run_with(fake, &[], POWER_OFF);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "poweroff: Operation not permitted\n");
        assert!(fake.rebooted.is_empty());
        let (status, fake) = run(&[c"-f"], POWER_OFF, false);
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "poweroff: Operation not permitted\n");
    }

    #[test]
    fn options_other_than_force_and_operands_are_refused_before_asking() {
        for (arguments, error) in [
            (&[c"-p"][..], "poweroff: invalid option -- 'p'\n"),
            (&[c"-fx"][..], "poweroff: invalid option -- 'x'\n"),
            (&[c"--halt"][..], "poweroff: unrecognized option '--halt'\n"),
            (&[c"now"][..], "poweroff: extra operand 'now'\n"),
            (&[c"-"][..], "poweroff: extra operand '-'\n"),
        ] {
            let (status, fake) = run(arguments, POWER_OFF, true);
            assert_eq!((status, fake.errors().as_str()), (1, error));
            assert!(fake.rebooted.is_empty());
            assert!(fake.init_signals.is_empty());
        }
    }
}

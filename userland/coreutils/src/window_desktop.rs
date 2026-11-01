//! `wm desktop` (D204): the virtual desktops. `wm desktop list [--json]` writes one line per
//! desktop, `NUMBER WINDOWS CURRENT`; `wm desktop switch <n>` shows desktop `n`; `wm desktop
//! new` makes one and shows it; `wm desktop close <n>` closes one, its windows moving to the
//! desktop before it; `wm desktop move <window> <n>` moves a window there without showing it.

use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::wm::open_device;
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

/// `LIST_DESKTOPS`.
pub const LIST_DESKTOPS: u32 = 0x8010_7722;
/// `DESKTOP_ACTION`.
pub const DESKTOP_ACTION: u32 = 0xc010_7723;

const SWITCH: u32 = 0;
const NEW: u32 = 1;
const CLOSE: u32 = 2;
const MOVE: u32 = 3;
const USAGE: &[u8] =
    b"usage: wm desktop list [--json] | switch <n> | new | close <n> | move <window> <n>";
const USAGE_STATUS: i32 = 2;

/// The `wm desktop` lines `help` shows.
pub const HELP: &[u8] =
    b"       wm desktop list [--json] | switch <n> | new | close <n> | move <window> <n>\n";

fn number(text: &CStr) -> Option<u32> {
    let text = core::str::from_utf8(text.to_bytes()).ok()?;
    text.parse().ok()
}

fn usage<S: System>(output: &mut Output<S>) -> i32 {
    output.coded(&errors::USAGE, &[USAGE]);
    USAGE_STATUS
}

fn decimal(value: u32, digits: &mut [u8; 10]) -> &[u8] {
    let mut at = digits.len();
    let mut rest = value;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    &digits[at..]
}

fn write_all<S: System>(output: &mut Output<S>, parts: &[&[u8]]) {
    for part in parts {
        output.write(part);
    }
}

fn list<S: System>(output: &mut Output<S>, device: usize, json: bool) -> i32 {
    let mut reply = [0u8; 16];
    if let Err(errno) = output
        .system
        .device_control(device, LIST_DESKTOPS, &mut reply)
    {
        output.coded(
            &errors::FAILED,
            &[b"cannot list the desktops: ", errno.message()],
        );
        return FAILURE;
    }
    let (count, current) = (reply[0], reply[1]);
    if !json {
        output.write(b"NUMBER WINDOWS CURRENT\n");
    }
    for desktop in 0..count.min(8) {
        let windows = reply[2 + usize::from(desktop)];
        let number = u32::from(desktop) + 1;
        let is_current = number == u32::from(current);
        if json {
            let mut record = [0u8; 96];
            let mut out = JsonWriter::new(&mut record);
            out.begin_object();
            out.key("number");
            out.u64(u64::from(number));
            out.key("windows");
            out.u64(u64::from(windows));
            out.key("current");
            out.bool(is_current);
            out.end_object();
            out.end_line();
            output.write(out.as_bytes());
            continue;
        }
        let mut digits = [0u8; 10];
        output.write(decimal(number, &mut digits));
        output.write(b" ");
        output.write(decimal(u32::from(windows), &mut digits));
        output.write(if is_current { b" *\n" } else { b" -\n" });
    }
    0
}

fn act<S: System>(
    output: &mut Output<S>,
    device: usize,
    action: u32,
    desktop: u32,
    window: u32,
) -> Result<u32, i32> {
    let mut request = [0u8; 16];
    for (index, value) in [action, desktop, window].into_iter().enumerate() {
        request[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    match output
        .system
        .device_control(device, DESKTOP_ACTION, &mut request)
    {
        Ok(()) => Ok(u32::from_ne_bytes([
            request[12],
            request[13],
            request[14],
            request[15],
        ])),
        Err(errno) => {
            let mut digits = [0u8; 10];
            let (error, words): (_, &[&[u8]]) = match errno {
                Errno::ENOSPC => (&errors::INVALID_VALUE, &[b"there are 8 desktops already"]),
                Errno::EBUSY => (
                    &errors::INVALID_VALUE,
                    &[b"the last desktop cannot be closed"],
                ),
                Errno::ENOENT => (&errors::NO_SUCH_WINDOW, &[b"no such window"]),
                _ => (
                    &errors::INVALID_VALUE,
                    &[b"no desktop ", decimal(desktop, &mut digits)],
                ),
            };
            output.coded(error, words);
            Err(FAILURE)
        }
    }
}

pub fn desktop<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some((verb, rest)) = arguments.split_first() else {
        return usage(output);
    };
    let rest = match rest.first() {
        Some(first) if first.to_bytes() == b"--" => &rest[1..],
        _ => rest,
    };
    let verb = verb.to_bytes();
    let json = rest.iter().any(|word| word.to_bytes() == b"--json");
    let wanted = match (verb, rest) {
        (b"list", []) => (None, 0, 0),
        (b"list", [option]) if option.to_bytes() == b"--json" => (None, 0, 0),
        (b"new", []) => (Some(NEW), 0, 0),
        (b"switch", [n]) => match number(n) {
            Some(n) => (Some(SWITCH), n, 0),
            None => return usage(output),
        },
        (b"close", [n]) => match number(n) {
            Some(n) => (Some(CLOSE), n, 0),
            None => return usage(output),
        },
        (b"move", [window, n]) => match (number(window), number(n)) {
            (Some(window), Some(n)) => (Some(MOVE), n, window),
            _ => return usage(output),
        },
        _ => return usage(output),
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let status = match wanted {
        (None, _, _) => list(output, device, json),
        (Some(action), desktop, window) => match act(output, device, action, desktop, window) {
            Ok(report) => {
                let mut digits = [0u8; 10];
                let mut more = [0u8; 10];
                match action {
                    SWITCH => write_all(
                        output,
                        &[
                            b"Switched to desktop ",
                            decimal(desktop, &mut digits),
                            b".\n",
                        ],
                    ),
                    NEW => write_all(
                        output,
                        &[
                            b"Made desktop ",
                            decimal(report, &mut digits),
                            b" and switched to it.\n",
                        ],
                    ),
                    CLOSE => write_all(
                        output,
                        &[
                            b"Closed desktop ",
                            decimal(desktop, &mut digits),
                            b"; ",
                            decimal(report, &mut more),
                            b" windows moved.\n",
                        ],
                    ),
                    _ => write_all(
                        output,
                        &[
                            b"Moved window ",
                            decimal(window, &mut digits),
                            b" to desktop ",
                            decimal(desktop, &mut more),
                            b".\n",
                        ],
                    ),
                }
                0
            }
            Err(status) => status,
        },
    };
    output.system.close(device);
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(arguments: &[&CStr]) -> (i32, String) {
        let mut fake = Fake::default();
        fake.window_server = true;
        let mut output = Output::new(&mut fake, b"wm");
        let status = desktop(&mut output, arguments);
        let status = output.finish(status);
        (
            status,
            String::from_utf8_lossy(&fake.standard_error).into_owned(),
        )
    }

    #[test]
    fn misused_verbs_are_usage_errors() {
        for arguments in [
            &[c"switch"][..],
            &[c"switch", c"x"],
            &[c"move", c"129"],
            &[c"close", c"1", c"2"],
            &[c"rename"],
            &[],
        ] {
            assert_eq!(run(arguments).0, USAGE_STATUS, "{arguments:?}");
        }
    }
}

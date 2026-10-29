use crate::output::{FAILURE, Output, error_for};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

pub const DEVICE: &CStr = c"/dev/window";
pub const LIST_WINDOW: u32 = 0xc060_770b;
pub const MANAGE_WINDOW: u32 = 0x4010_770c;
pub const CAPTURE_WINDOW: u32 = 0xc028_7713;
pub const SEND_INPUT: u32 = 0x4018_7714;
pub const GET_MENU: u32 = 0xc100_7716;
pub const CHOOSE_MENU: u32 = 0x4104_7717;
pub const LIST_SIZE: usize = 96;
pub const MANAGE_SIZE: usize = 16;
pub const CAPTURE_SIZE: usize = 40;
pub const SEND_INPUT_SIZE: usize = 24;
pub const GET_MENU_SIZE: usize = 256;
pub const CHOOSE_MENU_SIZE: usize = 260;
pub const GET_TREE: u32 = eggos_element_tree::request::GET_TREE;
pub const GET_TREE_SIZE: usize = eggos_element_tree::size::GET_TREE;
pub const ACT_ON_ELEMENT: u32 = eggos_element_tree::request::ACT_ON_ELEMENT;
pub const ACT_ON_ELEMENT_SIZE: usize = eggos_element_tree::size::ACT_ON_ELEMENT;
pub const KERNEL_CLIENT: u32 = 0xffff;
pub const HEADER: &[u8] = b"ID STATE X Y WIDTH HEIGHT CLIENT TITLE\n";
pub const USAGE_STATUS: i32 = 2;
pub const BUSY_STATUS: i32 = 75;

const USAGE: &[u8] =
    b"usage: wm list [--json] | wm focus|minimise|maximise|restore|close [--] <id> | \
      wm move [--] <id> <x> <y> | wm size [--] <id> <width> <height> | \
      wm capture [-o <file.png>] [--max-width <n>] [--max-bytes <n>] [--] <id> | \
      wm click [--button left|right|middle] [--] <id> <x> <y> | wm type [--] <id> <text> | \
      wm press [--] <id> <keys> | wm scroll [--at <x> <y>] [--] <id> <dx> <dy> | \
      wm menu [--] <id> | wm menu --choose [--label <label>] [--] <id> <menu> <entry> | \
      wm show [--json] [--] <id> | \
      wm act [--] <id> <element> invoke|focus|set-text|select [<text>] | wm help";
const TITLE_AT: usize = 32;
const TITLE_CAPACITY: usize = 64;
const WINDOW_RECORD_BYTES: usize = 1024;

const ACTIONS: [(&[u8], u32, usize); 7] = [
    (b"focus", 0, 0),
    (b"minimise", 1, 0),
    (b"maximise", 2, 0),
    (b"restore", 3, 0),
    (b"close", 4, 0),
    (b"move", 5, 2),
    (b"size", 6, 2),
];

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

pub fn number(text: &[u8]) -> Option<i32> {
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        _ => (false, text),
    };
    if digits.is_empty() || digits.len() > 9 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0i32, |value, &digit| value * 10 + i32::from(digit - b'0'));
    Some(if negative { -value } else { value })
}

pub fn usage<S: System>(output: &mut Output<S>) -> i32 {
    output.coded(&errors::USAGE, &[USAGE]);
    USAGE_STATUS
}

pub fn open_device<S: System>(output: &mut Output<S>) -> Option<usize> {
    match output.system.open(DEVICE) {
        Ok(device) => Some(device),
        Err(errno) => {
            output.coded(
                &errors::NO_WINDOW_SERVER,
                &[DEVICE.to_bytes(), b": ", errno.message()],
            );
            None
        }
    }
}

pub fn refused<S: System>(output: &mut Output<S>, window: &[u8], errno: Errno) -> i32 {
    match errno {
        Errno::EBUSY => {
            output.coded(
                &errors::PERSON_ACTIVE,
                &[
                    window,
                    b": a person is using the machine; try again in a moment",
                ],
            );
            BUSY_STATUS
        }
        Errno::ENOENT => {
            output.coded(&errors::NO_SUCH_WINDOW, &[window, b": no such window"]);
            FAILURE
        }
        _ => {
            output.coded(error_for(errno), &[window, b": ", errno.message()]);
            FAILURE
        }
    }
}

fn state_name(state: u32) -> &'static [u8] {
    match state {
        1 => b"minimised",
        2 => b"maximised",
        _ => b"normal",
    }
}

fn list<S: System>(output: &mut Output<S>, device: usize) -> i32 {
    if !output.is_json() {
        output.write(HEADER);
    }
    for index in 0.. {
        let mut entry = [0u8; LIST_SIZE];
        entry[..4].copy_from_slice(&(index as u32).to_ne_bytes());
        match output
            .system
            .device_control(device, LIST_WINDOW, &mut entry)
        {
            Ok(()) => {}
            Err(Errno::ENOENT) => break,
            Err(errno) => {
                output.coded(
                    error_for(errno),
                    &[b"cannot list the windows: ", errno.message()],
                );
                return FAILURE;
            }
        }
        if output.is_json() {
            write_window(output, &entry);
            continue;
        }
        for (field, offset) in [(0, 0), (1, 8), (2, 12), (3, 16), (4, 20)] {
            if field == 1 {
                output.write(state_name(word(&entry, 4)));
                output.write(b" ");
            }
            output.write_number(u64::from(word(&entry, offset)), 0);
            output.write(b" ");
        }
        match word(&entry, 24) {
            KERNEL_CLIENT => output.write(b"kernel"),
            client => output.write_number(u64::from(client), 0),
        }
        output.write(b" ");
        let length = (word(&entry, 28) as usize).min(TITLE_CAPACITY);
        output.write(&entry[TITLE_AT..TITLE_AT + length]);
        output.write(b"\n");
    }
    0
}

fn write_window<S: System>(output: &mut Output<S>, entry: &[u8; LIST_SIZE]) {
    let mut buffer = [0u8; WINDOW_RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("id");
    out.u64(u64::from(word(entry, 0)));
    out.key("state");
    out.string_lossy(state_name(word(entry, 4)));
    for (key, offset) in [("x", 8), ("y", 12), ("width", 16), ("height", 20)] {
        out.key(key);
        match key {
            "x" | "y" => out.i64(i64::from(word(entry, offset) as i32)),
            _ => out.u64(u64::from(word(entry, offset))),
        }
    }
    out.key("client_pid");
    match word(entry, 24) {
        KERNEL_CLIENT => out.null(),
        client => out.u64(u64::from(client)),
    }
    out.key("title");
    let length = (word(entry, 28) as usize).min(TITLE_CAPACITY);
    out.string_lossy(&entry[TITLE_AT..TITLE_AT + length]);
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

pub fn wm<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    match arguments.first().map(|verb| verb.to_bytes()) {
        Some(b"capture") => return crate::window_capture::capture(output, &arguments[1..]),
        Some(verb @ (b"click" | b"type" | b"press" | b"scroll")) => {
            return crate::window_input::send(output, verb, &arguments[1..]);
        }
        Some(b"menu") => return crate::window_menu::menu(output, &arguments[1..]),
        Some(b"show") => return crate::window_tree::show(output, &arguments[1..]),
        Some(b"act") => return crate::window_tree::act(output, &arguments[1..]),
        Some(b"help") if arguments.len() == 1 => {
            output.write(USAGE);
            output.write(b"\n");
            output.write(crate::window_input::HELP);
            output.write(crate::window_menu::HELP);
            output.write(crate::window_tree::HELP);
            return 0;
        }
        _ => {}
    }
    let Some((verb, rest)) = arguments.split_first() else {
        return usage(output);
    };
    let verb = verb.to_bytes();
    if verb == b"list" {
        match rest {
            [] => {}
            [json] if json.to_bytes() == b"--json" => output.use_json(),
            _ => return usage(output),
        }
        let Some(device) = open_device(output) else {
            return FAILURE;
        };
        let status = list(output, device);
        output.system.close(device);
        return status;
    }
    let Some(&(_, code, extra)) = ACTIONS.iter().find(|(name, _, _)| *name == verb) else {
        return usage(output);
    };
    let operands = match rest.first() {
        Some(first) if first.to_bytes() == b"--" => &rest[1..],
        _ => rest,
    };
    if operands.len() != 1 + extra || operands.iter().any(|operand| operand.is_empty()) {
        return usage(output);
    }
    let operands: [&[u8]; 3] = core::array::from_fn(|index| {
        operands
            .get(index)
            .map_or(&b""[..], |operand| operand.to_bytes())
    });
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let status = manage(output, device, code, &operands);
    output.system.close(device);
    status
}

fn manage<S: System>(
    output: &mut Output<S>,
    device: usize,
    action: u32,
    operands: &[&[u8]],
) -> i32 {
    let mut values = [0i32; 3];
    for (value, operand) in values.iter_mut().zip(operands) {
        if operand.is_empty() {
            continue;
        }
        let Some(parsed) = number(operand) else {
            output.coded(&errors::USAGE, &[b"invalid number '", operand, b"'"]);
            return USAGE_STATUS;
        };
        *value = parsed;
    }
    let mut request = [0u8; MANAGE_SIZE];
    for (index, value) in [values[0] as u32, action, values[1] as u32, values[2] as u32]
        .into_iter()
        .enumerate()
    {
        request[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    match output
        .system
        .device_control(device, MANAGE_WINDOW, &mut request)
    {
        Ok(()) => 0,
        Err(errno) => refused(output, operands[0], errno),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow};

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"wm");
        let status = wm(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    fn windows() -> Fake {
        let mut fake = Fake::default();
        fake.windows = vec![
            FakeWindow {
                id: 64,
                state: 0,
                rect: [311, 192, 658, 458],
                client: KERNEL_CLIENT,
                title: b"Terminal".to_vec(),
            },
            FakeWindow {
                id: 129,
                state: 2,
                rect: [0, 28, 1280, 772],
                client: 67,
                title: b"first".to_vec(),
            },
        ];
        fake
    }

    #[test]
    fn list_writes_each_window_s_id_state_geometry_client_and_title() {
        let (status, fake) = run(&[c"list"], windows());
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "ID STATE X Y WIDTH HEIGHT CLIENT TITLE\n\
             64 normal 311 192 658 458 kernel Terminal\n\
             129 maximised 0 28 1280 772 67 first\n"
        );
    }

    #[test]
    fn each_action_asks_the_server_with_its_number_and_operands() {
        let mut fake = windows();
        for (arguments, expected) in [
            (&[c"focus", c"129"][..], [129, 0, 0, 0]),
            (&[c"minimise", c"129"][..], [129, 1, 0, 0]),
            (&[c"maximise", c"129"][..], [129, 2, 0, 0]),
            (&[c"restore", c"129"][..], [129, 3, 0, 0]),
            (&[c"close", c"129"][..], [129, 4, 0, 0]),
            (
                &[c"move", c"129", c"-10", c"40"][..],
                [129, 5, -10i32 as u32, 40],
            ),
            (&[c"size", c"129", c"500", c"300"][..], [129, 6, 500, 300]),
        ] {
            let status;
            (status, fake) = run(arguments, fake);
            assert_eq!(status, 0);
            assert_eq!(fake.managed.last(), Some(&expected));
        }
        assert_eq!(fake.errors(), "");
    }

    #[test]
    fn json_lists_each_window_as_a_record() {
        let (status, fake) = run(&[c"list", c"--json"], windows());
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "{\"id\":64,\"state\":\"normal\",\"x\":311,\"y\":192,\"width\":658,\
             \"height\":458,\"client_pid\":null,\"title\":\"Terminal\"}\n\
             {\"id\":129,\"state\":\"maximised\",\"x\":0,\"y\":28,\"width\":1280,\
             \"height\":772,\"client_pid\":67,\"title\":\"first\"}\n"
        );
        let mut fake = windows();
        fake.window_server = false;
        let (status, fake) = run(&[c"list", c"--json"], fake);
        assert_eq!(status, 1);
        assert!(
            fake.errors().contains("\"code\":\"no_window_server\""),
            "{}",
            fake.errors()
        );
    }

    #[test]
    fn the_verbs_take_two_dashes_before_the_window_as_the_mcp_server_passes_them() {
        let (status, fake) = run(&[c"focus", c"--", c"129"], windows());
        assert_eq!(status, 0);
        assert_eq!(fake.managed.last(), Some(&[129, 0, 0, 0]));
        let (status, fake) = run(&[c"size", c"--", c"129", c"640", c"480"], windows());
        assert_eq!(status, 0);
        assert_eq!(fake.managed.last(), Some(&[129, 6, 640, 480]));
        let (status, _) = run(&[c"focus", c"--"], windows());
        assert_eq!(status, 2);
    }

    #[test]
    fn a_refused_request_is_reported_with_the_window() {
        let (status, fake) = run(&[c"close", c"7"], windows());
        assert_eq!(status, 1);
        assert_eq!(fake.errors(), "wm: 7: Invalid argument\n");
    }

    #[test]
    fn bad_commands_and_operands_are_usage_errors() {
        for arguments in [
            &[][..],
            &[c"list", c"x"][..],
            &[c"close"][..],
            &[c"move", c"1", c"2"][..],
            &[c"frobnicate", c"1"][..],
        ] {
            let (status, fake) = run(arguments, windows());
            assert_eq!(status, 2, "{arguments:?}");
            assert!(fake.errors().starts_with("wm: usage: wm list"));
            assert!(fake.managed.is_empty());
        }
        let (status, fake) = run(&[c"close", c"x1"], windows());
        assert_eq!(status, 2);
        assert_eq!(fake.errors(), "wm: invalid number 'x1'\n");
    }

    #[test]
    fn the_mcp_server_knows_wm_s_busy_status_and_largest_window_id() {
        assert_eq!(eggos_mcp::tools::BUSY_STATUS, BUSY_STATUS);
        let largest = eggos_mcp::arguments::MAX_WINDOW_ID.to_string();
        assert_eq!(number(largest.as_bytes()), Some(999_999_999));
        assert_eq!(number(format!("{largest}0").as_bytes()), None);
    }

    #[test]
    fn without_a_window_server_the_device_cannot_be_opened() {
        let mut fake = windows();
        fake.window_server = false;
        let (status, fake) = run(&[c"list"], fake);
        assert_eq!(status, 1);
        assert!(fake.errors().starts_with("wm: /dev/window: "));
    }
}

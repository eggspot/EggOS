use crate::options::parse_defined;
use crate::output::{FAILURE, Output, decimal, error_for};
use crate::page::Page;
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::commands::{self, LIST_PAGE_LIMIT};
use eggos_commands::page::Smallest;
use eggos_commands::{CommandDefinition, errors};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

pub const DEVICE: &CStr = c"/dev/window";
pub const LIST_WINDOW: u32 = 0xc060_770b;
pub const MANAGE_WINDOW: u32 = 0x4010_770c;
/// `/dev/window`'s `SHELL_NAVIGATE` (D303): 0 home, 1 back.
pub const SHELL_NAVIGATE: u32 = 0x4004_772e;
const FIXED_BY_FORM_ERRNO: Errno = Errno(30);
pub const CAPTURE_WINDOW: u32 = 0xc028_7713;
pub const SEND_INPUT: u32 = 0xc018_7714;
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
pub const NOT_RESPONDING_STATE: u32 = 0x100;
pub const NOT_RESPONDING_ERRNO: Errno = Errno(107);
pub const NOT_RESPONDING_SUFFIX: &[u8] = b" (not responding)";
const STATE_NUMBER: u32 = 0xff;

const USAGE: &[u8] =
    b"usage: wm list [--json] | wm focus|minimise|maximise|restore|close [--] <id> | \
      wm move [--] <id> <x> <y> | wm size [--] <id> <width> <height> | \
      wm capture [-o <file.png>] [--max-width <n>] [--max-bytes <n>] [--] <id> | \
      wm click [--button left|right|middle] [--double] [--] <id> <x> <y> | \
      wm drag [--button left|right|middle] [--] <id> <x> <y> <to-x> <to-y> | \
      wm type [--] <id> <text> | \
      wm press [--] <id> <keys> | wm scroll [--at <x> <y>] [--] <id> <dx> <dy> | \
      wm menu [--] <id> | wm menu --choose [--label <label>] [--] <id> <menu> <entry> | \
      wm show [--json] [--] <id> | \
      wm act [--] <id> <element> invoke|focus|set-text|select [<text>] | wm help";
const TITLE_AT: usize = 32;
const PAGE_SLOTS: usize = LIST_PAGE_LIMIT as usize + 1;
const TITLE_CAPACITY: usize = 64;
const WINDOW_RECORD_BYTES: usize = 1024;

const ACTIONS: [(&CommandDefinition, u32); 7] = [
    (&commands::WM_FOCUS, 0),
    (&commands::WM_MINIMISE, 1),
    (&commands::WM_MAXIMISE, 2),
    (&commands::WM_RESTORE, 3),
    (&commands::WM_CLOSE, 4),
    (&commands::WM_MOVE, 5),
    (&commands::WM_SIZE, 6),
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
        FIXED_BY_FORM_ERRNO => {
            output.coded(
                &errors::FIXED_BY_FORM,
                &[window, b": each window fills the screen in this form; show it with wm focus"],
            );
            FAILURE
        }
        NOT_RESPONDING_ERRNO => {
            output.coded(
                &errors::NOT_RESPONDING,
                &[
                    window,
                    b": not responding: its app has left its events unread; try again later, \
                      or end it with wm close",
                ],
            );
            BUSY_STATUS
        }
        _ => {
            output.coded(error_for(errno), &[window, b": ", errno.message()]);
            FAILURE
        }
    }
}

fn state_name(state: u32) -> &'static [u8] {
    match state & STATE_NUMBER {
        1 => b"minimised",
        2 => b"maximised",
        _ => b"normal",
    }
}

fn each_window<S: System>(
    output: &mut Output<S>,
    device: usize,
    mut on_window: impl FnMut(&mut Output<S>, &[u8; LIST_SIZE]),
) -> Result<(), i32> {
    for index in 0u32.. {
        let mut entry = [0u8; LIST_SIZE];
        entry[..4].copy_from_slice(&index.to_ne_bytes());
        match output
            .system
            .device_control(device, LIST_WINDOW, &mut entry)
        {
            Ok(()) => on_window(output, &entry),
            Err(Errno::ENOENT) => break,
            Err(errno) => {
                output.coded(
                    error_for(errno),
                    &[b"cannot list the windows: ", errno.message()],
                );
                return Err(FAILURE);
            }
        }
    }
    Ok(())
}

fn list<S: System>(output: &mut Output<S>, device: usize, page: &Page, after: Option<u64>) -> i32 {
    let comes_after = |id: u32| after.is_none_or(|after| u64::from(id) > after);
    let mut selected = None;
    if let Some(limit) = page.limit {
        let mut smallest = Smallest::<4, PAGE_SLOTS>::new(limit as usize);
        let walked = each_window(output, device, |_, entry| {
            let id = word(entry, 0);
            if comes_after(id) {
                smallest.offer(&id.to_be_bytes());
            }
        });
        if let Err(status) = walked {
            return status;
        }
        selected = Some(smallest);
    }
    if !output.is_json() {
        output.write(HEADER);
    }
    let walked = each_window(output, device, |output, entry| {
        let id = word(entry, 0);
        let shown = match &selected {
            Some(smallest) => smallest.holds(&id.to_be_bytes()),
            None => comes_after(id),
        };
        if shown {
            write_entry(output, entry);
        }
    });
    if let Err(status) = walked {
        return status;
    }
    let last = selected
        .as_ref()
        .filter(|smallest| smallest.more())
        .and_then(|smallest| smallest.last())
        .and_then(|last| <[u8; 4]>::try_from(last).ok());
    if let Some(last) = last {
        let mut digits = [0u8; 20];
        output.more(decimal(u64::from(u32::from_be_bytes(last)), &mut digits));
    }
    0
}

fn write_entry<S: System>(output: &mut Output<S>, entry: &[u8; LIST_SIZE]) {
    if output.is_json() {
        write_window(output, entry);
        return;
    }
    for (field, offset) in [(0, 0), (1, 8), (2, 12), (3, 16), (4, 20)] {
        if field == 1 {
            output.write(state_name(word(entry, 4)));
            output.write(b" ");
        }
        let value = word(entry, offset);
        if matches!(field, 1 | 2) && (value as i32) < 0 {
            output.write(b"-");
        }
        match field {
            1 | 2 => output.write_number(u64::from((value as i32).unsigned_abs()), 0),
            _ => output.write_number(u64::from(value), 0),
        }
        output.write(b" ");
    }
    match word(entry, 24) {
        KERNEL_CLIENT => output.write(b"kernel"),
        client => output.write_number(u64::from(client), 0),
    }
    output.write(b" ");
    let length = (word(entry, 28) as usize).min(TITLE_CAPACITY);
    output.write(&entry[TITLE_AT..TITLE_AT + length]);
    if word(entry, 4) & NOT_RESPONDING_STATE != 0 {
        output.write(NOT_RESPONDING_SUFFIX);
    }
    output.write(b"\n");
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
    out.key("responding");
    out.bool(word(entry, 4) & NOT_RESPONDING_STATE == 0);
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
        Some(verb @ (b"click" | b"drag" | b"type" | b"press" | b"scroll")) => {
            return crate::window_input::send(output, verb, &arguments[1..]);
        }
        Some(b"menu") => return crate::window_menu::menu(output, &arguments[1..]),
        Some(b"show") => return crate::window_tree::show(output, &arguments[1..]),
        Some(b"act") => return crate::window_tree::act(output, &arguments[1..]),
        Some(b"desktop") => return crate::window_desktop::desktop(output, &arguments[1..]),
        Some(b"help") if arguments.len() == 1 => {
            output.write(USAGE);
            output.write(b"\n");
            output.write(crate::window_input::HELP);
            output.write(crate::window_menu::HELP);
            output.write(crate::window_tree::HELP);
            output.write(crate::window_desktop::HELP);
            return 0;
        }
        _ => {}
    }
    let Some((verb, rest)) = arguments.split_first() else {
        return usage(output);
    };
    let verb = verb.to_bytes();
    if let Some((definition, to)) = match verb {
        b"home" => Some((&commands::WM_HOME, 0u32)),
        b"back" => Some((&commands::WM_BACK, 1u32)),
        _ => None,
    } {
        let Ok(operands) = parse_defined(output, rest, definition, |_, _| Ok(())) else {
            return USAGE_STATUS;
        };
        if !operands.all().is_empty() {
            return usage(output);
        }
        let Some(device) = open_device(output) else {
            return FAILURE;
        };
        let mut request = to.to_ne_bytes();
        let status = match output.system.device_control(device, SHELL_NAVIGATE, &mut request) {
            Ok(()) => 0,
            Err(Errno::EINVAL) => {
                output.coded(
                    &errors::NOT_IN_THIS_FORM,
                    &[verb, b": the shell shows no home screen in this form"],
                );
                FAILURE
            }
            Err(errno) => {
                output.coded(error_for(errno), &[verb, b": ", errno.message()]);
                FAILURE
            }
        };
        output.system.close(device);
        return status;
    }
    if verb == b"list" {
        let mut page = Page::default();
        let parsed = parse_defined(output, rest, &commands::WM_LIST, |output, given| {
            page.take(output, given).map(|_| ())
        });
        let Ok(operands) = parsed else {
            return USAGE_STATUS;
        };
        if !operands.all().is_empty() {
            return usage(output);
        }
        let Ok(after) = page.after_number(output) else {
            return USAGE_STATUS;
        };
        let Some(device) = open_device(output) else {
            return FAILURE;
        };
        let status = list(output, device, &page, after);
        output.system.close(device);
        return status;
    }
    let Some(&(definition, code)) = ACTIONS
        .iter()
        .find(|(definition, _)| definition.words[1].as_bytes() == verb)
    else {
        return usage(output);
    };
    let Ok(operands) = parse_defined(output, rest, definition, |_, _| Ok(())) else {
        return USAGE_STATUS;
    };
    let operands = operands.all();
    if operands.len() != definition.operands.len()
        || operands.iter().any(|operand| operand.is_empty())
    {
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
             \"height\":458,\"client_pid\":null,\"responding\":true,\"title\":\"Terminal\"}\n\
             {\"id\":129,\"state\":\"maximised\",\"x\":0,\"y\":28,\"width\":1280,\
             \"height\":772,\"client_pid\":67,\"responding\":true,\"title\":\"first\"}\n"
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

    fn three_windows() -> Fake {
        let mut fake = windows();
        fake.windows.push(FakeWindow {
            id: 3,
            state: 1,
            rect: [0, 0, 100, 100],
            client: 70,
            title: b"third".to_vec(),
        });
        fake
    }

    #[test]
    fn list_pages_by_window_id_and_keeps_the_stacking_order_within_a_page() {
        let (status, fake) = run(&[c"list", c"--limit", c"2"], three_windows());
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "ID STATE X Y WIDTH HEIGHT CLIENT TITLE\n\
             64 normal 311 192 658 458 kernel Terminal\n\
             3 minimised 0 0 100 100 70 third\n"
        );
        assert_eq!(fake.errors(), "more: --after 64\n");
        let arguments = [c"list", c"--after", c"64", c"--limit", c"2"];
        let (status, fake) = run(&arguments, three_windows());
        assert_eq!(status, 0);
        assert_eq!(
            fake.output(),
            "ID STATE X Y WIDTH HEIGHT CLIENT TITLE\n\
             129 maximised 0 28 1280 772 67 first\n"
        );
        assert_eq!(fake.errors(), "");
        let arguments = [c"list", c"--json", c"--after", c"64"];
        let (status, fake) = run(&arguments, three_windows());
        assert_eq!(status, 0);
        assert_eq!(fake.output().lines().count(), 1);
        let (status, fake) = run(&[c"list", c"--after", c"x"], three_windows());
        assert_eq!(status, 2);
        assert_eq!(fake.errors(), "wm: invalid cursor 'x'\n");
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
    fn the_window_verbs_parse_from_the_registry_with_negative_places_as_operands() {
        let (status, fake) = run(&[c"move", c"129", c"-10", c"-3"], windows());
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.managed.last(),
            Some(&[129, 5, -10i32 as u32, -3i32 as u32])
        );
        let (status, fake) = run(&[c"move", c"--", c"129", c"-10", c"40"], windows());
        assert_eq!(status, 0);
        assert_eq!(fake.managed.last(), Some(&[129, 5, -10i32 as u32, 40]));
        for (arguments, error) in [
            (
                &[c"focus", c"-x", c"129"][..],
                "wm: invalid option -- 'x'\n",
            ),
            (
                &[c"close", c"--force", c"129"][..],
                "wm: unrecognized option '--force'\n",
            ),
            (
                &[c"size", c"129", c"-5", c"10"][..],
                "wm: invalid option -- '5'\n",
            ),
        ] {
            let (status, fake) = run(arguments, windows());
            assert_eq!(
                (status, fake.errors().as_str()),
                (2, error),
                "{arguments:?}"
            );
            assert!(fake.managed.is_empty());
        }
        for arguments in [
            &[c"focus", c"129", c"130"][..],
            &[c"size", c"129", c"5"][..],
        ] {
            let (status, fake) = run(arguments, windows());
            assert_eq!(status, 2, "{arguments:?}");
            assert!(fake.errors().starts_with("wm: usage: wm list"));
        }
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
    #[test]
    fn a_window_hanging_off_the_left_and_one_not_responding_are_listed_as_they_show() {
        let mut fake = windows();
        fake.windows[1].state = NOT_RESPONDING_STATE;
        fake.windows[1].rect = [(-542i32) as u32, 100, 658, 330];
        fake.windows[1].title = b"Editor".to_vec();
        let (status, fake) = run(&[c"list"], fake);
        assert_eq!(status, 0);
        assert!(
            fake.output()
                .ends_with("129 normal -542 100 658 330 67 Editor (not responding)\n"),
            "{}",
            fake.output()
        );
        let mut fake = windows();
        fake.windows[1].state = NOT_RESPONDING_STATE | 2;
        let (status, fake) = run(&[c"list", c"--json"], fake);
        assert_eq!(status, 0);
        assert!(fake.output().contains(
            "\"state\":\"maximised\",\"x\":0,\"y\":28,\"width\":1280,\"height\":772,\
             \"client_pid\":67,\"responding\":false,\"title\":\"first\""
        ));
    }

    #[test]
    fn a_window_that_does_not_respond_is_refused_as_busy_with_its_own_code() {
        let mut output = Output::new(windows(), b"wm");
        output.use_json();
        let status = refused(&mut output, b"129", NOT_RESPONDING_ERRNO);
        assert_eq!(status, BUSY_STATUS);
        let _ = output.finish(status);
        assert!(
            output
                .system
                .errors()
                .contains("\"code\":\"not_responding\""),
            "{}",
            output.system.errors()
        );
    }
}

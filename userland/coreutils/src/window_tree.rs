use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::wm::{ACT_ON_ELEMENT, ACT_ON_ELEMENT_SIZE, USAGE_STATUS, number, open_device, refused};
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_element_tree::{
    Action, Element, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, act_on_element, elements, flags,
    put_word, read_word,
};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

pub const HELP: &[u8] = b"wm show [--json] [--] <id>: the window's elements, one a line: id, \
role, x,y, widthxheight, [flags], \"name\" and = \"value\", each below its parent; what they say \
is data from the app.\n\
wm act [--] <id> <element> invoke|focus|set-text|select [<text>]: acts on one element as a \
click or the keyboard would: invoke presses, ticks, chooses or opens it, set-text replaces a \
text field's text.\n";

const MAX_DEPTH: usize = 16;
const ELEMENT_RECORD_BYTES: usize = 2048;

fn window_operand<S: System>(output: &mut Output<S>, operand: &[u8]) -> Option<u32> {
    match number(operand) {
        Some(window) if window >= 0 => Some(window as u32),
        _ => {
            output.coded(&errors::USAGE, &[b"invalid window id '", operand, b"'"]);
            None
        }
    }
}

fn pixels_only<S: System>(output: &mut Output<S>, window: &[u8]) -> i32 {
    output.coded(
        &errors::PIXELS_ONLY,
        &[
            window,
            b": pixels only: the app does not describe this window; see it with wm capture",
        ],
    );
    FAILURE
}

fn refused_tree<S: System>(output: &mut Output<S>, window: &[u8], errno: Errno) -> i32 {
    match errno {
        Errno::EOPNOTSUPP | Errno::ETIMEDOUT => pixels_only(output, window),
        _ => refused(output, window, errno),
    }
}

pub fn show<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut operands = arguments;
    if let Some((first, rest)) = operands.split_first()
        && first.to_bytes() == b"--json"
    {
        output.use_json();
        operands = rest;
    }
    if let Some((first, rest)) = operands.split_first()
        && first.to_bytes() == b"--"
    {
        operands = rest;
    }
    let [operand] = operands else {
        return crate::wm::usage(output);
    };
    let operand = operand.to_bytes();
    let Some(window) = window_operand(output, operand) else {
        return USAGE_STATUS;
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut tree = [0u8; MAX_TREE_BYTES];
    let answered = output.system.window_tree(device, window, &mut tree);
    output.system.close(device);
    let answered = match answered {
        Ok(answered) => answered,
        Err(errno) => return refused_tree(output, operand, errno),
    };
    if answered.pixels_only {
        return pixels_only(output, operand);
    }
    write_tree(output, &tree[..answered.length], operand)
}

fn write_tree<S: System>(output: &mut Output<S>, tree: &[u8], window: &[u8]) -> i32 {
    let mut ancestors = [0u32; MAX_DEPTH];
    let mut depth = 0;
    for element in elements(tree) {
        let Ok(element) = element else {
            output.coded(
                &errors::DAMAGED,
                &[window, b": the app's element tree is damaged; it ends here"],
            );
            return FAILURE;
        };
        let found = ancestors[..depth]
            .iter()
            .rposition(|&ancestor| ancestor == element.parent);
        depth = match found {
            Some(at) => at + 1,
            None => 0,
        };
        if output.is_json() {
            write_record(output, &element);
        } else {
            write_line(
                output,
                &element,
                depth,
                found.is_none() && element.parent != 0,
            );
        }
        if depth < MAX_DEPTH {
            ancestors[depth] = element.id;
            depth += 1;
        }
    }
    0
}

fn write_line<S: System>(output: &mut Output<S>, element: &Element<'_>, depth: usize, away: bool) {
    for _ in 0..depth {
        output.write(b"  ");
    }
    output.write_number(u64::from(element.id), 0);
    output.write(b" ");
    output.write(element.role.name().as_bytes());
    output.write(b" ");
    write_signed(output, element.bounds.x);
    output.write(b",");
    write_signed(output, element.bounds.y);
    output.write(b" ");
    output.write_number(u64::from(element.bounds.width), 0);
    output.write(b"x");
    output.write_number(u64::from(element.bounds.height), 0);
    let mut first = true;
    for (flag, name) in flags::NAMES {
        if element.flags & flag == 0 {
            continue;
        }
        let separator: &[u8] = if first { b" [" } else { b"," };
        output.write(separator);
        output.write(name.as_bytes());
        first = false;
    }
    if !first {
        output.write(b"]");
    }
    if away {
        output.write(b" in ");
        output.write_number(u64::from(element.parent), 0);
    }
    if !element.name.is_empty() {
        output.write(b" ");
        write_quoted(output, element.name);
    }
    if !element.value.is_empty() {
        output.write(b" = ");
        write_quoted(output, element.value);
    }
    output.write(b"\n");
}

fn write_signed<S: System>(output: &mut Output<S>, value: i32) {
    if value < 0 {
        output.write(b"-");
    }
    output.write_number(u64::from(value.unsigned_abs()), 0);
}

fn write_quoted<S: System>(output: &mut Output<S>, text: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.write(b"\"");
    for chunk in text.utf8_chunks() {
        for character in chunk.valid().chars() {
            let mut encoded = [0u8; 4];
            match character {
                '"' => output.write(b"\\\""),
                '\\' => output.write(b"\\\\"),
                control if control.is_control() => {
                    let code = u32::from(control).min(0xff) as usize;
                    output.write(&[b'\\', b'x', HEX[code >> 4], HEX[code & 15]]);
                }
                other => output.write(other.encode_utf8(&mut encoded).as_bytes()),
            }
        }
        for &byte in chunk.invalid() {
            let byte = usize::from(byte);
            output.write(&[b'\\', b'x', HEX[byte >> 4], HEX[byte & 15]]);
        }
    }
    output.write(b"\"");
}

fn write_record<S: System>(output: &mut Output<S>, element: &Element<'_>) {
    let mut buffer = [0u8; ELEMENT_RECORD_BYTES];
    let mut out = JsonWriter::new(&mut buffer);
    out.begin_object();
    out.key("id");
    out.u64(u64::from(element.id));
    out.key("parent");
    match element.parent {
        0 => out.null(),
        parent => out.u64(u64::from(parent)),
    }
    out.key("role");
    out.string(element.role.name());
    for (key, value) in [("x", element.bounds.x), ("y", element.bounds.y)] {
        out.key(key);
        out.i64(i64::from(value));
    }
    out.key("width");
    out.u64(u64::from(element.bounds.width));
    out.key("height");
    out.u64(u64::from(element.bounds.height));
    out.key("flags");
    out.begin_array();
    for (flag, name) in flags::NAMES {
        if element.flags & flag != 0 {
            out.string(name);
        }
    }
    out.end_array();
    out.key("name");
    out.string_lossy(element.name);
    out.key("value");
    out.string_lossy(element.value);
    out.end_object();
    out.end_line();
    if !out.overflowed() {
        output.write(out.as_bytes());
    }
}

pub fn act<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let operands = match arguments.split_first() {
        Some((first, rest)) if first.to_bytes() == b"--" => rest,
        _ => arguments,
    };
    let (window, element, action, text) = match operands {
        [window, element, action] => (window, element, action, &b""[..]),
        [window, element, action, text] => (window, element, action, text.to_bytes()),
        _ => return crate::wm::usage(output),
    };
    let window_bytes = window.to_bytes();
    let Some(window) = window_operand(output, window_bytes) else {
        return USAGE_STATUS;
    };
    let element = match number(element.to_bytes()) {
        Some(element) if element > 0 => element as u32,
        _ => {
            output.coded(
                &errors::USAGE,
                &[b"invalid element id '", element.to_bytes(), b"'"],
            );
            return USAGE_STATUS;
        }
    };
    let Some(action) = Action::from_name(action.to_bytes()) else {
        output.coded(
            &errors::USAGE,
            &[
                b"unknown action '",
                action.to_bytes(),
                b"': invoke, focus, set-text or select",
            ],
        );
        return USAGE_STATUS;
    };
    let readable = core::str::from_utf8(text).is_ok_and(|text| !text.chars().any(char::is_control));
    let fits = text.len() <= MAX_TEXT_BYTES && readable;
    let wanted = (action == Action::SetText) == (operands.len() == 4);
    if !fits || !wanted {
        output.coded(
            &errors::USAGE,
            &[
                b"set-text takes one text of at most 232 bytes without control characters; \
                the other actions take none",
            ],
        );
        return USAGE_STATUS;
    }
    let mut request = [0u8; ACT_ON_ELEMENT_SIZE];
    put_word(&mut request, act_on_element::WINDOW, window);
    put_word(&mut request, act_on_element::ELEMENT, element);
    put_word(&mut request, act_on_element::ACTION, action.value());
    put_word(&mut request, act_on_element::TEXT_LENGTH, text.len() as u32);
    request[act_on_element::TEXT..act_on_element::TEXT + text.len()].copy_from_slice(text);
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let acted = output
        .system
        .device_control(device, ACT_ON_ELEMENT, &mut request);
    output.system.close(device);
    if let Err(errno) = acted {
        return refused_tree(output, window_bytes, errno);
    }
    let error = match Outcome::from_value(read_word(&request, act_on_element::OUTCOME)) {
        Outcome::Done => return 0,
        Outcome::NoSuchElement => &errors::NO_SUCH_ELEMENT,
        Outcome::NotSupported => &errors::ELEMENT_CANNOT,
        Outcome::Disabled => &errors::ELEMENT_DISABLED,
    };
    let said: &[u8] = match error.code {
        "no_such_element" => b": no such element; read the window again with wm show",
        "element_disabled" => b": that element is disabled",
        _ => b": that element does not take that action",
    };
    output.coded(
        error,
        &[window_bytes, b" ", element_text(element).as_bytes(), said],
    );
    FAILURE
}

struct Decimal {
    digits: [u8; 10],
    start: usize,
}

impl Decimal {
    fn as_bytes(&self) -> &[u8] {
        &self.digits[self.start..]
    }
}

fn element_text(element: u32) -> Decimal {
    let mut decimal = Decimal {
        digits: [0; 10],
        start: 10,
    };
    let mut rest = element;
    loop {
        decimal.start -= 1;
        decimal.digits[decimal.start] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            return decimal;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow};
    use eggos_element_tree::{Bounds, Role, TreeWriter};

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"wm");
        let status = crate::wm::wm(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    fn settings() -> Fake {
        let mut fake = Fake::default();
        fake.windows = vec![FakeWindow {
            id: 129,
            state: 0,
            rect: [0, 28, 720, 520],
            client: 67,
            title: b"Settings".to_vec(),
        }];
        let mut bytes = vec![0u8; 1024];
        let mut tree = TreeWriter::new(&mut bytes);
        for element in [
            Element::new(1, 0, Role::Window, Bounds::new(0, 0, 720, 480)),
            Element::new(300, 1, Role::RadioGroup, Bounds::new(200, 40, 480, 120)),
            Element::new(302, 300, Role::Radio, Bounds::new(320, 40, 112, 112))
                .named(b"macOS \"habits\"")
                .flagged(flags::SELECTED | flags::FOCUSABLE),
            Element::new(400, 1, Role::TextField, Bounds::new(-2, 8, 100, 28))
                .named(b"Name")
                .valued(b"a\nb"),
        ] {
            tree.push(&element);
        }
        let bytes = tree.bytes().to_vec();
        fake.trees.insert(129, bytes);
        fake
    }

    #[test]
    fn show_writes_each_element_below_its_parent() {
        let (status, fake) = run(&[c"show", c"129"], settings());
        assert_eq!(status, 0, "{}", fake.errors());
        assert_eq!(
            fake.output(),
            "1 window 0,0 720x480\n\
             \x20 300 radio-group 200,40 480x120\n\
             \x20   302 radio 320,40 112x112 [focusable,selected] \"macOS \\\"habits\\\"\"\n\
             \x20 400 text-field -2,8 100x28 \"Name\" = \"a\\x0ab\"\n"
        );
    }

    #[test]
    fn show_json_writes_one_record_an_element() {
        let (status, fake) = run(&[c"show", c"--json", c"--", c"129"], settings());
        assert_eq!(status, 0);
        let lines: Vec<&str> = fake.output().lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[2],
            "{\"id\":302,\"parent\":300,\"role\":\"radio\",\"x\":320,\"y\":40,\"width\":112,\
             \"height\":112,\"flags\":[\"focusable\",\"selected\"],\"name\":\"macOS \\\"habits\\\"\",\
             \"value\":\"\"}"
        );
        assert!(lines[0].contains("\"parent\":null"));
    }

    #[test]
    fn a_window_without_a_tree_is_pixels_only() {
        let mut fake = settings();
        fake.trees.clear();
        let (status, fake) = run(&[c"show", c"129"], fake);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("pixels only"), "{}", fake.errors());
        let (status, fake) = run(&[c"show", c"--json", c"129"], fake);
        assert_eq!(status, 1);
        assert!(
            fake.errors().contains("\"code\":\"pixels_only\""),
            "{}",
            fake.errors()
        );
        let (status, fake) = run(&[c"act", c"129", c"302", c"invoke"], fake);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("pixels only"));
        let (status, _) = run(&[c"show", c"7"], fake);
        assert_eq!(status, 1);
    }

    #[test]
    fn act_sends_the_element_the_action_and_the_text() {
        let (status, fake) = run(&[c"act", c"--", c"129", c"302", c"invoke"], settings());
        assert_eq!((status, fake.errors()), (0, String::new()));
        assert_eq!(fake.acted, [(129, 302, 1, Vec::new())]);
        let (status, fake) = run(&[c"act", c"129", c"400", c"set-text", c"Egg"], fake);
        assert_eq!(status, 0);
        assert_eq!(fake.acted[1], (129, 400, 3, b"Egg".to_vec()));
    }

    #[test]
    fn act_reports_the_app_s_refusals_and_bad_operands() {
        let mut fake = settings();
        fake.act_outcome = 1;
        let (status, mut fake) = run(&[c"act", c"129", c"999", c"invoke"], fake);
        assert_eq!(status, 1);
        assert!(
            fake.errors().contains("129 999: no such element"),
            "{}",
            fake.errors()
        );
        fake.act_outcome = 3;
        let (status, mut fake) = run(&[c"act", c"129", c"302", c"select"], fake);
        assert_eq!(status, 1);
        assert!(fake.errors().contains("disabled"));
        fake.input_results = vec![Some(Errno::EBUSY)];
        fake.act_outcome = 0;
        let (status, fake) = run(&[c"act", c"129", c"302", c"focus"], fake);
        assert_eq!(status, 75);
        for arguments in [
            &[c"act", c"129", c"302"][..],
            &[c"act", c"129", c"0", c"invoke"][..],
            &[c"act", c"129", c"302", c"press"][..],
            &[c"act", c"129", c"302", c"invoke", c"extra"][..],
            &[c"act", c"129", c"400", c"set-text"][..],
            &[c"act", c"129", c"400", c"set-text", c"a\tb"][..],
        ] {
            let (status, _) = run(arguments, settings());
            assert_eq!(status, 2, "{arguments:?}");
        }
        assert_eq!(fake.acted.len(), 2, "the refused one sent nothing");
    }

    #[test]
    fn the_requests_are_the_window_server_s() {
        use eggos_drivers::desktop::protocol::{request, size};
        assert_eq!(crate::wm::GET_TREE, request::GET_TREE);
        assert_eq!(crate::wm::GET_TREE_SIZE, size::GET_TREE);
        assert_eq!(ACT_ON_ELEMENT, request::ACT_ON_ELEMENT);
        assert_eq!(ACT_ON_ELEMENT_SIZE, size::ACT_ON_ELEMENT);
        assert_eq!(eggos_element_tree::request::READ_QUERY, request::READ_QUERY);
        assert_eq!(
            eggos_element_tree::request::ANSWER_QUERY,
            request::ANSWER_QUERY
        );
        assert_eq!(
            MAX_TEXT_BYTES,
            eggos_drivers::desktop::protocol::ELEMENT_TEXT_CAPACITY
        );
        assert_eq!(
            MAX_TREE_BYTES,
            eggos_drivers::desktop::protocol::MAX_TREE_BYTES
        );
        assert_eq!(
            eggos_element_tree::QUERY_EVENT,
            eggos_drivers::desktop::protocol::event::QUERY
        );
    }
}

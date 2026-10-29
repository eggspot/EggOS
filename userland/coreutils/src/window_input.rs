use crate::output::{FAILURE, Output};
use crate::system::{Duration, System};
use crate::wm::{SEND_INPUT, SEND_INPUT_SIZE, USAGE_STATUS, number, open_device, refused, usage};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

pub const HELP: &[u8] = b"Window ids are the ones `wm list` shows; coordinates are pixels of the \
window's content, from its top-left corner, as `wm capture` shows them at scale 1.\n\
wm click: --button left (the default), right or middle.\n\
wm type: printable ASCII only, typed with the US keyboard; Enter and other keys are `wm press`.\n\
wm press: one key, after any of ctrl+, shift+, alt+ and super+ (as in ctrl+s or ctrl+shift+t). \
Keys: a to z, 0 to 9, F1 to F12, Enter, Tab, Escape (Esc), Backspace, Delete, Insert, Home, \
End, PageUp, PageDown, Up, Down, Left, Right, Space, and Shift, Ctrl, Alt or Super alone. \
Names are not case-sensitive.\n\
wm scroll: dy notches of the wheel, up when positive, at --at x y or the content's centre; \
dx must be 0 (sideways scrolling is not supported yet).\n\
A person always wins: while someone holds a key or a button, or for half a second after \
they last used the keyboard or the pointer, input is refused and wm exits with 75.\n";

const MAX_POSITIONALS: usize = 3;
pub const RETRY_PAUSE: Duration = Duration {
    seconds: 0,
    nanoseconds: 20_000_000,
};
pub const MAX_RETRIES: u32 = 150;

const KIND_CLICK: u32 = 1;
const KIND_WHEEL: u32 = 2;
const KIND_KEY: u32 = 3;
const KIND_CHARACTER: u32 = 4;

pub const BUTTONS: [(&[u8], u32); 3] = [(b"left", 0x110), (b"right", 0x111), (b"middle", 0x112)];

pub const MODIFIERS: [(&[u8], u32); 4] = [(b"shift", 1), (b"ctrl", 2), (b"alt", 4), (b"super", 8)];

pub const KEYS: [(&[u8], u16); 70] = [
    (b"a", 30),
    (b"b", 48),
    (b"c", 46),
    (b"d", 32),
    (b"e", 18),
    (b"f", 33),
    (b"g", 34),
    (b"h", 35),
    (b"i", 23),
    (b"j", 36),
    (b"k", 37),
    (b"l", 38),
    (b"m", 50),
    (b"n", 49),
    (b"o", 24),
    (b"p", 25),
    (b"q", 16),
    (b"r", 19),
    (b"s", 31),
    (b"t", 20),
    (b"u", 22),
    (b"v", 47),
    (b"w", 17),
    (b"x", 45),
    (b"y", 21),
    (b"z", 44),
    (b"1", 2),
    (b"2", 3),
    (b"3", 4),
    (b"4", 5),
    (b"5", 6),
    (b"6", 7),
    (b"7", 8),
    (b"8", 9),
    (b"9", 10),
    (b"0", 11),
    (b"f1", 59),
    (b"f2", 60),
    (b"f3", 61),
    (b"f4", 62),
    (b"f5", 63),
    (b"f6", 64),
    (b"f7", 65),
    (b"f8", 66),
    (b"f9", 67),
    (b"f10", 68),
    (b"f11", 87),
    (b"f12", 88),
    (b"enter", 28),
    (b"tab", 15),
    (b"escape", 1),
    (b"esc", 1),
    (b"backspace", 14),
    (b"delete", 111),
    (b"insert", 110),
    (b"home", 102),
    (b"end", 107),
    (b"pageup", 104),
    (b"pagedown", 109),
    (b"up", 103),
    (b"down", 108),
    (b"left", 105),
    (b"right", 106),
    (b"space", 57),
    (b"shift", 42),
    (b"ctrl", 29),
    (b"alt", 56),
    (b"super", 125),
    (b"cmd", 125),
    (b"win", 125),
];

#[derive(Debug, Default)]
pub struct Arguments<'a> {
    pub positionals: [&'a [u8]; MAX_POSITIONALS],
    pub count: usize,
    pub output_file: Option<&'a CStr>,
    pub button: Option<&'a [u8]>,
    pub at: Option<(&'a [u8], &'a [u8])>,
    pub max_width: Option<&'a [u8]>,
    pub max_bytes: Option<&'a [u8]>,
    pub choose: bool,
    pub label: Option<&'a [u8]>,
}

fn is_option(argument: &[u8]) -> bool {
    argument.len() > 1 && argument[0] == b'-' && !argument[1].is_ascii_digit()
}

pub fn parse<'a>(arguments: &[&'a CStr], options: &[&[u8]]) -> Option<Arguments<'a>> {
    let mut parsed = Arguments::default();
    let mut rest = arguments.iter();
    let mut options_end = false;
    while let Some(&argument) = rest.next() {
        let bytes = argument.to_bytes();
        if !options_end && bytes == b"--" {
            options_end = true;
            continue;
        }
        if !options_end && is_option(bytes) {
            if !options.contains(&bytes) {
                return None;
            }
            let mut value = || rest.next().map(|value| value.to_bytes());
            match bytes {
                b"-o" => parsed.output_file = Some(*rest.next()?),
                b"--button" => parsed.button = Some(value()?),
                b"--max-width" => parsed.max_width = Some(value()?),
                b"--max-bytes" => parsed.max_bytes = Some(value()?),
                b"--choose" => parsed.choose = true,
                b"--label" => parsed.label = Some(value()?),
                b"--at" => {
                    let x = value()?;
                    parsed.at = Some((x, value()?));
                }
                _ => return None,
            }
            continue;
        }
        *parsed.positionals.get_mut(parsed.count)? = bytes;
        parsed.count += 1;
    }
    Some(parsed)
}

fn lowercase_equals(text: &[u8], name: &[u8]) -> bool {
    text.len() == name.len()
        && text
            .iter()
            .zip(name)
            .all(|(byte, name)| byte.to_ascii_lowercase() == *name)
}

pub fn key_named(name: &[u8]) -> Option<u16> {
    KEYS.iter()
        .find(|(known, _)| lowercase_equals(name, known))
        .map(|&(_, code)| code)
}

pub fn combination(keys: &[u8]) -> Option<(u16, u32)> {
    let mut parts = keys.split(|&byte| byte == b'+');
    let key = parts.next_back()?;
    let mut modifiers = 0;
    for part in parts {
        let (_, bit) = MODIFIERS
            .iter()
            .find(|(name, _)| lowercase_equals(part, name))?;
        if modifiers & bit != 0 {
            return None;
        }
        modifiers |= bit;
    }
    Some((key_named(key)?, modifiers))
}

fn coordinate(text: &[u8]) -> Option<i32> {
    number(text).filter(|&value| value >= 0)
}

#[derive(Debug)]
pub struct Request {
    pub window: u32,
    pub kind: u32,
    pub x: i32,
    pub y: i32,
    pub code: u32,
    pub value: u32,
}

impl Request {
    fn bytes(&self) -> [u8; SEND_INPUT_SIZE] {
        let mut bytes = [0u8; SEND_INPUT_SIZE];
        for (index, word) in [
            self.window,
            self.kind,
            self.x as u32,
            self.y as u32,
            self.code,
            self.value,
        ]
        .into_iter()
        .enumerate()
        {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
        }
        bytes
    }
}

#[derive(Debug)]
struct Sender {
    device: usize,
    retries: u32,
}

impl Sender {
    fn send<S: System>(&mut self, system: &mut S, request: &Request) -> Result<(), Errno> {
        loop {
            let mut bytes = request.bytes();
            match system.device_control(self.device, SEND_INPUT, &mut bytes) {
                Err(Errno::EAGAIN) if self.retries < MAX_RETRIES => {
                    self.retries += 1;
                    system.sleep(RETRY_PAUSE)?;
                }
                result => return result,
            }
        }
    }
}

pub fn send<S: System>(output: &mut Output<S>, verb: &[u8], arguments: &[&CStr]) -> i32 {
    let options: &[&[u8]] = match verb {
        b"click" => &[b"--button"],
        b"scroll" => &[b"--at"],
        _ => &[],
    };
    let Some(parsed) = parse(arguments, options) else {
        return usage(output);
    };
    let wanted = match verb {
        b"click" | b"scroll" => 3,
        _ => 2,
    };
    if parsed.count != wanted {
        return usage(output);
    }
    let window_text = parsed.positionals[0];
    let Some(window) = number(window_text).and_then(|id| u32::try_from(id).ok()) else {
        output.error(&[b"invalid window id '", window_text, b"'"]);
        return USAGE_STATUS;
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut sender = Sender { device, retries: 0 };
    let status = match verb {
        b"click" => click(output, &mut sender, window, &parsed),
        b"type" => type_text(output, &mut sender, window, parsed.positionals[1]),
        b"press" => press(output, &mut sender, window, parsed.positionals[1]),
        _ => scroll(output, &mut sender, window, &parsed),
    };
    output.system.close(device);
    status
}

fn click<S: System>(
    output: &mut Output<S>,
    sender: &mut Sender,
    window: u32,
    parsed: &Arguments<'_>,
) -> i32 {
    let name = parsed.button.unwrap_or(b"left");
    let Some(&(_, code)) = BUTTONS
        .iter()
        .find(|(known, _)| lowercase_equals(name, known))
    else {
        output.error(&[b"unknown button '", name, b"' (left, right or middle)"]);
        return USAGE_STATUS;
    };
    let (Some(x), Some(y)) = (
        coordinate(parsed.positionals[1]),
        coordinate(parsed.positionals[2]),
    ) else {
        output.error(&[
            b"invalid point ",
            parsed.positionals[1],
            b" ",
            parsed.positionals[2],
        ]);
        return USAGE_STATUS;
    };
    let request = Request {
        window,
        kind: KIND_CLICK,
        x,
        y,
        code,
        value: 0,
    };
    match sender.send(&mut output.system, &request) {
        Ok(()) => {
            output.write(b"clicked ");
            output.write(name);
            output.write(b" at ");
            output.write_number(x as u64, 0);
            output.write(b",");
            output.write_number(y as u64, 0);
            output.write(b" in window ");
            output.write(parsed.positionals[0]);
            output.write(b"\n");
            0
        }
        Err(Errno::EOPNOTSUPP) => {
            output.error(&[
                parsed.positionals[0],
                b": the terminal's window takes keys, characters and the wheel only",
            ]);
            FAILURE
        }
        Err(errno) => refused(output, parsed.positionals[0], errno),
    }
}

fn type_text<S: System>(
    output: &mut Output<S>,
    sender: &mut Sender,
    window: u32,
    text: &[u8],
) -> i32 {
    if text.is_empty() || !text.iter().all(|byte| (b' '..=b'~').contains(byte)) {
        output.error(&[
            b"wm type takes printable ASCII text only; press Enter and other keys with wm press",
        ]);
        return USAGE_STATUS;
    }
    let mut window_text = [0u8; 12];
    let window_text = decimal(window, &mut window_text);
    for (typed, &byte) in text.iter().enumerate() {
        let request = Request {
            window,
            kind: KIND_CHARACTER,
            x: 0,
            y: 0,
            code: u32::from(byte),
            value: 0,
        };
        if let Err(errno) = sender.send(&mut output.system, &request) {
            output.write(b"typed ");
            output.write_number(typed as u64, 0);
            output.write(b" of ");
            output.write_number(text.len() as u64, 0);
            output.write(b" characters\n");
            return refused(output, window_text, errno);
        }
    }
    output.write(b"typed ");
    output.write_number(text.len() as u64, 0);
    output.write(b" of ");
    output.write_number(text.len() as u64, 0);
    output.write(b" characters into window ");
    output.write(window_text);
    output.write(b"\n");
    0
}

fn decimal(value: u32, out: &mut [u8; 12]) -> &[u8] {
    let mut digits = [0u8; 20];
    let text = crate::output::decimal(u64::from(value), &mut digits);
    out[..text.len()].copy_from_slice(text);
    &out[..text.len()]
}

fn press<S: System>(output: &mut Output<S>, sender: &mut Sender, window: u32, keys: &[u8]) -> i32 {
    let Some((code, modifiers)) = combination(keys) else {
        output.error(&[b"unknown keys '", keys, b"' (see wm help)"]);
        return USAGE_STATUS;
    };
    let request = Request {
        window,
        kind: KIND_KEY,
        x: 0,
        y: 0,
        code: u32::from(code),
        value: modifiers,
    };
    let mut window_text = [0u8; 12];
    let window_text = decimal(window, &mut window_text);
    match sender.send(&mut output.system, &request) {
        Ok(()) => {
            output.write(b"pressed ");
            output.write(keys);
            output.write(b" in window ");
            output.write(window_text);
            output.write(b"\n");
            0
        }
        Err(errno) => refused(output, window_text, errno),
    }
}

fn scroll<S: System>(
    output: &mut Output<S>,
    sender: &mut Sender,
    window: u32,
    parsed: &Arguments<'_>,
) -> i32 {
    let (Some(dx), Some(dy)) = (number(parsed.positionals[1]), number(parsed.positionals[2]))
    else {
        output.error(&[
            b"invalid scroll ",
            parsed.positionals[1],
            b" ",
            parsed.positionals[2],
        ]);
        return USAGE_STATUS;
    };
    let window_text = parsed.positionals[0];
    if dx != 0 {
        output.error(&[b"sideways scrolling is not supported yet: dx must be 0"]);
        return FAILURE;
    }
    if dy == 0 {
        output.error(&[b"nothing to scroll: dy is 0"]);
        return USAGE_STATUS;
    }
    let point = match parsed.at {
        Some((x, y)) => match (coordinate(x), coordinate(y)) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                output.error(&[b"invalid point ", x, b" ", y]);
                return USAGE_STATUS;
            }
        },
        None => match output
            .system
            .capture_window_rows(sender.device, window, 0, 0, &mut [])
        {
            Ok(size) => ((size.width / 2) as i32, (size.height / 2) as i32),
            Err(Errno::EOPNOTSUPP) => (0, 0),
            Err(errno) => return refused(output, window_text, errno),
        },
    };
    let request = Request {
        window,
        kind: KIND_WHEEL,
        x: point.0,
        y: point.1,
        code: 0,
        value: dy as u32,
    };
    match sender.send(&mut output.system, &request) {
        Ok(()) => {
            output.write(b"scrolled ");
            output.write(parsed.positionals[2]);
            output.write(b" notches in window ");
            output.write(window_text);
            output.write(b"\n");
            0
        }
        Err(errno) => refused(output, window_text, errno),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow};
    use crate::wm::{BUSY_STATUS, KERNEL_CLIENT, wm};
    use eggos_drivers::desktop::protocol::{input_kind, modifier, request, size};
    use eggos_drivers::input::{button, key, keymap};

    fn machine() -> Fake {
        let mut fake = Fake::default();
        for (id, client) in [(64, KERNEL_CLIENT), (129, 67)] {
            fake.windows.push(FakeWindow {
                id,
                state: 0,
                rect: [0, 0, 100, 50],
                client,
                title: b"window".to_vec(),
            });
        }
        fake.window_pixels.insert(129, (100, 50, vec![0; 5000]));
        fake
    }

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"wm");
        let status = wm(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    #[test]
    fn the_requests_match_the_window_server_s() {
        assert_eq!(crate::wm::SEND_INPUT, request::SEND_INPUT);
        assert_eq!(crate::wm::CAPTURE_WINDOW, request::CAPTURE_WINDOW);
        assert_eq!(SEND_INPUT_SIZE, size::SEND_INPUT);
        assert_eq!(crate::wm::CAPTURE_SIZE, size::CAPTURE_WINDOW);
        assert_eq!(
            [KIND_CLICK, KIND_WHEEL, KIND_KEY, KIND_CHARACTER],
            [
                input_kind::CLICK,
                input_kind::WHEEL,
                input_kind::KEY,
                input_kind::CHARACTER
            ]
        );
        assert_eq!(
            BUTTONS.map(|(_, code)| code),
            [button::LEFT, button::RIGHT, button::MIDDLE].map(u32::from)
        );
        assert_eq!(
            MODIFIERS.map(|(_, bit)| bit),
            [
                modifier::SHIFT,
                modifier::CTRL,
                modifier::ALT,
                modifier::SUPER
            ]
        );
    }

    #[test]
    fn key_names_are_the_keymap_s_codes() {
        for (name, code) in KEYS {
            if name.len() == 1 {
                let [plain, _] = keymap::printable(code).unwrap();
                assert_eq!(&[plain][..], name, "{code}");
            }
        }
        for (name, code) in [
            ("enter", key::ENTER),
            ("tab", key::TAB),
            ("escape", key::ESC),
            ("backspace", key::BACKSPACE),
            ("delete", key::DELETE),
            ("insert", key::INSERT),
            ("home", key::HOME),
            ("end", key::END),
            ("pageup", key::PAGE_UP),
            ("pagedown", key::PAGE_DOWN),
            ("up", key::UP),
            ("down", key::DOWN),
            ("left", key::LEFT),
            ("right", key::RIGHT),
            ("space", key::SPACE),
            ("f1", key::F1),
            ("f10", key::F10),
            ("f11", key::F11),
            ("f12", key::F12),
            ("shift", key::LEFT_SHIFT),
            ("ctrl", key::LEFT_CTRL),
            ("alt", key::LEFT_ALT),
            ("super", key::LEFT_SUPER),
        ] {
            assert_eq!(key_named(name.as_bytes()), Some(code), "{name}");
        }
        assert_eq!(combination(b"Ctrl+S"), Some((key::A + 1, modifier::CTRL)));
        assert_eq!(
            combination(b"ctrl+shift+t"),
            Some((20, modifier::CTRL | modifier::SHIFT))
        );
        assert_eq!(combination(b"Shift"), Some((key::LEFT_SHIFT, 0)));
        for wrong in [&b"ctrl+ctrl+s"[..], b"hyper+s", b"ctrl+", b"", b"f13", b"+"] {
            assert_eq!(combination(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn a_click_sends_the_button_and_point() {
        let (status, fake) = run(&[c"click", c"129", c"10", c"20"], machine());
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(fake.sent_input, [[129, KIND_CLICK, 10, 20, 0x110, 0]]);
        assert_eq!(fake.output(), "clicked left at 10,20 in window 129\n");
        let (status, fake) = run(
            &[c"click", c"--button", c"right", c"--", c"129", c"0", c"49"],
            machine(),
        );
        assert_eq!(status, 0);
        assert_eq!(fake.sent_input, [[129, KIND_CLICK, 0, 49, 0x111, 0]]);
    }

    #[test]
    fn typing_sends_each_character_and_says_how_many_went() {
        let (status, fake) = run(&[c"type", c"--", c"129", c"a B"], machine());
        assert_eq!(status, 0);
        assert_eq!(
            fake.sent_input,
            b"a B".map(|byte| [129, KIND_CHARACTER, 0, 0, u32::from(byte), 0])
        );
        assert_eq!(fake.output(), "typed 3 of 3 characters into window 129\n");
        let (status, _) = run(&[c"type", c"129", c"-x"], machine());
        assert_eq!(status, 2, "an option-like text needs --");
        let (status, fake) = run(&[c"type", c"--", c"129", c"-x"], machine());
        assert_eq!((status, fake.sent_input.len()), (0, 2));
    }

    #[test]
    fn a_person_stops_typing_partway_and_says_how_far_it_got() {
        let mut busy = machine();
        busy.input_results = vec![None, Some(Errno::EBUSY)];
        let (status, fake) = run(&[c"type", c"129", c"abc"], busy);
        assert_eq!(status, BUSY_STATUS);
        assert_eq!(fake.output(), "typed 1 of 3 characters\n");
        assert_eq!(
            fake.errors(),
            "wm: 129: a person is using the machine; try again in a moment\n"
        );
    }

    #[test]
    fn a_full_queue_is_retried_within_the_budget() {
        let mut waiting = machine();
        waiting.input_results = vec![Some(Errno::EAGAIN), Some(Errno::EAGAIN)];
        let (status, fake) = run(&[c"type", c"129", c"a"], waiting);
        assert_eq!((status, fake.slept.len(), fake.sent_input.len()), (0, 2, 1));
        let mut stuck = machine();
        stuck.input_results = vec![Some(Errno::EAGAIN); MAX_RETRIES as usize + 1];
        let (status, fake) = run(&[c"type", c"129", c"ab"], stuck);
        assert_eq!(status, FAILURE);
        assert_eq!(fake.slept.len(), MAX_RETRIES as usize);
        assert_eq!(fake.output(), "typed 0 of 2 characters\n");
    }

    #[test]
    fn typing_refuses_control_characters_and_other_text() {
        for text in [c"", c"line\r", c"tab\there", c"caf\xc3\xa9"] {
            let (status, fake) = run(&[c"type", c"129", text], machine());
            assert_eq!(status, 2, "{text:?}");
            assert!(fake.sent_input.is_empty());
        }
    }

    #[test]
    fn a_key_press_sends_the_key_and_its_modifiers() {
        for (keys, code, modifiers) in [
            (c"ctrl+s", 31, modifier::CTRL),
            (c"Enter", 28, 0),
            (c"shift", 42, 0),
            (c"alt+F4", 62, modifier::ALT),
        ] {
            let (status, fake) = run(&[c"press", c"129", keys], machine());
            assert_eq!(status, 0, "{keys:?}");
            assert_eq!(fake.sent_input, [[129, KIND_KEY, 0, 0, code, modifiers]]);
        }
        let (status, fake) = run(&[c"press", c"129", c"ctrl+nothing"], machine());
        assert_eq!(status, 2);
        assert!(fake.errors().contains("unknown keys"));
    }

    #[test]
    fn scrolling_turns_the_wheel_at_the_centre_or_a_point() {
        let (status, fake) = run(&[c"scroll", c"129", c"0", c"-3"], machine());
        assert_eq!(status, 0);
        assert_eq!(
            fake.sent_input,
            [[129, KIND_WHEEL, 50, 25, 0, -3i32 as u32]]
        );
        let (status, fake) = run(
            &[c"scroll", c"--at", c"5", c"6", c"129", c"0", c"2"],
            machine(),
        );
        assert_eq!(status, 0);
        assert_eq!(fake.sent_input, [[129, KIND_WHEEL, 5, 6, 0, 2]]);
        let (status, fake) = run(&[c"scroll", c"64", c"0", c"1"], machine());
        assert_eq!(status, 0, "the terminal scrolls its view");
        assert_eq!(fake.sent_input, [[64, KIND_WHEEL, 0, 0, 0, 1]]);
        let (status, fake) = run(&[c"scroll", c"129", c"2", c"0"], machine());
        assert_eq!(status, FAILURE);
        assert!(fake.errors().contains("sideways"));
        assert!(fake.sent_input.is_empty());
    }

    #[test]
    fn refusals_name_the_window_and_bad_commands_are_usage_errors() {
        let (status, fake) = run(&[c"click", c"7", c"1", c"1"], machine());
        assert_eq!(status, FAILURE);
        assert_eq!(fake.errors(), "wm: 7: no such window\n  try: wm list\n");
        let mut busy = machine();
        busy.input_results = vec![Some(Errno::EBUSY)];
        let (status, _) = run(&[c"press", c"129", c"a"], busy);
        assert_eq!(status, BUSY_STATUS);
        for arguments in [
            &[c"click", c"129", c"1"][..],
            &[c"click", c"--button", c"left"][..],
            &[c"click", c"--frob", c"129", c"1", c"1"][..],
            &[c"press", c"129"][..],
            &[c"type"][..],
            &[c"scroll", c"129", c"0"][..],
        ] {
            let (status, fake) = run(arguments, machine());
            assert_eq!(status, 2, "{arguments:?}");
            assert!(fake.sent_input.is_empty());
        }
        let (status, _) = run(
            &[c"click", c"--button", c"side", c"129", c"1", c"1"],
            machine(),
        );
        assert_eq!(status, 2);
        let (status, _) = run(&[c"click", c"129", c"-1", c"1"], machine());
        assert_eq!(status, 2);
    }

    #[test]
    fn the_terminal_s_window_refuses_pointer_input_by_name() {
        let (status, fake) = run(&[c"click", c"64", c"1", c"1"], machine());
        assert_eq!(status, FAILURE);
        assert_eq!(
            fake.errors(),
            "wm: 64: the terminal's window takes keys, characters and the wheel only\n"
        );
        let (status, fake) = run(&[c"press", c"64", c"shift"], machine());
        assert_eq!(status, 0);
        assert_eq!(fake.sent_input, [[64, KIND_KEY, 0, 0, 42, 0]]);
    }

    #[test]
    fn help_lists_the_verbs_and_key_names() {
        let (status, fake) = run(&[c"help"], machine());
        assert_eq!(status, 0);
        let help = fake.output();
        for word in ["wm capture", "wm press", "PageDown", "ctrl+", "75"] {
            assert!(help.contains(word), "{word}");
        }
    }
}

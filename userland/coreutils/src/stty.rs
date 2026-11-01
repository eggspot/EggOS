use crate::output::{FAILURE, Output};
use crate::system::{STANDARD_INPUT, System};
use core::ffi::CStr;
use eggos_commands::errors;
use eggos_runtime::errno::Errno;

pub const TCGETS: u32 = 0x5401;
pub const TCSETSW: u32 = 0x5403;
pub const TIOCGWINSZ: u32 = 0x5413;
pub const TIOCSWINSZ: u32 = 0x5414;
pub const SETTINGS_SIZE: usize = 17 + CONTROL_CHARACTERS;
pub const WINDOW_SIZE_SIZE: usize = 8;
pub const CONTROL_CHARACTERS: usize = 19;

const INPUT: usize = 0;
const OUTPUT: usize = 1;
const CONTROL: usize = 2;
const LOCAL: usize = 3;
const GROUPS: [usize; 4] = [CONTROL, INPUT, OUTPUT, LOCAL];
const DEFAULT_COLUMNS: usize = 80;
const DISABLED: u8 = 0;
const DELETE: u8 = 0x7f;
const MAX_LINE: usize = 512;

const VTIME: usize = 5;
const VMIN: usize = 6;

pub const CBAUD: u32 = 0o10017;
pub const B38400: u32 = 0o17;
pub const CS8: u32 = 0o60;
pub const CREAD: u32 = 0o200;
pub const HUPCL: u32 = 0o2000;
pub const ICRNL: u32 = 0o400;
pub const IXON: u32 = 0o2000;
pub const OPOST: u32 = 0o1;
pub const ONLCR: u32 = 0o4;
pub const ISIG: u32 = 0o1;
pub const ICANON: u32 = 0o2;
pub const ECHO: u32 = 0o10;
pub const ECHOE: u32 = 0o20;
pub const ECHOK: u32 = 0o40;
pub const ECHOCTL: u32 = 0o1000;
pub const ECHOKE: u32 = 0o4000;
pub const IEXTEN: u32 = 0o100000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sane {
    Set,
    Unset,
    Neither,
}

#[derive(Debug, Clone, Copy)]
struct Mode {
    name: &'static str,
    word: usize,
    mask: u32,
    bits: u32,
    sane: Sane,
    choice: bool,
}

const fn bit(name: &'static str, word: usize, bits: u32, sane: Sane) -> Mode {
    Mode {
        name,
        word,
        mask: bits,
        bits,
        sane,
        choice: false,
    }
}

const fn choice(name: &'static str, word: usize, mask: u32, bits: u32, sane: Sane) -> Mode {
    Mode {
        name,
        word,
        mask,
        bits,
        sane,
        choice: true,
    }
}

impl Mode {
    fn is_choice(&self) -> bool {
        self.choice
    }
}

const CSIZE: u32 = 0o60;
const NLDLY: u32 = 0o400;
const CRDLY: u32 = 0o3000;
const TABDLY: u32 = 0o14000;
const BSDLY: u32 = 0o20000;
const VTDLY: u32 = 0o40000;
const FFDLY: u32 = 0o100000;

const MODES: [Mode; 59] = [
    bit("parenb", CONTROL, 0o400, Sane::Neither),
    bit("parodd", CONTROL, 0o1000, Sane::Neither),
    bit("cmspar", CONTROL, 0o10_000_000_000, Sane::Neither),
    choice("cs5", CONTROL, CSIZE, 0, Sane::Neither),
    choice("cs6", CONTROL, CSIZE, 0o20, Sane::Neither),
    choice("cs7", CONTROL, CSIZE, 0o40, Sane::Neither),
    choice("cs8", CONTROL, CSIZE, CS8, Sane::Neither),
    bit("hupcl", CONTROL, HUPCL, Sane::Neither),
    bit("cstopb", CONTROL, 0o100, Sane::Neither),
    bit("cread", CONTROL, CREAD, Sane::Set),
    bit("clocal", CONTROL, 0o4000, Sane::Neither),
    bit("crtscts", CONTROL, 0o20_000_000_000, Sane::Neither),
    bit("ignbrk", INPUT, 0o1, Sane::Unset),
    bit("brkint", INPUT, 0o2, Sane::Set),
    bit("ignpar", INPUT, 0o4, Sane::Neither),
    bit("parmrk", INPUT, 0o10, Sane::Neither),
    bit("inpck", INPUT, 0o20, Sane::Neither),
    bit("istrip", INPUT, 0o40, Sane::Neither),
    bit("inlcr", INPUT, 0o100, Sane::Unset),
    bit("igncr", INPUT, 0o200, Sane::Unset),
    bit("icrnl", INPUT, ICRNL, Sane::Set),
    bit("ixon", INPUT, IXON, Sane::Neither),
    bit("ixoff", INPUT, 0o10000, Sane::Unset),
    bit("iuclc", INPUT, 0o1000, Sane::Unset),
    bit("ixany", INPUT, 0o4000, Sane::Unset),
    bit("imaxbel", INPUT, 0o20000, Sane::Set),
    bit("iutf8", INPUT, 0o40000, Sane::Unset),
    bit("opost", OUTPUT, OPOST, Sane::Set),
    bit("olcuc", OUTPUT, 0o2, Sane::Unset),
    bit("ocrnl", OUTPUT, 0o10, Sane::Unset),
    bit("onlcr", OUTPUT, ONLCR, Sane::Set),
    bit("onocr", OUTPUT, 0o20, Sane::Unset),
    bit("onlret", OUTPUT, 0o40, Sane::Unset),
    bit("ofill", OUTPUT, 0o100, Sane::Unset),
    bit("ofdel", OUTPUT, 0o200, Sane::Unset),
    choice("nl0", OUTPUT, NLDLY, 0, Sane::Set),
    choice("nl1", OUTPUT, NLDLY, NLDLY, Sane::Neither),
    choice("cr0", OUTPUT, CRDLY, 0, Sane::Set),
    choice("cr1", OUTPUT, CRDLY, 0o1000, Sane::Neither),
    choice("cr2", OUTPUT, CRDLY, 0o2000, Sane::Neither),
    choice("cr3", OUTPUT, CRDLY, CRDLY, Sane::Neither),
    choice("tab0", OUTPUT, TABDLY, 0, Sane::Set),
    choice("tab1", OUTPUT, TABDLY, 0o4000, Sane::Neither),
    choice("tab2", OUTPUT, TABDLY, 0o10000, Sane::Neither),
    choice("tab3", OUTPUT, TABDLY, TABDLY, Sane::Neither),
    choice("bs0", OUTPUT, BSDLY, 0, Sane::Set),
    choice("bs1", OUTPUT, BSDLY, BSDLY, Sane::Neither),
    choice("vt0", OUTPUT, VTDLY, 0, Sane::Set),
    choice("vt1", OUTPUT, VTDLY, VTDLY, Sane::Neither),
    choice("ff0", OUTPUT, FFDLY, 0, Sane::Set),
    choice("ff1", OUTPUT, FFDLY, FFDLY, Sane::Neither),
    bit("isig", LOCAL, ISIG, Sane::Set),
    bit("icanon", LOCAL, ICANON, Sane::Set),
    bit("iexten", LOCAL, IEXTEN, Sane::Set),
    bit("echo", LOCAL, ECHO, Sane::Set),
    bit("echoe", LOCAL, ECHOE, Sane::Set),
    bit("echok", LOCAL, ECHOK, Sane::Set),
    bit("echonl", LOCAL, 0o100, Sane::Unset),
    bit("noflsh", LOCAL, 0o200, Sane::Unset),
];

const MORE_LOCAL_MODES: [Mode; 7] = [
    bit("xcase", LOCAL, 0o4, Sane::Unset),
    bit("tostop", LOCAL, 0o400, Sane::Unset),
    bit("echoprt", LOCAL, 0o2000, Sane::Unset),
    bit("echoctl", LOCAL, ECHOCTL, Sane::Set),
    bit("echoke", LOCAL, ECHOKE, Sane::Set),
    bit("flusho", LOCAL, 0o10000, Sane::Unset),
    bit("extproc", LOCAL, 0o200000, Sane::Unset),
];

fn all_modes() -> impl Iterator<Item = &'static Mode> {
    MODES.iter().chain(MORE_LOCAL_MODES.iter())
}

const CHARACTERS: [(&str, usize, u8); 15] = [
    ("intr", 0, 0x03),
    ("quit", 1, 0x1c),
    ("erase", 2, DELETE),
    ("kill", 3, 0x15),
    ("eof", 4, 0x04),
    ("eol", 11, DISABLED),
    ("eol2", 16, DISABLED),
    ("swtch", 7, DISABLED),
    ("start", 8, 0x11),
    ("stop", 9, 0x13),
    ("susp", 10, 0x1a),
    ("rprnt", 12, 0x12),
    ("werase", 14, 0x17),
    ("lnext", 15, 0x16),
    ("discard", 13, 0x0f),
];

const RAW_OFF: [&str; 18] = [
    "ignbrk", "brkint", "ignpar", "parmrk", "inpck", "istrip", "inlcr", "igncr", "icrnl", "ixon",
    "ixoff", "icanon", "opost", "isig", "iuclc", "ixany", "imaxbel", "xcase",
];

const COOKED_ON: [&str; 6] = ["brkint", "ignpar", "istrip", "icrnl", "ixon", "opost"];

const SPEEDS: [(u32, u32); 31] = [
    (0, 0),
    (1, 50),
    (2, 75),
    (3, 110),
    (4, 134),
    (5, 150),
    (6, 200),
    (7, 300),
    (8, 600),
    (9, 1200),
    (0o12, 1800),
    (0o13, 2400),
    (0o14, 4800),
    (0o15, 9600),
    (0o16, 19200),
    (0o17, 38400),
    (0o10001, 57600),
    (0o10002, 115_200),
    (0o10003, 230_400),
    (0o10004, 460_800),
    (0o10005, 500_000),
    (0o10006, 576_000),
    (0o10007, 921_600),
    (0o10010, 1_000_000),
    (0o10011, 1_152_000),
    (0o10012, 1_500_000),
    (0o10013, 2_000_000),
    (0o10014, 2_500_000),
    (0o10015, 3_000_000),
    (0o10016, 3_500_000),
    (0o10017, 4_000_000),
];

/// A terminal's settings as `TCGETS` lays them out: the input, output, control and local flag
/// words, the line discipline and the control characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub words: [u32; 4],
    pub line: u8,
    pub characters: [u8; CONTROL_CHARACTERS],
}

impl Settings {
    pub fn from_bytes(bytes: &[u8; SETTINGS_SIZE]) -> Self {
        let word = |at: usize| {
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let mut characters = [0; CONTROL_CHARACTERS];
        characters.copy_from_slice(&bytes[17..]);
        Self {
            words: [word(0), word(4), word(8), word(12)],
            line: bytes[16],
            characters,
        }
    }

    pub fn to_bytes(&self) -> [u8; SETTINGS_SIZE] {
        let mut bytes = [0; SETTINGS_SIZE];
        for (index, word) in self.words.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
        }
        bytes[16] = self.line;
        bytes[17..].copy_from_slice(&self.characters);
        bytes
    }

    /// A new pseudo-terminal's settings (D187): 38400 baud, 8 bits, canonical with echo and
    /// signals, carriage return read as newline, newline written as carriage return and
    /// newline.
    pub fn pseudo_terminal() -> Self {
        let mut characters = [0; CONTROL_CHARACTERS];
        for (_, index, value) in CHARACTERS {
            characters[index] = value;
        }
        characters[VMIN] = 1;
        Self {
            words: [
                ICRNL | IXON,
                OPOST | ONLCR,
                B38400 | CS8 | CREAD | HUPCL,
                ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
            ],
            line: 0,
            characters,
        }
    }

    fn has(&self, mode: &Mode) -> bool {
        self.words[mode.word] & mode.mask == mode.bits
    }

    fn set(&mut self, mode: &Mode, on: bool) {
        let word = &mut self.words[mode.word];
        *word &= !mode.mask;
        if on {
            *word |= mode.bits;
        }
    }

    fn speed(&self) -> u32 {
        let code = self.words[CONTROL] & CBAUD;
        SPEEDS
            .iter()
            .find(|(known, _)| *known == code)
            .map_or(0, |(_, baud)| *baud)
    }

    fn make_sane(&mut self) {
        for mode in all_modes() {
            match mode.sane {
                Sane::Set => self.set(mode, true),
                Sane::Unset if !mode.is_choice() => self.set(mode, false),
                _ => {}
            }
        }
        for (_, index, value) in CHARACTERS {
            self.characters[index] = value;
        }
        self.characters[VMIN] = 1;
        self.characters[VTIME] = 0;
    }

    fn make_raw(&mut self, raw: bool) {
        match raw {
            true => {
                for name in RAW_OFF {
                    if let Some(mode) = find_mode(name) {
                        self.set(mode, false);
                    }
                }
                self.characters[VMIN] = 1;
                self.characters[VTIME] = 0;
            }
            false => {
                for name in COOKED_ON {
                    if let Some(mode) = find_mode(name) {
                        self.set(mode, true);
                    }
                }
                if let Some(mode) = find_mode("icanon") {
                    self.set(mode, true);
                }
                if let Some(mode) = find_mode("isig") {
                    self.set(mode, true);
                }
            }
        }
    }
}

fn find_mode(name: &str) -> Option<&'static Mode> {
    all_modes().find(|mode| mode.name == name)
}

struct Wrapped<'a, S: System> {
    output: &'a mut Output<S>,
    column: usize,
    width: usize,
}

impl<S: System> Wrapped<'_, S> {
    fn item(&mut self, item: &[u8]) {
        if self.column > 0 {
            if self.column + 1 + item.len() > self.width {
                self.output.write(b"\n");
                self.column = 0;
            } else {
                self.output.write(b" ");
                self.column += 1;
            }
        }
        self.output.write(item);
        self.column += item.len();
    }

    fn end_line(&mut self) {
        if self.column > 0 {
            self.output.write(b"\n");
            self.column = 0;
        }
    }
}

struct Line {
    bytes: [u8; MAX_LINE],
    length: usize,
}

impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; MAX_LINE],
            length: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> &mut Self {
        let end = (self.length + bytes.len()).min(MAX_LINE);
        let count = end - self.length;
        self.bytes[self.length..end].copy_from_slice(&bytes[..count]);
        self.length = end;
        self
    }

    fn number(&mut self, value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let text = crate::output::decimal(value, &mut digits);
        self.push(text)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

fn character_text(value: u8, line: &mut Line) {
    match value {
        DISABLED => {
            line.push(b"<undef>");
        }
        DELETE => {
            line.push(b"^?");
        }
        value if value < 0x20 => {
            line.push(&[b'^', value + 0x40]);
        }
        value if value >= 0x80 => {
            line.push(b"M-");
            character_text(value & 0x7f, line);
        }
        value => {
            line.push(&[value]);
        }
    }
}

fn character_item(name: &str, value: u8) -> Line {
    let mut line = Line::new();
    line.push(name.as_bytes()).push(b" = ");
    character_text(value, &mut line);
    line.push(b";");
    line
}

fn number_item(name: &str, value: u8) -> Line {
    let mut line = Line::new();
    line.push(name.as_bytes())
        .push(b" = ")
        .number(u64::from(value))
        .push(b";");
    line
}

fn mode_item(mode: &Mode, on: bool) -> Line {
    let mut line = Line::new();
    if !on {
        line.push(b"-");
    }
    line.push(mode.name.as_bytes());
    line
}

fn show_all<S: System>(
    output: &mut Output<S>,
    settings: &Settings,
    size: Option<(u16, u16)>,
    width: usize,
) {
    let mut out = Wrapped {
        output,
        column: 0,
        width,
    };
    let mut speed = Line::new();
    speed
        .push(b"speed ")
        .number(u64::from(settings.speed()))
        .push(b" baud;");
    out.item(speed.as_bytes());
    if let Some((rows, columns)) = size {
        let mut item = Line::new();
        item.push(b"rows ").number(u64::from(rows)).push(b";");
        out.item(item.as_bytes());
        let mut item = Line::new();
        item.push(b"columns ").number(u64::from(columns)).push(b";");
        out.item(item.as_bytes());
    }
    out.item(number_item("line", settings.line).as_bytes());
    out.end_line();
    for (name, index, _) in CHARACTERS {
        out.item(character_item(name, settings.characters[index]).as_bytes());
    }
    out.item(number_item("min", settings.characters[VMIN]).as_bytes());
    out.item(number_item("time", settings.characters[VTIME]).as_bytes());
    out.end_line();
    for group in GROUPS {
        for mode in all_modes().filter(|mode| mode.word == group) {
            match mode.is_choice() {
                true if settings.has(mode) => out.item(mode.name.as_bytes()),
                true => {}
                false => out.item(mode_item(mode, settings.has(mode)).as_bytes()),
            }
        }
        out.end_line();
    }
}

fn show_changed<S: System>(output: &mut Output<S>, settings: &Settings, width: usize) {
    let mut out = Wrapped {
        output,
        column: 0,
        width,
    };
    let mut speed = Line::new();
    speed
        .push(b"speed ")
        .number(u64::from(settings.speed()))
        .push(b" baud;");
    out.item(speed.as_bytes());
    out.item(number_item("line", settings.line).as_bytes());
    out.end_line();
    for (name, index, default) in CHARACTERS {
        if settings.characters[index] != default {
            out.item(character_item(name, settings.characters[index]).as_bytes());
        }
    }
    if settings.words[LOCAL] & ICANON == 0 {
        out.item(number_item("min", settings.characters[VMIN]).as_bytes());
        out.item(number_item("time", settings.characters[VTIME]).as_bytes());
    }
    out.end_line();
    for group in GROUPS {
        for mode in all_modes().filter(|mode| mode.word == group) {
            let on = settings.has(mode);
            if mode.is_choice() {
                let sane_choice = all_modes()
                    .find(|other| other.mask == mode.mask && other.word == mode.word && other.sane == Sane::Set);
                if on && sane_choice.is_some_and(|sane| sane.bits != mode.bits) {
                    out.item(mode.name.as_bytes());
                }
                continue;
            }
            let differs = match mode.sane {
                Sane::Set => !on,
                Sane::Unset => on,
                Sane::Neither => false,
            };
            if differs {
                out.item(mode_item(mode, on).as_bytes());
            }
        }
        out.end_line();
    }
}

fn hex(value: u32, line: &mut Line) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut digits = [0u8; 8];
    let mut start = digits.len();
    let mut rest = value;
    loop {
        start -= 1;
        digits[start] = DIGITS[(rest & 0xf) as usize];
        rest >>= 4;
        if rest == 0 {
            break;
        }
    }
    line.push(&digits[start..]);
}

fn saved_form(settings: &Settings) -> Line {
    let mut line = Line::new();
    for (index, word) in settings.words.iter().enumerate() {
        if index > 0 {
            line.push(b":");
        }
        hex(*word, &mut line);
    }
    for character in settings.characters {
        line.push(b":");
        hex(u32::from(character), &mut line);
    }
    line
}

fn parse_hex(text: &[u8]) -> Option<u32> {
    if text.is_empty() || text.len() > 8 {
        return None;
    }
    text.iter().try_fold(0u32, |value, &byte| {
        let digit = (byte as char).to_digit(16)?;
        Some(value << 4 | digit)
    })
}

fn parse_saved(text: &[u8], settings: &mut Settings) -> bool {
    let mut parsed = *settings;
    let mut count = 0;
    for field in text.split(|&byte| byte == b':') {
        let Some(value) = parse_hex(field) else {
            return false;
        };
        match count {
            0..4 => parsed.words[count] = value,
            index if index < 4 + CONTROL_CHARACTERS => match u8::try_from(value) {
                Ok(character) => parsed.characters[index - 4] = character,
                Err(_) => return false,
            },
            _ => return false,
        }
        count += 1;
    }
    if count != 4 + CONTROL_CHARACTERS {
        return false;
    }
    *settings = parsed;
    true
}

fn parse_character(text: &[u8]) -> Option<u8> {
    match text {
        b"undef" | b"^-" | b"" => Some(DISABLED),
        b"^?" => Some(DELETE),
        [b'^', letter] => Some(letter.to_ascii_uppercase() & 0x1f),
        [single] => Some(*single),
        _ => None,
    }
}

fn parse_number(text: &[u8], maximum: u32) -> Option<u32> {
    if text.is_empty() || text.len() > 9 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = text
        .iter()
        .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0'));
    (value <= maximum).then_some(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Changed,
    All,
    Saved,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeChange {
    Rows(u16),
    Columns(u16),
}

fn window_size<S: System>(output: &mut Output<S>, descriptor: usize) -> Result<(u16, u16), Errno> {
    let mut size = [0u8; WINDOW_SIZE_SIZE];
    output
        .system
        .device_control(descriptor, TIOCGWINSZ, &mut size)?;
    Ok((
        u16::from_ne_bytes([size[0], size[1]]),
        u16::from_ne_bytes([size[2], size[3]]),
    ))
}

fn invalid<S: System>(output: &mut Output<S>, word: &[u8]) -> i32 {
    output.coded(&errors::USAGE, &[b"invalid argument '", word, b"'"]);
    FAILURE
}

pub fn stty<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let mut style = Style::Changed;
    let mut device: Option<&CStr> = None;
    let mut words: [&[u8]; 64] = [b""; 64];
    let mut count = 0;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_bytes();
        index += 1;
        match argument {
            b"-a" | b"--all" => style = Style::All,
            b"-g" | b"--save" => style = Style::Saved,
            b"-F" | b"--file" => match arguments.get(index) {
                Some(path) => {
                    device = Some(path);
                    index += 1;
                }
                None => {
                    output.coded(&errors::USAGE, &[b"option requires an argument -- 'F'"]);
                    return FAILURE;
                }
            },
            b"size" => style = Style::Size,
            word => {
                if count == words.len() {
                    return invalid(output, word);
                }
                words[count] = word;
                count += 1;
            }
        }
    }
    let words = &words[..count];
    if style != Style::Changed && !words.is_empty() {
        output.coded(
            &errors::USAGE,
            &[b"when specifying an output style, modes may not be set"],
        );
        return FAILURE;
    }
    let (descriptor, name): (usize, &[u8]) = match device {
        Some(path) => match output.system.open(path) {
            Ok(descriptor) => (descriptor, path.to_bytes()),
            Err(errno) => {
                output.file_error(path, errno);
                return FAILURE;
            }
        },
        None => (STANDARD_INPUT, b"standard input"),
    };
    let status = run_on(output, descriptor, name, style, words);
    if device.is_some() {
        output.system.close(descriptor);
    }
    status
}

fn run_on<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    name: &[u8],
    style: Style,
    words: &[&[u8]],
) -> i32 {
    let mut bytes = [0u8; SETTINGS_SIZE];
    if let Err(errno) = output.system.device_control(descriptor, TCGETS, &mut bytes) {
        output.error(&[b"'", name, b"': ", errno.message()]);
        return FAILURE;
    }
    let settings = Settings::from_bytes(&bytes);
    let size = window_size(output, descriptor).ok();
    let width = size
        .map(|(_, columns)| usize::from(columns))
        .filter(|&columns| columns > 0)
        .unwrap_or(DEFAULT_COLUMNS);
    match style {
        Style::Size => match size {
            Some((rows, columns)) => {
                output.write_number(u64::from(rows), 0);
                output.write(b" ");
                output.write_number(u64::from(columns), 0);
                output.write(b"\n");
                0
            }
            None => {
                output.error(&[b"'", name, b"': ", Errno::ENOTTY.message()]);
                FAILURE
            }
        },
        Style::All => {
            show_all(output, &settings, size, width);
            0
        }
        Style::Saved => {
            output.write(saved_form(&settings).as_bytes());
            output.write(b"\n");
            0
        }
        Style::Changed if words.is_empty() => {
            show_changed(output, &settings, width);
            0
        }
        Style::Changed => change(output, descriptor, name, settings, words),
    }
}

fn change<S: System>(
    output: &mut Output<S>,
    descriptor: usize,
    name: &[u8],
    mut settings: Settings,
    words: &[&[u8]],
) -> i32 {
    let mut size_changes: [Option<SizeChange>; 2] = [None; 2];
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        index += 1;
        let (negated, bare) = match word.strip_prefix(b"-") {
            Some(bare) => (true, bare),
            None => (false, word),
        };
        let mut value = || {
            let value = words.get(index).copied();
            index += 1;
            value
        };
        if let Some(mode) = all_modes().find(|mode| mode.name.as_bytes() == bare) {
            if negated && mode.is_choice() {
                return invalid(output, word);
            }
            settings.set(mode, !negated);
            continue;
        }
        match (negated, bare) {
            (false, b"raw") | (true, b"cooked") => settings.make_raw(true),
            (true, b"raw") | (false, b"cooked") => settings.make_raw(false),
            (false, b"sane") => settings.make_sane(),
            (false, b"rows" | b"cols" | b"columns" | b"min" | b"time") => {
                let Some(text) = value() else {
                    output.coded(&errors::USAGE, &[b"missing argument to '", bare, b"'"]);
                    return FAILURE;
                };
                let maximum = match bare {
                    b"min" | b"time" => u32::from(u8::MAX),
                    _ => u32::from(u16::MAX),
                };
                let Some(number) = parse_number(text, maximum) else {
                    return invalid(output, text);
                };
                match bare {
                    b"rows" => size_changes[0] = Some(SizeChange::Rows(number as u16)),
                    b"min" => settings.characters[VMIN] = number as u8,
                    b"time" => settings.characters[VTIME] = number as u8,
                    _ => size_changes[1] = Some(SizeChange::Columns(number as u16)),
                }
            }
            (false, bare) if CHARACTERS.iter().any(|(known, _, _)| known.as_bytes() == bare) => {
                let Some(text) = value() else {
                    output.coded(&errors::USAGE, &[b"missing argument to '", bare, b"'"]);
                    return FAILURE;
                };
                let Some(character) = parse_character(text) else {
                    return invalid(output, text);
                };
                if let Some((_, slot, _)) =
                    CHARACTERS.iter().find(|(known, _, _)| known.as_bytes() == bare)
                {
                    settings.characters[*slot] = character;
                }
            }
            (false, text) if text.contains(&b':') => {
                if !parse_saved(text, &mut settings) {
                    return invalid(output, word);
                }
            }
            _ => return invalid(output, word),
        }
    }
    if let Err(errno) = output
        .system
        .device_control(descriptor, TCSETSW, &mut settings.to_bytes())
    {
        output.error(&[b"'", name, b"': ", errno.message()]);
        return FAILURE;
    }
    if size_changes.iter().any(Option::is_some) {
        let mut size = [0u8; WINDOW_SIZE_SIZE];
        if let Err(errno) = output.system.device_control(descriptor, TIOCGWINSZ, &mut size) {
            output.error(&[b"'", name, b"': ", errno.message()]);
            return FAILURE;
        }
        for change in size_changes.into_iter().flatten() {
            match change {
                SizeChange::Rows(rows) => size[0..2].copy_from_slice(&rows.to_ne_bytes()),
                SizeChange::Columns(columns) => size[2..4].copy_from_slice(&columns.to_ne_bytes()),
            }
        }
        if let Err(errno) = output.system.device_control(descriptor, TIOCSWINSZ, &mut size) {
            output.error(&[b"'", name, b"': ", errno.message()]);
            return FAILURE;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeTerminal};

    fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String, Fake) {
        let mut output = Output::new(fake, b"stty");
        let status = stty(&mut output, arguments);
        let status = output.finish(status);
        let fake = output.system;
        let (out, errors) = (fake.output(), fake.errors());
        (status, out, errors, fake)
    }

    fn terminal() -> Fake {
        let mut fake = Fake::default();
        fake.terminal = Some(FakeTerminal::default());
        fake
    }

    const DEFAULTS: &str = "speed 38400 baud; rows 24; columns 80; line = 0;\n\
        intr = ^C; quit = ^\\; erase = ^?; kill = ^U; eof = ^D; eol = <undef>;\n\
        eol2 = <undef>; swtch = <undef>; start = ^Q; stop = ^S; susp = ^Z; rprnt = ^R;\n\
        werase = ^W; lnext = ^V; discard = ^O; min = 1; time = 0;\n\
        -parenb -parodd -cmspar cs8 hupcl -cstopb cread -clocal -crtscts\n\
        -ignbrk -brkint -ignpar -parmrk -inpck -istrip -inlcr -igncr icrnl ixon -ixoff\n\
        -iuclc -ixany -imaxbel -iutf8\n\
        opost -olcuc -ocrnl onlcr -onocr -onlret -ofill -ofdel nl0 cr0 tab0 bs0 vt0 ff0\n\
        isig icanon iexten echo echoe echok -echonl -noflsh -xcase -tostop -echoprt\n\
        echoctl echoke -flusho -extproc\n";

    #[test]
    fn size_prints_rows_then_columns_and_follows_a_new_window_size() {
        let (status, out, _, mut fake) = run(terminal(), &[c"size"]);
        assert_eq!((status, out.as_str()), (0, "24 80\n"));
        if let Some(terminal) = fake.terminal.as_mut() {
            terminal.set_window_size(30, 100);
        }
        fake.standard_output.clear();
        let (status, out, _, _) = run(fake, &[c"size"]);
        assert_eq!((status, out.as_str()), (0, "30 100\n"));
    }

    #[test]
    fn all_shows_every_setting_in_gnu_s_order_and_wrapping() {
        let (status, out, _, _) = run(terminal(), &[c"-a"]);
        assert_eq!(status, 0);
        assert_eq!(out, DEFAULTS);
        let (_, out, _, _) = run(terminal(), &[]);
        assert_eq!(out, "speed 38400 baud; line = 0;\n-brkint -imaxbel\n");
    }

    #[test]
    fn raw_then_saved_differs_and_the_saved_form_restores_and_sane_resets() {
        let (_, saved, _, fake) = run(terminal(), &[c"-g"]);
        let saved = saved.trim_end().to_string();
        assert_eq!(saved.split(':').count(), 4 + CONTROL_CHARACTERS);
        let (status, out, _, mut fake) = run(fake, &[c"raw"]);
        assert_eq!((status, out.as_str()), (0, ""));
        let raw = fake.terminal.as_ref().map(|terminal| terminal.settings);
        let raw = Settings::from_bytes(&raw.unwrap());
        assert_eq!(raw.words[LOCAL] & ICANON, 0);
        assert_eq!(raw.words[OUTPUT] & OPOST, 0);
        fake.standard_output.clear();
        let (_, raw_saved, _, mut fake) = run(fake, &[c"-g"]);
        assert_ne!(raw_saved.trim_end(), saved);
        let restore = std::ffi::CString::new(saved.clone()).unwrap();
        fake.standard_output.clear();
        let (status, _, _, mut fake) = run(fake, &[restore.as_c_str()]);
        assert_eq!(status, 0);
        assert_eq!(
            fake.terminal.as_ref().map(|terminal| terminal.settings),
            Some(Settings::pseudo_terminal().to_bytes())
        );
        fake.standard_output.clear();
        let (_, _, _, fake) = run(fake, &[c"raw", c"sane"]);
        let sane = Settings::from_bytes(&fake.terminal.as_ref().unwrap().settings);
        assert_eq!(sane.words[LOCAL] & ICANON, ICANON);
        assert_eq!(sane.words[INPUT] & 0o2, 0o2, "sane sets brkint");
        assert_eq!(sane.characters, Settings::pseudo_terminal().characters);
    }

    #[test]
    fn modes_characters_and_the_window_size_are_set() {
        let (status, _, _, fake) = run(
            terminal(),
            &[c"-echo", c"intr", c"^X", c"erase", c"undef", c"min", c"5", c"cols", c"100"],
        );
        assert_eq!(status, 0);
        let terminal = fake.terminal.as_ref().unwrap();
        let settings = Settings::from_bytes(&terminal.settings);
        assert_eq!(settings.words[LOCAL] & ECHO, 0);
        assert_eq!(settings.characters[0], 0x18);
        assert_eq!(settings.characters[2], DISABLED);
        assert_eq!(settings.characters[VMIN], 5);
        assert_eq!(terminal.window_size(), (24, 100));
        assert_eq!(terminal.window_size_changes, 1, "SIGWINCH goes to the foreground");
    }

    #[test]
    fn it_says_what_is_wrong_as_gnu_does() {
        let (status, _, errors, _) = run(Fake::default(), &[c"size"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "stty: 'standard input': Inappropriate ioctl for device\n");
        let (status, _, errors, _) = run(terminal(), &[c"frob"]);
        assert_eq!(status, 1);
        assert_eq!(errors, "stty: invalid argument 'frob'\n");
        let (_, _, errors, _) = run(terminal(), &[c"-a", c"echo"]);
        assert_eq!(
            errors,
            "stty: when specifying an output style, modes may not be set\n"
        );
        let (_, _, errors, _) = run(terminal(), &[c"rows"]);
        assert_eq!(errors, "stty: missing argument to 'rows'\n");
        let (_, _, errors, _) = run(terminal(), &[c"-cs8"]);
        assert_eq!(errors, "stty: invalid argument '-cs8'\n");
    }
}

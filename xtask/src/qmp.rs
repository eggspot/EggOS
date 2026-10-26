//! A minimal QEMU Machine Protocol client: what the boot tests need to look at the display
//! (`screendump`) and to type and point on its input devices (`input-send-event`), over a TCP
//! socket on localhost. QMP speaks one JSON object per line; the
//! client only sends commands it writes itself and looks for the `return` or `error` answer,
//! skipping the asynchronous events in between, so it needs no JSON parser.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// A free TCP port on localhost for QEMU's `-qmp tcp:...` to listen on.
pub fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|e| format!("no free local port for QMP: {e}"))
}

/// QEMU's arguments to listen for QMP on `port`.
pub fn qemu_args(port: u16) -> Vec<String> {
    vec![
        "-qmp".to_string(),
        format!("tcp:127.0.0.1:{port},server=on,wait=off"),
    ]
}

/// A QMP session in command mode.
#[derive(Debug)]
pub struct Qmp {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Qmp {
    /// Connects to QEMU's QMP socket on `port`, retrying while QEMU starts, and leaves the
    /// capabilities negotiation.
    pub fn connect(port: u16) -> Result<Self, String> {
        let started = Instant::now();
        let stream = loop {
            match TcpStream::connect((Ipv4Addr::LOCALHOST, port)) {
                Ok(stream) => break stream,
                Err(e) if started.elapsed() > CONNECT_TIMEOUT => {
                    return Err(format!("could not connect to QEMU's QMP socket: {e}"));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        stream
            .set_read_timeout(Some(ANSWER_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut qmp = Self {
            writer: stream,
            reader,
        };
        let greeting = qmp.line()?;
        if !greeting.contains("\"QMP\"") {
            return Err(format!("QEMU's QMP socket greeted with {greeting:?}"));
        }
        qmp.execute("{\"execute\": \"qmp_capabilities\"}")?;
        Ok(qmp)
    }

    /// Saves the display's current picture to `path` as a binary PPM.
    pub fn screendump(&mut self, path: &Path) -> Result<(), String> {
        let command = format!(
            "{{\"execute\": \"screendump\", \"arguments\": {{\"filename\": {}}}}}",
            json_string(&path.to_string_lossy())
        );
        self.execute(&command)
    }

    /// Sends `events` to the machine's input devices as one report (QEMU ends it with a
    /// `SYN_REPORT`).
    pub fn send_input(&mut self, events: &[InputEvent]) -> Result<(), String> {
        self.execute(&input_command(events))
    }

    /// Types `text` on the machine's keyboard, a key press and release per character (with
    /// Shift around those that need it), one report each, `pause` apart.
    pub fn type_text(&mut self, text: &str, pause: Duration) -> Result<(), String> {
        for character in text.chars() {
            self.send_input(&typing(character)?)?;
            std::thread::sleep(pause);
        }
        Ok(())
    }

    fn execute(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.writer, "{command}").map_err(|e| format!("QMP: {e}"))?;
        loop {
            let line = self.line()?;
            if line.contains("\"return\"") {
                return Ok(());
            }
            if line.contains("\"error\"") {
                return Err(format!("QMP refused {command}: {line}"));
            }
        }
    }

    fn line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("QEMU closed its QMP socket".into()),
            Ok(_) => Ok(line),
            Err(e) => Err(format!("QMP: {e}")),
        }
    }
}

/// One event for QEMU's `input-send-event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// A key, by QEMU's key name (`qcode`), pressed (`down`) or released.
    Key { qcode: &'static str, down: bool },
    /// A pointer button (`left`, `right`, `middle`, `wheel-up`, `wheel-down`).
    Button { button: &'static str, down: bool },
    /// Relative motion on `axis` (`x` or `y`).
    Relative { axis: &'static str, value: i32 },
    /// An absolute position on `axis` (`x` or `y`), 0 to 32767.
    Absolute { axis: &'static str, value: i32 },
}

impl InputEvent {
    fn json(&self) -> String {
        match *self {
            InputEvent::Key { qcode, down } => format!(
                "{{\"type\": \"key\", \"data\": {{\"down\": {down}, \"key\": {{\"type\": \"qcode\", \"data\": {}}}}}}}",
                json_string(qcode)
            ),
            InputEvent::Button { button, down } => format!(
                "{{\"type\": \"btn\", \"data\": {{\"down\": {down}, \"button\": {}}}}}",
                json_string(button)
            ),
            InputEvent::Relative { axis, value } => format!(
                "{{\"type\": \"rel\", \"data\": {{\"axis\": {}, \"value\": {value}}}}}",
                json_string(axis)
            ),
            InputEvent::Absolute { axis, value } => format!(
                "{{\"type\": \"abs\", \"data\": {{\"axis\": {}, \"value\": {value}}}}}",
                json_string(axis)
            ),
        }
    }
}

fn input_command(events: &[InputEvent]) -> String {
    let events: Vec<String> = events.iter().map(InputEvent::json).collect();
    format!(
        "{{\"execute\": \"input-send-event\", \"arguments\": {{\"events\": [{}]}}}}",
        events.join(", ")
    )
}

const UNSHIFTED_KEYS: [(char, &str); 17] = [
    ('-', "minus"),
    ('=', "equal"),
    ('[', "bracket_left"),
    (']', "bracket_right"),
    (';', "semicolon"),
    ('\'', "apostrophe"),
    ('`', "grave_accent"),
    ('\\', "backslash"),
    (',', "comma"),
    ('.', "dot"),
    ('/', "slash"),
    (' ', "spc"),
    ('\r', "ret"),
    ('\n', "ret"),
    ('\t', "tab"),
    ('\x7f', "backspace"),
    ('\x1b', "esc"),
];

const SHIFTED_KEYS: [(char, char); 21] = [
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    (':', ';'),
    ('"', '\''),
    ('~', '`'),
    ('|', '\\'),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
];

const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";

fn unshifted_key(character: char) -> Option<&'static str> {
    if let Some(index) = LETTERS.find(character) {
        return Some(&LETTERS[index..index + 1]);
    }
    if let Some(index) = DIGITS.find(character) {
        return Some(&DIGITS[index..index + 1]);
    }
    UNSHIFTED_KEYS
        .iter()
        .find(|(key, _)| *key == character)
        .map(|(_, qcode)| *qcode)
}

/// The report that types `character` on a US keyboard: its key pressed and released, inside
/// Shift for the characters that need it.
pub fn typing(character: char) -> Result<Vec<InputEvent>, String> {
    let unshifted = SHIFTED_KEYS
        .iter()
        .find(|(shifted, _)| *shifted == character)
        .map(|(_, unshifted)| *unshifted);
    let shift = unshifted.is_some() || character.is_ascii_uppercase();
    let base = unshifted.unwrap_or(character.to_ascii_lowercase());
    let qcode = unshifted_key(base).ok_or_else(|| format!("no US key types {character:?}"))?;
    let key = |down| InputEvent::Key { qcode, down };
    let shift_key = |down| InputEvent::Key {
        qcode: "shift",
        down,
    };
    Ok(match shift {
        true => vec![shift_key(true), key(true), key(false), shift_key(false)],
        false => vec![key(true), key(false)],
    })
}

/// `text` as a JSON string literal.
pub fn json_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if u32::from(control) < 0x20 => {
                quoted.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// A picture of the display: its size and its pixels, row after row, each `0x00RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Picture {
    /// Reads a binary PPM (`P6`, 8 bits per channel), as QEMU's `screendump` writes it.
    pub fn from_ppm(bytes: &[u8]) -> Result<Self, String> {
        let mut fields = Vec::new();
        let mut at = 0;
        while fields.len() < 4 {
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                at += 1;
            }
            if start == at {
                return Err("the PPM header is cut short".into());
            }
            fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
        }
        let number = |field: &str| {
            field
                .parse::<u32>()
                .map_err(|_| format!("bad PPM header field {field:?}"))
        };
        if fields[0] != "P6" || number(&fields[3])? != 255 {
            return Err(format!(
                "not an 8-bit binary PPM: {} {}",
                fields[0], fields[3]
            ));
        }
        let (width, height) = (number(&fields[1])?, number(&fields[2])?);
        let data = bytes.get(at + 1..).unwrap_or_default();
        let expected = width as usize * height as usize * 3;
        if data.len() != expected {
            return Err(format!(
                "the PPM holds {} bytes of pixels, not {expected}",
                data.len()
            ));
        }
        let pixels = data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[red, green, blue]| {
                u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
            })
            .collect();
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// FNV-1a, 64 bits, over the pixels' little-endian bytes: the kernel's `pixel_hash`.
    pub fn hash(&self) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        self.pixels
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .fold(OFFSET_BASIS, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(PRIME)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(
            json_string("C:\\target\\a \"b\".ppm\n"),
            "\"C:\\\\target\\\\a \\\"b\\\".ppm\\u000a\""
        );
    }

    #[test]
    fn input_events_are_sent_as_qemu_s_input_send_event_command() {
        let command = input_command(&[
            InputEvent::Key {
                qcode: "a",
                down: true,
            },
            InputEvent::Button {
                button: "left",
                down: false,
            },
            InputEvent::Relative {
                axis: "x",
                value: -7,
            },
            InputEvent::Absolute {
                axis: "y",
                value: 32767,
            },
        ]);
        assert_eq!(
            command,
            "{\"execute\": \"input-send-event\", \"arguments\": {\"events\": [\
             {\"type\": \"key\", \"data\": {\"down\": true, \"key\": {\"type\": \"qcode\", \"data\": \"a\"}}}, \
             {\"type\": \"btn\", \"data\": {\"down\": false, \"button\": \"left\"}}, \
             {\"type\": \"rel\", \"data\": {\"axis\": \"x\", \"value\": -7}}, \
             {\"type\": \"abs\", \"data\": {\"axis\": \"y\", \"value\": 32767}}]}}"
        );
    }

    #[test]
    fn characters_are_typed_with_their_us_key_and_shift_when_they_need_it() {
        let keys = |character| -> Vec<(&str, bool)> {
            typing(character)
                .unwrap()
                .into_iter()
                .map(|event| match event {
                    InputEvent::Key { qcode, down } => (qcode, down),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(keys('u'), [("u", true), ("u", false)]);
        assert_eq!(keys('7'), [("7", true), ("7", false)]);
        assert_eq!(keys('-'), [("minus", true), ("minus", false)]);
        assert_eq!(keys('\r'), [("ret", true), ("ret", false)]);
        assert_eq!(
            keys('T'),
            [("shift", true), ("t", true), ("t", false), ("shift", false)]
        );
        assert_eq!(
            keys(':'),
            [
                ("shift", true),
                ("semicolon", true),
                ("semicolon", false),
                ("shift", false)
            ]
        );
        assert_eq!(keys('_')[1], ("minus", true));
        assert_eq!(keys('+')[1], ("equal", true));
        assert_eq!(keys('"')[1], ("apostrophe", true));
        for character in (' '..='~').chain(['\r', '\t']) {
            assert!(typing(character).is_ok(), "{character:?}");
        }
        assert!(typing('\u{e9}').is_err());
    }

    #[test]
    fn a_ppm_reads_as_packed_pixels_and_hashes_as_the_kernel_does() {
        let mut ppm = b"P6\n2 1\n255\n".to_vec();
        ppm.extend([0x0e, 0x17, 0x26, 0x64, 0x63, 0x62]);
        let picture = Picture::from_ppm(&ppm).unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(picture.pixels, [0x000e_1726, 0x0064_6362]);
        let one = Picture {
            width: 1,
            height: 1,
            pixels: vec![0x6463_6261],
        };
        assert_eq!(one.hash(), 0xfc17_9f83_ee07_24dd);
    }

    #[test]
    fn a_ppm_cut_short_or_of_another_kind_is_refused() {
        assert!(Picture::from_ppm(b"P6\n2 1\n255\n\x01\x02").is_err());
        assert!(Picture::from_ppm(b"P5\n1 1\n255\n\x01").is_err());
        assert!(Picture::from_ppm(b"P6\n1 1\n65535\n\x01\x02\x03").is_err());
        assert!(Picture::from_ppm(b"P6\n1").is_err());
    }
}

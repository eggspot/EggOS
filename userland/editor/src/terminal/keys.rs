//! What a terminal sends for a key (D581): UTF-8 text, control bytes, and the escape
//! sequences of xterm and the VT100/VT220 family (`CSI` and `SS3`, with xterm's modifier
//! parameter), read from the start of a buffer.

/// A key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F12.
    Function(u8),
}

/// A key with the modifiers held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: Key,
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

/// What the start of a buffer held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A key, which took this many bytes.
    Press(Press, usize),
    /// The bytes so far start a sequence that may go on: wait a little for more; a lone
    /// `ESC` that stays alone is the Escape key.
    Incomplete,
    /// This many bytes mean nothing known: skip them.
    Unknown(usize),
}

const ESC: u8 = 0x1b;
const MAX_PARAMETERS: usize = 4;

impl Press {
    /// `key` with no modifier.
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            shift: false,
            alt: false,
            control: false,
        }
    }

    fn with_modifier(key: Key, parameter: u16) -> Self {
        let bits = parameter.saturating_sub(1);
        Self {
            key,
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            control: bits & 4 != 0,
        }
    }

    /// `Ctrl` and `letter` (lower case).
    pub const fn control(letter: char) -> Self {
        Self {
            key: Key::Character(letter),
            shift: false,
            alt: false,
            control: true,
        }
    }
}

fn utf8(bytes: &[u8]) -> Decoded {
    let length = match bytes[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Decoded::Unknown(1),
    };
    let Some(sequence) = bytes.get(..length) else {
        return Decoded::Incomplete;
    };
    match core::str::from_utf8(sequence).ok().and_then(|text| text.chars().next()) {
        Some(character) => Decoded::Press(Press::plain(Key::Character(character)), length),
        None => Decoded::Unknown(1),
    }
}

fn control_byte(byte: u8) -> Option<Press> {
    Some(match byte {
        b'\r' | b'\n' => Press::plain(Key::Enter),
        b'\t' => Press::plain(Key::Tab),
        0x7f | 0x08 => Press::plain(Key::Backspace),
        0x00 => Press::control(' '),
        0x01..=0x1a => Press::control((b'a' + byte - 1) as char),
        0x1c..=0x1f => Press::control((b'\\' + byte - 0x1c) as char),
        _ => return None,
    })
}

fn tilde_key(number: u16) -> Option<Key> {
    Some(match number {
        1 | 7 => Key::Home,
        2 => Key::Insert,
        3 => Key::Delete,
        4 | 8 => Key::End,
        5 => Key::PageUp,
        6 => Key::PageDown,
        11..=15 => Key::Function((number - 10) as u8),
        17..=21 => Key::Function((number - 11) as u8),
        23 | 24 => Key::Function((number - 12) as u8),
        _ => return None,
    })
}

fn final_key(byte: u8) -> Option<Key> {
    Some(match byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'P' => Key::Function(1),
        b'Q' => Key::Function(2),
        b'R' => Key::Function(3),
        b'S' => Key::Function(4),
        _ => return None,
    })
}

fn control_sequence(bytes: &[u8]) -> Decoded {
    let mut parameters = [0u16; MAX_PARAMETERS];
    let mut count = 0;
    let mut at = 2;
    loop {
        let Some(&byte) = bytes.get(at) else {
            return Decoded::Incomplete;
        };
        at += 1;
        match byte {
            b'0'..=b'9' => {
                let slot = &mut parameters[count.min(MAX_PARAMETERS - 1)];
                *slot = slot.saturating_mul(10).saturating_add(u16::from(byte - b'0'));
                count = count.max(1);
            }
            b';' => count = (count.max(1) + 1).min(MAX_PARAMETERS),
            0x20..=0x2f | b'<'..=b'?' => {}
            b'~' => {
                let key = tilde_key(parameters[0]);
                return match key {
                    Some(key) => Decoded::Press(Press::with_modifier(key, parameters[1]), at),
                    None => Decoded::Unknown(at),
                };
            }
            b'Z' => {
                let press = Press {
                    shift: true,
                    ..Press::plain(Key::Tab)
                };
                return Decoded::Press(press, at);
            }
            0x40..=0x7e => {
                return match final_key(byte) {
                    Some(key) => Decoded::Press(Press::with_modifier(key, parameters[1]), at),
                    None => Decoded::Unknown(at),
                };
            }
            _ => return Decoded::Unknown(at),
        }
    }
}

fn single_shift(bytes: &[u8]) -> Decoded {
    let Some(&byte) = bytes.get(2) else {
        return Decoded::Incomplete;
    };
    match final_key(byte) {
        Some(key) => Decoded::Press(Press::plain(key), 3),
        None => Decoded::Unknown(3),
    }
}

/// The key at the start of `bytes` (not empty).
pub fn decode(bytes: &[u8]) -> Decoded {
    let Some(&first) = bytes.first() else {
        return Decoded::Incomplete;
    };
    if first != ESC {
        return match control_byte(first) {
            Some(press) => Decoded::Press(press, 1),
            None => utf8(bytes),
        };
    }
    match bytes.get(1) {
        None => Decoded::Incomplete,
        Some(b'[') => control_sequence(bytes),
        Some(b'O') => single_shift(bytes),
        Some(&ESC) => Decoded::Press(Press::plain(Key::Escape), 1),
        Some(_) => match decode(&bytes[1..]) {
            Decoded::Press(press, used) => Decoded::Press(Press { alt: true, ..press }, used + 1),
            Decoded::Incomplete => Decoded::Incomplete,
            Decoded::Unknown(used) => Decoded::Unknown(used + 1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(bytes: &[u8]) -> Press {
        match decode(bytes) {
            Decoded::Press(press, used) => {
                assert_eq!(used, bytes.len(), "{bytes:?}");
                press
            }
            other => panic!("{bytes:?}: {other:?}"),
        }
    }

    #[test]
    fn text_controls_and_utf8_are_keys() {
        assert_eq!(press(b"a"), Press::plain(Key::Character('a')));
        assert_eq!(press("é".as_bytes()), Press::plain(Key::Character('é')));
        assert_eq!(press(b"\x13"), Press::control('s'));
        assert_eq!(press(b"\r"), Press::plain(Key::Enter));
        assert_eq!(press(b"\x7f"), Press::plain(Key::Backspace));
        assert_eq!(decode(&"é".as_bytes()[..1]), Decoded::Incomplete);
    }

    #[test]
    fn xterm_and_vt220_sequences_with_modifiers_decode() {
        assert_eq!(press(b"\x1b[A"), Press::plain(Key::Up));
        assert_eq!(press(b"\x1bOH"), Press::plain(Key::Home));
        assert_eq!(press(b"\x1b[3~"), Press::plain(Key::Delete));
        assert_eq!(press(b"\x1b[24~"), Press::plain(Key::Function(12)));
        let shifted = press(b"\x1b[1;2C");
        assert!(shifted.shift && !shifted.control && shifted.key == Key::Right);
        let control_end = press(b"\x1b[1;5F");
        assert!(control_end.control && control_end.key == Key::End);
        let shift_f3 = press(b"\x1b[1;2R");
        assert!(shift_f3.shift && shift_f3.key == Key::Function(3));
        assert!(press(b"\x1b[Z").shift);
        assert!(press(b"\x1bx").alt);
    }

    #[test]
    fn a_cut_sequence_waits_and_an_unknown_one_is_skipped() {
        assert_eq!(decode(b"\x1b"), Decoded::Incomplete);
        assert_eq!(decode(b"\x1b[1;"), Decoded::Incomplete);
        assert_eq!(decode(b"\x1b[99~x"), Decoded::Unknown(5));
        assert_eq!(decode(b"\x1b[200~"), Decoded::Unknown(6));
    }
}

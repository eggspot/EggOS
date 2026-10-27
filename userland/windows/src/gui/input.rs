//! The window server's event records (docs/windows.md, "Events") as Win32 messages: pointer
//! motion and buttons as `WM_MOUSEMOVE` and `WM_xBUTTONDOWN`/`UP` with the `MK_` flags and
//! client coordinates, keys as `WM_KEYDOWN`/`UP` (`WM_SYSKEYDOWN`/`UP` while Alt is held) with
//! Windows' virtual-key codes and the `lParam` bits Windows sets, and the rest as what the layer
//! does for the window.

use super::message::{self, point_parameter};

/// An event record's size in bytes.
pub const RECORD_SIZE: usize = 32;

/// The event kinds the layer reads.
pub mod kind {
    pub const POINTER_ENTER: u16 = 1;
    pub const POINTER_MOVE: u16 = 2;
    pub const POINTER_LEAVE: u16 = 3;
    pub const BUTTON: u16 = 4;
    pub const WHEEL: u16 = 5;
    pub const KEY: u16 = 6;
    pub const FOCUS_IN: u16 = 7;
    pub const FOCUS_OUT: u16 = 8;
    pub const CLOSE_REQUEST: u16 = 9;
    pub const RESIZE: u16 = 10;
    pub const STATE_CHANGED: u16 = 11;
}

const BUTTON_LEFT: u32 = 0x110;
const BUTTON_RIGHT: u32 = 0x111;
const BUTTON_MIDDLE: u32 = 0x112;
const HELD_LEFT: u32 = 1;
const HELD_RIGHT: u32 = 2;
const HELD_MIDDLE: u32 = 4;
const SHIFT: u32 = 1;
const CONTROL: u32 = 2;
const ALT: u32 = 4;

/// `wParam`'s `MK_` flags on mouse messages.
pub mod mouse_keys {
    pub const LEFT: usize = 0x1;
    pub const RIGHT: usize = 0x2;
    pub const SHIFT: usize = 0x4;
    pub const CONTROL: usize = 0x8;
    pub const MIDDLE: usize = 0x10;
}

const WHEEL_DELTA: i32 = 120;

/// One event record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Record {
    pub kind: u16,
    pub window: u16,
    pub time: u32,
    pub a: i32,
    pub b: i32,
    pub c: u32,
    pub d: u32,
}

impl Record {
    pub fn parse(bytes: &[u8; RECORD_SIZE]) -> Record {
        let word = |at: usize| {
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        Record {
            kind: u16::from_ne_bytes([bytes[0], bytes[1]]),
            window: u16::from_ne_bytes([bytes[2], bytes[3]]),
            time: word(4),
            a: word(8) as i32,
            b: word(12) as i32,
            c: word(16),
            d: word(20),
        }
    }
}

/// What a record means for its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Input to queue; `character` is what `TranslateMessage` makes of a key press.
    Input {
        message: u32,
        wparam: usize,
        lparam: usize,
        point: (i32, i32),
        character: Option<u32>,
    },
    Resize {
        width: u32,
        height: u32,
    },
    StateChanged(u32),
    CloseRequest,
    FocusIn,
    FocusOut,
    PointerLeft,
    Ignored,
}

/// The buttons and modifiers held, which mouse messages report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputState {
    buttons: u32,
    modifiers: u32,
    point: (i32, i32),
}

impl InputState {
    fn mouse_keys(&self) -> usize {
        let mut keys = 0;
        for (held, flag) in [
            (self.buttons & HELD_LEFT, mouse_keys::LEFT),
            (self.buttons & HELD_RIGHT, mouse_keys::RIGHT),
            (self.buttons & HELD_MIDDLE, mouse_keys::MIDDLE),
            (self.modifiers & SHIFT, mouse_keys::SHIFT),
            (self.modifiers & CONTROL, mouse_keys::CONTROL),
        ] {
            if held != 0 {
                keys |= flag;
            }
        }
        keys
    }

    fn mouse(&mut self, number: u32, x: i32, y: i32) -> Event {
        self.point = (x, y);
        Event::Input {
            message: number,
            wparam: self.mouse_keys(),
            lparam: point_parameter(x, y),
            point: (x, y),
            character: None,
        }
    }

    /// What `record` means; it also updates the buttons and modifiers held.
    pub fn translate(&mut self, record: &Record) -> Event {
        match record.kind {
            kind::POINTER_ENTER | kind::POINTER_MOVE => {
                if record.kind == kind::POINTER_MOVE {
                    self.buttons = record.c;
                }
                self.mouse(message::MOUSE_MOVE, record.a, record.b)
            }
            kind::POINTER_LEAVE => Event::PointerLeft,
            kind::BUTTON => {
                let pressed = record.d != 0;
                let (held, down, up) = match record.c {
                    BUTTON_LEFT => (
                        HELD_LEFT,
                        message::LEFT_BUTTON_DOWN,
                        message::LEFT_BUTTON_UP,
                    ),
                    BUTTON_RIGHT => (
                        HELD_RIGHT,
                        message::RIGHT_BUTTON_DOWN,
                        message::RIGHT_BUTTON_UP,
                    ),
                    BUTTON_MIDDLE => (
                        HELD_MIDDLE,
                        message::MIDDLE_BUTTON_DOWN,
                        message::MIDDLE_BUTTON_UP,
                    ),
                    _ => return Event::Ignored,
                };
                match pressed {
                    true => self.buttons |= held,
                    false => self.buttons &= !held,
                }
                self.mouse(if pressed { down } else { up }, record.a, record.b)
            }
            kind::WHEEL => {
                let delta = record.d as i32 * WHEEL_DELTA;
                Event::Input {
                    message: message::MOUSE_WHEEL,
                    wparam: (usize::from(delta as i16 as u16) << 16) | self.mouse_keys(),
                    lparam: point_parameter(record.a, record.b),
                    point: (record.a, record.b),
                    character: None,
                }
            }
            kind::KEY => self.key(record),
            kind::FOCUS_IN => Event::FocusIn,
            kind::FOCUS_OUT => {
                self.buttons = 0;
                Event::FocusOut
            }
            kind::CLOSE_REQUEST => Event::CloseRequest,
            kind::RESIZE => Event::Resize {
                width: record.a.max(0) as u32,
                height: record.b.max(0) as u32,
            },
            kind::STATE_CHANGED => Event::StateChanged(record.a.max(0) as u32),
            _ => Event::Ignored,
        }
    }

    fn key(&mut self, record: &Record) -> Event {
        let Ok(code) = u16::try_from(record.a) else {
            return Event::Ignored;
        };
        self.modifiers = record.c;
        let Some(key) = virtual_key(code) else {
            return Event::Ignored;
        };
        let pressed = record.b != 0;
        let alt = self.modifiers & ALT != 0;
        let number = match (pressed, alt) {
            (true, false) => message::KEY_DOWN,
            (false, false) => message::KEY_UP,
            (true, true) => message::SYS_KEY_DOWN,
            (false, true) => message::SYS_KEY_UP,
        };
        let mut lparam = 1 | (usize::from(key.scan) << 16);
        if key.extended {
            lparam |= 1 << 24;
        }
        if alt {
            lparam |= 1 << 29;
        }
        if !pressed {
            lparam |= (1 << 30) | (1 << 31);
        }
        Event::Input {
            message: number,
            wparam: usize::from(key.virtual_key),
            lparam,
            point: self.point,
            character: (pressed && record.d != 0).then_some(record.d),
        }
    }
}

/// A key as Windows names it: its virtual-key code, its scan code and whether it is one of the
/// extended keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualKey {
    pub virtual_key: u8,
    pub scan: u8,
    pub extended: bool,
}

const LETTER_ROWS: [(u16, &[u8]); 3] = [(16, b"QWERTYUIOP"), (30, b"ASDFGHJKL"), (44, b"ZXCVBNM")];

/// The virtual key of Linux key `code`, if Windows has one. Linux's codes 1 to 88 are the PC
/// keyboard's scan codes; the higher ones are the extended keys.
pub fn virtual_key(code: u16) -> Option<VirtualKey> {
    let plain = |virtual_key: u8| {
        Some(VirtualKey {
            virtual_key,
            scan: code as u8,
            extended: false,
        })
    };
    let extended = |virtual_key: u8, scan: u8| {
        Some(VirtualKey {
            virtual_key,
            scan,
            extended: true,
        })
    };
    for (first, letters) in LETTER_ROWS {
        if let Some(&letter) = code
            .checked_sub(first)
            .and_then(|index| letters.get(usize::from(index)))
        {
            return plain(letter);
        }
    }
    match code {
        1 => plain(0x1b),
        2..=10 => plain(b'1' + (code - 2) as u8),
        11 => plain(b'0'),
        12 => plain(0xbd),
        13 => plain(0xbb),
        14 => plain(0x08),
        15 => plain(0x09),
        26 => plain(0xdb),
        27 => plain(0xdd),
        28 => plain(0x0d),
        29 => plain(0x11),
        39 => plain(0xba),
        40 => plain(0xde),
        41 => plain(0xc0),
        42 | 54 => plain(0x10),
        43 => plain(0xdc),
        51 => plain(0xbc),
        52 => plain(0xbe),
        53 => plain(0xbf),
        55 => plain(0x6a),
        56 => plain(0x12),
        57 => plain(0x20),
        58 => plain(0x14),
        59..=68 => plain(0x70 + (code - 59) as u8),
        69 => plain(0x90),
        70 => plain(0x91),
        71 => plain(0x67),
        72 => plain(0x68),
        73 => plain(0x69),
        74 => plain(0x6d),
        75 => plain(0x64),
        76 => plain(0x65),
        77 => plain(0x66),
        78 => plain(0x6b),
        79 => plain(0x61),
        80 => plain(0x62),
        81 => plain(0x63),
        82 => plain(0x60),
        83 => plain(0x6e),
        87 => plain(0x7a),
        88 => plain(0x7b),
        96 => extended(0x0d, 0x1c),
        97 => extended(0x11, 0x1d),
        98 => extended(0x6f, 0x35),
        100 => extended(0x12, 0x38),
        102 => extended(0x24, 0x47),
        103 => extended(0x26, 0x48),
        104 => extended(0x21, 0x49),
        105 => extended(0x25, 0x4b),
        106 => extended(0x27, 0x4d),
        107 => extended(0x23, 0x4f),
        108 => extended(0x28, 0x50),
        109 => extended(0x22, 0x51),
        110 => extended(0x2d, 0x52),
        111 => extended(0x2e, 0x53),
        125 => extended(0x5b, 0x5b),
        126 => extended(0x5c, 0x5c),
        127 => extended(0x5d, 0x5d),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u16, a: i32, b: i32, c: u32, d: u32) -> Record {
        Record {
            kind,
            window: 3,
            time: 77,
            a,
            b,
            c,
            d,
        }
    }

    #[test]
    fn records_are_read_in_the_machines_byte_order() {
        let mut bytes = [0u8; RECORD_SIZE];
        bytes[0..2].copy_from_slice(&6u16.to_ne_bytes());
        bytes[2..4].copy_from_slice(&65u16.to_ne_bytes());
        bytes[4..8].copy_from_slice(&1234u32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&(-5i32).to_ne_bytes());
        bytes[12..16].copy_from_slice(&1i32.to_ne_bytes());
        bytes[16..20].copy_from_slice(&3u32.to_ne_bytes());
        bytes[20..24].copy_from_slice(&97u32.to_ne_bytes());
        assert_eq!(
            Record::parse(&bytes),
            Record {
                kind: 6,
                window: 65,
                time: 1234,
                a: -5,
                b: 1,
                c: 3,
                d: 97
            }
        );
    }

    #[test]
    fn pointer_records_become_mouse_messages_with_held_buttons() {
        let mut state = InputState::default();
        let input = |message, wparam, lparam, point| Event::Input {
            message,
            wparam,
            lparam,
            point,
            character: None,
        };
        assert_eq!(
            state.translate(&record(kind::POINTER_MOVE, 10, 20, 0, 0)),
            input(message::MOUSE_MOVE, 0, (20 << 16) | 10, (10, 20))
        );
        assert_eq!(
            state.translate(&record(kind::BUTTON, 10, 20, BUTTON_LEFT, 1)),
            input(
                message::LEFT_BUTTON_DOWN,
                mouse_keys::LEFT,
                (20 << 16) | 10,
                (10, 20)
            )
        );
        assert_eq!(
            state.translate(&record(kind::POINTER_MOVE, -3, 5, HELD_LEFT, 0)),
            input(
                message::MOUSE_MOVE,
                mouse_keys::LEFT,
                (5 << 16) | 0xfffd,
                (-3, 5)
            )
        );
        assert_eq!(
            state.translate(&record(kind::BUTTON, 1, 2, BUTTON_LEFT, 0)),
            input(message::LEFT_BUTTON_UP, 0, (2 << 16) | 1, (1, 2))
        );
        state.translate(&record(kind::KEY, 42, 1, SHIFT, 0));
        assert_eq!(
            state.translate(&record(kind::BUTTON, 1, 2, BUTTON_RIGHT, 1)),
            input(
                message::RIGHT_BUTTON_DOWN,
                mouse_keys::RIGHT | mouse_keys::SHIFT,
                (2 << 16) | 1,
                (1, 2)
            )
        );
        assert_eq!(
            state.translate(&record(kind::BUTTON, 1, 2, 0x113, 1)),
            Event::Ignored
        );
        let Event::Input { wparam, .. } = state.translate(&record(kind::WHEEL, 0, 0, 0, 1)) else {
            panic!("a wheel record is input");
        };
        assert_eq!(wparam >> 16, 120);
    }

    #[test]
    fn key_records_become_key_messages_with_windows_lparam_bits() {
        let mut state = InputState::default();
        assert_eq!(
            state.translate(&record(kind::KEY, 30, 1, 0, u32::from(b'a'))),
            Event::Input {
                message: message::KEY_DOWN,
                wparam: usize::from(b'A'),
                lparam: 1 | (30 << 16),
                point: (0, 0),
                character: Some(u32::from(b'a')),
            }
        );
        assert_eq!(
            state.translate(&record(kind::KEY, 30, 0, 0, u32::from(b'a'))),
            Event::Input {
                message: message::KEY_UP,
                wparam: usize::from(b'A'),
                lparam: 1 | (30 << 16) | (1 << 30) | (1 << 31),
                point: (0, 0),
                character: None,
            }
        );
        let Event::Input {
            message: number,
            lparam,
            ..
        } = state.translate(&record(kind::KEY, 106, 1, ALT, 0))
        else {
            panic!("a key record is input");
        };
        assert_eq!(number, message::SYS_KEY_DOWN);
        assert_eq!(lparam, 1 | (0x4d << 16) | (1 << 24) | (1 << 29));
        assert_eq!(
            state.translate(&record(kind::KEY, 240, 1, 0, 0)),
            Event::Ignored
        );
    }

    #[test]
    fn virtual_keys_follow_windows_codes() {
        let key = |code| virtual_key(code).map(|key| key.virtual_key);
        assert_eq!(key(16), Some(b'Q'));
        assert_eq!(key(38), Some(b'L'));
        assert_eq!(key(50), Some(b'M'));
        assert_eq!(key(2), Some(b'1'));
        assert_eq!(key(11), Some(b'0'));
        assert_eq!(key(57), Some(0x20));
        assert_eq!(key(28), Some(0x0d));
        assert_eq!(key(59), Some(0x70));
        assert_eq!(key(88), Some(0x7b));
        assert_eq!(key(103), Some(0x26));
        assert_eq!(key(84), None);
    }

    #[test]
    fn window_events_are_named() {
        let mut state = InputState::default();
        assert_eq!(
            state.translate(&record(kind::RESIZE, 500, 360, 0, 0)),
            Event::Resize {
                width: 500,
                height: 360
            }
        );
        assert_eq!(
            state.translate(&record(kind::STATE_CHANGED, 2, 0, 0, 0)),
            Event::StateChanged(2)
        );
        assert_eq!(
            state.translate(&record(kind::CLOSE_REQUEST, 0, 0, 0, 0)),
            Event::CloseRequest
        );
        assert_eq!(
            state.translate(&record(kind::FOCUS_IN, 0, 0, 0, 0)),
            Event::FocusIn
        );
        assert_eq!(
            state.translate(&record(kind::POINTER_LEAVE, 0, 0, 0, 0)),
            Event::PointerLeft
        );
        assert_eq!(state.translate(&record(13, 4, 0, 0, 0)), Event::Ignored);
    }
}

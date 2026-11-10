//! The window server's event records (docs/windows.md, "Events") as the toolkit reads them, and
//! the Linux key codes it acts on.

/// A record's size.
pub const RECORD_SIZE: usize = 32;

/// Linux key codes (`input-event-codes.h`) the toolkit's widgets answer.
pub mod key {
    pub const A: u16 = 30;
    pub const X: u16 = 45;
    pub const C: u16 = 46;
    pub const V: u16 = 47;
    pub const ESC: u16 = 1;
    pub const BACKSPACE: u16 = 14;
    pub const TAB: u16 = 15;
    pub const ENTER: u16 = 28;
    pub const SPACE: u16 = 57;
    pub const HOME: u16 = 102;
    pub const UP: u16 = 103;
    pub const PAGE_UP: u16 = 104;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const END: u16 = 107;
    pub const DOWN: u16 = 108;
    pub const PAGE_DOWN: u16 = 109;
    pub const DELETE: u16 = 111;
    pub const F10: u16 = 68;
    pub const LEFT_SHIFT: u16 = 42;
    pub const RIGHT_SHIFT: u16 = 54;
    pub const LEFT_ALT: u16 = 56;
    pub const RIGHT_ALT: u16 = 100;
    pub const LEFT_CTRL: u16 = 29;
    pub const RIGHT_CTRL: u16 = 97;
    pub const LEFT_SUPER: u16 = 125;
    pub const RIGHT_SUPER: u16 = 126;
    pub const MENU: u16 = 127;
    pub const Z: u16 = 44;
    pub const F: u16 = 33;
    pub const Q: u16 = 16;
    pub const CAPS_LOCK: u16 = 58;
    pub const NUM_LOCK: u16 = 69;
    pub const SCROLL_LOCK: u16 = 70;

    /// Whether key `code` is a modifier or a lock key, which leaves a dead key's accent
    /// waiting (D221).
    pub fn is_modifier_or_lock(code: u16) -> bool {
        matches!(
            code,
            LEFT_SHIFT
                | RIGHT_SHIFT
                | LEFT_ALT
                | RIGHT_ALT
                | LEFT_CTRL
                | RIGHT_CTRL
                | LEFT_SUPER
                | RIGHT_SUPER
                | CAPS_LOCK
                | NUM_LOCK
                | SCROLL_LOCK
        )
    }
}

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const ALT: u32 = 4;
    pub const SUPER: u32 = 8;
    /// The third-level key, AltGr, or Option in the macOS profile (D185): Alt held with it
    /// types a character instead of making a shortcut.
    pub const ALT_GRAPH: u32 = 32;
    /// A dead key's record (D221): its character is the accent waiting for the next key, or 0
    /// when Backspace took the accent back; [`decode`] makes it an [`Input::DeadKey`].
    pub const DEAD: u32 = 256;

    /// Whether Shift is held.
    pub fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    /// Whether Ctrl or Super is held: the shortcut modifier of every profile's text fields
    /// (docs/ux/toolkit-v0.4.md section 4.3).
    pub fn control_or_super(self) -> bool {
        self.0 & (Self::CTRL | Self::SUPER) != 0
    }

    /// Whether Ctrl or Super is held, or Alt without the third-level key.
    pub fn command(self) -> bool {
        self.0 & (Self::CTRL | Self::SUPER) != 0
            || (self.0 & Self::ALT != 0 && self.0 & Self::ALT_GRAPH == 0)
    }
}

/// The mouse button of a `Button` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// What a record says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The pointer is at (`x`, `y`) of the content: it entered or moved.
    PointerMove { x: i32, y: i32 },
    /// The pointer left the content.
    PointerLeave,
    /// A button was pressed or released at (`x`, `y`).
    Button {
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    },
    /// The wheel turned by `notches` (positive up) over (`x`, `y`).
    Wheel { x: i32, y: i32, notches: i32 },
    /// The wheel turned sideways by `notches` (positive right) over (`x`, `y`) (D162).
    SidewaysWheel { x: i32, y: i32, notches: i32 },
    /// A key was pressed or released; `character` is what it types under the US layout, or 0.
    Key {
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
        character: u32,
    },
    /// A dead key's accent waits for the next key (D221): `accent` is the spacing accent the
    /// caret shows highlighted until the next key resolves it, `None` when Backspace took it
    /// back. The keys that follow arrive as [`Input::Key`] with the finished characters.
    DeadKey { accent: Option<char> },
    /// The window got the keyboard focus.
    FocusIn,
    /// The window lost it.
    FocusOut,
    /// The user asked to close the window.
    CloseRequest,
    /// The server gave the window a content of `width` by `height`.
    Resize { width: u32, height: u32 },
    /// The window was minimised (1), maximised (2) or restored (0).
    StateChanged { state: u32 },
    /// Records were dropped.
    Overflow,
}

/// One record: the window it is for, when, and what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub window: u16,
    pub time_ms: u32,
    pub input: Input,
}

fn word(record: &[u8; RECORD_SIZE], offset: usize) -> u32 {
    u32::from_ne_bytes([
        record[offset],
        record[offset + 1],
        record[offset + 2],
        record[offset + 3],
    ])
}

/// The event `record` holds; `None` for a kind the toolkit does not know.
pub fn decode(record: &[u8; RECORD_SIZE]) -> Option<Event> {
    let kind = u16::from_ne_bytes([record[0], record[1]]);
    let window = u16::from_ne_bytes([record[2], record[3]]);
    let time_ms = word(record, 4);
    let (a, b, c, d) = (
        word(record, 8) as i32,
        word(record, 12) as i32,
        word(record, 16),
        word(record, 20),
    );
    let input = match kind {
        1 | 2 => Input::PointerMove { x: a, y: b },
        3 => Input::PointerLeave,
        4 => Input::Button {
            x: a,
            y: b,
            button: match c {
                0x110 => MouseButton::Left,
                0x111 => MouseButton::Right,
                _ => MouseButton::Middle,
            },
            pressed: d != 0,
        },
        5 if d == 0 && c != 0 => Input::SidewaysWheel {
            x: a,
            y: b,
            notches: c as i32,
        },
        5 => Input::Wheel {
            x: a,
            y: b,
            notches: d as i32,
        },
        6 if c & Modifiers::DEAD != 0 => Input::DeadKey {
            accent: char::from_u32(d).filter(|&accent| accent != '\0'),
        },
        6 => Input::Key {
            code: a as u16,
            pressed: b != 0,
            modifiers: Modifiers(c),
            character: d,
        },
        7 => Input::FocusIn,
        8 => Input::FocusOut,
        9 => Input::CloseRequest,
        10 => Input::Resize {
            width: a as u32,
            height: b as u32,
        },
        11 => Input::StateChanged { state: a as u32 },
        13 => Input::Overflow,
        _ => return None,
    };
    Some(Event {
        window,
        time_ms,
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u16, window: u16, values: [u32; 5]) -> [u8; RECORD_SIZE] {
        let mut record = [0u8; RECORD_SIZE];
        record[0..2].copy_from_slice(&kind.to_ne_bytes());
        record[2..4].copy_from_slice(&window.to_ne_bytes());
        for (index, value) in values.iter().enumerate() {
            record[4 + index * 4..8 + index * 4].copy_from_slice(&value.to_ne_bytes());
        }
        record
    }

    #[test]
    fn records_decode_at_the_contract_s_offsets() {
        assert_eq!(
            decode(&record(4, 3, [99, 10, 20, 0x110, 1])),
            Some(Event {
                window: 3,
                time_ms: 99,
                input: Input::Button {
                    x: 10,
                    y: 20,
                    button: MouseButton::Left,
                    pressed: true
                }
            })
        );
        assert_eq!(
            decode(&record(6, 1, [0, 30, 1, 1, 0x41])).map(|event| event.input),
            Some(Input::Key {
                code: 30,
                pressed: true,
                modifiers: Modifiers(1),
                character: 0x41
            })
        );
        assert_eq!(
            decode(&record(5, 1, [0, 4, 5, 0, (-2i32) as u32])).map(|event| event.input),
            Some(Input::Wheel {
                x: 4,
                y: 5,
                notches: -2
            })
        );
        assert_eq!(
            decode(&record(5, 1, [0, 4, 5, (-3i32) as u32, 0])).map(|event| event.input),
            Some(Input::SidewaysWheel {
                x: 4,
                y: 5,
                notches: -3
            })
        );
        assert_eq!(
            decode(&record(2, 1, [0, (-3i32) as u32, 7, 1, 0])).map(|event| event.input),
            Some(Input::PointerMove { x: -3, y: 7 })
        );
        assert_eq!(
            decode(&record(10, 1, [0, 640, 480, 0, 0])).map(|event| event.input),
            Some(Input::Resize {
                width: 640,
                height: 480
            })
        );
        assert_eq!(decode(&record(12, 1, [0; 5])), None);
        assert_eq!(decode(&record(99, 1, [0; 5])), None);
    }

    #[test]
    fn a_dead_key_s_record_decodes_as_its_accent_and_backspace_s_as_none() {
        let dead = Modifiers::DEAD | Modifiers::SHIFT;
        assert_eq!(
            decode(&record(6, 1, [0, 13, 1, dead, u32::from('`')])).map(|event| event.input),
            Some(Input::DeadKey { accent: Some('`') })
        );
        assert_eq!(
            decode(&record(6, 1, [0, 0, 1, Modifiers::DEAD, 0])).map(|event| event.input),
            Some(Input::DeadKey { accent: None })
        );
        assert_eq!(
            decode(&record(6, 1, [0, 13, 0, 0, 0])).map(|event| event.input),
            Some(Input::Key {
                code: 13,
                pressed: false,
                modifiers: Modifiers(0),
                character: 0
            })
        );
    }
}

//! The US English keyboard layout, the one the layer's keyboard is (D178): the character a
//! virtual key types with Shift, Ctrl, Alt and Caps Lock (`ToUnicodeEx`), the key and modifiers
//! that type a character (`VkKeyScanExW`), the scan code of a virtual key and back
//! (`MapVirtualKeyW`), and the `lParam` of a key message a program sends itself.

use super::input::virtual_key;

/// The layout's handle (`HKL`), as `GetKeyboardLayout` gives it.
pub const LAYOUT: usize = 0x0409_0409;
/// The layout's name, as `GetKeyboardLayoutNameW` gives it.
pub const LAYOUT_NAME: &str = "00000409";

/// The modifiers [`key_for`] says a character needs, as `VkKeyScan`'s high byte holds them.
pub mod shift_state {
    pub const SHIFT: u16 = 0x1;
    pub const CONTROL: u16 = 0x2;
}

/// `MapVirtualKeyW`'s kinds of mapping.
pub mod mapping {
    pub const KEY_TO_SCAN: u32 = 0;
    pub const SCAN_TO_KEY: u32 = 1;
    pub const KEY_TO_CHARACTER: u32 = 2;
    pub const SCAN_TO_SIDED_KEY: u32 = 3;
    pub const KEY_TO_EXTENDED_SCAN: u32 = 4;
}

const VK_BACK: u8 = 0x08;
const VK_TAB: u8 = 0x09;
const VK_RETURN: u8 = 0x0d;
const VK_ESCAPE: u8 = 0x1b;
const VK_SPACE: u8 = 0x20;
const VK_NUMPAD0: u8 = 0x60;
const VK_NUMPAD9: u8 = 0x69;
const VK_SEPARATOR: u8 = 0x6c;
const VK_DIVIDE: u8 = 0x6f;
const EXTENDED: u16 = 0xe000;
const FIRST_EXTENDED_CODE: u16 = 96;
const LAST_EXTENDED_CODE: u16 = 127;
const LAST_PLAIN_CODE: u16 = 88;
const SHIFTED_DIGITS: &[u8; 10] = b")!@#$%^&*(";
const PUNCTUATION: [(u8, u8, u8); 11] = [
    (0xba, b';', b':'),
    (0xbb, b'=', b'+'),
    (0xbc, b',', b'<'),
    (0xbd, b'-', b'_'),
    (0xbe, b'.', b'>'),
    (0xbf, b'/', b'?'),
    (0xc0, b'`', b'~'),
    (0xdb, b'[', b'{'),
    (0xdc, b'\\', b'|'),
    (0xdd, b']', b'}'),
    (0xde, b'\'', b'"'),
];
const SIDED: [(u8, u16); 6] = [
    (0xa0, 0x2a),
    (0xa1, 0x36),
    (0xa2, 0x1d),
    (0xa3, EXTENDED | 0x1d),
    (0xa4, 0x38),
    (0xa5, EXTENDED | 0x38),
];

/// The modifiers held while a key is pressed, for [`character`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub caps_lock: bool,
}

/// The character virtual key `key` types with `modifiers` (`ToUnicodeEx`); Ctrl makes the
/// control characters, and Ctrl with Alt types nothing on this layout.
pub fn character(key: u8, modifiers: Modifiers) -> Option<u16> {
    if modifiers.control {
        return match modifiers.alt {
            true => None,
            false => control_character(key, modifiers.shift),
        };
    }
    let shift = modifiers.shift;
    let typed = match key {
        b'A'..=b'Z' if shift != modifiers.caps_lock => key,
        b'A'..=b'Z' => key + (b'a' - b'A'),
        b'0'..=b'9' if shift => SHIFTED_DIGITS[usize::from(key - b'0')],
        b'0'..=b'9' => key,
        VK_SPACE => b' ',
        VK_RETURN => b'\r',
        VK_TAB => b'\t',
        VK_BACK => VK_BACK,
        VK_ESCAPE => VK_ESCAPE,
        VK_NUMPAD0..=VK_NUMPAD9 => b'0' + (key - VK_NUMPAD0),
        0x6a => b'*',
        0x6b => b'+',
        0x6d => b'-',
        0x6e => b'.',
        VK_DIVIDE => b'/',
        _ => {
            let &(_, plain, shifted) = PUNCTUATION.iter().find(|entry| entry.0 == key)?;
            match shift {
                true => shifted,
                false => plain,
            }
        }
    };
    Some(u16::from(typed))
}

fn control_character(key: u8, shift: bool) -> Option<u16> {
    let typed = match (key, shift) {
        (b'A'..=b'Z', _) => key - b'A' + 1,
        (0xdb, false) => 0x1b,
        (0xdc, false) => 0x1c,
        (0xdd, false) => 0x1d,
        (b'6', true) => 0x1e,
        (0xbd, true) => 0x1f,
        (VK_BACK, _) => 0x7f,
        (VK_RETURN, _) => b'\n',
        (VK_SPACE, _) => b' ',
        (VK_ESCAPE, _) => VK_ESCAPE,
        _ => return None,
    };
    Some(u16::from(typed))
}

fn search(keys: impl Iterator<Item = u8> + Clone, unit: u16) -> Option<u16> {
    let states = [
        0,
        shift_state::SHIFT,
        shift_state::CONTROL,
        shift_state::CONTROL | shift_state::SHIFT,
    ];
    states.into_iter().find_map(|state| {
        let modifiers = Modifiers {
            shift: state & shift_state::SHIFT != 0,
            control: state & shift_state::CONTROL != 0,
            ..Modifiers::default()
        };
        keys.clone()
            .find(|&key| character(key, modifiers) == Some(unit))
            .map(|key| (state << 8) | u16::from(key))
    })
}

/// The virtual key that types `unit` in the low byte and the [`shift_state`] it needs in the
/// high byte (`VkKeyScanExW`), the editing keys before the letters (so Ctrl+Enter types a line
/// feed, not Ctrl+J) and the main keys before the numeric keypad's; `None` when no key types
/// it.
pub fn key_for(unit: u16) -> Option<u16> {
    let main = [VK_SPACE, VK_RETURN, VK_TAB, VK_BACK, VK_ESCAPE]
        .into_iter()
        .chain(b'0'..=b'9')
        .chain(b'A'..=b'Z')
        .chain(PUNCTUATION.iter().map(|entry| entry.0));
    let keypad = (VK_NUMPAD0..=VK_DIVIDE).filter(|&key| key != VK_SEPARATOR);
    search(main, unit).or_else(|| search(keypad, unit))
}

/// The set 1 scan code of virtual key `key`, with `0xe0` in the high byte for an extended key;
/// Shift, Ctrl and Alt are their left keys unless named by side.
pub fn scan_of(key: u8) -> Option<u16> {
    if let Some(&(_, scan)) = SIDED.iter().find(|entry| entry.0 == key) {
        return Some(scan);
    }
    let plain = (1..=LAST_PLAIN_CODE).find(|&code| {
        virtual_key(code).is_some_and(|found| found.virtual_key == key && !found.extended)
    });
    if let Some(code) = plain {
        return Some(code);
    }
    (FIRST_EXTENDED_CODE..=LAST_EXTENDED_CODE).find_map(|code| {
        let found = virtual_key(code)?;
        (found.virtual_key == key && found.extended).then_some(EXTENDED | u16::from(found.scan))
    })
}

/// The virtual key of set 1 scan code `scan` (`0xe0` in the high byte for an extended key),
/// Shift, Ctrl and Alt not told apart by side.
pub fn key_of_scan(scan: u16) -> Option<u8> {
    let low = scan & 0xff;
    match scan >> 8 {
        0 => (1..=LAST_PLAIN_CODE)
            .contains(&low)
            .then(|| virtual_key(low))
            .flatten()
            .map(|found| found.virtual_key),
        0xe0 => (FIRST_EXTENDED_CODE..=LAST_EXTENDED_CODE).find_map(|code| {
            let found = virtual_key(code)?;
            (u16::from(found.scan) == low).then_some(found.virtual_key)
        }),
        _ => None,
    }
}

/// `MapVirtualKeyW(code, kind)`: 0 when there is no mapping.
pub fn map_virtual_key(code: u32, kind: u32) -> u32 {
    let key = u8::try_from(code).ok();
    let scan = u16::try_from(code).ok();
    let mapped = match kind {
        mapping::KEY_TO_SCAN => key.and_then(scan_of).map(|scan| u32::from(scan & 0xff)),
        mapping::KEY_TO_EXTENDED_SCAN => key.and_then(scan_of).map(u32::from),
        mapping::SCAN_TO_KEY => scan.and_then(key_of_scan).map(u32::from),
        mapping::SCAN_TO_SIDED_KEY => scan.and_then(|scan| {
            SIDED
                .iter()
                .find(|entry| entry.1 == scan)
                .map(|entry| u32::from(entry.0))
                .or_else(|| key_of_scan(scan).map(u32::from))
        }),
        mapping::KEY_TO_CHARACTER => key.and_then(|key| {
            let upper = Modifiers {
                caps_lock: true,
                ..Modifiers::default()
            };
            character(key, upper).map(u32::from)
        }),
        _ => None,
    };
    mapped.unwrap_or(0)
}

/// The `lParam` of a key message for scan code `scan` (`0xe0` in its high byte for an extended
/// key): a repeat count of 1, the scan code, the extended and Alt bits, and for a release the
/// previous-state and transition bits.
pub fn key_lparam(scan: u16, released: bool, alt: bool) -> usize {
    let mut lparam = 1 | (usize::from(scan & 0xff) << 16);
    if scan >> 8 == 0xe0 {
        lparam |= 1 << 24;
    }
    if alt {
        lparam |= 1 << 29;
    }
    if released {
        lparam |= (1 << 30) | (1 << 31);
    }
    lparam
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(key: u8, shift: bool, control: bool) -> Option<char> {
        let modifiers = Modifiers {
            shift,
            control,
            ..Modifiers::default()
        };
        character(key, modifiers).and_then(|unit| char::from_u32(u32::from(unit)))
    }

    #[test]
    fn keys_type_the_us_layouts_characters() {
        assert_eq!(typed(b'A', false, false), Some('a'));
        assert_eq!(typed(b'A', true, false), Some('A'));
        let caps = Modifiers {
            caps_lock: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(character(b'Q', caps), Some(u16::from(b'q')));
        assert_eq!(typed(b'2', true, false), Some('@'));
        assert_eq!(typed(b'0', true, false), Some(')'));
        assert_eq!(typed(0xde, true, false), Some('"'));
        assert_eq!(typed(0xc0, false, false), Some('`'));
        assert_eq!(typed(0x6b, false, false), Some('+'));
        assert_eq!(typed(VK_RETURN, false, false), Some('\r'));
        assert_eq!(typed(b'C', false, true), Some('\u{3}'));
        assert_eq!(typed(0xdb, false, true), Some('\u{1b}'));
        assert_eq!(typed(VK_RETURN, false, true), Some('\n'));
        assert_eq!(typed(0x70, false, false), None);
        let alt_gr = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(character(b'E', alt_gr), None);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(character(b'F', alt), Some(u16::from(b'f')));
    }

    #[test]
    fn characters_are_found_on_their_keys_main_block_first() {
        let unit = |c: char| c as u16;
        assert_eq!(key_for(unit('a')), Some(0x41));
        assert_eq!(key_for(unit('A')), Some(0x0141));
        assert_eq!(key_for(unit('1')), Some(0x31));
        assert_eq!(key_for(unit('*')), Some(0x0138));
        assert_eq!(key_for(unit('+')), Some(0x01bb));
        assert_eq!(key_for(unit('?')), Some(0x01bf));
        assert_eq!(key_for(unit('\r')), Some(0x0d));
        assert_eq!(key_for(unit('\n')), Some(0x020d));
        assert_eq!(key_for(1), Some(0x0241));
        assert_eq!(key_for(unit('é')), None);
    }

    #[test]
    fn scan_codes_map_both_ways_and_by_side() {
        assert_eq!(scan_of(b'A'), Some(0x1e));
        assert_eq!(scan_of(0x10), Some(0x2a));
        assert_eq!(scan_of(0xa3), Some(0xe01d));
        assert_eq!(scan_of(0x25), Some(0xe04b));
        assert_eq!(scan_of(0x0d), Some(0x1c));
        assert_eq!(key_of_scan(0x1e), Some(b'A'));
        assert_eq!(key_of_scan(0xe04b), Some(0x25));
        assert_eq!(key_of_scan(0x36), Some(0x10));
        assert_eq!(key_of_scan(0x7f), None);
        assert_eq!(map_virtual_key(0x41, mapping::KEY_TO_SCAN), 0x1e);
        assert_eq!(map_virtual_key(0x25, mapping::KEY_TO_SCAN), 0x4b);
        assert_eq!(map_virtual_key(0x25, mapping::KEY_TO_EXTENDED_SCAN), 0xe04b);
        assert_eq!(map_virtual_key(0x1e, mapping::SCAN_TO_KEY), 0x41);
        assert_eq!(map_virtual_key(0x36, mapping::SCAN_TO_SIDED_KEY), 0xa1);
        assert_eq!(map_virtual_key(0x41, mapping::KEY_TO_CHARACTER), 0x41);
        assert_eq!(map_virtual_key(0xbd, mapping::KEY_TO_CHARACTER), 0x2d);
        assert_eq!(map_virtual_key(0x41, 9), 0);
        assert_eq!(map_virtual_key(0x1_0000, mapping::KEY_TO_SCAN), 0);
    }

    #[test]
    fn key_messages_carry_windows_lparam_bits() {
        assert_eq!(key_lparam(0x1e, false, false), 1 | (0x1e << 16));
        assert_eq!(
            key_lparam(0xe04b, true, true),
            1 | (0x4b << 16) | (1 << 24) | (1 << 29) | (1 << 30) | (1 << 31)
        );
    }
}

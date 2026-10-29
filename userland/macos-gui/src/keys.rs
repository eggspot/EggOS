//! The window server's key records (docs/windows.md, "Events": a Linux key code, the modifier
//! bits and the character under the US keymap) as `NSEvent` reports a key: its virtual key code
//! (`kVK_*`, Apple's documented numbers), its `characters` (Unicode, with AppKit's function-key
//! characters for the keys that type nothing) and its modifier flags.

/// `NSEventModifierFlags` bits.
pub mod modifier {
    pub const CAPS_LOCK: u64 = 1 << 16;
    pub const SHIFT: u64 = 1 << 17;
    pub const CONTROL: u64 = 1 << 18;
    pub const OPTION: u64 = 1 << 19;
    pub const COMMAND: u64 = 1 << 20;
    /// The four a key equivalent's mask can hold.
    pub const KEY_EQUIVALENT: u64 = SHIFT | CONTROL | OPTION | COMMAND;
}

/// AppKit's characters for keys that type nothing (`NSUpArrowFunctionKey` and its kin).
pub mod function {
    pub const UP: u32 = 0xf700;
    pub const DOWN: u32 = 0xf701;
    pub const LEFT: u32 = 0xf702;
    pub const RIGHT: u32 = 0xf703;
    pub const F1: u32 = 0xf704;
    pub const F12: u32 = 0xf70f;
    pub const INSERT: u32 = 0xf727;
    pub const DELETE: u32 = 0xf728;
    pub const HOME: u32 = 0xf729;
    pub const END: u32 = 0xf72b;
    pub const PAGE_UP: u32 = 0xf72c;
    pub const PAGE_DOWN: u32 = 0xf72d;
}

/// The characters AppKit gives the keys that type control characters.
pub mod control {
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0d;
    pub const ESCAPE: u32 = 0x1b;
    pub const BACKSPACE: u32 = 0x7f;
}

const SERVER_SHIFT: u32 = 1;
const SERVER_CONTROL: u32 = 2;
const SERVER_ALT: u32 = 4;
const SERVER_SUPER: u32 = 8;
const SERVER_CAPS_LOCK: u32 = 16;

/// The modifier flags of a key record's modifier bits: Super is Command, Alt is Option.
pub fn modifier_flags(bits: u32) -> u64 {
    [
        (SERVER_SHIFT, modifier::SHIFT),
        (SERVER_CONTROL, modifier::CONTROL),
        (SERVER_ALT, modifier::OPTION),
        (SERVER_SUPER, modifier::COMMAND),
        (SERVER_CAPS_LOCK, modifier::CAPS_LOCK),
    ]
    .iter()
    .filter(|(bit, _)| bits & bit != 0)
    .fold(0, |flags, (_, flag)| flags | flag)
}

const LETTERS: [(u16, u8, u16); 26] = [
    (30, b'a', 0x00),
    (48, b'b', 0x0b),
    (46, b'c', 0x08),
    (32, b'd', 0x02),
    (18, b'e', 0x0e),
    (33, b'f', 0x03),
    (34, b'g', 0x05),
    (35, b'h', 0x04),
    (23, b'i', 0x22),
    (36, b'j', 0x26),
    (37, b'k', 0x28),
    (38, b'l', 0x25),
    (50, b'm', 0x2e),
    (49, b'n', 0x2d),
    (24, b'o', 0x1f),
    (25, b'p', 0x23),
    (16, b'q', 0x0c),
    (19, b'r', 0x0f),
    (31, b's', 0x01),
    (20, b't', 0x11),
    (22, b'u', 0x20),
    (47, b'v', 0x09),
    (17, b'w', 0x0d),
    (45, b'x', 0x07),
    (21, b'y', 0x10),
    (44, b'z', 0x06),
];

const DIGIT_KEY_CODES: [u16; 10] = [0x1d, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19];
const FUNCTION_KEY_CODES: [u16; 12] = [
    0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f,
];

fn letter(code: u16) -> Option<(u8, u16)> {
    LETTERS
        .iter()
        .find(|(linux, _, _)| *linux == code)
        .map(|&(_, letter, mac)| (letter, mac))
}

fn function_key_index(code: u16) -> Option<usize> {
    match code {
        59..=68 => Some(usize::from(code - 59)),
        87 => Some(10),
        88 => Some(11),
        _ => None,
    }
}

/// The virtual key code (`keyCode`) of Linux key `code`, if the Mac keyboard has the key.
pub fn key_code(code: u16) -> Option<u16> {
    if let Some((_, mac)) = letter(code) {
        return Some(mac);
    }
    if let Some(index) = function_key_index(code) {
        return Some(FUNCTION_KEY_CODES[index]);
    }
    Some(match code {
        2..=10 => DIGIT_KEY_CODES[usize::from(code - 1)],
        11 => DIGIT_KEY_CODES[0],
        1 => 0x35,
        12 => 0x1b,
        13 => 0x18,
        14 => 0x33,
        15 => 0x30,
        26 => 0x21,
        27 => 0x1e,
        28 | 96 => 0x24,
        29 | 97 => 0x3b,
        39 => 0x29,
        40 => 0x27,
        41 => 0x32,
        42 => 0x38,
        43 => 0x2a,
        51 => 0x2b,
        52 => 0x2f,
        53 => 0x2c,
        54 => 0x3c,
        56 => 0x3a,
        57 => 0x31,
        58 => 0x39,
        100 => 0x3d,
        102 => 0x73,
        103 => 0x7e,
        104 => 0x74,
        105 => 0x7b,
        106 => 0x7c,
        107 => 0x77,
        108 => 0x7d,
        109 => 0x79,
        110 => 0x72,
        111 => 0x75,
        125 => 0x37,
        126 => 0x36,
        _ => return None,
    })
}

fn special_character(code: u16) -> Option<u32> {
    if let Some(index) = function_key_index(code) {
        return Some(function::F1 + index as u32);
    }
    Some(match code {
        1 => control::ESCAPE,
        14 => control::BACKSPACE,
        15 => control::TAB,
        28 | 96 => control::RETURN,
        102 => function::HOME,
        103 => function::UP,
        104 => function::PAGE_UP,
        105 => function::LEFT,
        106 => function::RIGHT,
        107 => function::END,
        108 => function::DOWN,
        109 => function::PAGE_DOWN,
        110 => function::INSERT,
        111 => function::DELETE,
        _ => return None,
    })
}

/// What `characters` holds for Linux key `code` typing `character` (the record's, 0 for none):
/// the function-key and control characters for the keys that have them, else the character.
pub fn characters(code: u16, character: u32) -> Option<u32> {
    special_character(code).or((character != 0).then_some(character))
}

/// What `charactersIgnoringModifiers` holds: as [`characters`], except that a letter typed
/// with Control is the letter itself (Shift still counts, as AppKit's does).
pub fn characters_ignoring_modifiers(code: u16, character: u32, flags: u64) -> Option<u32> {
    if let Some(special) = special_character(code) {
        return Some(special);
    }
    if let Some((letter, _)) = letter(code) {
        let shifted = flags & (modifier::SHIFT | modifier::CAPS_LOCK) != 0;
        return Some(u32::from(if shifted {
            letter.to_ascii_uppercase()
        } else {
            letter
        }));
    }
    (character != 0).then_some(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_bits_become_appkit_flags_super_as_command() {
        assert_eq!(modifier_flags(0), 0);
        assert_eq!(modifier_flags(SERVER_SUPER), modifier::COMMAND);
        assert_eq!(
            modifier_flags(SERVER_SHIFT | SERVER_ALT | SERVER_CONTROL),
            modifier::SHIFT | modifier::OPTION | modifier::CONTROL
        );
        assert_eq!(modifier_flags(SERVER_CAPS_LOCK), modifier::CAPS_LOCK);
    }

    #[test]
    fn key_codes_follow_apples_virtual_key_numbers() {
        assert_eq!(key_code(30), Some(0x00));
        assert_eq!(key_code(16), Some(0x0c));
        assert_eq!(key_code(46), Some(0x08));
        assert_eq!(key_code(2), Some(0x12));
        assert_eq!(key_code(11), Some(0x1d));
        assert_eq!(key_code(10), Some(0x19));
        assert_eq!(key_code(28), Some(0x24));
        assert_eq!(key_code(1), Some(0x35));
        assert_eq!(key_code(59), Some(0x7a));
        assert_eq!(key_code(88), Some(0x6f));
        assert_eq!(key_code(103), Some(0x7e));
        assert_eq!(key_code(125), Some(0x37));
        assert_eq!(key_code(240), None);
    }

    #[test]
    fn characters_name_keys_that_type_nothing() {
        assert_eq!(characters(30, u32::from(b'a')), Some(u32::from(b'a')));
        assert_eq!(characters(28, 13), Some(control::RETURN));
        assert_eq!(characters(14, 8), Some(control::BACKSPACE));
        assert_eq!(characters(103, 0), Some(function::UP));
        assert_eq!(characters(88, 0), Some(function::F12));
        assert_eq!(characters(42, 0), None);
        assert_eq!(
            characters_ignoring_modifiers(16, 0x11, modifier::CONTROL),
            Some(u32::from(b'q'))
        );
        assert_eq!(
            characters_ignoring_modifiers(16, u32::from(b'Q'), modifier::SHIFT),
            Some(u32::from(b'Q'))
        );
        assert_eq!(
            characters_ignoring_modifiers(2, u32::from(b'1'), 0),
            Some(u32::from(b'1'))
        );
    }
}

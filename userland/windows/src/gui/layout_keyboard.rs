//! The keyboard layout EggOS types with, for the Win32 calls that ask what keys type (D220):
//! `ToUnicodeEx` ([`to_unicode`]), `VkKeyScanExW` ([`key_for`]) and `MapVirtualKeyW`'s
//! character mapping ([`key_character`]) answer for the layout in force
//! ([`eggos_drivers::input::layout`]), its dead keys and third level included. AltGr is Ctrl
//! with Alt, as on Windows. The keys keep the US virtual-key codes of their places.

use super::input::virtual_key;
use super::keyboard::{self, Modifiers, shift_state};
use eggos_drivers::input::keymap::Modifiers as Held;
use eggos_drivers::input::layout::{Press, Typist};

/// `VkKeyScan`'s Alt bit in the shift state.
pub const ALT_STATE: u16 = 0x4;

/// The top bit `MapVirtualKeyW` sets on a dead key's character.
pub const DEAD_KEY_BIT: u32 = 0x8000_0000;

const FIRST_CODE: u16 = 1;
const LAST_PLAIN_CODE: u16 = 88;

/// What `ToUnicodeEx` makes of a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Translated {
    /// No character: `ToUnicodeEx` returns 0.
    Nothing,
    /// A dead key, whose spacing accent this is: `ToUnicodeEx` returns -1.
    Dead(u16),
    /// The first `count` of `units`, UTF-16: `ToUnicodeEx` returns the count.
    Typed { units: [u16; 4], count: u8 },
}

impl Translated {
    fn of(characters: &[char]) -> Self {
        let mut units = [0u16; 4];
        let mut count = 0;
        for character in characters {
            let mut encoded = [0u16; 2];
            for &unit in character.encode_utf16(&mut encoded).iter() {
                if count < units.len() {
                    units[count] = unit;
                    count += 1;
                }
            }
        }
        match count {
            0 => Self::Nothing,
            _ => Self::Typed {
                units,
                count: count as u8,
            },
        }
    }
}

/// The Linux key code of the key virtual key `key` names, the main keys before the extended
/// ones.
pub fn linux_code(key: u8) -> Option<u16> {
    (FIRST_CODE..=LAST_PLAIN_CODE)
        .find(|&code| virtual_key(code).is_some_and(|found| found.virtual_key == key))
}

fn held(modifiers: Modifiers) -> Held {
    let mut held = Held::NONE;
    if modifiers.shift {
        held = held.with(Held::SHIFT);
    }
    if modifiers.caps_lock {
        held = held.with(Held::CAPS_LOCK);
    }
    match (modifiers.control, modifiers.alt) {
        (true, true) => held.with(Held::ALT_GRAPH),
        (true, false) => held.with(Held::CTRL),
        (false, true) => held.with(Held::ALT),
        (false, false) => held,
    }
}

fn units(unit: Option<u16>) -> Translated {
    match unit {
        Some(unit) => Translated::Typed {
            units: [unit, 0, 0, 0],
            count: 1,
        },
        None => Translated::Nothing,
    }
}

/// What pressing virtual key `key` with `modifiers` types on `typist`'s layout, its waiting
/// dead key taken into account and changed as the press changes it: Ctrl alone makes the
/// control characters, Ctrl with Alt reaches the third level, a dead key waits.
pub fn to_unicode(typist: &mut Typist, key: u8, modifiers: Modifiers) -> Translated {
    if modifiers.control && !modifiers.alt {
        return units(keyboard::character(key, modifiers));
    }
    let Some(code) = linux_code(key) else {
        return units(keyboard::character(key, modifiers));
    };
    match typist.press(code, held(modifiers)) {
        Press::Typed { typed, .. } => match typed.characters() {
            [] => Translated::Nothing,
            characters => Translated::of(characters),
        },
        Press::Dead => typist
            .pending_accent()
            .and_then(|accent| u16::try_from(u32::from(accent)).ok())
            .map_or(Translated::Nothing, Translated::Dead),
        Press::Cancelled => Translated::Nothing,
        Press::Shortcut(_) | Press::Other => units(keyboard::character(
            key,
            Modifiers {
                control: false,
                alt: false,
                ..modifiers
            },
        )),
    }
}

const STATES: [u16; 4] = [
    0,
    shift_state::SHIFT,
    shift_state::CONTROL | ALT_STATE,
    shift_state::CONTROL | ALT_STATE | shift_state::SHIFT,
];

/// The virtual key that types `unit` on `typist`'s layout in the low byte and the shift state
/// it needs in the high byte (`VkKeyScanExW`: 1 Shift, 2 Ctrl, 4 Alt; AltGr is Ctrl with Alt),
/// plain and Shift before AltGr; the US layout's answer for the keys every layout shares
/// (Enter, Tab, the control characters); `None` when no key types it.
pub fn key_for(typist: &Typist, unit: u16) -> Option<u16> {
    let layout_key = STATES.into_iter().find_map(|state| {
        let modifiers = Modifiers {
            shift: state & shift_state::SHIFT != 0,
            control: state & shift_state::CONTROL != 0,
            alt: state & ALT_STATE != 0,
            caps_lock: false,
        };
        (FIRST_CODE..=LAST_PLAIN_CODE).find_map(|code| {
            let key = virtual_key(code)?.virtual_key;
            let typed = typist.character_of(code, held(modifiers));
            (typed != 0 && typed == u32::from(unit)).then_some(state << 8 | u16::from(key))
        })
    });
    layout_key.or_else(|| keyboard::key_for(unit))
}

/// The character virtual key `key` types alone on `typist`'s layout (`MapVirtualKeyW`'s
/// `MAPVK_VK_TO_CHAR`): a dead key's spacing accent with [`DEAD_KEY_BIT`], 0 for none.
pub fn key_character(typist: &Typist, key: u8) -> u32 {
    let Some(code) = linux_code(key) else {
        return keyboard::map_virtual_key(u32::from(key), keyboard::mapping::KEY_TO_CHARACTER);
    };
    let mut trial = *typist;
    match to_unicode(&mut trial, key, Modifiers::default()) {
        Translated::Dead(accent) => u32::from(accent) | DEAD_KEY_BIT,
        _ => match typist.character_of(code, Held::NONE) {
            0 => keyboard::map_virtual_key(u32::from(key), keyboard::mapping::KEY_TO_CHARACTER),
            character => character,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_drivers::input::layout;

    fn typist(name: &str) -> Typist {
        let mut typist = Typist::new();
        typist.set_layout(layout::named(name.as_bytes()).unwrap());
        typist
    }

    fn typed(characters: &str) -> Translated {
        let characters: Vec<char> = characters.chars().collect();
        Translated::of(&characters)
    }

    const PLAIN: Modifiers = Modifiers {
        shift: false,
        control: false,
        alt: false,
        caps_lock: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..PLAIN
    };
    const ALT_GR: Modifiers = Modifiers {
        control: true,
        alt: true,
        ..PLAIN
    };
    const VK_OEM_6: u8 = 0xdd;
    const VK_OEM_PLUS: u8 = 0xbb;

    #[test]
    fn to_unicode_types_the_layout_s_dead_keys_and_altgr() {
        let mut german = typist("de");
        assert_eq!(to_unicode(&mut german, VK_OEM_PLUS, PLAIN), Translated::Dead(0xb4));
        assert_eq!(to_unicode(&mut german, b'E', PLAIN), typed("é"));
        assert_eq!(to_unicode(&mut german, b'E', ALT_GR), typed("€"));
        assert_eq!(to_unicode(&mut german, b'Q', ALT_GR), typed("@"));
        assert_eq!(to_unicode(&mut german, b'Y', PLAIN), typed("z"));
        assert_eq!(to_unicode(&mut german, VK_OEM_PLUS, PLAIN), Translated::Dead(0xb4));
        assert_eq!(to_unicode(&mut german, b'2', PLAIN), typed("´2"));
        let mut swiss = typist("ch");
        assert_eq!(to_unicode(&mut swiss, VK_OEM_6, PLAIN), Translated::Dead(0xa8));
        assert_eq!(to_unicode(&mut swiss, b'U', SHIFT), typed("Ü"));
        let mut us = typist("us");
        assert_eq!(to_unicode(&mut us, b'E', ALT_GR), Translated::Nothing);
        assert_eq!(
            to_unicode(&mut us, b'C', Modifiers { control: true, ..PLAIN }),
            typed("\u{3}")
        );
        assert_eq!(to_unicode(&mut us, 0x0d, PLAIN), typed("\r"));
    }

    #[test]
    fn vk_key_scan_finds_the_key_and_shift_state_on_the_layout() {
        let german = typist("de");
        assert_eq!(key_for(&german, u16::from(b'z')), Some(u16::from(b'Y')));
        assert_eq!(key_for(&german, u16::from(b'Z')), Some(0x100 | u16::from(b'Y')));
        assert_eq!(key_for(&german, 0x20ac), Some(0x600 | u16::from(b'E')));
        assert_eq!(key_for(&german, u16::from(b'\r')), Some(0x0d));
        assert_eq!(key_for(&german, 0x1234), None);
        let us = typist("us");
        assert_eq!(key_for(&us, u16::from(b'z')), Some(u16::from(b'Z')));
    }

    #[test]
    fn map_virtual_key_marks_dead_keys() {
        let german = typist("de");
        assert_eq!(key_character(&german, VK_OEM_PLUS), 0xb4 | DEAD_KEY_BIT);
        assert_eq!(key_character(&german, b'Y'), u32::from('z'));
        assert_eq!(key_character(&typist("us"), b'A'), u32::from('a'));
    }
}

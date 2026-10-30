use super::*;

const NONE: Modifiers = Modifiers::NONE;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT_GRAPH: Modifiers = Modifiers::ALT_GRAPH;
const CAPS: Modifiers = Modifiers::CAPS_LOCK;
const E: u16 = 18;
const Q: u16 = 16;
const Y_ON_US: u16 = 21;
const C: u16 = 46;
const TWO: u16 = 3;
const ACUTE_ON_DE: u16 = 13;
const CIRCUMFLEX_ON_DE: u16 = 41;
const APOSTROPHE_ON_US: u16 = 40;

fn typist(name: &str) -> Typist {
    let mut typist = Typist::new();
    typist.set_layout(named(name.as_bytes()).unwrap());
    typist
}

fn typed(typist: &mut Typist, presses: &[(u16, Modifiers)]) -> String {
    let mut text = String::new();
    for &(code, modifiers) in presses {
        if let Press::Typed { typed, .. } = typist.press(code, modifiers) {
            text.extend(typed.characters());
        }
    }
    text
}

#[test]
fn every_layout_row_has_one_character_per_key_and_the_names_are_unique() {
    for (index, layout) in LAYOUTS.iter().enumerate() {
        assert_eq!(named(layout.name.as_bytes()), Some(index));
        assert!(layout.is_character_key(E), "{}", layout.name);
    }
    assert_eq!(LAYOUTS[US_LAYOUT].name, "us");
    assert_eq!(named(b"US"), None);
    assert_eq!(layout(99).name, "us");
    assert!(core::ptr::eq(
        LAYOUTS[named(b"fi").unwrap()].keys,
        LAYOUTS[named(b"se").unwrap()].keys
    ));
}

#[test]
fn the_us_layout_types_what_the_us_keymap_does() {
    let mut us = Typist::new();
    for code in 0..=98u16 {
        let Some([plain, shifted]) = keymap::printable(code) else {
            continue;
        };
        for (modifiers, expected) in [(NONE, plain), (SHIFT, shifted)] {
            assert_eq!(
                us.press(code, modifiers),
                Press::Typed {
                    typed: Typed::one(char::from(expected)),
                    meta: false
                },
                "code {code}"
            );
        }
    }
    assert!(!LAYOUTS[US_LAYOUT].has_third_level());
    assert_eq!(typed(&mut us, &[(E, ALT_GRAPH)]), "");
}

#[test]
fn caps_lock_capitalises_letters_only_accented_ones_included() {
    let mut french = typist("fr");
    assert_eq!(typed(&mut french, &[(Q, CAPS), (TWO, CAPS)]), "Aé");
    assert_eq!(typed(&mut french, &[(TWO, SHIFT)]), "2");
    assert_eq!(typed(&mut french, &[(Q, CAPS.with(SHIFT))]), "a");
    let mut international = typist("us-intl");
    assert_eq!(
        typed(&mut international, &[(38, ALT_GRAPH.with(CAPS))]),
        "Ø"
    );
}

#[test]
fn a_dead_key_waits_and_accents_the_next_letter() {
    let mut german = typist("de");
    assert_eq!(german.press(ACUTE_ON_DE, NONE), Press::Dead);
    assert!(german.is_pending());
    assert_eq!(typed(&mut german, &[(E, NONE)]), "é");
    assert_eq!(typed(&mut german, &[(ACUTE_ON_DE, NONE), (E, SHIFT)]), "É");
    assert_eq!(
        typed(&mut german, &[(ACUTE_ON_DE, NONE), (key::SPACE, NONE)]),
        "´"
    );
    assert_eq!(
        typed(&mut german, &[(ACUTE_ON_DE, NONE), (ACUTE_ON_DE, NONE)]),
        "´"
    );
    assert_eq!(
        typed(&mut german, &[(CIRCUMFLEX_ON_DE, NONE), (TWO, NONE)]),
        "^2"
    );
    assert_eq!(
        typed(
            &mut german,
            &[(ACUTE_ON_DE, NONE), (key::LEFT_SHIFT, SHIFT), (E, SHIFT)]
        ),
        "É"
    );
    assert_eq!(
        typed(
            &mut german,
            &[(ACUTE_ON_DE, NONE), (CIRCUMFLEX_ON_DE, NONE)]
        ),
        "´"
    );
    assert!(german.is_pending());
    assert_eq!(typed(&mut german, &[(E, NONE)]), "ê");
}

#[test]
fn backspace_takes_a_waiting_accent_back_and_other_keys_drop_it() {
    let mut german = typist("de");
    german.press(ACUTE_ON_DE, NONE);
    assert_eq!(german.press(key::BACKSPACE, NONE), Press::Cancelled);
    assert!(!german.is_pending());
    assert_eq!(german.press(key::BACKSPACE, NONE), Press::Other);
    german.press(ACUTE_ON_DE, NONE);
    assert_eq!(german.press(key::ENTER, NONE), Press::Other);
    assert_eq!(typed(&mut german, &[(E, NONE)]), "e");
    german.press(ACUTE_ON_DE, NONE);
    german.set_layout(named(b"fr").unwrap());
    assert!(!german.is_pending());
}

#[test]
fn the_third_level_types_altgr_characters() {
    let mut german = typist("de");
    assert_eq!(typed(&mut german, &[(E, ALT_GRAPH), (Q, ALT_GRAPH)]), "€@");
    assert_eq!(typed(&mut german, &[(TWO, ALT_GRAPH)]), "²");
    assert_eq!(typed(&mut german, &[(Y_ON_US, ALT_GRAPH)]), "");
    assert!(LAYOUTS[named(b"de").unwrap()].has_third_level());
    let option = ALT_GRAPH.with(Modifiers::ALT);
    assert_eq!(typed(&mut german, &[(E, option)]), "€");
    assert_eq!(
        german.press(Y_ON_US, option),
        Press::Typed {
            typed: Typed::one('z'),
            meta: true
        }
    );
}

#[test]
fn us_international_dead_keys_type_ascii_accents_and_c_cedilla() {
    let mut international = typist("us-intl");
    assert_eq!(
        typed(
            &mut international,
            &[(APOSTROPHE_ON_US, NONE), (key::SPACE, NONE)]
        ),
        "'"
    );
    assert_eq!(
        typed(&mut international, &[(APOSTROPHE_ON_US, NONE), (C, NONE)]),
        "ç"
    );
    assert_eq!(
        typed(
            &mut international,
            &[(APOSTROPHE_ON_US, SHIFT), (key::SPACE, NONE)]
        ),
        "\""
    );
    assert_eq!(
        typed(
            &mut international,
            &[(APOSTROPHE_ON_US, SHIFT), (Y_ON_US, SHIFT)]
        ),
        "\"Y"
    );
    assert_eq!(
        typed(
            &mut international,
            &[(APOSTROPHE_ON_US, SHIFT), (Y_ON_US, NONE)]
        ),
        "ÿ"
    );
}

#[test]
fn ctrl_and_super_make_shortcuts_from_the_first_two_levels() {
    let mut german = typist("de");
    let ctrl = Modifiers::CTRL;
    assert_eq!(german.press(Y_ON_US, ctrl), Press::Shortcut('z'));
    assert_eq!(german.press(ACUTE_ON_DE, ctrl), Press::Shortcut('\0'));
    let press = german.press(Y_ON_US, ctrl);
    assert_eq!(press.character(Y_ON_US, ctrl), 0x1a);
    assert_eq!(
        press
            .terminal_bytes(Y_ON_US, ctrl, CursorKeys::Normal)
            .as_bytes(),
        [0x1a]
    );
    let command = Modifiers::SUPER;
    let press = german.press(Y_ON_US, command);
    assert_eq!(press.character(Y_ON_US, command), u32::from('z'));
    assert!(
        press
            .terminal_bytes(Y_ON_US, command, CursorKeys::Normal)
            .is_empty()
    );
}

#[test]
fn a_press_sends_utf_8_to_the_terminal_and_its_last_character_to_windows() {
    let mut german = typist("de");
    let press = german.press(ACUTE_ON_DE, NONE);
    assert!(
        press
            .terminal_bytes(ACUTE_ON_DE, NONE, CursorKeys::Normal)
            .is_empty()
    );
    assert_eq!(press.character(ACUTE_ON_DE, NONE), 0);
    let press = german.press(TWO, NONE);
    let Press::Typed { typed, .. } = press else {
        panic!("{press:?}");
    };
    assert_eq!(typed.before_last(), Some('´'));
    assert_eq!(press.character(TWO, NONE), u32::from('2'));
    assert_eq!(
        press
            .terminal_bytes(TWO, NONE, CursorKeys::Normal)
            .as_bytes(),
        "´2".as_bytes()
    );
    let alt = Modifiers::ALT;
    let press = german.press(E, alt);
    assert_eq!(
        press.terminal_bytes(E, alt, CursorKeys::Normal).as_bytes(),
        b"\x1be"
    );
    let press = german.press(key::UP, NONE);
    assert_eq!(
        press
            .terminal_bytes(key::UP, NONE, CursorKeys::Normal)
            .as_bytes(),
        b"\x1b[A"
    );
    assert_eq!(press.character(key::UP, NONE), 0);
    assert_eq!(
        german.press(key::ENTER, NONE).character(key::ENTER, NONE),
        0x0d
    );
    assert_eq!(german.character_of(E, ALT_GRAPH), u32::from('€'));
    assert_eq!(german.character_of(ACUTE_ON_DE, NONE), 0);
    assert_eq!(
        german.press(key::KEYPAD_DOT, NONE),
        Press::Typed {
            typed: Typed::one(','),
            meta: false
        }
    );
}

#[test]
fn the_dead_keys_each_layout_has() {
    let listed = |name: &str| -> String {
        let layout = &LAYOUTS[named(name.as_bytes()).unwrap()];
        DEAD_ACCENTS
            .iter()
            .filter(|&&accent| layout.has_dead_key(accent))
            .collect()
    };
    assert_eq!(listed("us"), "");
    assert_eq!(listed("us-intl"), "´`^¨~");
    assert_eq!(listed("de"), "´`^");
    assert_eq!(listed("fr"), "^¨");
    assert_eq!(listed("es"), "´`^¨");
    assert_eq!(listed("se"), "´`^¨~");
}

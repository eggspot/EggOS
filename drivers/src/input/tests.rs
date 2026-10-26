use super::keymap::{CursorKeys, KeyBytes, KeyChange, KeyRepeat, Keyboard, Modifiers, encode};
use super::*;

const DESIGN_PRINTABLE_TABLE: [(u16, u16, &str, &str); 14] = [
    (2, 13, "1234567890-=", "!@#$%^&*()_+"),
    (16, 27, "qwertyuiop[]", "QWERTYUIOP{}"),
    (30, 40, "asdfghjkl;'", "ASDFGHJKL:\""),
    (41, 41, "`", "~"),
    (43, 43, "\\", "|"),
    (44, 53, "zxcvbnm,./", "ZXCVBNM<>?"),
    (57, 57, " ", " "),
    (71, 73, "789", "789"),
    (75, 77, "456", "456"),
    (79, 83, "1230.", "1230."),
    (55, 55, "*", "*"),
    (74, 74, "-", "-"),
    (78, 78, "+", "+"),
    (98, 98, "/", "/"),
];

fn design_character(code: u16) -> Option<(u8, u8)> {
    DESIGN_PRINTABLE_TABLE
        .iter()
        .find(|(first, last, _, _)| (*first..=*last).contains(&code))
        .map(|&(first, _, unshifted, shifted)| {
            let index = usize::from(code - first);
            (unshifted.as_bytes()[index], shifted.as_bytes()[index])
        })
}

fn bytes(code: u16, modifiers: Modifiers) -> Vec<u8> {
    encode(code, modifiers, CursorKeys::Normal)
        .as_bytes()
        .to_vec()
}

const SHIFT: Modifiers = Modifiers::SHIFT;
const CTRL: Modifiers = Modifiers::CTRL;
const ALT: Modifiers = Modifiers::ALT;
const NONE: Modifiers = Modifiers::NONE;

#[test]
fn every_key_types_the_design_table_s_character_unshifted_and_shifted_and_no_other_does() {
    for code in 0..=key::MAX {
        match design_character(code) {
            Some((unshifted, shifted)) => {
                assert_eq!(bytes(code, NONE), [unshifted], "key {code}");
                assert_eq!(bytes(code, SHIFT), [shifted], "key {code} with Shift");
                assert_eq!(keymap::printable(code), Some([unshifted, shifted]));
            }
            None => assert_eq!(keymap::printable(code), None, "key {code}"),
        }
    }
}

#[test]
fn caps_lock_inverts_letters_only() {
    let caps = Modifiers::CAPS_LOCK;
    assert_eq!(bytes(key::A, caps), b"A");
    assert_eq!(bytes(key::A, caps.with(SHIFT)), b"a");
    assert_eq!(bytes(2, caps), b"1");
    assert_eq!(bytes(39, caps), b";");
    assert_eq!(bytes(39, caps.with(SHIFT)), b":");
}

#[test]
fn ctrl_makes_control_characters_as_the_design_lists_them() {
    let letter = |character: u8| {
        (16..=50)
            .find(|&code| keymap::printable(code).is_some_and(|[plain, _]| plain == character))
            .unwrap()
    };
    for (character, control) in [
        (b'c', 0x03),
        (b'd', 0x04),
        (b'u', 0x15),
        (b'w', 0x17),
        (b'h', 0x08),
        (b'i', 0x09),
        (b'm', 0x0d),
        (b'[', 0x1b),
        (b']', 0x1d),
        (b'\\', 0x1c),
    ] {
        assert_eq!(
            bytes(letter(character), CTRL),
            [control],
            "Ctrl+{character}"
        );
    }
    assert_eq!(bytes(7, CTRL.with(SHIFT)), [0x1e], "Ctrl+^");
    assert_eq!(bytes(key::MINUS, CTRL.with(SHIFT)), [0x1f], "Ctrl+_");
    assert_eq!(bytes(3, CTRL.with(SHIFT)), [0x00], "Ctrl+@");
    assert_eq!(bytes(key::SPACE, CTRL), [0x00], "Ctrl+Space");
    assert_eq!(bytes(key::A, CTRL.with(SHIFT)), [0x01], "Ctrl+Shift+A");
    assert_eq!(bytes(2, CTRL), b"1", "Ctrl+1 has no control character");
}

#[test]
fn enter_backspace_tab_and_esc_send_their_bytes_and_alt_prefixes_esc() {
    let table: [(u16, Modifiers, &[u8]); 12] = [
        (key::ENTER, NONE, b"\r"),
        (key::KEYPAD_ENTER, NONE, b"\r"),
        (key::BACKSPACE, NONE, b"\x7f"),
        (key::BACKSPACE, CTRL, b"\x08"),
        (key::TAB, NONE, b"\t"),
        (key::TAB, SHIFT, b"\x1b[Z"),
        (key::ESC, NONE, b"\x1b"),
        (key::A, ALT, b"\x1ba"),
        (key::A, ALT.with(SHIFT), b"\x1bA"),
        (key::A, ALT.with(CTRL), b"\x1b\x01"),
        (key::ENTER, ALT, b"\x1b\r"),
        (key::ESC, ALT, b"\x1b\x1b"),
    ];
    for (code, modifiers, expected) in table {
        assert_eq!(bytes(code, modifiers), expected, "key {code} {modifiers:?}");
    }
}

#[test]
fn cursor_editing_and_function_keys_send_xterm_s_sequences() {
    let table: [(u16, &[u8]); 20] = [
        (key::UP, b"\x1b[A"),
        (key::DOWN, b"\x1b[B"),
        (key::RIGHT, b"\x1b[C"),
        (key::LEFT, b"\x1b[D"),
        (key::HOME, b"\x1b[H"),
        (key::END, b"\x1b[F"),
        (key::INSERT, b"\x1b[2~"),
        (key::DELETE, b"\x1b[3~"),
        (key::PAGE_UP, b"\x1b[5~"),
        (key::PAGE_DOWN, b"\x1b[6~"),
        (key::F1, b"\x1bOP"),
        (key::F2, b"\x1bOQ"),
        (key::F3, b"\x1bOR"),
        (key::F4, b"\x1bOS"),
        (key::F5, b"\x1b[15~"),
        (key::F6, b"\x1b[17~"),
        (key::F8, b"\x1b[19~"),
        (key::F10, b"\x1b[21~"),
        (key::F11, b"\x1b[23~"),
        (key::F12, b"\x1b[24~"),
    ];
    for (code, expected) in table {
        assert_eq!(bytes(code, NONE), expected, "key {code}");
    }
    assert_eq!(bytes(key::F7, NONE), b"\x1b[18~");
    assert_eq!(bytes(key::F9, NONE), b"\x1b[20~");
}

#[test]
fn application_cursor_mode_changes_only_the_unmodified_arrows() {
    let application = |code, modifiers| {
        encode(code, modifiers, CursorKeys::Application)
            .as_bytes()
            .to_vec()
    };
    assert_eq!(application(key::UP, NONE), b"\x1bOA");
    assert_eq!(application(key::LEFT, NONE), b"\x1bOD");
    assert_eq!(application(key::UP, CTRL), b"\x1b[1;5A");
    assert_eq!(application(key::HOME, NONE), b"\x1b[H");
}

#[test]
fn modified_special_keys_carry_xterm_s_modifier_parameter() {
    let table: [(u16, Modifiers, &[u8]); 9] = [
        (key::UP, SHIFT, b"\x1b[1;2A"),
        (key::RIGHT, ALT.with(SHIFT), b"\x1b[1;4C"),
        (key::LEFT, CTRL, b"\x1b[1;5D"),
        (key::DOWN, CTRL.with(ALT).with(SHIFT), b"\x1b[1;8B"),
        (key::DELETE, CTRL, b"\x1b[3;5~"),
        (key::F5, ALT, b"\x1b[15;3~"),
        (key::F1, SHIFT, b"\x1b[1;2P"),
        (key::HOME, CTRL, b"\x1b[1;5H"),
        (key::PAGE_UP, CTRL.with(SHIFT), b"\x1b[5;6~"),
    ];
    for (code, modifiers, expected) in table {
        assert_eq!(bytes(code, modifiers), expected, "key {code} {modifiers:?}");
    }
}

#[test]
fn view_keys_super_modifiers_and_locks_send_nothing() {
    for code in [key::PAGE_UP, key::PAGE_DOWN, key::HOME, key::END] {
        assert!(encode(code, SHIFT, CursorKeys::Normal).is_empty(), "{code}");
    }
    for code in [
        key::LEFT_SHIFT,
        key::RIGHT_SHIFT,
        key::LEFT_CTRL,
        key::RIGHT_CTRL,
        key::LEFT_ALT,
        key::RIGHT_ALT,
        key::LEFT_SUPER,
        key::RIGHT_SUPER,
        key::CAPS_LOCK,
        key::NUM_LOCK,
        key::SCROLL_LOCK,
        button::LEFT,
        key::MAX,
    ] {
        assert!(encode(code, NONE, CursorKeys::Normal).is_empty(), "{code}");
    }
    assert!(encode(key::A, Modifiers::SUPER, CursorKeys::Normal).is_empty());
    assert!(encode(key::UP, Modifiers::SUPER, CursorKeys::Normal).is_empty());
}

#[test]
fn no_sequence_is_longer_than_key_bytes_capacity() {
    let every_modifier = (0..32).map(Modifiers::from_bits);
    for modifiers in every_modifier {
        for code in 0..=key::MAX {
            for mode in [CursorKeys::Normal, CursorKeys::Application] {
                assert!(encode(code, modifiers, mode).as_bytes().len() < KeyBytes::CAPACITY);
            }
        }
    }
}

#[test]
fn the_keyboard_tracks_both_sides_of_each_modifier_and_toggles_caps_lock() {
    let mut keyboard = Keyboard::new();
    assert_eq!(keyboard.modifiers(), NONE);
    assert_eq!(keyboard.key(key::LEFT_SHIFT, 1), Some(KeyChange::Pressed));
    keyboard.key(key::RIGHT_SHIFT, 1);
    keyboard.key(key::LEFT_SHIFT, 0);
    assert_eq!(keyboard.modifiers(), SHIFT);
    keyboard.key(key::RIGHT_SHIFT, 0);
    keyboard.key(key::RIGHT_CTRL, 1);
    keyboard.key(key::RIGHT_ALT, 1);
    keyboard.key(key::LEFT_SUPER, 1);
    assert_eq!(keyboard.modifiers(), CTRL.with(ALT).with(Modifiers::SUPER));
    for code in [key::RIGHT_CTRL, key::RIGHT_ALT, key::LEFT_SUPER] {
        keyboard.key(code, 0);
    }
    assert_eq!(keyboard.modifiers(), NONE);
    keyboard.key(key::CAPS_LOCK, 1);
    assert_eq!(keyboard.modifiers(), Modifiers::CAPS_LOCK);
    keyboard.key(key::CAPS_LOCK, 0);
    assert_eq!(keyboard.modifiers(), Modifiers::CAPS_LOCK);
    keyboard.key(key::CAPS_LOCK, 1);
    keyboard.key(key::CAPS_LOCK, 0);
    assert_eq!(keyboard.modifiers(), NONE);
    assert!(!keyboard.any_held());
}

#[test]
fn the_keyboard_ignores_host_autorepeat_repeats_unheld_releases_and_unknown_codes() {
    let mut keyboard = Keyboard::new();
    assert_eq!(keyboard.key(key::A, 1), Some(KeyChange::Pressed));
    assert_eq!(keyboard.key(key::A, 1), None);
    assert_eq!(keyboard.key(key::A, 2), None);
    assert!(keyboard.is_held(key::A) && keyboard.any_held());
    assert_eq!(keyboard.key(key::A, 0), Some(KeyChange::Released));
    assert_eq!(keyboard.key(key::A, 0), None);
    assert_eq!(keyboard.key(key::MAX + 1, 1), None);
    assert_eq!(keyboard.key(key::LEFT_SHIFT, 0), None);
    assert_eq!(keyboard.modifiers(), NONE);
    assert!(!keyboard.any_held());
    assert_eq!(keyboard.key(key::MAX, 1), Some(KeyChange::Pressed));
    assert!(keyboard.is_held(key::MAX));
}

#[test]
fn a_key_repeats_after_the_delay_then_at_the_interval_until_released() {
    let mut repeat = KeyRepeat::new();
    assert_eq!(repeat.next(), None);
    repeat.pressed(key::A, true, 1000, 400);
    assert_eq!(repeat.next(), Some(1400));
    assert_eq!(repeat.due(1399, 40), None);
    assert_eq!(repeat.due(1400, 40), Some(key::A));
    assert_eq!(repeat.due(1400, 40), None);
    assert_eq!(repeat.due(1445, 40), Some(key::A));
    assert_eq!(repeat.next(), Some(1480));
    repeat.released(key::SPACE);
    assert_eq!(repeat.next(), Some(1480));
    repeat.released(key::A);
    assert_eq!(repeat.due(2000, 40), None);
    assert_eq!(repeat.next(), None);
}

#[test]
fn another_press_stops_a_repeat_and_late_polls_send_one_repeat_not_a_burst() {
    let mut repeat = KeyRepeat::new();
    repeat.pressed(key::A, true, 0, 400);
    repeat.pressed(key::LEFT_SHIFT, false, 100, 400);
    assert_eq!(repeat.due(1000, 40), None);
    repeat.pressed(key::A, true, 0, 400);
    assert_eq!(repeat.due(5000, 40), Some(key::A));
    assert_eq!(repeat.due(5000, 40), None);
    assert_eq!(repeat.next(), Some(5040));
}

#[test]
fn absolute_positions_scale_to_the_pixel_a_host_truncated_them_from() {
    let range = AxisRange { min: 0, max: 32767 };
    for size in [1280u32, 800, 1024, 768, 1] {
        for pixel in 0..size {
            let value = (i64::from(pixel) * 32767 / i64::from(size)) as i32;
            assert_eq!(
                scale_to_screen(value, range, size),
                pixel,
                "pixel {pixel} of {size} (value {value})"
            );
        }
        assert_eq!(scale_to_screen(32767, range, size), size - 1);
    }
}

#[test]
fn absolute_positions_are_clamped_and_empty_ranges_or_screens_give_0() {
    let range = AxisRange { min: 100, max: 200 };
    assert_eq!(scale_to_screen(50, range, 1000), 0);
    assert_eq!(scale_to_screen(100, range, 1000), 0);
    assert_eq!(scale_to_screen(150, range, 1000), 500);
    assert_eq!(scale_to_screen(200, range, 1000), 999);
    assert_eq!(scale_to_screen(i32::MAX, range, 1000), 999);
    assert_eq!(scale_to_screen(150, AxisRange { min: 5, max: 5 }, 1000), 0);
    assert_eq!(scale_to_screen(150, range, 0), 0);
    let wide = AxisRange {
        min: i32::MIN,
        max: i32::MAX,
    };
    assert_eq!(scale_to_screen(i32::MIN, wide, u32::MAX), 0);
    assert_eq!(scale_to_screen(i32::MAX, wide, u32::MAX), u32::MAX - 1);
}

#[test]
fn a_device_moves_a_pointer_with_relative_or_absolute_axes() {
    let mouse = Capabilities {
        relative: true,
        ..Capabilities::default()
    };
    let tablet = Capabilities {
        absolute: Some((AxisRange::default(), AxisRange::default())),
        ..Capabilities::default()
    };
    let keyboard = Capabilities {
        keyboard: true,
        ..Capabilities::default()
    };
    assert!(mouse.is_pointer() && tablet.is_pointer() && !keyboard.is_pointer());
}

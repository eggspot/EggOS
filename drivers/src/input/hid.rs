//! USB HID boot protocol reports (Device Class Definition for HID 1.11, appendix B; usage IDs
//! from HID Usage Tables 1.4, chapter 10) turned into the kernel's input events (D239).

use super::{RawEvent, SYN_REPORT, button, event_type, relative};

/// The Linux key code of keyboard usage `usage` (usage page 07h), 0 for none.
pub fn key_of_usage(usage: u8) -> u16 {
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    const FROM_0X1E: [u16; 0x68 - 0x1e] = [
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 28, 1, 14, 15, 57, 12, 13, 26, 27, 43, 43, 39, 40, 41, 51,
        52, 53, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 99, 70, 119, 110, 102, 104,
        111, 107, 109, 106, 105, 108, 103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71, 72,
        73, 82, 83, 86, 127, 116, 117,
    ];
    match usage {
        0x04..=0x1d => LETTERS[usize::from(usage - 0x04)],
        0x1e..=0x67 => FROM_0X1E[usize::from(usage - 0x1e)],
        0x68..=0x73 => 183 + u16::from(usage - 0x68),
        0x87 => 89,
        0x88 => 93,
        0x89 => 124,
        0x8a => 92,
        0x8b => 94,
        0x90 => 122,
        0x91 => 123,
        _ => 0,
    }
}

/// The Linux key codes of the boot report's modifier bits, bit 0 first.
pub const MODIFIER_KEYS: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

/// Bytes of a boot keyboard report: modifiers, a reserved byte, six key usages.
pub const KEYBOARD_REPORT_BYTES: usize = 8;

/// A boot keyboard: the last report, so each new one becomes presses and releases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BootKeyboard {
    previous: [u8; KEYBOARD_REPORT_BYTES],
}

fn key(code: u16, pressed: bool) -> RawEvent {
    RawEvent {
        kind: event_type::KEY,
        code,
        value: i32::from(pressed),
    }
}

const SYNC: RawEvent = RawEvent {
    kind: event_type::SYN,
    code: SYN_REPORT,
    value: 0,
};

impl BootKeyboard {
    /// Hands `event` what changed since the last report, then a report's end; nothing for a
    /// report of rollover errors (every key usage 01h) or one too short.
    pub fn report(&mut self, report: &[u8], event: &mut dyn FnMut(RawEvent)) -> usize {
        let Some(report) = report.get(..KEYBOARD_REPORT_BYTES) else {
            return 0;
        };
        if report[2..].iter().all(|&usage| usage == 0x01) {
            return 0;
        }
        let mut sent = 0;
        let mut send = |raw: RawEvent| {
            event(raw);
            sent += 1;
        };
        let (old, new) = (self.previous[0], report[0]);
        for (bit, &code) in MODIFIER_KEYS.iter().enumerate() {
            let mask = 1 << bit;
            if old & mask != new & mask {
                send(key(code, new & mask != 0));
            }
        }
        for &usage in &self.previous[2..] {
            if usage > 0x03 && !report[2..].contains(&usage) {
                let code = key_of_usage(usage);
                if code != 0 {
                    send(key(code, false));
                }
            }
        }
        for &usage in &report[2..] {
            if usage > 0x03 && !self.previous[2..].contains(&usage) {
                let code = key_of_usage(usage);
                if code != 0 {
                    send(key(code, true));
                }
            }
        }
        self.previous.copy_from_slice(report);
        if sent > 0 {
            event(SYNC);
            sent += 1;
        }
        sent
    }
}

/// The boot keyboard's output report for its lock lights: Num Lock bit 0, Caps Lock bit 1,
/// Scroll Lock bit 2.
pub fn lights_report(lights: super::LockLights) -> u8 {
    u8::from(lights.num) | (u8::from(lights.caps) << 1) | (u8::from(lights.scroll) << 2)
}

/// A boot mouse: the buttons it last reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BootMouse {
    buttons: u8,
}

const MOUSE_BUTTONS: [u16; 3] = [button::LEFT, button::RIGHT, button::MIDDLE];

impl BootMouse {
    /// Hands `event` a boot mouse report's buttons, motion and wheel (its optional fourth
    /// byte), then a report's end. USB's Y grows downwards, as the kernel's does.
    pub fn report(&mut self, report: &[u8], event: &mut dyn FnMut(RawEvent)) -> usize {
        if report.len() < 3 {
            return 0;
        }
        let mut sent = 0;
        let mut send = |raw: RawEvent| {
            event(raw);
            sent += 1;
        };
        let buttons = report[0] & 0b111;
        for (bit, &code) in MOUSE_BUTTONS.iter().enumerate() {
            let mask = 1 << bit;
            if self.buttons & mask != buttons & mask {
                send(key(code, buttons & mask != 0));
            }
        }
        self.buttons = buttons;
        let motion = [
            (relative::X, i32::from(report[1] as i8)),
            (relative::Y, i32::from(report[2] as i8)),
            (relative::WHEEL, report.get(3).map_or(0, |&wheel| i32::from(wheel as i8))),
        ];
        for (code, value) in motion {
            if value != 0 {
                send(RawEvent {
                    kind: event_type::REL,
                    code,
                    value,
                });
            }
        }
        if sent > 0 {
            event(SYNC);
            sent += 1;
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events_of(keyboard: &mut BootKeyboard, report: [u8; 8]) -> Vec<RawEvent> {
        let mut events = Vec::new();
        keyboard.report(&report, &mut |event| events.push(event));
        events
    }

    #[test]
    fn usages_map_to_linux_key_codes() {
        assert_eq!(key_of_usage(0x04), 30);
        assert_eq!(key_of_usage(0x1d), 44);
        assert_eq!(key_of_usage(0x27), 11);
        assert_eq!(key_of_usage(0x28), 28);
        assert_eq!(key_of_usage(0x3a), 59);
        assert_eq!(key_of_usage(0x45), 88);
        assert_eq!(key_of_usage(0x4f), 106);
        assert_eq!(key_of_usage(0x52), 103);
        assert_eq!(key_of_usage(0x58), 96);
        assert_eq!(key_of_usage(0x63), 83);
        assert_eq!(key_of_usage(0x64), 86);
        assert_eq!(key_of_usage(0x68), 183);
        assert_eq!(key_of_usage(0x00), 0);
    }

    #[test]
    fn a_key_held_then_let_go_is_pressed_and_released_once() {
        let mut keyboard = BootKeyboard::default();
        let pressed = events_of(&mut keyboard, [0x02, 0, 0x04, 0, 0, 0, 0, 0]);
        assert_eq!(pressed[0], key(42, true));
        assert_eq!(pressed[1], key(30, true));
        assert_eq!(pressed[2], SYNC);
        assert!(events_of(&mut keyboard, [0x02, 0, 0x04, 0, 0, 0, 0, 0]).is_empty());
        let released = events_of(&mut keyboard, [0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(released, vec![key(42, false), key(30, false), SYNC]);
    }

    #[test]
    fn a_rollover_report_changes_nothing() {
        let mut keyboard = BootKeyboard::default();
        events_of(&mut keyboard, [0, 0, 0x04, 0, 0, 0, 0, 0]);
        assert!(events_of(&mut keyboard, [0, 0, 1, 1, 1, 1, 1, 1]).is_empty());
    }

    #[test]
    fn a_mouse_report_moves_clicks_and_scrolls() {
        let mut mouse = BootMouse::default();
        let mut events = Vec::new();
        mouse.report(&[0x01, 5, 0xfe, 0xff], &mut |event| events.push(event));
        assert_eq!(events[0], key(button::LEFT, true));
        assert_eq!(events[1].code, relative::X);
        assert_eq!(events[1].value, 5);
        assert_eq!(events[2].value, -2);
        assert_eq!(events[3].code, relative::WHEEL);
        assert_eq!(events[3].value, -1);
        assert_eq!(events[4], SYNC);
    }

    #[test]
    fn the_lights_report_has_num_caps_and_scroll() {
        let lights = super::super::LockLights {
            caps: true,
            num: false,
            scroll: true,
        };
        assert_eq!(lights_report(lights), 0b110);
    }
}

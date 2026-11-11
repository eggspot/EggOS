use super::protocol::{DiscardEvents, EventRecord, WindowError, shell_menu_flags};
use super::*;
use crate::input::keymap::Modifiers;
use alloc::vec::Vec;

const CLIENT: u16 = 3;
const WM_RBUTTONUP: i32 = 0x0205;
const WM_LBUTTONUP: i32 = 0x0202;

#[derive(Default)]
struct Recorded(Vec<(u16, EventRecord)>);

impl EventSink for Recorded {
    fn send(&mut self, client: u16, record: EventRecord) {
        self.0.push((client, record));
    }
}

fn with_icons(profile: Profile, count: u32) -> (Desktop, WindowId) {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    desktop.set_profile(profile, &mut Damage::new(), &mut DiscardEvents);
    let created = desktop.create(CLIENT, 0, 0, 400, 300, 0).unwrap();
    let window = u32::from(created.window);
    desktop
        .commit(CLIENT, window, &[], &mut Damage::new(), &mut DiscardEvents)
        .unwrap();
    for key in 0..count {
        desktop
            .set_tray_icon(
                CLIENT,
                window,
                key,
                (false, false),
                b"Icon",
                None,
                &mut Damage::new(),
            )
            .unwrap();
    }
    (desktop, created.window)
}

fn overflow_button(desktop: &Desktop) -> Option<Rect> {
    let mut button = None;
    desktop.for_each_tray_item(|item, rect| {
        if item == shell::PanelItem::TrayOverflow {
            button = Some(rect);
        }
    });
    button
}

fn press(desktop: &mut Desktop, code: u16, rect: Rect, events: &mut Recorded) {
    let mut damage = Damage::new();
    desktop.pointer_moved(rect.x + rect.width / 2, rect.y + rect.height / 2, &mut damage, events);
    desktop.button(code, true, &mut damage, events);
    desktop.button(code, false, &mut damage, events);
}

fn tray_messages(events: &Recorded) -> Vec<(i32, i32)> {
    events
        .0
        .iter()
        .filter(|(_, record)| record.kind == event::TRAY)
        .map(|(_, record)| (record.a, record.b))
        .collect()
}

#[test]
fn four_icons_need_no_overflow_and_five_show_the_button() {
    for profile in Profile::ALL {
        let (four, _) = with_icons(profile, 4);
        assert_eq!(overflow_button(&four), None, "{profile:?}");
        let (five, _) = with_icons(profile, 5);
        assert!(overflow_button(&five).is_some(), "{profile:?}");
    }
}

#[test]
fn the_overflow_button_opens_a_popup_listing_the_icons_it_stands_for() {
    for profile in Profile::ALL {
        let (mut desktop, _) = with_icons(profile, 6);
        let button = overflow_button(&desktop).unwrap();
        press(&mut desktop, button::LEFT, button, &mut Recorded::default());
        assert!(desktop.tray_overflow_open(), "{profile:?}");
        let popup = desktop.tray_overflow().unwrap();
        assert_eq!(popup.width, tray::TRAY_OVERFLOW_WIDTH);
        assert_eq!(popup.right(), button.right());
        assert_eq!(popup.height, 3 * MENU_ITEM_HEIGHT + 2);
        assert!(desktop.tray_overflow_row(2).is_some());
        assert!(desktop.tray_overflow_row(3).is_none());
    }
}

#[test]
fn the_popup_sits_under_the_top_bar_or_above_the_taskbar() {
    let (mut top, _) = with_icons(Profile::Macos, 5);
    let button = overflow_button(&top).unwrap();
    press(&mut top, button::LEFT, button, &mut Recorded::default());
    assert_eq!(top.tray_overflow().unwrap().y, button.bottom() + MENU_GAP);
    let (mut bottom, _) = with_icons(Profile::Windows, 5);
    let button = overflow_button(&bottom).unwrap();
    press(&mut bottom, button::LEFT, button, &mut Recorded::default());
    let popup = bottom.tray_overflow().unwrap();
    assert_eq!(popup.bottom() + MENU_GAP, bottom.layout.taskbar.unwrap().y);
}

#[test]
fn a_press_on_the_button_again_closes_the_popup() {
    let (mut desktop, _) = with_icons(Profile::Linux, 5);
    let button = overflow_button(&desktop).unwrap();
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    assert!(!desktop.tray_overflow_open());
}

#[test]
fn a_right_click_on_a_row_closes_the_popup_and_reaches_the_icon_s_window() {
    let (mut desktop, _) = with_icons(Profile::Windows, 5);
    let button = overflow_button(&desktop).unwrap();
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    let row = desktop.tray_overflow_row(1).unwrap();
    let mut events = Recorded::default();
    press(&mut desktop, button::RIGHT, row, &mut events);
    assert!(!desktop.tray_overflow_open());
    assert!(tray_messages(&events).contains(&(WM_RBUTTONUP, 4)));
}

#[test]
fn keys_move_through_the_popup_and_enter_is_a_left_click() {
    let (mut desktop, _) = with_icons(Profile::Macos, 5);
    let button = overflow_button(&desktop).unwrap();
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    let mut events = Recorded::default();
    for code in [key::DOWN, key::DOWN, key::ENTER] {
        desktop.key(code, true, Modifiers::NONE, &mut Damage::new(), &mut events);
        desktop.key(code, false, Modifiers::NONE, &mut Damage::new(), &mut events);
    }
    assert!(!desktop.tray_overflow_open());
    assert!(tray_messages(&events).contains(&(WM_LBUTTONUP, 4)));
}

#[test]
fn esc_closes_the_popup_and_a_removed_icon_closes_it_too() {
    let (mut desktop, window) = with_icons(Profile::Windows, 6);
    let button = overflow_button(&desktop).unwrap();
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    desktop.key(key::ESC, true, Modifiers::NONE, &mut Damage::new(), &mut DiscardEvents);
    assert!(!desktop.tray_overflow_open());
    press(&mut desktop, button::LEFT, button, &mut Recorded::default());
    desktop
        .set_tray_icon(
            CLIENT,
            u32::from(window),
            5,
            (false, true),
            b"",
            None,
            &mut Damage::new(),
        )
        .unwrap();
    assert!(!desktop.tray_overflow_open());
}

const OPEN: &[u8] = &[0, 4, b'O', b'p', b'e', b'n'];
const GRAYED_QUIT: &[u8] = &[shell_menu_flags::GRAYED, 4, b'Q', b'u', b'i', b't'];
const SEPARATOR: &[u8] = &[shell_menu_flags::SEPARATOR, 0];
const EXIT: &[u8] = &[shell_menu_flags::CHECKED, 4, b'E', b'x', b'i', b't'];

fn entries(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|part| part.iter().copied()).collect()
}

fn shell_menu_answers(events: &Recorded) -> Vec<i32> {
    events
        .0
        .iter()
        .filter(|(_, record)| record.kind == event::SHELL_MENU)
        .map(|(_, record)| record.a)
        .collect()
}

fn icon_rect(desktop: &Desktop, at: u8) -> Rect {
    desktop.tray_target(at).unwrap()
}

fn menu_after_right_click(profile: Profile) -> (Desktop, WindowId) {
    let (mut desktop, window) = with_icons(profile, 1);
    let icon = icon_rect(&desktop, 0);
    press(&mut desktop, button::RIGHT, icon, &mut Recorded::default());
    let bytes = entries(&[OPEN, GRAYED_QUIT, SEPARATOR, EXIT]);
    desktop
        .show_shell_menu(
            CLIENT,
            u32::from(window),
            &bytes,
            &mut Damage::new(),
            &mut DiscardEvents,
        )
        .unwrap();
    (desktop, window)
}

#[test]
fn a_shell_menu_is_anchored_to_the_icon_last_pressed() {
    for profile in Profile::ALL {
        let (desktop, _) = menu_after_right_click(profile);
        let icon = icon_rect(&desktop, 0);
        let menu = desktop.shell_menu_rect().unwrap();
        assert_eq!(menu.right(), icon.right(), "{profile:?}");
        assert_eq!(menu.width, 160, "{profile:?}");
        assert_eq!(menu.height, 3 * MENU_ITEM_HEIGHT + 8 + 2, "{profile:?}");
        match desktop.layout.taskbar {
            Some(bar) => assert_eq!(menu.bottom() + MENU_GAP, bar.y),
            None => assert_eq!(menu.y, icon.bottom() + MENU_GAP),
        }
    }
}

#[test]
fn a_click_on_an_entry_answers_its_index_from_one() {
    let (mut desktop, _) = menu_after_right_click(Profile::Windows);
    let row = desktop.shell_menu_row(3).unwrap();
    let mut events = Recorded::default();
    press(&mut desktop, button::LEFT, row, &mut events);
    assert!(!desktop.shell_menu_open());
    assert_eq!(shell_menu_answers(&events), [4]);
}

#[test]
fn a_grayed_entry_or_a_separator_cannot_be_chosen() {
    let (mut desktop, _) = menu_after_right_click(Profile::Macos);
    let mut events = Recorded::default();
    for index in [1, 2] {
        let row = desktop.shell_menu_row(index).unwrap();
        press(&mut desktop, button::LEFT, row, &mut events);
        assert!(desktop.shell_menu_open());
    }
    assert!(shell_menu_answers(&events).is_empty());
}

#[test]
fn keys_skip_what_cannot_be_chosen_and_esc_answers_nothing() {
    let (mut desktop, _) = menu_after_right_click(Profile::Linux);
    let mut events = Recorded::default();
    for code in [key::DOWN, key::DOWN] {
        desktop.key(code, true, Modifiers::NONE, &mut Damage::new(), &mut events);
    }
    assert_eq!(desktop.shell_menu_highlight(), Some(3));
    desktop.key(key::ESC, true, Modifiers::NONE, &mut Damage::new(), &mut events);
    assert_eq!(shell_menu_answers(&events), [0]);
}

#[test]
fn a_click_elsewhere_closes_the_shell_menu_with_nothing_chosen() {
    let (mut desktop, _) = menu_after_right_click(Profile::Windows);
    let mut events = Recorded::default();
    press(&mut desktop, button::LEFT, Rect::new(300, 300, 1, 1), &mut events);
    assert!(!desktop.shell_menu_open());
    assert_eq!(shell_menu_answers(&events), [0]);
}

#[test]
fn malformed_entries_are_refused() {
    let (mut desktop, window) = with_icons(Profile::Windows, 1);
    for bytes in [&[][..], &[0, 9, b'x'][..], &[0, 2, 0xff, 0xfe][..]] {
        let shown = desktop.show_shell_menu(
            CLIENT,
            u32::from(window),
            bytes,
            &mut Damage::new(),
            &mut DiscardEvents,
        );
        assert_eq!(shown, Err(WindowError::Invalid));
    }
}

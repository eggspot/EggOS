use super::protocol::{DiscardEvents, EventRecord, DEAD_KEY_CHARACTER};
use super::tests::{fnv, painted};
use super::*;
use crate::input::keymap::Modifiers;
use alloc::vec;

const US: KeyboardLayoutChoice = KeyboardLayoutChoice {
    number: 0,
    name: "us",
    title: "English (US)",
    code: "US",
};
const GERMAN: KeyboardLayoutChoice = KeyboardLayoutChoice {
    number: 11,
    name: "de",
    title: "German",
    code: "DE",
};
const SUPER_SPACE: Option<&str> = Some("Super+Space");

#[derive(Default)]
struct Recorded(Vec<EventRecord>);

impl EventSink for Recorded {
    fn send(&mut self, _client: u16, record: EventRecord) {
        self.0.push(record);
    }
}

fn desktop(profile: Profile) -> Desktop {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    desktop.set_profile(profile, &mut Damage::new(), &mut DiscardEvents);
    desktop
}

fn with_layouts(profile: Profile, choices: &[KeyboardLayoutChoice]) -> Desktop {
    let mut desktop = desktop(profile);
    desktop.set_keyboard_layouts(choices, 0, SUPER_SPACE, &mut Damage::new());
    desktop
}

fn press_at(desktop: &mut Desktop, x: u32, y: u32) -> Action {
    let mut damage = Damage::new();
    desktop.pointer_moved(x, y, &mut damage, &mut DiscardEvents);
    let action = desktop.left_button(true, &mut damage, &mut DiscardEvents);
    desktop.left_button(false, &mut damage, &mut DiscardEvents);
    action
}

fn press_in(desktop: &mut Desktop, rect: Rect) -> Action {
    press_at(desktop, rect.x + rect.width / 2, rect.y + rect.height / 2)
}

fn tap(desktop: &mut Desktop, code: u16) -> Action {
    let action = desktop.key(
        code,
        true,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    desktop.key(
        code,
        false,
        Modifiers::NONE,
        &mut Damage::new(),
        &mut DiscardEvents,
    );
    action
}

#[test]
fn one_layout_shows_no_indicator_and_leaves_the_bar_as_it_was() {
    for profile in Profile::ALL {
        let plain = desktop(profile);
        let mut one = desktop(profile);
        one.set_keyboard_layouts(&[US], 0, SUPER_SPACE, &mut Damage::new());
        assert_eq!(one.keyboard_layout_indicator(), None, "{profile:?}");
        assert_eq!(one.panel_item_at(1230, 14), plain.panel_item_at(1230, 14));
        assert_eq!(fnv(&painted(&one)), fnv(&painted(&plain)), "{profile:?}");
    }
}

#[test]
fn two_layouts_put_the_indicator_left_of_the_power_button_on_the_top_bar() {
    for profile in [Profile::Linux, Profile::Macos] {
        let mut desktop = with_layouts(profile, &[US, GERMAN]);
        assert_eq!(
            desktop.keyboard_layout_indicator(),
            Some(Rect::new(1216, 0, 28, 28)),
            "{profile:?}"
        );
        assert_eq!(desktop.keyboard_layout_text_at(), Some((1222, 6)));
        assert_eq!(desktop.keyboard_layout_shown(), Some(US));
        assert_eq!(
            desktop.panel_item_at(1230, 14),
            Some(shell::PanelItem::KeyboardLayout)
        );
        desktop.set_agent_connected(true, &mut Damage::new());
        assert_eq!(desktop.agent_chip(), Some(Rect::new(1152, 4, 56, 20)));
    }
}

#[test]
fn two_layouts_put_the_indicator_left_of_the_clock_on_the_taskbar() {
    let mut desktop = with_layouts(Profile::Windows, &[US, GERMAN]);
    assert_eq!(
        desktop.keyboard_layout_indicator(),
        Some(Rect::new(1096, 760, 40, 40))
    );
    assert_eq!(desktop.keyboard_layout_text_at(), Some((1108, 772)));
    desktop.set_agent_connected(true, &mut Damage::new());
    assert_eq!(desktop.agent_chip(), Some(Rect::new(1032, 770, 56, 20)));
}

#[test]
fn the_set_appearing_relays_the_bar_and_a_new_layout_in_force_redraws_only_the_target() {
    let mut desktop = desktop(Profile::Linux);
    let mut damage = Damage::new();
    desktop.set_keyboard_layouts(&[US, GERMAN], 0, SUPER_SPACE, &mut damage);
    let bar = desktop.layout().top_bar.unwrap();
    assert!(damage.rects().iter().any(|rect| rect.union(bar) == *rect));
    let before = painted(&desktop);
    let mut damage = Damage::new();
    desktop.set_keyboard_layouts(&[US, GERMAN], 11, SUPER_SPACE, &mut damage);
    let indicator = desktop.keyboard_layout_indicator().unwrap();
    assert_eq!(damage.rects(), &[indicator]);
    assert_eq!(desktop.keyboard_layout_shown(), Some(GERMAN));
    let after = painted(&desktop);
    for (index, (old, new)) in before.iter().zip(&after).enumerate() {
        if old != new {
            let (x, y) = ((index % 1280) as u32, (index / 1280) as u32);
            assert!(contains(indicator, x, y), "({x}, {y}) changed");
        }
    }
    assert_ne!(before, after);
    let mut damage = Damage::new();
    desktop.set_keyboard_layouts(&[US, GERMAN], 11, SUPER_SPACE, &mut damage);
    assert!(damage.rects().is_empty());
}

#[test]
fn resting_on_the_indicator_shows_the_layout_and_the_switch_key() {
    for (chord, text) in [
        (Some("Super+Space"), "English (US)  Super+Space"),
        (Some("Ctrl+Space"), "English (US)  Ctrl+Space"),
        (None, "English (US)"),
    ] {
        let mut desktop = desktop(Profile::Linux);
        let mut damage = Damage::new();
        desktop.set_keyboard_layouts(&[US, GERMAN], 0, chord, &mut damage);
        desktop.set_time(1000);
        desktop.pointer_moved(1230, 14, &mut damage, &mut DiscardEvents);
        desktop.tick(1000 + TOOLTIP_DELAY_MS - 1, &mut damage, &mut DiscardEvents);
        assert_eq!(desktop.tooltip_rect(), None);
        desktop.tick(1700, &mut damage, &mut DiscardEvents);
        assert_eq!(desktop.keyboard_layout_tooltip(), Some(text));
        let tip = desktop.tooltip_rect().unwrap();
        assert_eq!(tip.y, 32);
        assert_eq!(tip.width, text.len() as u32 * 8 + 16);
        assert!(tip.right() <= 1280 - 8);
    }
}

#[test]
fn a_press_on_the_indicator_opens_its_menu_under_the_top_bar() {
    let mut desktop = with_layouts(Profile::Linux, &[US, GERMAN]);
    assert_eq!(press_at(&mut desktop, 1230, 14), Action::None);
    assert!(desktop.keyboard_layout_menu_open());
    let menu = desktop.keyboard_layout_menu().unwrap();
    assert_eq!(menu, Rect::new(1244 - LAYOUT_MENU_WIDTH, 32, LAYOUT_MENU_WIDTH, 106));
    assert!(!desktop.keys_go_to_terminal());
    let german = desktop
        .keyboard_layout_menu_row(LayoutMenuRow::Layout(1))
        .unwrap();
    assert_eq!(press_in(&mut desktop, german), Action::ChooseKeyboardLayout(11));
    assert!(!desktop.keyboard_layout_menu_open());
}

#[test]
fn the_taskbar_s_menu_opens_above_it() {
    let mut desktop = with_layouts(Profile::Windows, &[US, GERMAN]);
    press_at(&mut desktop, 1116, 780);
    let menu = desktop.keyboard_layout_menu().unwrap();
    assert_eq!(menu.bottom(), 756);
    assert_eq!(menu.right(), 1136);
    assert_eq!(menu.width, LAYOUT_MENU_WIDTH);
}

#[test]
fn keyboard_settings_opens_the_keyboard_pane_and_a_second_press_closes_the_menu() {
    let mut desktop = with_layouts(Profile::Macos, &[US, GERMAN]);
    press_at(&mut desktop, 1230, 14);
    let settings = desktop
        .keyboard_layout_menu_row(LayoutMenuRow::KeyboardSettings)
        .unwrap();
    let german = desktop
        .keyboard_layout_menu_row(LayoutMenuRow::Layout(1))
        .unwrap();
    assert_eq!(settings.y, german.bottom() + 8);
    assert_eq!(
        press_in(&mut desktop, settings),
        Action::OpenSettingsPane(SettingsPane::Keyboard)
    );
    assert_eq!(
        desktop
            .settings_pane_command(SettingsPane::Keyboard)
            .map(|command| command.words().map(<[u8]>::to_vec).collect::<Vec<_>>()),
        Some(vec![
            b"/bin/settings".to_vec(),
            b"--pane".to_vec(),
            b"keyboard".to_vec()
        ])
    );
    press_at(&mut desktop, 1230, 14);
    assert!(desktop.keyboard_layout_menu_open());
    assert_eq!(press_at(&mut desktop, 1230, 14), Action::None);
    assert!(!desktop.keyboard_layout_menu_open());
}

#[test]
fn esc_and_a_press_outside_close_the_menu_and_down_then_enter_chooses_the_next_layout() {
    let mut desktop = with_layouts(Profile::Linux, &[US, GERMAN]);
    press_at(&mut desktop, 1230, 14);
    tap(&mut desktop, key::ESC);
    assert!(!desktop.keyboard_layout_menu_open());
    press_at(&mut desktop, 1230, 14);
    press_at(&mut desktop, 400, 400);
    assert!(!desktop.keyboard_layout_menu_open());
    press_at(&mut desktop, 1230, 14);
    assert_eq!(
        desktop.keyboard_layout_menu_highlight(),
        Some(LayoutMenuRow::Layout(0))
    );
    tap(&mut desktop, key::DOWN);
    assert_eq!(
        tap(&mut desktop, key::ENTER),
        Action::ChooseKeyboardLayout(11)
    );
    assert!(!desktop.keyboard_layout_menu_open());
}

#[test]
fn the_menu_marks_the_layout_in_force_and_closes_when_the_set_shrinks_to_one() {
    let mut desktop = with_layouts(Profile::Linux, &[US, GERMAN]);
    press_at(&mut desktop, 1230, 14);
    let pixels = painted(&desktop);
    let english = desktop
        .keyboard_layout_menu_row(LayoutMenuRow::Layout(0))
        .unwrap();
    let menu = desktop.keyboard_layout_menu().unwrap();
    let mark = (menu.x + 8..menu.x + 16)
        .flat_map(|x| (english.y..english.bottom()).map(move |y| (x, y)))
        .any(|(x, y)| pixels[(y * 1280 + x) as usize] == colour::ACCENT);
    assert!(mark, "the > mark in the accent");
    desktop.set_keyboard_layouts(&[GERMAN], 11, SUPER_SPACE, &mut Damage::new());
    assert!(!desktop.keyboard_layout_menu_open());
    assert_eq!(desktop.keyboard_layout_indicator(), None);
}

#[test]
fn a_dead_key_reaches_the_focused_window_as_its_accent_with_the_dead_bit() {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    let created = desktop.create(3, 0, 0, 400, 300, 0).unwrap();
    let mut events = Recorded::default();
    desktop
        .commit(
            3,
            u32::from(created.window),
            &[],
            &mut Damage::new(),
            &mut events,
        )
        .unwrap();
    let mut events = Recorded::default();
    for (code, character) in [
        (13, u32::from('´') | DEAD_KEY_CHARACTER),
        (0, DEAD_KEY_CHARACTER),
        (18, u32::from('e')),
    ] {
        desktop.key_typing(
            code,
            true,
            Modifiers::NONE,
            character,
            &mut Damage::new(),
            &mut events,
        );
    }
    let keys: Vec<(i32, u32, u32)> = events
        .0
        .iter()
        .filter(|record| record.kind == event::KEY)
        .map(|record| (record.a, record.c, record.d))
        .collect();
    assert_eq!(
        keys,
        [
            (13, modifier::DEAD, u32::from('´')),
            (0, modifier::DEAD, 0),
            (18, 0, u32::from('e')),
        ]
    );
}

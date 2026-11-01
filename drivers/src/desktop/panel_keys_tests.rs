use super::protocol::DiscardEvents;
use super::*;
use crate::input::keymap::Modifiers;
use alloc::vec::Vec;

fn desktop(profile: Profile) -> Desktop {
    let mut desktop = Desktop::new(1280, 800).unwrap();
    desktop.set_profile(profile, &mut Damage::new(), &mut DiscardEvents);
    desktop
}

fn chips(desktop: &Desktop) -> Vec<(Rect, &'static str, bool)> {
    let mut chips = Vec::new();
    desktop.for_each_sticky_chip(|rect, word, locked| chips.push((rect, word, locked)));
    chips
}

fn latch(desktop: &mut Desktop, latched: Modifiers, locked: Modifiers) {
    desktop.set_latched(latched, locked, &mut Damage::new());
}

#[test]
fn no_modifier_held_draws_no_chip() {
    let desktop = desktop(Profile::Macos);
    assert_eq!(desktop.sticky_strip(), None);
    assert!(chips(&desktop).is_empty());
}

#[test]
fn a_latched_shift_is_a_chip_left_of_the_power_button_and_a_locked_one_says_so_in_capitals() {
    let mut desktop = desktop(Profile::Macos);
    assert_eq!(desktop.power_button().x, 1244);
    latch(&mut desktop, Modifiers::SHIFT, Modifiers::NONE);
    assert_eq!(
        chips(&desktop),
        [(Rect::new(1180, 4, 56, 20), "Shift", false)]
    );
    latch(&mut desktop, Modifiers::NONE, Modifiers::SHIFT);
    assert_eq!(
        chips(&desktop),
        [(Rect::new(1180, 4, 56, 20), "SHIFT", true)]
    );
    let mut damage = Damage::new();
    desktop.set_agent_connected(true, &mut damage);
    assert_eq!(desktop.agent_chip().unwrap().x, 1180);
    assert_eq!(desktop.sticky_strip(), Some(Rect::new(1116, 4, 56, 20)));
}

#[test]
fn several_modifiers_show_in_their_order_four_pixels_apart() {
    let mut desktop = desktop(Profile::Macos);
    let shift_ctrl = Modifiers::from_bits(Modifiers::SHIFT.bits() | Modifiers::CTRL.bits());
    latch(&mut desktop, shift_ctrl, Modifiers::ALT);
    assert_eq!(
        chips(&desktop),
        [
            (Rect::new(1084, 4, 56, 20), "Shift", false),
            (Rect::new(1144, 4, 48, 20), "Ctrl", false),
            (Rect::new(1196, 4, 40, 20), "ALT", true),
        ]
    );
    assert_eq!(desktop.sticky_strip().unwrap().right(), 1244 - 8);
}

#[test]
fn caps_lock_and_unknown_bits_draw_no_chip() {
    let mut desktop = desktop(Profile::Macos);
    latch(&mut desktop, Modifiers::CAPS_LOCK, Modifiers::NONE);
    assert_eq!(desktop.sticky_strip(), None);
}

#[test]
fn in_the_taskbar_the_strip_ends_sixteen_left_of_the_clock_and_the_items_stop_before_it() {
    let mut desktop = desktop(Profile::Windows);
    let bar = desktop.layout().taskbar.unwrap();
    latch(&mut desktop, Modifiers::SUPER, Modifiers::NONE);
    let strip = desktop.sticky_strip().unwrap();
    assert_eq!(strip.right(), desktop.clock_rect().x - 16);
    assert_eq!(strip.y, bar.y + (bar.height - 20) / 2);
    assert_eq!(chips(&desktop), [(strip, "Super", false)]);
    desktop.set_agent_connected(true, &mut Damage::new());
    assert_eq!(
        desktop.sticky_strip().unwrap().right(),
        desktop.agent_chip().unwrap().x - 8
    );
}

#[test]
fn turning_sticky_keys_on_or_off_shows_a_notice_for_three_seconds_then_the_chips() {
    let mut desktop = desktop(Profile::Macos);
    let mut damage = Damage::new();
    desktop.tick(1000, &mut damage, &mut DiscardEvents);
    latch(&mut desktop, Modifiers::SHIFT, Modifiers::NONE);
    desktop.sticky_keys_turned(true, &mut damage);
    assert_eq!(desktop.sticky_keys_notice(), Some(STICKY_KEYS_ON));
    assert_eq!(
        chips(&desktop),
        [(Rect::new(1236 - 128, 4, 128, 20), STICKY_KEYS_ON, false)]
    );
    desktop.tick(1000 + NOTICE_MS - 1, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.sticky_keys_notice(), Some(STICKY_KEYS_ON));
    let mut damage = Damage::new();
    desktop.tick(1000 + NOTICE_MS, &mut damage, &mut DiscardEvents);
    assert_eq!(desktop.sticky_keys_notice(), None);
    assert!(
        damage
            .rects()
            .iter()
            .any(|rect| rect.x <= 1236 - 128 && rect.right() >= 1236)
    );
    assert_eq!(
        chips(&desktop),
        [(Rect::new(1180, 4, 56, 20), "Shift", false)]
    );
    desktop.sticky_keys_turned(false, &mut damage);
    assert_eq!(desktop.sticky_keys_notice(), Some(STICKY_KEYS_OFF));
    assert_eq!(desktop.sticky_strip().unwrap().width, 136);
}

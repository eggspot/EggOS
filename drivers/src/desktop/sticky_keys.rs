//! The sticky keys' chips in the panel (docs/ux/first-run-accessibility-v0.3.md section 2.6,
//! D224): one chip per modifier the kernel's input layer holds latched or locked, the mouse
//! keys' button chip (D602), and a notice in their place for a while: sticky keys or mouse keys
//! turned on or off, a lock key's change with toggle keys on, a zoom level
//! (docs/ux/accessibility-v0.4.md sections 2.2, 3.3 and 3.4).

use super::launcher::NOTICE_MS;
use super::paint::{self, Canvas};
use super::{CHIP_HEIGHT, CHIP_PADDING, CHIP_Y, Desktop, colour};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use crate::input::keymap::Modifiers;

/// The modifiers whose chips can show, in the order they are drawn, each with its word while
/// latched and while locked.
pub const STICKY_CHIPS: [(Modifiers, &str, &str); 5] = [
    (Modifiers::SHIFT, "Shift", "SHIFT"),
    (Modifiers::CTRL, "Ctrl", "CTRL"),
    (Modifiers::ALT, "Alt", "ALT"),
    (Modifiers::ALT_GRAPH, "AltGr", "ALTGR"),
    (Modifiers::SUPER, "Super", "SUPER"),
];
/// What the strip says for [`NOTICE_MS`] after sticky keys are turned on.
pub const STICKY_KEYS_ON: &str = "Sticky keys on";
/// What the strip says for [`NOTICE_MS`] after sticky keys are turned off.
pub const STICKY_KEYS_OFF: &str = "Sticky keys off";

const STICKY_CHIP_GAP: u32 = 4;
const STICKY_STRIP_BEFORE_CHIP: u32 = 8;
const STICKY_STRIP_BEFORE_CLOCK: u32 = 16;
const STICKY_LABEL_Y: u32 = 2;

/// The modifiers the chips show, the mouse keys' button chip, and the notice, if one shows:
/// its words, when it showed and for how long (D224, D602).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StickyChips {
    latched: u8,
    locked: u8,
    notice: Option<StripNotice>,
    mouse_keys_button: Option<&'static str>,
    mouse_keys_on: bool,
    alt_taps: (u8, u32),
}

#[derive(Debug, Clone, Copy)]
struct StripNotice {
    words: &'static str,
    at: u32,
    lasting_ms: u32,
}

fn chip_width(word: &str) -> u32 {
    word.len() as u32 * GLYPH_WIDTH as u32 + 2 * CHIP_PADDING
}

impl Desktop {
    /// The kernel's input layer holds `latched` modifiers for the next key or click and
    /// `locked` ones until tapped again: their chips show in the panel.
    pub fn set_latched(&mut self, latched: Modifiers, locked: Modifiers, damage: &mut Damage) {
        let mask = STICKY_CHIPS
            .iter()
            .fold(0, |bits, (modifier, _, _)| bits | modifier.bits());
        let locked = locked.bits() & mask;
        let latched = latched.bits() & mask & !locked;
        if (latched, locked) == (self.sticky.latched, self.sticky.locked) {
            return;
        }
        let before = (self.sticky.latched, self.sticky.locked);
        self.damage_sticky_strip(damage);
        self.sticky.latched = latched;
        self.sticky.locked = locked;
        self.damage_sticky_strip(damage);
        self.reader_sticky_changed(before, (latched, locked), damage);
    }

    /// Sticky keys were turned `on` or off: the strip says so for [`NOTICE_MS`].
    pub fn sticky_keys_turned(&mut self, on: bool, damage: &mut Damage) {
        let words = match on {
            true => STICKY_KEYS_ON,
            false => STICKY_KEYS_OFF,
        };
        self.show_strip_notice(words, NOTICE_MS, damage);
    }

    /// The strip shows `words` for `lasting_ms` in place of its chips: a lock key's change with
    /// toggle keys on, a zoom level, mouse keys turned on (D602). A new notice replaces the one
    /// showing.
    pub fn show_strip_notice(&mut self, words: &'static str, lasting_ms: u32, damage: &mut Damage) {
        self.say(
            eggos_element_tree::reader::Source::Keys,
            eggos_element_tree::reader::Priority::Queue,
            words,
            damage,
        );
        self.damage_sticky_strip(damage);
        self.sticky.notice = Some(StripNotice {
            words,
            at: self.now_ms,
            lasting_ms,
        });
        self.damage_sticky_strip(damage);
    }

    /// The mouse keys' button chip, while a button other than the left is selected or one is
    /// held (`Right button`, `Left button held`); `None` hides it (D602).
    pub fn set_mouse_keys_button(&mut self, words: Option<&'static str>, damage: &mut Damage) {
        if words == self.sticky.mouse_keys_button {
            return;
        }
        self.damage_sticky_strip(damage);
        self.sticky.mouse_keys_button = words;
        self.damage_sticky_strip(damage);
        if let Some(words) = words {
            self.reader_mouse_button(words, damage);
        }
    }

    /// Mouse keys are on or off now, so their toggle chord knows which way to turn them.
    pub fn set_mouse_keys_on(&mut self, on: bool) {
        self.sticky.mouse_keys_on = on;
    }

    /// Whether `code` pressed with `modifiers` turns mouse keys on or off (D602): Left Alt and
    /// Left Shift with Num Lock in every profile, and Alt tapped five times within two seconds
    /// in the macOS profile. Both start `settings set accessibility.mouse_keys`, so the pane,
    /// the CLI and the next boot agree.
    pub(super) fn mouse_keys_toggle(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: Modifiers,
    ) -> Option<super::Action> {
        if !pressed {
            return None;
        }
        let held = modifiers.bits() & !Modifiers::CAPS_LOCK.bits();
        let chord = Modifiers::ALT.bits() | Modifiers::SHIFT.bits();
        let toggled = match code {
            crate::input::key::NUM_LOCK => held == chord,
            crate::input::key::LEFT_ALT | crate::input::key::RIGHT_ALT
                if self.profile == super::Profile::Macos && held & !Modifiers::ALT.bits() == 0 =>
            {
                let (taps, last) = self.sticky.alt_taps;
                let within = self.now_ms.wrapping_sub(last) <= super::STICKY_TOGGLE_WITHIN_MS;
                let taps = match taps > 0 && within {
                    true => taps + 1,
                    false => 1,
                };
                let done = taps == super::STICKY_TOGGLE_TAPS;
                self.sticky.alt_taps = (if done { 0 } else { taps }, self.now_ms);
                done
            }
            _ => {
                self.sticky.alt_taps = (0, self.now_ms);
                false
            }
        };
        toggled.then_some(super::Action::SetSetting {
            key: "accessibility.mouse_keys",
            value: match self.sticky.mouse_keys_on {
                true => "false",
                false => "true",
            },
        })
    }

    /// The notice the strip shows now, if any.
    pub fn sticky_keys_notice(&self) -> Option<&'static str> {
        let notice = self.sticky.notice?;
        if self.now_ms.wrapping_sub(notice.at) >= notice.lasting_ms {
            return None;
        }
        Some(notice.words)
    }

    /// The chips that show, left to right: each one's rectangle, its word and whether its
    /// modifier is locked; the notice's one chip instead while it shows.
    pub fn for_each_sticky_chip(&self, mut chip: impl FnMut(Rect, &'static str, bool)) {
        let Some(strip) = self.sticky_strip() else {
            return;
        };
        if let Some(notice) = self.sticky_keys_notice() {
            chip(strip, notice, false);
            return;
        }
        let mut x = strip.x;
        for (modifier, latched, locked) in STICKY_CHIPS {
            let bits = modifier.bits();
            let word = match (
                self.sticky.locked & bits != 0,
                self.sticky.latched & bits != 0,
            ) {
                (true, _) => locked,
                (false, true) => latched,
                (false, false) => continue,
            };
            let width = chip_width(word);
            chip(
                Rect::new(x, strip.y, width, CHIP_HEIGHT),
                word,
                self.sticky.locked & bits != 0,
            );
            x += width + STICKY_CHIP_GAP;
        }
        if let Some(word) = self.sticky.mouse_keys_button {
            chip(Rect::new(x, strip.y, chip_width(word), CHIP_HEIGHT), word, false);
        }
    }

    /// The strip the chips or the notice fill, while any shows: its right edge 8 px left of
    /// the Agent chip, the keyboard layout indicator, the network indicator or the top bar's
    /// power button, or 16 px left of the taskbar's clock.
    pub fn sticky_strip(&self) -> Option<Rect> {
        let width = match self.sticky_keys_notice() {
            Some(notice) => chip_width(notice),
            None => {
                let shown = self.sticky.latched | self.sticky.locked;
                let mut width = 0;
                for (modifier, latched, _) in STICKY_CHIPS {
                    if shown & modifier.bits() != 0 {
                        width += chip_width(latched) + STICKY_CHIP_GAP;
                    }
                }
                if let Some(word) = self.sticky.mouse_keys_button {
                    width += chip_width(word) + STICKY_CHIP_GAP;
                }
                width.checked_sub(STICKY_CHIP_GAP)?
            }
        };
        self.sticky_strip_of(width)
    }

    fn sticky_strip_of(&self, width: u32) -> Option<Rect> {
        let neighbour = self
            .updates_chip()
            .or(self.agent_chip())
            .or(self.keyboard_layout_indicator())
            .or(self.network_indicator());
        let (right, y) = match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => (
                match neighbour {
                    Some(item) => item.x.checked_sub(STICKY_STRIP_BEFORE_CHIP)?,
                    None => self.clock_rect().x.checked_sub(STICKY_STRIP_BEFORE_CLOCK)?,
                },
                bar.y + (bar.height - CHIP_HEIGHT) / 2,
            ),
            (None, Some(_)) => (
                neighbour
                    .unwrap_or(self.power_button())
                    .x
                    .checked_sub(STICKY_STRIP_BEFORE_CHIP)?,
                CHIP_Y,
            ),
            (None, None) => return None,
        };
        Some(Rect::new(right.checked_sub(width)?, y, width, CHIP_HEIGHT))
    }

    pub(super) fn tick_sticky_notice(&mut self, damage: &mut Damage) {
        let Some(notice) = self.sticky.notice else {
            return;
        };
        if self.now_ms.wrapping_sub(notice.at) < notice.lasting_ms {
            return;
        }
        if let Some(notice) = self.sticky_strip_of(chip_width(notice.words)) {
            damage.add(notice);
        }
        self.sticky.notice = None;
        self.damage_sticky_strip(damage);
    }

    fn damage_sticky_strip(&mut self, damage: &mut Damage) {
        match self.layout.taskbar {
            Some(bar) => {
                damage.add(bar);
                self.panels.moved = true;
            }
            None => {
                if let Some(strip) = self.sticky_strip() {
                    damage.add(strip);
                }
            }
        }
    }

    pub(super) fn draw_sticky_chips(&self, canvas: &mut Canvas<'_>) {
        self.for_each_sticky_chip(|chip, word, locked| {
            let (fill, ink) = match locked {
                true => (colour::ACCENT, colour::ICON_DARK),
                false => (colour::BAR_HOVER, colour::TEXT),
            };
            canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
            canvas.text(chip.x + CHIP_PADDING, chip.y + STICKY_LABEL_Y, word, ink);
        });
    }
}

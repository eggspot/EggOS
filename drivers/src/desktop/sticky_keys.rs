//! The sticky keys' chips in the panel (docs/ux/first-run-accessibility-v0.3.md section 2.6,
//! D224): one chip per modifier the kernel's input layer holds latched or locked, and for
//! [`NOTICE_MS`] after sticky keys are turned on or off, a notice saying so in their place.

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

/// The modifiers the chips show and the notice, if one shows: whether sticky keys went on,
/// and when.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StickyChips {
    latched: u8,
    locked: u8,
    notice: Option<(bool, u32)>,
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
        self.damage_sticky_strip(damage);
        self.sticky.latched = latched;
        self.sticky.locked = locked;
        self.damage_sticky_strip(damage);
    }

    /// Sticky keys were turned `on` or off: the strip says so for [`NOTICE_MS`].
    pub fn sticky_keys_turned(&mut self, on: bool, damage: &mut Damage) {
        self.damage_sticky_strip(damage);
        self.sticky.notice = Some((on, self.now_ms));
        self.damage_sticky_strip(damage);
    }

    /// The notice the strip shows now, if any.
    pub fn sticky_keys_notice(&self) -> Option<&'static str> {
        let (on, at) = self.sticky.notice?;
        if self.now_ms.wrapping_sub(at) >= NOTICE_MS {
            return None;
        }
        Some(match on {
            true => STICKY_KEYS_ON,
            false => STICKY_KEYS_OFF,
        })
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
            let word = match (self.sticky.locked & bits != 0, self.sticky.latched & bits != 0) {
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
                width.checked_sub(STICKY_CHIP_GAP)?
            }
        };
        self.sticky_strip_of(width)
    }

    fn sticky_strip_of(&self, width: u32) -> Option<Rect> {
        let neighbour = self
            .agent_chip()
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
        let Some((on, at)) = self.sticky.notice else {
            return;
        };
        if self.now_ms.wrapping_sub(at) < NOTICE_MS {
            return;
        }
        let shown = match on {
            true => STICKY_KEYS_ON,
            false => STICKY_KEYS_OFF,
        };
        if let Some(notice) = self.sticky_strip_of(chip_width(shown)) {
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

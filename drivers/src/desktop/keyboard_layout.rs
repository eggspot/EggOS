//! The bar's keyboard layout indicator (docs/ux/keyboard-v0.3.md section 9.3, D220): shown
//! only while two or more layouts are chosen, the layout in force's two-letter code on the top
//! bar left of the power button or on the taskbar left of the clock, its tooltip naming the
//! layout and the switch key, and the menu its click opens: the chosen layouts, the one in force
//! marked, and `Keyboard settings...`.

use super::paint::{self, Canvas};
use super::popups::with_shadow;
use super::protocol::EventSink;
use super::shell::{PanelItem, TASKBAR_ITEM};
use super::{Action, Desktop, MENU_GAP, MENU_ITEM_HEIGHT, SettingsPane, colour, contains};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;

/// The most layouts the indicator's menu lists, as many as the settings file's set holds.
pub const MAX_SHOWN_LAYOUTS: usize = 4;
/// The indicator's menu's width, its border included.
pub const LAYOUT_MENU_WIDTH: u32 = 256;
/// The menu's last row, which opens the Keyboard pane of Settings.
pub const KEYBOARD_SETTINGS: &str = "Keyboard settings...";

const INDICATOR_BEFORE_CLOCK: u32 = 8;
const MENU_TEXT_X: u32 = 24;
const MENU_MARK_X: u32 = 8;
const MENU_MARK: &str = ">";
const SEPARATOR_ROW: u32 = 8;
const TOOLTIP_SEPARATOR: &str = "  ";
const TOOLTIP_CAPACITY: usize = 64;

/// One layout the switch key cycles through, as the indicator shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardLayoutChoice {
    /// Its number in the kernel's and the settings file's table.
    pub number: u8,
    /// Its id, the value of `keyboard.layout`.
    pub name: &'static str,
    /// Its title, as the Keyboard pane lists it.
    pub title: &'static str,
    /// Its two capital letters, the indicator's text.
    pub code: &'static str,
}

/// A row of the indicator's menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMenuRow {
    /// The layout at this place of the set.
    Layout(usize),
    /// `Keyboard settings...`.
    KeyboardSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KeyboardLayouts {
    choices: [Option<KeyboardLayoutChoice>; MAX_SHOWN_LAYOUTS],
    in_force: Option<usize>,
    chord: Option<&'static str>,
    tooltip: [u8; TOOLTIP_CAPACITY],
    tooltip_length: usize,
    menu_open: bool,
    menu_hover: Option<LayoutMenuRow>,
}

impl Default for KeyboardLayouts {
    fn default() -> Self {
        Self {
            choices: [None; MAX_SHOWN_LAYOUTS],
            in_force: None,
            chord: None,
            tooltip: [0; TOOLTIP_CAPACITY],
            tooltip_length: 0,
            menu_open: false,
            menu_hover: None,
        }
    }
}

impl KeyboardLayouts {
    fn count(&self) -> usize {
        self.choices.iter().flatten().count()
    }

    fn shown(&self) -> bool {
        self.count() >= 2
    }

    fn in_force_choice(&self) -> Option<KeyboardLayoutChoice> {
        self.choices.get(self.in_force?).copied().flatten()
    }

    fn rows(&self) -> impl Iterator<Item = LayoutMenuRow> {
        (0..self.count())
            .map(LayoutMenuRow::Layout)
            .chain(core::iter::once(LayoutMenuRow::KeyboardSettings))
    }

    fn write_tooltip(&mut self) {
        let mut length = 0;
        let mut push = |part: &str| {
            let end = (length + part.len()).min(TOOLTIP_CAPACITY);
            self.tooltip[length..end].copy_from_slice(&part.as_bytes()[..end - length]);
            length = end;
        };
        if let Some(choice) = self.choices.get(self.in_force.unwrap_or(0)).copied().flatten() {
            push(choice.title);
            if let Some(chord) = self.chord {
                push(TOOLTIP_SEPARATOR);
                push(chord);
            }
        }
        self.tooltip_length = length;
    }
}

impl Desktop {
    /// Shows `choices` (the first [`MAX_SHOWN_LAYOUTS`]) as the layouts the switch key cycles
    /// through, layout number `in_force` as the one in force and `chord` as the switch key the
    /// tooltip names (`None` for none) (D220). With two or more the bar shows the indicator, and
    /// its items move over in the same frame; a new layout in force redraws only the indicator.
    pub fn set_keyboard_layouts(
        &mut self,
        choices: &[KeyboardLayoutChoice],
        in_force: u8,
        chord: Option<&'static str>,
        damage: &mut Damage,
    ) {
        let mut wanted = KeyboardLayouts {
            menu_open: self.keyboard_layouts.menu_open,
            menu_hover: self.keyboard_layouts.menu_hover,
            ..KeyboardLayouts::default()
        };
        for (slot, choice) in wanted.choices.iter_mut().zip(choices) {
            *slot = Some(*choice);
        }
        wanted.in_force = wanted
            .choices
            .iter()
            .position(|choice| choice.is_some_and(|choice| choice.number == in_force));
        wanted.chord = chord;
        wanted.write_tooltip();
        let before = self.keyboard_layouts;
        if wanted == before {
            return;
        }
        let same_set = wanted.choices == before.choices;
        if !same_set {
            self.damage_keyboard_layout_menu(damage);
            for panel in self.layout.panels() {
                damage.add(panel);
            }
            self.panels.moved = true;
        }
        self.keyboard_layouts = wanted;
        if !wanted.shown() {
            self.keyboard_layouts.menu_open = false;
            self.keyboard_layouts.menu_hover = None;
        }
        if !same_set {
            self.damage_keyboard_layout_menu(damage);
        } else if before.in_force != wanted.in_force {
            if let Some(indicator) = self.keyboard_layout_indicator() {
                damage.add(indicator);
            }
            self.damage_keyboard_layout_menu(damage);
        }
        if before.tooltip[..before.tooltip_length] != wanted.tooltip[..wanted.tooltip_length]
            && self.panels.tooltip == Some(PanelItem::KeyboardLayout)
        {
            self.hide_tooltip(damage);
        }
    }

    /// The indicator's hit target while two or more layouts are chosen: 28 px square directly
    /// left of the top bar's network indicator or power button, or a taskbar item 8 px left of
    /// the taskbar's network indicator or clock.
    pub fn keyboard_layout_indicator(&self) -> Option<Rect> {
        if !self.keyboard_layouts.shown() {
            return None;
        }
        let network = self.network_indicator();
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                network
                    .unwrap_or(self.clock_rect())
                    .x
                    .checked_sub(INDICATOR_BEFORE_CLOCK + TASKBAR_ITEM)?,
                bar.y,
                TASKBAR_ITEM,
                TASKBAR_ITEM,
            )),
            (None, Some(bar)) => Some(Rect::new(
                network
                    .unwrap_or(self.power_button())
                    .x
                    .checked_sub(bar.height)?,
                bar.y,
                bar.height,
                bar.height,
            )),
            (None, None) => None,
        }
    }

    /// Where the indicator's text starts: its code centred in the target.
    pub fn keyboard_layout_text_at(&self) -> Option<(u32, u32)> {
        let indicator = self.keyboard_layout_indicator()?;
        let code = self.keyboard_layouts.in_force_choice()?.code;
        let width = code.chars().count() as u32 * GLYPH_WIDTH as u32;
        Some((
            indicator.x + indicator.width.saturating_sub(width) / 2,
            indicator.y + indicator.height.saturating_sub(GLYPH_HEIGHT as u32) / 2,
        ))
    }

    /// The layout in force as the indicator shows it, while it shows.
    pub fn keyboard_layout_shown(&self) -> Option<KeyboardLayoutChoice> {
        self.keyboard_layout_indicator()?;
        self.keyboard_layouts.in_force_choice()
    }

    /// Whether the indicator's menu is open.
    pub fn keyboard_layout_menu_open(&self) -> bool {
        self.keyboard_layouts.menu_open
    }

    /// The indicator's menu, its border included, while the indicator shows: under the top
    /// bar with its right edge at the indicator's, or above the taskbar.
    pub fn keyboard_layout_menu(&self) -> Option<Rect> {
        let indicator = self.keyboard_layout_indicator()?;
        let rows = self.keyboard_layouts.count() as u32 + 1;
        let height = rows * MENU_ITEM_HEIGHT + SEPARATOR_ROW + 2;
        let y = match self.layout.taskbar {
            Some(bar) => bar.y.saturating_sub(MENU_GAP + height),
            None => indicator.bottom() + MENU_GAP,
        };
        Some(Rect::new(
            indicator.right().saturating_sub(LAYOUT_MENU_WIDTH),
            y,
            LAYOUT_MENU_WIDTH,
            height,
        ))
    }

    /// The rectangle of `row` of the indicator's menu.
    pub fn keyboard_layout_menu_row(&self, row: LayoutMenuRow) -> Option<Rect> {
        let menu = self.keyboard_layout_menu()?;
        let count = self.keyboard_layouts.count();
        let index = match row {
            LayoutMenuRow::Layout(index) if index < count => index as u32,
            LayoutMenuRow::Layout(_) => return None,
            LayoutMenuRow::KeyboardSettings => count as u32,
        };
        let separator = match row {
            LayoutMenuRow::KeyboardSettings => SEPARATOR_ROW,
            LayoutMenuRow::Layout(_) => 0,
        };
        Some(Rect::new(
            menu.x + 1,
            menu.y + 1 + index * MENU_ITEM_HEIGHT + separator,
            menu.width - 2,
            MENU_ITEM_HEIGHT,
        ))
    }

    /// The tooltip's text: the layout's title, two spaces and the switch key, or the title
    /// alone with no switch key.
    pub(super) fn keyboard_layout_tooltip(&self) -> Option<&str> {
        let layouts = &self.keyboard_layouts;
        core::str::from_utf8(&layouts.tooltip[..layouts.tooltip_length]).ok()
    }

    fn keyboard_layout_menu_row_at(&self, x: u32, y: u32) -> Option<LayoutMenuRow> {
        self.keyboard_layouts.rows().find(|&row| {
            self.keyboard_layout_menu_row(row)
                .is_some_and(|rect| contains(rect, x, y))
        })
    }

    fn damage_keyboard_layout_menu(&self, damage: &mut Damage) {
        if self.keyboard_layouts.menu_open
            && let Some(menu) = self.keyboard_layout_menu()
        {
            damage.add(with_shadow(menu));
        }
        if let Some(indicator) = self.keyboard_layout_indicator() {
            damage.add(indicator);
        }
    }

    pub(super) fn open_keyboard_layout_menu(&mut self, damage: &mut Damage) {
        if self.keyboard_layout_indicator().is_none() {
            return;
        }
        let (x, y) = self.pointer;
        self.keyboard_layouts.menu_open = true;
        self.keyboard_layouts.menu_hover = self
            .keyboard_layout_menu_row_at(x, y)
            .or(self.keyboard_layouts.in_force.map(LayoutMenuRow::Layout));
        self.damage_keyboard_layout_menu(damage);
    }

    /// Closes the indicator's menu if it is open.
    pub fn close_keyboard_layout_menu(&mut self, damage: &mut Damage) {
        if !self.keyboard_layouts.menu_open {
            return;
        }
        self.damage_keyboard_layout_menu(damage);
        self.keyboard_layouts.menu_open = false;
        self.keyboard_layouts.menu_hover = None;
    }

    fn choose_keyboard_layout_row(
        &mut self,
        row: LayoutMenuRow,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        self.close_keyboard_layout_menu(damage);
        match row {
            LayoutMenuRow::Layout(index) => {
                match self.keyboard_layouts.choices.get(index).copied().flatten() {
                    Some(choice) => Action::ChooseKeyboardLayout(choice.number),
                    None => Action::None,
                }
            }
            LayoutMenuRow::KeyboardSettings => {
                self.open_settings_pane(SettingsPane::Keyboard, damage, events)
            }
        }
    }

    /// A button press at (`x`, `y`) while the indicator's menu is open: the menu closes, and a
    /// left press on a row chooses it. `None` when the press was not the menu's, so it goes on
    /// to whatever is under it.
    pub(super) fn keyboard_layout_menu_press(
        &mut self,
        code: u16,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        if !self.keyboard_layouts.menu_open {
            return None;
        }
        let row = self.keyboard_layout_menu_row_at(x, y);
        let on_menu = self
            .keyboard_layout_menu()
            .is_some_and(|menu| contains(menu, x, y));
        let on_indicator = self
            .keyboard_layout_indicator()
            .is_some_and(|indicator| contains(indicator, x, y));
        self.close_keyboard_layout_menu(damage);
        match row {
            Some(row) if code == crate::input::button::LEFT => {
                Some(self.choose_keyboard_layout_row(row, damage, events))
            }
            _ if on_menu || on_indicator => Some(Action::None),
            _ => None,
        }
    }

    /// The pointer moved to (`x`, `y`): the menu's row under it is drawn hovered.
    pub(super) fn keyboard_layout_menu_hover(&mut self, x: u32, y: u32, damage: &mut Damage) {
        if !self.keyboard_layouts.menu_open {
            return;
        }
        let hover = self.keyboard_layout_menu_row_at(x, y);
        if hover != self.keyboard_layouts.menu_hover {
            self.keyboard_layouts.menu_hover = hover;
            self.damage_keyboard_layout_menu(damage);
        }
    }

    /// Key `code` went down (`pressed`) or up while the indicator's menu is open: Up and Down
    /// move the highlight, Enter chooses, Esc closes.
    pub(super) fn keyboard_layout_menu_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        if !pressed {
            return Action::None;
        }
        let rows = self.keyboard_layouts.count() + 1;
        let index_of = |row: LayoutMenuRow| match row {
            LayoutMenuRow::Layout(index) => index,
            LayoutMenuRow::KeyboardSettings => rows - 1,
        };
        let row_at = |index: usize| match index + 1 == rows {
            true => LayoutMenuRow::KeyboardSettings,
            false => LayoutMenuRow::Layout(index),
        };
        let hover = self.keyboard_layouts.menu_hover.map(index_of);
        match code {
            key::ESC => self.close_keyboard_layout_menu(damage),
            key::UP => {
                let index = hover.map_or(rows - 1, |index| index.saturating_sub(1));
                self.keyboard_layouts.menu_hover = Some(row_at(index));
                self.damage_keyboard_layout_menu(damage);
            }
            key::DOWN => {
                let index = hover.map_or(0, |index| (index + 1).min(rows - 1));
                self.keyboard_layouts.menu_hover = Some(row_at(index));
                self.damage_keyboard_layout_menu(damage);
            }
            key::ENTER | key::KEYPAD_ENTER => {
                if let Some(row) = self.keyboard_layouts.menu_hover {
                    return self.choose_keyboard_layout_row(row, damage, events);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn keyboard_layout_menu_overlaps(&self, clip: Rect) -> bool {
        self.keyboard_layouts.menu_open
            && self
                .keyboard_layout_menu()
                .is_some_and(|menu| with_shadow(menu).overlaps(&clip))
    }

    pub(super) fn draw_keyboard_layout_indicator(
        &self,
        canvas: &mut Canvas<'_>,
        fill: Option<u32>,
    ) {
        let (Some(indicator), Some(choice), Some((x, y))) = (
            self.keyboard_layout_indicator(),
            self.keyboard_layouts.in_force_choice(),
            self.keyboard_layout_text_at(),
        ) else {
            return;
        };
        if !indicator.overlaps(&canvas.clip()) {
            return;
        }
        if let Some(fill) = fill {
            let area = match self.layout.taskbar {
                Some(_) => Rect::new(
                    indicator.x,
                    indicator.y + 1,
                    indicator.width,
                    indicator.height - 1,
                ),
                None => Rect::new(
                    indicator.x,
                    indicator.y,
                    indicator.width,
                    indicator.height - 1,
                ),
            };
            canvas.fill(area, fill);
        }
        canvas.text(x, y, choice.code, colour::TEXT);
    }

    pub(super) fn draw_keyboard_layout_menu(&self, canvas: &mut Canvas<'_>) {
        if !self.keyboard_layouts.menu_open {
            return;
        }
        let Some(menu) = self.keyboard_layout_menu() else {
            return;
        };
        if !with_shadow(menu).overlaps(&canvas.clip()) {
            return;
        }
        canvas.shadow(menu);
        canvas.rounded(menu, &paint::INSET_R6, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(menu.x + 1, menu.y + 1, menu.width - 2, menu.height - 2),
            &paint::INSET_INSIDE_R6,
            true,
            true,
            colour::MENU,
        );
        let text_y = (MENU_ITEM_HEIGHT - GLYPH_HEIGHT as u32) / 2;
        for row in self.keyboard_layouts.rows() {
            let Some(rect) = self.keyboard_layout_menu_row(row) else {
                continue;
            };
            if self.keyboard_layouts.menu_hover == Some(row) {
                canvas.fill(
                    Rect::new(rect.x, rect.y + 2, rect.width, MENU_ITEM_HEIGHT - 4),
                    colour::MENU_HOVER,
                );
            }
            let label = match row {
                LayoutMenuRow::Layout(index) => {
                    let Some(choice) = self.keyboard_layouts.choices[index] else {
                        continue;
                    };
                    if self.keyboard_layouts.in_force == Some(index) {
                        canvas.text(
                            menu.x + MENU_MARK_X,
                            rect.y + text_y,
                            MENU_MARK,
                            colour::ACCENT,
                        );
                    }
                    choice.title
                }
                LayoutMenuRow::KeyboardSettings => {
                    canvas.fill(
                        Rect::new(rect.x, rect.y - SEPARATOR_ROW / 2, rect.width, 1),
                        colour::MENU_BORDER,
                    );
                    KEYBOARD_SETTINGS
                }
            };
            canvas.text(menu.x + MENU_TEXT_X, rect.y + text_y, label, colour::TEXT);
        }
    }

    /// The menu row drawn highlighted, if any.
    pub fn keyboard_layout_menu_highlight(&self) -> Option<LayoutMenuRow> {
        self.keyboard_layouts.menu_hover
    }
}

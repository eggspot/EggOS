//! `EDIT`, single line (sections 3 and 13 of the design): the [`Edit`] model (its text, the
//! selection between an anchor and the caret, scroll in whole cells, `ES_PASSWORD` and
//! `ES_NUMBER`) and its painting.

use super::{Palette, cell_top, framed, ring};
use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_HEIGHT, CELL_WIDTH};
use eggos_font as font;

/// The most UTF-16 units an edit holds.
pub const EDIT_CAPACITY: usize = 1024;
const TEXT_INSET: i32 = 4;
const DARK_FOCUS_INSET: i32 = 2;

/// `EDIT`'s styles this layer reads.
pub mod style {
    pub const PASSWORD: u32 = 0x0020;
    pub const NO_HIDE_SELECTION: u32 = 0x0100;
    pub const READ_ONLY: u32 = 0x0800;
    pub const NUMBER: u32 = 0x2000;
}

/// The character `ES_PASSWORD` shows for every character until `EM_SETPASSWORDCHAR`.
pub const DEFAULT_PASSWORD_CHARACTER: u16 = b'*' as u16;

/// The modifier keys held with a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
}

/// An edit's text and caret.
#[derive(Debug, Clone)]
pub struct Edit {
    units: [u16; EDIT_CAPACITY],
    length: usize,
    anchor: usize,
    caret: usize,
    offset: usize,
    limit: usize,
    pub read_only: bool,
    pub password: u16,
    pub digits_only: bool,
    pub keep_selection: bool,
}

/// What a key did to an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Unchanged,
    Moved,
    Changed,
}

/// The keys an edit reads, as Windows' virtual keys.
pub mod key {
    pub const BACK: u32 = 0x08;
    pub const A: u32 = 0x41;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const RIGHT: u32 = 0x27;
    pub const DELETE: u32 = 0x2e;
}

impl Default for Edit {
    fn default() -> Self {
        Self::new()
    }
}

impl Edit {
    pub const fn new() -> Edit {
        Edit {
            units: [0; EDIT_CAPACITY],
            length: 0,
            anchor: 0,
            caret: 0,
            offset: 0,
            limit: EDIT_CAPACITY,
            read_only: false,
            password: 0,
            digits_only: false,
            keep_selection: false,
        }
    }

    /// A new edit set up from its window's `ES_*` styles.
    pub fn with_style(style: u32) -> Edit {
        let mut edit = Edit::new();
        edit.read_only = style & style::READ_ONLY != 0;
        edit.digits_only = style & style::NUMBER != 0;
        edit.keep_selection = style & style::NO_HIDE_SELECTION != 0;
        if style & style::PASSWORD != 0 {
            edit.password = DEFAULT_PASSWORD_CHARACTER;
        }
        edit
    }

    /// The selection's start and end (equal when nothing is selected); the caret is one end.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// `EM_SETSEL`: `start` -1 removes the selection, `end` -1 (or past the text) is the text's
    /// end; the caret goes to `end`.
    pub fn set_selection(&mut self, start: isize, end: isize, visible: usize) {
        let at = |value: isize| match usize::try_from(value) {
            Ok(value) => value.min(self.length),
            Err(_) => self.length,
        };
        match start {
            -1 => self.anchor = self.caret,
            start => {
                self.anchor = at(start);
                self.caret = at(end);
            }
        }
        self.scroll(visible);
    }

    /// `EM_REPLACESEL` and typing: the selection replaced by `units`, as many as the limit
    /// leaves room for, the caret after them.
    pub fn replace_selection(&mut self, units: &[u16], visible: usize) -> EditOutcome {
        let (start, end) = self.selection();
        let room = self.limit.saturating_sub(self.length - (end - start));
        let inserted = &units[..units.len().min(room)];
        if start == end && inserted.is_empty() {
            return EditOutcome::Unchanged;
        }
        let length = self.length - (end - start) + inserted.len();
        self.units
            .copy_within(end..self.length, start + inserted.len());
        self.units[start..start + inserted.len()].copy_from_slice(inserted);
        self.length = length;
        self.caret = start + inserted.len();
        self.anchor = self.caret;
        self.scroll(visible);
        EditOutcome::Changed
    }

    pub fn text(&self) -> &[u16] {
        &self.units[..self.length]
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// `EM_SETLIMITTEXT`: 0 is the most the edit holds.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = match limit {
            0 => EDIT_CAPACITY,
            limit => limit.min(EDIT_CAPACITY),
        };
    }

    /// `WM_SETTEXT`: the text replaced, cut at the capacity, the caret at its start.
    pub fn set_text(&mut self, units: &[u16]) {
        let length = units.len().min(EDIT_CAPACITY);
        self.units[..length].copy_from_slice(&units[..length]);
        self.length = length;
        self.anchor = 0;
        self.caret = 0;
        self.offset = 0;
    }

    /// Puts the caret after the text, as the picker does after filling in a name.
    pub fn caret_to_end(&mut self, visible: usize) {
        self.caret = self.length;
        self.anchor = self.caret;
        self.scroll(visible);
    }

    /// A typed character: a printable one replaces the selection; with `ES_NUMBER` only
    /// digits are taken.
    pub fn character(&mut self, character: u32, visible: usize) -> EditOutcome {
        let printable = character >= 0x20 && character != 0x7f && character <= 0xffff;
        let refused = self.digits_only && !(0x30..=0x39).contains(&character);
        if !printable || refused || self.read_only {
            return EditOutcome::Unchanged;
        }
        let (start, end) = self.selection();
        if self.length - (end - start) >= self.limit {
            return EditOutcome::Unchanged;
        }
        self.replace_selection(&[character as u16], visible)
    }

    /// A key press without modifiers: Backspace, Delete and the caret's moves.
    pub fn key(&mut self, virtual_key: u32, visible: usize) -> EditOutcome {
        self.key_with(virtual_key, Modifiers::default(), visible)
    }

    /// A key press: Backspace and Delete erase the selection or a character; the arrows, Home
    /// and End move the caret (by words with Ctrl), Shift extending the selection; Ctrl+A
    /// selects all.
    pub fn key_with(&mut self, virtual_key: u32, held: Modifiers, visible: usize) -> EditOutcome {
        let (start, end) = self.selection();
        let selected = start != end;
        let outcome = match virtual_key {
            key::A if held.control => self.select(0, self.length),
            key::BACK | key::DELETE if self.read_only => EditOutcome::Unchanged,
            key::BACK | key::DELETE if selected => self.replace_selection(&[], visible),
            key::BACK if self.caret > 0 => {
                self.anchor = self.caret - 1;
                self.replace_selection(&[], visible)
            }
            key::DELETE if self.caret < self.length => {
                self.anchor = self.caret + 1;
                self.replace_selection(&[], visible)
            }
            key::LEFT | key::RIGHT | key::HOME | key::END => {
                let target = match virtual_key {
                    key::LEFT if held.control => self.previous_word(self.caret),
                    key::RIGHT if held.control => self.next_word(self.caret),
                    key::LEFT if selected && !held.shift => start,
                    key::RIGHT if selected && !held.shift => end,
                    key::LEFT => self.caret.saturating_sub(1),
                    key::RIGHT => (self.caret + 1).min(self.length),
                    key::HOME => 0,
                    _ => self.length,
                };
                match held.shift {
                    true => self.select(self.anchor, target),
                    false => self.select(target, target),
                }
            }
            _ => EditOutcome::Unchanged,
        };
        self.scroll(visible);
        outcome
    }

    fn select(&mut self, anchor: usize, caret: usize) -> EditOutcome {
        match (anchor, caret) == (self.anchor, self.caret) {
            true => EditOutcome::Unchanged,
            false => {
                self.anchor = anchor;
                self.caret = caret;
                EditOutcome::Moved
            }
        }
    }

    fn class_at(&self, index: usize) -> WordClass {
        word_class(self.units[index])
    }

    fn next_word(&self, from: usize) -> usize {
        let mut at = from;
        if at < self.length {
            let class = self.class_at(at);
            while class != WordClass::Space && at < self.length && self.class_at(at) == class {
                at += 1;
            }
        }
        while at < self.length && self.class_at(at) == WordClass::Space {
            at += 1;
        }
        at
    }

    fn previous_word(&self, from: usize) -> usize {
        let mut at = from;
        while at > 0 && self.class_at(at - 1) == WordClass::Space {
            at -= 1;
        }
        if at > 0 {
            let class = self.class_at(at - 1);
            while at > 0 && self.class_at(at - 1) == class {
                at -= 1;
            }
        }
        at
    }

    fn position_at(&self, x: i32) -> usize {
        let cells = (x - TEXT_INSET + CELL_WIDTH / 2).max(0) / CELL_WIDTH;
        (self.offset + cells as usize).min(self.length)
    }

    /// A click at `x` pixels from the edit's left: the caret at the nearest cell boundary, the
    /// anchor there too unless `extend` (Shift held).
    pub fn click(&mut self, x: i32, extend: bool, visible: usize) -> EditOutcome {
        let caret = self.position_at(x);
        let anchor = match extend {
            true => self.anchor,
            false => caret,
        };
        let outcome = self.select(anchor, caret);
        self.scroll(visible);
        outcome
    }

    /// The pointer dragged to `x` with the button held: only the caret follows it.
    pub fn drag(&mut self, x: i32, visible: usize) -> EditOutcome {
        let caret = self.position_at(x);
        let outcome = self.select(self.anchor, caret);
        self.scroll(visible);
        outcome
    }

    /// A double-click at `x`: the word under it selected (a run of letters, digits and `_`, of
    /// other glyphs, or of spaces), the caret at its end.
    pub fn select_word(&mut self, x: i32, visible: usize) -> EditOutcome {
        if self.length == 0 {
            return EditOutcome::Unchanged;
        }
        let index = self.position_at(x).min(self.length - 1);
        let class = self.class_at(index);
        let mut start = index;
        while start > 0 && self.class_at(start - 1) == class {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.length && self.class_at(end) == class {
            end += 1;
        }
        let outcome = self.select(start, end);
        self.scroll(visible);
        outcome
    }

    fn scroll(&mut self, visible: usize) {
        let visible = visible.max(1);
        let mut offset = self.offset.max((self.caret + 1).saturating_sub(visible));
        offset = offset.min(self.caret);
        offset = offset.min((self.length + 1).saturating_sub(visible));
        if self.length < visible {
            offset = 0;
        }
        self.offset = offset;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Space,
    Word,
    Other,
}

fn word_class(unit: u16) -> WordClass {
    match char::from_u32(u32::from(unit)) {
        Some(' ' | '\t') => WordClass::Space,
        Some(character) if character.is_alphanumeric() || character == '_' => WordClass::Word,
        _ => WordClass::Other,
    }
}

/// The cells an edit `width` wide shows.
pub fn visible_cells(width: i32) -> usize {
    ((width - 2 * TEXT_INSET).max(0) / CELL_WIDTH) as usize
}

/// How an edit looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditLook {
    pub focused: bool,
    pub enabled: bool,
}

/// Paints `edit` over the whole of `rect`.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    edit: &Edit,
    look: EditLook,
    palette: &Palette,
) {
    let (fill, border, colour) = match () {
        () if !look.enabled => (
            palette.field_read_only_fill,
            palette.field_read_only_border,
            palette.text_disabled,
        ),
        () if edit.read_only => (
            palette.field_read_only_fill,
            palette.field_read_only_border,
            palette.text,
        ),
        () if look.focused => (palette.field_fill, palette.field_focus_border, palette.text),
        () => (palette.field_fill, palette.field_border, palette.text),
    };
    let saved = surface.clip();
    surface.clip_to(rect);
    framed(surface, rect, palette.rounded, border, fill);
    let caret_shown = look.focused && look.enabled;
    if caret_shown && palette.rounded {
        ring(surface, rect.inset(DARK_FOCUS_INSET), palette.focus);
    }
    surface.clip_to(rect.inset(1));
    let top = cell_top(rect.top, rect.height());
    let visible = visible_cells(rect.width());
    let first = edit.offset.min(edit.length);
    let shown = &edit.text()[first..];
    let (start, end) = edit.selection();
    let selection_shown = look.enabled && (look.focused || edit.keep_selection);
    for (index, character) in char::decode_utf16(shown.iter().copied())
        .take(visible)
        .enumerate()
    {
        let shown_unit = match edit.password {
            0 => character.map_or(font::REPLACEMENT, |character| {
                font::glyph_id(u32::from(character))
            }),
            password => font::glyph_id(u32::from(password)),
        };
        let x = rect.left + TEXT_INSET + index as i32 * CELL_WIDTH;
        let selected = selection_shown && (start..end).contains(&(first + index));
        let glyph_colour = match selected {
            true => {
                surface.fill(Rect::sized(x, top, CELL_WIDTH, CELL_HEIGHT), palette.highlight);
                palette.highlight_text
            }
            false => colour,
        };
        text::glyph(surface, x, top, shown_unit, false, glyph_colour, None);
    }
    if caret_shown {
        let x =
            rect.left + TEXT_INSET + (edit.caret - edit.offset.min(edit.caret)) as i32 * CELL_WIDTH;
        surface.fill(Rect::sized(x, top, 1, CELL_HEIGHT), palette.caret);
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;

    fn typed(edit: &mut Edit, text: &str, visible: usize) {
        for unit in text.encode_utf16() {
            edit.character(u32::from(unit), visible);
        }
    }

    #[test]
    fn typing_inserts_at_the_caret_and_keys_move_and_erase() {
        let mut edit = Edit::new();
        typed(&mut edit, "egg", 24);
        assert_eq!(edit.text(), "egg".encode_utf16().collect::<Vec<_>>());
        assert_eq!(edit.caret(), 3);
        assert_eq!(edit.key(key::LEFT, 24), EditOutcome::Moved);
        assert_eq!(edit.character(u32::from('x'), 24), EditOutcome::Changed);
        assert_eq!(edit.text(), "egxg".encode_utf16().collect::<Vec<_>>());
        assert_eq!(edit.key(key::BACK, 24), EditOutcome::Changed);
        assert_eq!(edit.key(key::DELETE, 24), EditOutcome::Changed);
        assert_eq!(edit.text(), "eg".encode_utf16().collect::<Vec<_>>());
        assert_eq!(edit.key(key::DELETE, 24), EditOutcome::Unchanged);
        assert_eq!(edit.key(key::HOME, 24), EditOutcome::Moved);
        assert_eq!(edit.character(0x0d, 24), EditOutcome::Unchanged);
        edit.read_only = true;
        assert_eq!(edit.character(u32::from('a'), 24), EditOutcome::Unchanged);
        assert_eq!(edit.key(key::END, 24), EditOutcome::Moved);
    }

    #[test]
    fn the_edit_scrolls_in_whole_cells_to_keep_the_caret_visible() {
        let mut edit = Edit::new();
        typed(&mut edit, &"a".repeat(40), 24);
        assert_eq!(edit.offset(), 17);
        edit.key(key::HOME, 24);
        assert_eq!(edit.offset(), 0);
        edit.key(key::END, 24);
        assert_eq!(edit.offset(), 17);
        edit.set_text(&[]);
        assert_eq!((edit.caret(), edit.offset()), (0, 0));
    }

    #[test]
    fn a_click_puts_the_caret_at_the_nearest_boundary() {
        let mut edit = Edit::new();
        typed(&mut edit, "egg", 24);
        edit.click(4 + 9, false, 24);
        assert_eq!(edit.caret(), 1);
        edit.click(4 + 12, false, 24);
        assert_eq!(edit.caret(), 2);
        edit.click(500, false, 24);
        assert_eq!(edit.caret(), 3);
    }

    #[test]
    fn a_focused_light_edit_has_the_blue_border_and_a_steady_caret_after_the_text() {
        let mut edit = Edit::new();
        typed(&mut edit, "egg", visible_cells(200));
        let mut pixels = vec![0u32; 200 * 24];
        let mut surface = Surface::new(&mut pixels, 200, 24).unwrap();
        let look = EditLook {
            focused: true,
            enabled: true,
        };
        paint(&mut surface, Rect::new(0, 0, 200, 24), &edit, look, &LIGHT);
        assert_eq!(surface.get(0, 0), Some(0x0000_78d7));
        assert_eq!(surface.get(28, 4), Some(0));
        assert_eq!(surface.get(28, 19), Some(0));
        assert_eq!(surface.get(29, 10), Some(0x00ff_ffff));
        let unfocused = EditLook {
            focused: false,
            ..look
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 200, 24),
            &edit,
            unfocused,
            &LIGHT,
        );
        assert_eq!(surface.get(0, 0), Some(0x007a_7a7a));
        assert_eq!(surface.get(28, 4), Some(0x00ff_ffff));
    }

    fn text_of(edit: &Edit) -> String {
        String::from_utf16(edit.text()).unwrap()
    }

    const SHIFT: Modifiers = Modifiers {
        shift: true,
        control: false,
    };
    const CONTROL: Modifiers = Modifiers {
        shift: false,
        control: true,
    };

    #[test]
    fn shift_extends_and_typing_replaces_the_selection() {
        let mut edit = Edit::new();
        typed(&mut edit, "hello world", 24);
        edit.key_with(key::HOME, SHIFT, 24);
        assert_eq!(edit.selection(), (0, 11));
        edit.key_with(key::RIGHT, SHIFT, 24);
        assert_eq!(edit.selection(), (1, 11));
        assert_eq!(edit.caret(), 1);
        typed(&mut edit, "J", 24);
        assert_eq!(text_of(&edit), "hJ");
        edit.key_with(key::A, CONTROL, 24);
        assert_eq!(edit.selection(), (0, 2));
        assert_eq!(edit.key(key::BACK, 24), EditOutcome::Changed);
        assert_eq!(text_of(&edit), "");
    }

    #[test]
    fn control_arrows_move_by_words_and_left_collapses_a_selection_to_its_start() {
        let mut edit = Edit::new();
        typed(&mut edit, "one, two_2  three", 32);
        edit.key(key::HOME, 32);
        edit.key_with(key::RIGHT, CONTROL, 32);
        assert_eq!(edit.caret(), 3);
        edit.key_with(key::RIGHT, CONTROL, 32);
        assert_eq!(edit.caret(), 5);
        edit.key_with(key::RIGHT, CONTROL, 32);
        assert_eq!(edit.caret(), 12);
        edit.key_with(key::LEFT, CONTROL, 32);
        assert_eq!(edit.caret(), 5);
        edit.key_with(key::END, SHIFT, 32);
        assert_eq!(edit.key(key::LEFT, 32), EditOutcome::Moved);
        assert_eq!(edit.selection(), (5, 5));
    }

    #[test]
    fn set_selection_follows_em_setsel_and_replace_selection_keeps_the_limit() {
        let mut edit = Edit::new();
        typed(&mut edit, "abcdef", 24);
        edit.set_selection(0, -1, 24);
        assert_eq!(edit.selection(), (0, 6));
        edit.set_selection(-1, 0, 24);
        assert_eq!(edit.selection(), (6, 6));
        edit.set_selection(4, 2, 24);
        assert_eq!((edit.selection(), edit.caret()), ((2, 4), 2));
        edit.set_limit(5);
        let x: Vec<u16> = "XYZ".encode_utf16().collect();
        assert_eq!(edit.replace_selection(&x, 24), EditOutcome::Changed);
        assert_eq!(text_of(&edit), "abXef");
        assert_eq!(edit.replace_selection(&x, 24), EditOutcome::Unchanged);
    }

    #[test]
    fn number_edits_take_digits_and_password_edits_draw_stars() {
        let mut edit = Edit::with_style(style::NUMBER | style::PASSWORD);
        typed(&mut edit, "1a2", 24);
        assert_eq!(text_of(&edit), "12");
        let mut pixels = vec![0u32; 100 * 24];
        let mut surface = Surface::new(&mut pixels, 100, 24).unwrap();
        let look = EditLook {
            focused: false,
            enabled: true,
        };
        paint(&mut surface, Rect::new(0, 0, 100, 24), &edit, look, &LIGHT);
        let mut star = vec![0u32; 8 * 16];
        let mut star_surface = Surface::new(&mut star, 8, 16).unwrap();
        star_surface.fill(Rect::new(0, 0, 8, 16), 0x00ff_ffff);
        text::glyph(&mut star_surface, 0, 0, font::glyph_id(0x2a), false, 0, None);
        for y in 0..16 {
            for x in 0..8 {
                assert_eq!(surface.get(4 + x, 4 + y), star_surface.get(x, y));
                assert_eq!(surface.get(12 + x, 4 + y), star_surface.get(x, y));
            }
        }
    }

    #[test]
    fn a_double_click_selects_the_word_and_a_drag_moves_only_the_caret() {
        let mut edit = Edit::new();
        typed(&mut edit, "big egg.txt", 24);
        assert_eq!(edit.select_word(4 + 8 * 5, 24), EditOutcome::Moved);
        assert_eq!(edit.selection(), (4, 7));
        edit.click(4, false, 24);
        edit.drag(4 + 8 * 3, 24);
        assert_eq!((edit.selection(), edit.caret()), ((0, 3), 3));
        edit.click(4 + 8 * 5, true, 24);
        assert_eq!(edit.selection(), (0, 5));
    }

    #[test]
    fn a_focused_selection_is_drawn_highlighted() {
        let mut edit = Edit::new();
        typed(&mut edit, "egg", 24);
        edit.set_selection(0, 1, 24);
        let mut pixels = vec![0u32; 100 * 24];
        let mut surface = Surface::new(&mut pixels, 100, 24).unwrap();
        let focused = EditLook {
            focused: true,
            enabled: true,
        };
        paint(&mut surface, Rect::new(0, 0, 100, 24), &edit, focused, &LIGHT);
        assert_eq!(surface.get(4, 4), Some(LIGHT.highlight));
        assert_eq!(surface.get(13, 4), Some(LIGHT.field_fill));
        let unfocused = EditLook {
            focused: false,
            ..focused
        };
        paint(&mut surface, Rect::new(0, 0, 100, 24), &edit, unfocused, &LIGHT);
        assert_eq!(surface.get(4, 4), Some(LIGHT.field_fill));
    }
}

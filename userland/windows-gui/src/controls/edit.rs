//! `EDIT`, single line (section 3 of the design): the [`Edit`] model (its text, caret and
//! scroll in whole cells) and its painting.

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
    pub const READ_ONLY: u32 = 0x0800;
}

/// An edit's text and caret.
#[derive(Debug, Clone)]
pub struct Edit {
    units: [u16; EDIT_CAPACITY],
    length: usize,
    caret: usize,
    offset: usize,
    limit: usize,
    pub read_only: bool,
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
            caret: 0,
            offset: 0,
            limit: EDIT_CAPACITY,
            read_only: false,
        }
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
        self.caret = 0;
        self.offset = 0;
    }

    /// Puts the caret after the text, as the picker does after filling in a name.
    pub fn caret_to_end(&mut self, visible: usize) {
        self.caret = self.length;
        self.scroll(visible);
    }

    /// A typed character: a printable one is inserted at the caret.
    pub fn character(&mut self, character: u32, visible: usize) -> EditOutcome {
        let printable = character >= 0x20 && character != 0x7f && character <= 0xffff;
        if !printable || self.read_only || self.length >= self.limit {
            return EditOutcome::Unchanged;
        }
        self.units
            .copy_within(self.caret..self.length, self.caret + 1);
        self.units[self.caret] = character as u16;
        self.length += 1;
        self.caret += 1;
        self.scroll(visible);
        EditOutcome::Changed
    }

    /// A key press: Backspace, Delete and the caret's moves.
    pub fn key(&mut self, virtual_key: u32, visible: usize) -> EditOutcome {
        let outcome = match virtual_key {
            key::BACK if !self.read_only && self.caret > 0 => {
                self.units
                    .copy_within(self.caret..self.length, self.caret - 1);
                self.length -= 1;
                self.caret -= 1;
                EditOutcome::Changed
            }
            key::DELETE if !self.read_only && self.caret < self.length => {
                self.units
                    .copy_within(self.caret + 1..self.length, self.caret);
                self.length -= 1;
                EditOutcome::Changed
            }
            key::LEFT => self.move_to(self.caret.saturating_sub(1)),
            key::RIGHT => self.move_to((self.caret + 1).min(self.length)),
            key::HOME => self.move_to(0),
            key::END => self.move_to(self.length),
            _ => EditOutcome::Unchanged,
        };
        self.scroll(visible);
        outcome
    }

    fn move_to(&mut self, caret: usize) -> EditOutcome {
        match caret == self.caret {
            true => EditOutcome::Unchanged,
            false => {
                self.caret = caret;
                EditOutcome::Moved
            }
        }
    }

    /// A click at `x` pixels from the edit's left: the caret at the nearest cell boundary.
    pub fn click(&mut self, x: i32, visible: usize) -> EditOutcome {
        let cells = (x - TEXT_INSET + CELL_WIDTH / 2).max(0) / CELL_WIDTH;
        let caret = (self.offset + cells as usize).min(self.length);
        let outcome = self.move_to(caret);
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
    let shown = &edit.text()[edit.offset.min(edit.length)..];
    for (index, character) in char::decode_utf16(shown.iter().copied())
        .take(visible)
        .enumerate()
    {
        let id = character.map_or(font::REPLACEMENT, |character| {
            font::glyph_id(u32::from(character))
        });
        let x = rect.left + TEXT_INSET + index as i32 * CELL_WIDTH;
        text::glyph(surface, x, top, id, false, colour, None);
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
        edit.click(4 + 9, 24);
        assert_eq!(edit.caret(), 1);
        edit.click(4 + 12, 24);
        assert_eq!(edit.caret(), 2);
        edit.click(500, 24);
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
}

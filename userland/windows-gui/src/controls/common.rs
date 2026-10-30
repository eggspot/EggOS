//! What comctl32's common controls (D178, `docs/ux/windows-controls-v0.3.md` sections 20-28)
//! share: the palette roles section 20 adds to section 0's, the images their rows, items, parts
//! and buttons show (drawn by the layer from its image lists through [`Images`]), and one line
//! of plain text cut with `...` at its room.

use super::{MAX_LABEL_CELLS, draw_fitted};
use crate::surface::Surface;
use crate::text::{self, Cell, Reading};

/// The colours section 20 adds, with the Light and Dark values of its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonPalette {
    pub selection_inactive: u32,
    pub selection_inactive_text: u32,
    pub bar_track: u32,
    pub bar_border: u32,
    pub bar_fill: u32,
    pub bar_error: u32,
    pub bar_paused: u32,
    pub tab_fill: u32,
    pub tab_hover_fill: u32,
    pub tab_border: u32,
    pub page_fill: u32,
    pub header_fill: u32,
    pub header_hover_fill: u32,
    pub header_pressed_fill: u32,
    pub header_line: u32,
    pub tree_line: u32,
    pub button_hover_fill: u32,
    pub button_hover_border: u32,
}

/// Inside a program (Windows 10's themed constants).
pub const COMMON_LIGHT: CommonPalette = CommonPalette {
    selection_inactive: 0x00d9_d9d9,
    selection_inactive_text: 0x0000_0000,
    bar_track: 0x00e6_e6e6,
    bar_border: 0x00bc_bcbc,
    bar_fill: 0x0006_b025,
    bar_error: 0x00e8_1123,
    bar_paused: 0x00ff_b900,
    tab_fill: 0x00f0_f0f0,
    tab_hover_fill: 0x00d8_eaf9,
    tab_border: 0x00d9_d9d9,
    page_fill: 0x00ff_ffff,
    header_fill: 0x00ff_ffff,
    header_hover_fill: 0x00d9_ebf9,
    header_pressed_fill: 0x00bc_dcf4,
    header_line: 0x00e5_e5e5,
    tree_line: 0x00a0_a0a0,
    button_hover_fill: 0x00e5_f1fb,
    button_hover_border: 0x0000_78d7,
};

/// The desktop's dark tokens, for completeness: common controls only live inside programs.
pub const COMMON_DARK: CommonPalette = CommonPalette {
    selection_inactive: 0x0026_303f,
    selection_inactive_text: 0x00e6_eaf0,
    bar_track: 0x0026_303f,
    bar_border: 0x003a_4454,
    bar_fill: 0x00f2_b33d,
    bar_error: 0x00e8_1123,
    bar_paused: 0x00ff_b900,
    tab_fill: 0x002a_3440,
    tab_hover_fill: 0x0034_4050,
    tab_border: 0x003a_4454,
    page_fill: 0x001c_2430,
    header_fill: 0x000b_0e13,
    header_hover_fill: 0x0034_4050,
    header_pressed_fill: 0x0020_2833,
    header_line: 0x003a_4454,
    tree_line: 0x008a_94a6,
    button_hover_fill: 0x0034_4050,
    button_hover_border: 0x008a_94a6,
};

/// The images a control shows, by image list and index, drawn by section 27's rule: the layer
/// keeps the lists, and host tests and the boot test's render give their own.
pub trait Images {
    /// The size of every image of `list`; `None` when there is no such list.
    fn size(&self, list: usize) -> Option<(i32, i32)>;
    /// Draws image `index` of `list` with its top-left at (`x`, `y`), greyed by section 27's
    /// formula when `greyed`; nothing when there is no such image.
    fn draw(
        &self,
        surface: &mut Surface<'_>,
        list: usize,
        index: i32,
        x: i32,
        y: i32,
        greyed: bool,
    );
}

/// No images at all: what a control shows when nothing gave it a list.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoImages;

impl Images for NoImages {
    fn size(&self, _list: usize) -> Option<(i32, i32)> {
        None
    }

    fn draw(&self, _: &mut Surface<'_>, _: usize, _: i32, _: i32, _: i32, _: bool) {}
}

/// The cells of one line of a common control's text: no prefixes (`&` is a glyph), tabs and line
/// ends as spaces and nothing; how many it filled.
pub fn plain_cells(units: &[u16], cells: &mut [Cell; MAX_LABEL_CELLS]) -> usize {
    let reading = Reading {
        prefixes: false,
        expand_tabs: false,
        single_line: true,
    };
    text::decode(units, reading, cells)
}

/// Draws `units` as one line from (`x`, `y`), cut with `...` at `room` pixels (section 1's
/// rule); the cells drawn.
pub fn draw_plain(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    units: &[u16],
    room: i32,
    colour: u32,
) -> usize {
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = plain_cells(units, &mut cells);
    draw_fitted(surface, x, y, &cells[..count], room, colour)
}

/// How many cells `units` takes as one line of plain text.
pub fn plain_width_cells(units: &[u16]) -> usize {
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    plain_cells(units, &mut cells)
}

/// `SLOTS` texts sharing one arena of `UNITS` UTF-16 units, each slot empty until set: a
/// control's part, tab or button texts.
#[derive(Debug, Clone)]
pub struct TextArena<const UNITS: usize, const SLOTS: usize> {
    units: [u16; UNITS],
    used: usize,
    runs: [(u16, u16); SLOTS],
}

impl<const UNITS: usize, const SLOTS: usize> Default for TextArena<UNITS, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const UNITS: usize, const SLOTS: usize> TextArena<UNITS, SLOTS> {
    pub const fn new() -> Self {
        Self {
            units: [0; UNITS],
            used: 0,
            runs: [(0, 0); SLOTS],
        }
    }

    /// Slot `slot`'s text; empty past the slots.
    pub fn get(&self, slot: usize) -> &[u16] {
        match self.runs.get(slot) {
            Some(&(start, length)) => {
                &self.units[usize::from(start)..usize::from(start) + usize::from(length)]
            }
            None => &[],
        }
    }

    fn release(&mut self, slot: usize) {
        let (start, length) = self.runs[slot];
        let (start, length) = (usize::from(start), usize::from(length));
        if length == 0 {
            return;
        }
        self.units.copy_within(start + length..self.used, start);
        self.used -= length;
        for run in &mut self.runs {
            if usize::from(run.0) > start {
                run.0 -= length as u16;
            }
        }
        self.runs[slot] = (0, 0);
    }

    /// Gives slot `slot` the text `units`; false, with the slot emptied, when there is no such
    /// slot or no room.
    pub fn set(&mut self, slot: usize, units: &[u16]) -> bool {
        if slot >= SLOTS {
            return false;
        }
        self.release(slot);
        let length = units.len().min(u16::MAX as usize);
        if self.used + length > UNITS {
            return false;
        }
        self.units[self.used..self.used + length].copy_from_slice(&units[..length]);
        self.runs[slot] = (self.used as u16, length as u16);
        self.used += length;
        true
    }

    /// Moves slots `slot..` one up (the last one's text is lost) and gives `slot` the text
    /// `units`; as [`TextArena::set`].
    pub fn insert(&mut self, slot: usize, units: &[u16]) -> bool {
        if slot >= SLOTS {
            return false;
        }
        self.release(SLOTS - 1);
        self.runs.copy_within(slot..SLOTS - 1, slot + 1);
        self.runs[slot] = (0, 0);
        self.set(slot, units)
    }

    /// Takes slot `slot`'s text out and moves the slots after it one down.
    pub fn remove(&mut self, slot: usize) {
        if slot >= SLOTS {
            return;
        }
        self.release(slot);
        self.runs.copy_within(slot + 1..SLOTS, slot);
        self.runs[SLOTS - 1] = (0, 0);
    }

    /// Empties every slot.
    pub fn clear(&mut self) {
        self.used = 0;
        self.runs = [(0, 0); SLOTS];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Rect;

    #[test]
    fn plain_text_keeps_ampersands_and_is_cut_at_its_room() {
        let units: Vec<u16> = "A&B".encode_utf16().collect();
        assert_eq!(plain_width_cells(&units), 3);
        let mut pixels = vec![0u32; 64 * 16];
        let mut surface = Surface::new(&mut pixels, 64, 16).unwrap();
        let long: Vec<u16> = "abcdefghij".encode_utf16().collect();
        assert_eq!(draw_plain(&mut surface, 0, 0, &long, 40, 1), 5);
        assert!(surface.drawn().right <= 40);
        assert_eq!(NoImages.size(0), None);
        NoImages.draw(&mut surface, 0, 0, 0, 0, false);
        assert_eq!(Rect::sized(0, 0, 1, 1).width(), 1);
    }

    #[test]
    fn an_arenas_slots_are_set_inserted_and_removed_in_place() {
        let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
        let mut arena: TextArena<16, 4> = TextArena::new();
        assert!(arena.set(0, &units("one")));
        assert!(arena.set(1, &units("two")));
        assert!(arena.set(0, &units("first")));
        assert_eq!(arena.get(0), units("first"));
        assert_eq!(arena.get(1), units("two"));
        assert!(arena.insert(1, &units("mid")));
        assert_eq!(arena.get(2), units("two"));
        arena.remove(0);
        assert_eq!(arena.get(0), units("mid"));
        assert_eq!(arena.get(1), units("two"));
        assert!(arena.get(3).is_empty());
        assert!(!arena.set(2, &units("far too long a text")));
        assert!(arena.get(2).is_empty());
        assert!(!arena.set(4, &units("x")));
        arena.clear();
        assert!(arena.get(0).is_empty());
    }
}

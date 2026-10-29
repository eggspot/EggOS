//! The system's controls (`BUTTON`, `EDIT`, `STATIC`, `LISTBOX`) as `docs/ux/windows-controls-v0.3.md`
//! designs them (D145): each drawn by one function from a [`Palette`] (light inside a program,
//! dark in EggOS's own dialogs), and the state each one keeps as a model the layer drives and
//! the boot test's host render paints from.

pub mod button;
pub mod edit;
pub mod label;
pub mod list_box;

use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_HEIGHT, CELL_WIDTH, Cell, Reading};

/// The longest label a control shows, in cells.
pub const MAX_LABEL_CELLS: usize = 256;

const INSET_R4: [i32; 4] = [2, 1, 0, 0];
const INNER_CORNER: [i32; 1] = [1];
const ELLIPSIS_CELLS: usize = 3;
const FEWEST_CELLS_FOR_ELLIPSIS: usize = 4;

/// The colours and shapes controls are drawn with (section 0 of the design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub rounded: bool,
    pub surface: u32,
    pub text: u32,
    pub text_disabled: u32,
    pub caret: u32,
    pub button_fill: u32,
    pub button_border: u32,
    pub button_pressed_fill: u32,
    pub button_pressed_border: u32,
    pub button_disabled_fill: u32,
    pub button_disabled_border: u32,
    pub default_fill: u32,
    pub default_border: Option<u32>,
    pub default_text: u32,
    pub default_pressed_fill: u32,
    pub field_fill: u32,
    pub field_border: u32,
    pub field_focus_border: u32,
    pub field_read_only_fill: u32,
    pub field_read_only_border: u32,
    pub box_fill: u32,
    pub box_border: u32,
    pub box_mark: u32,
    pub box_checked_fill: u32,
    pub box_pressed_fill: u32,
    pub box_disabled_fill: u32,
    pub box_disabled_border: u32,
    pub box_disabled_mark: u32,
    pub highlight: u32,
    pub highlight_text: u32,
    pub focus: u32,
    pub menu_bar: u32,
    pub menu_fill: u32,
    pub menu_border: u32,
    pub menu_separator: u32,
    pub menu_highlight: u32,
    pub menu_text: u32,
    pub menu_text_disabled: u32,
}

/// Inside a program: Win32's light colours.
pub const LIGHT: Palette = Palette {
    rounded: false,
    surface: 0x00f0_f0f0,
    text: 0x0000_0000,
    text_disabled: 0x006d_6d6d,
    caret: 0x0000_0000,
    button_fill: 0x00e1_e1e1,
    button_border: 0x00ad_adad,
    button_pressed_fill: 0x00cc_e4f7,
    button_pressed_border: 0x0000_5499,
    button_disabled_fill: 0x00cc_cccc,
    button_disabled_border: 0x00bf_bfbf,
    default_fill: 0x00e1_e1e1,
    default_border: Some(0x0000_78d7),
    default_text: 0x0000_0000,
    default_pressed_fill: 0x00cc_e4f7,
    field_fill: 0x00ff_ffff,
    field_border: 0x007a_7a7a,
    field_focus_border: 0x0000_78d7,
    field_read_only_fill: 0x00f0_f0f0,
    field_read_only_border: 0x00cc_cccc,
    box_fill: 0x00ff_ffff,
    box_border: 0x0033_3333,
    box_mark: 0x0000_0000,
    box_checked_fill: 0x00ff_ffff,
    box_pressed_fill: 0x00cc_e4f7,
    box_disabled_fill: 0x00f0_f0f0,
    box_disabled_border: 0x00bf_bfbf,
    box_disabled_mark: 0x006d_6d6d,
    highlight: 0x0000_78d7,
    highlight_text: 0x00ff_ffff,
    focus: 0x0000_0000,
    menu_bar: 0x00f0_f0f0,
    menu_fill: 0x00f0_f0f0,
    menu_border: 0x00a0_a0a0,
    menu_separator: 0x00d7_d7d7,
    menu_highlight: 0x0091_c9f7,
    menu_text: 0x0000_0000,
    menu_text_disabled: 0x006d_6d6d,
};

/// In EggOS's own dialogs: the desktop's dark tokens.
pub const DARK: Palette = Palette {
    rounded: true,
    surface: 0x001c_2430,
    text: 0x00e6_eaf0,
    text_disabled: 0x008a_94a6,
    caret: 0x00e6_eaf0,
    button_fill: 0x002a_3440,
    button_border: 0x003a_4454,
    button_pressed_fill: 0x0020_2833,
    button_pressed_border: 0x008a_94a6,
    button_disabled_fill: 0x002a_3440,
    button_disabled_border: 0x003a_4454,
    default_fill: 0x00f2_b33d,
    default_border: None,
    default_text: 0x000b_0e13,
    default_pressed_fill: 0x00d9_9e2e,
    field_fill: 0x000b_0e13,
    field_border: 0x003a_4454,
    field_focus_border: 0x003a_4454,
    field_read_only_fill: 0x000b_0e13,
    field_read_only_border: 0x003a_4454,
    box_fill: 0x000b_0e13,
    box_border: 0x003a_4454,
    box_mark: 0x000b_0e13,
    box_checked_fill: 0x00f2_b33d,
    box_pressed_fill: 0x0020_2833,
    box_disabled_fill: 0x002a_3440,
    box_disabled_border: 0x008a_94a6,
    box_disabled_mark: 0x008a_94a6,
    highlight: 0x002e_4a6b,
    highlight_text: 0x00e6_eaf0,
    focus: 0x00f2_b33d,
    menu_bar: 0x0014_1b26,
    menu_fill: 0x001c_2430,
    menu_border: 0x002a_3340,
    menu_separator: 0x002a_3340,
    menu_highlight: 0x002a_3440,
    menu_text: 0x00e6_eaf0,
    menu_text_disabled: 0x008a_94a6,
};

/// The dark palette's focus ring on a default button.
pub const DARK_RING_ON_PRIMARY: u32 = 0x000b_0e13;

/// The cell top of one line of text centred in a control `height` tall at `top`.
pub fn cell_top(top: i32, height: i32) -> i32 {
    top.saturating_add((height - CELL_HEIGHT).max(0) / 2)
}

/// A label's cells: `&` underlines the next glyph unless `prefixes` is off, one line.
pub fn label_cells(units: &[u16], prefixes: bool, cells: &mut [Cell; MAX_LABEL_CELLS]) -> usize {
    let reading = Reading {
        prefixes,
        expand_tabs: false,
        single_line: true,
    };
    text::decode(units, reading, cells)
}

/// The cells of `cells` that fit `room` pixels, as the design cuts a label: all of them when
/// they fit; else, with `k = room / 8` cells, the first `k - 3` and `...` when `k >= 4`, the
/// first `k` otherwise. Returns how many of `cells` show and whether `...` follows.
pub fn fit_cells(count: usize, room: i32) -> (usize, bool) {
    let fit = (room.max(0) / CELL_WIDTH) as usize;
    match () {
        () if count <= fit => (count, false),
        () if fit >= FEWEST_CELLS_FOR_ELLIPSIS => (fit - ELLIPSIS_CELLS, true),
        () => (fit, false),
    }
}

/// Draws `cells` from (`x`, `y`) cut to `room` pixels; returns the cells drawn, `...` included.
pub fn draw_fitted(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    cells: &[Cell],
    room: i32,
    colour: u32,
) -> usize {
    let (shown, ellipsis) = fit_cells(cells.len(), room);
    text::draw_cells(surface, x, y, &cells[..shown], colour, None);
    if !ellipsis {
        return shown;
    }
    let dots = [Cell::Glyph {
        id: b'.',
        underline: false,
    }; ELLIPSIS_CELLS];
    let after = x.saturating_add(shown as i32 * CELL_WIDTH);
    text::draw_cells(surface, after, y, &dots, colour, None);
    shown + ELLIPSIS_CELLS
}

/// A 1 px outline of `rect`.
pub fn outline(surface: &mut Surface<'_>, rect: Rect, colour: u32) {
    if rect.is_empty() {
        return;
    }
    surface.fill(
        Rect::new(rect.left, rect.top, rect.right, rect.top + 1),
        colour,
    );
    surface.fill(
        Rect::new(rect.left, rect.bottom - 1, rect.right, rect.bottom),
        colour,
    );
    surface.fill(
        Rect::new(rect.left, rect.top, rect.left + 1, rect.bottom),
        colour,
    );
    surface.fill(
        Rect::new(rect.right - 1, rect.top, rect.right, rect.bottom),
        colour,
    );
}

/// The light palette's focus indicator: the pixels of `rect`'s 1 px outline whose `x + y` is
/// even in the buffer's coordinates.
pub fn dotted_outline(surface: &mut Surface<'_>, rect: Rect, colour: u32) {
    if rect.is_empty() {
        return;
    }
    let (ox, oy) = surface.origin();
    let dot = |surface: &mut Surface<'_>, x: i32, y: i32| {
        if (x + ox + y + oy).rem_euclid(2) == 0 {
            surface.pixel(x, y, colour);
        }
    };
    for x in rect.left..rect.right {
        dot(surface, x, rect.top);
        dot(surface, x, rect.bottom - 1);
    }
    for y in rect.top + 1..rect.bottom - 1 {
        dot(surface, rect.left, y);
        dot(surface, rect.right - 1, y);
    }
}

/// The dark palette's focus ring: a 2 px outline with `INSET_R4` corners.
pub fn ring(surface: &mut Surface<'_>, rect: Rect, colour: u32) {
    rounded_outline(surface, rect, &INSET_R4, 2, &INNER_CORNER, colour);
}

/// The pixels of `rect` filled with `outer` corner insets but not of `rect.inset(width)` with
/// `inner` corner insets.
pub fn rounded_outline(
    surface: &mut Surface<'_>,
    rect: Rect,
    outer: &[i32],
    width: i32,
    inner: &[i32],
    colour: u32,
) {
    let hole = rect.inset(width);
    for y in rect.top..rect.bottom {
        let (left, right) = row_span(rect, outer, y);
        if y < hole.top || y >= hole.bottom || hole.is_empty() {
            surface.span(y, left, right, colour);
            continue;
        }
        let (hole_left, hole_right) = row_span(hole, inner, y);
        surface.span(y, left, hole_left, colour);
        surface.span(y, hole_right, right, colour);
    }
}

fn row_span(rect: Rect, insets: &[i32], y: i32) -> (i32, i32) {
    let offset = y - rect.top;
    let from_bottom = rect.bottom - 1 - y;
    let inset = match () {
        () if (offset as usize) < insets.len() => insets[offset as usize],
        () if (from_bottom as usize) < insets.len() => insets[from_bottom as usize],
        () => 0,
    };
    (rect.left + inset, rect.right - inset)
}

/// A filled frame: `fill` inside a 1 px `border`, square or with `INSET_R4` corners.
pub fn framed(surface: &mut Surface<'_>, rect: Rect, rounded: bool, border: u32, fill: u32) {
    match rounded {
        true => {
            surface.rounded(rect, &INSET_R4, border);
            surface.rounded(rect.inset(1), &INNER_CORNER, fill);
        }
        false => {
            surface.fill(rect, fill);
            outline(surface, rect, border);
        }
    }
}

/// Fills `rect`, square or with `INSET_R4` corners.
pub fn filled(surface: &mut Surface<'_>, rect: Rect, rounded: bool, fill: u32) {
    match rounded {
        true => surface.rounded(rect, &INSET_R4, fill),
        false => surface.fill(rect, fill),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_cut_with_an_ellipsis_only_when_four_cells_fit() {
        assert_eq!(fit_cells(5, 40), (5, false));
        assert_eq!(fit_cells(10, 40), (2, true));
        assert_eq!(fit_cells(10, 32), (1, true));
        assert_eq!(fit_cells(10, 24), (3, false));
        assert_eq!(fit_cells(10, -8), (0, false));
    }

    #[test]
    fn the_dotted_outline_follows_the_buffers_parity() {
        let mut pixels = [0u32; 6 * 6];
        let mut surface = Surface::new(&mut pixels, 6, 6).unwrap();
        surface.translate(1, 0);
        dotted_outline(&mut surface, Rect::new(0, 0, 4, 4), 9);
        assert_eq!(surface.get(0, 0), Some(0));
        assert_eq!(surface.get(1, 0), Some(9));
        assert_eq!(surface.get(0, 1), Some(9));
        assert_eq!(surface.get(1, 1), Some(0));
    }

    #[test]
    fn a_ring_leaves_its_inside_alone() {
        let mut pixels = [0u32; 10 * 10];
        let mut surface = Surface::new(&mut pixels, 10, 10).unwrap();
        ring(&mut surface, Rect::new(0, 0, 10, 10), 3);
        assert_eq!(surface.get(0, 0), Some(0));
        assert_eq!(surface.get(2, 0), Some(3));
        assert_eq!(surface.get(5, 1), Some(3));
        assert_eq!(surface.get(5, 2), Some(0));
        assert_eq!(surface.get(1, 5), Some(3));
        assert_eq!(surface.get(2, 5), Some(0));
        assert_eq!(surface.get(2, 2), Some(3));
    }
}

//! comctl32's tab control (section 23 of the design, D178): the [`TabControl`] model (tab
//! labels and data, the selection, `TCS_FIXEDWIDTH`'s width) with its geometry (the tab row
//! from `x + 2`, the selected tab 2 px larger on each side, `TCM_ADJUSTRECT`, hit testing), its
//! keys, and its painting: the unselected tabs, the page frame, then the selected tab joining
//! the page.

use super::common::{CommonPalette, TextArena, plain_cells};
use super::{MAX_LABEL_CELLS, Palette, dotted_outline, draw_fitted, fit_cells};
use crate::surface::{Rect, Surface};
use crate::text::{CELL_WIDTH, Cell};

/// The most tabs a control holds.
pub const MAX_TABS: usize = 64;
/// The tab row's height.
pub const ROW_HEIGHT: i32 = 24;
/// `TCS_FIXEDWIDTH`'s width until `TCM_SETITEMSIZE` sets one.
pub const DEFAULT_FIXED_WIDTH: i32 = 96;
const TEXT_UNITS: usize = 2048;
const ROW_LEFT: i32 = 2;
const TAB_TOP: i32 = 2;
const TAB_HEIGHT: i32 = 22;
const LABEL_LEFT: i32 = 8;
const LABEL_PADDING: i32 = 16;
const SELECTED_GROWTH: i32 = 2;
const FRAME_TOP: i32 = 23;
const FOCUS_INSET: i32 = 3;
const DISPLAY_INSET: i32 = 4;
const DISPLAY_TOP: i32 = 28;

/// The tab control's styles this layer reads.
pub mod style {
    pub const FIXED_WIDTH: u32 = 0x400;
    pub const FOCUS_NEVER: u32 = 0x8000;
}

/// The keys a tab control reads, as Windows' virtual keys.
pub mod key {
    pub const TAB: u32 = 0x09;
    pub const PAGE_UP: u32 = 0x21;
    pub const PAGE_DOWN: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const RIGHT: u32 = 0x27;
}

/// A tab control's tabs and selection.
#[derive(Debug, Clone)]
pub struct TabControl {
    texts: TextArena<TEXT_UNITS, MAX_TABS>,
    data: [usize; MAX_TABS],
    count: usize,
    selected: Option<usize>,
    pub fixed_width: i32,
    pub fixed_height: i32,
}

impl Default for TabControl {
    fn default() -> Self {
        Self::new()
    }
}

impl TabControl {
    pub const fn new() -> Self {
        Self {
            texts: TextArena::new(),
            data: [0; MAX_TABS],
            count: 0,
            selected: None,
            fixed_width: DEFAULT_FIXED_WIDTH,
            fixed_height: ROW_HEIGHT,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Tab `index`'s label.
    pub fn text(&self, index: usize) -> &[u16] {
        match index < self.count {
            true => self.texts.get(index),
            false => &[],
        }
    }

    /// Tab `index`'s data (`TCIF_PARAM`).
    pub fn data(&self, index: usize) -> Option<usize> {
        (index < self.count).then(|| self.data[index])
    }

    /// `TCM_INSERTITEMW` at `index` (the end when past it); the first tab is selected. Its
    /// index, `None` when the control is full.
    pub fn insert(&mut self, index: usize, label: &[u16], data: usize) -> Option<usize> {
        if self.count == MAX_TABS {
            return None;
        }
        let index = index.min(self.count);
        if !self.texts.insert(index, label) {
            self.texts.remove(index);
            return None;
        }
        self.data.copy_within(index..self.count, index + 1);
        self.data[index] = data;
        self.count += 1;
        self.selected = match self.selected {
            None if self.count == 1 => Some(0),
            Some(selected) if selected >= index => Some(selected + 1),
            other => other,
        };
        Some(index)
    }

    /// `TCM_SETITEMW`: a new label and data (each when given); false for no such tab or no
    /// room.
    pub fn set(&mut self, index: usize, label: Option<&[u16]>, data: Option<usize>) -> bool {
        if index >= self.count {
            return false;
        }
        if let Some(data) = data {
            self.data[index] = data;
        }
        label.is_none_or(|label| self.texts.set(index, label))
    }

    /// `TCM_DELETEITEM`; deleting the selected tab selects none. False for no such tab.
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.count {
            return false;
        }
        self.texts.remove(index);
        self.data.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.selected = match self.selected {
            Some(selected) if selected == index => None,
            Some(selected) if selected > index => Some(selected - 1),
            other => other,
        };
        true
    }

    /// `TCM_DELETEALLITEMS`.
    pub fn clear(&mut self) {
        self.texts.clear();
        self.count = 0;
        self.selected = None;
    }

    /// Selects tab `index` (`None` for none); the previous selection, `None` for no such tab.
    pub fn select(&mut self, index: Option<usize>) -> Option<Option<usize>> {
        match index {
            Some(index) if index >= self.count => None,
            index => Some(core::mem::replace(&mut self.selected, index)),
        }
    }

    /// Tab `index`'s width with `style`: its label's cells and 16, or the fixed width.
    pub fn tab_width(&self, index: usize, style: u32) -> i32 {
        match style & style::FIXED_WIDTH {
            0 => plain_width(self.text(index)) * CELL_WIDTH + LABEL_PADDING,
            _ => self.fixed_width,
        }
    }

    fn tab_left(&self, index: usize, style: u32) -> i32 {
        (0..index).fold(ROW_LEFT, |left, at| left + self.tab_width(at, style))
    }

    /// Tab `index`'s rectangle in a control `width` wide (from (0, 0)) as drawn unselected:
    /// `(tx, 2, tw, 22)`.
    pub fn plain_rect(&self, index: usize, style: u32) -> Option<Rect> {
        (index < self.count).then(|| {
            let left = self.tab_left(index, style);
            Rect::sized(left, TAB_TOP, self.tab_width(index, style), TAB_HEIGHT)
        })
    }

    /// Tab `index`'s rectangle as `TCM_GETITEMRECT` answers it: the selected tab's is 2 px
    /// larger on each side and reaches the control's top.
    pub fn item_rect(&self, index: usize, style: u32) -> Option<Rect> {
        let plain = self.plain_rect(index, style)?;
        Some(match self.selected == Some(index) {
            true => selected_rect(plain),
            false => plain,
        })
    }

    /// Whether tab `index` is drawn in a control `width` wide: it and every tab before it end
    /// inside.
    pub fn is_drawn(&self, index: usize, style: u32, width: i32) -> bool {
        self.plain_rect(index, style)
            .is_some_and(|rect| rect.right <= width)
    }

    /// The drawn tab under (`x`, `y`) of a control `width` wide, the selected one first.
    pub fn hit(&self, x: i32, y: i32, style: u32, width: i32) -> Option<usize> {
        let drawn = |index: usize| self.is_drawn(index, style, width);
        if let Some(selected) = self.selected.filter(|&selected| drawn(selected))
            && self
                .item_rect(selected, style)
                .is_some_and(|rect| rect.contains(x, y))
        {
            return Some(selected);
        }
        (0..self.count)
            .take_while(|&index| drawn(index))
            .find(|&index| {
                self.plain_rect(index, style)
                    .is_some_and(|rect| rect.contains(x, y))
            })
    }

    /// The tab a key press selects while the control is focused (`control` and `shift` held
    /// or not); `None` when the key selects nothing.
    pub fn key_target(&self, virtual_key: u32, control: bool, shift: bool) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let last = self.count - 1;
        let current = self.selected;
        let next_wrapping = || current.map_or(0, |at| if at >= last { 0 } else { at + 1 });
        let previous_wrapping = || current.map_or(last, |at| if at == 0 { last } else { at - 1 });
        match virtual_key {
            key::TAB if control && shift => Some(previous_wrapping()),
            key::TAB if control => Some(next_wrapping()),
            key::PAGE_DOWN if control => Some(next_wrapping()),
            key::PAGE_UP if control => Some(previous_wrapping()),
            key::LEFT => Some(current.map_or(0, |at| at.saturating_sub(1))),
            key::RIGHT => Some(current.map_or(0, |at| (at + 1).min(last))),
            key::HOME => Some(0),
            key::END => Some(last),
            _ => None,
        }
    }
}

fn plain_width(units: &[u16]) -> i32 {
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    plain_cells(units, &mut cells) as i32
}

fn selected_rect(plain: Rect) -> Rect {
    Rect::new(
        plain.left - SELECTED_GROWTH,
        0,
        plain.right + SELECTED_GROWTH,
        ROW_HEIGHT,
    )
}

/// `TCM_ADJUSTRECT`: the display rectangle of a control at `rect` (`larger` false), or the
/// control rectangle of a display rectangle (`larger` true).
pub fn adjust_rect(rect: Rect, larger: bool) -> Rect {
    match larger {
        false => Rect::new(
            rect.left + DISPLAY_INSET,
            rect.top + DISPLAY_TOP,
            rect.right - DISPLAY_INSET,
            rect.bottom - DISPLAY_INSET,
        ),
        true => Rect::new(
            rect.left - DISPLAY_INSET,
            rect.top - DISPLAY_TOP,
            rect.right + DISPLAY_INSET,
            rect.bottom + DISPLAY_INSET,
        ),
    }
}

/// How a tab control looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabLook {
    pub style: u32,
    pub focused: bool,
    pub enabled: bool,
}

fn three_lines(surface: &mut Surface<'_>, rect: Rect, colour: u32) {
    surface.fill(
        Rect::new(rect.left, rect.top, rect.right, rect.top + 1),
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

fn label(
    surface: &mut Surface<'_>,
    tabs: &TabControl,
    index: usize,
    (rect, top): (Rect, i32),
    look: TabLook,
    colour: u32,
) {
    let mut cells = [Cell::Space; MAX_LABEL_CELLS];
    let count = plain_cells(tabs.text(index), &mut cells);
    let room = rect.width() - LABEL_PADDING;
    let x = match look.style & style::FIXED_WIDTH {
        0 => rect.left + LABEL_LEFT,
        _ => {
            let (shown, ellipsis) = fit_cells(count, room);
            let cells = shown + if ellipsis { 3 } else { 0 };
            rect.left + (rect.width() - cells as i32 * CELL_WIDTH) / 2
        }
    };
    draw_fitted(surface, x, top, &cells[..count], room, colour);
}

/// Paints `tabs` over the whole of `rect` (a control whose top-left is (0, 0)): the unselected
/// tabs, the page frame filled `page.fill`, then the selected tab over it.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    tabs: &TabControl,
    look: TabLook,
    palette: &Palette,
    common: &CommonPalette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(
        Rect::new(rect.left, rect.top, rect.right, rect.top + FRAME_TOP),
        palette.surface,
    );
    let colour = match look.enabled {
        true => palette.text,
        false => palette.text_disabled,
    };
    let width = rect.width();
    let drawn = (0..tabs.count)
        .take_while(|&index| tabs.is_drawn(index, look.style, width))
        .count();
    for index in (0..drawn).filter(|&index| tabs.selected != Some(index)) {
        let Some(plain) = tabs.plain_rect(index, look.style) else {
            continue;
        };
        let plain = plain.offset(rect.left, rect.top);
        surface.fill(plain, common.tab_fill);
        three_lines(surface, plain, common.tab_border);
        label(surface, tabs, index, (plain, plain.top + 3), look, colour);
    }
    let frame = Rect::new(rect.left, rect.top + FRAME_TOP, rect.right, rect.bottom);
    surface.fill(frame.inset(1), common.page_fill);
    super::outline(surface, frame, common.tab_border);
    if let Some(selected) = tabs.selected.filter(|&selected| selected < drawn)
        && let Some(plain) = tabs.plain_rect(selected, look.style)
    {
        let tab = selected_rect(plain).offset(rect.left, rect.top);
        surface.fill(tab, common.page_fill);
        three_lines(surface, tab, common.tab_border);
        label(
            surface,
            tabs,
            selected,
            (plain.offset(rect.left, rect.top), tab.top + 4),
            look,
            colour,
        );
        if look.focused {
            dotted_outline(surface, tab.inset(FOCUS_INSET), palette.focus);
        }
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;
    use crate::controls::common::COMMON_LIGHT;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn example() -> TabControl {
        let mut tabs = TabControl::new();
        tabs.insert(0, &units("List"), 0);
        tabs.insert(1, &units("Tree"), 0);
        tabs
    }

    const RECT: Rect = Rect::new(8, 36, 8 + 464, 36 + 240);

    fn render(tabs: &TabControl, focused: bool) -> Vec<u32> {
        let mut pixels = vec![0u32; 480 * 360];
        let mut surface = Surface::new(&mut pixels, 480, 360).unwrap();
        surface.translate(RECT.left, RECT.top);
        let look = TabLook {
            style: 0,
            focused,
            enabled: true,
        };
        paint(
            &mut surface,
            Rect::new(0, 0, RECT.width(), RECT.height()),
            tabs,
            look,
            &LIGHT,
            &COMMON_LIGHT,
        );
        pixels
    }

    fn glyphs(text: &str, x: i32, y: i32) -> Vec<(i32, i32)> {
        let mut pixels = vec![1u32; 64 * 16];
        let mut surface = Surface::new(&mut pixels, 64, 16).unwrap();
        let mut cells = [Cell::Space; MAX_LABEL_CELLS];
        let count = plain_cells(&units(text), &mut cells);
        crate::text::draw_cells(&mut surface, 0, 0, &cells[..count], 0, None);
        (0..16)
            .flat_map(|row| (0..64).map(move |column| (column, row)))
            .filter(|&(column, row)| pixels[(row * 64 + column) as usize] == 0)
            .map(|(column, row)| (x + column, y + row))
            .collect()
    }

    #[test]
    fn the_first_tab_inserted_is_selected_and_the_example_row_is_drawn() {
        let tabs = example();
        assert_eq!(tabs.selected(), Some(0));
        let pixels = render(&tabs, false);
        let at = |x: i32, y: i32| pixels[(y * 480 + x) as usize];
        let border = 0x00d9_d9d9;
        assert!((8..60).all(|x| at(x, 36) == border));
        assert!((36..60).all(|y| at(8, y) == border && at(59, y) == border));
        assert_eq!(at(9, 37), 0x00ff_ffff);
        assert!((60..106).all(|x| at(x, 38) == border));
        assert!((38..60).all(|y| at(105, y) == border));
        assert_eq!(at(60, 39), 0x00f0_f0f0);
        assert_eq!(at(70, 37), 0x00f0_f0f0);
        assert!((9..59).all(|x| at(x, 59) == 0x00ff_ffff));
        assert!((59..472).all(|x| at(x, 59) == border));
        assert!((59..276).all(|y| at(8, y) == border && at(471, y) == border));
        assert!((8..472).all(|x| at(x, 275) == border));
        assert_eq!(at(200, 200), 0x00ff_ffff);
        for (x, y) in glyphs("List", 18, 40)
            .into_iter()
            .chain(glyphs("Tree", 66, 41))
        {
            assert_eq!(at(x, y), 0, "ink at ({x}, {y})");
        }
    }

    #[test]
    fn the_selected_tab_overlaps_its_neighbours_and_shows_the_focus() {
        let mut tabs = example();
        assert_eq!(tabs.select(Some(1)), Some(Some(0)));
        let pixels = render(&tabs, true);
        let at = |x: i32, y: i32| pixels[(y * 480 + x) as usize];
        let border = 0x00d9_d9d9;
        assert!((56..108).all(|x| at(x, 36) == border));
        assert!((10..56).all(|x| at(x, 38) == border));
        assert_eq!(at(57, 38), 0x00ff_ffff);
        assert_eq!(at(56, 45), border);
        assert_eq!(at(57, 45), 0x00ff_ffff);
        assert!((57..107).all(|x| at(x, 59) == 0x00ff_ffff));
        assert_eq!(at(56, 59), border);
        assert_eq!(at(107, 59), border);
        assert_eq!(at(9, 59), border);
        assert_eq!(at(59, 39), 0);
        assert_eq!(at(60, 39), 0x00ff_ffff);
        assert_eq!(at(104, 56), 0);
        for (x, y) in glyphs("Tree", 66, 40)
            .into_iter()
            .chain(glyphs("List", 18, 41))
        {
            assert_eq!(at(x, y), 0, "ink at ({x}, {y})");
        }
    }

    #[test]
    fn five_tabs_clip_the_last_and_rectangles_follow_the_row() {
        let mut tabs = TabControl::new();
        for name in ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"] {
            tabs.insert(usize::MAX, &units(name), 0);
        }
        assert_eq!(tabs.plain_rect(1, 0), Some(Rect::sized(58, 2, 48, 22)));
        assert_eq!(tabs.item_rect(0, 0), Some(Rect::new(0, 0, 60, 24)));
        assert!(tabs.is_drawn(3, 0, 280));
        assert!(!tabs.is_drawn(4, 0, 280));
        assert_eq!(tabs.hit(250, 10, 0, 280), None);
        assert_eq!(tabs.hit(250, 10, 0, 300), Some(4));
        assert_eq!(tabs.hit(59, 10, 0, 280), Some(0));
        assert_eq!(tabs.hit(61, 10, 0, 280), Some(1));
        assert_eq!(tabs.hit(61, 1, 0, 280), None);
        assert_eq!(
            tabs.plain_rect(1, style::FIXED_WIDTH),
            Some(Rect::sized(98, 2, 96, 22))
        );
        assert!(tabs.delete(0));
        assert_eq!(tabs.selected(), None);
        assert_eq!(tabs.text(0), units("Beta"));
    }

    #[test]
    fn adjust_rect_goes_both_ways() {
        let display = adjust_rect(RECT, false);
        assert_eq!(display, Rect::sized(12, 64, 456, 208));
        assert_eq!(adjust_rect(display, true), RECT);
    }

    #[test]
    fn control_tab_wraps_and_arrows_clamp() {
        let mut tabs = example();
        tabs.insert(2, &units("Third"), 0);
        assert_eq!(tabs.key_target(key::TAB, true, false), Some(1));
        tabs.select(Some(2));
        assert_eq!(tabs.key_target(key::TAB, true, false), Some(0));
        assert_eq!(tabs.key_target(key::PAGE_DOWN, true, false), Some(0));
        assert_eq!(tabs.key_target(key::RIGHT, false, false), Some(2));
        tabs.select(Some(0));
        assert_eq!(tabs.key_target(key::TAB, true, true), Some(2));
        assert_eq!(tabs.key_target(key::PAGE_UP, true, false), Some(2));
        assert_eq!(tabs.key_target(key::LEFT, false, false), Some(0));
        assert_eq!(tabs.key_target(key::END, false, false), Some(2));
        assert_eq!(tabs.key_target(key::TAB, false, false), None);
        assert_eq!(tabs.select(Some(5)), None);
    }
}

//! comctl32's list view, report view, and its header (section 24 of the design, D178): the
//! [`ListView`] model (rows of up to [`MAX_COLUMNS`] cells sharing one text store that is packed
//! again only when its end is full, each row's state, image and data, the [`Columns`] the header
//! shows, the first row shown and the horizontal scroll), its geometry and hit testing, the state
//! changes a selection asks for ([`Wanted`]), handed to the layer one row at a time
//! ([`ListView::next_change`]) so the program can refuse each, and the painting of the list, its
//! header and its horizontal scroll bar. The other views draw as this report view without the
//! header, one column as wide as the rows.

use super::button::tick;
use super::common::{CommonPalette, Images, TextArena, draw_plain, plain_width_cells};
use super::list_box::compare_names;
use super::scroll_bar::{
    self, CHEVRON_RIGHT, Chevron, Part, ScrollInfo, WIDTH as BAR, draw_chevron,
};
use super::{Palette, dotted_outline, fit_cells, outline};
use crate::surface::{Rect, Surface};
use crate::text::CELL_WIDTH;
use core::cmp::Ordering;

/// The most rows a list holds.
pub const MAX_ROWS: usize = 512;
/// The most columns a list or a header holds.
pub const MAX_COLUMNS: usize = 8;
/// The UTF-16 units all of a list's cells hold together.
pub const ROW_UNITS: usize = 32 * 1024;
/// The UTF-16 units all of a list's column titles hold together.
pub const COLUMN_UNITS: usize = 1024;
/// A row's height.
pub const ROW_HEIGHT: i32 = 20;
/// The header's height.
pub const HEADER_HEIGHT: i32 = 24;
/// The narrowest the first column gets.
pub const SMALLEST_FIRST_WIDTH: i32 = 12;
/// What a click on a scroll arrow moves a list by horizontally, in pixels.
pub const HORIZONTAL_LINE: i32 = 8;
const CELLS: usize = MAX_ROWS * MAX_COLUMNS;
const SLOT_BITS: u32 = 12;
const TEXT_INSET: i32 = 6;
const TEXT_PADDING: i32 = 2 * TEXT_INSET;
const CELL_TEXT_TOP: i32 = 2;
const HEADER_TEXT_TOP: i32 = 4;
const DIVIDER_TOP: i32 = 4;
const DIVIDER_HEIGHT: i32 = 16;
const PREFIX_STEP: i32 = 20;
const CHECK_BOX_LEFT: i32 = 3;
const CHECK_BOX_TOP: i32 = 3;
/// A check box's size, in the list and the tree alike.
pub const CHECK_BOX_SIZE: i32 = 14;
const IMAGE_LEFT: i32 = 2;
const IMAGE_TOP: i32 = 2;
const IMAGE_SIZE: i32 = 16;
const HORIZONTAL_CHEVRON_AT: (i32, i32) = (6, 4);
const ELLIPSIS_CELLS: usize = 3;

/// The 4 by 8 chevron pointing left.
pub const CHEVRON_LEFT: Chevron = Chevron {
    rows: [
        0b0001_0000,
        0b0011_0000,
        0b0110_0000,
        0b1100_0000,
        0b1100_0000,
        0b0110_0000,
        0b0011_0000,
        0b0001_0000,
    ],
    height: 8,
    width: 4,
};

/// The list view's styles this layer reads (`LVS_*`).
pub mod style {
    pub const ICON: u32 = 0x0;
    pub const REPORT: u32 = 0x1;
    pub const SMALL_ICON: u32 = 0x2;
    pub const LIST: u32 = 0x3;
    pub const TYPE_MASK: u32 = 0x3;
    pub const SINGLE_SELECTION: u32 = 0x4;
    pub const SHOW_SELECTION_ALWAYS: u32 = 0x8;
    pub const SORT_ASCENDING: u32 = 0x10;
    pub const SORT_DESCENDING: u32 = 0x20;
    pub const NO_SCROLL: u32 = 0x2000;
    pub const NO_COLUMN_HEADER: u32 = 0x4000;
    pub const NO_SORT_HEADER: u32 = 0x8000;
}

/// The extended styles this layer draws (`LVS_EX_*`).
pub mod extended {
    pub const GRID_LINES: u32 = 0x1;
    pub const CHECK_BOXES: u32 = 0x4;
    pub const FULL_ROW_SELECT: u32 = 0x20;
}

/// A row's state bits (`LVIS_*`).
pub mod state {
    pub const FOCUSED: u32 = 0x1;
    pub const SELECTED: u32 = 0x2;
    pub const STATE_IMAGE_MASK: u32 = 0xf000;
    /// The state image of an unchecked row with `LVS_EX_CHECKBOXES`.
    pub const UNCHECKED: u32 = 1;
    /// The state image of a checked row.
    pub const CHECKED: u32 = 2;
    const STATE_IMAGE_SHIFT: u32 = 12;

    /// `INDEXTOSTATEIMAGEMASK(image)`.
    pub const fn image_bits(image: u32) -> u32 {
        (image << STATE_IMAGE_SHIFT) & STATE_IMAGE_MASK
    }

    /// The state image a state names.
    pub const fn image_of(state: u32) -> u32 {
        (state & STATE_IMAGE_MASK) >> STATE_IMAGE_SHIFT
    }
}

/// A column's alignment (`LVCFMT_*`, which `HDF_*` shares).
pub mod format {
    pub const LEFT: u32 = 0x0;
    pub const RIGHT: u32 = 0x1;
    pub const CENTER: u32 = 0x2;
    pub const JUSTIFY_MASK: u32 = 0x3;
}

/// The keys a list reads, as Windows' virtual keys.
pub mod key {
    pub const RETURN: u32 = 0x0d;
    pub const SPACE: u32 = 0x20;
    pub const PAGE_UP: u32 = 0x21;
    pub const PAGE_DOWN: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const UP: u32 = 0x26;
    pub const DOWN: u32 = 0x28;
    pub const LETTER_A: u32 = 0x41;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Column {
    width: i32,
    format: u32,
    lparam: usize,
}

const NO_COLUMN: Column = Column {
    width: 0,
    format: 0,
    lparam: 0,
};

/// A header's items: a list view's columns, or a header window's own.
#[derive(Debug, Clone)]
pub struct Columns {
    titles: TextArena<COLUMN_UNITS, MAX_COLUMNS>,
    columns: [Column; MAX_COLUMNS],
    count: usize,
}

impl Default for Columns {
    fn default() -> Self {
        Self::new()
    }
}

impl Columns {
    pub const fn new() -> Self {
        Self {
            titles: TextArena::new(),
            columns: [NO_COLUMN; MAX_COLUMNS],
            count: 0,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Column `index`'s title; empty past the columns.
    pub fn text(&self, index: usize) -> &[u16] {
        match index < self.count {
            true => self.titles.get(index),
            false => &[],
        }
    }

    pub fn width(&self, index: usize) -> i32 {
        self.columns[..self.count]
            .get(index)
            .map_or(0, |column| column.width)
    }

    pub fn format(&self, index: usize) -> u32 {
        self.columns[..self.count]
            .get(index)
            .map_or(0, |column| column.format)
    }

    pub fn lparam(&self, index: usize) -> usize {
        self.columns[..self.count]
            .get(index)
            .map_or(0, |column| column.lparam)
    }

    fn clamped(index: usize, width: i32) -> i32 {
        match index {
            0 => width.max(SMALLEST_FIRST_WIDTH),
            _ => width.max(0),
        }
    }

    /// Inserts a column at `at` (the end when past it); its index, `None` when the header is
    /// full or its title does not fit.
    pub fn insert(
        &mut self,
        at: usize,
        text: &[u16],
        width: i32,
        format: u32,
        lparam: usize,
    ) -> Option<usize> {
        if self.count == MAX_COLUMNS {
            return None;
        }
        let at = at.min(self.count);
        if !self.titles.insert(at, text) {
            self.titles.remove(at);
            return None;
        }
        self.columns.copy_within(at..self.count, at + 1);
        self.columns[at] = Column {
            width: Self::clamped(at, width),
            format,
            lparam,
        };
        self.count += 1;
        Some(at)
    }

    /// Deletes column `index`; whether there was one.
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.count {
            return false;
        }
        self.titles.remove(index);
        self.columns.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.columns[self.count] = NO_COLUMN;
        if self.count > 0 {
            self.columns[0].width = Self::clamped(0, self.columns[0].width);
        }
        true
    }

    /// Gives column `index` the title `text`; false when there is no such column or no room.
    pub fn set_text(&mut self, index: usize, text: &[u16]) -> bool {
        index < self.count && self.titles.set(index, text)
    }

    /// Sets column `index`'s width (the first at least [`SMALLEST_FIRST_WIDTH`]).
    pub fn set_width(&mut self, index: usize, width: i32) -> bool {
        match self.columns[..self.count].get_mut(index) {
            Some(column) => {
                column.width = Self::clamped(index, width);
                true
            }
            None => false,
        }
    }

    pub fn set_format(&mut self, index: usize, format: u32) -> bool {
        match self.columns[..self.count].get_mut(index) {
            Some(column) => {
                column.format = format;
                true
            }
            None => false,
        }
    }

    pub fn set_lparam(&mut self, index: usize, lparam: usize) -> bool {
        match self.columns[..self.count].get_mut(index) {
            Some(column) => {
                column.lparam = lparam;
                true
            }
            None => false,
        }
    }

    /// The widths of the columns before `index`, together.
    pub fn left(&self, index: usize) -> i32 {
        self.columns[..index.min(self.count)]
            .iter()
            .map(|column| column.width)
            .sum()
    }

    /// Every column's width together: `T`.
    pub fn total(&self) -> i32 {
        self.left(self.count)
    }

    /// The column at `x` pixels from the first column's left edge.
    pub fn at(&self, x: i32) -> Option<usize> {
        let mut left = 0;
        for (index, column) in self.columns[..self.count].iter().enumerate() {
            if x >= left && x < left + column.width {
                return Some(index);
            }
            left += column.width;
        }
        None
    }

    /// Deletes every column.
    pub fn clear(&mut self) {
        self.titles.clear();
        self.columns = [NO_COLUMN; MAX_COLUMNS];
        self.count = 0;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Row {
    state: u32,
    image: i32,
    lparam: usize,
}

const NO_ROW: Row = Row {
    state: 0,
    image: 0,
    lparam: 0,
};

/// The selection and focus a user's input or a program's message wants: the focused row (the
/// focus leaves every other row) and the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub focus: Option<usize>,
    pub selection: Selection,
}

/// What a [`Wanted`] does to the rows' selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Leaves it as it is.
    Keep,
    /// Rows `first..=last` selected, every other row not.
    Only { first: usize, last: usize },
    /// Row `index` selected or not, the others as they are (with single selection, a row
    /// becoming selected deselects the others).
    Row { index: usize, selected: bool },
    /// Every row selected.
    All,
    /// No row selected.
    Clear,
    /// `LVM_SETITEMSTATE`: the bits of `mask` set to `state` on `row` (every row when `None`);
    /// focusing one row unfocuses the others, and with single selection selecting one deselects
    /// the others.
    Set {
        row: Option<usize>,
        state: u32,
        mask: u32,
    },
}

/// One row's state change, as its `LVN_ITEMCHANGING`/`LVN_ITEMCHANGED` pair carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateChange {
    pub index: usize,
    pub old: u32,
    pub new: u32,
    pub lparam: usize,
}

/// Where a list's parts are inside its rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLayout {
    /// The rectangle inside the border.
    pub inner: Rect,
    /// The header's rectangle; empty without one.
    pub header: Rect,
    /// Where rows are drawn: under the header, left of the vertical bar, above the horizontal.
    pub rows: Rect,
    /// `V`: the whole rows the rows area shows (at least 1).
    pub visible: usize,
    pub vertical: Option<Rect>,
    pub horizontal: Option<Rect>,
    /// `T`: the columns' total width (the rows area's width outside report view).
    pub total: i32,
    /// Whether the list draws as a report with its columns.
    pub report: bool,
}

/// What is at a point of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListHit {
    /// A row's cell, and which part of the first column's cell.
    Cell {
        index: usize,
        column: usize,
        part: CellPart,
    },
    /// The rows area where no row is: below the rows, or right of `T`; the column, if any.
    Empty {
        column: Option<usize>,
    },
    Vertical(Part),
    Horizontal(Part),
    Header,
    /// The border, the size box, or outside.
    Nowhere,
}

/// The part of a cell a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellPart {
    CheckBox,
    Image,
    Text,
}

/// A rectangle `LVM_GETITEMRECT` answers (`LVIR_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemPart {
    Bounds,
    Icon,
    Label,
}

/// A list's rows, columns, selection and scroll.
#[derive(Debug, Clone)]
pub struct ListView {
    units: [u16; ROW_UNITS],
    used: usize,
    cells: [(u16, u16); CELLS],
    rows: [Row; MAX_ROWS],
    count: usize,
    top: usize,
    mark: Option<usize>,
    columns: Columns,
    pub hpos: i32,
    pub extended: u32,
    pub small_images: usize,
    pub normal_images: usize,
    pub state_images: usize,
    pub background: Option<u32>,
    pub text_colour: Option<u32>,
    pub text_background: Option<u32>,
}

impl Default for ListView {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a list of `style` draws as a report with its columns and header.
pub fn is_report(style: u32) -> bool {
    style & style::TYPE_MASK == style::REPORT
}

/// Whether a list of `style` shows its header.
pub fn shows_header(style: u32) -> bool {
    is_report(style) && style & style::NO_COLUMN_HEADER == 0
}

fn with_bit(value: u32, bit: u32, on: bool) -> u32 {
    match on {
        true => value | bit,
        false => value & !bit,
    }
}

fn fold(unit: u32) -> u32 {
    match unit {
        0x41..=0x5a => unit + 0x20,
        unit => unit,
    }
}

const fn slot(index: usize, column: usize) -> usize {
    index * MAX_COLUMNS + column
}

impl ListView {
    pub const fn new() -> Self {
        Self {
            units: [0; ROW_UNITS],
            used: 0,
            cells: [(0, 0); CELLS],
            rows: [NO_ROW; MAX_ROWS],
            count: 0,
            top: 0,
            mark: None,
            columns: Columns::new(),
            hpos: 0,
            extended: 0,
            small_images: 0,
            normal_images: 0,
            state_images: 0,
            background: None,
            text_colour: None,
            text_background: None,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// The first row shown.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The selection mark: the anchor Shift extends from.
    pub fn mark(&self) -> Option<usize> {
        self.mark
    }

    pub fn set_mark(&mut self, mark: Option<usize>) {
        self.mark = mark.filter(|&mark| mark < self.count);
    }

    pub fn columns(&self) -> &Columns {
        &self.columns
    }

    /// The columns, for changes that keep their number (titles, widths, formats, data):
    /// inserting and deleting go through [`ListView::insert_column`] and
    /// [`ListView::delete_column`], which move the cells too.
    pub fn columns_mut(&mut self) -> &mut Columns {
        &mut self.columns
    }

    /// Row `index`'s text in `column`; empty when there is none.
    pub fn text(&self, index: usize, column: usize) -> &[u16] {
        if index >= self.count || column >= MAX_COLUMNS {
            return &[];
        }
        let (start, length) = self.cells[slot(index, column)];
        let start = usize::from(start);
        &self.units[start..start + usize::from(length)]
    }

    pub fn state(&self, index: usize) -> u32 {
        self.rows[..self.count]
            .get(index)
            .map_or(0, |row| row.state)
    }

    pub fn image(&self, index: usize) -> i32 {
        self.rows[..self.count]
            .get(index)
            .map_or(0, |row| row.image)
    }

    pub fn lparam(&self, index: usize) -> usize {
        self.rows[..self.count]
            .get(index)
            .map_or(0, |row| row.lparam)
    }

    pub fn set_image(&mut self, index: usize, image: i32) -> bool {
        match self.rows[..self.count].get_mut(index) {
            Some(row) => {
                row.image = image;
                true
            }
            None => false,
        }
    }

    pub fn set_lparam(&mut self, index: usize, lparam: usize) -> bool {
        match self.rows[..self.count].get_mut(index) {
            Some(row) => {
                row.lparam = lparam;
                true
            }
            None => false,
        }
    }

    /// Gives row `index`'s cell in `column` (the first, or one the header has) the text
    /// `units`; false, the old text kept, when there is no such cell or no room.
    pub fn set_text(&mut self, index: usize, column: usize, units: &[u16]) -> bool {
        let column_exists = column == 0 || column < self.columns.count().min(MAX_COLUMNS);
        if index >= self.count || !column_exists {
            return false;
        }
        let at = slot(index, column);
        let (start, length) = self.cells[at];
        if units.len() <= usize::from(length) {
            let start = usize::from(start);
            self.units[start..start + units.len()].copy_from_slice(units);
            self.cells[at] = (start as u16, units.len() as u16);
            return true;
        }
        if self.used + units.len() > ROW_UNITS {
            let live: usize = self.cells[..slot(self.count, 0)]
                .iter()
                .map(|&(_, length)| usize::from(length))
                .sum();
            if live - usize::from(length) + units.len() > ROW_UNITS {
                return false;
            }
            self.cells[at] = (0, 0);
            self.pack();
        }
        let start = self.used;
        self.units[start..start + units.len()].copy_from_slice(units);
        self.used += units.len();
        self.cells[at] = (start as u16, units.len() as u16);
        true
    }

    fn pack(&mut self) {
        let mut order = [0u32; CELLS];
        let mut live = 0;
        for (at, &(start, length)) in self.cells[..slot(self.count, 0)].iter().enumerate() {
            if length > 0 {
                order[live] = (u32::from(start) << SLOT_BITS) | at as u32;
                live += 1;
            }
        }
        order[..live].sort_unstable();
        let mut used = 0;
        for key in &order[..live] {
            let at = (key & ((1 << SLOT_BITS) - 1)) as usize;
            let (start, length) = self.cells[at];
            let (start, length) = (usize::from(start), usize::from(length));
            self.units.copy_within(start..start + length, used);
            self.cells[at].0 = used as u16;
            used += length;
        }
        self.used = used;
    }

    /// Where a row with the first text `text` goes with `LVS_SORTASCENDING` (or descending).
    pub fn sorted_position(&self, text: &[u16], descending: bool) -> usize {
        let wanted = match descending {
            true => Ordering::Greater,
            false => Ordering::Less,
        };
        (0..self.count)
            .find(|&index| compare_names(text, self.text(index, 0)) == wanted)
            .unwrap_or(self.count)
    }

    /// Inserts a row at `at` (the end when past it) with its first text, image and data; its
    /// index, `None` when the list is full or the text does not fit.
    pub fn insert(&mut self, at: usize, text: &[u16], image: i32, lparam: usize) -> Option<usize> {
        if self.count == MAX_ROWS {
            return None;
        }
        let at = at.min(self.count);
        self.rows.copy_within(at..self.count, at + 1);
        self.cells
            .copy_within(slot(at, 0)..slot(self.count, 0), slot(at + 1, 0));
        self.rows[at] = Row {
            state: 0,
            image,
            lparam,
        };
        self.cells[slot(at, 0)..slot(at + 1, 0)].fill((0, 0));
        self.count += 1;
        self.mark = self
            .mark
            .map(|mark| if mark >= at { mark + 1 } else { mark });
        if !self.set_text(at, 0, text) {
            self.delete(at);
            return None;
        }
        Some(at)
    }

    /// Deletes row `index` (the focus moves to the row taking its place); whether there was one.
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.count {
            return false;
        }
        let focused = self.rows[index].state & state::FOCUSED != 0;
        self.rows.copy_within(index + 1..self.count, index);
        self.cells
            .copy_within(slot(index + 1, 0)..slot(self.count, 0), slot(index, 0));
        self.count -= 1;
        self.rows[self.count] = NO_ROW;
        self.cells[slot(self.count, 0)..slot(self.count + 1, 0)].fill((0, 0));
        self.mark = match self.mark {
            Some(mark) if mark == index => None,
            Some(mark) if mark > index => Some(mark - 1),
            other => other,
        };
        self.top = self.top.min(self.count.saturating_sub(1));
        if focused && self.count > 0 {
            self.rows[index.min(self.count - 1)].state |= state::FOCUSED;
        }
        true
    }

    /// Deletes every row.
    pub fn clear(&mut self) {
        self.cells[..slot(self.count, 0)].fill((0, 0));
        self.rows[..self.count].fill(NO_ROW);
        self.count = 0;
        self.used = 0;
        self.top = 0;
        self.mark = None;
    }

    /// Inserts a column at `at`, moving the cells from there one column right (the first
    /// cell, the row's own text, stays first); its index, `None` when full.
    pub fn insert_column(
        &mut self,
        at: usize,
        text: &[u16],
        width: i32,
        format: u32,
        lparam: usize,
    ) -> Option<usize> {
        let at = self.columns.insert(at, text, width, format, lparam)?;
        let from = at.max(1);
        for row in 0..self.count {
            let base = slot(row, 0);
            self.cells
                .copy_within(base + from..base + MAX_COLUMNS - 1, base + from + 1);
            self.cells[base + from] = (0, 0);
        }
        Some(at)
    }

    /// Deletes column `index` and its cells; whether there was one.
    pub fn delete_column(&mut self, index: usize) -> bool {
        if !self.columns.delete(index) {
            return false;
        }
        for row in 0..self.count {
            let base = slot(row, 0);
            self.cells
                .copy_within(base + index + 1..base + MAX_COLUMNS, base + index);
            self.cells[base + MAX_COLUMNS - 1] = (0, 0);
        }
        true
    }

    fn first_cell_prefix(&self) -> i32 {
        let checks = i32::from(self.extended & extended::CHECK_BOXES != 0);
        let images = i32::from(self.small_images != 0);
        PREFIX_STEP * (checks + images)
    }

    /// `LVSCW_AUTOSIZE` (or `LVSCW_AUTOSIZE_USEHEADER` with `use_header`) for `column`, in a
    /// list whose rows area is `rows_width` wide.
    pub fn auto_width(&self, column: usize, use_header: bool, rows_width: i32) -> i32 {
        let prefix = match column {
            0 => self.first_cell_prefix(),
            _ => 0,
        };
        let mut width = (0..self.count)
            .map(|index| {
                plain_width_cells(self.text(index, column)) as i32 * CELL_WIDTH
                    + TEXT_PADDING
                    + prefix
            })
            .max()
            .unwrap_or(TEXT_PADDING);
        if use_header {
            let title = plain_width_cells(self.columns.text(column)) as i32 * CELL_WIDTH;
            width = width.max(title + TEXT_PADDING);
            if column + 1 == self.columns.count() {
                width = width.max(rows_width - self.columns.left(column));
            }
        }
        match column {
            0 => width.max(SMALLEST_FIRST_WIDTH),
            _ => width,
        }
    }

    /// The focused row.
    pub fn focused(&self) -> Option<usize> {
        self.rows[..self.count]
            .iter()
            .position(|row| row.state & state::FOCUSED != 0)
    }

    pub fn selected_count(&self) -> usize {
        self.rows[..self.count]
            .iter()
            .filter(|row| row.state & state::SELECTED != 0)
            .count()
    }

    /// `LVM_GETNEXTITEM`: the first row after `start` (from the first when `None`; before it,
    /// searching up, when `backwards`) whose state has every bit of `wanted`.
    pub fn next_item(&self, start: Option<usize>, wanted: u32, backwards: bool) -> Option<usize> {
        let matches = |index: &usize| self.rows[*index].state & wanted == wanted;
        match backwards {
            false => (start.map_or(0, |start| start + 1)..self.count).find(matches),
            true => (0..start.unwrap_or(self.count).min(self.count))
                .rev()
                .find(matches),
        }
    }

    /// The state row `index` has once `wanted` is done, in a list with single selection when
    /// `single`.
    pub fn wanted_state(&self, index: usize, wanted: &Wanted, single: bool) -> u32 {
        let old = self.state(index);
        let mut new = old;
        if let Some(focus) = wanted.focus {
            new = with_bit(new, state::FOCUSED, focus == index);
        }
        match wanted.selection {
            Selection::Keep => {}
            Selection::Only { first, last } => {
                new = with_bit(new, state::SELECTED, (first..=last).contains(&index));
            }
            Selection::Row {
                index: row,
                selected,
            } => {
                if row == index {
                    new = with_bit(new, state::SELECTED, selected);
                } else if single && selected {
                    new &= !state::SELECTED;
                }
            }
            Selection::All => new |= state::SELECTED,
            Selection::Clear => new &= !state::SELECTED,
            Selection::Set {
                row,
                state: bits,
                mask,
            } => match row {
                Some(row) if row != index => {
                    if mask & bits & state::FOCUSED != 0 {
                        new &= !state::FOCUSED;
                    }
                    if single && mask & bits & state::SELECTED != 0 {
                        new &= !state::SELECTED;
                    }
                }
                Some(_) => new = (new & !mask) | (bits & mask),
                None => {
                    let mask = mask & !state::FOCUSED;
                    new = (new & !mask) | (bits & mask);
                }
            },
        }
        new
    }

    /// The first row from `from` whose state `wanted` changes, among the rows that only lose
    /// bits (or, with `gaining`, the rows that gain one): the layer sends each row's pair, the
    /// losing rows first, both in index order.
    pub fn next_change(
        &self,
        from: usize,
        wanted: &Wanted,
        single: bool,
        gaining: bool,
    ) -> Option<StateChange> {
        (from..self.count).find_map(|index| {
            let old = self.rows[index].state;
            let new = self.wanted_state(index, wanted, single);
            let gains = new & !old != 0;
            (old != new && gains == gaining).then_some(StateChange {
                index,
                old,
                new,
                lparam: self.rows[index].lparam,
            })
        })
    }

    /// Makes `change` if its row still has its old state; whether it did.
    pub fn apply(&mut self, change: &StateChange) -> bool {
        match self.rows[..self.count].get_mut(change.index) {
            Some(row) if row.state == change.old => {
                row.state = change.new;
                true
            }
            _ => false,
        }
    }

    /// The row an arrow, Home, End, PgUp or PgDn moves the focus to, `visible` rows showing.
    pub fn step(&self, virtual_key: u32, visible: usize) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let last = self.count - 1;
        let page = visible.max(2) - 1;
        let focused = self.focused();
        Some(match (virtual_key, focused) {
            (key::UP, Some(focused)) => focused.saturating_sub(1),
            (key::DOWN, Some(focused)) => (focused + 1).min(last),
            (key::UP | key::DOWN, None) => 0,
            (key::HOME, _) => 0,
            (key::END, _) => last,
            (key::PAGE_UP, focused) => focused.unwrap_or(0).saturating_sub(page),
            (key::PAGE_DOWN, focused) => (focused.unwrap_or(0) + page).min(last),
            _ => return None,
        })
    }

    /// The next row after the focused one, wrapping, whose first text starts with `character`
    /// (ASCII letters folded).
    pub fn find_by_first_glyph(&self, character: u32) -> Option<usize> {
        let wanted = fold(character);
        let start = self.focused().map_or(0, |focused| focused + 1);
        (0..self.count)
            .map(|step| (start + step) % self.count)
            .find(|&index| {
                self.text(index, 0)
                    .first()
                    .is_some_and(|&unit| fold(u32::from(unit)) == wanted)
            })
    }

    /// Scrolls so row `index` shows whole, `visible` rows showing (section 5's rule).
    pub fn ensure_visible(&mut self, index: usize, visible: usize) {
        let visible = visible.max(1);
        if index >= self.count {
            return;
        }
        if index < self.top {
            self.top = index;
        }
        if index >= self.top + visible {
            self.top = index + 1 - visible;
        }
    }

    /// Makes `top` the first row shown, as far as the rows go.
    pub fn set_top(&mut self, top: usize, visible: usize) {
        self.top = top.min(self.count.saturating_sub(visible.max(1)));
    }

    /// Scrolls the rows by `rows` (up when negative).
    pub fn scroll_rows(&mut self, rows: isize, visible: usize) {
        let top = self.top.saturating_add_signed(rows);
        self.set_top(top, visible);
    }

    /// Sets the horizontal scroll, kept within what `layout`'s rows area leaves.
    pub fn set_hpos(&mut self, hpos: i32, layout: &ListLayout) {
        let most = (layout.total - layout.rows.width()).max(0);
        self.hpos = hpos.clamp(0, most);
    }

    /// Puts the rows in the order `order` gives (`order[i]` the old index of the row now at
    /// `i`); false, nothing moved, when it is not an order of every row.
    pub fn reorder(&mut self, order: &[u16]) -> bool {
        if order.len() != self.count {
            return false;
        }
        let mut seen = [false; MAX_ROWS];
        for &from in order {
            let from = usize::from(from);
            if from >= self.count || seen[from] {
                return false;
            }
            seen[from] = true;
        }
        let mut done = [false; MAX_ROWS];
        for start in 0..self.count {
            if done[start] {
                continue;
            }
            let kept_row = self.rows[start];
            let mut kept_cells = [(0u16, 0u16); MAX_COLUMNS];
            kept_cells.copy_from_slice(&self.cells[slot(start, 0)..slot(start + 1, 0)]);
            let mut at = start;
            loop {
                done[at] = true;
                let from = usize::from(order[at]);
                if from == start {
                    break;
                }
                self.rows[at] = self.rows[from];
                self.cells
                    .copy_within(slot(from, 0)..slot(from + 1, 0), slot(at, 0));
                at = from;
            }
            self.rows[at] = kept_row;
            self.cells[slot(at, 0)..slot(at + 1, 0)].copy_from_slice(&kept_cells);
        }
        self.mark = self
            .mark
            .and_then(|mark| order.iter().position(|&from| usize::from(from) == mark));
        true
    }

    /// Where the parts of a list of `style` drawn over `rect` are.
    pub fn layout(&self, rect: Rect, style: u32) -> ListLayout {
        let inner = rect.inset(1);
        let report = is_report(style);
        let header_height = match shows_header(style) {
            true => HEADER_HEIGHT,
            false => 0,
        };
        let scrolls = style & style::NO_SCROLL == 0;
        let columns_total = self.columns.total();
        let size = |vertical: bool, horizontal: bool| {
            let width = inner.width() - if vertical { BAR } else { 0 };
            let height = inner.height() - header_height - if horizontal { BAR } else { 0 };
            (width, height, ((height / ROW_HEIGHT).max(1)) as usize)
        };
        let (mut vertical, mut horizontal) = (false, false);
        for _ in 0..3 {
            let (width, _, visible) = size(vertical, horizontal);
            vertical = scrolls && self.count > visible;
            horizontal = scrolls && report && columns_total > width;
        }
        let (width, height, visible) = size(vertical, horizontal);
        let rows_top = inner.top + header_height;
        let rows = Rect::new(
            inner.left,
            rows_top,
            inner.left + width.max(0),
            rows_top + height.max(0),
        );
        let header = match header_height {
            0 => Rect::default(),
            _ => Rect::sized(inner.left, inner.top, width.max(0), header_height),
        };
        let below_bar = match horizontal {
            true => BAR,
            false => 0,
        };
        ListLayout {
            inner,
            header,
            rows,
            visible,
            vertical: vertical.then_some(Rect::new(
                inner.right - BAR,
                inner.top,
                inner.right,
                inner.bottom - below_bar,
            )),
            horizontal: horizontal.then_some(Rect::new(
                inner.left,
                inner.bottom - BAR,
                rows.right,
                inner.bottom,
            )),
            total: match report {
                true => columns_total,
                false => rows.width(),
            },
            report,
        }
    }

    /// The vertical bar's `SCROLLINFO`: the rows, `V` and the first row shown.
    pub fn vertical_info(&self, layout: &ListLayout) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: self.count as i32 - 1,
            page: layout.visible as u32,
            position: self.top as i32,
        }
    }

    /// The horizontal bar's `SCROLLINFO`: `T`, the rows area's width and the scroll, in pixels.
    pub fn horizontal_info(&self, layout: &ListLayout) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: layout.total - 1,
            page: layout.rows.width().max(0) as u32,
            position: self.hpos,
        }
    }

    fn shown_columns(&self, layout: &ListLayout) -> usize {
        match layout.report {
            true => self.columns.count(),
            false => 1,
        }
    }

    /// Column `column`'s left edge, width and alignment as drawn (the first always left).
    pub fn column_span(&self, layout: &ListLayout, column: usize) -> (i32, i32, u32) {
        match layout.report {
            true => (
                layout.inner.left - self.hpos + self.columns.left(column),
                self.columns.width(column),
                match column {
                    0 => format::LEFT,
                    _ => self.columns.format(column),
                },
            ),
            false => (layout.rows.left, layout.rows.width(), format::LEFT),
        }
    }

    /// Row `index`'s top edge, shown or not.
    pub fn row_top(&self, layout: &ListLayout, index: usize) -> i32 {
        layout.rows.top + ROW_HEIGHT * (index as i32 - self.top as i32)
    }

    /// The area a selected row fills: the row over `T` with full-row selection, else its first
    /// cell, inside the rows area.
    pub fn selection_area(&self, layout: &ListLayout, index: usize, full_row: bool) -> Rect {
        let y = self.row_top(layout, index);
        let row = Rect::new(layout.rows.left, y, layout.rows.right, y + ROW_HEIGHT);
        let area = match full_row || !layout.report {
            true => Rect::sized(layout.inner.left - self.hpos, y, layout.total, ROW_HEIGHT),
            false => {
                let (left, width, _) = self.column_span(layout, 0);
                Rect::sized(left, y, width, ROW_HEIGHT)
            }
        };
        area.intersection(&row)
    }

    /// Row `index`'s bounds as agents see them: over `min(T, inner_w)`; a row out of view is
    /// as wide as the rows area and 0 high at its top or bottom.
    pub fn row_bounds(&self, layout: &ListLayout, index: usize) -> Rect {
        let y = self.row_top(layout, index);
        let rows = layout.rows;
        if y + ROW_HEIGHT <= rows.top {
            return Rect::new(rows.left, rows.top, rows.right, rows.top);
        }
        if y >= rows.bottom {
            return Rect::new(rows.left, rows.bottom, rows.right, rows.bottom);
        }
        let left = layout.inner.left - self.hpos;
        Rect::new(
            left.max(rows.left),
            y.max(rows.top),
            (left + layout.total)
                .min(rows.right)
                .max(left.max(rows.left)),
            (y + ROW_HEIGHT).min(rows.bottom),
        )
    }

    /// `LVM_GETITEMRECT`'s rectangle of row `index`.
    pub fn item_rect(&self, layout: &ListLayout, index: usize, part: ItemPart) -> Rect {
        let y = self.row_top(layout, index);
        let (left, width, _) = self.column_span(layout, 0);
        match part {
            ItemPart::Bounds => {
                Rect::sized(layout.inner.left - self.hpos, y, layout.total, ROW_HEIGHT)
            }
            ItemPart::Icon => {
                let checks = match self.extended & extended::CHECK_BOXES != 0 {
                    true => PREFIX_STEP,
                    false => 0,
                };
                Rect::sized(
                    left + IMAGE_LEFT + checks,
                    y + IMAGE_TOP,
                    IMAGE_SIZE,
                    IMAGE_SIZE,
                )
            }
            ItemPart::Label => Rect::sized(left, y, width, ROW_HEIGHT),
        }
    }

    /// `LVM_GETSUBITEMRECT`'s rectangle of row `index`'s cell in `column`.
    pub fn cell_rect(&self, layout: &ListLayout, index: usize, column: usize) -> Rect {
        let (left, width, _) = self.column_span(layout, column);
        Rect::sized(left, self.row_top(layout, index), width, ROW_HEIGHT)
    }

    /// What is at (`x`, `y`) of the list.
    pub fn hit(&self, layout: &ListLayout, x: i32, y: i32) -> ListHit {
        if let Some(bar) = layout.vertical
            && bar.contains(x, y)
        {
            return scroll_bar::part_at(bar, &self.vertical_info(layout), y)
                .map_or(ListHit::Nowhere, ListHit::Vertical);
        }
        if let Some(bar) = layout.horizontal
            && bar.contains(x, y)
        {
            return horizontal_part_at(bar, &self.horizontal_info(layout), x)
                .map_or(ListHit::Nowhere, ListHit::Horizontal);
        }
        if layout.header.contains(x, y) {
            return ListHit::Header;
        }
        if !layout.rows.contains(x, y) {
            return ListHit::Nowhere;
        }
        let index = self.top + ((y - layout.rows.top) / ROW_HEIGHT) as usize;
        let column = (0..self.shown_columns(layout)).find(|&column| {
            let (left, width, _) = self.column_span(layout, column);
            x >= left && x < left + width
        });
        match column {
            Some(column) if index < self.count => ListHit::Cell {
                index,
                column,
                part: self.part_at(layout, index, column, x, y),
            },
            column => ListHit::Empty { column },
        }
    }

    fn part_at(
        &self,
        layout: &ListLayout,
        index: usize,
        column: usize,
        x: i32,
        y: i32,
    ) -> CellPart {
        if column != 0 {
            return CellPart::Text;
        }
        let (left, _, _) = self.column_span(layout, 0);
        let row_y = self.row_top(layout, index);
        let checks = self.extended & extended::CHECK_BOXES != 0;
        let check_box = Rect::sized(
            left + CHECK_BOX_LEFT,
            row_y + CHECK_BOX_TOP,
            CHECK_BOX_SIZE,
            CHECK_BOX_SIZE,
        );
        if checks && check_box.contains(x, y) {
            return CellPart::CheckBox;
        }
        let image = self.item_rect(layout, index, ItemPart::Icon);
        if self.small_images != 0 && image.contains(x, y) {
            return CellPart::Image;
        }
        CellPart::Text
    }
}

/// Puts `order` in the order `compare` says, keeping equal items in their order (a merge sort,
/// bottom up); `scratch` is at least as long as `order`.
pub fn stable_sort(
    order: &mut [u16],
    scratch: &mut [u16],
    compare: &mut dyn FnMut(u16, u16) -> Ordering,
) {
    let length = order.len().min(scratch.len());
    let mut width = 1;
    while width < length {
        let mut start = 0;
        while start < length {
            let middle = (start + width).min(length);
            let end = (start + 2 * width).min(length);
            let (mut left, mut right, mut out) = (start, middle, start);
            while left < middle && right < end {
                match compare(order[left], order[right]) {
                    Ordering::Greater => {
                        scratch[out] = order[right];
                        right += 1;
                    }
                    _ => {
                        scratch[out] = order[left];
                        left += 1;
                    }
                }
                out += 1;
            }
            scratch[out..out + middle - left].copy_from_slice(&order[left..middle]);
            out += middle - left;
            scratch[out..out + end - right].copy_from_slice(&order[right..end]);
            start = end;
        }
        order[..length].copy_from_slice(&scratch[..length]);
        width *= 2;
    }
}

/// The position a scroll bar `length` long shows when its thumb starts `start` pixels from the
/// bar's start (section 14's drag, rounded to nearest).
pub fn thumb_position(length: i32, info: &ScrollInfo, start: i32) -> i32 {
    let Some((_, size)) = scroll_bar::thumb(length, info) else {
        return info.minimum;
    };
    let room = i64::from(length - 2 * BAR - size);
    let last = i64::from(info.last_position());
    let first = i64::from(info.minimum);
    if room <= 0 {
        return info.minimum;
    }
    let travelled = i64::from(start - BAR).clamp(0, room);
    let span = last - first;
    (first + (2 * travelled * span + room) / (2 * room)).clamp(first, last) as i32
}

/// The part of the horizontal bar at `rect` under `x`, if `x` is on it.
pub fn horizontal_part_at(rect: Rect, info: &ScrollInfo, x: i32) -> Option<Part> {
    if x < rect.left || x >= rect.right {
        return None;
    }
    let arrow = BAR.min(rect.width() / 2);
    if x < rect.left + arrow {
        return Some(Part::LineUp);
    }
    if x >= rect.right - arrow {
        return Some(Part::LineDown);
    }
    let (start, size) = scroll_bar::thumb(rect.width(), info)?;
    let offset = x - rect.left;
    Some(match () {
        () if offset < start => Part::PageUp,
        () if offset < start + size => Part::Thumb,
        () => Part::PageDown,
    })
}

/// Paints a horizontal scroll bar over `rect` (16 px high), section 14's look turned on its
/// side.
pub fn paint_horizontal(
    surface: &mut Surface<'_>,
    rect: Rect,
    info: &ScrollInfo,
    enabled: bool,
    palette: &Palette,
) {
    surface.fill(rect, palette.scroll_track);
    let placed = scroll_bar::thumb(rect.width(), info).filter(|_| enabled);
    let colour = match placed {
        Some(_) => palette.scroll_arrow,
        None => palette.scroll_arrow_disabled,
    };
    let arrow = BAR.min(rect.width() / 2);
    let (dx, dy) = HORIZONTAL_CHEVRON_AT;
    if arrow >= dx + CHEVRON_LEFT.width {
        draw_chevron(
            surface,
            rect.left + dx,
            rect.top + dy,
            &CHEVRON_LEFT,
            colour,
        );
        draw_chevron(
            surface,
            rect.right - arrow + dx,
            rect.top + dy,
            &CHEVRON_RIGHT,
            colour,
        );
    }
    if let Some((start, size)) = placed {
        surface.fill(
            Rect::sized(rect.left + start, rect.top, size, rect.height()),
            palette.scroll_thumb,
        );
    }
}

/// Paints the scroll bars of `layout` (and the size box between them) with `vertical` and
/// `horizontal` as their `SCROLLINFO`s.
pub fn paint_scroll_bars(
    surface: &mut Surface<'_>,
    vertical: Option<(Rect, ScrollInfo)>,
    horizontal: Option<(Rect, ScrollInfo)>,
    enabled: bool,
    palette: &Palette,
) {
    if let Some((bar, info)) = vertical {
        scroll_bar::paint_vertical(surface, bar, &info, enabled, palette);
    }
    if let Some((bar, info)) = horizontal {
        paint_horizontal(surface, bar, &info, enabled, palette);
    }
    if let (Some((vertical, _)), Some((horizontal, _))) = (vertical, horizontal) {
        surface.fill(
            Rect::new(
                vertical.left,
                horizontal.top,
                vertical.right,
                horizontal.bottom,
            ),
            palette.scroll_track,
        );
    }
}

/// Draws a section 2 check box (14x14) with its top-left at (`x`, `y`).
pub fn draw_check_box(
    surface: &mut Surface<'_>,
    x: i32,
    y: i32,
    checked: bool,
    enabled: bool,
    palette: &Palette,
) {
    let rect = Rect::sized(x, y, CHECK_BOX_SIZE, CHECK_BOX_SIZE);
    let (fill, border, mark) = match (enabled, checked) {
        (false, _) => (
            palette.box_disabled_fill,
            palette.box_disabled_border,
            palette.box_disabled_mark,
        ),
        (true, true) => (
            palette.box_checked_fill,
            palette.box_border,
            palette.box_mark,
        ),
        (true, false) => (palette.box_fill, palette.box_border, palette.box_mark),
    };
    surface.fill(rect, fill);
    outline(surface, rect, border);
    if checked {
        tick(surface, x, y, mark);
    }
}

fn shown_cells(units: &[u16], room: i32) -> usize {
    let (shown, ellipsis) = fit_cells(plain_width_cells(units), room);
    match ellipsis {
        true => shown + ELLIPSIS_CELLS,
        false => shown,
    }
}

/// Draws `units` in a column at `left`, `width` wide, aligned by `alignment` (section 24's
/// placement, `prefix` pixels kept free at a left-aligned cell's start), cell top `top`.
#[allow(clippy::too_many_arguments)]
fn draw_aligned(
    surface: &mut Surface<'_>,
    left: i32,
    width: i32,
    alignment: u32,
    prefix: i32,
    top: i32,
    units: &[u16],
    colour: u32,
) {
    let room = width - TEXT_PADDING - prefix;
    let span = shown_cells(units, room) as i32 * CELL_WIDTH;
    let x = match alignment & format::JUSTIFY_MASK {
        format::RIGHT => left + width - TEXT_INSET - span,
        format::CENTER => left + (width - span) / 2,
        _ => left + TEXT_INSET + prefix,
    };
    draw_plain(surface, x, top, units, room, colour);
}

/// How a header looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeaderLook {
    /// The item held down with the pointer inside it.
    pub pressed: Option<usize>,
    /// The item under the pointer.
    pub hot: Option<usize>,
    /// Whether its items are buttons (`HDS_BUTTONS`; a list's header without
    /// `LVS_NOSORTHEADER`).
    pub buttons: bool,
    pub enabled: bool,
    /// Whether it is a list view's header, whose first column is always left-aligned.
    pub of_list: bool,
}

/// The header item under `x` pixels from the header's left edge, scrolled by `hpos`.
pub fn header_item_at(columns: &Columns, hpos: i32, x: i32) -> Option<usize> {
    columns.at(x + hpos)
}

/// Header item `index`'s rectangle in a header at `rect`, scrolled by `hpos`.
pub fn header_item_rect(columns: &Columns, rect: Rect, hpos: i32, index: usize) -> Rect {
    Rect::sized(
        rect.left - hpos + columns.left(index),
        rect.top,
        columns.width(index),
        rect.height(),
    )
}

/// Paints a header over `rect` with `columns` as its items, scrolled by `hpos`.
pub fn paint_header(
    surface: &mut Surface<'_>,
    rect: Rect,
    columns: &Columns,
    hpos: i32,
    look: HeaderLook,
    palette: &Palette,
    common: &CommonPalette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    surface.fill(rect, common.header_fill);
    let colour = match look.enabled {
        true => palette.text,
        false => palette.text_disabled,
    };
    for index in 0..columns.count() {
        let item = header_item_rect(columns, rect, hpos, index);
        if look.enabled && look.buttons {
            if look.pressed == Some(index) {
                surface.fill(item, common.header_pressed_fill);
            } else if look.hot == Some(index) {
                surface.fill(item, common.header_hover_fill);
            }
        }
        surface.fill(
            Rect::sized(item.right - 1, rect.top + DIVIDER_TOP, 1, DIVIDER_HEIGHT),
            common.header_line,
        );
        let alignment = match (look.of_list, index) {
            (true, 0) => format::LEFT,
            _ => columns.format(index),
        };
        let clip = surface.clip();
        surface.clip_to(item);
        draw_aligned(
            surface,
            item.left,
            item.width(),
            alignment,
            0,
            rect.top + HEADER_TEXT_TOP,
            columns.text(index),
            colour,
        );
        surface.set_clip(clip);
    }
    surface.fill(
        Rect::new(rect.left, rect.bottom - 1, rect.right, rect.bottom),
        common.header_line,
    );
    surface.set_clip(saved);
}

/// How a list looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLook {
    /// The window's style (`LVS_*`).
    pub style: u32,
    pub focused: bool,
    pub enabled: bool,
    /// Whether the header is a window of its own that paints itself (else the list paints it).
    pub header_window: bool,
}

/// Paints `list` over the whole of `rect`: the frame, the header (unless it is a window of its
/// own), the rows and the scroll bars.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    list: &ListView,
    look: ListLook,
    images: &dyn Images,
    palette: &Palette,
    common: &CommonPalette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    let layout = list.layout(rect, look.style);
    let fill = list.background.unwrap_or(palette.field_fill);
    outline(surface, rect, palette.field_border);
    let inner = layout.inner;
    match look.header_window || layout.header.is_empty() {
        true => {
            surface.fill(
                Rect::new(
                    inner.left,
                    layout.header.bottom.max(inner.top),
                    inner.right,
                    inner.bottom,
                ),
                fill,
            );
            surface.fill(
                Rect::new(
                    layout.header.right,
                    inner.top,
                    inner.right,
                    layout.header.bottom,
                ),
                fill,
            );
        }
        false => {
            surface.fill(inner, fill);
            let header_look = HeaderLook {
                pressed: None,
                hot: None,
                buttons: look.style & style::NO_SORT_HEADER == 0,
                enabled: look.enabled,
                of_list: true,
            };
            paint_header(
                surface,
                layout.header,
                &list.columns,
                list.hpos,
                header_look,
                palette,
                common,
            );
        }
    }
    let rows_clip = surface.clip().intersection(&layout.rows);
    surface.set_clip(rows_clip);
    let mut index = list.top;
    while index < list.count {
        let y = list.row_top(&layout, index);
        if y >= layout.rows.bottom {
            break;
        }
        paint_row(surface, list, &layout, index, look, images, palette, common);
        surface.set_clip(rows_clip);
        index += 1;
    }
    if layout.report && list.extended & extended::GRID_LINES != 0 {
        for column in 0..list.columns.count() {
            let (left, width, _) = list.column_span(&layout, column);
            surface.fill(
                Rect::new(
                    left + width - 1,
                    layout.rows.top,
                    left + width,
                    layout.rows.bottom,
                ),
                common.header_line,
            );
        }
    }
    surface.set_clip(saved.intersection(&rect));
    paint_scroll_bars(
        surface,
        layout
            .vertical
            .map(|bar| (bar, list.vertical_info(&layout))),
        layout
            .horizontal
            .map(|bar| (bar, list.horizontal_info(&layout))),
        look.enabled,
        palette,
    );
    surface.set_clip(saved);
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    surface: &mut Surface<'_>,
    list: &ListView,
    layout: &ListLayout,
    index: usize,
    look: ListLook,
    images: &dyn Images,
    palette: &Palette,
    common: &CommonPalette,
) {
    let rows_clip = surface.clip();
    let y = list.row_top(layout, index);
    let row = list.rows[index];
    let full_row = list.extended & extended::FULL_ROW_SELECT != 0;
    let area = list.selection_area(layout, index, full_row);
    if let Some(colour) = list.text_background {
        surface.fill(
            Rect::sized(layout.inner.left - list.hpos, y, layout.total, ROW_HEIGHT),
            colour,
        );
    }
    let selected = row.state & state::SELECTED != 0 && look.enabled;
    let shows_inactive = look.style & style::SHOW_SELECTION_ALWAYS != 0;
    let (area_fill, area_text) = match () {
        () if !selected => (None, None),
        () if look.focused => (Some(palette.highlight), Some(palette.highlight_text)),
        () if shows_inactive => (
            Some(common.selection_inactive),
            Some(common.selection_inactive_text),
        ),
        () => (None, None),
    };
    if let Some(colour) = area_fill {
        surface.fill(area, colour);
    }
    let plain_text = match look.enabled {
        true => list.text_colour.unwrap_or(palette.text),
        false => palette.text_disabled,
    };
    for column in 0..list.shown_columns(layout) {
        let (left, width, alignment) = list.column_span(layout, column);
        let cell = Rect::sized(left, y, width, ROW_HEIGHT);
        let in_area = full_row || column == 0 || !layout.report;
        let colour = match (area_text, in_area) {
            (Some(colour), true) => colour,
            _ => plain_text,
        };
        surface.set_clip(rows_clip.intersection(&cell));
        let mut prefix = 0;
        if column == 0 {
            if list.extended & extended::CHECK_BOXES != 0 {
                let checked = state::image_of(row.state) == state::CHECKED;
                draw_check_box(
                    surface,
                    left + CHECK_BOX_LEFT,
                    y + CHECK_BOX_TOP,
                    checked,
                    look.enabled,
                    palette,
                );
                prefix += PREFIX_STEP;
            }
            if list.small_images != 0 {
                images.draw(
                    surface,
                    list.small_images,
                    row.image,
                    left + IMAGE_LEFT + prefix,
                    y + IMAGE_TOP,
                    false,
                );
                prefix += PREFIX_STEP;
            }
        }
        draw_aligned(
            surface,
            left,
            width,
            alignment,
            prefix,
            y + CELL_TEXT_TOP,
            list.text(index, column),
            colour,
        );
    }
    surface.set_clip(rows_clip);
    if layout.report && list.extended & extended::GRID_LINES != 0 {
        surface.fill(
            Rect::sized(
                layout.inner.left - list.hpos,
                y + ROW_HEIGHT - 1,
                layout.total,
                1,
            ),
            common.header_line,
        );
    }
    if look.focused && row.state & state::FOCUSED != 0 && !area.is_empty() {
        let dots = match area_fill == Some(palette.highlight) {
            true => palette.highlight_text,
            false => palette.focus,
        };
        dotted_outline(surface, area, dots);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;
    use crate::controls::common::{COMMON_LIGHT, NoImages};

    const WIDTH: u32 = 480;
    const HEIGHT: u32 = 300;
    const AHK_STYLE: u32 = style::REPORT | style::SHOW_SELECTION_ALWAYS | style::SINGLE_SELECTION;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn example() -> ListView {
        let mut list = ListView::new();
        list.extended = extended::FULL_ROW_SELECT;
        list.insert_column(0, &units("Fruit"), 0, format::LEFT, 0);
        list.insert_column(1, &units("Count"), 0, format::LEFT, 0);
        for (name, count) in [
            ("Apples", "3"),
            ("Bananas", "5"),
            ("Cherries", "7"),
            ("Dates", "11"),
            ("Elderberries", "13"),
        ] {
            let at = list.insert(list.count(), &units(name), 0, 0).unwrap();
            assert!(list.set_text(at, 1, &units(count)));
        }
        list.columns_mut().set_width(0, 200);
        list.columns_mut().set_width(1, 80);
        list.columns_mut().set_format(1, format::RIGHT);
        let wanted = Wanted {
            focus: Some(0),
            selection: Selection::Set {
                row: Some(0),
                state: state::SELECTED | state::FOCUSED,
                mask: state::SELECTED | state::FOCUSED,
            },
        };
        apply_all(&mut list, &wanted, true);
        list
    }

    fn apply_all(list: &mut ListView, wanted: &Wanted, single: bool) -> Vec<StateChange> {
        let mut changes = Vec::new();
        for gaining in [false, true] {
            let mut from = 0;
            while let Some(change) = list.next_change(from, wanted, single, gaining) {
                from = change.index + 1;
                assert!(list.apply(&change));
                changes.push(change);
            }
        }
        changes
    }

    fn render(list: &ListView, rect: Rect, look: ListLook) -> Vec<u32> {
        let mut pixels = vec![0x00f0_f0f0u32; (WIDTH * HEIGHT) as usize];
        let mut surface = Surface::new(&mut pixels, WIDTH, HEIGHT).unwrap();
        paint(
            &mut surface,
            rect,
            list,
            look,
            &NoImages,
            &LIGHT,
            &COMMON_LIGHT,
        );
        pixels
    }

    fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
        pixels[y as usize * WIDTH as usize + x as usize]
    }

    fn cell_has(pixels: &[u32], x: i32, y: i32, colour: u32) -> bool {
        (x..x + 8).any(|px| (y..y + 16).any(|py| at(pixels, px, py) == colour))
    }

    fn focused_look() -> ListLook {
        ListLook {
            style: AHK_STYLE,
            focused: true,
            enabled: true,
            header_window: false,
        }
    }

    #[test]
    fn the_example_list_draws_its_header_and_rows_where_the_design_says() {
        let list = example();
        let rect = Rect::sized(12, 64, 456, 208);
        let layout = list.layout(rect, AHK_STYLE);
        assert_eq!(layout.inner, Rect::sized(13, 65, 454, 206));
        assert_eq!(layout.total, 280);
        assert_eq!(layout.visible, 9);
        assert_eq!((layout.vertical, layout.horizontal), (None, None));
        let pixels = render(&list, rect, focused_look());
        assert_eq!(at(&pixels, 12, 64), 0x007a_7a7a);
        assert_eq!(at(&pixels, 100, 70), 0x00ff_ffff);
        assert!(cell_has(&pixels, 19, 69, 0));
        assert!(!cell_has(&pixels, 11, 69, 0));
        assert_eq!(at(&pixels, 212, 69), 0x00e5_e5e5);
        assert_eq!(at(&pixels, 212, 84), 0x00e5_e5e5);
        assert_ne!(at(&pixels, 212, 85), 0x00e5_e5e5);
        assert!(cell_has(&pixels, 247, 69, 0));
        assert!(cell_has(&pixels, 279, 69, 0));
        assert!(!cell_has(&pixels, 287, 69, 0));
        assert_eq!(at(&pixels, 292, 69), 0x00e5_e5e5);
        assert_eq!(at(&pixels, 13, 88), 0x00e5_e5e5);
        assert_eq!(at(&pixels, 466, 88), 0x00e5_e5e5);
        assert_eq!(at(&pixels, 14, 90), 0x0000_78d7);
        assert_eq!(at(&pixels, 291, 100), 0x0000_78d7);
        assert_eq!(at(&pixels, 293, 100), 0x00ff_ffff);
        assert!(cell_has(&pixels, 19, 91, 0x00ff_ffff));
        assert!(cell_has(&pixels, 279, 91, 0x00ff_ffff));
        assert_eq!(at(&pixels, 13, 89), 0x00ff_ffff);
        assert_eq!(at(&pixels, 14, 89), 0x0000_78d7);
        assert_eq!(at(&pixels, 292, 90), 0x00ff_ffff);
        for (row, glyph_x) in [(1, 279), (2, 279), (3, 271), (4, 271)] {
            let y = 91 + 20 * row;
            assert!(cell_has(&pixels, 19, y, 0));
            assert!(cell_has(&pixels, glyph_x, y, 0));
            assert!(!cell_has(&pixels, glyph_x - 8, y, 0));
            assert_eq!(at(&pixels, 14, y - 1), 0x00ff_ffff);
        }
    }

    #[test]
    fn the_header_places_left_centred_and_right_titles() {
        let mut columns = Columns::new();
        columns.insert(0, &units("ab"), 100, format::LEFT, 0);
        columns.insert(1, &units("ab"), 100, format::CENTER, 0);
        columns.insert(2, &units("ab"), 100, format::RIGHT, 0);
        let mut pixels = vec![0u32; (WIDTH * HEIGHT) as usize];
        let mut surface = Surface::new(&mut pixels, WIDTH, HEIGHT).unwrap();
        let look = HeaderLook {
            enabled: true,
            buttons: true,
            pressed: Some(1),
            ..HeaderLook::default()
        };
        paint_header(
            &mut surface,
            Rect::sized(0, 0, 400, 24),
            &columns,
            0,
            look,
            &LIGHT,
            &COMMON_LIGHT,
        );
        assert!(cell_has(&pixels, 6, 4, 0));
        assert!(cell_has(&pixels, 142, 4, 0));
        assert!(!cell_has(&pixels, 134, 4, 0));
        assert!(cell_has(&pixels, 278, 4, 0));
        assert!(cell_has(&pixels, 286, 4, 0));
        assert!(!cell_has(&pixels, 270, 4, 0));
        assert_eq!(at(&pixels, 120, 1), COMMON_LIGHT.header_pressed_fill);
        assert_eq!(at(&pixels, 99, 4), COMMON_LIGHT.header_line);
        assert_eq!(at(&pixels, 299, 19), COMMON_LIGHT.header_line);
        assert_eq!(at(&pixels, 350, 23), COMMON_LIGHT.header_line);
        assert_eq!(at(&pixels, 350, 10), COMMON_LIGHT.header_fill);
        assert_eq!(header_item_at(&columns, 0, 150), Some(1));
        assert_eq!(header_item_at(&columns, 50, 150), Some(2));
        assert_eq!(header_item_at(&columns, 0, 300), None);
    }

    #[test]
    fn automatic_widths_follow_the_texts_the_header_and_the_images() {
        let mut list = example();
        assert_eq!(list.auto_width(0, false, 438), 12 * 8 + 12);
        assert_eq!(list.auto_width(1, false, 438), 2 * 8 + 12);
        assert_eq!(list.auto_width(1, true, 438), 438 - 200);
        list.small_images = 7;
        assert_eq!(list.auto_width(0, false, 438), 12 * 8 + 32);
        list.extended |= extended::CHECK_BOXES;
        assert_eq!(list.auto_width(0, false, 438), 12 * 8 + 52);
        let empty = ListView::new();
        assert_eq!(empty.auto_width(0, false, 100), 12);
        let mut titled = ListView::new();
        titled.insert_column(0, &units("Longer title"), 0, 0, 0);
        titled.insert_column(1, &units("x"), 0, 0, 0);
        assert_eq!(titled.auto_width(0, true, 50), 12 * 8 + 12);
    }

    #[test]
    fn a_plain_selection_change_sends_the_losing_row_first() {
        let mut list = example();
        let wanted = Wanted {
            focus: Some(1),
            selection: Selection::Only { first: 1, last: 1 },
        };
        let changes = apply_all(&mut list, &wanted, true);
        let both = state::SELECTED | state::FOCUSED;
        assert_eq!(
            changes,
            [
                StateChange {
                    index: 0,
                    old: both,
                    new: 0,
                    lparam: 0
                },
                StateChange {
                    index: 1,
                    old: 0,
                    new: both,
                    lparam: 0
                },
            ]
        );
        let toggle = Wanted {
            focus: Some(3),
            selection: Selection::Row {
                index: 3,
                selected: true,
            },
        };
        let changes = apply_all(&mut list, &toggle, false);
        assert_eq!(changes.len(), 2);
        assert_eq!((changes[0].index, changes[0].new), (1, state::SELECTED));
        assert_eq!((changes[1].index, changes[1].new), (3, both));
        let range = Wanted {
            focus: Some(4),
            selection: Selection::Only { first: 1, last: 4 },
        };
        let changes = apply_all(&mut list, &range, false);
        let indexes: Vec<usize> = changes.iter().map(|change| change.index).collect();
        assert_eq!(indexes, [3, 2, 4]);
        assert_eq!(list.selected_count(), 4);
        let all = Wanted {
            focus: None,
            selection: Selection::All,
        };
        assert_eq!(apply_all(&mut list, &all, false).len(), 1);
        let only_focus = Wanted {
            focus: Some(2),
            selection: Selection::Keep,
        };
        let changes = apply_all(&mut list, &only_focus, false);
        assert_eq!(
            changes
                .iter()
                .map(|change| change.index)
                .collect::<Vec<_>>(),
            [4, 2]
        );
        assert_eq!(list.selected_count(), 5);
    }

    #[test]
    fn a_refused_change_keeps_its_row_and_single_selection_holds() {
        let mut list = example();
        let wanted = Wanted {
            focus: None,
            selection: Selection::Set {
                row: Some(2),
                state: state::SELECTED,
                mask: state::SELECTED,
            },
        };
        let losing = list.next_change(0, &wanted, true, false).unwrap();
        assert_eq!(losing.index, 0);
        assert_eq!(losing.new, state::FOCUSED);
        let gaining = list.next_change(0, &wanted, true, true).unwrap();
        assert_eq!(gaining.index, 2);
        assert!(list.apply(&gaining));
        assert_eq!(list.selected_count(), 2);
        assert!(!list.apply(&gaining));
    }

    #[test]
    fn keys_move_the_focus_and_the_list_scrolls_to_it() {
        let mut list = ListView::new();
        list.insert_column(0, &units("Name"), 100, 0, 0);
        for index in 0..30 {
            list.insert(index, &units(&format!("row {index}")), 0, 0);
        }
        fn focus(list: &mut ListView, key: u32) {
            let target = list.step(key, 9).unwrap();
            let wanted = Wanted {
                focus: Some(target),
                selection: Selection::Only {
                    first: target,
                    last: target,
                },
            };
            apply_all(list, &wanted, true);
            list.ensure_visible(target, 9);
        }
        for _ in 0..9 {
            focus(&mut list, key::DOWN);
        }
        assert_eq!((list.focused(), list.top()), (Some(8), 0));
        focus(&mut list, key::DOWN);
        assert_eq!((list.focused(), list.top()), (Some(9), 1));
        focus(&mut list, key::PAGE_DOWN);
        assert_eq!((list.focused(), list.top()), (Some(17), 9));
        focus(&mut list, key::END);
        assert_eq!((list.focused(), list.top()), (Some(29), 21));
        focus(&mut list, key::HOME);
        assert_eq!((list.focused(), list.top()), (Some(0), 0));
        assert_eq!(list.find_by_first_glyph(u32::from(b'R')), Some(1));
        assert_eq!(list.next_item(None, state::SELECTED, false), Some(0));
        assert_eq!(list.next_item(Some(0), 0, false), Some(1));
        assert_eq!(list.next_item(Some(5), 0, true), Some(4));
    }

    #[test]
    fn rows_and_columns_are_inserted_deleted_and_reordered() {
        let mut list = example();
        assert!(list.delete(0));
        assert_eq!(list.text(0, 0), units("Bananas"));
        assert_eq!(list.focused(), Some(0));
        list.insert_column(1, &units("Mid"), 40, 0, 0);
        assert_eq!(list.text(0, 2), units("5"));
        assert!(list.text(0, 1).is_empty());
        assert!(list.set_text(0, 1, &units("m")));
        assert!(list.delete_column(1));
        assert_eq!(list.text(0, 1), units("5"));
        let order = [3u16, 2, 1, 0];
        assert!(list.reorder(&order));
        assert_eq!(list.text(0, 0), units("Elderberries"));
        assert_eq!(list.text(3, 1), units("5"));
        assert!(!list.reorder(&[0, 0, 1, 2]));
        assert_eq!(list.sorted_position(&units("cabbage"), false), 0);
        assert_eq!(list.sorted_position(&units("cabbage"), true), 2);
        list.clear();
        assert_eq!(list.count(), 0);
    }

    #[test]
    fn a_full_store_packs_its_texts_and_fails_honestly() {
        let mut list = ListView::new();
        list.insert_column(0, &units("A"), 50, 0, 0);
        let long = vec![u16::from(b'x'); 16 * 1024];
        assert!(list.insert(0, &long, 0, 0).is_some());
        assert!(list.insert(1, &long, 0, 0).is_some());
        assert!(list.insert(2, &units("y"), 0, 0).is_none());
        assert!(list.set_text(0, 0, &units("short")));
        assert!(list.insert(2, &units("y"), 0, 0).is_some());
        let longer = vec![u16::from(b'z'); 16 * 1024 - 5];
        assert!(list.set_text(0, 0, &longer));
        assert_eq!(list.text(1, 0), long);
        assert_eq!(list.text(2, 0), units("y"));
    }

    #[test]
    fn stable_sorting_keeps_equal_items_in_order() {
        let keys = [3, 1, 2, 1, 3, 0];
        let mut order: Vec<u16> = (0..6).collect();
        let mut scratch = vec![0u16; 6];
        stable_sort(&mut order, &mut scratch, &mut |a: u16, b: u16| {
            keys[usize::from(a)].cmp(&keys[usize::from(b)])
        });
        assert_eq!(order, [5, 1, 3, 2, 0, 4]);
    }

    #[test]
    fn selections_look_by_focus_and_style() {
        let list = example();
        let rect = Rect::sized(12, 64, 456, 208);
        let look = ListLook {
            focused: false,
            ..focused_look()
        };
        let pixels = render(&list, rect, look);
        assert_eq!(at(&pixels, 14, 90), COMMON_LIGHT.selection_inactive);
        assert!(cell_has(&pixels, 19, 91, 0));
        assert_eq!(at(&pixels, 13, 89), COMMON_LIGHT.selection_inactive);
        let bare = ListLook {
            style: style::REPORT,
            ..look
        };
        let pixels = render(&list, rect, bare);
        assert_eq!(at(&pixels, 14, 90), 0x00ff_ffff);
        let mut first_cell = example();
        first_cell.extended = 0;
        let pixels = render(&first_cell, rect, focused_look());
        assert_eq!(at(&pixels, 100, 95), 0x0000_78d7);
        assert_eq!(at(&pixels, 250, 95), 0x00ff_ffff);
        assert!(cell_has(&pixels, 279, 91, 0));
        assert_eq!(at(&pixels, 212, 89), 0x0000_78d7);
        let mut unselected = example();
        let wanted = Wanted {
            focus: None,
            selection: Selection::Clear,
        };
        apply_all(&mut unselected, &wanted, true);
        let pixels = render(&unselected, rect, focused_look());
        assert_eq!(at(&pixels, 13, 89), 0);
        assert_eq!(at(&pixels, 14, 89), 0x00ff_ffff);
    }

    #[test]
    fn grid_lines_and_check_boxes_are_drawn_in_the_rows() {
        let mut list = example();
        list.extended |= extended::GRID_LINES | extended::CHECK_BOXES;
        let wanted = Wanted {
            focus: None,
            selection: Selection::Set {
                row: Some(1),
                state: state::image_bits(state::CHECKED),
                mask: state::STATE_IMAGE_MASK,
            },
        };
        apply_all(&mut list, &wanted, true);
        let rect = Rect::sized(12, 64, 456, 208);
        let pixels = render(&list, rect, focused_look());
        assert_eq!(at(&pixels, 100, 128), COMMON_LIGHT.header_line);
        assert_eq!(at(&pixels, 212, 200), COMMON_LIGHT.header_line);
        assert_eq!(at(&pixels, 16, 112), LIGHT.box_border);
        assert_eq!(at(&pixels, 19, 119), LIGHT.box_mark);
        assert!(cell_has(&pixels, 39, 111, 0));
        let layout = list.layout(rect, AHK_STYLE);
        assert_eq!(
            list.hit(&layout, 20, 115),
            ListHit::Cell {
                index: 1,
                column: 0,
                part: CellPart::CheckBox
            }
        );
    }

    #[test]
    fn long_lists_and_wide_columns_get_scroll_bars() {
        let mut list = ListView::new();
        list.insert_column(0, &units("A"), 300, 0, 0);
        list.insert_column(1, &units("B"), 300, 0, 0);
        for index in 0..30 {
            list.insert(index, &units("row"), 0, 0);
        }
        let rect = Rect::sized(0, 0, 456, 208);
        let layout = list.layout(rect, style::REPORT);
        assert_eq!(layout.vertical, Some(Rect::new(439, 1, 455, 191)));
        assert_eq!(layout.horizontal, Some(Rect::new(1, 191, 439, 207)));
        assert_eq!(layout.header, Rect::sized(1, 1, 438, 24));
        assert_eq!(layout.visible, 8);
        list.set_hpos(1000, &layout);
        assert_eq!(list.hpos, 600 - 438);
        assert_eq!(list.hit(&layout, 447, 5), ListHit::Vertical(Part::LineUp));
        assert_eq!(list.hit(&layout, 5, 199), ListHit::Horizontal(Part::LineUp));
        assert_eq!(list.hit(&layout, 447, 199), ListHit::Nowhere);
        let mut pixels = vec![0u32; (WIDTH * HEIGHT) as usize];
        let mut surface = Surface::new(&mut pixels, WIDTH, HEIGHT).unwrap();
        let look = ListLook {
            style: style::REPORT,
            focused: false,
            enabled: true,
            header_window: false,
        };
        paint(
            &mut surface,
            rect,
            &list,
            look,
            &NoImages,
            &LIGHT,
            &COMMON_LIGHT,
        );
        assert_eq!(at(&pixels, 447, 20), LIGHT.scroll_thumb);
        assert_eq!(at(&pixels, 447, 199), LIGHT.scroll_track);
        assert_eq!(at(&pixels, 7, 199), LIGHT.scroll_arrow);
        let info = list.horizontal_info(&layout);
        let (start, _) = scroll_bar::thumb(438, &info).unwrap();
        assert_eq!(thumb_position(438, &info, start), list.hpos);
    }

    #[test]
    fn other_views_draw_as_a_report_without_the_header() {
        let mut list = example();
        list.columns_mut().set_width(0, 20);
        let rect = Rect::sized(12, 64, 456, 208);
        let look = ListLook {
            style: style::LIST | style::SHOW_SELECTION_ALWAYS,
            ..focused_look()
        };
        let layout = list.layout(rect, look.style);
        assert!(layout.header.is_empty());
        assert_eq!(layout.rows.top, 65);
        assert_eq!(layout.total, 454);
        let pixels = render(&list, rect, look);
        assert!(cell_has(&pixels, 19, 67, 0x00ff_ffff));
        assert_eq!(at(&pixels, 400, 70), 0x0000_78d7);
        assert_eq!(list.row_bounds(&layout, 1), Rect::sized(13, 85, 454, 20));
    }
}

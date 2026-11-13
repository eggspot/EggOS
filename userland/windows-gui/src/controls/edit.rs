//! `EDIT` (sections 3 and 13 of the design): the [`Edit`] model (its text, the selection
//! between an anchor and the caret, scroll in whole cells, `ES_PASSWORD`, `ES_NUMBER` and
//! `ES_MULTILINE`'s lines) and its painting.

use super::scroll_bar::{self, BarLook, ScrollBar, ScrollInfo, mask};
use super::{Palette, cell_top, framed, ring};
use crate::surface::{Rect, Surface};
use crate::text::{self, CELL_HEIGHT, CELL_WIDTH};
use eggos_font as font;

/// The most UTF-16 units an edit holds.
pub const EDIT_CAPACITY: usize = 1024;
const TEXT_INSET: i32 = 4;
const LINES_TOP: i32 = 2;
const DARK_FOCUS_INSET: i32 = 2;
const LINE_FEED: u16 = 0x0a;
const CARRIAGE_RETURN: u16 = 0x0d;
const TAB: u16 = 0x09;
const LINE_BREAK: [u16; 2] = [CARRIAGE_RETURN, LINE_FEED];
const SPACE: u16 = 0x20;
const BAR_CELLS: usize = (scroll_bar::WIDTH / CELL_WIDTH) as usize;

/// `EDIT`'s styles this layer reads.
pub mod style {
    pub const MULTILINE: u32 = 0x0004;
    pub const AUTO_HORIZONTAL_SCROLL: u32 = 0x0080;
    pub const VERTICAL_SCROLL_BAR: u32 = 0x0020_0000;
    pub const HORIZONTAL_SCROLL_BAR: u32 = 0x0010_0000;
    pub const PASSWORD: u32 = 0x0020;
    pub const NO_HIDE_SELECTION: u32 = 0x0100;
    pub const READ_ONLY: u32 = 0x0800;
    pub const WANT_RETURN: u32 = 0x1000;
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

/// What an edit's window shows: whole cells across and, for a multi-line edit, whole lines
/// down. A bare cell count is a single row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub cells: usize,
    pub rows: usize,
}

impl From<usize> for View {
    fn from(cells: usize) -> View {
        View { cells, rows: 1 }
    }
}

/// An edit's text and caret. A multi-line edit breaks its lines at `\r\n` (or a lone `\n`)
/// and never puts the caret between the two.
#[derive(Debug, Clone)]
pub struct Edit {
    units: [u16; EDIT_CAPACITY],
    length: usize,
    anchor: usize,
    caret: usize,
    offset: usize,
    top: usize,
    limit: usize,
    pub read_only: bool,
    pub password: u16,
    pub digits_only: bool,
    pub keep_selection: bool,
    pub multiline: bool,
    /// Whether the text changed since `WM_SETTEXT` or `EM_SETMODIFY` (`EM_GETMODIFY`).
    pub modified: bool,
    /// Whether a multi-line edit wraps its lines (no `ES_AUTOHSCROLL`, D351).
    pub wraps: bool,
    wrap_cells: usize,
    /// Whether it draws a vertical and a horizontal scroll bar inside its border
    /// (`WS_VSCROLL`, `WS_HSCROLL` on a multi-line edit, D351).
    pub bars: (bool, bool),
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
    pub const PRIOR: u32 = 0x21;
    pub const NEXT: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
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
            top: 0,
            limit: EDIT_CAPACITY,
            read_only: false,
            password: 0,
            digits_only: false,
            keep_selection: false,
            multiline: false,
            modified: false,
            wraps: false,
            wrap_cells: 0,
            bars: (false, false),
        }
    }

    /// A new edit set up from its window's `ES_*` styles.
    pub fn with_style(style: u32) -> Edit {
        let mut edit = Edit::new();
        edit.read_only = style & style::READ_ONLY != 0;
        edit.digits_only = style & style::NUMBER != 0;
        edit.keep_selection = style & style::NO_HIDE_SELECTION != 0;
        edit.multiline = style & style::MULTILINE != 0;
        edit.wraps = edit.multiline && style & style::AUTO_HORIZONTAL_SCROLL == 0;
        edit.bars = match edit.multiline {
            true => (
                style & style::VERTICAL_SCROLL_BAR != 0,
                style & style::HORIZONTAL_SCROLL_BAR != 0 && !edit.wraps,
            ),
            false => (false, false),
        };
        if style & style::PASSWORD != 0 && !edit.multiline {
            edit.password = DEFAULT_PASSWORD_CHARACTER;
        }
        edit
    }

    /// What the text shows in a window that shows `view`: less the scroll bars' cells and row.
    pub fn inner(&self, view: View) -> View {
        View {
            cells: view.cells.saturating_sub(if self.bars.0 { BAR_CELLS } else { 0 }),
            rows: view.rows.saturating_sub(usize::from(self.bars.1)).max(1),
        }
    }

    /// Takes the window's `view`, so the lines wrap at its width: what the text shows.
    pub fn fit(&mut self, view: impl Into<View>) -> View {
        let inner = self.inner(view.into());
        self.wrap_cells = match self.wraps {
            true => inner.cells.max(1),
            false => 0,
        };
        inner
    }

    /// The selection's start and end (equal when nothing is selected); the caret is one end.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// `EM_SETSEL`: `start` -1 removes the selection, `end` -1 (or past the text) is the text's
    /// end; the caret goes to `end`.
    pub fn set_selection(&mut self, start: isize, end: isize, view: impl Into<View>) {
        let view = self.fit(view);
        let at = |value: isize| match usize::try_from(value) {
            Ok(value) => self.boundary(value.min(self.length)),
            Err(_) => self.length,
        };
        match start {
            -1 => self.anchor = self.caret,
            start => {
                let (anchor, caret) = (at(start), at(end));
                self.anchor = anchor;
                self.caret = caret;
            }
        }
        self.scroll(view);
    }

    /// `EM_REPLACESEL` and typing: the selection replaced by `units`, as many as the limit
    /// leaves room for, the caret after them.
    pub fn replace_selection(&mut self, units: &[u16], view: impl Into<View>) -> EditOutcome {
        let view = self.fit(view);
        self.replace(units, view)
    }

    fn replace(&mut self, units: &[u16], view: View) -> EditOutcome {
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
        self.modified = true;
        self.caret = self.boundary(start + inserted.len());
        self.anchor = self.caret;
        self.scroll(view);
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
        self.top = 0;
        self.modified = false;
    }

    /// Puts the caret after the text, as the picker does after filling in a name.
    pub fn caret_to_end(&mut self, view: impl Into<View>) {
        let view = self.fit(view);
        self.caret = self.length;
        self.anchor = self.caret;
        self.scroll(view);
    }

    /// A typed character: a printable one replaces the selection; with `ES_NUMBER` only
    /// digits are taken. A multi-line edit also takes Enter (`\r`, or Ctrl+Enter's `\n`) as a
    /// line break and Tab as a tab.
    pub fn character(&mut self, character: u32, view: impl Into<View>) -> EditOutcome {
        let units = match character {
            0x0a | 0x0d if self.multiline => &LINE_BREAK[..],
            0x09 if self.multiline => &[TAB][..],
            _ => &[],
        };
        let printable = character >= 0x20 && character != 0x7f && character <= 0xffff;
        let refused = self.digits_only && !(0x30..=0x39).contains(&character);
        if !(printable || !units.is_empty()) || refused || self.read_only {
            return EditOutcome::Unchanged;
        }
        let typed = [character as u16];
        let units = match units.is_empty() {
            true => &typed[..],
            false => units,
        };
        let (start, end) = self.selection();
        if self.length - (end - start) + units.len() > self.limit {
            return EditOutcome::Unchanged;
        }
        self.replace_selection(units, view)
    }

    /// A key press without modifiers: Backspace, Delete and the caret's moves.
    pub fn key(&mut self, virtual_key: u32, view: impl Into<View>) -> EditOutcome {
        self.key_with(virtual_key, Modifiers::default(), view)
    }

    /// A key press: Backspace and Delete erase the selection or a character; the arrows, Home
    /// and End move the caret (by words with Ctrl, to the text's ends with Ctrl+Home and
    /// Ctrl+End in a multi-line edit), Shift extending the selection; Ctrl+A selects all. In
    /// a multi-line edit Up and Down move a line and Page Up and Page Down a window's lines less
    /// one, keeping the column.
    pub fn key_with(
        &mut self,
        virtual_key: u32,
        held: Modifiers,
        view: impl Into<View>,
    ) -> EditOutcome {
        let view = self.fit(view);
        let (start, end) = self.selection();
        let selected = start != end;
        let vertical = matches!(virtual_key, key::UP | key::DOWN | key::PRIOR | key::NEXT);
        let outcome = match virtual_key {
            key::A if held.control => self.select(0, self.length),
            key::BACK | key::DELETE if self.read_only => EditOutcome::Unchanged,
            key::BACK | key::DELETE if selected => self.replace(&[], view),
            key::BACK if self.caret > 0 => {
                self.anchor = self.step_back(self.caret);
                self.replace(&[], view)
            }
            key::DELETE if self.caret < self.length => {
                self.anchor = self.step_forward(self.caret);
                self.replace(&[], view)
            }
            _ if vertical && !self.multiline => EditOutcome::Unchanged,
            key::LEFT | key::RIGHT | key::HOME | key::END | key::UP | key::DOWN | key::PRIOR
            | key::NEXT => {
                let lines = view.rows.max(2) as isize - 1;
                let target = match virtual_key {
                    key::LEFT if held.control => self.previous_word(self.caret),
                    key::RIGHT if held.control => self.next_word(self.caret),
                    key::LEFT if selected && !held.shift => start,
                    key::RIGHT if selected && !held.shift => end,
                    key::LEFT => self.step_back(self.caret),
                    key::RIGHT => self.step_forward(self.caret),
                    key::HOME if held.control => 0,
                    key::HOME => self.line_start_at(self.caret),
                    key::END if held.control => self.length,
                    key::END => self.line_end_at(self.caret),
                    key::UP => self.line_moved(-1),
                    key::DOWN => self.line_moved(1),
                    key::PRIOR => self.line_moved(-lines),
                    _ => self.line_moved(lines),
                };
                match held.shift {
                    true => self.select(self.anchor, target),
                    false => self.select(target, target),
                }
            }
            _ => EditOutcome::Unchanged,
        };
        self.scroll(view);
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

    fn is_line_break_at(&self, index: usize) -> bool {
        self.multiline
            && index + 1 < self.length
            && self.units[index] == CARRIAGE_RETURN
            && self.units[index + 1] == LINE_FEED
    }

    fn boundary(&self, index: usize) -> usize {
        match index > 0 && self.is_line_break_at(index - 1) {
            true => index - 1,
            false => index,
        }
    }

    fn step_back(&self, from: usize) -> usize {
        match from >= 2 && self.is_line_break_at(from - 2) {
            true => from - 2,
            false => from.saturating_sub(1),
        }
    }

    fn step_forward(&self, from: usize) -> usize {
        match self.is_line_break_at(from) {
            true => from + 2,
            false => (from + 1).min(self.length),
        }
    }

    fn breaks_after(&self, index: usize) -> bool {
        let unit = self.units[index];
        let pair = index + 1 < self.length && self.units[index + 1] == LINE_FEED;
        unit == LINE_FEED || (unit == CARRIAGE_RETURN && !pair)
    }

    fn paragraph_start(&self, index: usize) -> usize {
        let mut at = index.min(self.length);
        while at > 0 && !self.breaks_after(at - 1) {
            at -= 1;
        }
        at
    }

    fn paragraph_end(&self, start: usize) -> usize {
        let mut at = start;
        while at < self.length
            && !(self.multiline
                && (self.units[at] == LINE_FEED || self.units[at] == CARRIAGE_RETURN))
        {
            at += 1;
        }
        at
    }

    fn after_break(&self, at: usize) -> usize {
        match self.is_line_break_at(at) {
            true => at + 2,
            false => at + 1,
        }
    }

    /// The display line that starts at `start`: where its text ends and where the next one
    /// starts (none after the last). A line longer than the wrap width breaks after the last
    /// space that fits, or at the width when no space does (D351, the greedy rule).
    fn row(&self, start: usize) -> (usize, Option<usize>) {
        let end = self.paragraph_end(start);
        let width = self.wrap_cells;
        if width == 0 || end - start <= width {
            return (end, (end < self.length).then(|| self.after_break(end)));
        }
        let limit = start + width;
        let cut = (start + 1..=limit)
            .rev()
            .find(|&at| self.units[at - 1] == SPACE)
            .unwrap_or(limit);
        (cut, Some(cut))
    }

    /// Where the display line holding `index` starts; a caret at a wrapped line's break is at
    /// the start of the next line.
    pub fn line_start_at(&self, index: usize) -> usize {
        if !self.multiline {
            return 0;
        }
        let index = index.min(self.length);
        let mut start = self.paragraph_start(index);
        loop {
            match self.row(start) {
                (end, Some(next)) if next == end && next <= index && next > start => start = next,
                _ => return start,
            }
        }
    }

    /// Where the display line holding `index` ends, before its break or wrap.
    pub fn line_end_at(&self, index: usize) -> usize {
        self.row(self.line_start_at(index)).0
    }

    fn next_line_start(&self, start: usize) -> Option<usize> {
        match self.multiline {
            true => self.row(start).1,
            false => None,
        }
    }

    /// `EM_LINEFROMCHAR`: the display line holding `index`, from 0.
    pub fn line_of(&self, index: usize) -> usize {
        if !self.multiline {
            return 0;
        }
        let index = index.min(self.length);
        let mut line = 0;
        let mut start = 0;
        while let Some(next) = self.row(start).1.filter(|&next| next <= index && next > start) {
            line += 1;
            start = next;
        }
        line
    }

    /// `EM_GETLINECOUNT`: the lines in the text, at least 1.
    pub fn line_count(&self) -> usize {
        self.line_of(self.length) + 1
    }

    /// `EM_LINEINDEX`: where `line` starts, if the text has it.
    pub fn line_index(&self, line: usize) -> Option<usize> {
        let mut start = 0;
        for _ in 0..line {
            start = self.next_line_start(start)?;
        }
        Some(start)
    }

    /// `EM_GETLINE`: `line`'s units without its break, if the text has it.
    pub fn line(&self, line: usize) -> Option<&[u16]> {
        let start = self.line_index(line)?;
        Some(&self.units[start..self.line_end_at(start)])
    }

    /// `EM_GETFIRSTVISIBLELINE`: the top line shown by a multi-line edit, the first character
    /// shown by a single-line one.
    pub fn first_visible_line(&self) -> usize {
        match self.multiline {
            true => self.top,
            false => self.offset,
        }
    }

    /// `EM_LINESCROLL`: a multi-line edit scrolled by `columns` and `lines`, kept within its
    /// text; whether it moved.
    pub fn scroll_by(&mut self, columns: isize, lines: isize) -> bool {
        if !self.multiline {
            return false;
        }
        let top = self
            .top
            .saturating_add_signed(lines)
            .min(self.line_count() - 1);
        let offset = match self.wrap_cells {
            0 => self.offset.saturating_add_signed(columns).min(EDIT_CAPACITY),
            _ => 0,
        };
        let moved = (top, offset) != (self.top, self.offset);
        self.top = top;
        self.offset = offset;
        moved
    }

    /// `EM_SCROLLCARET`: scrolled until the caret shows.
    pub fn scroll_to_caret(&mut self, view: impl Into<View>) {
        let view = self.fit(view);
        self.scroll(view);
    }

    /// Where a multi-line edit at `rect` draws its scroll bars (D351): inside its border, the
    /// vertical one at the right, the horizontal one at the bottom.
    pub fn bar_rects(&self, rect: Rect) -> (Option<Rect>, Option<Rect>) {
        let inside = rect.inset(1);
        let width = scroll_bar::WIDTH;
        let (vertical, horizontal) = self.bars;
        let right = Rect::new(
            inside.right - width,
            inside.top,
            inside.right,
            inside.bottom - if horizontal { width } else { 0 },
        );
        let bottom = Rect::new(
            inside.left,
            inside.bottom - width,
            inside.right - if vertical { width } else { 0 },
            inside.bottom,
        );
        (vertical.then_some(right), horizontal.then_some(bottom))
    }

    fn widest_line(&self) -> usize {
        let mut widest = 0;
        let mut start = Some(0);
        while let Some(at) = start {
            let (end, next) = self.row(at);
            widest = widest.max(end - at);
            start = next.filter(|&next| next > at);
        }
        widest
    }

    fn bar_model(&self, vertical: bool, view: View) -> ScrollBar {
        let info = match vertical {
            true => ScrollInfo {
                minimum: 0,
                maximum: self.line_count() as i32 - 1,
                page: view.rows as u32,
                position: self.top as i32,
            },
            false => ScrollInfo {
                minimum: 0,
                maximum: self.widest_line() as i32,
                page: view.cells as u32,
                position: self.offset as i32,
            },
        };
        let mut bar = ScrollBar::new(vertical);
        bar.set_info(mask::RANGE | mask::PAGE | mask::POSITION, &info);
        bar
    }

    /// A press at `at` (pixels in the edit at `rect`, which shows `view`): on one of its scroll
    /// bars, the edit scrolls as the part pressed asks, a line or a page, without moving the
    /// caret; whether the press was on a bar.
    pub fn press_bar(&mut self, rect: Rect, at: (i32, i32), view: impl Into<View>) -> bool {
        let view = self.fit(view);
        let (right, bottom) = self.bar_rects(rect);
        let pressed = [(true, right), (false, bottom)]
            .into_iter()
            .find_map(|(vertical, bar)| {
                bar.filter(|bar| bar.contains(at.0, at.1))
                    .map(|bar| (vertical, bar))
            });
        let Some((vertical, place)) = pressed else {
            return false;
        };
        let mut bar = self.bar_model(vertical, view);
        let (offset, length) = match vertical {
            true => (at.1 - place.top, place.height()),
            false => (at.0 - place.left, place.width()),
        };
        let scroll = bar.button_down(offset, length);
        let _ = bar.button_up();
        let shown = if vertical { view.rows } else { view.cells };
        let page = shown.max(2) as isize - 1;
        let step = match scroll.map(|scroll| scroll.code) {
            Some(scroll_bar::request::LINE_UP) => -1,
            Some(scroll_bar::request::LINE_DOWN) => 1,
            Some(scroll_bar::request::PAGE_UP) => -page,
            Some(scroll_bar::request::PAGE_DOWN) => page,
            _ => 0,
        };
        match vertical {
            true => self.scroll_by(0, step),
            false => self.scroll_by(step, 0),
        };
        true
    }

    fn line_moved(&self, lines: isize) -> usize {
        let line = self.line_of(self.caret);
        let target = line
            .saturating_add_signed(lines)
            .min(self.line_count() - 1);
        let column = self.caret - self.line_start_at(self.caret);
        match self.line_index(target) {
            Some(start) => (start + column).min(self.line_end_at(start)),
            None => self.caret,
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

    fn position_at(&self, (x, y): (i32, i32)) -> usize {
        let cells = ((x - TEXT_INSET + CELL_WIDTH / 2).max(0) / CELL_WIDTH) as usize;
        if !self.multiline {
            return (self.offset + cells).min(self.length);
        }
        let row = ((y - LINES_TOP).max(0) / CELL_HEIGHT) as usize;
        let line = (self.top + row).min(self.line_count() - 1);
        let start = self.line_index(line).unwrap_or(self.length);
        (start + self.offset + cells).min(self.line_end_at(start))
    }

    /// A click at `at`, pixels from the edit's top left: the caret at the nearest cell
    /// boundary, the anchor there too unless `extend` (Shift held).
    pub fn click(&mut self, at: (i32, i32), extend: bool, view: impl Into<View>) -> EditOutcome {
        let view = self.fit(view);
        let caret = self.position_at(at);
        let anchor = match extend {
            true => self.anchor,
            false => caret,
        };
        let outcome = self.select(anchor, caret);
        self.scroll(view);
        outcome
    }

    /// The pointer dragged to `at` with the button held: only the caret follows it.
    pub fn drag(&mut self, at: (i32, i32), view: impl Into<View>) -> EditOutcome {
        let view = self.fit(view);
        let caret = self.position_at(at);
        let outcome = self.select(self.anchor, caret);
        self.scroll(view);
        outcome
    }

    /// A double-click at `at`: the word under it selected (a run of letters, digits and `_`,
    /// of other glyphs, or of spaces), the caret at its end.
    pub fn select_word(&mut self, at: (i32, i32), view: impl Into<View>) -> EditOutcome {
        let view = self.fit(view);
        if self.length == 0 {
            return EditOutcome::Unchanged;
        }
        let index = self.position_at(at).min(self.length - 1);
        let class = self.class_at(index);
        let mut start = index;
        while start > 0 && self.class_at(start - 1) == class {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.length && self.class_at(end) == class {
            end += 1;
        }
        let outcome = self.select(self.boundary(start), self.boundary(end));
        self.scroll(view);
        outcome
    }

    fn scroll(&mut self, view: View) {
        let visible = view.cells.max(1);
        if self.multiline {
            let column = self.caret - self.line_start_at(self.caret);
            self.offset = match self.wrap_cells {
                0 => self
                    .offset
                    .max((column + 1).saturating_sub(visible))
                    .min(column),
                _ => 0,
            };
            let line = self.line_of(self.caret);
            self.top = self
                .top
                .max((line + 1).saturating_sub(view.rows.max(1)))
                .min(line);
            return;
        }
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
        Some(' ' | '\t' | '\r' | '\n') => WordClass::Space,
        Some(character) if character.is_alphanumeric() || character == '_' => WordClass::Word,
        _ => WordClass::Other,
    }
}

/// The cells an edit `width` wide shows.
pub fn visible_cells(width: i32) -> usize {
    ((width - 2 * TEXT_INSET).max(0) / CELL_WIDTH) as usize
}

/// The whole lines a multi-line edit `height` tall shows, at least 1: its lines start 2 pixels
/// down and end 2 pixels above its bottom.
pub fn visible_lines(height: i32) -> usize {
    ((height - 2 * LINES_TOP).max(0) / CELL_HEIGHT).max(1) as usize
}

/// What an edit `width` by `height` shows.
pub fn view(width: i32, height: i32) -> View {
    View {
        cells: visible_cells(width),
        rows: visible_lines(height),
    }
}

/// How an edit looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditLook {
    pub focused: bool,
    pub enabled: bool,
    pub hovered: bool,
}

#[derive(Clone, Copy)]
struct Run<'a> {
    colour: u32,
    selection: Option<(usize, usize)>,
    password: u16,
    palette: &'a Palette,
}

fn paint_run(
    surface: &mut Surface<'_>,
    (left, top): (i32, i32),
    edit: &Edit,
    (first, end): (usize, usize),
    cells: usize,
    run: Run<'_>,
) {
    for (index, character) in char::decode_utf16(edit.units[first..end].iter().copied())
        .take(cells)
        .enumerate()
    {
        let shown_unit = match run.password {
            0 => character.map_or(font::REPLACEMENT, |character| {
                font::glyph_id(u32::from(character))
            }),
            password => font::glyph_id(u32::from(password)),
        };
        let x = left + index as i32 * CELL_WIDTH;
        let selected = run
            .selection
            .is_some_and(|(start, end)| (start..end).contains(&(first + index)));
        let glyph_colour = match selected {
            true => {
                surface.fill(
                    Rect::sized(x, top, CELL_WIDTH, CELL_HEIGHT),
                    run.palette.highlight,
                );
                run.palette.highlight_text
            }
            false => run.colour,
        };
        text::glyph(surface, x, top, shown_unit, false, glyph_colour, None);
    }
}

fn paint_bars(
    surface: &mut Surface<'_>,
    rect: Rect,
    edit: &Edit,
    shown: View,
    enabled: bool,
    palette: &Palette,
) {
    let (right, bottom) = edit.bar_rects(rect);
    let look = BarLook {
        enabled,
        focused: false,
        hovered: false,
    };
    for (vertical, place) in [(true, right), (false, bottom)] {
        if let Some(place) = place {
            let bar = edit.bar_model(vertical, shown);
            scroll_bar::paint(surface, place, &bar, look, palette);
        }
    }
    if let (Some(right), Some(bottom)) = (right, bottom) {
        let corner = Rect::new(right.left, bottom.top, right.right, bottom.bottom);
        surface.fill(corner, palette.scroll_track);
    }
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
        () if look.hovered => (palette.field_fill, palette.field_hover_border, palette.text),
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
    let shown = edit.inner(view(rect.width(), rect.height()));
    let selection_shown = look.enabled && (look.focused || edit.keep_selection);
    let run = Run {
        colour,
        selection: selection_shown.then(|| edit.selection()),
        password: edit.password,
        palette,
    };
    let left = rect.left + TEXT_INSET;
    match edit.multiline {
        false => {
            let top = cell_top(rect.top, rect.height());
            let first = edit.offset.min(edit.length);
            paint_run(surface, (left, top), edit, (first, edit.length), shown.cells, run);
            if caret_shown {
                let x = left + (edit.caret - edit.offset.min(edit.caret)) as i32 * CELL_WIDTH;
                surface.fill(Rect::sized(x, top, 1, CELL_HEIGHT), palette.caret);
            }
        }
        true => {
            let mut start = edit.line_index(edit.top);
            for row in 0..=shown.rows {
                let Some(line_start) = start else {
                    break;
                };
                let end = edit.line_end_at(line_start);
                let top = rect.top + LINES_TOP + row as i32 * CELL_HEIGHT;
                let first = (line_start + edit.offset).min(end);
                paint_run(surface, (left, top), edit, (first, end), shown.cells, run);
                let next = edit.next_line_start(line_start);
                let hard_break = end < edit.length && next != Some(end);
                let break_selected = hard_break
                    && run
                        .selection
                        .is_some_and(|(from, to)| (from..to).contains(&end));
                let break_column = (end - line_start).checked_sub(edit.offset);
                if let (true, Some(column)) = (break_selected, break_column) {
                    let x = left + column as i32 * CELL_WIDTH;
                    let cell = Rect::sized(x, top, CELL_WIDTH, CELL_HEIGHT);
                    surface.fill(cell, palette.highlight);
                }
                start = next;
            }
            let line = edit.line_of(edit.caret);
            let column = edit.caret - edit.line_start_at(edit.caret);
            let row = line.wrapping_sub(edit.top);
            if caret_shown && row < shown.rows && column >= edit.offset {
                let x = left + (column - edit.offset) as i32 * CELL_WIDTH;
                let top = rect.top + LINES_TOP + row as i32 * CELL_HEIGHT;
                surface.fill(Rect::sized(x, top, 1, CELL_HEIGHT), palette.caret);
            }
        }
    }
    if edit.multiline {
        paint_bars(surface, rect, edit, shown, look.enabled, palette);
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
        edit.click((4 + 9, 0), false, 24);
        assert_eq!(edit.caret(), 1);
        edit.click((4 + 12, 0), false, 24);
        assert_eq!(edit.caret(), 2);
        edit.click((500, 0), false, 24);
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
            hovered: false,
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
            hovered: false,
        };
        paint(&mut surface, Rect::new(0, 0, 100, 24), &edit, look, &LIGHT);
        let mut star = vec![0u32; 8 * 16];
        let mut star_surface = Surface::new(&mut star, 8, 16).unwrap();
        star_surface.fill(Rect::new(0, 0, 8, 16), 0x00ff_ffff);
        text::glyph(
            &mut star_surface,
            0,
            0,
            font::glyph_id(0x2a),
            false,
            0,
            None,
        );
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
        assert_eq!(edit.select_word((4 + 8 * 5, 0), 24), EditOutcome::Moved);
        assert_eq!(edit.selection(), (4, 7));
        edit.click((4, 0), false, 24);
        edit.drag((4 + 8 * 3, 0), 24);
        assert_eq!((edit.selection(), edit.caret()), ((0, 3), 3));
        edit.click((4 + 8 * 5, 0), true, 24);
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
            hovered: false,
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 100, 24),
            &edit,
            focused,
            &LIGHT,
        );
        assert_eq!(surface.get(4, 4), Some(LIGHT.highlight));
        assert_eq!(surface.get(13, 4), Some(LIGHT.field_fill));
        let unfocused = EditLook {
            focused: false,
            ..focused
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 100, 24),
            &edit,
            unfocused,
            &LIGHT,
        );
        assert_eq!(surface.get(4, 4), Some(LIGHT.field_fill));
    }

    #[test]
    fn a_hovered_edit_darkens_its_border_unless_it_has_the_focus() {
        let edit = Edit::new();
        let mut pixels = vec![0u32; 100 * 24];
        let mut surface = Surface::new(&mut pixels, 100, 24).unwrap();
        let hovered = EditLook {
            focused: false,
            enabled: true,
            hovered: true,
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 100, 24),
            &edit,
            hovered,
            &LIGHT,
        );
        assert_eq!(surface.get(0, 0), Some(LIGHT.field_hover_border));
        let focused = EditLook {
            focused: true,
            ..hovered
        };
        paint(
            &mut surface,
            Rect::new(0, 0, 100, 24),
            &edit,
            focused,
            &LIGHT,
        );
        assert_eq!(surface.get(0, 0), Some(LIGHT.field_focus_border));
    }

    const PAGE: View = View { cells: 24, rows: 2 };

    fn multiline(text: &str) -> Edit {
        let mut edit = Edit::with_style(style::MULTILINE);
        typed(&mut edit, text, 24);
        edit
    }

    #[test]
    fn enter_breaks_the_line_with_cr_lf_and_the_caret_steps_over_it() {
        let mut edit = multiline("egg");
        assert_eq!(edit.character(0x0d, PAGE), EditOutcome::Changed);
        typed(&mut edit, "os", 24);
        assert_eq!(text_of(&edit), "egg\r\nos");
        assert_eq!((edit.line_count(), edit.line_of(edit.caret())), (2, 1));
        edit.key(key::HOME, PAGE);
        assert_eq!(edit.caret(), 5);
        edit.key(key::LEFT, PAGE);
        assert_eq!(edit.caret(), 3);
        edit.key(key::RIGHT, PAGE);
        assert_eq!(edit.caret(), 5);
        assert_eq!(edit.key(key::BACK, PAGE), EditOutcome::Changed);
        assert_eq!(text_of(&edit), "eggos");
        assert_eq!(Edit::new().character(0x0d, 24), EditOutcome::Unchanged);
        assert!(edit.modified);
        edit.set_text(&"a\rb\nc".encode_utf16().collect::<Vec<_>>());
        assert!(!edit.modified);
        assert_eq!((edit.line_count(), edit.line_index(2)), (3, Some(4)));
    }

    #[test]
    fn up_and_down_keep_the_column_within_the_shorter_line() {
        let mut edit = multiline("hello");
        edit.character(0x0d, PAGE);
        typed(&mut edit, "hi", 24);
        edit.character(0x0d, PAGE);
        typed(&mut edit, "world", 24);
        edit.key(key::UP, PAGE);
        assert_eq!(edit.caret(), 9);
        edit.key(key::UP, PAGE);
        assert_eq!(edit.caret(), 2);
        edit.key(key::END, PAGE);
        edit.key(key::NEXT, PAGE);
        assert_eq!(edit.caret(), 9);
        edit.key_with(key::HOME, CONTROL, PAGE);
        assert_eq!(edit.caret(), 0);
        edit.key_with(key::END, CONTROL, PAGE);
        assert_eq!(edit.caret(), edit.text().len());
    }

    #[test]
    fn the_lines_scroll_to_keep_the_caret_shown_and_answer_the_line_messages() {
        let mut edit = multiline("a\rb\rc\rd");
        assert_eq!(edit.line_count(), 4);
        edit.scroll_to_caret(PAGE);
        assert_eq!(edit.first_visible_line(), 3);
        assert_eq!(edit.line_index(1), Some(3));
        assert_eq!(edit.line_index(4), None);
        assert_eq!(edit.line(3), Some(&[u16::from(b'd')][..]));
        assert_eq!(edit.line_end_at(3), 4);
        assert!(edit.scroll_by(0, -5));
        assert_eq!(edit.first_visible_line(), 0);
        assert!(!edit.scroll_by(0, -1));
        edit.click((4, 4 + CELL_HEIGHT), false, PAGE);
        assert_eq!(edit.caret(), 3);
        edit.set_selection(2, 2, PAGE);
        assert_eq!(edit.selection(), (1, 1));
    }

    #[test]
    fn a_multi_line_edit_paints_each_line_on_its_own_row_with_the_caret_on_the_last() {
        let mut edit = multiline("ab\rcd");
        let mut pixels = vec![0u32; 120 * 48];
        let mut surface = Surface::new(&mut pixels, 120, 48).unwrap();
        let look = EditLook {
            focused: true,
            enabled: true,
            hovered: false,
        };
        assert!(edit.scroll_by(0, -1));
        edit.scroll_to_caret(view(120, 48));
        assert_eq!(edit.first_visible_line(), 0);
        paint(&mut surface, Rect::new(0, 0, 120, 48), &edit, look, &LIGHT);
        let caret_x = TEXT_INSET + 2 * CELL_WIDTH;
        assert_eq!(surface.get(caret_x, TEXT_INSET + CELL_HEIGHT + 1), Some(LIGHT.caret));
        assert_ne!(surface.get(caret_x, TEXT_INSET + 1), Some(LIGHT.caret));
    }

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn without_autohscroll_lines_wrap_after_the_last_space_that_fits() {
        let narrow = View { cells: 8, rows: 4 };
        let mut edit = Edit::with_style(style::MULTILINE);
        edit.set_text(&units("hello big world"));
        edit.fit(narrow);
        assert_eq!(edit.line_count(), 3);
        assert_eq!((edit.line_index(1), edit.line_index(2)), (Some(6), Some(10)));
        assert_eq!(edit.line_end_at(7), 10);
        assert_eq!((edit.line_of(10), edit.line_start_at(10)), (2, 10));
        let mut word = Edit::with_style(style::MULTILINE);
        word.set_text(&units("abcdefghij"));
        word.fit(narrow);
        assert_eq!((word.line_count(), word.line_index(1)), (2, Some(8)));
        let mut scrolling =
            Edit::with_style(style::MULTILINE | style::AUTO_HORIZONTAL_SCROLL);
        scrolling.set_text(&units("hello big world"));
        scrolling.fit(narrow);
        assert_eq!(scrolling.line_count(), 1);
    }

    #[test]
    fn the_edit_s_own_vertical_bar_scrolls_it_a_line_from_its_arrow() {
        let mut edit = Edit::with_style(style::MULTILINE | style::VERTICAL_SCROLL_BAR);
        assert_eq!(edit.inner(View { cells: 20, rows: 3 }).cells, 18);
        edit.set_text(&units("1\r2\r3\r4\r5"));
        let rect = Rect::new(0, 0, 120, 52);
        assert!(edit.press_bar(rect, (110, 45), view(120, 52)));
        assert_eq!(edit.first_visible_line(), 1);
        assert_eq!(edit.caret(), 0);
        assert!(!edit.press_bar(rect, (10, 10), view(120, 52)));
    }
}

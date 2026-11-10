//! Multi-line text that wraps at words (docs/ux/toolkit-v0.5.md section 5): the bytes live in
//! a text store the tree carries, with the index of where each visual line starts, which the
//! wrap pass rebuilds after an edit, a resize or a scale change and never while painting.

use crate::canvas::{Canvas, INSET_R4};
use crate::geometry::Rect;
use crate::text;
use crate::widget::{
    LINE_HEIGHT, Look, SCROLL_BAR_WIDTH, TEXT_AREA_CAPACITY, TEXT_AREA_LINE_PITCH,
    TEXT_AREA_LINES, TEXT_AREA_PADDING, TEXT_AREA_TAB, TEXT_AREA_TOP, focus_ring, scroll_bar,
};

/// A text area's state; its text is in the tree's store `store`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextArea {
    pub store: usize,
    /// The caret's byte.
    pub caret: usize,
    /// The selection's other end.
    pub anchor: usize,
    /// How far it is scrolled down, in whole lines' pixels.
    pub scroll: i32,
    /// The x the caret keeps across Up and Down, from the first vertical move until a
    /// horizontal move, a click or an edit.
    pub wanted_x: Option<i32>,
    pub placeholder: &'static str,
    /// Its visual lines.
    pub lines: usize,
}

impl TextArea {
    /// An empty area keeping its text in store `store`.
    pub const fn new(store: usize, placeholder: &'static str) -> Self {
        Self {
            store,
            caret: 0,
            anchor: 0,
            scroll: 0,
            wanted_x: None,
            placeholder,
            lines: 1,
        }
    }

    /// The selection's first and last byte.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
}

/// Where a text area at `rect` puts its text: the padding on the left, the padding and the
/// bar's strip (always kept) on the right.
pub fn inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + TEXT_AREA_PADDING,
        rect.y + TEXT_AREA_TOP,
        rect.width - 2 * TEXT_AREA_PADDING - SCROLL_BAR_WIDTH,
        rect.height - 2 * TEXT_AREA_TOP,
    )
}

/// How many whole lines a text area at `rect` shows.
pub fn visible_lines(rect: Rect) -> usize {
    (inner(rect).height / TEXT_AREA_LINE_PITCH).max(1) as usize
}

fn advance(character: char) -> i32 {
    match character {
        '\t' => TEXT_AREA_TAB,
        '\n' => 0,
        character => text::advance(character),
    }
}

/// A text area's bytes and its visual line index.
#[derive(Clone)]
pub struct Store {
    bytes: [u8; TEXT_AREA_CAPACITY],
    length: usize,
    lines: [u32; TEXT_AREA_LINES],
    line_count: usize,
}

impl core::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("length", &self.length)
            .field("line_count", &self.line_count)
            .finish()
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Store {
    /// No text, one empty line.
    pub const EMPTY: Self = Self {
        bytes: [0; TEXT_AREA_CAPACITY],
        length: 0,
        lines: [0; TEXT_AREA_LINES],
        line_count: 1,
    };

    /// Its text.
    pub fn text(&self) -> &str {
        match core::str::from_utf8(&self.bytes[..self.length]) {
            Ok(text) => text,
            Err(error) => core::str::from_utf8(&self.bytes[..error.valid_up_to()]).unwrap_or(""),
        }
    }

    /// How many bytes it holds.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Whether it holds no text.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// How many visual lines it has.
    pub fn line_count(&self) -> usize {
        self.line_count
    }

    /// Replaces its text with the longest prefix of `words` that fits, at a character boundary.
    pub fn set(&mut self, words: &str) {
        self.length = 0;
        self.insert(0, words);
    }

    /// Inserts `words` at byte `at`, as many whole characters as fit; the bytes inserted.
    pub fn insert(&mut self, at: usize, words: &str) -> usize {
        let at = at.min(self.length);
        let room = TEXT_AREA_CAPACITY - self.length;
        let mut taken = words.len().min(room);
        while !words.is_char_boundary(taken) {
            taken -= 1;
        }
        self.bytes.copy_within(at..self.length, at + taken);
        self.bytes[at..at + taken].copy_from_slice(&words.as_bytes()[..taken]);
        self.length += taken;
        taken
    }

    /// Removes bytes `start .. end`.
    pub fn remove(&mut self, start: usize, end: usize) {
        let (start, end) = (start.min(self.length), end.min(self.length));
        if start >= end {
            return;
        }
        self.bytes.copy_within(end..self.length, start);
        self.length -= end - start;
    }

    /// The character boundary before byte `at`.
    pub fn previous_boundary(&self, at: usize) -> usize {
        let text = self.text();
        text[..at.min(text.len())]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    /// The character boundary after byte `at`.
    pub fn next_boundary(&self, at: usize) -> usize {
        let text = self.text();
        match text.get(at..).and_then(|rest| rest.chars().next()) {
            Some(character) => at + character.len_utf8(),
            None => text.len(),
        }
    }

    /// Visual line `line`'s first byte and the byte after it (the next line's first, or the
    /// text's end).
    pub fn line_range(&self, line: usize) -> (usize, usize) {
        let line = line.min(self.line_count - 1);
        let start = self.lines[line] as usize;
        let end = match line + 1 < self.line_count {
            true => self.lines[line + 1] as usize,
            false => self.length,
        };
        (start, end)
    }

    /// The visual line byte `at` is on: a wrapped line's first byte is on that line.
    pub fn line_of(&self, at: usize) -> usize {
        let at = at as u32;
        self.lines[..self.line_count]
            .partition_point(|&start| start <= at)
            .saturating_sub(1)
    }

    /// The x of byte `at` on visual line `line`, from the line's start.
    pub fn x_of(&self, line: usize, at: usize) -> i32 {
        let (start, end) = self.line_range(line);
        let text = self.text();
        text.get(start..at.clamp(start, end))
            .map_or(0, |part| part.chars().map(advance).sum())
    }

    /// The last byte the caret may stand on in visual line `line`: before the next line's
    /// first byte, or the text's end on the last line.
    pub fn line_end(&self, line: usize) -> usize {
        let (start, end) = self.line_range(line);
        match line + 1 < self.line_count {
            true => self.previous_boundary(end).max(start),
            false => end,
        }
    }

    /// Where End puts the caret on visual line `line`: before its trailing spaces when it
    /// wrapped, before its line break when it has one.
    pub fn end_of_line(&self, line: usize) -> usize {
        let (start, end) = self.line_range(line);
        let text = self.text();
        if line + 1 >= self.line_count {
            return end;
        }
        let part = &text[start..end];
        match part.strip_suffix('\n') {
            Some(before) => start + before.len(),
            None => start + part.trim_end_matches([' ', '\t']).len(),
        }
    }

    /// The boundary on visual line `line` nearest to x `x` from the line's start.
    pub fn byte_at_x(&self, line: usize, x: i32) -> usize {
        let (start, _) = self.line_range(line);
        let last = self.line_end(line);
        let text = self.text();
        let mut best = start;
        let mut pen = 0;
        for (offset, character) in text[start..last].char_indices() {
            let next = pen + advance(character);
            if (next - x).abs() < (pen - x).abs() {
                best = start + offset + character.len_utf8();
            } else if x <= pen {
                break;
            }
            pen = next;
        }
        best.min(last)
    }

    /// Rebuilds the visual line index for lines `width` wide from the line before the one
    /// holding byte `from` to the end, in one pass and without allocating: lines break at
    /// `\n`, after the last space or tab that keeps a line within `width` (spaces stay on the
    /// upper line), or, for a word wider than a line, before the glyph that does not fit.
    pub fn wrap(&mut self, from: usize, width: i32) {
        let first = self.line_of(from.min(self.length)).saturating_sub(1);
        let width = width.max(1);
        let mut count = first + 1;
        let mut position = self.lines[first] as usize;
        let mut pen = 0;
        let mut opportunity: Option<(usize, i32)> = None;
        let length = self.length;
        while position < length {
            let character = match core::str::from_utf8(
                &self.bytes[position..(position + 4).min(length)],
            ) {
                Ok(rest) => rest.chars().next(),
                Err(error) if error.valid_up_to() > 0 => {
                    core::str::from_utf8(&self.bytes[position..position + error.valid_up_to()])
                        .ok()
                        .and_then(|rest| rest.chars().next())
                }
                Err(_) => None,
            };
            let Some(character) = character else {
                position += 1;
                continue;
            };
            let next = position + character.len_utf8();
            if character == '\n' {
                if count >= TEXT_AREA_LINES {
                    break;
                }
                self.lines[count] = next as u32;
                count += 1;
                pen = 0;
                opportunity = None;
                position = next;
                continue;
            }
            let step = advance(character);
            if character == ' ' || character == '\t' {
                pen += step;
                opportunity = Some((next, pen));
                position = next;
                continue;
            }
            if pen > 0 && pen + step > width {
                if count >= TEXT_AREA_LINES {
                    break;
                }
                let (start, used) = match opportunity.take() {
                    Some((start, at)) if start <= position => (start, pen - at),
                    _ => (position, 0),
                };
                self.lines[count] = start as u32;
                count += 1;
                pen = used;
                if pen > 0 && pen + step > width {
                    if count >= TEXT_AREA_LINES {
                        break;
                    }
                    self.lines[count] = position as u32;
                    count += 1;
                    pen = 0;
                }
            }
            pen += step;
            position = next;
        }
        self.line_count = count;
    }

    /// Rebuilds the whole index for lines `width` wide.
    pub fn wrap_all(&mut self, width: i32) {
        self.lines[0] = 0;
        self.line_count = 1;
        self.wrap(0, width);
    }
}

/// The rectangle of the caret of `area` over `store` in a text area at `rect`.
pub fn caret_rect(area: &TextArea, store: &Store, rect: Rect) -> Rect {
    let inner = inner(rect);
    let line = store.line_of(area.caret);
    Rect::new(
        inner.x + store.x_of(line, area.caret),
        inner.y + line as i32 * TEXT_AREA_LINE_PITCH - area.scroll,
        1,
        LINE_HEIGHT,
    )
}

/// Draws a waiting dead key's `accent` at the caret of `area` over `store` at `rect` in place
/// of the caret (D221), the rest of the caret's visual line moved right by the cell's width.
pub fn paint_dead_key(
    canvas: &mut Canvas<'_>,
    area: &TextArea,
    store: &Store,
    rect: Rect,
    accent: char,
) {
    let inner = inner(rect);
    let clip = Rect::new(inner.x, rect.y + 1, inner.width, rect.height - 2);
    let previous = canvas.narrow(clip);
    let line = store.line_of(area.caret);
    let (_, end) = store.line_range(line);
    let x = inner.x + store.x_of(line, area.caret);
    let y = inner.y + line as i32 * TEXT_AREA_LINE_PITCH - area.scroll;
    crate::widget::paint_dead_key_cell(canvas, (x, y), clip.right(), accent, "");
    let ink = canvas.palette().text;
    let mut pen = crate::widget::dead_key_cell(x, y, accent).right();
    for character in store.text().get(area.caret..end).unwrap_or("").chars() {
        match character {
            '\n' => {}
            '\t' => pen += TEXT_AREA_TAB,
            character => {
                let mut bytes = [0; 4];
                pen = canvas.text(pen, y, character.encode_utf8(&mut bytes), ink, 1);
            }
        }
    }
    canvas.restore(previous);
}

/// Draws `area` over `store` at `rect` as `look` says: the field's chrome made tall, the
/// visible lines from the first one shown, the selection, the caret and the bar.
pub fn paint(canvas: &mut Canvas<'_>, area: &TextArea, store: &Store, rect: Rect, look: Look) {
    let palette = canvas.palette();
    canvas.rounded(rect, &INSET_R4, palette.field);
    let border = match (look.enabled, look.hovered.is_some() && !look.focused) {
        (false, _) | (true, true) => palette.text_dim,
        (true, false) => palette.button_border,
    };
    canvas.outline(rect, &INSET_R4, 1, border);
    let inner = inner(rect);
    let clip = Rect::new(inner.x, rect.y + 1, inner.width, rect.height - 2);
    let previous = canvas.narrow(clip);
    let ink = match look.enabled {
        true => palette.text,
        false => palette.text_dim,
    };
    if store.is_empty() {
        canvas.text(inner.x, inner.y, area.placeholder, palette.text_dim, 1);
    }
    let first = (area.scroll / TEXT_AREA_LINE_PITCH).max(0) as usize;
    let shown = visible_lines(rect) + 1;
    let (selection_start, selection_end) = area.selection();
    let text = store.text();
    for line in first..(first + shown).min(store.line_count()) {
        let (start, end) = store.line_range(line);
        let y = inner.y + line as i32 * TEXT_AREA_LINE_PITCH - area.scroll;
        if look.focused && selection_start < selection_end {
            let from = selection_start.clamp(start, end);
            let to = selection_end.clamp(start, end);
            if from < to {
                let left = inner.x + store.x_of(line, from);
                let right = inner.x + store.x_of(line, to);
                canvas.fill(
                    Rect::new(left, y, right - left, LINE_HEIGHT),
                    palette.selection,
                );
            }
        }
        let mut pen = inner.x;
        for character in text[start..end].chars() {
            match character {
                '\n' => {}
                '\t' => pen += TEXT_AREA_TAB,
                character => {
                    let mut bytes = [0; 4];
                    pen = canvas.text(pen, y, character.encode_utf8(&mut bytes), ink, 1);
                }
            }
        }
    }
    canvas.restore(previous);
    if look.focused && look.caret_shown && look.enabled {
        let caret = caret_rect(area, store, rect).intersection(&clip);
        canvas.fill(caret, palette.text);
    }
    let content = store.line_count() as i32 * TEXT_AREA_LINE_PITCH + 2 * TEXT_AREA_TOP;
    let track = Rect::new(
        rect.right() - 1 - SCROLL_BAR_WIDTH,
        rect.y + 1,
        SCROLL_BAR_WIDTH,
        rect.height - 2,
    );
    if let Some((track, thumb)) = scroll_bar(track, content, area.scroll) {
        canvas.fill(track, palette.progress_track);
        canvas.rounded(thumb, &INSET_R4, palette.bar_pressed);
    }
    if look.focused {
        focus_ring(canvas, rect, 2);
    }
}

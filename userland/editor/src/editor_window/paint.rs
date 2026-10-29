use super::{
    Bar, CELL_WIDTH, Drag, EditorWindow, FIND_FIELD, Focus, MATCH_CASE, MATCH_CASE_X,
    NEXT_BUTTON, PREVIOUS_BUTTON, Part, RESULT_FROM_RIGHT, RESULT_LEFT, ROW_HEIGHT, TOOLBAR_HEIGHT,
    TOOLS,
};
use crate::columns::TAB_COLUMNS;
use core::fmt::Write;
use eggos_app::messages::Line;
use eggos_toolkit::canvas::{Canvas, INSET_R4, INSET_R6};
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text::{self, Text};
use eggos_toolkit::widget::{self, Kind as Widget, LINE_HEIGHT, Look};

const BAR_TEXT_Y: i32 = 40;
const STATUS_TEXT_X: i32 = 12;
const STATUS_TEXT_TOP: i32 = 4;
const STATE_GAP: i32 = 16;
const NOTICE_HEIGHT: i32 = 40;
const NOTICE_PADDING: i32 = 16;
const NOTICE_LEAST_WIDTH: i32 = 160;
const NOTICE_ABOVE_STATUS: i32 = 16;
const NOTICE_TEXT_TOP: i32 = 12;
const EDGE: i32 = 2;
const NUMBER_END_FROM_GUTTER: i32 = 8;
const CARET_WIDTH: i32 = 1;

fn edged(canvas: &mut Canvas<'_>, rect: Rect) {
    canvas.rounded(
        Rect::new(
            rect.x - EDGE,
            rect.y - EDGE,
            rect.width + 2 * EDGE,
            rect.height + 2 * EDGE,
        ),
        &INSET_R6,
        colour::FIELD,
    );
    canvas.rounded(rect, &INSET_R6, colour::MENU);
    canvas.outline(rect, &INSET_R6, 1, colour::MENU_BORDER);
}

fn mono(canvas: &mut Canvas<'_>, x: i32, y: i32, character: char, ink: u32) {
    if character != ' ' {
        canvas.mono_glyph(x, y, eggos_font::glyph_id(character as u32), ink);
    }
}

impl EditorWindow<'_> {
    /// Draws the whole content; the canvas's clip limits what is drawn.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        self.paint_with_caret(canvas, self.caret_shown);
    }

    /// Draws the whole content with the caret shown or not, whatever the blink says: the two
    /// frames a test may see.
    pub fn paint_with_caret(&self, canvas: &mut Canvas<'_>, caret_shown: bool) {
        self.paint_toolbar(canvas);
        self.paint_bar(canvas, caret_shown);
        self.paint_text(canvas, caret_shown);
        self.paint_status(canvas);
        self.paint_notice(canvas);
        if let Some(menu) = &self.menu {
            menu.paint(canvas);
        }
        if let Some((panel, _)) = &self.panel {
            panel.paint(canvas, self.area(), self.window_focused);
        }
    }

    /// Draws the whole content into `pixels`, `width` by `height` at `scale`, with the caret
    /// shown or not.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32, scale: u32, caret: bool) {
        if let Some(mut canvas) = Canvas::new(pixels, width, height, scale) {
            self.paint_with_caret(&mut canvas, caret);
        }
    }

    fn flat_button(
        &self,
        canvas: &mut Canvas<'_>,
        rect: Rect,
        words: &str,
        part: Part,
        on: bool,
    ) {
        let fill = match () {
            () if !on => None,
            () if self.pressed == Some(part) && self.hover == Some(part) => {
                Some(colour::BAR_PRESSED)
            }
            () if self.hover == Some(part) => Some(colour::BAR_HOVER),
            () => None,
        };
        if let Some(fill) = fill {
            canvas.rounded(rect, &INSET_R4, fill);
        }
        let ink = match on {
            true => colour::TEXT,
            false => colour::TEXT_DIM,
        };
        let width = text::width(words);
        canvas.text(
            rect.x + (rect.width - width) / 2,
            rect.y + (rect.height - LINE_HEIGHT) / 2,
            words,
            ink,
            1,
        );
    }

    fn paint_toolbar(&self, canvas: &mut Canvas<'_>) {
        let toolbar = Rect::new(0, 0, self.width, TOOLBAR_HEIGHT);
        canvas.fill(toolbar, colour::BAR);
        canvas.fill(
            Rect::new(0, toolbar.bottom() - 1, toolbar.width, 1),
            colour::BAR_EDGE,
        );
        for (tool, label) in TOOLS {
            self.flat_button(canvas, self.tool_rect(tool), label, Part::Tool(tool), true);
        }
    }

    fn look(&self, focused: bool, caret_shown: bool) -> Look {
        Look {
            hovered: None,
            pressed: None,
            focused: focused && self.window_focused && self.panel.is_none(),
            enabled: true,
            caret_shown,
        }
    }

    fn paint_bar(&self, canvas: &mut Canvas<'_>, caret_shown: bool) {
        if self.bar == Bar::Closed {
            return;
        }
        let bar = self.bar_rect();
        canvas.fill(bar, colour::BAR);
        canvas.fill(
            Rect::new(0, bar.bottom() - 1, bar.width, 1),
            colour::BAR_EDGE,
        );
        match self.bar {
            Bar::Find => self.paint_find_bar(canvas, caret_shown),
            Bar::Open | Bar::SaveAs => self.paint_path_bar(canvas, caret_shown),
            Bar::Closed => {}
        }
    }

    fn paint_find_bar(&self, canvas: &mut Canvas<'_>, caret_shown: bool) {
        widget::paint(
            canvas,
            &Widget::TextField(self.find_field),
            FIND_FIELD,
            self.look(self.focus == Focus::FindField, caret_shown),
        );
        let searchable = !self.find_field.text.is_empty();
        self.flat_button(canvas, PREVIOUS_BUTTON, "<", Part::Previous, searchable);
        self.flat_button(canvas, NEXT_BUTTON, ">", Part::Next, searchable);
        let mut look = self.look(self.focus == Focus::MatchCase, caret_shown);
        if self.hover == Some(Part::MatchCase) {
            look.hovered = Some(0);
        }
        widget::paint(
            canvas,
            &Widget::Checkbox {
                text: Text::from(MATCH_CASE),
                checked: self.match_case,
            },
            self.match_case_rect(),
            look,
        );
        let found = self.found_text();
        let words = found.as_str();
        let right = self.width - RESULT_FROM_RIGHT;
        let room = right - RESULT_LEFT.max(MATCH_CASE_X + self.match_case_rect().width + 8);
        if !words.is_empty() && room > 0 {
            let (end, ellipsis) = text::fit(words, room);
            let width = text::width(&words[..end])
                + if ellipsis {
                    text::width(text::ELLIPSIS)
                } else {
                    0
                };
            widget::clipped_text(
                canvas,
                right - width,
                BAR_TEXT_Y,
                room,
                words,
                colour::TEXT_DIM,
            );
        }
        self.flat_button(canvas, self.close_find_rect(), "\u{d7}", Part::CloseFind, true);
    }

    fn paint_path_bar(&self, canvas: &mut Canvas<'_>, caret_shown: bool) {
        let (label, action) = match self.bar {
            Bar::Open => ("Open:", "Open"),
            _ => ("Save as:", "Save"),
        };
        canvas.text(12, BAR_TEXT_Y, label, colour::TEXT, 1);
        widget::paint(
            canvas,
            &Widget::TextField(self.path_field),
            self.path_field_rect(),
            self.look(self.focus == Focus::PathField, caret_shown),
        );
        self.flat_button(canvas, self.action_rect(), action, Part::Action, true);
        self.flat_button(canvas, self.cancel_rect(), "Cancel", Part::Cancel, true);
    }

    fn paint_text(&self, canvas: &mut Canvas<'_>, caret_shown: bool) {
        let area = self.area();
        let gutter = self.gutter_width();
        let view = self.text_view();
        canvas.fill(Rect::new(0, area.y, gutter, area.height), colour::MENU);
        canvas.fill(view, colour::FIELD);
        let document = self.editor.document();
        let first = (self.scroll_y / ROW_HEIGHT).max(0) as usize;
        let shown = (area.height / ROW_HEIGHT + 2) as usize;
        let last = (first + shown).min(document.line_count());
        let caret_line = self.editor.caret_line();
        let previous = canvas.narrow(Rect::new(0, area.y, gutter, area.height));
        for line in first..last {
            let mut number = Line::new();
            let _ = write!(number, "{}", line + 1);
            let digits = number.as_str().len() as i32;
            let mut x = gutter - NUMBER_END_FROM_GUTTER - digits * CELL_WIDTH;
            let ink = match line == caret_line {
                true => colour::TEXT,
                false => colour::TEXT_DIM,
            };
            for character in number.as_str().chars() {
                mono(canvas, x, self.row_top(line), character, ink);
                x += CELL_WIDTH;
            }
        }
        canvas.restore(previous);
        let previous = canvas.narrow(view);
        let (start, end) = self.editor.selection();
        for line in first..last {
            self.paint_line(canvas, line, (start, end));
        }
        let caret_drawn = caret_shown
            && self.focus == Focus::Text
            && self.window_focused
            && self.panel.is_none();
        if caret_drawn {
            let x = self.column_left(self.editor.view_column()) - CARET_WIDTH;
            canvas.fill(
                Rect::new(x, self.row_top(caret_line), CARET_WIDTH, ROW_HEIGHT),
                colour::TEXT,
            );
        }
        if let Some((track, thumb)) = self.vertical_bar() {
            self.paint_scroll_bar(canvas, track, thumb, Part::VerticalThumb);
        }
        if let Some((track, thumb)) = self.horizontal_bar() {
            self.paint_scroll_bar(canvas, track, thumb, Part::HorizontalThumb);
        }
        if let (Some((vertical, _)), Some((horizontal, _))) =
            (self.vertical_bar(), self.horizontal_bar())
        {
            canvas.fill(
                Rect::new(vertical.x, horizontal.y, vertical.width, horizontal.height),
                colour::PROGRESS_TRACK,
            );
        }
        canvas.restore(previous);
    }

    fn paint_line(&self, canvas: &mut Canvas<'_>, line: usize, (start, end): (usize, usize)) {
        let document = self.editor.document();
        let top = self.row_top(line);
        let line_start = document.line_start(line);
        let bytes = document.line(line);
        let right = canvas.clip().right();
        let mut column = 0;
        for (at, character) in core::str::from_utf8(bytes).unwrap_or("").char_indices() {
            let next = match character {
                '\t' => (column / TAB_COLUMNS + 1) * TAB_COLUMNS,
                _ => column + 1,
            };
            let left = self.column_left(column);
            if left >= right {
                return;
            }
            let offset = line_start + at;
            let selected = offset >= start && offset < end;
            let cells = (next - column) as i32;
            if left + cells * CELL_WIDTH > canvas.clip().x {
                if selected {
                    canvas.fill(
                        Rect::new(left, top, cells * CELL_WIDTH, ROW_HEIGHT),
                        colour::SELECTION,
                    );
                }
                if character != '\t' {
                    mono(canvas, left, top, character, colour::TEXT);
                }
            }
            column = next;
        }
        let line_end = line_start + bytes.len();
        let has_break = line + 1 < document.line_count();
        if has_break && line_end >= start && line_end < end {
            canvas.fill(
                Rect::new(self.column_left(column), top, CELL_WIDTH, ROW_HEIGHT),
                colour::SELECTION,
            );
        }
    }

    fn paint_scroll_bar(&self, canvas: &mut Canvas<'_>, track: Rect, thumb: Rect, part: Part) {
        canvas.fill(track, colour::PROGRESS_TRACK);
        let dragged = matches!(
            (self.drag, part),
            (Some(Drag::Vertical { .. }), Part::VerticalThumb)
                | (Some(Drag::Horizontal { .. }), Part::HorizontalThumb)
        );
        let ink = match self.hover == Some(part) || dragged {
            true => colour::BUTTON_INACTIVE,
            false => colour::BAR_PRESSED,
        };
        canvas.rounded(thumb, &INSET_R4, ink);
    }

    /// What the status bar's left side says: `Ln <line>, Col <column>`, and how many
    /// characters are selected.
    pub fn status_text(&self) -> Line {
        let (line, column) = self.editor.line_and_column();
        let mut text = Line::new();
        let _ = write!(text, "Ln {line}, Col {column}");
        if self.editor.has_selection() {
            let _ = write!(
                text,
                " \u{b7} {} selected",
                self.editor.selected_characters()
            );
        }
        text
    }

    /// What the status bar says before the line ending: `Modified`, `New file` or nothing.
    pub fn state_text(&self) -> &'static str {
        match (self.editor.is_modified(), self.is_new_file()) {
            (true, _) => "Modified",
            (false, true) => "New file",
            (false, false) => "",
        }
    }

    fn paint_status(&self, canvas: &mut Canvas<'_>) {
        let status = self.status_bar();
        canvas.fill(status, colour::BAR);
        canvas.fill(Rect::new(0, status.y, status.width, 1), colour::BAR_EDGE);
        let y = status.y + STATUS_TEXT_TOP;
        canvas.text(
            STATUS_TEXT_X,
            y,
            self.status_text().as_str(),
            colour::TEXT_DIM,
            1,
        );
        let ending = self.editor.document().ending().label();
        let ending_x = self.width - STATUS_TEXT_X - text::width(ending);
        canvas.text(ending_x, y, ending, colour::TEXT_DIM, 1);
        let state = self.state_text();
        if !state.is_empty() {
            canvas.text(
                ending_x - STATE_GAP - text::width(state),
                y,
                state,
                colour::TEXT_DIM,
                1,
            );
        }
    }

    /// Where the notice shown is, if one is.
    pub fn notice_rect(&self) -> Option<Rect> {
        let notice = self.notice.as_ref()?;
        let view = self.text_view();
        let most = (view.width - 2 * NOTICE_PADDING).max(NOTICE_LEAST_WIDTH);
        let width = (text::width(notice.text.as_str()) + 2 * NOTICE_PADDING)
            .clamp(NOTICE_LEAST_WIDTH, most);
        Some(Rect::new(
            view.x + (view.width - width) / 2,
            self.status_bar().y - NOTICE_ABOVE_STATUS - NOTICE_HEIGHT,
            width,
            NOTICE_HEIGHT,
        ))
    }

    fn paint_notice(&self, canvas: &mut Canvas<'_>) {
        let (Some(notice), Some(rect)) = (self.notice.as_ref(), self.notice_rect()) else {
            return;
        };
        edged(canvas, rect);
        let words = notice.text.as_str();
        let space = rect.width - 2 * NOTICE_PADDING;
        let (end, ellipsis) = text::fit(words, space);
        let width = text::width(&words[..end])
            + if ellipsis {
                text::width(text::ELLIPSIS)
            } else {
                0
            };
        widget::clipped_text(
            canvas,
            rect.x + (rect.width - width) / 2,
            rect.y + NOTICE_TEXT_TOP,
            space,
            words,
            colour::TEXT,
        );
    }
}

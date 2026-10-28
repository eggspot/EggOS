use super::{
    BACK_BUTTON, COLUMN_GAP, FilesWindow, Focus, HEADER_HEIGHT, Hover, HOST_SHARE, Location,
    NAME_X, Place, Pressed, ROW_HEIGHT, SIZE_END_FROM_RIGHT, SIZE_WIDTH, SIDEBAR_WIDTH,
    TIME_WIDTH, UP_BUTTON, text_of,
};
use crate::clock::list_time;
use crate::kind::Kind;
use crate::listing::{items_text, size_text};
use crate::names;
use eggos_app_manifest::Source;
use eggos_toolkit::canvas::{Canvas, INSET_R4, INSET_R6};
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text;
use eggos_toolkit::widget::{self, Kind as Widget, LINE_HEIGHT, Look};

const TEXT_TOP: i32 = 4;
const PLACES_HEADING: (i32, i32) = (12, 40);
const MARK_X: i32 = 8;
const PLACE_TEXT_X: i32 = 24;
const STATUS_TEXT_X: i32 = 12;
const BADGE: i32 = 6;
const BADGE_GAP: i32 = 8;
const BADGE_TOP: i32 = 9;
const NOTICE_HEIGHT: i32 = 40;
const NOTICE_PADDING: i32 = 16;
const NOTICE_LEAST_WIDTH: i32 = 160;
const NOTICE_ABOVE_STATUS: i32 = 16;
const NOTICE_TEXT_TOP: i32 = 12;
const EDGE: i32 = 2;

fn badge_colour(source: Source) -> u32 {
    match source {
        Source::Windows => colour::ICON_WINDOWS,
        Source::Macos => colour::ICON_MAC,
        Source::Linux => colour::ICON_LINUX,
    }
}

fn folder_mark(canvas: &mut Canvas<'_>, x: i32, y: i32) {
    canvas.fill(Rect::new(x, y + 2, 12, 8), colour::ICON_FOLDER_BACK);
    canvas.fill(Rect::new(x, y + 4, 12, 6), colour::ICON_FOLDER);
}

fn page_mark(canvas: &mut Canvas<'_>, x: i32, y: i32) {
    let ink = colour::TEXT_DIM;
    canvas.fill(Rect::new(x, y, 1, 12), ink);
    canvas.fill(Rect::new(x, y + 11, 10, 1), ink);
    canvas.fill(Rect::new(x, y, 8, 1), ink);
    canvas.pixel(x + 8, y + 1, ink);
    canvas.fill(Rect::new(x + 9, y + 2, 1, 10), ink);
}

fn right_aligned(canvas: &mut Canvas<'_>, right: i32, y: i32, words: &str, ink: u32) {
    canvas.text(right - text::width(words), y, words, ink, 1);
}

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

impl FilesWindow {
    /// Draws the whole content; the canvas's clip limits what is drawn.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        self.paint_toolbar(canvas);
        self.paint_sidebar(canvas);
        self.paint_list(canvas);
        self.paint_status(canvas);
        self.paint_notice(canvas);
        if let Some((menu, _)) = &self.menu {
            menu.paint(canvas);
        }
        if let Some((panel, _)) = &self.panel {
            panel.paint(canvas, self.middle(), self.window_focused);
        }
    }

    /// Draws the whole content into `pixels`, `width` by `height` at `scale`.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32, scale: u32) {
        if let Some(mut canvas) = Canvas::new(pixels, width, height, scale) {
            self.paint(&mut canvas);
        }
    }

    fn paint_button(&self, canvas: &mut Canvas<'_>, rect: Rect, glyph: &str, enabled: bool, hover: Hover) {
        let pressed = matches!(
            (self.pressed, hover),
            (Some(Pressed::Back), Hover::Back) | (Some(Pressed::Up), Hover::Up)
        );
        let fill = match () {
            () if !enabled => None,
            () if pressed => Some(colour::BAR_PRESSED),
            () if self.hover == Some(hover) => Some(colour::BAR_HOVER),
            () => None,
        };
        if let Some(fill) = fill {
            canvas.rounded(rect, &INSET_R4, fill);
        }
        let ink = if enabled { colour::TEXT } else { colour::TEXT_DIM };
        let width = text::width(glyph);
        canvas.text(
            rect.x + (rect.width - width) / 2,
            rect.y + (rect.height - LINE_HEIGHT) / 2,
            glyph,
            ink,
            1,
        );
    }

    fn paint_toolbar(&self, canvas: &mut Canvas<'_>) {
        let toolbar = self.toolbar();
        canvas.fill(toolbar, colour::BAR);
        canvas.fill(
            Rect::new(0, toolbar.bottom() - 1, toolbar.width, 1),
            colour::BAR_EDGE,
        );
        self.paint_button(canvas, BACK_BUTTON, "<", self.history_length > 0, Hover::Back);
        self.paint_button(canvas, UP_BUTTON, "^", self.can_go_up(), Hover::Up);
        let focused = self.window_focused && self.focus == Focus::Path && self.rename.is_none();
        widget::paint(
            canvas,
            &Widget::TextField(self.path_field),
            self.path_field_rect(),
            Look {
                hovered: None,
                pressed: None,
                focused,
                enabled: true,
                caret_shown: self.caret_shown,
            },
        );
    }

    fn paint_sidebar(&self, canvas: &mut Canvas<'_>) {
        let sidebar = self.sidebar();
        canvas.fill(sidebar, colour::DOCK);
        canvas.fill(
            Rect::new(SIDEBAR_WIDTH - 1, sidebar.y, 1, sidebar.height),
            colour::MENU_BORDER,
        );
        let previous = canvas.narrow(sidebar);
        canvas.text(
            PLACES_HEADING.0,
            PLACES_HEADING.1,
            "Places",
            colour::TEXT_DIM,
            1,
        );
        let current = self.current_place();
        for (index, place) in self.places().iter().enumerate() {
            let rect = self.place_rect(index);
            let chosen = current == Some(index);
            if chosen {
                canvas.fill(rect, colour::SELECTION);
                canvas.text(rect.x + MARK_X, rect.y + TEXT_TOP, ">", colour::ACCENT, 1);
            } else if self.hover == Some(Hover::Place(index)) {
                canvas.fill(rect, colour::MENU_HOVER);
            }
            widget::clipped_text(
                canvas,
                rect.x + PLACE_TEXT_X,
                rect.y + TEXT_TOP,
                rect.width - PLACE_TEXT_X - 8,
                place.label(self.windows_words),
                colour::TEXT,
            );
            if self.window_focused && self.focus == Focus::Sidebar && self.place_ring == index {
                widget::focus_ring(canvas, rect, 0);
            }
        }
        canvas.restore(previous);
    }

    fn paint_list(&self, canvas: &mut Canvas<'_>) {
        let list = self.list();
        canvas.fill(list, colour::MENU);
        let right = self.columns_right();
        let header_y = list.y + TEXT_TOP;
        canvas.text(list.x + NAME_X, header_y, "Name", colour::TEXT_DIM, 1);
        right_aligned(
            canvas,
            right - SIZE_END_FROM_RIGHT,
            header_y,
            "Size",
            colour::TEXT_DIM,
        );
        let time_heading = match self.location {
            Location::Trash => "Deleted",
            Location::Folder(_) => "Modified",
        };
        canvas.text(
            right - COLUMN_GAP - TIME_WIDTH,
            header_y,
            time_heading,
            colour::TEXT_DIM,
            1,
        );
        canvas.fill(
            Rect::new(list.x, list.y + HEADER_HEIGHT - 1, list.width, 1),
            colour::BAR_EDGE,
        );
        let rows = self.rows();
        let previous = canvas.narrow(rows);
        if self.listing.is_empty() {
            self.paint_empty(canvas, rows);
        }
        let first = (self.offset / ROW_HEIGHT).max(0) as usize;
        let shown = (rows.height / ROW_HEIGHT + 2) as usize;
        for index in first..(first + shown).min(self.listing.len()) {
            self.paint_row(canvas, index);
        }
        if let Some((track, thumb)) = self.scroll_bar() {
            canvas.fill(track, colour::PROGRESS_TRACK);
            let active = self.hover == Some(Hover::Thumb)
                || matches!(self.pressed, Some(Pressed::Thumb { .. }));
            let ink = match active {
                true => colour::BUTTON_INACTIVE,
                false => colour::BAR_PRESSED,
            };
            canvas.rounded(thumb, &INSET_R4, ink);
        }
        canvas.restore(previous);
    }

    fn paint_empty(&self, canvas: &mut Canvas<'_>, rows: Rect) {
        let mut line = eggos_app::messages::Line::new();
        let words = match (self.listing.error(), self.location) {
            (Some(error), _) => {
                line.push_bytes(b"Cannot read this folder (");
                line.push_bytes(error.message());
                line.push_bytes(b")");
                line.as_str()
            }
            (None, Location::Trash) if self.windows_words => "Recycle Bin is empty",
            (None, Location::Trash) => "Trash is empty",
            (None, Location::Folder(folder))
                if names::is_within(folder.as_bytes(), HOST_SHARE)
                    && !self.places().contains(&Place::HostShare) =>
            {
                "The host share is not mounted"
            }
            (None, Location::Folder(_)) => "This folder is empty",
        };
        widget::clipped_text(
            canvas,
            rows.x + NAME_X,
            rows.y + TEXT_TOP,
            rows.width - NAME_X - 8,
            words,
            colour::TEXT_DIM,
        );
    }

    fn paint_row(&self, canvas: &mut Canvas<'_>, index: usize) {
        let Some(entry) = self.listing.get(index).copied() else {
            return;
        };
        let row = self.row_rect(index);
        if row.intersection(&canvas.clip()).is_empty() {
            return;
        }
        let chosen = self.selected == Some(index);
        if chosen {
            canvas.fill(row, colour::SELECTION);
            canvas.text(row.x + MARK_X, row.y + TEXT_TOP, ">", colour::ACCENT, 1);
        } else if entry.kind.is_folder() {
            folder_mark(canvas, row.x + MARK_X, row.y + 7);
        } else {
            page_mark(canvas, row.x + MARK_X + 1, row.y + 6);
        }
        let ink = match self.is_cut(index) && !chosen {
            true => colour::TEXT_DIM,
            false => colour::TEXT,
        };
        let renaming = self.rename.filter(|rename| rename.index == index);
        if let Some(rename) = renaming {
            widget::paint(
                canvas,
                &Widget::TextField(rename.field),
                self.rename_rect(index),
                Look {
                    hovered: None,
                    pressed: None,
                    focused: self.window_focused,
                    enabled: true,
                    caret_shown: self.caret_shown,
                },
            );
        } else {
            let right = self.columns_right();
            let size_start = right - SIZE_END_FROM_RIGHT - SIZE_WIDTH;
            let name_x = row.x + NAME_X;
            let space = size_start - COLUMN_GAP - name_x;
            let name = text_of(self.listing.name(index));
            let (end, ellipsis) = text::fit(name.as_str(), space);
            let mut after = canvas.text(name_x, row.y + TEXT_TOP, &name.as_str()[..end], ink, 1);
            if ellipsis {
                after = canvas.text(after, row.y + TEXT_TOP, text::ELLIPSIS, ink, 1);
            }
            if let Some(source) = entry.kind.source()
                && after + BADGE_GAP + BADGE <= size_start
            {
                canvas.fill(
                    Rect::new(after + BADGE_GAP, row.y + BADGE_TOP, BADGE, BADGE),
                    badge_colour(source),
                );
            }
            let size = match entry.kind {
                Kind::Folder | Kind::App(_) | Kind::Bundle => {
                    let mut dash = crate::clock::Stamp::new();
                    let _ = core::fmt::Write::write_str(&mut dash, "-");
                    dash
                }
                _ => size_text(entry.size),
            };
            right_aligned(
                canvas,
                right - SIZE_END_FROM_RIGHT,
                row.y + TEXT_TOP,
                size.as_str(),
                ink,
            );
            canvas.text(
                right - COLUMN_GAP - TIME_WIDTH,
                row.y + TEXT_TOP,
                list_time(entry.time).as_str(),
                ink,
                1,
            );
        }
        if chosen && self.window_focused && self.focus == Focus::List && renaming.is_none() {
            widget::focus_ring(canvas, row, 0);
        }
    }

    fn paint_status(&self, canvas: &mut Canvas<'_>) {
        let status = self.status_bar();
        canvas.fill(status, colour::BAR);
        canvas.fill(Rect::new(0, status.y, status.width, 1), colour::BAR_EDGE);
        canvas.text(
            STATUS_TEXT_X,
            status.y + TEXT_TOP,
            items_text(self.listing.len()).as_str(),
            colour::TEXT_DIM,
            1,
        );
    }

    /// Where the notice shown is, if one is.
    pub fn notice_rect(&self) -> Option<Rect> {
        let notice = self.notice.as_ref()?;
        let list = self.list();
        let most = (list.width - 2 * NOTICE_PADDING).max(NOTICE_LEAST_WIDTH);
        let width = (text::width(notice.text.as_str()) + 2 * NOTICE_PADDING)
            .clamp(NOTICE_LEAST_WIDTH, most);
        Some(Rect::new(
            list.x + (list.width - width) / 2,
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

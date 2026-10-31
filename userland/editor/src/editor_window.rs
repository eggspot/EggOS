//! The Text Editor's window (docs/ux/text-editor-v0.3.md): the toolbar, the bar slot with the
//! find bar or the path bar for Open and Save As, the gutter and the text view, the status bar,
//! and the context menu, panels and notices drawn inside it. It is pure state over an
//! [`Editor`] and [`Files`], so host tests and the QEMU test's expected frames run the same
//! code the program shows.

mod elements;
mod paint;
#[cfg(test)]
mod tests;

pub use elements::TEXT_ELEMENT;

use crate::columns::{byte_nearest, byte_under, view_width};
use crate::document::{Full, TEXT_CAPACITY};
use crate::editor::{Editor, Motion};
use crate::file::{self, OpenProblem, SaveProblem, folder_of};
use crate::find::{self, Wrap};
use core::fmt::Write;
use eggos_app::files::{self, Files, Path};
use eggos_app::messages::Line;
use eggos_runtime::errno::Errno;
use eggos_toolkit::damage::Damage;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Input, Modifiers, MouseButton, key};
use eggos_toolkit::menu::{Entry, Menu, MenuKey};
use eggos_toolkit::panel::{Panel, PanelResponse};
use eggos_toolkit::text::Text;
use eggos_toolkit::tree::PointerShape;
use eggos_toolkit::widget::{
    self, Escape, Field, FieldKey, MIN_THUMB, SCROLL_BAR_WIDTH, scroll_bar,
};

/// The app's name, as the launcher lists it and `app open` takes it.
pub const APP_NAME: &str = "Text Editor";
/// What every editor window's title ends with, which the shell matches to the app.
pub const TITLE_SUFFIX: &str = " - Text Editor";
/// A document's name before it is saved.
pub const UNTITLED: &str = "Untitled";
/// The content's size when it opens.
pub const DEFAULT_SIZE: (u32, u32) = (720, 480);
/// The smallest content.
pub const MINIMUM_SIZE: (u32, u32) = (480, 300);
/// How long a notice shows.
pub const NOTICE_MS: u32 = 3000;
/// The folder Open and Save As start in for an untitled document.
pub const HOME: &[u8] = b"/home/root";
/// The path Save As offers an untitled document.
pub const UNTITLED_PATH: &str = "/home/root/Untitled.txt";
/// A text row's height.
pub const ROW_HEIGHT: i32 = 16;
/// A text column's width.
pub const CELL_WIDTH: i32 = 8;

pub(crate) const TOOLBAR_HEIGHT: i32 = 32;
pub(crate) const BAR_HEIGHT: i32 = 32;
pub(crate) const STATUS_HEIGHT: i32 = 24;
/// The gap above the first row of text in the text view, in pixels.
pub const TEXT_TOP: i32 = 4;
/// The gap between the text view's left edge and the first column, in pixels.
pub const TEXT_LEFT: i32 = 8;
pub(crate) const TOOL_WIDTH: i32 = 64;
pub(crate) const TOOL_HEIGHT: i32 = 28;
const TOOL_PITCH: i32 = 68;
const TOOL_X: i32 = 4;
const TOOL_Y: i32 = 2;
const BAR_CONTROL_Y: i32 = 34;
pub(crate) const FIND_FIELD: Rect = Rect::new(12, 34, 180, 28);
pub(crate) const PREVIOUS_BUTTON: Rect = Rect::new(200, 34, 28, 28);
pub(crate) const NEXT_BUTTON: Rect = Rect::new(232, 34, 28, 28);
pub(crate) const MATCH_CASE_X: i32 = 272;
pub(crate) const MATCH_CASE: &str = "Match case";
pub(crate) const RESULT_FROM_RIGHT: i32 = 44;
pub(crate) const RESULT_LEFT: i32 = 360;
const CLOSE_FROM_RIGHT: i32 = 36;
pub(crate) const PATH_FIELD_X: i32 = 80;
const PATH_FIELD_FROM_RIGHT: i32 = 152;
const ACTION_FROM_RIGHT: i32 = 140;
const CANCEL_FROM_RIGHT: i32 = 72;
const CARET_BLINK_MS: u32 = 500;
const DOUBLE_CLICK_MS: u32 = 400;
const CLICK_SLOP: i32 = 4;
const WHEEL_ROWS: i32 = 3;
const WHEEL_COLUMNS: i32 = 3;
const DRAG_SCROLL_MS: u32 = 100;
const TRACK_KEEP: i32 = 24;
const MIN_GUTTER_DIGITS: usize = 2;
const MOST_COUNTED: usize = 999;

mod code {
    pub const A: u16 = 30;
    pub const C: u16 = 46;
    pub const F: u16 = 33;
    pub const G: u16 = 34;
    pub const N: u16 = 49;
    pub const O: u16 = 24;
    pub const S: u16 = 31;
    pub const V: u16 = 47;
    pub const X: u16 = 45;
    pub const Y: u16 = 21;
    pub const Z: u16 = 44;
    pub const F3: u16 = 61;
    pub const F10: u16 = 68;
    pub const MENU: u16 = 127;
    pub const LEFT_SHIFT: u16 = 42;
    pub const RIGHT_SHIFT: u16 = 54;
}

mod item {
    pub const UNDO: u8 = 1;
    pub const REDO: u8 = 2;
    pub const CUT: u8 = 3;
    pub const COPY: u8 = 4;
    pub const PASTE: u8 = 5;
    pub const SELECT_ALL: u8 = 6;
}

/// A toolbar button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    New,
    Open,
    Save,
    SaveAs,
    Find,
}

/// The toolbar's buttons, left to right, with their labels.
pub const TOOLS: [(Tool, &str); 5] = [
    (Tool::New, "New"),
    (Tool::Open, "Open"),
    (Tool::Save, "Save"),
    (Tool::SaveAs, "Save As"),
    (Tool::Find, "Find"),
];

/// What the bar slot under the toolbar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    Closed,
    Find,
    Open,
    SaveAs,
}

/// What the find bar's result says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Nothing,
    Match {
        index: usize,
        total: usize,
        wrap: Wrap,
    },
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Text,
    FindField,
    MatchCase,
    PathField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Close,
    New,
    Open,
    OpenFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    SaveChanges(Then),
    Replace(Option<Then>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Tool(Tool),
    Previous,
    Next,
    CloseFind,
    MatchCase,
    Action,
    Cancel,
    VerticalThumb,
    HorizontalThumb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Text,
    Lines { anchor_line: usize },
    Vertical { grab: i32, start: i32 },
    Horizontal { grab: i32, start: i32 },
}

#[derive(Debug, Clone, Copy)]
struct Notice {
    text: Line,
    shown_at: u32,
}

#[derive(Debug, Clone, Copy)]
struct Click {
    x: i32,
    y: i32,
    at: u32,
    count: u8,
}

/// The editor's window. See the module documentation.
#[derive(Debug)]
pub struct EditorWindow<'a> {
    editor: Editor<'a>,
    path: Option<Path>,
    exists: bool,
    width: i32,
    height: i32,
    scroll_x: i32,
    scroll_y: i32,
    widest: Option<usize>,
    bar: Bar,
    focus: Focus,
    find_field: Field,
    match_case: bool,
    found: Found,
    path_field: Field,
    pending: Option<Then>,
    file_to_open: Path,
    panel: Option<(Panel, Asked)>,
    menu: Option<Menu>,
    notice: Option<Notice>,
    hover: Option<Part>,
    pressed: Option<Part>,
    drag: Option<Drag>,
    pointer: Option<(i32, i32)>,
    shift_held: bool,
    last_click: Option<Click>,
    window_focused: bool,
    caret_epoch: u32,
    caret_shown: bool,
    drag_scrolled_at: u32,
    now_ms: u32,
    copied: bool,
    title: Text<64>,
    title_changed: bool,
    closing: bool,
    damage: Damage,
}

fn field(placeholder: &'static str) -> Field {
    Field {
        text: Text::new(),
        caret: 0,
        anchor: 0,
        scroll: 0,
        placeholder,
        escape: Escape::Clear,
    }
}

fn scaled(value: i32, times: i32, over: i32) -> i32 {
    if over == 0 {
        return 0;
    }
    let product = i64::from(value) * i64::from(times) / i64::from(over);
    product.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn shown_name(path: &[u8]) -> &str {
    core::str::from_utf8(files::file_name(path)).unwrap_or("?")
}

fn digits(mut number: usize) -> usize {
    let mut count = 1;
    while number >= 10 {
        number /= 10;
        count += 1;
    }
    count
}

/// The title of a window holding the document called `name`, `modified` or not:
/// `*<name> - Text Editor`, a name longer than 49 bytes cut to 46 and `...`.
pub fn title_of(name: &str, modified: bool) -> Text<64> {
    let mut title = Text::new();
    if modified {
        title.push_str("*");
    }
    match name.len() > 49 {
        true => {
            let mut end = 46;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            title.push_str(&name[..end]);
            title.push_str("...");
        }
        false => {
            title.push_str(name);
        }
    }
    title.push_str(TITLE_SUFFIX);
    title
}

impl<'a> EditorWindow<'a> {
    /// The window for `editor`, holding the file at `path` (absolute) if given: a file that
    /// exists is read; a path with nothing at it is a new file that the first save makes;
    /// one that cannot be read leaves an untitled document and a notice.
    pub fn new(
        editor: Editor<'a>,
        files: &mut impl Files,
        path: Option<&[u8]>,
        now_ms: u32,
    ) -> Self {
        let mut window = Self {
            editor,
            path: None,
            exists: false,
            width: DEFAULT_SIZE.0 as i32,
            height: DEFAULT_SIZE.1 as i32,
            scroll_x: 0,
            scroll_y: 0,
            widest: None,
            bar: Bar::Closed,
            focus: Focus::Text,
            find_field: field("Find"),
            match_case: false,
            found: Found::Nothing,
            path_field: field(""),
            pending: None,
            file_to_open: Path::new(),
            panel: None,
            menu: None,
            notice: None,
            hover: None,
            pressed: None,
            drag: None,
            pointer: None,
            shift_held: false,
            last_click: None,
            window_focused: true,
            caret_epoch: now_ms,
            caret_shown: true,
            drag_scrolled_at: now_ms,
            now_ms,
            copied: false,
            title: Text::new(),
            title_changed: true,
            closing: false,
            damage: Damage::new(),
        };
        if let Some(path) = path {
            window.open_path(files, path, true);
        }
        window.update_title();
        window.damage_all();
        window
    }

    /// The editing.
    pub fn editor(&self) -> &Editor<'a> {
        &self.editor
    }

    /// The text copied or cut since the last call, for the clipboard every app shares (D197).
    pub fn take_copied(&mut self) -> Option<&[u8]> {
        let copied = core::mem::take(&mut self.copied);
        (copied && self.editor.can_paste()).then(|| self.editor.copied())
    }

    /// Takes the clipboard every app shares as what Paste inserts
    /// ([`Editor::take_shared_clipboard`]).
    pub fn take_shared_clipboard(&mut self, fill: impl FnOnce(&mut [u8]) -> Option<usize>) {
        self.editor.take_shared_clipboard(fill);
    }

    /// The document's path, if it has one.
    pub fn path(&self) -> Option<&[u8]> {
        self.path.as_ref().map(Path::as_bytes)
    }

    /// The document's name: its file's name, or `Untitled`.
    pub fn name(&self) -> &str {
        self.path().map_or(UNTITLED, shown_name)
    }

    /// The window's title now.
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    /// The title once after it changed, for the window server.
    pub fn take_title(&mut self) -> Option<&str> {
        match core::mem::take(&mut self.title_changed) {
            true => Some(self.title.as_str()),
            false => None,
        }
    }

    /// What the bar slot shows.
    pub fn bar(&self) -> Bar {
        self.bar
    }

    /// What the find bar's result says.
    pub fn found(&self) -> Found {
        self.found
    }

    /// The text of the notice shown, if one is.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    /// Line `index` of the panel shown, if one is.
    pub fn dialog_line(&self, index: usize) -> Option<&str> {
        self.panel.as_ref().map(|(panel, _)| panel.line(index))
    }

    /// Whether the document's path is not on the disk yet.
    pub fn is_new_file(&self) -> bool {
        self.path.is_some() && !self.exists
    }

    fn update_title(&mut self) {
        let title = title_of(self.name(), self.editor.is_modified());
        if title != self.title {
            self.title = title;
            self.title_changed = true;
        }
    }

    /// The content is now `width` by `height`.
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.clamp_scroll();
        self.damage_all();
    }

    /// What changed since the last call.
    pub fn take_damage(&mut self) -> Damage {
        self.damage.take()
    }

    fn damage_all(&mut self) {
        self.damage.add(self.content());
    }

    pub(crate) fn content(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// The rectangle of toolbar button `tool`.
    pub fn tool_rect(&self, tool: Tool) -> Rect {
        let index = TOOLS
            .iter()
            .position(|(each, _)| *each == tool)
            .unwrap_or(0) as i32;
        Rect::new(TOOL_X + index * TOOL_PITCH, TOOL_Y, TOOL_WIDTH, TOOL_HEIGHT)
    }

    pub(crate) fn bar_rect(&self) -> Rect {
        Rect::new(0, TOOLBAR_HEIGHT, self.width, BAR_HEIGHT)
    }

    pub(crate) fn close_find_rect(&self) -> Rect {
        Rect::new(self.width - CLOSE_FROM_RIGHT, BAR_CONTROL_Y, 28, 28)
    }

    pub(crate) fn match_case_rect(&self) -> Rect {
        Rect::new(
            MATCH_CASE_X,
            BAR_CONTROL_Y,
            widget::LABEL_OFFSET + eggos_toolkit::text::width(MATCH_CASE),
            widget::CONTROL_HEIGHT,
        )
    }

    /// The path bar's field.
    pub fn path_field_rect(&self) -> Rect {
        Rect::new(
            PATH_FIELD_X,
            BAR_CONTROL_Y,
            (self.width - PATH_FIELD_FROM_RIGHT - PATH_FIELD_X).max(0),
            widget::CONTROL_HEIGHT,
        )
    }

    /// The path bar's `Open` or `Save` button.
    pub fn action_rect(&self) -> Rect {
        Rect::new(
            self.width - ACTION_FROM_RIGHT,
            BAR_CONTROL_Y,
            TOOL_WIDTH,
            TOOL_HEIGHT,
        )
    }

    pub(crate) fn cancel_rect(&self) -> Rect {
        Rect::new(
            self.width - CANCEL_FROM_RIGHT,
            BAR_CONTROL_Y,
            TOOL_WIDTH,
            TOOL_HEIGHT,
        )
    }

    /// The editing area: the gutter and the text view, between the bars and the status bar.
    pub fn area(&self) -> Rect {
        let top = match self.bar {
            Bar::Closed => TOOLBAR_HEIGHT,
            _ => TOOLBAR_HEIGHT + BAR_HEIGHT,
        };
        Rect::new(
            0,
            top,
            self.width,
            (self.height - STATUS_HEIGHT - top).max(0),
        )
    }

    pub(crate) fn status_bar(&self) -> Rect {
        Rect::new(0, self.height - STATUS_HEIGHT, self.width, STATUS_HEIGHT)
    }

    /// The gutter's width: room for the line count's digits, at least two.
    pub fn gutter_width(&self) -> i32 {
        let digits = digits(self.editor.document().line_count()).max(MIN_GUTTER_DIGITS);
        16 + CELL_WIDTH * digits as i32
    }

    /// The text view: the editing area right of the gutter.
    pub fn text_view(&self) -> Rect {
        let area = self.area();
        let gutter = self.gutter_width();
        Rect::new(gutter, area.y, (area.width - gutter).max(0), area.height)
    }

    fn widest(&mut self) -> usize {
        if let Some(widest) = self.widest {
            return widest;
        }
        let document = self.editor.document();
        let widest = (0..document.line_count())
            .map(|line| view_width(document.line(line)))
            .max()
            .unwrap_or(0);
        self.widest = Some(widest);
        widest
    }

    pub(crate) fn content_height(&self) -> i32 {
        self.editor.document().line_count() as i32 * ROW_HEIGHT + 2 * TEXT_TOP
    }

    pub(crate) fn content_width(&self) -> i32 {
        (self.widest.unwrap_or(0) as i32 + 1) * CELL_WIDTH + 2 * TEXT_LEFT
    }

    fn bars_shown(&self) -> (bool, bool) {
        let view = self.text_view();
        let mut vertical = self.content_height() > view.height;
        let horizontal =
            self.content_width() > view.width - if vertical { SCROLL_BAR_WIDTH } else { 0 };
        if horizontal && !vertical {
            vertical = self.content_height() > view.height - SCROLL_BAR_WIDTH;
        }
        (vertical, horizontal)
    }

    /// The part of the text view that shows text: less the scroll bars that show.
    pub fn view_size(&self) -> (i32, i32) {
        let view = self.text_view();
        let (vertical, horizontal) = self.bars_shown();
        (
            view.width - if vertical { SCROLL_BAR_WIDTH } else { 0 },
            view.height - if horizontal { SCROLL_BAR_WIDTH } else { 0 },
        )
    }

    /// The vertical scroll bar's track and thumb, when it shows.
    pub fn vertical_bar(&self) -> Option<(Rect, Rect)> {
        let (vertical, _) = self.bars_shown();
        if !vertical {
            return None;
        }
        let view = self.text_view();
        let (_, height) = self.view_size();
        scroll_bar(
            Rect::new(view.x, view.y, view.width, height),
            self.content_height(),
            self.scroll_y,
        )
    }

    /// The horizontal scroll bar's track and thumb, when it shows.
    pub fn horizontal_bar(&self) -> Option<(Rect, Rect)> {
        let (_, horizontal) = self.bars_shown();
        if !horizontal {
            return None;
        }
        let view = self.text_view();
        let (width, _) = self.view_size();
        let content = self.content_width();
        if width <= 0 || content <= width {
            return None;
        }
        let track = Rect::new(
            view.x,
            view.bottom() - SCROLL_BAR_WIDTH,
            width,
            SCROLL_BAR_WIDTH,
        );
        let length = (width * width / content).max(MIN_THUMB).min(width);
        let travel = width - length;
        let position = scaled(self.scroll_x, travel, content - width);
        Some((
            track,
            Rect::new(track.x + position, track.y, length, SCROLL_BAR_WIDTH),
        ))
    }

    fn clamp_scroll(&mut self) {
        self.widest();
        let (width, height) = self.view_size();
        self.scroll_y = self.scroll_y.min(self.content_height() - height).max(0);
        self.scroll_x = self.scroll_x.min(self.content_width() - width).max(0);
    }

    /// Where row `line`'s top is in the content.
    pub fn row_top(&self, line: usize) -> i32 {
        self.area().y + TEXT_TOP + line as i32 * ROW_HEIGHT - self.scroll_y
    }

    /// Where view column `column`'s left edge is in the content.
    pub fn column_left(&self, column: usize) -> i32 {
        self.gutter_width() + TEXT_LEFT + column as i32 * CELL_WIDTH - self.scroll_x
    }

    fn keep_caret_visible(&mut self) {
        self.widest();
        let line = self.editor.caret_line() as i32;
        let column = self.editor.view_column() as i32;
        let (width, height) = self.view_size();
        let top = line * ROW_HEIGHT;
        let bottom = top + ROW_HEIGHT + 2 * TEXT_TOP;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + height {
            self.scroll_y = bottom - height;
        }
        let left = column * CELL_WIDTH;
        let right = left + 2 * TEXT_LEFT;
        if left < self.scroll_x {
            self.scroll_x = left;
        } else if right > self.scroll_x + width {
            self.scroll_x = right - width;
        }
        self.clamp_scroll();
    }

    fn show_notice(&mut self, text: core::fmt::Arguments<'_>) {
        let mut line = Line::new();
        let _ = line.write_fmt(text);
        self.notice = Some(Notice {
            text: line,
            shown_at: self.now_ms,
        });
        self.damage_all();
    }

    fn close_notice(&mut self) {
        if self.notice.take().is_some() {
            self.damage_all();
        }
    }

    fn restart_caret(&mut self) {
        self.caret_epoch = self.now_ms;
        self.caret_shown = true;
    }

    fn changed(&mut self) {
        self.changed_with_widest(None);
    }

    fn changed_with_widest(&mut self, widest: Option<usize>) {
        self.widest = widest;
        self.keep_caret_visible();
        self.restart_caret();
        self.update_title();
        self.damage_all();
    }

    fn moved(&mut self) {
        self.keep_caret_visible();
        self.restart_caret();
        self.damage_all();
    }

    fn blinking(&self) -> Option<Rect> {
        if !self.window_focused || self.panel.is_some() {
            return None;
        }
        match self.focus {
            Focus::Text => Some(self.text_view()),
            Focus::FindField if self.bar == Bar::Find => Some(FIND_FIELD),
            Focus::PathField if matches!(self.bar, Bar::Open | Bar::SaveAs) => {
                Some(self.path_field_rect())
            }
            _ => None,
        }
    }

    /// When the window next wants [`Self::tick`]: the caret's blink, a notice's end, or the
    /// next step of a drag past the text view's edge.
    pub fn deadline(&self, now_ms: u32) -> Option<u32> {
        let blink = self.blinking().map(|_| {
            let since = now_ms.wrapping_sub(self.caret_epoch);
            now_ms.wrapping_add(CARET_BLINK_MS - since % CARET_BLINK_MS)
        });
        let notice = self
            .notice
            .as_ref()
            .map(|notice| notice.shown_at.wrapping_add(NOTICE_MS));
        let dragging = self
            .drag_outside()
            .map(|_| self.drag_scrolled_at.wrapping_add(DRAG_SCROLL_MS));
        [blink, notice, dragging]
            .into_iter()
            .flatten()
            .min_by_key(|deadline| deadline.wrapping_sub(now_ms))
    }

    /// The time is `now_ms`: the caret blinks, a notice ends, a drag past an edge scrolls.
    pub fn tick(&mut self, now_ms: u32) {
        self.now_ms = now_ms;
        if let Some(rect) = self.blinking() {
            let shown = (now_ms.wrapping_sub(self.caret_epoch) / CARET_BLINK_MS).is_multiple_of(2);
            if shown != self.caret_shown {
                self.caret_shown = shown;
                self.damage.add(rect);
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| now_ms.wrapping_sub(notice.shown_at) >= NOTICE_MS)
        {
            self.close_notice();
        }
        if let Some((x, y)) = self.drag_outside()
            && now_ms.wrapping_sub(self.drag_scrolled_at) >= DRAG_SCROLL_MS
        {
            self.drag_scrolled_at = now_ms;
            let view = self.text_view();
            let (width, height) = self.view_size();
            if y < view.y {
                self.scroll_y -= ROW_HEIGHT;
            } else if y >= view.y + height {
                self.scroll_y += ROW_HEIGHT;
            }
            if x < view.x {
                self.scroll_x -= CELL_WIDTH;
            } else if x >= view.x + width {
                self.scroll_x += CELL_WIDTH;
            }
            self.clamp_scroll();
            let at = self.offset_at(x, y);
            self.editor.select(self.editor.anchor(), at);
            self.restart_caret();
            self.damage_all();
        }
    }

    fn drag_outside(&self) -> Option<(i32, i32)> {
        if self.drag != Some(Drag::Text) {
            return None;
        }
        let (x, y) = self.pointer?;
        let view = self.text_view();
        let (width, height) = self.view_size();
        let inside = Rect::new(view.x, view.y, width, height).contains(x, y);
        (!inside).then_some((x, y))
    }

    /// The pointer shape now: the I-beam over the text view and the fields.
    pub fn pointer_shape(&self) -> PointerShape {
        let Some((x, y)) = self
            .pointer
            .filter(|_| self.panel.is_none() && self.menu.is_none())
        else {
            return PointerShape::Arrow;
        };
        let (width, height) = self.view_size();
        let view = self.text_view();
        let over_text = Rect::new(view.x, view.y, width, height).contains(x, y);
        let over_field = match self.bar {
            Bar::Find => FIND_FIELD.contains(x, y),
            Bar::Open | Bar::SaveAs => self.path_field_rect().contains(x, y),
            Bar::Closed => false,
        };
        match over_text || over_field {
            true => PointerShape::IBeam,
            false => PointerShape::Arrow,
        }
    }

    /// The user asked to close the window: `true` closes it at once; with unsaved changes
    /// the window asks first and [`Self::handle`] says when to close.
    pub fn close_requested(&mut self) -> bool {
        if self.closing || !self.editor.is_modified() {
            return true;
        }
        if self.panel.is_none() {
            self.ask_to_save(Then::Close);
        }
        false
    }

    fn ask_to_save(&mut self, then: Then) {
        self.menu = None;
        let mut question = Line::new();
        let _ = write!(question, "Save changes to {}?", self.name());
        self.panel = Some((
            Panel::new(
                &[
                    question.as_str(),
                    "Your changes will be lost if you don't save them.",
                ],
                &["Don't Save", "Cancel", "Save"],
                Some(2),
                1,
            ),
            Asked::SaveChanges(then),
        ));
        self.damage_all();
    }

    fn when_saved(&mut self, files: &mut impl Files, then: Then) {
        match self.editor.is_modified() {
            true => self.ask_to_save(then),
            false => self.carry_out(files, then),
        }
    }

    fn carry_out(&mut self, files: &mut impl Files, then: Then) {
        match then {
            Then::Close => self.closing = true,
            Then::New => {
                self.editor.clear();
                self.path = None;
                self.exists = false;
                self.after_load();
            }
            Then::Open => self.open_bar(Bar::Open),
            Then::OpenFile => {
                let path = self.file_to_open;
                self.open_path(files, path.as_bytes(), false);
            }
        }
    }

    fn after_load(&mut self) {
        self.scroll_x = 0;
        self.scroll_y = 0;
        self.close_bar_if(Bar::Open);
        self.close_bar_if(Bar::SaveAs);
        self.changed();
    }

    fn open_problem(&mut self, name: &str, problem: OpenProblem) {
        let line = file::open_message(name, problem);
        self.show_notice(format_args!("{}", line.as_str()));
    }

    fn open_path(&mut self, files: &mut impl Files, path: &[u8], missing_is_new: bool) {
        let Some(held) = Path::from(path) else {
            let name = shown_name(path);
            self.show_notice(format_args!("Could not open {name} (File name too long)."));
            return;
        };
        match file::check(files, path, self.editor.document().capacity()) {
            Err(OpenProblem::Missing) if missing_is_new => {
                self.editor.clear();
                self.path = Some(held);
                self.exists = false;
                self.after_load();
            }
            Err(problem) => self.open_problem(shown_name(path), problem),
            Ok(_) => {
                let read = file::read(files, path, self.editor.document_mut());
                self.editor.loaded();
                match read {
                    Ok(()) => {
                        self.path = Some(held);
                        self.exists = true;
                    }
                    Err(problem) => {
                        self.path = None;
                        self.exists = false;
                        self.open_problem(shown_name(path), problem);
                    }
                }
                self.after_load();
            }
        }
    }

    fn save_problem(&mut self, path: &[u8], problem: SaveProblem) {
        let line = file::save_message(path, problem);
        self.show_notice(format_args!("{}", line.as_str()));
    }

    fn write_to(&mut self, files: &mut impl Files, path: &[u8]) -> bool {
        let Some(held) = Path::from(path) else {
            self.save_problem(path, SaveProblem::Failed(Errno::ENAMETOOLONG));
            return false;
        };
        match file::save(files, path, self.editor.document()) {
            Ok(()) => {
                self.editor.saved();
                self.path = Some(held);
                self.exists = true;
                self.update_title();
                self.damage_all();
                true
            }
            Err(problem) => {
                self.save_problem(path, problem);
                false
            }
        }
    }

    fn save(&mut self, files: &mut impl Files, then: Option<Then>) {
        let Some(path) = self.path else {
            self.pending = then;
            self.open_bar(Bar::SaveAs);
            return;
        };
        if self.write_to(files, path.as_bytes())
            && let Some(then) = then
        {
            self.carry_out(files, then);
        }
    }

    fn typed_path(&self) -> Option<Path> {
        let typed = self.path_field.text.as_str().as_bytes();
        if typed.is_empty() {
            return None;
        }
        if typed.starts_with(b"/") {
            return Path::from(typed);
        }
        let folder = self.path().map_or(HOME, folder_of);
        Path::from(folder).and_then(|folder| folder.join(typed))
    }

    fn path_action(&mut self, files: &mut impl Files) {
        let Some(path) = self.typed_path() else {
            return;
        };
        match self.bar {
            Bar::Open if self.editor.is_modified() => {
                self.file_to_open = path;
                self.ask_to_save(Then::OpenFile);
            }
            Bar::Open => self.open_path(files, path.as_bytes(), false),
            Bar::SaveAs => {
                let other = self.path() != Some(path.as_bytes());
                if other && files.status(path.as_bytes()).is_ok() {
                    let mut question = Line::new();
                    let _ = write!(question, "Replace {}?", shown_name(path.as_bytes()));
                    let mut place = Line::new();
                    let _ = place.write_str("A file with that name already exists in ");
                    place.push_bytes(folder_of(path.as_bytes()));
                    let _ = place.write_str(".");
                    self.panel = Some((
                        Panel::new(
                            &[question.as_str(), place.as_str()],
                            &["Cancel", "Replace"],
                            Some(1),
                            0,
                        ),
                        Asked::Replace(self.pending),
                    ));
                    self.damage_all();
                    return;
                }
                self.save_as(files, path);
            }
            _ => {}
        }
    }

    fn save_as(&mut self, files: &mut impl Files, path: Path) {
        if self.write_to(files, path.as_bytes()) {
            let then = self.pending.take();
            self.close_bar();
            if let Some(then) = then {
                self.carry_out(files, then);
            }
        }
    }

    fn open_bar(&mut self, bar: Bar) {
        self.bar = bar;
        self.menu = None;
        match bar {
            Bar::Find => {
                self.focus = Focus::FindField;
                self.find_field.caret = self.find_field.text.len();
            }
            Bar::Open | Bar::SaveAs => {
                let mut text = Line::new();
                match (bar, self.path()) {
                    (Bar::Open, Some(path)) => text.push_bytes(folder_of(path)),
                    (Bar::Open, None) => text.push_bytes(HOME),
                    (_, Some(path)) => text.push_bytes(path),
                    (_, None) => {
                        let _ = text.write_str(UNTITLED_PATH);
                    }
                }
                if bar == Bar::Open && !text.as_str().ends_with('/') {
                    let _ = text.write_str("/");
                }
                self.path_field.text.set(text.as_str());
                self.path_field.caret = self.path_field.text.len();
                self.path_field.scroll = 0;
                let rect = self.path_field_rect();
                widget::keep_caret_visible(&mut self.path_field, rect);
                self.focus = Focus::PathField;
            }
            Bar::Closed => self.focus = Focus::Text,
        }
        self.restart_caret();
        self.clamp_scroll();
        self.damage_all();
    }

    fn close_bar(&mut self) {
        if self.bar == Bar::SaveAs {
            self.pending = None;
        }
        self.bar = Bar::Closed;
        self.focus = Focus::Text;
        self.restart_caret();
        self.clamp_scroll();
        self.damage_all();
    }

    fn close_bar_if(&mut self, bar: Bar) {
        if self.bar == bar {
            self.close_bar();
        }
    }

    fn search(&mut self, from: usize, forward: bool) {
        let needle = self.find_field.text.as_str().as_bytes();
        if needle.is_empty() {
            self.found = Found::Nothing;
            self.damage_all();
            return;
        }
        let text = self.editor.document().bytes();
        let hit = find::search(text, from, needle, self.match_case, forward);
        self.found = match hit {
            Some(hit) => {
                let (index, total) = find::rank(text, hit.at, needle, self.match_case);
                let length = needle.len();
                self.editor.select(hit.at, hit.at + length);
                self.keep_caret_visible();
                Found::Match {
                    index,
                    total,
                    wrap: hit.wrap,
                }
            }
            None => Found::NotFound,
        };
        self.restart_caret();
        self.damage_all();
    }

    fn find_again(&mut self, forward: bool) {
        let (start, end) = self.editor.selection();
        match forward {
            true => self.search(end, true),
            false => self.search(start, false),
        }
    }

    fn find_from_selection(&mut self) {
        let (start, _) = self.editor.selection();
        self.search(start, true);
    }

    /// What the find bar's result says, as drawn.
    pub fn found_text(&self) -> Line {
        let mut line = Line::new();
        let count = |line: &mut Line, number: usize| {
            let _ = match number > MOST_COUNTED {
                true => line.write_str("999+"),
                false => write!(line, "{number}"),
            };
        };
        match self.found {
            Found::Nothing => {}
            Found::NotFound => {
                let _ = line.write_str("Not found");
            }
            Found::Match { index, total, wrap } => {
                count(&mut line, index);
                let _ = line.write_str(" of ");
                count(&mut line, total);
                let _ = line.write_str(match wrap {
                    Wrap::No => "",
                    Wrap::FromTop => ", from top",
                    Wrap::FromEnd => ", from end",
                });
            }
        }
        line
    }

    fn full(&mut self) {
        self.show_notice(format_args!(
            "The document is full ({} MB).",
            TEXT_CAPACITY >> 20
        ));
    }

    fn edited(&mut self, result: Result<(), Full>) {
        match result {
            Ok(()) => self.changed(),
            Err(Full) => self.full(),
        }
    }

    fn typed_in_line(&mut self, widest_before: Option<usize>, result: Result<(), Full>) {
        match result {
            Ok(()) => {
                let line = self.editor.document().line(self.editor.caret_line());
                let widest = widest_before.map(|widest| widest.max(view_width(line)));
                self.changed_with_widest(widest);
            }
            Err(Full) => self.full(),
        }
    }

    fn rows_per_page(&self) -> usize {
        let (_, height) = self.view_size();
        ((height / ROW_HEIGHT) - 1).max(1) as usize
    }

    /// Hands the window `input`, which happened at `time_ms`; `false` once the window should
    /// close.
    pub fn handle(&mut self, files: &mut impl Files, input: Input, time_ms: u32) -> bool {
        self.now_ms = time_ms;
        if matches!(
            input,
            Input::Key { pressed: true, .. } | Input::Button { pressed: true, .. }
        ) {
            self.close_notice();
        }
        match input {
            Input::FocusIn | Input::FocusOut => {
                self.window_focused = input == Input::FocusIn;
                if !self.window_focused {
                    self.menu = None;
                    self.pressed = None;
                    self.drag = None;
                    self.shift_held = false;
                }
                self.restart_caret();
                self.damage_all();
                return !self.closing;
            }
            Input::PointerLeave => {
                self.pointer = None;
                self.hover = None;
                if let Some(menu) = self.menu.as_mut() {
                    menu.leave();
                }
                if let Some((panel, _)) = self.panel.as_mut() {
                    panel.pointer_leave();
                }
                self.damage_all();
                return !self.closing;
            }
            Input::PointerMove { x, y } => self.pointer = Some((x, y)),
            Input::Key {
                code: code::LEFT_SHIFT | code::RIGHT_SHIFT,
                pressed,
                ..
            } => self.shift_held = pressed,
            _ => {}
        }
        if self.panel.is_some() {
            self.panel_input(files, input);
            return !self.closing;
        }
        if self.menu.is_some() && self.menu_input(input) {
            return !self.closing;
        }
        match input {
            Input::Key {
                code,
                pressed: true,
                modifiers,
                character,
            } => self.key(files, code, modifiers, character),
            Input::PointerMove { x, y } => self.pointer_moved(x, y),
            Input::Button {
                x,
                y,
                button,
                pressed,
            } => self.button(files, x, y, button, pressed),
            Input::Wheel { x, y, notches } if self.area().contains(x, y) => {
                self.wheel(notches, self.shift_held);
            }
            Input::SidewaysWheel { x, y, notches } if self.area().contains(x, y) => {
                self.wheel(-notches, true);
            }
            _ => {}
        }
        !self.closing
    }

    fn wheel(&mut self, notches: i32, sideways: bool) {
        match sideways {
            true => self.scroll_x -= notches * WHEEL_COLUMNS * CELL_WIDTH,
            false => self.scroll_y -= notches * WHEEL_ROWS * ROW_HEIGHT,
        }
        self.clamp_scroll();
        self.damage_all();
    }

    fn panel_input(&mut self, files: &mut impl Files, input: Input) {
        let area = self.area();
        let Some((panel, _)) = self.panel.as_mut() else {
            return;
        };
        let response = match input {
            Input::Key {
                code,
                pressed,
                modifiers,
                ..
            } => panel.key(code, modifiers.shift(), pressed),
            Input::PointerMove { x, y } => {
                panel.pointer_move(area, x, y);
                PanelResponse::Nothing
            }
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => panel.button(area, x, y, pressed),
            _ => PanelResponse::Nothing,
        };
        self.damage_all();
        if let PanelResponse::Pressed(button) = response {
            self.answered(files, button);
        }
    }

    fn answered(&mut self, files: &mut impl Files, button: usize) {
        let Some((_, asked)) = self.panel.take() else {
            return;
        };
        self.damage_all();
        match (asked, button) {
            (Asked::SaveChanges(then), 0) => self.carry_out(files, then),
            (Asked::SaveChanges(then), 2) => self.save(files, Some(then)),
            (Asked::Replace(then), 1) => {
                if let Some(path) = self.typed_path() {
                    self.pending = then;
                    self.save_as(files, path);
                }
            }
            _ => {}
        }
    }

    fn menu_input(&mut self, input: Input) -> bool {
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match input {
            Input::Key {
                code,
                pressed: true,
                ..
            } => {
                let answer = menu.key(code);
                self.damage_all();
                match answer {
                    MenuKey::Chosen(id) => {
                        self.menu = None;
                        self.chosen(id);
                    }
                    MenuKey::Closed => self.menu = None,
                    MenuKey::Nothing => {}
                }
                true
            }
            Input::Key { .. } => true,
            Input::PointerMove { x, y } => {
                let changed = menu.hover(x, y);
                let inside = menu.rect().contains(x, y);
                if changed {
                    self.damage_all();
                }
                inside
            }
            Input::Button { x, y, pressed, .. } if menu.rect().contains(x, y) => {
                if !pressed && let Some(id) = menu.id_at(x, y) {
                    self.menu = None;
                    self.chosen(id);
                    self.damage_all();
                }
                true
            }
            Input::Button { pressed: true, .. } => {
                self.menu = None;
                self.damage_all();
                false
            }
            _ => false,
        }
    }

    fn open_menu(&mut self, x: i32, y: i32) {
        let editor = &self.editor;
        let entries = [
            Entry::Item {
                label: "Undo",
                enabled: editor.can_undo(),
                id: item::UNDO,
            },
            Entry::Item {
                label: "Redo",
                enabled: editor.can_redo(),
                id: item::REDO,
            },
            Entry::Separator,
            Entry::Item {
                label: "Cut",
                enabled: editor.has_selection(),
                id: item::CUT,
            },
            Entry::Item {
                label: "Copy",
                enabled: editor.has_selection(),
                id: item::COPY,
            },
            Entry::Item {
                label: "Paste",
                enabled: editor.can_paste(),
                id: item::PASTE,
            },
            Entry::Separator,
            Entry::Item {
                label: "Select All",
                enabled: !editor.document().is_empty(),
                id: item::SELECT_ALL,
            },
        ];
        self.menu = Some(Menu::new(&entries, x, y, self.content()));
        self.damage_all();
    }

    /// The rectangle of the context menu shown, if one is.
    pub fn menu_rect(&self) -> Option<Rect> {
        self.menu.as_ref().map(Menu::rect)
    }

    /// The rectangle of entry `index` of the context menu shown.
    pub fn menu_entry_rect(&self, index: usize) -> Option<Rect> {
        self.menu.as_ref().map(|menu| menu.entry_rect(index))
    }

    /// The rectangle of button `index` of the panel shown.
    pub fn dialog_button_rect(&self, index: usize) -> Option<Rect> {
        let area = self.area();
        self.panel
            .as_ref()
            .map(|(panel, _)| panel.button_rect(area, index))
    }

    fn chosen(&mut self, id: u8) {
        match id {
            item::UNDO => {
                self.editor.undo();
                self.changed();
            }
            item::REDO => {
                self.editor.redo();
                self.changed();
            }
            item::CUT => {
                self.editor.cut();
                self.copied = true;
                self.changed();
            }
            item::COPY => {
                self.editor.copy();
                self.copied = true;
            }
            item::PASTE => {
                let pasted = self.editor.paste();
                self.edited(pasted);
            }
            item::SELECT_ALL => {
                self.editor.select_all();
                self.moved();
            }
            _ => {}
        }
    }

    fn tool(&mut self, files: &mut impl Files, tool: Tool) {
        match tool {
            Tool::New => self.when_saved(files, Then::New),
            Tool::Open => self.when_saved(files, Then::Open),
            Tool::Save => self.save(files, None),
            Tool::SaveAs => {
                self.pending = None;
                self.open_bar(Bar::SaveAs);
            }
            Tool::Find => self.open_bar(Bar::Find),
        }
    }

    fn find_key(&mut self, forward: bool) {
        match (self.bar, self.find_field.text.is_empty()) {
            (Bar::Find, _) => self.find_again(forward),
            (_, true) => self.open_bar(Bar::Find),
            (_, false) => {
                self.open_bar(Bar::Find);
                self.focus = Focus::Text;
                self.find_again(forward);
            }
        }
    }

    fn key(&mut self, files: &mut impl Files, code: u16, modifiers: Modifiers, character: u32) {
        let held = modifiers.0;
        let primary = held & (Modifiers::CTRL | Modifiers::SUPER) != 0;
        let shift = modifiers.shift();
        let in_text = self.focus == Focus::Text;
        match code {
            code::N if primary => return self.tool(files, Tool::New),
            code::O if primary => return self.tool(files, Tool::Open),
            code::S if primary && shift => return self.tool(files, Tool::SaveAs),
            code::S if primary => return self.tool(files, Tool::Save),
            code::F if primary => return self.tool(files, Tool::Find),
            code::G if primary => return self.find_key(!shift),
            code::F3 => return self.find_key(!shift),
            key::ESC => {
                if self.bar != Bar::Closed {
                    self.close_bar();
                }
                return;
            }
            code::F10 if shift && in_text => return self.menu_at_caret(),
            code::MENU if in_text => return self.menu_at_caret(),
            _ => {}
        }
        match self.focus {
            Focus::Text => self.text_key(code, modifiers, character),
            Focus::FindField => self.find_field_key(code, shift, primary, character),
            Focus::MatchCase => match code {
                key::SPACE => self.toggle_match_case(),
                key::TAB => {
                    self.focus = Focus::FindField;
                    self.restart_caret();
                    self.damage_all();
                }
                key::ENTER => self.find_again(!shift),
                _ => {}
            },
            Focus::PathField => self.path_field_key(files, code, primary, character),
        }
    }

    fn toggle_match_case(&mut self) {
        self.match_case = !self.match_case;
        self.find_from_selection();
    }

    fn find_field_key(&mut self, code: u16, shift: bool, primary: bool, character: u32) {
        match code {
            key::ENTER => return self.find_again(!shift),
            key::TAB => {
                self.focus = Focus::MatchCase;
                self.damage_all();
                return;
            }
            _ => {}
        }
        let before = self.find_field.text;
        let typed = widget::field_key(&mut self.find_field, code, character, primary, &before);
        widget::keep_caret_visible(&mut self.find_field, FIND_FIELD);
        self.restart_caret();
        self.damage_all();
        if typed == FieldKey::Changed {
            self.find_from_selection();
        }
    }

    fn path_field_key(&mut self, files: &mut impl Files, code: u16, primary: bool, character: u32) {
        match code {
            key::ENTER => return self.path_action(files),
            key::TAB => return,
            _ => {}
        }
        let before = self.path_field.text;
        widget::field_key(&mut self.path_field, code, character, primary, &before);
        let rect = self.path_field_rect();
        widget::keep_caret_visible(&mut self.path_field, rect);
        self.restart_caret();
        self.damage_all();
    }

    fn menu_at_caret(&mut self) {
        let column = self.editor.view_column();
        let line = self.editor.caret_line();
        let x = self.column_left(column);
        let y = self.row_top(line) + ROW_HEIGHT;
        self.open_menu(x, y);
    }

    fn text_key(&mut self, code: u16, modifiers: Modifiers, character: u32) {
        let held = modifiers.0;
        let ctrl = held & Modifiers::CTRL != 0;
        let command = held & Modifiers::SUPER != 0;
        let alt = held & Modifiers::ALT != 0;
        let typing_alt = alt && held & Modifiers::ALT_GRAPH == 0;
        let primary = ctrl || command;
        let shift = modifiers.shift();
        let motion = match code {
            key::LEFT if command => Some(Motion::LineStart),
            key::RIGHT if command => Some(Motion::LineEnd),
            key::LEFT if ctrl || alt => Some(Motion::WordLeft),
            key::RIGHT if ctrl || alt => Some(Motion::WordRight),
            key::LEFT => Some(Motion::Left),
            key::RIGHT => Some(Motion::Right),
            key::UP => Some(Motion::Up),
            key::DOWN => Some(Motion::Down),
            key::HOME if primary => Some(Motion::DocumentStart),
            key::END if primary => Some(Motion::DocumentEnd),
            key::HOME => Some(Motion::LineStart),
            key::END => Some(Motion::LineEnd),
            key::PAGE_UP => Some(Motion::PageUp(self.rows_per_page())),
            key::PAGE_DOWN => Some(Motion::PageDown(self.rows_per_page())),
            _ => None,
        };
        if let Some(motion) = motion {
            self.editor.go(motion, shift);
            if let Motion::PageUp(rows) | Motion::PageDown(rows) = motion {
                let step = rows as i32 * ROW_HEIGHT;
                self.scroll_y += match motion {
                    Motion::PageUp(_) => -step,
                    _ => step,
                };
                self.clamp_scroll();
            }
            return self.moved();
        }
        match code {
            code::Z if primary && shift => self.chosen(item::REDO),
            code::Z if primary => self.chosen(item::UNDO),
            code::Y if primary => self.chosen(item::REDO),
            code::X if primary => self.chosen(item::CUT),
            code::C if primary => self.chosen(item::COPY),
            code::V if primary => self.chosen(item::PASTE),
            code::A if primary => self.chosen(item::SELECT_ALL),
            key::BACKSPACE => {
                self.editor.backspace();
                self.changed();
            }
            key::DELETE => {
                self.editor.delete();
                self.changed();
            }
            key::ENTER => {
                let typed = self.editor.type_character('\n');
                self.edited(typed);
            }
            key::TAB if !primary && !alt => {
                let typed = self.editor.type_character('\t');
                self.edited(typed);
            }
            _ if !primary && !typing_alt => {
                let Some(typed) = char::from_u32(character).filter(|typed| !typed.is_control())
                else {
                    return;
                };
                let widest = self.widest.filter(|_| !self.editor.has_selection());
                let typed = self.editor.type_character(typed);
                self.typed_in_line(widest, typed);
            }
            _ => {}
        }
    }

    fn hovered_part(&self, x: i32, y: i32) -> Option<Part> {
        if let Some((tool, _)) = TOOLS
            .iter()
            .find(|(tool, _)| self.tool_rect(*tool).contains(x, y))
        {
            return Some(Part::Tool(*tool));
        }
        match self.bar {
            Bar::Find if PREVIOUS_BUTTON.contains(x, y) => return Some(Part::Previous),
            Bar::Find if NEXT_BUTTON.contains(x, y) => return Some(Part::Next),
            Bar::Find if self.close_find_rect().contains(x, y) => return Some(Part::CloseFind),
            Bar::Find if self.match_case_rect().contains(x, y) => return Some(Part::MatchCase),
            Bar::Open | Bar::SaveAs if self.action_rect().contains(x, y) => {
                return Some(Part::Action);
            }
            Bar::Open | Bar::SaveAs if self.cancel_rect().contains(x, y) => {
                return Some(Part::Cancel);
            }
            _ => {}
        }
        if let Some((_, thumb)) = self.vertical_bar()
            && thumb.contains(x, y)
        {
            return Some(Part::VerticalThumb);
        }
        if let Some((_, thumb)) = self.horizontal_bar()
            && thumb.contains(x, y)
        {
            return Some(Part::HorizontalThumb);
        }
        None
    }

    fn pointer_moved(&mut self, x: i32, y: i32) {
        let hover = self.hovered_part(x, y);
        if hover != self.hover {
            self.hover = hover;
            self.damage_all();
        }
        match self.drag {
            Some(Drag::Text) => {
                let at = self.offset_at(x, y);
                if at != self.editor.caret() {
                    self.editor.select(self.editor.anchor(), at);
                    self.moved();
                }
            }
            Some(Drag::Lines { anchor_line }) => {
                let line = self.line_at(y);
                self.editor.select_lines(anchor_line, line);
                self.moved();
            }
            Some(Drag::Vertical { grab, start }) => {
                if let Some((track, thumb)) = self.vertical_bar() {
                    let travel = (track.height - thumb.height).max(1);
                    let (_, height) = self.view_size();
                    let range = self.content_height() - height;
                    self.scroll_y = start.saturating_add(scaled(y - grab, range, travel));
                    self.clamp_scroll();
                    self.damage_all();
                }
            }
            Some(Drag::Horizontal { grab, start }) => {
                if let Some((track, thumb)) = self.horizontal_bar() {
                    let travel = (track.width - thumb.width).max(1);
                    let (width, _) = self.view_size();
                    let range = self.content_width() - width;
                    self.scroll_x = start.saturating_add(scaled(x - grab, range, travel));
                    self.clamp_scroll();
                    self.damage_all();
                }
            }
            None => {}
        }
    }

    fn line_at(&self, y: i32) -> usize {
        let row = (y - self.area().y - TEXT_TOP + self.scroll_y).div_euclid(ROW_HEIGHT);
        (row.max(0) as usize).min(self.editor.document().line_count() - 1)
    }

    fn offset_at(&self, x: i32, y: i32) -> usize {
        let line = self.line_at(y);
        let document = self.editor.document();
        let into = x - self.column_left(0);
        document.line_start(line) + byte_nearest(document.line(line), into, CELL_WIDTH)
    }

    fn offset_under(&self, x: i32, y: i32) -> usize {
        let line = self.line_at(y);
        let document = self.editor.document();
        let into = x - self.column_left(0);
        document.line_start(line) + byte_under(document.line(line), into, CELL_WIDTH)
    }

    fn click_count(&mut self, x: i32, y: i32) -> u8 {
        let count = match self.last_click {
            Some(click)
                if self.now_ms.wrapping_sub(click.at) <= DOUBLE_CLICK_MS
                    && (click.x - x).abs() <= CLICK_SLOP
                    && (click.y - y).abs() <= CLICK_SLOP =>
            {
                click.count % 3 + 1
            }
            _ => 1,
        };
        self.last_click = Some(Click {
            x,
            y,
            at: self.now_ms,
            count,
        });
        count
    }

    fn button(
        &mut self,
        files: &mut impl Files,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    ) {
        if !pressed {
            self.drag = None;
            if let Some(part) = self.pressed.take() {
                self.damage_all();
                if self.hovered_part(x, y) == Some(part) {
                    self.part_clicked(files, part);
                }
            }
            return;
        }
        let area = self.area();
        let view = self.text_view();
        let (width, height) = self.view_size();
        let in_text = Rect::new(view.x, view.y, width, height).contains(x, y);
        let in_gutter = Rect::new(0, area.y, view.x, area.height).contains(x, y);
        if button == MouseButton::Right {
            if in_text || in_gutter {
                let at = self.offset_at(x, y);
                let (start, end) = self.editor.selection();
                if at < start || at > end || start == end {
                    self.editor.place(at);
                }
                self.focus = Focus::Text;
                self.open_menu(x, y);
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        if let Some(part) = self.hovered_part(x, y) {
            match part {
                Part::VerticalThumb => {
                    self.drag = Some(Drag::Vertical {
                        grab: y,
                        start: self.scroll_y,
                    });
                }
                Part::HorizontalThumb => {
                    self.drag = Some(Drag::Horizontal {
                        grab: x,
                        start: self.scroll_x,
                    });
                }
                part => self.pressed = Some(part),
            }
            self.damage_all();
            return;
        }
        if let Some((track, thumb)) = self.vertical_bar()
            && track.contains(x, y)
        {
            let page = (track.height - TRACK_KEEP).max(ROW_HEIGHT);
            self.scroll_y += if y < thumb.y { -page } else { page };
            self.clamp_scroll();
            self.damage_all();
            return;
        }
        if let Some((track, thumb)) = self.horizontal_bar()
            && track.contains(x, y)
        {
            let page = (track.width - TRACK_KEEP).max(CELL_WIDTH);
            self.scroll_x += if x < thumb.x { -page } else { page };
            self.clamp_scroll();
            self.damage_all();
            return;
        }
        match self.bar {
            Bar::Find if FIND_FIELD.contains(x, y) => {
                self.focus = Focus::FindField;
                self.find_field.caret = widget::caret_at(&self.find_field, FIND_FIELD, x);
                self.restart_caret();
                self.damage_all();
                return;
            }
            Bar::Open | Bar::SaveAs if self.path_field_rect().contains(x, y) => {
                self.focus = Focus::PathField;
                self.path_field.caret =
                    widget::caret_at(&self.path_field, self.path_field_rect(), x);
                self.restart_caret();
                self.damage_all();
                return;
            }
            _ => {}
        }
        if in_gutter {
            let line = self.line_at(y);
            self.focus = Focus::Text;
            self.editor.select_lines(line, line);
            self.drag = Some(Drag::Lines { anchor_line: line });
            self.moved();
            return;
        }
        if !in_text {
            return;
        }
        self.focus = Focus::Text;
        let shift = self.shift_held;
        match self.click_count(x, y) {
            2 => {
                let at = self.offset_under(x, y);
                self.editor.select_run(at);
            }
            3 => {
                let line = self.line_at(y);
                self.editor.select_lines(line, line);
            }
            1 if shift => {
                let at = self.offset_at(x, y);
                self.editor.select(self.editor.anchor(), at);
                self.drag = Some(Drag::Text);
            }
            _ => {
                let at = self.offset_at(x, y);
                self.editor.place(at);
                self.drag = Some(Drag::Text);
            }
        }
        self.drag_scrolled_at = self.now_ms;
        self.moved();
    }

    fn part_clicked(&mut self, files: &mut impl Files, part: Part) {
        match part {
            Part::Tool(tool) => self.tool(files, tool),
            Part::Previous => self.find_again(false),
            Part::Next => self.find_again(true),
            Part::CloseFind | Part::Cancel => self.close_bar(),
            Part::MatchCase => {
                self.focus = Focus::MatchCase;
                self.toggle_match_case();
            }
            Part::Action => self.path_action(files),
            Part::VerticalThumb | Part::HorizontalThumb => {}
        }
    }
}

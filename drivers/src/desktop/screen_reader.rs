//! The screen reader's stream (D603, docs/ux/accessibility-v0.4.md section 1): utterances from
//! the focus, the windows, the shell's popups and notices, the keys and the reading keys, shown
//! in a caption panel at the bottom of the work area and kept in a log of the last
//! [`READER_LOG_ENTRIES`]. An interrupting utterance shows at once and drops the queue; a queued
//! one waits until the shown line has been up for [`caption_ms`]. Off, nothing here costs
//! anything: no log, no panel, no timer. Speech, later, reads the same stream.

use super::paint::{self, Canvas};
use super::{Desktop, colour};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use alloc::boxed::Box;
use eggos_element_tree::reader::{MAX_UTTERANCE_BYTES, Priority, Source, UtteranceRecord};

/// The utterances the log keeps.
pub const READER_LOG_ENTRIES: usize = 256;
const QUEUE_ENTRIES: usize = 32;
const PANEL_HEIGHT: u32 = 56;
const PANEL_MOST_WIDTH: u32 = 1024;
const PANEL_MARGIN: u32 = 16;
const TEXT_X: u32 = 16;
const FIRST_LINE_Y: u32 = 12;
const SECOND_LINE_Y: u32 = 28;
const OFF_SHOWN_MS: u32 = 1500;
const ELLIPSIS: &str = "…";
/// What the reader says when it turns on.
pub const READER_ON: &str = "Screen reader on";
/// What it says when it turns off.
pub const READER_OFF: &str = "Screen reader off";

/// What a reading key asks (docs/ux/accessibility-v0.4.md 1.5): the reading cursor moves
/// through the focused window's tree, never the focus, until `Activate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderCommand {
    Next,
    Previous,
    First,
    Last,
    /// Says the cursor's line again, its container's first.
    Current,
    /// Says the window's line.
    Title,
    /// Says the window's line, then every element in reading order.
    Window,
    /// Every element from the cursor to the end.
    FromHere,
    /// Invokes the cursor's element (focuses a text field).
    Activate,
}

/// Caps Lock held as the reader's modifier: whether it is held, and whether a chord used it
/// since it went down (its toggle is then given back).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ReaderKeys {
    caps_held: bool,
    chorded: bool,
}

/// The screen reader's settings (`SET_ACCESSIBILITY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderSettings {
    pub on: bool,
    pub captions: bool,
    /// Words a minute, 100 to 400.
    pub rate: u32,
    pub echo_characters: bool,
}

impl ReaderSettings {
    /// Off, captions on, 200 words a minute, characters echoed.
    pub const DEFAULT: Self = Self {
        on: false,
        captions: true,
        rate: 200,
        echo_characters: true,
    };
}

/// How long a caption of `glyphs` characters stays before a queued one replaces it, at `rate`
/// words a minute: 500 ms, and 12000 / rate ms a character.
pub fn caption_ms(glyphs: usize, rate: u32) -> u32 {
    500 + (glyphs as u32).saturating_mul(12_000) / rate.max(1)
}

#[derive(Debug, Clone, Copy)]
struct Line {
    serial: u32,
    at_ms: u64,
    source: Source,
    priority: Priority,
    text: [u8; MAX_UTTERANCE_BYTES],
    length: u8,
}

impl Line {
    const EMPTY: Self = Self {
        serial: 0,
        at_ms: 0,
        source: Source::Say,
        priority: Priority::Queue,
        text: [0; MAX_UTTERANCE_BYTES],
        length: 0,
    };

    fn text(&self) -> &str {
        core::str::from_utf8(&self.text[..usize::from(self.length)]).unwrap_or("")
    }
}

/// The reader's state in the window server.
#[derive(Debug)]
pub(super) struct ScreenReader {
    settings: ReaderSettings,
    log: Option<Box<[Line; READER_LOG_ENTRIES]>>,
    logged: u32,
    queue: [u32; QUEUE_ENTRIES],
    queued: usize,
    shown: Option<(u32, u32)>,
    previous: u32,
    hide_at: Option<u32>,
    panel: Option<Rect>,
    at_top: bool,
    last_container: u32,
    reports_wanted: bool,
}

impl Default for ScreenReader {
    fn default() -> Self {
        Self {
            settings: ReaderSettings::DEFAULT,
            log: None,
            logged: 0,
            queue: [0; QUEUE_ENTRIES],
            queued: 0,
            shown: None,
            previous: 0,
            hide_at: None,
            panel: None,
            at_top: false,
            last_container: 0,
            reports_wanted: false,
        }
    }
}

/// A line built from parts, cut at [`MAX_UTTERANCE_BYTES`].
pub(super) struct LineText {
    bytes: [u8; MAX_UTTERANCE_BYTES],
    length: usize,
}

impl LineText {
    pub(super) const fn new() -> Self {
        Self {
            bytes: [0; MAX_UTTERANCE_BYTES],
            length: 0,
        }
    }

    pub(super) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

impl core::fmt::Write for LineText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let text = cut_to(text, self.bytes.len() - self.length);
        self.bytes[self.length..self.length + text.len()].copy_from_slice(text.as_bytes());
        self.length += text.len();
        Ok(())
    }
}

fn cut_to(text: &str, most: usize) -> &str {
    let mut cut = text.len().min(most);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    &text[..cut]
}

impl ScreenReader {
    fn line(&self, serial: u32) -> Option<&Line> {
        let log = self.log.as_ref()?;
        let line = &log[serial as usize % READER_LOG_ENTRIES];
        (serial != 0 && line.serial == serial).then_some(line)
    }

    fn log(&mut self, source: Source, priority: Priority, text: &str, at_ms: u64) -> u32 {
        let log = self
            .log
            .get_or_insert_with(|| Box::new([Line::EMPTY; READER_LOG_ENTRIES]));
        self.logged = self.logged.wrapping_add(1).max(1);
        let text = cut_to(text, MAX_UTTERANCE_BYTES);
        let mut line = Line {
            serial: self.logged,
            at_ms,
            source,
            priority,
            length: text.len() as u8,
            ..Line::EMPTY
        };
        line.text[..text.len()].copy_from_slice(text.as_bytes());
        log[self.logged as usize % READER_LOG_ENTRIES] = line;
        self.logged
    }
}

impl Desktop {
    /// Takes the reader's settings: turning it on says so, then the focused window's line;
    /// turning it off says so for a moment, then the panel goes.
    pub fn set_screen_reader(&mut self, settings: ReaderSettings, damage: &mut Damage) {
        let was_on = self.reader.settings.on;
        self.reader.settings = settings;
        match (was_on, settings.on) {
            (false, true) => {
                self.reader.hide_at = None;
                self.say(Source::On, Priority::Interrupt, READER_ON, damage);
                self.say_focused_window(Priority::Queue, damage);
            }
            (true, false) => {
                self.reader.settings.on = true;
                self.say(Source::On, Priority::Interrupt, READER_OFF, damage);
                self.reader.settings.on = false;
                self.reader.hide_at = Some(self.now_ms.wrapping_add(OFF_SHOWN_MS));
            }
            _ => self.damage_caption_panel(damage),
        }
    }

    /// The reader's settings in force.
    pub fn screen_reader(&self) -> ReaderSettings {
        self.reader.settings
    }

    /// Whether apps should report their focused element: while the reader or zoom is on.
    pub fn wants_element_reports(&self) -> bool {
        self.reader.settings.on || self.zoom_view().is_some()
    }

    /// Tells every window whether to report its focused element when that changed since the
    /// last time (`READER_CHANGED`, D603).
    pub fn send_report_wish(&mut self, events: &mut dyn super::protocol::EventSink) {
        let wanted = self.wants_element_reports();
        if wanted == self.reader.reports_wanted {
            return;
        }
        self.reader.reports_wanted = wanted;
        for slot in 0..self.windows.len() {
            if self.windows[slot].is_some() {
                self.send(
                    slot,
                    super::protocol::EventRecord::new(
                        super::protocol::event::READER_CHANGED,
                        0,
                        i32::from(wanted),
                        0,
                        0,
                        0,
                    ),
                    events,
                );
            }
        }
    }

    /// Says `text` from `source`: logged, then shown at once and the queue dropped when it
    /// interrupts, else after what shows. Nothing while the reader is off.
    pub fn say(&mut self, source: Source, priority: Priority, text: &str, damage: &mut Damage) {
        if !self.reader.settings.on || text.is_empty() {
            return;
        }
        let serial = self
            .reader
            .log(source, priority, text, u64::from(self.now_ms));
        match priority {
            Priority::Interrupt => {
                self.reader.queued = 0;
                self.show_caption(serial, damage);
            }
            Priority::Queue if self.reader.shown.is_none() => self.show_caption(serial, damage),
            Priority::Queue => {
                if self.reader.queued == QUEUE_ENTRIES {
                    self.reader.queue.copy_within(1.., 0);
                    self.reader.queued -= 1;
                }
                self.reader.queue[self.reader.queued] = serial;
                self.reader.queued += 1;
            }
        }
    }

    /// Drops what is queued; the panel keeps its line (`Reader+Esc`, a Ctrl tap).
    pub fn stop_reading(&mut self) {
        self.reader.queued = 0;
    }

    /// A notice showed: the reader says it after what it is saying.
    pub(super) fn reader_notice(&mut self, text: &str, damage: &mut Damage) {
        if !self.reader.settings.on || text.is_empty() {
            return;
        }
        let mut line = LineText::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!("Notice: {text}"));
        self.say(Source::Notice, Priority::Queue, line.as_str(), damage);
    }

    /// Takes the reading keys while the reader is on: Caps Lock held with an arrow, Home, End,
    /// T, W, R, Enter or Esc (and Ctrl+Alt with them in the macOS profile, VoiceOver's keys);
    /// a Ctrl tapped alone stops reading. Returns what the kernel must do.
    pub(super) fn reader_key(
        &mut self,
        code: u16,
        pressed: bool,
        modifiers: crate::input::keymap::Modifiers,
    ) -> Option<super::Action> {
        use crate::input::key;
        use crate::input::keymap::Modifiers;
        if !self.reader.settings.on {
            return None;
        }
        if code == key::CAPS_LOCK {
            match pressed {
                true => self.reader_caps = ReaderKeys {
                    caps_held: true,
                    chorded: false,
                },
                false => self.reader_caps.caps_held = false,
            }
            return None;
        }
        if !pressed {
            return None;
        }
        if matches!(code, key::LEFT_CTRL | key::RIGHT_CTRL) && !self.reader_caps.caps_held {
            self.stop_reading();
            return None;
        }
        let held = modifiers.bits() & !Modifiers::CAPS_LOCK.bits();
        let voice_over = self.profile == super::Profile::Macos
            && held == Modifiers::CTRL.bits() | Modifiers::ALT.bits();
        if !self.reader_caps.caps_held && !voice_over {
            return None;
        }
        let command = match code {
            key::RIGHT => ReaderCommand::Next,
            key::LEFT => ReaderCommand::Previous,
            key::HOME => ReaderCommand::First,
            key::END => ReaderCommand::Last,
            key::UP => ReaderCommand::Current,
            key::T => ReaderCommand::Title,
            key::W => ReaderCommand::Window,
            key::R => ReaderCommand::FromHere,
            key::ENTER => ReaderCommand::Activate,
            key::ESC => {
                self.stop_reading();
                return Some(super::Action::None);
            }
            _ => return None,
        };
        let undo_caps_lock = self.reader_caps.caps_held && !self.reader_caps.chorded;
        self.reader_caps.chorded |= self.reader_caps.caps_held;
        Some(super::Action::Reader {
            command,
            undo_caps_lock,
        })
    }

    fn show_caption(&mut self, serial: u32, damage: &mut Damage) {
        if let Some((shown, _)) = self.reader.shown {
            self.reader.previous = shown;
        }
        self.reader.shown = Some((serial, self.now_ms));
        self.damage_caption_panel(damage);
    }

    fn say_focused_window(&mut self, priority: Priority, damage: &mut Damage) {
        let Some(slot) = self.focus.filter(|&slot| self.is_visible(slot)) else {
            let mut line = LineText::new();
            let (count, shown, _) = self.desktops();
            let _ = core::fmt::Write::write_fmt(
                &mut line,
                format_args!("Desktop {} of {count}", shown + 1),
            );
            self.say(Source::Window, priority, line.as_str(), damage);
            return;
        };
        let window = self.window_at(self.owner_window(slot));
        let kind = match window.role {
            super::protocol::Role::Dialog => "dialog",
            _ => "window",
        };
        let mut line = LineText::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{}, {kind}", window.title()));
        if window.not_responding {
            let _ = core::fmt::Write::write_str(&mut line, ", not responding");
        }
        self.say(Source::Window, priority, line.as_str(), damage);
    }

    /// The focus moved to another window: the reader says the new one (D603).
    pub(super) fn reader_focus_moved(&mut self, damage: &mut Damage) {
        if self.reader.settings.on {
            self.reader.last_container = 0;
            self.say_focused_window(Priority::Interrupt, damage);
        }
    }

    /// An app reported its focused element (`REPORT_ELEMENT`): `line` is its line and
    /// `container` its container's id with the container's line, said first when it differs
    /// from the last one's. `focus` says whether the focus moved (else a value changed).
    pub fn reader_element(
        &mut self,
        line: &str,
        container: Option<(u32, &str)>,
        focus: bool,
        damage: &mut Damage,
    ) {
        if !focus {
            self.say(Source::Change, Priority::Queue, line, damage);
            return;
        }
        if let Some((id, container_line)) = container
            && id != self.reader.last_container
        {
            self.reader.last_container = id;
            self.say(Source::Container, Priority::Interrupt, container_line, damage);
            self.say(Source::Focus, Priority::Queue, line, damage);
            return;
        }
        self.say(Source::Focus, Priority::Interrupt, line, damage);
    }

    /// A character typed into the focused field, said while echo is on.
    pub fn reader_echo(&mut self, character: char, damage: &mut Damage) {
        if !self.reader.settings.echo_characters {
            return;
        }
        let mut word = [0u8; 4];
        let said = match character {
            ' ' => "space",
            '\u{8}' => "backspace",
            '\n' | '\r' => "enter",
            other => other.encode_utf8(&mut word),
        };
        self.say(Source::Echo, Priority::Queue, said, damage);
    }

    /// Moves the panel to the work area's top while `rect` (the focused element's or the
    /// caret's, on the screen) would be under it, and back otherwise.
    pub fn keep_caption_clear_of(&mut self, rect: Rect, damage: &mut Damage) {
        let bottom = self.caption_panel_at(false);
        let at_top = bottom.is_some_and(|panel| panel.overlaps(&rect));
        if at_top != self.reader.at_top {
            self.damage_caption_panel(damage);
            self.reader.at_top = at_top;
            self.damage_caption_panel(damage);
        }
    }

    /// Shows the next queued line once the one shown has been up long enough, and hides the
    /// panel once `Screen reader off` has shown.
    pub(super) fn tick_screen_reader(&mut self, damage: &mut Damage) {
        if let Some(hide_at) = self.reader.hide_at
            && self.now_ms.wrapping_sub(hide_at) < u32::MAX / 2
        {
            self.damage_caption_panel(damage);
            self.reader.hide_at = None;
            self.reader.shown = None;
            self.reader.queued = 0;
            return;
        }
        if self.reader.queued == 0 {
            return;
        }
        let Some((serial, since)) = self.reader.shown else {
            return;
        };
        let glyphs = self
            .reader
            .line(serial)
            .map_or(0, |line| line.text().chars().count());
        if self.now_ms.wrapping_sub(since) < caption_ms(glyphs, self.reader.settings.rate) {
            return;
        }
        let next = self.reader.queue[0];
        self.reader.queue.copy_within(1..self.reader.queued, 0);
        self.reader.queued -= 1;
        self.show_caption(next, damage);
    }

    /// Writes the log's utterances after `since`, oldest first, as `READ_READER_LOG`'s records
    /// into `out`; the bytes written and the records.
    pub fn read_reader_log(&self, since: u32, out: &mut [u8]) -> (usize, u32) {
        let Some(log) = self.reader.log.as_ref() else {
            return (0, 0);
        };
        let newest = self.reader.logged;
        let oldest = newest.saturating_sub(READER_LOG_ENTRIES as u32 - 1).max(1);
        let mut written = 0;
        let mut records = 0;
        for serial in since.saturating_add(1).max(oldest)..=newest {
            let line = &log[serial as usize % READER_LOG_ENTRIES];
            if line.serial != serial {
                continue;
            }
            let record = UtteranceRecord {
                serial,
                at_ms: line.at_ms,
                source: line.source,
                priority: line.priority,
                text: line.text(),
            };
            match record.write(&mut out[written..]) {
                Some(length) => written += length,
                None => break,
            }
            records += 1;
        }
        (written, records)
    }

    /// How many utterances were logged since boot, and the last one's text.
    pub fn reader_logged(&self) -> (u32, &str) {
        let last = self
            .reader
            .line(self.reader.logged)
            .map_or("", |line| line.text());
        (self.reader.logged, last)
    }

    fn caption_panel_at(&self, top: bool) -> Option<Rect> {
        let area = self.work_area();
        let width = PANEL_MOST_WIDTH.min(area.width.checked_sub(2 * PANEL_MARGIN)?);
        let x = area.x + (area.width - width) / 2;
        let y = match top {
            true => area.y + PANEL_MARGIN,
            false => area.bottom().checked_sub(PANEL_MARGIN + PANEL_HEIGHT)?,
        };
        Some(Rect::new(x, y, width, PANEL_HEIGHT))
    }

    /// The caption panel's rectangle while it shows.
    pub fn caption_panel(&self) -> Option<Rect> {
        if !self.reader.settings.captions || self.reader.shown.is_none() {
            return None;
        }
        self.caption_panel_at(self.reader.at_top)
    }

    fn damage_caption_panel(&mut self, damage: &mut Damage) {
        if let Some(panel) = self.reader.panel.take() {
            damage.add(panel);
        }
        if let Some(panel) = self.caption_panel() {
            damage.add(panel);
            self.reader.panel = Some(panel);
        }
    }

    /// Draws the caption panel inside `canvas`'s clip: the current utterance, and the previous
    /// one dimmed under it. Drawn after the zoom's lens, never magnified.
    pub(super) fn draw_caption_panel(&self, canvas: &mut Canvas<'_>) {
        let Some(panel) = self.caption_panel() else {
            return;
        };
        if !panel.overlaps(&canvas.clip()) {
            return;
        }
        canvas.rounded(panel, &paint::INSET_R4, true, true, colour::MENU_BORDER);
        canvas.rounded(
            Rect::new(panel.x + 1, panel.y + 1, panel.width - 2, panel.height - 2),
            &paint::INSET_R4,
            true,
            true,
            colour::MENU,
        );
        let fits = ((panel.width - 2 * TEXT_X) / GLYPH_WIDTH as u32) as usize;
        let lines = [
            (self.reader.shown.map(|(serial, _)| serial), FIRST_LINE_Y, colour::TEXT),
            (Some(self.reader.previous), SECOND_LINE_Y, colour::TEXT_DIM),
        ];
        for (serial, y, ink) in lines {
            let Some(line) = serial.and_then(|serial| self.reader.line(serial)) else {
                continue;
            };
            let text = line.text();
            let x = panel.x + TEXT_X;
            match text.chars().count() > fits {
                false => canvas.text(x, panel.y + y, text, ink),
                true => {
                    let end = text
                        .char_indices()
                        .nth(fits.saturating_sub(1))
                        .map_or(text.len(), |(at, _)| at);
                    canvas.text(x, panel.y + y, &text[..end], ink);
                    let ellipsis_x = x + (fits.saturating_sub(1) * GLYPH_WIDTH) as u32;
                    canvas.text(ellipsis_x, panel.y + y, ELLIPSIS, ink);
                }
            }
        }
    }

    /// Draws the caption panel into `pixels` (rows `stride` apart) inside `clip`: the kernel
    /// calls it after the zoom's lens, so the panel is never magnified.
    pub fn draw_caption_panel_over(&self, pixels: &mut [u32], stride: usize, clip: Rect) {
        if let Some(mut canvas) = Canvas::new(pixels, stride, clip) {
            self.draw_caption_panel(&mut canvas);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caption_stays_half_a_second_and_sixty_ms_a_glyph_at_two_hundred_words() {
        assert_eq!(caption_ms("Save, button".len(), 200), 1220);
        assert_eq!(caption_ms(60, 200), 4100);
    }
}

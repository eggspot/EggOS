//! The server's own word on its clients (D222, docs/ux/desktop-polish-v0.3.md section 4):
//! `Working` while an app it started has no window yet, `Busy` and ` (not responding)` over
//! the windows of a client that leaves an event unread for [`UNREAD_EVENTS_BUSY_MS`], and the
//! End app dialog a press on such a window opens once it has not responded for
//! [`END_APP_OFFER_MS`].

use super::management::NOT_RESPONDING_AFTER_MS;
use super::protocol::{EventSink, MAX_CLIENTS, PointerShape, WindowError};
use super::windows::{self, KERNEL_CLIENT};
use super::{Action, Desktop, ListedWindow, colour, contains, paint, popups};
use crate::display::{Damage, Rect};
use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::input::key;

/// How long the pointer says `Working` after a launch at most, when the app shows no window.
pub const APP_STARTING_MS: u32 = 5000;
/// How long a client may leave an event unread before its windows are not responding.
pub const UNREAD_EVENTS_BUSY_MS: u32 = 2000;
/// How long a window has not responded before a press on its content offers to end its app.
pub const END_APP_OFFER_MS: u32 = 5000;
/// The End app dialog's size.
pub const END_APP_SIZE: (u32, u32) = (320, 120);
/// The title cells line 1 of the End app dialog keeps ([`windows::cut_title`]).
pub const END_APP_TITLE_CELLS: usize = 19;
/// What follows the title on line 1 of the End app dialog.
pub const END_APP_QUESTION: &str = " is not responding";
/// Line 2 of the End app dialog.
pub const END_APP_WARNING: &str = "Ending it loses its unsaved work.";
/// Line 3 of the End app dialog, when the client serves every Linux window app (eggway's one
/// connection for all Wayland apps): ending it ends them all.
pub const END_APP_LINUX_APPS_WARNING: &str = "It also ends every Linux window app.";
/// The dialog's button that leaves the window alone, focused when it opens.
pub const WAIT_LABEL: &str = "Wait";
/// The dialog's button that ends the app.
pub const END_APP_LABEL: &str = "End app";

const LINE_X: u32 = 12;
const FIRST_LINE_Y: u32 = 12;
const SECOND_LINE_Y: u32 = 36;
const LINE_STEP: u32 = SECOND_LINE_Y - FIRST_LINE_Y;
const BUTTON_SIZE: (u32, u32) = (96, 28);
const BUTTON_Y: u32 = 80;
const WAIT_X: u32 = 212;
const END_APP_X: u32 = 108;
const FOCUS_RING_INSET: u32 = 2;
const FOCUS_RING_WIDTH: u32 = 2;
const FOCUS_RING_INNER: [u32; 1] = [1];

/// The End app dialog's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndAppButton {
    Wait,
    EndApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EndAppDialog {
    pub window: usize,
    pub client: u16,
    pub focused: EndAppButton,
}

/// What the server knows of its clients' responsiveness.
#[derive(Debug, Clone, Copy)]
pub(super) struct Responsiveness {
    pub app_starting_since: Option<u32>,
    pub unread_since: [Option<(u16, u32)>; MAX_CLIENTS],
    pub dialog: Option<EndAppDialog>,
    pub linux_apps_client: Option<u16>,
}

impl Default for Responsiveness {
    fn default() -> Self {
        Self {
            app_starting_since: None,
            unread_since: [None; MAX_CLIENTS],
            dialog: None,
            linux_apps_client: None,
        }
    }
}

impl Responsiveness {
    fn unread_since(&self, client: u16) -> Option<u32> {
        self.unread_since[usize::from(client) & (MAX_CLIENTS - 1)]
            .filter(|&(known, _)| known == client)
            .map(|(_, since)| since)
    }
}

/// How long it is from `since` to `now_ms`; none when the clock is behind it.
pub fn waited(now_ms: u32, since: u32) -> u32 {
    (now_ms.wrapping_sub(since) as i32).max(0) as u32
}

impl Desktop {
    /// Client `client`'s oldest unread event was queued at `since` on the monotonic clock
    /// (milliseconds), or it has none (D222): its windows say `(not responding)` once it has
    /// waited [`UNREAD_EVENTS_BUSY_MS`], and stop saying it, the End app dialog closing, as
    /// soon as it reads.
    pub fn set_unread_since(&mut self, client: u16, since: Option<u32>, damage: &mut Damage) {
        let place = &mut self.responsiveness.unread_since[usize::from(client) & (MAX_CLIENTS - 1)];
        let new = since.map(|since| (client, since));
        if *place == new {
            return;
        }
        *place = new;
        self.refresh_responding(damage);
    }

    /// Forgets client `client`'s unread clock, and that it serves every Linux window app, as
    /// when it disconnects.
    pub fn forget_client(&mut self, client: u16, damage: &mut Damage) {
        if self.responsiveness.linux_apps_client == Some(client) {
            self.responsiveness.linux_apps_client = None;
        }
        self.set_unread_since(client, None, damage);
    }

    /// Client `client` serves every Linux window app through its one connection (eggway): the
    /// End app dialog for its windows says ending it ends them all.
    pub fn set_linux_apps_client(&mut self, client: u16) {
        self.responsiveness.linux_apps_client = Some(client);
    }

    /// Whether any window of client `client`'s says `(not responding)`.
    pub fn client_not_responding(&self, client: u16) -> bool {
        self.windows
            .iter()
            .flatten()
            .any(|window| window.client == client && window.not_responding)
    }

    /// Refuses (`NotResponding`) what an agent asks of window `window` while it does not
    /// respond: an action on an element, typed text or a main menu (D222).
    pub fn refuse_if_not_responding(&self, window: u32) -> Result<(), WindowError> {
        let busy = super::WindowId::try_from(window)
            .ok()
            .and_then(|id| self.find(id))
            .is_some_and(|slot| self.window_at(slot).not_responding);
        match busy {
            true => Err(WindowError::NotResponding),
            false => Ok(()),
        }
    }

    /// Window `window` as `wm list` shows it, while it does not respond: what `wm show`
    /// answers at once instead of asking its app (D222).
    pub fn not_responding_window(&self, window: u32) -> Option<ListedWindow> {
        let id = super::WindowId::try_from(window).ok()?;
        let slot = self.find(id)?;
        let window = self.window_at(slot);
        (window.shown && window.not_responding).then(|| self.listed(slot))
    }

    pub(super) fn is_unresponsive(&self, slot: usize) -> bool {
        let window = self.window_at(slot);
        let closing = window
            .close_requested_ms
            .is_some_and(|asked| waited(self.now_ms, asked) >= NOT_RESPONDING_AFTER_MS);
        let unread = window.client != KERNEL_CLIENT
            && self
                .responsiveness
                .unread_since(window.client)
                .is_some_and(|since| waited(self.now_ms, since) >= UNREAD_EVENTS_BUSY_MS);
        closing || unread
    }

    /// Brings every window's `(not responding)` up to date with the clocks: its title bar and
    /// the pointer over it change with it, and the End app dialog closes when its window
    /// answers again. Returns whether any changed.
    pub(super) fn refresh_responding(&mut self, damage: &mut Damage) -> bool {
        let mut changed = false;
        for slot in 0..self.windows.len() {
            if self.windows[slot].is_none() {
                continue;
            }
            let wanted = self.is_unresponsive(slot);
            if wanted == self.window_at(slot).not_responding {
                continue;
            }
            self.window_mut(slot).not_responding = wanted;
            changed = true;
            if self.is_visible(slot) {
                damage.add(self.window_at(slot).title_bar());
            }
        }
        if let Some(dialog) = self.responsiveness.dialog
            && !self.window_at(dialog.window).not_responding
        {
            self.close_end_app(damage);
        }
        if changed {
            self.update_pointer(damage);
        }
        changed
    }

    fn not_responding_for(&self, slot: usize) -> Option<u32> {
        let window = self.window_at(slot);
        if !window.not_responding || window.client == KERNEL_CLIENT {
            return None;
        }
        let since = self.responsiveness.unread_since(window.client)?;
        waited(self.now_ms, since).checked_sub(UNREAD_EVENTS_BUSY_MS)
    }

    /// Whether a press on window `slot`'s content opens the End app dialog instead of
    /// reaching its client.
    pub(super) fn offers_end_app(&self, slot: usize) -> bool {
        self.not_responding_for(slot)
            .is_some_and(|busy| busy >= END_APP_OFFER_MS)
    }

    /// An app was started: the pointer says `Working` until it shows a window, it ends or
    /// [`APP_STARTING_MS`] pass. Another start restarts the wait.
    pub(super) fn app_starting(&mut self, damage: &mut Damage) {
        self.responsiveness.app_starting_since = Some(self.now_ms);
        self.update_pointer(damage);
    }

    /// The program the last launch started ended before it showed a window: the pointer stops
    /// saying `Working` (D222).
    pub fn started_app_ended(&mut self, damage: &mut Damage) {
        self.end_app_starting(damage);
    }

    pub(super) fn end_app_starting(&mut self, damage: &mut Damage) {
        if self.responsiveness.app_starting_since.take().is_some() {
            self.update_pointer(damage);
        }
    }

    /// Whether an app the desktop started has no window yet, within [`APP_STARTING_MS`].
    pub fn is_app_starting(&self) -> bool {
        self.responsiveness
            .app_starting_since
            .is_some_and(|since| waited(self.now_ms, since) < APP_STARTING_MS)
    }

    /// `shape` with the server's own `Working` (D222): it takes the arrow's place while an app
    /// starts; every other shape says something more specific and stays.
    pub(super) fn with_server_shapes(&self, shape: PointerShape) -> PointerShape {
        match shape == PointerShape::Arrow && self.is_app_starting() {
            true => PointerShape::Working,
            false => shape,
        }
    }

    /// The clocks reached `now_ms`: the start's wait may be over, a client may have become
    /// not responding.
    pub(super) fn tick_responsiveness(&mut self, damage: &mut Damage) {
        if self
            .responsiveness
            .app_starting_since
            .is_some_and(|since| waited(self.now_ms, since) >= APP_STARTING_MS)
        {
            self.end_app_starting(damage);
        }
        self.refresh_responding(damage);
    }

    /// The next time on the monotonic clock (milliseconds) the desktop changes by itself: a
    /// client becoming not responding, a start's wait ending, the launcher's caret blinking;
    /// `None` when nothing is waiting, so no timer need run (D24).
    pub fn next_change_ms(&self) -> Option<u32> {
        let now = self.now_ms;
        let mut next: Option<u32> = None;
        let mut consider = |after: u32| {
            next = Some(next.map_or(after, |earlier| earlier.min(after)));
        };
        if let Some(since) = self.responsiveness.app_starting_since {
            consider(APP_STARTING_MS.saturating_sub(waited(now, since)));
        }
        for slot in 0..self.windows.len() {
            let Some(window) = self.windows[slot] else {
                continue;
            };
            if window.not_responding || window.client == KERNEL_CLIENT {
                continue;
            }
            if let Some(since) = self.responsiveness.unread_since(window.client) {
                consider(UNREAD_EVENTS_BUSY_MS.saturating_sub(waited(now, since)));
            }
        }
        if let Some(after) = self.caret_change_after() {
            consider(after);
        }
        next.map(|after| now.wrapping_add(after.max(1)))
    }

    /// The End app dialog's rectangle, its border included, while it is open.
    pub fn end_app_area(&self) -> Option<Rect> {
        let dialog = self.responsiveness.dialog?;
        Some(self.end_app_rect(dialog.window))
    }

    /// The End app dialog's lines under its first, while it is open: the unsaved work, and for
    /// the client serving every Linux window app that ending it ends them all.
    pub fn end_app_warnings(&self) -> Option<&'static [&'static str]> {
        let dialog = self.responsiveness.dialog?;
        let warnings: &'static [&'static str] =
            match self.responsiveness.linux_apps_client == Some(dialog.client) {
                true => &[END_APP_WARNING, END_APP_LINUX_APPS_WARNING],
                false => &[END_APP_WARNING],
            };
        Some(warnings)
    }

    /// The End app dialog's focused button, while it is open.
    pub fn end_app_focus(&self) -> Option<EndAppButton> {
        self.responsiveness.dialog.map(|dialog| dialog.focused)
    }

    /// The hit target of the End app dialog's `button`, while it is open.
    pub fn end_app_button(&self, button: EndAppButton) -> Option<Rect> {
        let rect = self.end_app_area()?;
        Some(Self::button_rect(rect, button))
    }

    fn button_rect(rect: Rect, button: EndAppButton) -> Rect {
        let x = match button {
            EndAppButton::Wait => WAIT_X,
            EndAppButton::EndApp => END_APP_X,
        };
        Rect::new(rect.x + x, rect.y + BUTTON_Y, BUTTON_SIZE.0, BUTTON_SIZE.1)
    }

    fn end_app_rect(&self, slot: usize) -> Rect {
        let work = self.layout.work_area;
        let shown = self
            .window_at(slot)
            .outside()
            .clipped(self.width, self.height);
        let (width, height) = (
            END_APP_SIZE.0.min(work.width),
            END_APP_SIZE.1.min(work.height),
        );
        let centre = |start: u32, size: u32, room_start: u32, room: u32, wanted: u32| {
            let at = i64::from(start) + i64::from(size) / 2 - i64::from(wanted) / 2;
            let last = i64::from(room_start) + i64::from(room.saturating_sub(wanted));
            at.max(i64::from(room_start)).min(last) as u32
        };
        Rect::new(
            centre(shown.x, shown.width, work.x, work.width, width),
            centre(shown.y, shown.height, work.y, work.height, height),
            width,
            height,
        )
    }

    pub(super) fn end_app_overlaps(&self, clip: Rect) -> bool {
        self.end_app_area()
            .is_some_and(|rect| popups::with_shadow(rect).overlaps(&clip))
    }

    /// Opens the End app dialog for not-responding window `slot`.
    pub(super) fn open_end_app(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        self.close_popups(damage, events);
        self.responsiveness.dialog = Some(EndAppDialog {
            window: slot,
            client: self.window_at(slot).client,
            focused: EndAppButton::Wait,
        });
        if let Some(rect) = self.end_app_area() {
            damage.add(popups::with_shadow(rect));
        }
    }

    pub(super) fn close_end_app(&mut self, damage: &mut Damage) {
        if let Some(rect) = self.end_app_area() {
            damage.add(popups::with_shadow(rect));
        }
        self.responsiveness.dialog = None;
    }

    /// Window `slot` is going away: the End app dialog for it closes.
    pub(super) fn forget_end_app(&mut self, slot: usize, damage: &mut Damage) {
        if self
            .responsiveness
            .dialog
            .is_some_and(|dialog| dialog.window == slot)
        {
            self.close_end_app(damage);
        }
    }

    fn end_app(&mut self, damage: &mut Damage, events: &mut dyn EventSink) -> Action {
        let Some(dialog) = self.responsiveness.dialog else {
            return Action::None;
        };
        self.close_end_app(damage);
        self.destroy_client(dialog.client, damage, events);
        Action::EndClient(dialog.client)
    }

    /// A button press at (`x`, `y`) while the End app dialog is open: its buttons, or outside
    /// it, which closes it (as `Wait` does) and goes nowhere else. `None` when it is closed.
    pub(super) fn end_app_press(
        &mut self,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let rect = self.end_app_area()?;
        if contains(Self::button_rect(rect, EndAppButton::EndApp), x, y) {
            return Some(self.end_app(damage, events));
        }
        if contains(rect, x, y) && !contains(Self::button_rect(rect, EndAppButton::Wait), x, y) {
            return Some(Action::None);
        }
        self.close_end_app(damage);
        Some(Action::None)
    }

    /// Key `code` while the End app dialog is open: Enter presses the focused button, Esc
    /// waits, Tab and the arrows move the focus. `None` when it is closed.
    pub(super) fn end_app_key(
        &mut self,
        code: u16,
        pressed: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Option<Action> {
        let mut dialog = self.responsiveness.dialog?;
        if !pressed {
            return Some(Action::None);
        }
        match code {
            key::ESC => self.close_end_app(damage),
            key::ENTER | key::KEYPAD_ENTER => match dialog.focused {
                EndAppButton::Wait => self.close_end_app(damage),
                EndAppButton::EndApp => return Some(self.end_app(damage, events)),
            },
            key::TAB | key::LEFT | key::RIGHT => {
                dialog.focused = match dialog.focused {
                    EndAppButton::Wait => EndAppButton::EndApp,
                    EndAppButton::EndApp => EndAppButton::Wait,
                };
                self.responsiveness.dialog = Some(dialog);
                if let Some(rect) = self.end_app_area() {
                    damage.add(rect);
                }
            }
            _ => {}
        }
        Some(Action::None)
    }

    pub(super) fn draw_end_app(&self, canvas: &mut paint::Canvas<'_>) {
        let Some(dialog) = self.responsiveness.dialog else {
            return;
        };
        let rect = self.end_app_rect(dialog.window);
        Self::draw_popup_surface(canvas, rect, colour::DOCK);
        let (title, cut) =
            windows::cut_title(self.window_at(dialog.window).title(), END_APP_TITLE_CELLS);
        let mut x = rect.x + LINE_X;
        let ellipsis = if cut { windows::ellipsis() } else { "" };
        for part in [title, ellipsis, END_APP_QUESTION] {
            canvas.text(x, rect.y + FIRST_LINE_Y, part, colour::TEXT);
            x += part.chars().count() as u32 * GLYPH_WIDTH as u32;
        }
        let mut y = rect.y + SECOND_LINE_Y;
        for warning in self.end_app_warnings().unwrap_or_default() {
            canvas.text(rect.x + LINE_X, y, warning, colour::TEXT_DIM);
            y += LINE_STEP;
        }
        for (button, label) in [
            (EndAppButton::Wait, WAIT_LABEL),
            (EndAppButton::EndApp, END_APP_LABEL),
        ] {
            let area = Self::button_rect(rect, button);
            if dialog.focused == button {
                let ring = Rect::new(
                    area.x - FOCUS_RING_INSET,
                    area.y - FOCUS_RING_INSET,
                    area.width + 2 * FOCUS_RING_INSET,
                    area.height + 2 * FOCUS_RING_INSET,
                );
                canvas.rounded(ring, &paint::INSET_R4, true, true, colour::FOCUS_RING);
                canvas.rounded(
                    Rect::new(
                        ring.x + FOCUS_RING_WIDTH,
                        ring.y + FOCUS_RING_WIDTH,
                        ring.width - 2 * FOCUS_RING_WIDTH,
                        ring.height - 2 * FOCUS_RING_WIDTH,
                    ),
                    &FOCUS_RING_INNER,
                    true,
                    true,
                    colour::DOCK,
                );
            }
            let (fill, ink) = match button {
                EndAppButton::Wait => (colour::ACCENT, colour::ICON_DARK),
                EndAppButton::EndApp => (colour::MENU_BORDER, colour::TEXT),
            };
            canvas.rounded(area, &paint::INSET_R4, true, true, colour::BUTTON_BORDER);
            canvas.rounded(
                Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2),
                &paint::INSET_INSIDE_R4,
                true,
                true,
                fill,
            );
            let width = label.len() as u32 * GLYPH_WIDTH as u32;
            canvas.text(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + (area.height - GLYPH_HEIGHT as u32) / 2,
                label,
                ink,
            );
        }
    }
}

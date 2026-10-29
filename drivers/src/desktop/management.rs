//! Window management (docs/ux/desktop-v0.3-gui.md, sections 3.5, 3.6 and 5, item 31): edge
//! resizing, maximising, restoring and snapping, the server's shortcuts, the 2 s rule for a
//! close request nobody answers, show desktop, and which pointer shape is drawn where.

use super::pointer;
use super::popups::{Switcher, WindowMenu};
use super::protocol::{
    EventRecord, EventSink, PointerShape, Role, TITLE_CAPACITY, WindowAction, WindowError,
    WindowState, event, move_or_resize,
};
use super::shortcuts::{Shortcut, shortcut};
use super::windows::{self, Geometry, Part, edge, resize_pointer};
use super::{Desktop, contains};
use crate::display::{Damage, Rect};
use crate::input::keymap::Modifiers;

/// How long a close request may go unread before the server closes the window itself.
pub const CLOSE_TIMEOUT_MS: u32 = 2000;
/// How long a close request goes unread before the title bar says `(not responding)`.
pub const NOT_RESPONDING_AFTER_MS: u32 = 500;
/// How far the window menu's keyboard move and size go per arrow key.
pub const KEYBOARD_STEP: u32 = 8;

const LEFT_BUTTON_BIT: u8 = 1;

fn valid_edges(edges: u32) -> bool {
    let all = u32::from(edge::LEFT | edge::RIGHT | edge::TOP | edge::BOTTOM);
    let both = |first: u8, second: u8| {
        let pair = u32::from(first | second);
        edges & pair == pair
    };
    edges != 0
        && edges & !all == 0
        && !both(edge::LEFT, edge::RIGHT)
        && !both(edge::TOP, edge::BOTTOM)
}

/// One window as `wm list` shows it ([`Desktop::listed_window`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListedWindow {
    /// Its id.
    pub id: super::WindowId,
    /// Normal, minimised or maximised.
    pub state: WindowState,
    /// Its outside rectangle on the screen.
    pub rect: Rect,
    /// Its client's number, [`super::KERNEL_CLIENT`] for the kernel's.
    pub client: u16,
    /// Its title, [`Self::title_length`] bytes of UTF-8.
    pub title: [u8; TITLE_CAPACITY],
    /// See [`Self::title`].
    pub title_length: usize,
}

/// What a point is on ([`Desktop::hit`]): a window's resize zone, or a window and its part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Hit {
    pub resize: Option<(usize, u8)>,
    pub window: Option<(usize, Part)>,
    pub content: Option<usize>,
}

/// A resize the user started on a window's edge or corner: the window, its edges, its outside
/// rectangle when it started and where the pointer was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Resize {
    pub window: usize,
    pub edges: u8,
    pub start: Rect,
    pub from: (u32, u32),
}

impl Desktop {
    /// The time on the monotonic clock, in milliseconds, that the next events happen at:
    /// double-clicks and close requests are timed by it.
    pub fn set_time(&mut self, now_ms: u32) {
        self.now_ms = now_ms;
    }

    /// The window at `index` among the shown windows (minimised ones included), in the order
    /// they were created; [`WindowError::NoSuchWindow`] past the last.
    pub fn listed_window(&self, index: usize) -> Result<ListedWindow, WindowError> {
        let slot = self.created[..self.created_length]
            .iter()
            .map(|&slot| usize::from(slot))
            .filter(|&slot| self.window_at(slot).shown)
            .nth(index)
            .ok_or(WindowError::NoSuchWindow)?;
        let window = self.window_at(slot);
        let state = match (window.minimised, window.maximised) {
            (true, _) => WindowState::Minimised,
            (false, true) => WindowState::Maximised,
            (false, false) => WindowState::Normal,
        };
        Ok(ListedWindow {
            id: window.id,
            state,
            rect: window.outside(),
            client: window.client,
            title: window.title,
            title_length: usize::from(window.title_length),
        })
    }

    /// Does `action` ([`WindowAction`]'s numbers) to window `id`, whoever's it is, as `wm`
    /// asks: with (`a`, `b`) the position of `Move` or the content size of `Size`.
    pub fn manage(
        &mut self,
        id: u32,
        action: u32,
        a: i32,
        b: i32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let id = super::WindowId::try_from(id).map_err(|_| WindowError::Invalid)?;
        let slot = self
            .find(id)
            .filter(|&slot| self.window_at(slot).shown)
            .ok_or(WindowError::Invalid)?;
        let action = WindowAction::from_value(action).ok_or(WindowError::Invalid)?;
        let window = *self.window_at(slot);
        match action {
            WindowAction::Focus => self.activate(slot, damage, events),
            WindowAction::Minimise if window.role != Role::Normal => {
                return Err(WindowError::Invalid);
            }
            WindowAction::Minimise => self.minimise(slot, damage, events),
            WindowAction::Maximise if !window.can_maximise() => return Err(WindowError::Invalid),
            WindowAction::Maximise => {
                self.restore(slot, damage, events);
                self.maximise(slot, damage, events);
            }
            WindowAction::Restore => {
                self.restore(slot, damage, events);
                self.unmaximise(slot, damage, events);
            }
            WindowAction::Close if !window.closable => return Err(WindowError::NotAllowed),
            WindowAction::Close => self.request_close(slot, events),
            WindowAction::Move => {
                self.unmaximise(slot, damage, events);
                self.move_window(slot, i64::from(a), i64::from(b), damage);
            }
            WindowAction::Size if window.role != Role::Normal || window.is_fixed_size() => {
                return Err(WindowError::Invalid);
            }
            WindowAction::Size => {
                self.unmaximise(slot, damage, events);
                let window = *self.window_at(slot);
                let outside = window.outside();
                let wanted = |value: i32, chrome: u32| i64::from(value.max(0)) + i64::from(chrome);
                let outside = windows::resized(
                    outside,
                    edge::RIGHT | edge::BOTTOM,
                    wanted(a, windows::CHROME.0) - i64::from(outside.width),
                    wanted(b, windows::CHROME.1) - i64::from(outside.height),
                    self.minimum_outside_of(slot),
                    self.work_area(),
                );
                let geometry = Geometry {
                    x: outside.x,
                    y: outside.y,
                    content_width: outside.width - windows::CHROME.0,
                    content_height: outside.height - windows::CHROME.1,
                };
                self.apply_geometry(slot, geometry, window.with_shadow(), damage, events);
            }
        }
        self.update_pointer(damage);
        Ok(())
    }

    /// Makes `profile` the desktop's layout profile: the panels, the work area, the side of
    /// the controls and the shortcuts. The shell's popups close; maximised windows fill the new
    /// work area, resizable windows larger than it shrink to fit it (their clients get
    /// `Resize`), and every window is moved into it; everything is drawn again.
    pub fn set_profile(
        &mut self,
        profile: super::Profile,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        if self.profile == profile {
            return;
        }
        self.close_popups(damage, events);
        self.close_notice(damage);
        self.profile = profile;
        self.layout = super::layout::layout(self.width, self.height, 1, profile);
        self.control_hover = None;
        self.control_pressed = None;
        self.reset_panels();
        self.icons.last_press = None;
        let work = self.work_area();
        for slot in 0..self.windows.len() {
            let Some(window) = self.windows[slot] else {
                continue;
            };
            if !window.shown {
                continue;
            }
            if window.maximised {
                self.apply_geometry(
                    slot,
                    windows::maximised(work),
                    window.with_shadow(),
                    damage,
                    events,
                );
                continue;
            }
            if let Some(before) = window.before_maximised {
                let (x, y) = self.clamped(slot, i64::from(before.x), i64::from(before.y));
                self.window_mut(slot).before_maximised = Some(Geometry { x, y, ..before });
            }
            let outside = window.outside();
            let too_large = outside.width > work.width || outside.height > work.height;
            if too_large && window.is_resizable() {
                let geometry = Geometry {
                    x: window.x,
                    y: window.y,
                    content_width: outside.width.min(work.width) - windows::CHROME.0,
                    content_height: outside.height.min(work.height) - windows::CHROME.1,
                };
                self.apply_geometry(slot, geometry, window.with_shadow(), damage, events);
            }
            self.move_window(slot, i64::from(window.x), i64::from(window.y), damage);
        }
        damage.add(self.screen());
        self.update_pointer(damage);
    }

    /// The layout profile.
    pub fn profile(&self) -> super::Profile {
        self.profile
    }

    /// Whether the focused window takes the Ctrl keys the shell's shortcuts would take.
    pub fn focused_takes_ctrl_keys(&self) -> bool {
        self.focus
            .is_some_and(|slot| self.window_at(slot).takes_ctrl_keys())
    }

    /// Whether key `code` pressed with `modifiers` is a shortcut the server takes now.
    pub fn takes_key(&self, code: u16, modifiers: Modifiers) -> bool {
        shortcut(
            code,
            modifiers,
            self.profile,
            self.focused_takes_ctrl_keys(),
        )
        .is_some()
    }

    /// Whether window `id` is maximised.
    pub fn is_maximised(&self, id: super::WindowId) -> bool {
        self.find(id)
            .is_some_and(|slot| self.window_at(slot).maximised)
    }

    /// Whether window `id` is minimised.
    pub fn is_minimised(&self, id: super::WindowId) -> bool {
        self.find(id)
            .is_some_and(|slot| self.window_at(slot).minimised)
    }

    /// Whether window `id`'s title bar says `(not responding)`.
    pub fn is_not_responding(&self, id: super::WindowId) -> bool {
        self.find(id)
            .is_some_and(|slot| self.window_at(slot).not_responding)
    }

    /// The pointer as drawn: its hotspot and shape, `None` until the pointer first moves.
    pub fn pointer(&self) -> Option<(u32, u32, PointerShape)> {
        self.pointer_drawn
    }

    pub(super) fn minimum_outside_of(&self, slot: usize) -> (u32, u32) {
        windows::minimum_outside(self.window_at(slot).minimum, self.work_area())
    }

    /// Moves window `slot` to `geometry`, damaging where it was (`old`, shadow included) and
    /// where it is; a new content size is sent to its client as `Resize`.
    pub(super) fn apply_geometry(
        &mut self,
        slot: usize,
        geometry: Geometry,
        old: Rect,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let visible = self.is_visible(slot);
        if visible {
            damage.add(old);
        }
        let window = self.window_mut(slot);
        let resized = (window.content_width, window.content_height)
            != (geometry.content_width, geometry.content_height);
        window.x = geometry.x;
        window.y = geometry.y;
        window.content_width = geometry.content_width;
        window.content_height = geometry.content_height;
        if resized {
            window.pending_resize = Some((geometry.content_width, geometry.content_height));
        }
        if visible {
            damage.add(self.window_at(slot).with_shadow());
        }
        if resized {
            self.send(
                slot,
                EventRecord::new(
                    event::RESIZE,
                    0,
                    geometry.content_width as i32,
                    geometry.content_height as i32,
                    0,
                    0,
                ),
                events,
            );
        }
    }

    pub(super) fn send_state(&self, slot: usize, events: &mut dyn EventSink) {
        let window = self.window_at(slot);
        let state = match (window.minimised, window.maximised) {
            (true, _) => WindowState::Minimised,
            (false, true) => WindowState::Maximised,
            (false, false) => WindowState::Normal,
        };
        self.send(
            slot,
            EventRecord::new(event::STATE_CHANGED, 0, state.value() as i32, 0, 0, 0),
            events,
        );
    }

    /// Maximises window `slot`: it fills the work area, with square corners and no shadow,
    /// and remembers where it was.
    pub(super) fn maximise(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let window = *self.window_at(slot);
        if !window.can_maximise() || window.maximised {
            return;
        }
        let old = window.with_shadow();
        let target = windows::maximised(self.work_area());
        let maximised = self.window_mut(slot);
        maximised.before_maximised = Some(window.geometry());
        maximised.maximised = true;
        maximised.square_corners = false;
        self.end_resize_of(slot);
        self.apply_geometry(slot, target, old, damage, events);
        self.send_state(slot, events);
    }

    /// Restores maximised window `slot` to where it was before.
    pub(super) fn unmaximise(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let window = *self.window_at(slot);
        if !window.maximised {
            return;
        }
        let old = window.with_shadow();
        let before = window.before_maximised.unwrap_or(window.geometry());
        let restored = self.window_mut(slot);
        restored.maximised = false;
        restored.before_maximised = None;
        self.apply_geometry(slot, before, old, damage, events);
        let (x, y) = (self.window_at(slot).x, self.window_at(slot).y);
        self.move_window(slot, i64::from(x), i64::from(y), damage);
        self.send_state(slot, events);
    }

    /// Maximises window `slot`, or restores it if it is maximised.
    pub(super) fn toggle_maximise(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        match self.window_at(slot).maximised {
            true => self.unmaximise(slot, damage, events),
            false => self.maximise(slot, damage, events),
        }
    }

    /// Sets window `slot` to the left or right half of the work area.
    pub(super) fn snap(
        &mut self,
        slot: usize,
        right: bool,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let window = *self.window_at(slot);
        if !window.can_maximise() {
            return;
        }
        let old = window.with_shadow();
        let target = windows::snapped(self.work_area(), right, self.minimum_outside_of(slot));
        let was_maximised = window.maximised;
        let snapped = self.window_mut(slot);
        snapped.maximised = false;
        snapped.before_maximised = None;
        snapped.square_corners = true;
        self.apply_geometry(slot, target, old, damage, events);
        if was_maximised {
            self.send_state(slot, events);
        }
    }

    /// What is at (`x`, `y`), in one walk down the stack: the resize zone of the top-most
    /// window whose zone or body is there, or that window and the part of it the point is on;
    /// nothing over the bar or an open popup.
    pub(super) fn hit(&self, x: u32, y: u32) -> Hit {
        if self.over_panel(x, y) || self.popup_at(x, y) {
            return Hit::default();
        }
        for &slot in self.stack[..self.stack_length].iter().rev() {
            let slot = usize::from(slot);
            if !self.is_visible(slot) {
                continue;
            }
            let window = self.window_at(slot);
            let edges = window.resize_edges(x, y);
            if edges != 0 && self.top_dialog(slot).is_none() {
                return Hit {
                    resize: Some((slot, edges)),
                    window: None,
                    content: None,
                };
            }
            if contains(window.outside(), x, y) {
                let part = window.part_at(x, y, self.profile.control_side());
                let content = (part == Some(Part::Content) && self.top_dialog(slot).is_none())
                    .then_some(slot);
                return Hit {
                    resize: None,
                    window: part.map(|part| (slot, part)),
                    content,
                };
            }
        }
        Hit::default()
    }

    /// The resize zone of the top-most window there at (`x`, `y`): its slot and edges.
    pub(super) fn resize_target(&self, x: u32, y: u32) -> Option<(usize, u8)> {
        self.hit(x, y).resize
    }

    /// The window whose content is under `hit`, unless a dialog of it takes its input.
    pub(super) fn content_under(&self, hit: Hit) -> Option<usize> {
        hit.content
    }

    pub(super) fn start_resize(&mut self, slot: usize, edges: u8) {
        self.resize = Some(Resize {
            window: slot,
            edges,
            start: self.window_at(slot).outside(),
            from: self.pointer,
        });
    }

    /// `client` asks that the press its window `id` holds start moving the window (`kind`
    /// [`move_or_resize::MOVE`], `edges` 0) or resizing it by `edges` ([`move_or_resize::RESIZE`]),
    /// as a press on its title bar or edges would; the release that ends it reaches no client.
    /// Refused (`Invalid`) unless the client owns the normal window or dialog, the window holds
    /// the content press with the left button down, the kind is known and the edges fit it.
    pub fn start_move_or_resize(
        &mut self,
        client: u16,
        id: u32,
        kind: u32,
        edges: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Result<(), WindowError> {
        let slot = self.owned(client, id)?;
        let window = *self.window_at(slot);
        let held = self.grab == Some(slot) && self.buttons & LEFT_BUTTON_BIT != 0;
        if !matches!(window.role, Role::Normal | Role::Dialog) || !held {
            return Err(WindowError::Invalid);
        }
        match kind {
            move_or_resize::MOVE if edges == 0 => {
                let (x, y) = self.pointer;
                self.drag = Some(super::Drag {
                    window: slot,
                    from_pointer_x: i64::from(x) - i64::from(window.x),
                    from_pointer_y: i64::from(y) - i64::from(window.y),
                    pressed_at: (x, y),
                });
            }
            move_or_resize::RESIZE if window.is_resizable() && valid_edges(edges) => {
                self.start_resize(slot, edges as u8);
            }
            _ => return Err(WindowError::Invalid),
        }
        self.grab = None;
        let (x, y) = self.pointer;
        self.pointer_moved(x, y, damage, events);
        Ok(())
    }

    pub(super) fn end_resize_of(&mut self, slot: usize) {
        if self.resize.is_some_and(|resize| resize.window == slot) {
            self.resize = None;
        }
    }

    /// The pointer moved to (`x`, `y`) during `resize`: the window follows it.
    pub(super) fn resize_to(
        &mut self,
        resize: Resize,
        x: u32,
        y: u32,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let slot = resize.window;
        let outside = windows::resized(
            resize.start,
            resize.edges,
            i64::from(x) - i64::from(resize.from.0),
            i64::from(y) - i64::from(resize.from.1),
            self.minimum_outside_of(slot),
            self.work_area(),
        );
        let window = *self.window_at(slot);
        let geometry = Geometry {
            x: outside.x,
            y: outside.y,
            content_width: outside.width - windows::CHROME.0,
            content_height: outside.height - windows::CHROME.1,
        };
        if geometry == window.geometry() {
            return;
        }
        self.window_mut(slot).square_corners = false;
        self.apply_geometry(slot, geometry, window.with_shadow(), damage, events);
    }

    /// Sends window `slot`'s client a close request, as the close control and the close keys
    /// do, and starts the 2 s rule's clock if it is not running.
    pub(super) fn request_close(&mut self, slot: usize, events: &mut dyn EventSink) {
        if !self.window_at(slot).closable {
            return;
        }
        self.send(
            slot,
            EventRecord::new(event::CLOSE_REQUEST, 0, 0, 0, 0, 0),
            events,
        );
        let now = self.now_ms;
        let window = self.window_mut(slot);
        if window.close_requested_ms.is_none() {
            window.close_requested_ms = Some(now);
        }
    }

    /// Client `client` read a close request: its windows are answering.
    pub fn close_answered(&mut self, client: u16, damage: &mut Damage) {
        for slot in 0..self.windows.len() {
            let Some(window) = self.windows[slot].as_mut() else {
                continue;
            };
            if window.client != client || window.close_requested_ms.is_none() {
                continue;
            }
            window.close_requested_ms = None;
            if core::mem::take(&mut window.not_responding) && self.is_visible(slot) {
                damage.add(self.window_at(slot).title_bar());
            }
        }
    }

    /// The clock reached `now_ms`: windows whose close request went unread for
    /// [`NOT_RESPONDING_AFTER_MS`] say `(not responding)`, and those unread for
    /// [`CLOSE_TIMEOUT_MS`] are destroyed (their programs keep running). A clock behind the
    /// request counts as no time waited. Returns whether any was destroyed.
    pub fn tick(&mut self, now_ms: u32, damage: &mut Damage, events: &mut dyn EventSink) -> bool {
        self.now_ms = now_ms;
        let mut destroyed = false;
        for slot in 0..self.windows.len() {
            let Some(window) = self.windows[slot] else {
                continue;
            };
            let Some(asked) = window.close_requested_ms else {
                continue;
            };
            let waited = (now_ms.wrapping_sub(asked) as i32).max(0) as u32;
            if waited >= CLOSE_TIMEOUT_MS {
                self.destroy_slot(slot, damage, events);
                destroyed = true;
            } else if waited >= NOT_RESPONDING_AFTER_MS && !window.not_responding {
                self.window_mut(slot).not_responding = true;
                if self.is_visible(slot) {
                    damage.add(window.title_bar());
                }
            }
        }
        if destroyed {
            self.update_pointer(damage);
        }
        self.tick_notice(damage);
        self.tick_tooltip(damage);
        self.tick_agent(damage);
        destroyed
    }

    /// Focuses and raises window `slot`, restoring it first if it is minimised.
    pub(super) fn activate(
        &mut self,
        slot: usize,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        if self.window_at(slot).minimised {
            self.restore(slot, damage, events);
            return;
        }
        self.raise(slot, damage);
        let focus = self.top_dialog(slot).unwrap_or(slot);
        self.set_focus(Some(focus), damage, events);
    }

    /// The normal windows the switcher lists (shown or minimised), most recently focused
    /// first.
    pub(super) fn switcher_order(&self, order: &mut [u8]) -> usize {
        let mut length = 0;
        for &slot in &self.created[..self.created_length] {
            let window = self.window_at(usize::from(slot));
            if window.role != Role::Normal || !window.shown || length == order.len() {
                continue;
            }
            let serial = window.focus_serial;
            let mut at = length;
            while at > 0 && self.window_at(usize::from(order[at - 1])).focus_serial < serial {
                order[at] = order[at - 1];
                at -= 1;
            }
            order[at] = slot;
            length += 1;
        }
        length
    }

    pub(super) fn focused_normal(&self) -> Option<usize> {
        let focus = self.focus.filter(|&slot| self.is_visible(slot))?;
        Some(self.window_at(focus).parent.map_or(focus, usize::from))
    }

    /// Runs `shortcut`, pressed now.
    pub(super) fn run_shortcut(
        &mut self,
        shortcut: Shortcut,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> super::Action {
        let focused = self.focus.filter(|&slot| self.is_visible(slot));
        let normal = self.focused_normal();
        match shortcut {
            Shortcut::Switch { back, held } => self.open_switcher(back, held, damage, events),
            Shortcut::Cycle { back } => {
                let mut order = [0u8; super::MAX_WINDOWS];
                let length = self.switcher_order(&mut order);
                if length > 1 {
                    let next = match back {
                        true => order[length - 1],
                        false => order[1],
                    };
                    self.activate(usize::from(next), damage, events);
                }
            }
            Shortcut::Close => {
                if let Some(slot) = focused {
                    self.request_close(slot, events);
                }
            }
            Shortcut::WindowMenu => {
                if let Some(slot) = focused {
                    self.open_window_menu(slot, None, damage, events);
                }
            }
            Shortcut::Maximise => {
                if let Some(slot) = normal {
                    self.maximise(slot, damage, events);
                }
            }
            Shortcut::RestoreOrMinimise => {
                if let Some(slot) = normal {
                    match self.window_at(slot).maximised {
                        true => self.unmaximise(slot, damage, events),
                        false => self.minimise(slot, damage, events),
                    }
                }
            }
            Shortcut::Minimise => {
                if let Some(slot) = normal {
                    self.minimise(slot, damage, events);
                }
            }
            Shortcut::Snap { right } => {
                if let Some(slot) = normal {
                    self.snap(slot, right, damage, events);
                }
            }
            Shortcut::ShowDesktop => self.show_desktop(damage, events),
            Shortcut::PowerMenu => {
                self.open_menu(damage);
                self.menu_hover = Some(0);
            }
            Shortcut::Launcher => self.toggle_launcher(damage, events),
        }
        super::Action::None
    }

    fn show_desktop(&mut self, damage: &mut Damage, events: &mut dyn EventSink) {
        let hidden = self
            .windows
            .iter()
            .flatten()
            .any(|window| window.hidden_by_show_desktop && window.minimised);
        if hidden {
            let stack = self.stack;
            for &slot in &stack[..self.stack_length] {
                let slot = usize::from(slot);
                let window = self.window_at(slot);
                if window.hidden_by_show_desktop && window.minimised {
                    self.restore(slot, damage, events);
                }
            }
        } else {
            for index in 0..self.stack_length {
                let slot = usize::from(self.stack[index]);
                if self.is_visible(slot) && self.window_at(slot).role == Role::Normal {
                    self.minimise(slot, damage, events);
                    self.window_mut(slot).hidden_by_show_desktop = true;
                }
            }
        }
        for window in self.windows.iter_mut().flatten() {
            if !window.minimised {
                window.hidden_by_show_desktop = false;
            }
        }
    }

    /// The pointer shape the desktop shows now at the pointer, over `hit`.
    pub(super) fn wanted_pointer(&self, hit: Hit) -> PointerShape {
        if let Some(resize) = self.resize {
            return resize_pointer(resize.edges);
        }
        if let Some(menu) = self.window_menu {
            return menu.pointer();
        }
        if self.drag.is_some() {
            return PointerShape::Arrow;
        }
        if let Some(grab) = self.grab {
            return self.window_at(grab).pointer_shape;
        }
        if let Some((_, edges)) = hit.resize {
            return resize_pointer(edges);
        }
        let (x, y) = self.pointer;
        if self.over_launcher_field(x, y) {
            return PointerShape::IBeam;
        }
        match self.content_under(hit) {
            Some(slot) => self.window_at(slot).pointer_shape,
            None => PointerShape::Arrow,
        }
    }

    pub(super) fn popup_at(&self, x: u32, y: u32) -> bool {
        (self.menu_open && contains(self.menu_with_shadow(), x, y))
            || self
                .switcher
                .as_ref()
                .is_some_and(|switcher| contains(self.switcher_rect(switcher), x, y))
            || self
                .window_menu_area()
                .is_some_and(|rect| contains(rect, x, y))
            || self
                .main_menu_area()
                .is_some_and(|rect| contains(rect, x, y))
            || self
                .launcher_area()
                .is_some_and(|rect| contains(rect, x, y))
            || self
                .icon_menu_area()
                .is_some_and(|rect| contains(rect, x, y))
            || self.notice_area().is_some_and(|rect| contains(rect, x, y))
    }

    /// Brings the drawn pointer up to date with the desktop's state, damaging where it was and
    /// where it is when it moved or changed shape.
    pub(super) fn update_pointer(&mut self, damage: &mut Damage) {
        if self.pointer_visible {
            let (x, y) = self.pointer;
            self.update_pointer_over(self.hit(x, y), damage);
        }
    }

    /// [`Self::update_pointer`] with `hit` what is under the pointer.
    pub(super) fn update_pointer_over(&mut self, hit: Hit, damage: &mut Damage) {
        if !self.pointer_visible {
            return;
        }
        let (x, y) = self.pointer;
        let drawn = Some((x, y, self.wanted_pointer(hit)));
        if drawn == self.pointer_drawn {
            return;
        }
        if let Some((old_x, old_y, old_shape)) = self.pointer_drawn {
            damage.add(pointer::area(old_shape, old_x, old_y).clipped(self.width, self.height));
        }
        if let Some((x, y, shape)) = drawn {
            damage.add(pointer::area(shape, x, y).clipped(self.width, self.height));
        }
        self.pointer_drawn = drawn;
    }

    pub(super) fn control_under(&self, hit: Hit) -> Option<(usize, Part)> {
        if self.drag.is_some() || self.resize.is_some() || self.grab.is_some() {
            return None;
        }
        match hit.window {
            Some((slot, part @ (Part::Close | Part::Minimise | Part::Maximise))) => {
                Some((slot, part))
            }
            _ => None,
        }
    }

    pub(super) fn update_control_hover(&mut self, damage: &mut Damage) {
        let (x, y) = self.pointer;
        self.update_control_hover_over(self.hit(x, y), damage);
    }

    /// [`Self::update_control_hover`] with `hit` what is under the pointer.
    pub(super) fn update_control_hover_over(&mut self, hit: Hit, damage: &mut Damage) {
        let hover = self.control_under(hit);
        if hover == self.control_hover {
            return;
        }
        for (slot, _) in [self.control_hover, hover].into_iter().flatten() {
            if self.is_visible(slot) {
                damage.add(self.window_at(slot).title_bar());
            }
        }
        self.control_hover = hover;
    }

    /// The window menu's `Move` or `Size` moved the window by one step in direction `code`.
    pub(super) fn keyboard_step(
        &mut self,
        slot: usize,
        sizing: bool,
        code: u16,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        use crate::input::key;
        let step = i64::from(KEYBOARD_STEP);
        let (dx, dy) = match code {
            key::LEFT => (-step, 0),
            key::RIGHT => (step, 0),
            key::UP => (0, -step),
            key::DOWN => (0, step),
            _ => return,
        };
        let window = *self.window_at(slot);
        if !sizing {
            self.move_window(
                slot,
                i64::from(window.x) + dx,
                i64::from(window.y) + dy,
                damage,
            );
            return;
        }
        let outside = windows::resized(
            window.outside(),
            edge::RIGHT | edge::BOTTOM,
            dx,
            dy,
            self.minimum_outside_of(slot),
            self.work_area(),
        );
        let geometry = Geometry {
            x: outside.x,
            y: outside.y,
            content_width: outside.width - windows::CHROME.0,
            content_height: outside.height - windows::CHROME.1,
        };
        self.window_mut(slot).square_corners = false;
        self.apply_geometry(slot, geometry, window.with_shadow(), damage, events);
    }

    /// Puts window `slot` back to `geometry`, as Esc does after the window menu's `Move` or
    /// `Size`.
    pub(super) fn put_back(
        &mut self,
        slot: usize,
        geometry: Geometry,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) {
        let old = self.window_at(slot).with_shadow();
        self.apply_geometry(slot, geometry, old, damage, events);
    }

    pub(super) fn switcher_rect(&self, switcher: &Switcher) -> Rect {
        switcher.rect(self.work_area())
    }

    pub(super) fn window_menu_rect(&self, menu: &WindowMenu) -> Rect {
        menu.rect(self.screen())
    }
}

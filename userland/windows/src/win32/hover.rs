//! Where the pointer is among a program's windows (D227, `docs/ux/windows-controls-v0.3.md`
//! sections 1, 14 and 16): the window under it, which the system controls draw hovered;
//! `TrackMouseEvent`'s `WM_MOUSELEAVE` and `WM_MOUSEHOVER`; and `WM_SETCURSOR`, sent when a
//! pointer message is taken from the queue and answered by `DefWindowProc` with the class's
//! cursor after the parents had their say.

use super::gui::{self, Gui, with};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::message;
use crate::gui::queue::{Filter, Message};
use crate::gui::windows::{self, ControlKind, Procedure};
use crate::library::Export;

/// `WM_SETCURSOR`.
pub const SET_CURSOR: u32 = 0x0020;
/// `WM_MOUSEHOVER`.
pub const MOUSE_HOVER: u32 = 0x02a1;
/// `WM_MOUSELEAVE`.
pub const MOUSE_LEAVE: u32 = 0x02a3;
/// `SPI_GETMOUSEHOVERTIME`'s answer: how long the pointer rests before `WM_MOUSEHOVER`.
pub const HOVER_TIME_MS: u32 = 400;
/// `SM_CXMOUSEHOVER` and `SM_CYMOUSEHOVER`: the rectangle the pointer rests in, centred on it.
pub const HOVER_RECT: i32 = 4;
const HT_CLIENT: usize = 1;
const TME_HOVER: u32 = 0x1;
const TME_LEAVE: u32 = 0x2;
const TME_QUERY: u32 = 0x4000_0000;
const TME_CANCEL: u32 = 0x8000_0000;
const HOVER_DEFAULT: u32 = 0xffff_ffff;
const MK_SHIFT: usize = 0x4;
const MK_CONTROL: usize = 0x8;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const TRACK_RECORD_SIZE: u32 = core::mem::size_of::<TrackRecord>() as u32;

#[repr(C)]
#[derive(Clone, Copy)]
struct TrackRecord {
    size: u32,
    flags: u32,
    window: usize,
    hover_time: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HoverWait {
    window: usize,
    delay: u32,
    anchor: (i32, i32),
    since: u32,
}

/// The window under the pointer and what `TrackMouseEvent` asked about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hover {
    under: usize,
    point: (i32, i32),
    leave: usize,
    wait: Option<HoverWait>,
}

/// What a pointer move changed: the windows to repaint and the window owed `WM_MOUSELEAVE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Moved {
    pub left: usize,
    pub entered: usize,
    pub leave_owed: usize,
}

impl Hover {
    pub const fn new() -> Hover {
        Hover {
            under: 0,
            point: (0, 0),
            leave: 0,
            wait: None,
        }
    }

    /// The window under the pointer, 0 for none of the program's.
    pub fn under(&self) -> usize {
        self.under
    }

    /// The pointer is at `point` in window `window` (0: over none of the program's windows'
    /// client areas) at `time`.
    pub fn moved(&mut self, window: usize, point: (i32, i32), time: u32) -> Moved {
        let old = self.under;
        self.under = window;
        self.point = point;
        if old == window {
            if let Some(wait) = self.wait.as_mut().filter(|wait| wait.window == window) {
                let (dx, dy) = (point.0 - wait.anchor.0, point.1 - wait.anchor.1);
                if dx.abs() > HOVER_RECT / 2 || dy.abs() > HOVER_RECT / 2 {
                    wait.anchor = point;
                    wait.since = time;
                }
            }
            return Moved::default();
        }
        let leave_owed = match self.leave == old && old != 0 {
            true => {
                self.leave = 0;
                old
            }
            false => 0,
        };
        if self.wait.is_some_and(|wait| wait.window == old) {
            self.wait = None;
        }
        Moved {
            left: old,
            entered: window,
            leave_owed,
        }
    }

    /// `TME_LEAVE` for `window`: tracked while it is under the pointer, else owed at once
    /// (returned).
    pub fn track_leave(&mut self, window: usize) -> Option<usize> {
        match self.under == window {
            true => {
                self.leave = window;
                None
            }
            false => Some(window),
        }
    }

    /// `TME_HOVER` for `window` with `delay` milliseconds, from `time`; nothing when the
    /// pointer is not over it.
    pub fn track_hover(&mut self, window: usize, delay: u32, time: u32) {
        if self.under == window {
            self.wait = Some(HoverWait {
                window,
                delay,
                anchor: self.point,
                since: time,
            });
        }
    }

    /// `TME_CANCEL` of `flags` for `window`.
    pub fn cancel(&mut self, window: usize, flags: u32) {
        if flags & TME_LEAVE != 0 && self.leave == window {
            self.leave = 0;
        }
        if flags & TME_HOVER != 0 && self.wait.is_some_and(|wait| wait.window == window) {
            self.wait = None;
        }
    }

    /// The window `WM_MOUSEHOVER` is due for at `time`, with the point.
    pub fn hover_due(&self, time: u32) -> Option<(usize, (i32, i32))> {
        let wait = self.wait?;
        (time.wrapping_sub(wait.since) >= wait.delay).then_some((wait.window, wait.anchor))
    }

    /// Milliseconds until a `WM_MOUSEHOVER` is due, from `time`.
    pub fn hover_in(&self, time: u32) -> Option<u32> {
        let wait = self.wait?;
        Some(wait.delay.saturating_sub(time.wrapping_sub(wait.since)))
    }

    /// `WM_MOUSEHOVER` was delivered: hover tracking ends, as Win32's does.
    pub fn hover_delivered(&mut self) {
        self.wait = None;
    }

    /// Window `window` was destroyed.
    pub fn forget(&mut self, window: usize) {
        if self.under == window {
            self.under = 0;
        }
        if self.leave == window {
            self.leave = 0;
        }
        if self.wait.is_some_and(|wait| wait.window == window) {
            self.wait = None;
        }
    }
}

fn has_hover_look(gui: &Gui, window: usize) -> bool {
    gui.windows.get(window).is_some_and(|entry| {
        matches!(
            entry.procedure,
            Procedure::Control(ControlKind::Button | ControlKind::Edit | ControlKind::ScrollBar, _)
        )
    })
}

/// A pointer input reached top-level window `root`'s tree: `target` is the window under it and
/// the point there, `None` when it is over the menu bar or a menu or combo list took it.
/// Repaints the controls it left and entered and posts the `WM_MOUSELEAVE` owed.
pub fn pointer_input(gui: &mut Gui, target: Option<(usize, (i32, i32))>, time: u32) {
    let (window, point) = target.unwrap_or((0, (0, 0)));
    let moved = gui.hover.moved(window, point, time);
    finish_move(gui, moved);
}

/// The pointer left top-level window `root`.
pub fn pointer_left(gui: &mut Gui, root: usize) {
    let under = gui.hover.under();
    if under != 0 && windows::is_in_tree(&gui.windows, root, under) {
        let moved = gui.hover.moved(0, (0, 0), super::user32::now_ms());
        finish_move(gui, moved);
    }
}

fn finish_move(gui: &mut Gui, moved: Moved) {
    for window in [moved.left, moved.entered] {
        if window != 0
            && has_hover_look(gui, window)
            && let Some(entry) = gui.windows.get_mut(window)
        {
            entry.invalidate(None, false);
        }
    }
    if moved.leave_owed != 0 {
        post_leave(gui, moved.leave_owed);
    }
}

fn post_leave(gui: &mut Gui, window: usize) {
    let Some(thread) = gui.windows.get(window).map(|entry| entry.thread) else {
        return;
    };
    let _ = gui.post(window, thread, MOUSE_LEAVE, 0, 0);
}

fn key_bits(gui: &Gui) -> usize {
    let keys = gui.keyboard_state();
    let mut bits = 0;
    if keys[VK_SHIFT] & 0x80 != 0 {
        bits |= MK_SHIFT;
    }
    if keys[VK_CONTROL] & 0x80 != 0 {
        bits |= MK_CONTROL;
    }
    bits
}

/// The `WM_MOUSEHOVER` due at `now` that `filter` accepts, taken when `remove` says so.
pub fn take_due(gui: &mut Gui, filter: &Filter, now: u32, remove: bool) -> Option<Message> {
    let (window, point) = gui.hover.hover_due(now)?;
    let thread = gui.windows.get(window)?.thread;
    let message = Message {
        window,
        message: MOUSE_HOVER,
        wparam: key_bits(gui),
        lparam: message::point_parameter(point.0, point.1),
        time: now,
        point,
        thread,
    };
    if !filter.accepts(&message) {
        return None;
    }
    if remove {
        gui.hover.hover_delivered();
    }
    Some(message)
}

/// Milliseconds until the next `WM_MOUSEHOVER` for `filter`'s thread, from `now`.
pub fn next_due(gui: &Gui, filter: &Filter, now: u32) -> Option<u32> {
    let window = gui.hover.wait?.window;
    let thread = gui.windows.get(window)?.thread;
    if filter.thread != 0 && filter.thread != thread {
        return None;
    }
    gui.hover.hover_in(now)
}

/// Sends `WM_SETCURSOR` for a pointer message `taken` from the queue, as Win32 does before
/// handing it to the program, unless a window has the capture.
pub fn send_set_cursor(taken: &Message) {
    if !matches!(
        taken.message,
        message::MOUSE_MOVE
            | message::LEFT_BUTTON_DOWN
            | message::LEFT_BUTTON_DOUBLE_CLICK
            | message::RIGHT_BUTTON_DOWN
            | message::MIDDLE_BUTTON_DOWN
    ) {
        return;
    }
    let free = with(|gui| {
        windows::root_of(&gui.windows, taken.window)
            .and_then(|root| gui.windows.get(root))
            .is_some_and(|root| root.capture == 0)
    });
    if free == Some(true) {
        let lparam = message::word_pair(HT_CLIENT, taken.message as usize);
        gui::send(taken.window, SET_CURSOR, taken.window, lparam);
    }
}

/// `DefWindowProc`'s `WM_SETCURSOR`: a child's parent is asked first and a nonzero answer
/// ends it; then, over the client area of the window under the pointer, its class's cursor
/// (a null one leaves the shape as it is).
pub fn default_set_cursor(window: usize, wparam: usize, lparam: usize) -> usize {
    let found = with(|gui| {
        let entry = gui.windows.get(window)?;
        let parent = match entry.is_child() {
            true => entry.parent,
            false => 0,
        };
        Some((parent, super::window_class::cursor_of(gui, window)))
    })
    .flatten();
    let Some((parent, cursor)) = found else {
        return 0;
    };
    if parent != 0 && gui::send(parent, SET_CURSOR, wparam, lparam) != 0 {
        return 1;
    }
    if wparam != window || lparam & 0xffff != HT_CLIENT || cursor == 0 {
        return 0;
    }
    super::pointer::replace(cursor);
    1
}

/// `TrackMouseEvent`, and comctl32's `_TrackMouseEvent`.
pub(super) extern "efiapi" fn track_mouse_event(record: usize) -> Bool {
    if record == 0 {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: `TrackMouseEvent`'s contract: a non-null pointer to a `TRACKMOUSEEVENT`; only
    // its size field is read before it is checked.
    let size = unsafe { (record as *const u32).read_unaligned() };
    if size != TRACK_RECORD_SIZE {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: as above, and `cbSize` says the whole structure is there.
    let mut track = unsafe { (record as *const TrackRecord).read_unaligned() };
    if track.flags & TME_QUERY != 0 {
        let (leave, wait) = with(|gui| (gui.hover.leave, gui.hover.wait)).unwrap_or((0, None));
        track.flags = 0;
        track.window = 0;
        track.hover_time = 0;
        if leave != 0 {
            track.flags |= TME_LEAVE;
            track.window = leave;
        }
        if let Some(wait) = wait {
            track.flags |= TME_HOVER;
            track.window = wait.window;
            track.hover_time = wait.delay;
        }
        // SAFETY: as above; `TME_QUERY` writes the structure back.
        unsafe { (record as *mut TrackRecord).write_unaligned(track) };
        return TRUE;
    }
    let done = gui::try_with(|gui| {
        gui.window(track.window)?;
        if track.flags & TME_CANCEL != 0 {
            gui.hover.cancel(track.window, track.flags);
            return Ok(());
        }
        if track.flags & TME_LEAVE != 0
            && let Some(owed) = gui.hover.track_leave(track.window)
        {
            post_leave(gui, owed);
        }
        if track.flags & TME_HOVER != 0 {
            let delay = match track.hover_time {
                HOVER_DEFAULT => HOVER_TIME_MS,
                delay => delay,
            };
            gui.hover.track_hover(track.window, delay, super::user32::now_ms());
        }
        Ok(())
    });
    gui::wake();
    match done {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"TrackMouseEvent" => track_mouse_event,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_onto_another_window_leaves_the_tracked_one_once() {
        let mut hover = Hover::new();
        assert_eq!(hover.moved(7, (3, 3), 0).entered, 7);
        assert_eq!(hover.track_leave(7), None);
        assert_eq!(hover.moved(7, (5, 3), 10), Moved::default());
        let moved = hover.moved(9, (1, 1), 20);
        assert_eq!(
            moved,
            Moved {
                left: 7,
                entered: 9,
                leave_owed: 7
            }
        );
        assert_eq!(hover.moved(7, (1, 1), 30).leave_owed, 0);
        assert_eq!(hover.track_leave(9), Some(9));
    }

    #[test]
    fn hover_is_due_after_the_delay_unless_the_pointer_moves_out_of_its_rectangle() {
        let mut hover = Hover::new();
        hover.moved(7, (10, 10), 0);
        hover.track_hover(7, HOVER_TIME_MS, 0);
        assert_eq!(hover.hover_due(399), None);
        assert_eq!(hover.hover_in(100), Some(300));
        hover.moved(7, (12, 8), 200);
        assert_eq!(hover.hover_due(400), Some((7, (10, 10))));
        hover.moved(7, (13, 10), 300);
        assert_eq!(hover.hover_due(400), None);
        assert_eq!(hover.hover_due(700), Some((7, (13, 10))));
        hover.hover_delivered();
        assert_eq!(hover.hover_due(5000), None);
        hover.track_hover(7, 100, 1000);
        hover.moved(0, (0, 0), 1010);
        assert_eq!(hover.hover_due(5000), None);
    }

    #[test]
    fn hover_is_only_tracked_over_the_window_and_cancel_and_forget_end_it() {
        let mut hover = Hover::new();
        hover.moved(7, (1, 1), 0);
        hover.track_hover(8, 10, 0);
        assert_eq!(hover.hover_due(100), None);
        hover.track_hover(7, 10, 0);
        hover.cancel(7, TME_HOVER);
        assert_eq!(hover.hover_due(100), None);
        hover.track_leave(7);
        hover.forget(7);
        assert_eq!(hover.under(), 0);
        assert_eq!(hover.moved(9, (0, 0), 1).leave_owed, 0);
    }
}

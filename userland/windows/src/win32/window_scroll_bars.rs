//! The scroll bars of the program's own windows (`WS_VSCROLL`, `WS_HSCROLL`, D342): drawn right
//! of and under the client area, on the window's top-level buffer whenever that buffer is
//! shown, driven by the pointer before the program sees it, and reporting to the window
//! itself with `WM_VSCROLL` and `WM_HSCROLL` (a `lParam` of 0), as Windows' standard bars do.

use super::gui::{self, Gui, with};
use crate::gui::message;
use crate::gui::windows::{self, Window};
use eggos_windows_gui::controls::scroll_bar::{self as bar_model, BarLook, Scroll};
use eggos_windows_gui::surface::{Rect, Surface};

const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;

/// The window's bar the pointer holds, from the press to the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub window: usize,
    pub vertical: bool,
}

fn outer_clip(gui: &Gui, entry: &Window, origin: (i32, i32)) -> Option<Rect> {
    let more = |shown: bool| if shown { windows::SCROLL_BAR_SIZE } else { 0 };
    let outer = Rect::sized(
        origin.0,
        origin.1,
        entry.width as i32 + more(entry.shows_bar(true)),
        entry.height as i32 + more(entry.shows_bar(false)),
    );
    let within = match entry.is_child() {
        true if !entry.shown => return None,
        true => windows::client_placement(&gui.windows, entry.parent)?.clip,
        false => Rect::new(
            0,
            entry.menu_height,
            entry.buffer_width as i32,
            entry.buffer_height as i32,
        ),
    };
    let clip = outer.intersection(&within);
    (!clip.is_empty()).then_some(clip)
}

/// Paints window `handle`'s own bars on its top-level buffer: the area painted, in that
/// buffer's coordinates.
pub fn paint_window(gui: &Gui, handle: usize) -> Option<Rect> {
    let entry = gui.windows.get(handle)?;
    if !(entry.shows_bar(true) || entry.shows_bar(false)) {
        return None;
    }
    let placement = windows::client_placement(&gui.windows, handle)?;
    let top = gui.windows.get(placement.root)?;
    let clip = outer_clip(gui, entry, placement.origin)?;
    // SAFETY: a top-level window's buffer is mapped, `buffer_width * buffer_height` pixels,
    // while the window is in the table, and the table's lock is held.
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            top.pixels as *mut u32,
            top.buffer_width as usize * top.buffer_height as usize,
        )
    };
    let mut surface = Surface::new(pixels, top.buffer_width, top.buffer_height)?;
    surface.translate(placement.origin.0, placement.origin.1);
    surface.clip_to(clip.offset(-placement.origin.0, -placement.origin.1));
    let palette = super::controls::palette_of(gui, handle);
    let held = gui.held_bar;
    for vertical in [true, false] {
        let Some(rect) = entry.bar_rect(vertical) else {
            continue;
        };
        let look = BarLook {
            enabled: entry.enabled(),
            focused: false,
            hovered: held
                == Some(Held {
                    window: handle,
                    vertical,
                }),
        };
        bar_model::paint(&mut surface, rect, entry.bar(vertical), look, palette);
    }
    if let (Some(right), Some(under)) = (entry.bar_rect(true), entry.bar_rect(false)) {
        let corner = Rect::new(right.left, under.top, right.right, under.bottom);
        surface.fill(corner, palette.scroll_track);
    }
    let drawn = surface.drawn();
    (!drawn.is_empty()).then(|| drawn.offset(placement.origin.0, placement.origin.1))
}

/// Paints the own bars of every window of `root`'s tree: the area painted, in the buffer's
/// coordinates.
pub fn paint_tree(gui: &Gui, root: usize) -> Option<Rect> {
    let (handles, count) = windows::tree(&gui.windows, root);
    handles[..count]
        .iter()
        .filter_map(|&handle| paint_window(gui, handle))
        .reduce(|area, painted| area.union(&painted))
}

/// Repaints window `handle`'s own bars and shows them, once its top-level window was shown.
pub fn show(handle: usize) {
    let commit = with(|gui| {
        let rect = paint_window(gui, handle)?;
        let root = windows::root_of(&gui.windows, handle)?;
        let top = gui.windows.get(root)?;
        let device = gui.device?;
        top.committed.then_some((device, top.server, rect))
    })
    .flatten();
    if let Some((device, server, rect)) = commit {
        let _ = device.commit(server, Some(rect));
    }
}

fn along(entry: &Window, vertical: bool, local: (i32, i32)) -> (i32, i32) {
    match vertical {
        true => (local.1, entry.height as i32),
        false => (local.0, entry.width as i32),
    }
}

fn post_scroll(gui: &mut Gui, held: Held, scroll: Option<Scroll>) {
    let Some(scroll) = scroll else {
        return;
    };
    let Some(thread) = gui.windows.get(held.window).map(|entry| entry.thread) else {
        return;
    };
    let number = match held.vertical {
        true => WM_VSCROLL,
        false => WM_HSCROLL,
    };
    let wparam = message::word_pair(scroll.code as usize, scroll.position as u16 as usize);
    let _ = gui.post(held.window, thread, number, wparam, 0);
}

fn local_point(gui: &Gui, window: usize, point: (i32, i32)) -> Option<(i32, i32)> {
    let origin = windows::client_placement(&gui.windows, window)?.origin;
    Some((point.0 - origin.0, point.1 - origin.1))
}

fn bar_under(gui: &Gui, root: usize, point: (i32, i32)) -> Option<(Held, (i32, i32))> {
    let top = gui.windows.get(root)?;
    if point.1 < top.menu_height || !top.enabled() || top.modal_disabled {
        return None;
    }
    let (window, _) = windows::child_at(&gui.windows, root, point.0, point.1 - top.menu_height);
    let entry = gui.windows.get(window)?;
    if !windows::enabled_in_tree(&gui.windows, window) {
        return None;
    }
    let local = local_point(gui, window, point)?;
    let vertical = [true, false].into_iter().find(|&vertical| {
        entry
            .bar_rect(vertical)
            .is_some_and(|rect| rect.contains(local.0, local.1))
    })?;
    Some((Held { window, vertical }, local))
}

/// A pointer input for `root`'s tree at `point` (its buffer's coordinates) that a window's own
/// bar takes: a press on a bar, and the moves and the release while one is held. The area to
/// show again, when the input was the bar's.
pub fn input(gui: &mut Gui, root: usize, number: u32, point: (i32, i32)) -> Option<Option<Rect>> {
    if gui
        .held_bar
        .is_some_and(|held| gui.windows.get(held.window).is_none())
    {
        gui.held_bar = None;
    }
    let held = match (gui.held_bar, number) {
        (Some(held), message::MOUSE_MOVE | message::LEFT_BUTTON_UP) => held,
        (None, message::LEFT_BUTTON_DOWN) => {
            let (held, local) = bar_under(gui, root, point)?;
            let entry = gui.windows.get_mut(held.window)?;
            let (offset, length) = along(entry, held.vertical, local);
            let scroll = entry.bar_mut(held.vertical).button_down(offset, length);
            gui.held_bar = Some(held);
            post_scroll(gui, held, scroll);
            return Some(paint_window(gui, held.window));
        }
        _ => return None,
    };
    let local = local_point(gui, held.window, point)?;
    let entry = gui.windows.get_mut(held.window)?;
    let (offset, length) = along(entry, held.vertical, local);
    let bar = entry.bar_mut(held.vertical);
    let scrolls = match number {
        message::MOUSE_MOVE => [bar.pointer_moved(offset, length), None],
        _ => {
            gui.held_bar = None;
            bar.button_up()
        }
    };
    for scroll in scrolls {
        post_scroll(gui, held, scroll);
    }
    Some(paint_window(gui, held.window))
}

/// Shows what [`input`] painted, once the top-level window was shown.
pub fn finish_input(root: usize, painted: Option<Rect>) {
    let shown = with(|gui| {
        let window = gui.windows.get(root)?;
        Some((gui.device?, window.server, window.committed))
    })
    .flatten();
    if let (Some(rect), Some((device, server, true))) = (painted, shown) {
        let _ = device.commit(server, Some(rect));
    }
    gui::wake();
}

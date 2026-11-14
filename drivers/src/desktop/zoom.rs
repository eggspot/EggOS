//! Zoom (D602, docs/ux/accessibility-v0.4.md section 2): the whole screen magnified by a whole
//! factor, nearest neighbour, around a view that follows the pointer, the focus and the caret,
//! and pans by key. The desktop keeps the level and the view; the kernel composes the frame as
//! always into a source buffer, then writes output pixel `(ox, oy)` from source
//! `(x + ox / level, y + oy / level)` ([`magnify_rows`]).

use super::{Action, Desktop};
use crate::display::{Damage, Rect};

/// The levels zoom takes, in order: `accessibility.zoom_level`'s values.
pub const ZOOM_LEVELS: [u32; 5] = [2, 3, 4, 6, 8];
/// [`ZOOM_LEVELS`] as `settings set` takes them.
pub const ZOOM_LEVEL_NAMES: [&str; 5] = ["2", "3", "4", "6", "8"];
/// How long the strip says a zoom change.
pub const ZOOM_NOTICE_MS: u32 = 1500;
const ZOOM_NOTICES: [&str; 5] = ["Zoom 2x", "Zoom 3x", "Zoom 4x", "Zoom 6x", "Zoom 8x"];
const ZOOM_OFF: &str = "Zoom off";

/// A way the keys move the view, by a quarter of its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pan {
    Up,
    Down,
    Left,
    Right,
}

/// What a zoom key asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCommand {
    /// On at the saved level, or off.
    Toggle,
    /// The next level; on at 2x when off.
    In,
    /// The previous level; off from 2x.
    Out,
    /// Off, and nothing when off: Win+Esc.
    Off,
    /// Moves the view while zoom is on.
    Pan(Pan),
}

/// What the lens shows: the level and the view's top-left corner on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoomView {
    pub level: u32,
    pub x: u32,
    pub y: u32,
}

/// The zoom's state: the level in force (0 off), the one the settings hold, and the view.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Zoom {
    level: u32,
    saved_level: u32,
    x: u32,
    y: u32,
}

fn level_index(level: u32) -> Option<usize> {
    ZOOM_LEVELS.iter().position(|&known| known == level)
}

/// The view's size at `level` on a `width` by `height` screen: each rounded up.
pub fn view_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    let level = level.max(1);
    (width.div_ceil(level), height.div_ceil(level))
}

/// The least move of `start` (a view `length` long) that brings `from..to` into it, a range
/// longer than the view aligned at its start.
fn brought_in(start: u32, length: u32, from: u32, to: u32) -> u32 {
    if to.saturating_sub(from) > length || from < start {
        return from;
    }
    match to > start + length {
        true => to - length,
        false => start,
    }
}

/// Writes rows `rows` of the magnified frame: output pixel `(ox, oy)` takes source pixel
/// `(view.x + ox / level, view.y + oy / level)`, one read and one write each; both buffers are
/// `width` pixels a row.
pub fn magnify_rows(
    source: &[u32],
    output: &mut [u32],
    width: u32,
    view: ZoomView,
    rows: Rect,
) {
    let level = view.level.max(1);
    let stride = width as usize;
    for oy in rows.y..rows.bottom() {
        let sy = (view.y + oy / level) as usize;
        let Some(source_row) = source.get(sy * stride..(sy + 1) * stride) else {
            continue;
        };
        let Some(out_row) = output.get_mut(oy as usize * stride..(oy as usize + 1) * stride) else {
            continue;
        };
        for ox in rows.x..rows.right().min(width) {
            let sx = (view.x + ox / level) as usize;
            if let Some(&pixel) = source_row.get(sx) {
                out_row[ox as usize] = pixel;
            }
        }
    }
}

impl Desktop {
    /// Sets zoom from the settings (`SET_ACCESSIBILITY`): on at `level` or off (`None`); the
    /// view keeps its place, clamped. A change repaints the screen.
    pub fn set_zoom(&mut self, level: Option<u32>, saved_level: u32, damage: &mut Damage) {
        if level_index(saved_level).is_some() {
            self.zoom.saved_level = saved_level;
        }
        let level = level.filter(|&level| level_index(level).is_some()).unwrap_or(0);
        if level == self.zoom.level {
            return;
        }
        self.zoom.level = level;
        self.clamp_zoom_view();
        damage.add(Rect::new(0, 0, self.width, self.height));
    }

    /// What the lens shows, while zoom is on.
    pub fn zoom_view(&self) -> Option<ZoomView> {
        (self.zoom.level > 1).then_some(ZoomView {
            level: self.zoom.level,
            x: self.zoom.x,
            y: self.zoom.y,
        })
    }

    /// Whether `command` is one the desktop takes now: panning only while zoom is on, so
    /// Ctrl+Alt+arrows reach the focused window otherwise.
    pub(super) fn takes_zoom_command(&self, command: ZoomCommand) -> bool {
        !matches!(command, ZoomCommand::Pan(_) | ZoomCommand::Off) || self.zoom.level > 1
    }

    /// Runs a zoom key at once, says the new level in the strip, and asks for the setting it
    /// changed to be saved, so the pane and the next boot agree.
    pub(super) fn zoom_command(&mut self, command: ZoomCommand, damage: &mut Damage) -> Action {
        let on = self.zoom.level > 1;
        let index = level_index(self.zoom.level);
        let (level, saved) = match command {
            ZoomCommand::Pan(direction) => {
                self.pan_zoom(direction, damage);
                return Action::None;
            }
            ZoomCommand::Toggle if on => (0, ("accessibility.zoom", "false")),
            ZoomCommand::Off => (0, ("accessibility.zoom", "false")),
            ZoomCommand::Toggle => (
                self.zoom.saved_level.max(ZOOM_LEVELS[0]),
                ("accessibility.zoom", "true"),
            ),
            ZoomCommand::In => match index {
                None => (ZOOM_LEVELS[0], ("accessibility.zoom", "true")),
                Some(index) => {
                    let next = (index + 1).min(ZOOM_LEVELS.len() - 1);
                    (
                        ZOOM_LEVELS[next],
                        ("accessibility.zoom_level", ZOOM_LEVEL_NAMES[next]),
                    )
                }
            },
            ZoomCommand::Out => match index {
                None | Some(0) => (0, ("accessibility.zoom", "false")),
                Some(index) => (
                    ZOOM_LEVELS[index - 1],
                    ("accessibility.zoom_level", ZOOM_LEVEL_NAMES[index - 1]),
                ),
            },
        };
        if level != self.zoom.level {
            self.zoom.level = level;
            if level > 1 {
                self.zoom.saved_level = level;
            }
            let (x, y) = self.pointer;
            let (view_width, view_height) = view_size(self.width, self.height, level);
            self.zoom.x = x.saturating_sub(view_width / 2);
            self.zoom.y = y.saturating_sub(view_height / 2);
            self.clamp_zoom_view();
            damage.add(Rect::new(0, 0, self.width, self.height));
        }
        let notice = level_index(level).map_or(ZOOM_OFF, |index| ZOOM_NOTICES[index]);
        self.show_strip_notice(notice, ZOOM_NOTICE_MS, damage);
        Action::SetSetting {
            key: saved.0,
            value: saved.1,
        }
    }

    /// Keeps the pointer's hotspot in the view: a hotspot leaving it moves the view by the
    /// overshoot.
    pub(super) fn zoom_follow_pointer(&mut self, x: u32, y: u32, damage: &mut Damage) {
        if self.zoom.level <= 1 {
            return;
        }
        self.bring_into_zoom_view(Rect::new(x, y, 1, 1), damage);
    }

    /// Brings `rect`, the focused element's or the caret's on the screen, into the view by the
    /// least move; a rect larger than the view is aligned at its top-left (D602, D603).
    pub fn bring_into_zoom_view(&mut self, rect: Rect, damage: &mut Damage) {
        if self.zoom.level <= 1 {
            return;
        }
        let (view_width, view_height) = view_size(self.width, self.height, self.zoom.level);
        let x = brought_in(self.zoom.x, view_width, rect.x, rect.right());
        let y = brought_in(self.zoom.y, view_height, rect.y, rect.bottom());
        if (x, y) == (self.zoom.x, self.zoom.y) {
            return;
        }
        self.zoom.x = x;
        self.zoom.y = y;
        self.clamp_zoom_view();
        damage.add(Rect::new(0, 0, self.width, self.height));
    }

    fn pan_zoom(&mut self, direction: Pan, damage: &mut Damage) {
        let (view_width, view_height) = view_size(self.width, self.height, self.zoom.level);
        let (step_x, step_y) = ((view_width / 4).max(1), (view_height / 4).max(1));
        match direction {
            Pan::Up => self.zoom.y = self.zoom.y.saturating_sub(step_y),
            Pan::Down => self.zoom.y += step_y,
            Pan::Left => self.zoom.x = self.zoom.x.saturating_sub(step_x),
            Pan::Right => self.zoom.x += step_x,
        }
        self.clamp_zoom_view();
        damage.add(Rect::new(0, 0, self.width, self.height));
    }

    fn clamp_zoom_view(&mut self) {
        let (view_width, view_height) = view_size(self.width, self.height, self.zoom.level);
        self.zoom.x = self.zoom.x.min(self.width.saturating_sub(view_width));
        self.zoom.y = self.zoom.y.min(self.height.saturating_sub(view_height));
    }

    /// The screen rect a source rect `rect` covers through the lens, clipped: what a dirty
    /// source rect damages on the display; `rect` itself while zoom is off.
    pub fn zoomed_damage(&self, rect: Rect) -> Rect {
        let Some(view) = self.zoom_view() else {
            return rect;
        };
        let left = rect.x.saturating_sub(view.x) * view.level;
        let top = rect.y.saturating_sub(view.y) * view.level;
        let right = rect.right().saturating_sub(view.x) * view.level;
        let bottom = rect.bottom().saturating_sub(view.y) * view.level;
        Rect::new(left, top, right.saturating_sub(left), bottom.saturating_sub(top))
            .clipped(self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_is_the_screen_divided_rounded_up() {
        assert_eq!(view_size(1280, 800, 2), (640, 400));
        assert_eq!(view_size(1280, 800, 3), (427, 267));
        assert_eq!(view_size(1280, 800, 6), (214, 134));
        assert_eq!(view_size(1280, 800, 8), (160, 100));
    }

    #[test]
    fn a_range_comes_into_the_view_by_the_least_move() {
        assert_eq!(brought_in(100, 200, 150, 160), 100);
        assert_eq!(brought_in(100, 200, 290, 310), 110);
        assert_eq!(brought_in(100, 200, 40, 50), 40);
        assert_eq!(brought_in(100, 200, 0, 500), 0);
    }

    #[test]
    fn each_output_pixel_takes_the_source_pixel_under_it() {
        let source: Vec<u32> = (0..16).collect();
        let mut output = vec![0u32; 16];
        let view = ZoomView {
            level: 2,
            x: 1,
            y: 1,
        };
        magnify_rows(&source, &mut output, 4, view, Rect::new(0, 0, 4, 4));
        assert_eq!(&output[0..4], &[5, 5, 6, 6]);
        assert_eq!(&output[8..12], &[9, 9, 10, 10]);
    }
}

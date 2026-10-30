//! comctl32's progress bar (section 22 of the design, D178): the [`Progress`] model (range,
//! position, step, marquee, state and the program's colours) and its painting: a 1 px border,
//! the track, and the fill growing from the left or, with `PBS_VERTICAL`, from the bottom.

use super::common::CommonPalette;
use super::outline;
use crate::colour;
use crate::surface::{Rect, Surface};

/// `CLR_DEFAULT`: the palette's colour.
pub const DEFAULT_COLOUR: u32 = 0xff00_0000;
const DEFAULT_MAX: i32 = 100;
const DEFAULT_STEP: i32 = 10;

/// The progress bar's styles this layer reads.
pub mod style {
    pub const VERTICAL: u32 = 0x4;
    pub const MARQUEE: u32 = 0x8;
}

/// `PBM_SETSTATE`'s states.
pub mod state {
    pub const NORMAL: u32 = 1;
    pub const ERROR: u32 = 2;
    pub const PAUSED: u32 = 3;
}

/// A progress bar's range, position and colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    min: i32,
    max: i32,
    position: i32,
    step: i32,
    pub marquee: bool,
    state: u32,
    bar_colour: u32,
    back_colour: u32,
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    pub const fn new() -> Self {
        Self {
            min: 0,
            max: DEFAULT_MAX,
            position: 0,
            step: DEFAULT_STEP,
            marquee: false,
            state: state::NORMAL,
            bar_colour: DEFAULT_COLOUR,
            back_colour: DEFAULT_COLOUR,
        }
    }

    pub fn range(&self) -> (i32, i32) {
        (self.min, self.max)
    }

    pub fn position(&self) -> i32 {
        self.position
    }

    pub fn step(&self) -> i32 {
        self.step
    }

    pub fn state(&self) -> u32 {
        self.state
    }

    /// `PBM_SETRANGE32`: `max <= min` is made `min + 1` and the position is clamped into it;
    /// the previous range.
    pub fn set_range(&mut self, min: i32, max: i32) -> (i32, i32) {
        let previous = (self.min, self.max);
        self.min = min;
        self.max = match max <= min {
            true => min.saturating_add(1),
            false => max,
        };
        self.position = self.position.clamp(self.min, self.max);
        previous
    }

    /// `PBM_SETPOS`, clamped into the range; the previous position.
    pub fn set_position(&mut self, position: i32) -> i32 {
        core::mem::replace(&mut self.position, position.clamp(self.min, self.max))
    }

    /// `PBM_DELTAPOS`; the previous position.
    pub fn delta(&mut self, by: i32) -> i32 {
        self.set_position(self.position.saturating_add(by))
    }

    /// `PBM_SETSTEP`; the previous step.
    pub fn set_step(&mut self, step: i32) -> i32 {
        core::mem::replace(&mut self.step, step)
    }

    /// `PBM_STEPIT`: the step added, wrapping to the range's other end when it would pass it;
    /// the previous position.
    pub fn step_it(&mut self) -> i32 {
        let next = self.position.saturating_add(self.step);
        let next = match () {
            () if next > self.max => self.min,
            () if next < self.min => self.max,
            () => next,
        };
        core::mem::replace(&mut self.position, next)
    }

    /// `PBM_SETSTATE` with one of [`state`]'s; the previous state, 0 for an unknown one.
    pub fn set_state(&mut self, wanted: u32) -> u32 {
        match wanted {
            state::NORMAL | state::ERROR | state::PAUSED => {
                core::mem::replace(&mut self.state, wanted)
            }
            _ => 0,
        }
    }

    /// `PBM_SETBARCOLOR` (a `COLORREF`, [`DEFAULT_COLOUR`] for the palette's); the previous.
    pub fn set_bar_colour(&mut self, colour_ref: u32) -> u32 {
        core::mem::replace(&mut self.bar_colour, colour_ref)
    }

    /// `PBM_SETBKCOLOR`; the previous.
    pub fn set_back_colour(&mut self, colour_ref: u32) -> u32 {
        core::mem::replace(&mut self.back_colour, colour_ref)
    }

    pub fn bar_colour(&self) -> u32 {
        self.bar_colour
    }

    pub fn back_colour(&self) -> u32 {
        self.back_colour
    }

    /// How much is done, in thousandths: `1000 (pos - min) / (max - min)`.
    pub fn per_mille(&self) -> i32 {
        self.share(1000)
    }

    fn share(&self, of: i32) -> i32 {
        let done = i64::from(self.position) - i64::from(self.min);
        let whole = (i64::from(self.max) - i64::from(self.min)).max(1);
        (i64::from(of.max(0)) * done / whole) as i32
    }

    fn fill_colour(&self, palette: &CommonPalette) -> u32 {
        if self.bar_colour != DEFAULT_COLOUR {
            return colour::pixel(self.bar_colour);
        }
        match self.state {
            state::ERROR => palette.bar_error,
            state::PAUSED => palette.bar_paused,
            _ => palette.bar_fill,
        }
    }

    /// The filled part of a bar over `rect` with `style`: the position's share of the track, or
    /// the marquee's fixed middle third; empty when nothing is filled.
    pub fn fill_rect(&self, rect: Rect, style: u32) -> Rect {
        let (inner_width, inner_height) = (rect.width() - 2, rect.height() - 2);
        let vertical = style & style::VERTICAL != 0;
        if style & style::MARQUEE != 0 {
            if !self.marquee {
                return Rect::default();
            }
            return match vertical {
                true => {
                    let third = inner_height / 3;
                    Rect::sized(rect.left + 1, rect.top + 1 + third, inner_width, third)
                }
                false => {
                    let third = inner_width / 3;
                    Rect::sized(rect.left + 1 + third, rect.top + 1, third, inner_height)
                }
            };
        }
        match vertical {
            true => {
                let filled = self.share(inner_height);
                Rect::sized(rect.left + 1, rect.bottom - 1 - filled, inner_width, filled)
            }
            false => Rect::sized(
                rect.left + 1,
                rect.top + 1,
                self.share(inner_width),
                inner_height,
            ),
        }
    }
}

/// Paints `progress` with `style` over the whole of `rect`.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    progress: &Progress,
    style: u32,
    palette: &CommonPalette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    let track = match progress.back_colour {
        DEFAULT_COLOUR => palette.bar_track,
        colour_ref => colour::pixel(colour_ref),
    };
    surface.fill(rect.inset(1), track);
    outline(surface, rect, palette.bar_border);
    let fill = progress.fill_rect(rect, style);
    if !fill.is_empty() {
        surface.fill(fill, progress.fill_colour(palette));
    }
    surface.set_clip(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::common::COMMON_LIGHT;

    const BAR: Rect = Rect::new(8, 282, 8 + 424, 282 + 20);

    fn bar_at(position: i32) -> Progress {
        let mut progress = Progress::new();
        progress.set_position(position);
        progress
    }

    #[test]
    fn the_fill_is_the_floor_of_the_positions_share() {
        let fills = [0, 40, 60, 100].map(|position| bar_at(position).fill_rect(BAR, 0));
        assert!(fills[0].is_empty());
        assert_eq!(fills[1], Rect::sized(9, 283, 168, 18));
        assert_eq!(fills[2], Rect::sized(9, 283, 253, 18));
        assert_eq!(fills[3], Rect::sized(9, 283, 422, 18));
        assert_eq!(bar_at(40).per_mille(), 400);
        assert_eq!(bar_at(60).per_mille(), 600);
    }

    #[test]
    fn the_example_bar_has_its_border_track_and_fill() {
        let mut pixels = vec![0u32; 480 * 360];
        let mut surface = Surface::new(&mut pixels, 480, 360).unwrap();
        paint(&mut surface, BAR, &bar_at(40), 0, &COMMON_LIGHT);
        assert_eq!(surface.get(8, 282), Some(0x00bc_bcbc));
        assert_eq!(surface.get(431, 301), Some(0x00bc_bcbc));
        assert_eq!(surface.get(9, 283), Some(0x0006_b025));
        assert_eq!(surface.get(176, 300), Some(0x0006_b025));
        assert_eq!(surface.get(177, 283), Some(0x00e6_e6e6));
        assert_eq!(surface.get(430, 300), Some(0x00e6_e6e6));
    }

    #[test]
    fn ranges_positions_and_steps_follow_win32() {
        let mut progress = Progress::new();
        assert_eq!(progress.set_range(5, 5), (0, 100));
        assert_eq!(progress.range(), (5, 6));
        assert_eq!(progress.position(), 5);
        progress.set_range(0, 100);
        progress.set_position(95);
        assert_eq!(progress.step_it(), 95);
        assert_eq!(progress.position(), 0);
        assert_eq!(progress.set_position(250), 0);
        assert_eq!(progress.position(), 100);
        assert_eq!(progress.delta(-30), 100);
        assert_eq!(progress.position(), 70);
        assert_eq!(progress.set_step(25), 10);
        progress.step_it();
        assert_eq!(progress.position(), 95);
    }

    #[test]
    fn vertical_bars_grow_upwards_and_the_marquee_stays_in_the_middle_third() {
        let rect = Rect::sized(0, 0, 20, 102);
        assert_eq!(
            bar_at(50).fill_rect(rect, style::VERTICAL),
            Rect::sized(1, 51, 18, 50)
        );
        let mut marquee = Progress::new();
        assert!(marquee.fill_rect(BAR, style::MARQUEE).is_empty());
        marquee.marquee = true;
        assert_eq!(
            marquee.fill_rect(BAR, style::MARQUEE),
            Rect::sized(9 + 140, 283, 140, 18)
        );
        assert_eq!(
            marquee.fill_rect(rect, style::MARQUEE | style::VERTICAL),
            Rect::sized(1, 34, 18, 33)
        );
    }

    #[test]
    fn states_colour_the_fill_unless_the_program_chose_a_colour() {
        let mut progress = bar_at(50);
        assert_eq!(progress.fill_colour(&COMMON_LIGHT), 0x0006_b025);
        assert_eq!(progress.set_state(state::ERROR), state::NORMAL);
        assert_eq!(progress.fill_colour(&COMMON_LIGHT), 0x00e8_1123);
        progress.set_state(state::PAUSED);
        assert_eq!(progress.fill_colour(&COMMON_LIGHT), 0x00ff_b900);
        assert_eq!(progress.set_bar_colour(0x0000_00ff), DEFAULT_COLOUR);
        assert_eq!(progress.fill_colour(&COMMON_LIGHT), 0x00ff_0000);
        assert_eq!(progress.set_state(9), 0);
    }
}

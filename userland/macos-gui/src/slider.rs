//! NSSlider's value and look (D679): a horizontal slider drawn as the toolkit's, its track and
//! thumb without the readout, the value a double between `minValue` and `maxValue`, snapped to
//! the tick marks when `allowsTickMarkValuesOnly` says so. The thumb moves in [`STEPS`] steps.

use crate::geometry::Frame;
use crate::paint::control_rect;
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::widget::{self, Kind, Look};

/// The positions the thumb takes across the track.
pub const STEPS: i32 = 1000;

/// A slider's range, value and tick marks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderValue {
    pub min: f64,
    pub max: f64,
    pub value: f64,
    pub ticks: usize,
    pub ticks_only: bool,
}

impl Default for SliderValue {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            value: 0.0,
            ticks: 0,
            ticks_only: false,
        }
    }
}

impl SliderValue {
    /// `value` kept within the range, moved to the nearest tick when only ticks are allowed.
    pub fn settled(&self, value: f64) -> f64 {
        let (low, high) = if self.min <= self.max { (self.min, self.max) } else { (self.max, self.min) };
        let value = if value.is_nan() { low } else { value.clamp(low, high) };
        if !self.ticks_only || self.ticks < 2 || high == low {
            return value;
        }
        let gap = (high - low) / (self.ticks - 1) as f64;
        let nearest = ((value - low) / gap + 0.5) as usize;
        low + nearest.min(self.ticks - 1) as f64 * gap
    }

    /// The thumb's position, 0 to [`STEPS`].
    pub fn position(&self) -> i32 {
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0;
        }
        (((self.value - self.min) / span) * f64::from(STEPS) + 0.5).clamp(0.0, f64::from(STEPS)) as i32
    }

    /// The value the thumb at `position` (0 to [`STEPS`]) stands for.
    pub fn value_at_position(&self, position: i32) -> f64 {
        let fraction = f64::from(position.clamp(0, STEPS)) / f64::from(STEPS);
        self.settled(self.min + (self.max - self.min) * fraction)
    }

    fn toolkit(&self) -> widget::Slider {
        let Kind::Slider(mut slider) = Kind::slider("", 0, STEPS, 1, self.position(), "") else {
            unreachable!("Kind::slider makes a slider")
        };
        slider.readout = false;
        slider
    }

    /// The value a press or drag at window x `x` sets on a slider at `frame` in a window
    /// `height` tall.
    pub fn value_at(&self, frame: Frame, height: i32, x: i32) -> f64 {
        let rect = control_rect(frame, height);
        self.value_at_position(self.toolkit().value_at(rect, x))
    }
}

/// Draws the slider `value` at `frame` in a window `height` tall.
pub fn paint(canvas: &mut Canvas<'_>, height: i32, frame: Frame, value: &SliderValue, look: Look) {
    let rect = control_rect(frame, height);
    widget::paint(canvas, &Kind::Slider(value.toolkit()), rect, look);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_clamped_and_snapped_to_ticks_when_asked() {
        let mut slider = SliderValue {
            min: 0.0,
            max: 100.0,
            value: 25.0,
            ticks: 5,
            ticks_only: false,
        };
        assert_eq!(slider.position(), 250);
        assert_eq!(slider.settled(130.0), 100.0);
        assert_eq!(slider.settled(33.0), 33.0);
        slider.ticks_only = true;
        assert_eq!(slider.settled(33.0), 25.0);
        assert_eq!(slider.settled(40.0), 50.0);
        assert_eq!(slider.value_at_position(1000), 100.0);
        assert_eq!(SliderValue::default().settled(f64::NAN), 0.0);
    }

    #[test]
    fn the_thumb_follows_the_pointer_across_the_track() {
        let slider = SliderValue::default();
        let frame = Frame::new(20.0, 20.0, 200.0, 24.0);
        let left = slider.value_at(frame, 100, 0);
        let right = slider.value_at(frame, 100, 400);
        assert_eq!(left, 0.0);
        assert_eq!(right, 1.0);
    }
}

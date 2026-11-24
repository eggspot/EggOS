//! What CoreGraphics paints besides paths (D739): gradients along a line or between two
//! circles, with their colour stops and extension past either end, and the loop that paints
//! every pixel of an area through the canvas's clip and clip masks.

use crate::geometry::{Point, point_of_pixel};
use crate::path::ClipMask;
use alloc::vec::Vec;
use eggos_math::sqrt;
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;

/// A colour stop: where along the gradient, from 0 to 1, and its red, green, blue and alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub location: f64,
    pub colour: [f64; 4],
}

/// A `CGGradient`: colour stops in increasing order of location.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gradient {
    stops: Vec<Stop>,
}

impl Gradient {
    /// The gradient through `colours` at `locations`, or spread evenly from 0 to 1 when there
    /// are none.
    pub fn new(colours: &[[f64; 4]], locations: Option<&[f64]>) -> Self {
        let last = colours.len().saturating_sub(1).max(1) as f64;
        let mut stops: Vec<Stop> = colours
            .iter()
            .enumerate()
            .map(|(index, &colour)| Stop {
                location: locations
                    .and_then(|places| places.get(index).copied())
                    .unwrap_or(index as f64 / last)
                    .clamp(0.0, 1.0),
                colour: colour.map(|channel| channel.clamp(0.0, 1.0)),
            })
            .collect();
        stops.sort_by(|a, b| a.location.total_cmp(&b.location));
        Self { stops }
    }

    pub fn stops(&self) -> &[Stop] {
        &self.stops
    }

    /// The colour at `t` from 0 to 1, between the stops around it.
    pub fn colour_at(&self, t: f64) -> [f64; 4] {
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else {
            return [0.0; 4];
        };
        if t <= first.location {
            return first.colour;
        }
        if t >= last.location {
            return last.colour;
        }
        let after = self.stops.iter().position(|stop| stop.location >= t).unwrap_or(0);
        let (low, high) = (self.stops[after - 1], self.stops[after]);
        let span = high.location - low.location;
        let share = match span > 0.0 {
            true => (t - low.location) / span,
            false => 1.0,
        };
        core::array::from_fn(|channel| {
            low.colour[channel] + (high.colour[channel] - low.colour[channel]) * share
        })
    }
}

/// Whether a gradient goes on past its start or end with the colour there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Extend {
    pub before_start: bool,
    pub after_end: bool,
}

impl Extend {
    /// `kCGGradientDrawsBeforeStartLocation` is 1, `kCGGradientDrawsAfterEndLocation` 2.
    pub fn from_options(options: u32) -> Self {
        Self {
            before_start: options & 1 != 0,
            after_end: options & 2 != 0,
        }
    }

    fn allows(&self, t: f64) -> bool {
        (t >= 0.0 || self.before_start) && (t <= 1.0 || self.after_end)
    }
}

/// Where a gradient's colours change, in the same space as the points asked about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shading {
    Linear { start: Point, end: Point },
    Radial { start: Point, start_radius: f64, end: Point, end_radius: f64 },
}

impl Shading {
    /// Where `point` falls along the gradient, from 0 to 1, or `None` where it paints nothing.
    pub fn parameter(&self, point: Point, extend: Extend) -> Option<f64> {
        let t = match *self {
            Shading::Linear { start, end } => {
                let (dx, dy) = (end.x - start.x, end.y - start.y);
                let length = dx * dx + dy * dy;
                if length == 0.0 {
                    return None;
                }
                ((point.x - start.x) * dx + (point.y - start.y) * dy) / length
            }
            Shading::Radial { start, start_radius, end, end_radius } => {
                radial_parameter(point, (start, start_radius), (end, end_radius), extend)?
            }
        };
        extend.allows(t).then_some(t.clamp(0.0, 1.0))
    }
}

fn radial_parameter(
    point: Point,
    (start, start_radius): (Point, f64),
    (end, end_radius): (Point, f64),
    extend: Extend,
) -> Option<f64> {
    let (cx, cy) = (end.x - start.x, end.y - start.y);
    let (px, py) = (point.x - start.x, point.y - start.y);
    let dr = end_radius - start_radius;
    let a = cx * cx + cy * cy - dr * dr;
    let b = px * cx + py * cy + start_radius * dr;
    let c = px * px + py * py - start_radius * start_radius;
    let usable = |t: f64| start_radius + t * dr >= 0.0 && extend.allows(t);
    if a.abs() < 1e-12 {
        if b == 0.0 {
            return None;
        }
        let t = c / (2.0 * b);
        return usable(t).then_some(t);
    }
    let reach = b * b - a * c;
    if reach < 0.0 {
        return None;
    }
    let root = sqrt(reach);
    let (first, second) = ((b + root) / a, (b - root) / a);
    let (high, low) = match first >= second {
        true => (first, second),
        false => (second, first),
    };
    [high, low].into_iter().find(|&t| usable(t))
}

/// The premultiplied pixel for `colour` (red, green, blue, alpha from 0 to 1) at `alpha`.
pub fn premultiply(colour: [f64; 4], alpha: f64) -> u32 {
    let strength = (colour[3] * alpha).clamp(0.0, 1.0);
    let byte = |value: f64| (value.clamp(0.0, 1.0) * strength * 255.0 + 0.5) as u32;
    (byte(1.0) << 24) | (byte(colour[0]) << 16) | (byte(colour[1]) << 8) | byte(colour[2])
}

/// Scales a premultiplied pixel by `share` out of 255.
pub fn faded(pixel: u32, share: u32) -> u32 {
    let channel = |shift: u32| ((pixel >> shift & 0xff) * share + 127) / 255;
    (channel(24) << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Blends over every pixel of `area` (or the whole canvas) that the canvas's clip and each of
/// `masks` let through, the premultiplied pixel `paint` gives for the point at its centre on a
/// content `height` tall, faded by the masks.
pub fn paint_area(
    canvas: &mut Canvas<'_>,
    height: i32,
    area: Option<Rect>,
    masks: &[&ClipMask],
    paint: impl Fn(Point) -> Option<u32>,
) {
    let mut bounds = match area {
        Some(area) => canvas.clip().intersection(&area),
        None => canvas.clip(),
    };
    for mask in masks {
        bounds = bounds.intersection(&mask.area());
    }
    let mut span: Vec<u32> = Vec::with_capacity(bounds.width.max(0) as usize);
    for row in bounds.y..bounds.y + bounds.height {
        span.clear();
        for column in bounds.x..bounds.x + bounds.width {
            let centre = point_of_pixel(column, row, height);
            let mut pixel = paint(centre).unwrap_or(0);
            for mask in masks {
                pixel = faded(pixel, u32::from(mask.at(column, row)));
            }
            span.push(pixel);
        }
        if !span.is_empty() {
            canvas.composite((bounds.x, row), &span, span.len(), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [f64; 4] = [0.0, 0.0, 0.0, 1.0];
    const WHITE: [f64; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn colours_blend_between_stops() {
        let gradient = Gradient::new(&[BLACK, WHITE], None);
        assert_eq!(gradient.colour_at(0.5), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(gradient.colour_at(-1.0), BLACK);
        assert_eq!(gradient.colour_at(2.0), WHITE);
        let placed = Gradient::new(&[WHITE, BLACK], Some(&[0.8, 0.2][..]));
        assert_eq!(placed.stops()[0].colour, BLACK);
        assert_eq!(placed.colour_at(0.1), BLACK);
    }

    #[test]
    fn a_linear_gradient_stops_at_its_ends_unless_extended() {
        let shading = Shading::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(10.0, 0.0),
        };
        let none = Extend::default();
        assert_eq!(shading.parameter(Point::new(5.0, 3.0), none), Some(0.5));
        assert_eq!(shading.parameter(Point::new(-1.0, 0.0), none), None);
        let both = Extend::from_options(3);
        assert_eq!(shading.parameter(Point::new(-1.0, 0.0), both), Some(0.0));
        assert_eq!(shading.parameter(Point::new(12.0, 0.0), both), Some(1.0));
    }

    #[test]
    fn a_radial_gradient_grows_from_its_start_circle() {
        let shading = Shading::Radial {
            start: Point::new(0.0, 0.0),
            start_radius: 0.0,
            end: Point::new(0.0, 0.0),
            end_radius: 10.0,
        };
        let none = Extend::default();
        assert_eq!(shading.parameter(Point::new(5.0, 0.0), none), Some(0.5));
        assert_eq!(shading.parameter(Point::new(0.0, 15.0), none), None);
        assert_eq!(shading.parameter(Point::new(0.0, 15.0), Extend::from_options(2)), Some(1.0));
    }

    #[test]
    fn premultiplied_pixels_carry_their_alpha() {
        assert_eq!(premultiply(WHITE, 1.0), 0xffff_ffff);
        assert_eq!(premultiply([1.0, 0.0, 0.0, 0.5], 1.0), 0x8080_0000);
        assert_eq!(faded(0xffff_ffff, 0), 0);
    }
}

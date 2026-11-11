//! Paths as CoreGraphics builds them (D386): moves, lines, arcs, curves, rectangles and
//! ellipses flattened to polygons in AppKit's coordinates under an affine transform, filled by
//! scanlines with the non-zero or even-odd rule and stroked as lines (wider strokes as filled
//! quadrilaterals), clipped to the canvas.

use crate::colour::Colour;
use crate::geometry::{Point, pixel_of_point};
use alloc::vec::Vec;
use eggos_math::{atan2, ceil, cos, fabs, floor, sin, sqrt};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;

const ARC_SEGMENTS_PER_TURN: f64 = 64.0;
const CURVE_SEGMENTS: usize = 16;
const ELLIPSE_SEGMENTS: usize = 48;
const TAU: f64 = core::f64::consts::TAU;
const MAX_POINTS: usize = 1 << 16;

/// An affine transform: `x' = a x + c y + tx`, `y' = b x + d y + ty`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn apply(&self, point: Point) -> Point {
        Point::new(
            self.a * point.x + self.c * point.y + self.tx,
            self.b * point.x + self.d * point.y + self.ty,
        )
    }

    /// This transform after `first`: points go through `first`, then this.
    pub fn after(&self, first: &Transform) -> Transform {
        Transform {
            a: self.a * first.a + self.c * first.b,
            b: self.b * first.a + self.d * first.b,
            c: self.a * first.c + self.c * first.d,
            d: self.b * first.c + self.d * first.d,
            tx: self.a * first.tx + self.c * first.ty + self.tx,
            ty: self.b * first.tx + self.d * first.ty + self.ty,
        }
    }

    pub fn translated(&self, x: f64, y: f64) -> Transform {
        self.after(&Transform {
            tx: x,
            ty: y,
            ..Transform::IDENTITY
        })
    }

    pub fn scaled(&self, x: f64, y: f64) -> Transform {
        self.after(&Transform {
            a: x,
            d: y,
            ..Transform::IDENTITY
        })
    }

    pub fn rotated(&self, angle: f64) -> Transform {
        let (s, c) = (sin(angle), cos(angle));
        self.after(&Transform {
            a: c,
            b: s,
            c: -s,
            d: c,
            ..Transform::IDENTITY
        })
    }
}

/// A path flattened to polygons, its points already transformed.
#[derive(Debug, Clone, Default)]
pub struct Path {
    points: Vec<Point>,
    starts: Vec<usize>,
    closed: Vec<bool>,
    current: Option<Point>,
    start: Option<Point>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    fn push(&mut self, point: Point) {
        if self.points.len() < MAX_POINTS {
            self.points.push(point);
        }
    }

    /// The point the path is at, transformed.
    pub fn current(&self) -> Option<Point> {
        self.current
    }

    pub fn move_to(&mut self, point: Point) {
        self.starts.push(self.points.len());
        self.closed.push(false);
        self.push(point);
        self.current = Some(point);
        self.start = Some(point);
    }

    pub fn line_to(&mut self, point: Point) {
        if self.current.is_none() {
            return self.move_to(point);
        }
        self.push(point);
        self.current = Some(point);
    }

    pub fn close(&mut self) {
        if let Some(last) = self.closed.last_mut() {
            *last = true;
        }
        self.current = self.start;
    }

    /// A cubic curve from the current point through `first` and `second` to `end`.
    pub fn curve_to(&mut self, first: Point, second: Point, end: Point) {
        let Some(from) = self.current else {
            return self.move_to(end);
        };
        for step in 1..=CURVE_SEGMENTS {
            let t = step as f64 / CURVE_SEGMENTS as f64;
            let u = 1.0 - t;
            let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let points = [from, first, second, end];
            let x = points.iter().zip(weights).map(|(p, w)| p.x * w).sum();
            let y = points.iter().zip(weights).map(|(p, w)| p.y * w).sum();
            self.line_to(Point::new(x, y));
        }
    }

    /// An arc of the circle at `centre` with `radius` from angle `start` to `end` (radians),
    /// clockwise or not, joined to the current point by a line, all under `transform`.
    pub fn arc(&mut self, transform: &Transform, centre: Point, radius: f64, (start, end): (f64, f64), clockwise: bool) {
        let mut sweep = end - start;
        if clockwise && sweep > 0.0 {
            sweep -= TAU * ((sweep / TAU) as i64 + 1) as f64;
        } else if !clockwise && sweep < 0.0 {
            sweep += TAU * ((-sweep / TAU) as i64 + 1) as f64;
        }
        let segments = ((fabs(sweep) / TAU * ARC_SEGMENTS_PER_TURN) as usize).max(1);
        for step in 0..=segments {
            let angle = start + sweep * step as f64 / segments as f64;
            let point = transform.apply(Point::new(centre.x + radius * cos(angle), centre.y + radius * sin(angle)));
            match step == 0 && self.current.is_none() {
                true => self.move_to(point),
                false => self.line_to(point),
            }
        }
    }

    /// The arc `CGContextAddArcToPoint` draws: tangent to the lines from `untransformed` (the
    /// current point before the transform) to `corner` and from `corner` to `towards`, with
    /// `radius`, under `transform`.
    pub fn arc_to(&mut self, transform: &Transform, untransformed: Point, corner: Point, towards: Point, radius: f64) {
        let (ax, ay) = (untransformed.x - corner.x, untransformed.y - corner.y);
        let (bx, by) = (towards.x - corner.x, towards.y - corner.y);
        let (la, lb) = (sqrt(ax * ax + ay * ay), sqrt(bx * bx + by * by));
        if la == 0.0 || lb == 0.0 || radius <= 0.0 {
            self.line_to(transform.apply(corner));
            return;
        }
        let (ux, uy, vx, vy) = (ax / la, ay / la, bx / lb, by / lb);
        let cross = ux * vy - uy * vx;
        let dot = ux * vx + uy * vy;
        let half = atan2(fabs(cross), dot) / 2.0;
        let tangent = radius / tan_of(half);
        if !tangent.is_finite() {
            self.line_to(transform.apply(corner));
            return;
        }
        let first = Point::new(corner.x + ux * tangent, corner.y + uy * tangent);
        let (nx, ny) = if cross > 0.0 { (-uy, ux) } else { (uy, -ux) };
        let centre = Point::new(first.x + nx * radius, first.y + ny * radius);
        let second = Point::new(corner.x + vx * tangent, corner.y + vy * tangent);
        let start = atan2(first.y - centre.y, first.x - centre.x);
        let end = atan2(second.y - centre.y, second.x - centre.x);
        self.line_to(transform.apply(first));
        self.arc(transform, centre, radius, (start, end), cross > 0.0);
    }

    pub fn rect(&mut self, transform: &Transform, (x, y, width, height): (f64, f64, f64, f64)) {
        self.move_to(transform.apply(Point::new(x, y)));
        self.line_to(transform.apply(Point::new(x + width, y)));
        self.line_to(transform.apply(Point::new(x + width, y + height)));
        self.line_to(transform.apply(Point::new(x, y + height)));
        self.close();
    }

    pub fn ellipse(&mut self, transform: &Transform, (x, y, width, height): (f64, f64, f64, f64)) {
        let (cx, cy, rx, ry) = (x + width / 2.0, y + height / 2.0, width / 2.0, height / 2.0);
        for step in 0..=ELLIPSE_SEGMENTS {
            let angle = TAU * step as f64 / ELLIPSE_SEGMENTS as f64;
            let point = transform.apply(Point::new(cx + rx * cos(angle), cy + ry * sin(angle)));
            match step {
                0 => self.move_to(point),
                _ => self.line_to(point),
            }
        }
        self.close();
    }

    fn subpaths(&self) -> impl Iterator<Item = (&[Point], bool)> {
        self.starts.iter().enumerate().map(|(index, &start)| {
            let end = self.starts.get(index + 1).copied().unwrap_or(self.points.len());
            (&self.points[start.min(end)..end], self.closed[index])
        })
    }

    /// Fills the path in `colour` on `canvas` (content `height` pixels tall), with the
    /// even-odd rule when `even_odd`, else non-zero; every subpath is closed for filling.
    pub fn fill(&self, canvas: &mut Canvas<'_>, height: i32, colour: Colour, even_odd: bool) {
        if !colour.visible || self.points.is_empty() {
            return;
        }
        let clip = canvas.clip();
        let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
        let (mut top, mut bottom) = (f64::MAX, f64::MIN);
        for (points, _) in self.subpaths() {
            let count = points.len();
            for index in 0..count {
                let from = points[index];
                let to = points[(index + 1) % count];
                let (y0, y1) = (f64::from(height) - from.y, f64::from(height) - to.y);
                if y0 == y1 {
                    continue;
                }
                let direction = if y0 < y1 { 1 } else { -1 };
                let (upper, lower) = if y0 < y1 { ((from.x, y0), (to.x, y1)) } else { ((to.x, y1), (from.x, y0)) };
                edges.push((upper.0, upper.1, lower.0, lower.1, direction));
                top = top.min(upper.1);
                bottom = bottom.max(lower.1);
            }
        }
        let first_row = (floor(top) as i32).max(clip.y);
        let last_row = (ceil(bottom) as i32).min(clip.bottom());
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for row in first_row..last_row {
            let y = f64::from(row) + 0.5;
            crossings.clear();
            for &(x0, y0, x1, y1, direction) in &edges {
                if y >= y0 && y < y1 {
                    crossings.push((x0 + (y - y0) * (x1 - x0) / (y1 - y0), direction));
                }
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                let inside = match even_odd {
                    true => winding % 2 != 0,
                    false => winding != 0,
                };
                if inside {
                    let left = ceil(pair[0].0 - 0.5) as i32;
                    let right = ceil(pair[1].0 - 0.5) as i32;
                    if right > left {
                        canvas.fill(Rect::new(left, row, right - left, 1), colour.pixel);
                    }
                }
            }
        }
    }

    /// Strokes the path in `colour`, `width` wide (1 pixel at most draws plain lines).
    pub fn stroke(&self, canvas: &mut Canvas<'_>, height: i32, colour: Colour, width: f64) {
        if !colour.visible {
            return;
        }
        for (points, closed) in self.subpaths() {
            let count = points.len();
            let segments = match closed {
                true => count,
                false => count.saturating_sub(1),
            };
            for index in 0..segments {
                let (from, to) = (points[index], points[(index + 1) % count]);
                if width <= 1.0 {
                    crate::paint::line(canvas, height, from, to, colour);
                    continue;
                }
                let (dx, dy) = (to.x - from.x, to.y - from.y);
                let length = sqrt(dx * dx + dy * dy);
                if length == 0.0 {
                    continue;
                }
                let (nx, ny) = (-dy / length * width / 2.0, dx / length * width / 2.0);
                let mut quad = Path::new();
                quad.move_to(Point::new(from.x + nx, from.y + ny));
                quad.line_to(Point::new(to.x + nx, to.y + ny));
                quad.line_to(Point::new(to.x - nx, to.y - ny));
                quad.line_to(Point::new(from.x - nx, from.y - ny));
                quad.close();
                quad.fill(canvas, height, colour, false);
            }
        }
    }

    /// Moves every point by (`x`, `y`).
    pub fn offset(&mut self, x: f64, y: f64) {
        for point in &mut self.points {
            point.x += x;
            point.y += y;
        }
        if let Some(current) = self.current.as_mut() {
            current.x += x;
            current.y += y;
        }
    }

    /// The pixel bounds of the path's points on a content `height` pixels tall.
    pub fn pixel_bounds(&self, height: i32) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for &point in &self.points {
            let (x, y) = pixel_of_point(point, height);
            let pixel = Rect::new(x, y, 1, 1);
            bounds = Some(bounds.map_or(pixel, |rect| rect.union(&pixel)));
        }
        bounds
    }
}

fn tan_of(angle: f64) -> f64 {
    sin(angle) / cos(angle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_compose_in_order() {
        let moved = Transform::IDENTITY.translated(10.0, 0.0).scaled(2.0, 2.0);
        assert_eq!(moved.apply(Point::new(1.0, 1.0)), Point::new(12.0, 2.0));
    }

    #[test]
    fn a_rect_fills_its_pixels() {
        let mut pixels = alloc::vec![0u32; 20 * 20];
        let mut canvas = Canvas::new(&mut pixels, 20, 20, 1).unwrap();
        let mut path = Path::new();
        path.rect(&Transform::IDENTITY, (2.0, 2.0, 4.0, 3.0));
        path.fill(&mut canvas, 20, Colour::opaque(0x00ff_0000), false);
        let filled = pixels.iter().filter(|&&pixel| pixel == 0x00ff_0000).count();
        assert_eq!(filled, 12);
        assert_eq!(pixels[(20 - 3) * 20 + 2], 0x00ff_0000);
    }

    #[test]
    fn a_full_clockwise_arc_comes_back_to_its_start() {
        let mut path = Path::new();
        path.arc(&Transform::IDENTITY, Point::new(0.0, 0.0), 10.0, (0.0, TAU), false);
        let last = path.current().unwrap();
        assert!((last.x - 10.0).abs() < 1e-9 && last.y.abs() < 1e-9);
    }
}

//! Paths as CoreGraphics builds them (D386): moves, lines, arcs, curves, rectangles and
//! ellipses flattened to polygons in AppKit's coordinates under an affine transform, filled by
//! scanlines with the non-zero or even-odd rule, clipped to the canvas. Fills are anti-aliased
//! (four sub-scanlines a pixel, exact coverage across), blended at the colour's alpha and
//! limited by a clip mask, and strokes are outlined with CoreGraphics' caps, joins, miter limit
//! and dashes, then filled the same way (D738).

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
const SUBSAMPLES: usize = 4;
const FULL_COVER: u32 = 256;
const SUBSAMPLE_WEIGHT: f64 = (FULL_COVER as usize / SUBSAMPLES) as f64;
const ROUND_SEGMENTS: usize = 24;
const STRAIGHT: f64 = 1e-9;

/// How a stroke's open ends look (`CGLineCap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

/// How a stroke turns at a corner (`CGLineJoin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// What a stroke looks like: its width, ends, corners, miter limit and dash pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle<'a> {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub dashes: &'a [f64],
    pub dash_phase: f64,
}

impl StrokeStyle<'_> {
    /// CoreGraphics' defaults: 1 wide, butt ends, miter corners with a limit of 10, no dashes.
    pub const PLAIN: StrokeStyle<'static> = StrokeStyle {
        width: 1.0,
        cap: Cap::Butt,
        join: Join::Miter,
        miter_limit: 10.0,
        dashes: &[],
        dash_phase: 0.0,
    };
}

/// A clip: how much of each pixel of `area` drawing may cover, 0 to 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipMask {
    area: Rect,
    coverage: Vec<u8>,
}

impl ClipMask {
    /// A clip letting all of `area` through.
    pub fn rect(area: Rect) -> Self {
        let count = (area.width.max(0) as usize) * (area.height.max(0) as usize);
        Self {
            area,
            coverage: alloc::vec![u8::MAX; count],
        }
    }

    /// The pixels it may let through.
    pub fn area(&self) -> Rect {
        self.area
    }

    /// How much of pixel (`x`, `y`) it lets through.
    pub fn at(&self, x: i32, y: i32) -> u8 {
        let area = self.area;
        if x < area.x || y < area.y || x >= area.right() || y >= area.bottom() {
            return 0;
        }
        let index = (y - area.y) as usize * area.width as usize + (x - area.x) as usize;
        self.coverage.get(index).copied().unwrap_or(0)
    }

    /// Keeps only what `path` (on a content `height` tall) also covers.
    pub fn intersect_path(&mut self, path: &Path, height: i32, even_odd: bool) {
        let mut kept = alloc::vec![0u8; self.coverage.len()];
        let area = self.area;
        path.scan(height, even_odd, area, |row, left, cover| {
            for (index, &amount) in cover.iter().enumerate() {
                let x = left + index as i32;
                let own = u32::from(self.at(x, row));
                let combined = own * amount.min(FULL_COVER as u16) as u32 / FULL_COVER;
                let at = (row - area.y) as usize * area.width as usize + (x - area.x) as usize;
                if let Some(slot) = kept.get_mut(at) {
                    *slot = combined as u8;
                }
            }
        });
        self.coverage = kept;
    }
}

fn premultiplied(pixel: u32, alpha: u32) -> u32 {
    let channel = |shift: u32| (((pixel >> shift) & 0xff) * alpha + 127) / 255;
    (alpha << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn add_span(cover: &mut [u16], left: i32, (from, to): (f64, f64), weight: f64) {
    let right = left + cover.len() as i32;
    let (from, to) = (from.max(f64::from(left)), to.min(f64::from(right)));
    if to <= from {
        return;
    }
    let (first, last) = (floor(from) as i32, floor(to) as i32);
    let mut add = |pixel: i32, amount: f64| {
        if let Some(slot) = cover.get_mut((pixel - left) as usize) {
            *slot = slot.saturating_add((amount * weight + 0.5) as u16);
        }
    };
    if first == last {
        add(first, to - from);
        return;
    }
    add(first, f64::from(first + 1) - from);
    for pixel in first + 1..last {
        add(pixel, 1.0);
    }
    if last < right {
        add(last, to - f64::from(last));
    }
}

fn signed_area(points: &[Point]) -> f64 {
    let count = points.len();
    (0..count)
        .map(|index| {
            let (a, b) = (points[index], points[(index + 1) % count]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

fn add_polygon(out: &mut Path, points: &[Point]) {
    if points.len() < 3 {
        return;
    }
    let forward = signed_area(points) >= 0.0;
    let mut ordered = points.iter().copied();
    let mut reversed = points.iter().rev().copied();
    let walk: &mut dyn Iterator<Item = Point> = match forward {
        true => &mut ordered,
        false => &mut reversed,
    };
    if let Some(first) = walk.next() {
        out.move_to(first);
        for point in walk {
            out.line_to(point);
        }
        out.close();
    }
}

fn add_disc(out: &mut Path, centre: Point, radius: f64) {
    let mut points = [Point::new(0.0, 0.0); ROUND_SEGMENTS];
    for (index, slot) in points.iter_mut().enumerate() {
        let angle = TAU * index as f64 / ROUND_SEGMENTS as f64;
        *slot = Point::new(centre.x + radius * cos(angle), centre.y + radius * sin(angle));
    }
    add_polygon(out, &points);
}

fn direction(from: Point, to: Point) -> Option<(f64, f64)> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = sqrt(dx * dx + dy * dy);
    (length > STRAIGHT).then(|| (dx / length, dy / length))
}

fn shifted(point: Point, (x, y): (f64, f64), by: f64) -> Point {
    Point::new(point.x + x * by, point.y + y * by)
}

type Direction = (f64, f64);

fn add_join(
    out: &mut Path,
    at: Point,
    (d1, d2): (Direction, Direction),
    half: f64,
    style: &StrokeStyle<'_>,
) {
    let cross = d1.0 * d2.1 - d1.1 * d2.0;
    if fabs(cross) <= STRAIGHT && d1.0 * d2.0 + d1.1 * d2.1 > 0.0 {
        return;
    }
    let outer = if cross > 0.0 { -1.0 } else { 1.0 };
    let n1 = (-d1.1 * outer, d1.0 * outer);
    let n2 = (-d2.1 * outer, d2.0 * outer);
    match style.join {
        Join::Round => add_disc(out, at, half),
        Join::Bevel => add_polygon(out, &[at, shifted(at, n1, half), shifted(at, n2, half)]),
        Join::Miter => {
            let (mx, my) = (n1.0 + n2.0, n1.1 + n2.1);
            let length = sqrt(mx * mx + my * my);
            let cosine = length / 2.0;
            let reach = match cosine > STRAIGHT {
                true => 1.0 / cosine,
                false => f64::MAX,
            };
            if reach > style.miter_limit.max(1.0) {
                add_polygon(out, &[at, shifted(at, n1, half), shifted(at, n2, half)]);
                return;
            }
            let tip = shifted(at, (mx / length, my / length), half * reach);
            add_polygon(
                out,
                &[at, shifted(at, n1, half), tip, shifted(at, n2, half)],
            );
        }
    }
}

fn add_cap(out: &mut Path, at: Point, outward: (f64, f64), half: f64, cap: Cap) {
    let normal = (-outward.1, outward.0);
    match cap {
        Cap::Butt => {}
        Cap::Round => add_disc(out, at, half),
        Cap::Square => {
            let ahead = shifted(at, outward, half);
            add_polygon(
                out,
                &[
                    shifted(at, normal, half),
                    shifted(ahead, normal, half),
                    shifted(ahead, normal, -half),
                    shifted(at, normal, -half),
                ],
            );
        }
    }
}

fn outline_polyline(
    out: &mut Path,
    line: &[Point],
    closed: bool,
    half: f64,
    style: &StrokeStyle<'_>,
) {
    let mut points: Vec<Point> = Vec::with_capacity(line.len());
    for &point in line {
        if points.last().is_none_or(|last| direction(*last, point).is_some()) {
            points.push(point);
        }
    }
    if closed && points.len() > 2 && direction(points[points.len() - 1], points[0]).is_none() {
        points.pop();
    }
    if points.len() == 1 {
        match style.cap {
            Cap::Round => add_disc(out, points[0], half),
            Cap::Square => {
                let at = points[0];
                let corner = |x: f64, y: f64| Point::new(at.x + x * half, at.y + y * half);
                add_polygon(
                    out,
                    &[
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ],
                );
            }
            Cap::Butt => {}
        }
        return;
    }
    let count = points.len();
    let segments = if closed { count } else { count - 1 };
    for index in 0..segments {
        let (a, b) = (points[index], points[(index + 1) % count]);
        let Some(along) = direction(a, b) else {
            continue;
        };
        let normal = (-along.1, along.0);
        add_polygon(
            out,
            &[
                shifted(a, normal, half),
                shifted(b, normal, half),
                shifted(b, normal, -half),
                shifted(a, normal, -half),
            ],
        );
    }
    let corners = if closed { 0..count } else { 1..count - 1 };
    for index in corners {
        let previous = points[(index + count - 1) % count];
        let next = points[(index + 1) % count];
        let at = points[index];
        if let (Some(d1), Some(d2)) = (direction(previous, at), direction(at, next)) {
            add_join(out, at, (d1, d2), half, style);
        }
    }
    if !closed {
        if let Some(back) = direction(points[1], points[0]) {
            add_cap(out, points[0], back, half, style.cap);
        }
        if let Some(ahead) = direction(points[count - 2], points[count - 1]) {
            add_cap(out, points[count - 1], ahead, half, style.cap);
        }
    }
}

fn dashed(points: &[Point], closed: bool, dashes: &[f64], phase: f64) -> Vec<(Vec<Point>, bool)> {
    let usable = !dashes.is_empty()
        && dashes.iter().all(|&length| length >= 0.0)
        && dashes.iter().sum::<f64>() > 0.0;
    if !usable {
        return alloc::vec![(points.to_vec(), closed)];
    }
    let mut pieces: Vec<(Vec<Point>, bool)> = Vec::new();
    let total: f64 = dashes.iter().sum();
    let mut index = 0;
    let mut left = dashes[0];
    let mut skip = phase.rem_euclid(total * if dashes.len() % 2 == 1 { 2.0 } else { 1.0 });
    while skip > 0.0 {
        let step = skip.min(left);
        skip -= step;
        left -= step;
        if left <= 0.0 {
            index = (index + 1) % dashes.len();
            left = dashes[index];
        }
    }
    let count = points.len();
    let segments = if closed { count } else { count.saturating_sub(1) };
    let mut current: Vec<Point> = Vec::new();
    let mut on = index % 2 == 0;
    if on && count > 0 {
        current.push(points[0]);
    }
    for segment in 0..segments {
        let (a, b) = (points[segment], points[(segment + 1) % count]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = sqrt(dx * dx + dy * dy);
        let mut walked = 0.0;
        while length - walked > left {
            walked += left;
            let at = Point::new(a.x + dx * walked / length, a.y + dy * walked / length);
            if on {
                current.push(at);
                pieces.push((core::mem::take(&mut current), false));
            } else {
                current.push(at);
            }
            on = !on;
            if !on {
                current.clear();
            }
            index = (index + 1) % dashes.len();
            left = dashes[index];
        }
        left -= length - walked;
        if on {
            current.push(b);
        }
    }
    if on && current.len() > 1 {
        pieces.push((current, false));
    }
    pieces
}

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
    pub fn arc(
        &mut self,
        transform: &Transform,
        centre: Point,
        radius: f64,
        (start, end): (f64, f64),
        clockwise: bool,
    ) {
        let mut sweep = end - start;
        if clockwise && sweep > 0.0 {
            sweep -= TAU * ((sweep / TAU) as i64 + 1) as f64;
        } else if !clockwise && sweep < 0.0 {
            sweep += TAU * ((-sweep / TAU) as i64 + 1) as f64;
        }
        let segments = ((fabs(sweep) / TAU * ARC_SEGMENTS_PER_TURN) as usize).max(1);
        for step in 0..=segments {
            let angle = start + sweep * step as f64 / segments as f64;
            let point = transform.apply(Point::new(
                centre.x + radius * cos(angle),
                centre.y + radius * sin(angle),
            ));
            match step == 0 && self.current.is_none() {
                true => self.move_to(point),
                false => self.line_to(point),
            }
        }
    }

    /// The arc `CGContextAddArcToPoint` draws: tangent to the lines from `untransformed` (the
    /// current point before the transform) to `corner` and from `corner` to `towards`, with
    /// `radius`, under `transform`.
    pub fn arc_to(
        &mut self,
        transform: &Transform,
        untransformed: Point,
        corner: Point,
        towards: Point,
        radius: f64,
    ) {
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
            let end = self
                .starts
                .get(index + 1)
                .copied()
                .unwrap_or(self.points.len());
            (&self.points[start.min(end)..end], self.closed[index])
        })
    }

    /// Calls `work` with each pixel row of `clip` the path covers on a content `height` tall:
    /// the row, the left column of the coverage and each pixel's coverage from 0 to 256.
    pub fn scan(
        &self,
        height: i32,
        even_odd: bool,
        clip: Rect,
        mut work: impl FnMut(i32, i32, &[u16]),
    ) {
        let width = clip.width.max(0) as usize;
        if self.points.is_empty() || width == 0 {
            return;
        }
        let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
        let (mut top, mut bottom) = (f64::MAX, f64::MIN);
        for (points, _) in self.subpaths() {
            let count = points.len();
            for index in 0..count {
                let (from, to) = (points[index], points[(index + 1) % count]);
                let (y0, y1) = (f64::from(height) - from.y, f64::from(height) - to.y);
                if y0 == y1 {
                    continue;
                }
                let direction = if y0 < y1 { 1 } else { -1 };
                let (upper, lower) = match y0 < y1 {
                    true => ((from.x, y0), (to.x, y1)),
                    false => ((to.x, y1), (from.x, y0)),
                };
                edges.push((upper.0, upper.1, lower.0, lower.1, direction));
                top = top.min(upper.1);
                bottom = bottom.max(lower.1);
            }
        }
        let first_row = (floor(top) as i32).max(clip.y);
        let last_row = (ceil(bottom) as i32).min(clip.bottom());
        let mut cover = alloc::vec![0u16; width];
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for row in first_row..last_row {
            cover.fill(0);
            let mut any = false;
            for sub in 0..SUBSAMPLES {
                let y = f64::from(row) + (sub as f64 + 0.5) / SUBSAMPLES as f64;
                crossings.clear();
                for &(x0, y0, x1, y1, direction) in &edges {
                    if y >= y0 && y < y1 {
                        crossings.push((x0 + (y - y0) * (x1 - x0) / (y1 - y0), direction));
                    }
                }
                crossings.sort_by(|a, b| {
                    a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal)
                });
                let mut winding = 0;
                for pair in crossings.windows(2) {
                    winding += pair[0].1;
                    let inside = match even_odd {
                        true => winding % 2 != 0,
                        false => winding != 0,
                    };
                    if inside {
                        add_span(&mut cover, clip.x, (pair[0].0, pair[1].0), SUBSAMPLE_WEIGHT);
                        any = true;
                    }
                }
            }
            if any {
                work(row, clip.x, &cover);
            }
        }
    }

    /// Fills the path in `colour` at `alpha` on `canvas` (content `height` pixels tall),
    /// anti-aliased, with the even-odd rule when `even_odd`, else non-zero, only where `mask`
    /// lets it; every subpath is closed for filling.
    pub fn fill_with(
        &self,
        canvas: &mut Canvas<'_>,
        height: i32,
        (colour, alpha): (Colour, f64),
        even_odd: bool,
        mask: Option<&ClipMask>,
    ) {
        if !colour.visible || alpha <= 0.0 || self.points.is_empty() {
            return;
        }
        let clip = match mask {
            Some(mask) => canvas.clip().intersection(&mask.area()),
            None => canvas.clip(),
        };
        let strength = (alpha.min(1.0) * f64::from(FULL_COVER) + 0.5) as u32;
        let mut span: Vec<u32> = Vec::new();
        self.scan(height, even_odd, clip, |row, left, cover| {
            let first = cover.iter().position(|&amount| amount > 0);
            let last = cover.iter().rposition(|&amount| amount > 0);
            let (Some(first), Some(last)) = (first, last) else {
                return;
            };
            span.clear();
            for (index, &amount) in cover[first..=last].iter().enumerate() {
                let mut reach = u32::from(amount).min(FULL_COVER) * strength / FULL_COVER;
                if let Some(mask) = mask {
                    reach = reach * u32::from(mask.at(left + (first + index) as i32, row)) / 255;
                }
                span.push(premultiplied(colour.pixel, reach.min(255)));
            }
            canvas.composite((left + first as i32, row), &span, span.len(), false);
        });
    }

    /// The outline a stroke of this path in `style` covers, as polygons of one orientation, to
    /// fill with the non-zero rule.
    pub fn stroke_outline(&self, style: &StrokeStyle<'_>) -> Path {
        let half = style.width.max(1.0) / 2.0;
        let mut outline = Path::new();
        for (points, closed) in self.subpaths() {
            for (line, line_closed) in dashed(points, closed, style.dashes, style.dash_phase) {
                outline_polyline(&mut outline, &line, line_closed, half, style);
            }
        }
        outline
    }

    /// Strokes the path in `colour` at `alpha` in `style`, anti-aliased, only where `mask` lets
    /// it.
    pub fn stroke_with(
        &self,
        canvas: &mut Canvas<'_>,
        height: i32,
        (colour, alpha): (Colour, f64),
        style: &StrokeStyle<'_>,
        mask: Option<&ClipMask>,
    ) {
        if !colour.visible {
            return;
        }
        let outline = self.stroke_outline(style);
        outline.fill_with(canvas, height, (colour, alpha), false, mask);
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
                let (upper, lower) = if y0 < y1 {
                    ((from.x, y0), (to.x, y1))
                } else {
                    ((to.x, y1), (from.x, y0))
                };
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

    fn red_canvas_row(pixels: &[u32], width: usize, row: usize) -> Vec<u32> {
        pixels[row * width..(row + 1) * width]
            .iter()
            .map(|pixel| (pixel >> 16) & 0xff)
            .collect()
    }

    #[test]
    fn an_anti_aliased_fill_covers_half_of_a_half_covered_pixel() {
        let mut pixels = alloc::vec![0u32; 20 * 20];
        let mut canvas = Canvas::new(&mut pixels, 20, 20, 1).unwrap();
        let mut path = Path::new();
        path.rect(&Transform::IDENTITY, (2.5, 2.0, 4.0, 3.0));
        path.fill_with(&mut canvas, 20, (Colour::opaque(0x00ff_0000), 1.0), false, None);
        let row = red_canvas_row(&pixels, 20, 20 - 3);
        assert!((120..=136).contains(&row[2]), "{}", row[2]);
        assert_eq!(row[3], 0xff);
        assert!((120..=136).contains(&row[6]), "{}", row[6]);
        assert_eq!(row[7], 0);
    }

    #[test]
    fn half_alpha_blends_and_a_clip_mask_keeps_the_rest_untouched() {
        let mut pixels = alloc::vec![0u32; 20 * 20];
        let mut canvas = Canvas::new(&mut pixels, 20, 20, 1).unwrap();
        let mut keep = Path::new();
        keep.rect(&Transform::IDENTITY, (0.0, 0.0, 4.0, 20.0));
        let mut mask = ClipMask::rect(Rect::new(0, 0, 20, 20));
        mask.intersect_path(&keep, 20, false);
        let mut path = Path::new();
        path.rect(&Transform::IDENTITY, (0.0, 0.0, 10.0, 10.0));
        let half_red = (Colour::opaque(0x00ff_0000), 0.5);
        path.fill_with(&mut canvas, 20, half_red, false, Some(&mask));
        let row = red_canvas_row(&pixels, 20, 15);
        assert!((120..=136).contains(&row[1]), "{}", row[1]);
        assert_eq!(row[5], 0);
    }

    #[test]
    fn square_caps_reach_half_the_width_past_the_ends() {
        let mut line = Path::new();
        line.move_to(Point::new(2.0, 10.0));
        line.line_to(Point::new(8.0, 10.0));
        let style = StrokeStyle {
            width: 2.0,
            cap: Cap::Square,
            ..StrokeStyle::PLAIN
        };
        let mut pixels = alloc::vec![0u32; 20 * 20];
        let mut canvas = Canvas::new(&mut pixels, 20, 20, 1).unwrap();
        line.stroke_with(&mut canvas, 20, (Colour::opaque(0x00ff_0000), 1.0), &style, None);
        let row = red_canvas_row(&pixels, 20, 9);
        assert_eq!(row[0], 0);
        assert_eq!(row[1], 0xff);
        assert_eq!(row[8], 0xff);
        assert_eq!(row[9], 0);
    }

    #[test]
    fn dashes_cut_a_line_into_its_on_lengths() {
        let points = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let pieces = dashed(&points, false, &[2.0, 2.0], 0.0);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[1].0, alloc::vec![Point::new(4.0, 0.0), Point::new(6.0, 0.0)]);
        assert_eq!(dashed(&points, false, &[], 0.0).len(), 1);
    }

    #[test]
    fn a_full_clockwise_arc_comes_back_to_its_start() {
        let mut path = Path::new();
        path.arc(
            &Transform::IDENTITY,
            Point::new(0.0, 0.0),
            10.0,
            (0.0, TAU),
            false,
        );
        let last = path.current().unwrap();
        assert!((last.x - 10.0).abs() < 1e-9 && last.y.abs() < 1e-9);
    }
}

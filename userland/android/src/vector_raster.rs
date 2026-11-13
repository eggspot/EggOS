//! Vector drawables drawn (D518): a path's data (SVG's syntax: `M`, `L`, `H`, `V`, `C`, `S`, `Q`,
//! `T`, `A` and `Z`, absolute and relative) is flattened into line segments (curves in 16 steps
//! each, arcs as a line to their end), scaled from the viewport to the drawable's size in
//! pixels (a density-independent pixel each), and filled by scanlines four to a pixel, by the
//! non-zero or even-odd rule, its colour composited over the paths before it.

use alloc::vec;
use alloc::vec::Vec;
use eggos_android_package::vector::{Colour, Vector};
use eggos_math::roundf;

const CURVE_STEPS: usize = 16;
const SUBROWS: usize = 4;
const LARGEST_SIDE: f32 = 1024.0;

type Point = (f32, f32);

struct Tokens<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Tokens<'_> {
    fn skip(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|&byte| byte == b',' || byte.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let byte = *self.bytes.get(self.at)?;
        if byte.is_ascii_alphabetic() && byte != b'e' && byte != b'E' {
            self.at += 1;
            return Some(byte);
        }
        None
    }

    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.at;
        let mut seen_dot = false;
        let mut seen_exponent = false;
        while let Some(&byte) = self.bytes.get(self.at) {
            let sign_allowed =
                self.at == start || matches!(self.bytes.get(self.at - 1), Some(b'e' | b'E'));
            match byte {
                b'0'..=b'9' => {}
                b'-' | b'+' if sign_allowed => {}
                b'.' if !seen_dot && !seen_exponent => seen_dot = true,
                b'e' | b'E' if !seen_exponent && self.at > start => seen_exponent = true,
                _ => break,
            }
            self.at += 1;
        }
        core::str::from_utf8(&self.bytes[start..self.at])
            .ok()?
            .parse()
            .ok()
    }

    fn point(&mut self) -> Option<Point> {
        Some((self.number()?, self.number()?))
    }
}

fn quadratic(from: Point, control: Point, to: Point, out: &mut Vec<Point>) {
    for step in 1..=CURVE_STEPS {
        let t = step as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        out.push((
            u * u * from.0 + 2.0 * u * t * control.0 + t * t * to.0,
            u * u * from.1 + 2.0 * u * t * control.1 + t * t * to.1,
        ));
    }
}

fn cubic(from: Point, first: Point, second: Point, to: Point, out: &mut Vec<Point>) {
    for step in 1..=CURVE_STEPS {
        let t = step as f32 / CURVE_STEPS as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * from.0 + b * first.0 + c * second.0 + d * to.0,
            a * from.1 + b * first.1 + c * second.1 + d * to.1,
        ));
    }
}

fn reflected(control: Option<Point>, current: Point) -> Point {
    control.map_or(current, |c| (2.0 * current.0 - c.0, 2.0 * current.1 - c.1))
}

/// The closed polygons of path data `data`.
pub fn polygons(data: &str) -> Vec<Vec<Point>> {
    let mut tokens = Tokens {
        bytes: data.as_bytes(),
        at: 0,
    };
    let mut shapes = Vec::new();
    let mut shape: Vec<Point> = Vec::new();
    let mut current = (0.0f32, 0.0f32);
    let mut start = current;
    let mut last_control: Option<Point> = None;
    let mut command = b'M';
    loop {
        if let Some(next) = tokens.command() {
            command = next;
        } else if tokens.at >= tokens.bytes.len() {
            break;
        }
        let relative = command.is_ascii_lowercase();
        let offset = |point: Point, current: Point| match relative {
            true => (point.0 + current.0, point.1 + current.1),
            false => point,
        };
        let mut control = None;
        match command.to_ascii_uppercase() {
            b'Z' => {
                if shape.len() > 1 {
                    shapes.push(core::mem::take(&mut shape));
                }
                shape.clear();
                current = start;
                command = 0;
                last_control = None;
                continue;
            }
            b'M' => {
                let Some(point) = tokens.point() else { break };
                if shape.len() > 1 {
                    shapes.push(core::mem::take(&mut shape));
                }
                shape.clear();
                current = offset(point, current);
                start = current;
                shape.push(current);
                command = if relative { b'l' } else { b'L' };
            }
            b'L' => {
                let Some(point) = tokens.point() else { break };
                current = offset(point, current);
                shape.push(current);
            }
            b'H' => {
                let Some(x) = tokens.number() else { break };
                current.0 = if relative { current.0 + x } else { x };
                shape.push(current);
            }
            b'V' => {
                let Some(y) = tokens.number() else { break };
                current.1 = if relative { current.1 + y } else { y };
                shape.push(current);
            }
            b'C' | b'S' => {
                let first = match command.to_ascii_uppercase() {
                    b'C' => {
                        let Some(point) = tokens.point() else { break };
                        offset(point, current)
                    }
                    _ => reflected(last_control, current),
                };
                let (Some(second), Some(end)) = (tokens.point(), tokens.point()) else {
                    break;
                };
                let (second, end) = (offset(second, current), offset(end, current));
                cubic(current, first, second, end, &mut shape);
                control = Some(second);
                current = end;
            }
            b'Q' | b'T' => {
                let middle = match command.to_ascii_uppercase() {
                    b'Q' => {
                        let Some(point) = tokens.point() else { break };
                        offset(point, current)
                    }
                    _ => reflected(last_control, current),
                };
                let Some(end) = tokens.point() else { break };
                let end = offset(end, current);
                quadratic(current, middle, end, &mut shape);
                control = Some(middle);
                current = end;
            }
            b'A' => {
                for _ in 0..5 {
                    if tokens.number().is_none() {
                        break;
                    }
                }
                let Some(end) = tokens.point() else { break };
                current = offset(end, current);
                shape.push(current);
            }
            _ => break,
        }
        last_control = control;
    }
    if shape.len() > 1 {
        shapes.push(shape);
    }
    shapes
}

fn over(pixel: &mut u32, colour: u32, coverage: f32) {
    let alpha = (colour >> 24) as f32 / 255.0 * coverage;
    let channel = |shift: u32| (colour >> shift & 0xff) as f32 * alpha;
    let below = |shift: u32| (*pixel >> shift & 0xff) as f32 * (1.0 - alpha);
    let mix = |shift: u32| (roundf(channel(shift) + below(shift)) as u32).min(255) << shift;
    let out_alpha = roundf(alpha * 255.0 + (*pixel >> 24) as f32 * (1.0 - alpha)) as u32;
    *pixel = out_alpha.min(255) << 24 | mix(16) | mix(8) | mix(0);
}

fn fill(pixels: &mut [u32], width: usize, shapes: &[Vec<Point>], even_odd: bool, colour: u32) {
    let height = pixels.len() / width.max(1);
    let mut coverage = vec![0.0f32; width];
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for row in 0..height {
        coverage.iter_mut().for_each(|value| *value = 0.0);
        for sub in 0..SUBROWS {
            let y = row as f32 + (sub as f32 + 0.5) / SUBROWS as f32;
            crossings.clear();
            for shape in shapes {
                for (index, &from) in shape.iter().enumerate() {
                    let to = shape[(index + 1) % shape.len()];
                    if (from.1 <= y) == (to.1 <= y) {
                        continue;
                    }
                    let x = from.0 + (y - from.1) / (to.1 - from.1) * (to.0 - from.0);
                    crossings.push((x, if to.1 > from.1 { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                let inside = match even_odd {
                    true => winding % 2 != 0,
                    false => winding != 0,
                };
                if !inside {
                    continue;
                }
                let first = roundf(pair[0].0.max(0.0)) as usize;
                let last = roundf(pair[1].0.min(width as f32)) as usize;
                for value in coverage.iter_mut().take(last.min(width)).skip(first) {
                    *value += 1.0 / SUBROWS as f32;
                }
            }
        }
        for (pixel, &amount) in pixels[row * width..(row + 1) * width]
            .iter_mut()
            .zip(&coverage)
        {
            if amount > 0.0 {
                over(pixel, colour, amount.min(1.0));
            }
        }
    }
}

/// `vector` drawn: its width in pixels and its premultiplied pixels, colours looked up with
/// `colour_of`; `None` for an empty or oversized drawable.
pub fn rasterize(
    vector: &Vector,
    colour_of: impl Fn(Colour) -> Option<u32>,
) -> Option<(usize, Vec<u32>)> {
    let (width, height) = (roundf(vector.width), roundf(vector.height));
    if !(1.0..=LARGEST_SIDE).contains(&width) || !(1.0..=LARGEST_SIDE).contains(&height) {
        return None;
    }
    let scale_x = width / vector.viewport_width.max(f32::MIN_POSITIVE);
    let scale_y = height / vector.viewport_height.max(f32::MIN_POSITIVE);
    let (width, height) = (width as usize, height as usize);
    let mut pixels = vec![0u32; width * height];
    for path in &vector.paths {
        let Some(colour) = path.fill.and_then(&colour_of) else {
            continue;
        };
        let shapes: Vec<Vec<Point>> = polygons(&path.data)
            .into_iter()
            .map(|shape| {
                shape
                    .into_iter()
                    .map(|(x, y)| (x * scale_x, y * scale_y))
                    .collect()
            })
            .collect();
        fill(&mut pixels, width, &shapes, path.even_odd, colour);
    }
    for pixel in &mut pixels {
        let alpha = *pixel >> 24;
        let scale = |shift: u32| ((*pixel >> shift & 0xff) * alpha / 255) << shift;
        *pixel = alpha << 24 | scale(16) | scale(8) | scale(0);
    }
    Some((width, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_data_becomes_closed_polygons() {
        let shapes = polygons("M2,2 L10,2 L10,10 Z m20,0 h4 v4 h-4 z");
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0], [(2.0, 2.0), (10.0, 2.0), (10.0, 10.0)]);
        assert_eq!(shapes[1][1], (26.0, 2.0));
    }

    #[test]
    fn a_square_fills_its_pixels_whole_and_leaves_the_rest() {
        let vector = Vector {
            width: 4.0,
            height: 4.0,
            viewport_width: 4.0,
            viewport_height: 4.0,
            paths: alloc::vec![eggos_android_package::vector::VectorPath {
                data: alloc::string::String::from("M1,1h2v2h-2z"),
                fill: Some(Colour::Value(0xffff_0000)),
                even_odd: false,
            }],
        };
        let (width, pixels) = rasterize(&vector, |colour| match colour {
            Colour::Value(value) => Some(value),
            Colour::Resource(_) => None,
        })
        .unwrap();
        assert_eq!(width, 4);
        assert_eq!(pixels[5], 0xffff_0000);
        assert_eq!(pixels[0], 0);
    }
}

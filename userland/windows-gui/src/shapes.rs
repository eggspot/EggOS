//! GDI's shapes with Win32's rules for which pixels each covers: a shape's bounding rectangle
//! excludes its right and bottom edges; with no pen (`NULL_PEN`) a rectangle or an ellipse is
//! one pixel smaller each way; the pen outlines inside the bounding rectangle, `width` pixels
//! thick, and the brush fills what is inside the outline; a line leaves out its end point.

use crate::surface::{Rect, Surface};

/// A pen that draws: its colour (a pixel) and width in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pen {
    pub colour: u32,
    pub width: u32,
}

impl Pen {
    fn thickness(&self) -> i32 {
        self.width.clamp(1, i32::MAX as u32) as i32
    }
}

/// `Rectangle`: outlined with `pen` and filled with `brush`, either of which may be absent.
pub fn rectangle(surface: &mut Surface<'_>, rect: Rect, pen: Option<Pen>, brush: Option<u32>) {
    let rect = rect.normalised();
    let Some(pen) = pen else {
        if let Some(brush) = brush {
            surface.fill(
                Rect::new(rect.left, rect.top, rect.right - 1, rect.bottom - 1),
                brush,
            );
        }
        return;
    };
    let thickness = pen.thickness();
    if let Some(brush) = brush {
        surface.fill(rect.inset(thickness), brush);
    }
    let across = thickness.min(rect.height());
    let down = thickness.min(rect.width());
    for band in [
        Rect::new(rect.left, rect.top, rect.right, rect.top + across),
        Rect::new(rect.left, rect.bottom - across, rect.right, rect.bottom),
        Rect::new(rect.left, rect.top, rect.left + down, rect.bottom),
        Rect::new(rect.right - down, rect.top, rect.right, rect.bottom),
    ] {
        surface.fill(band, pen.colour);
    }
}

/// `Ellipse`: the ellipse inscribed in `rect`, outlined with `pen` and filled with `brush`. A
/// pixel is inside when its centre is inside the ellipse; the outline is what is inside the
/// ellipse but not inside the one inscribed in `rect` made the pen's width smaller each side.
pub fn ellipse(surface: &mut Surface<'_>, rect: Rect, pen: Option<Pen>, brush: Option<u32>) {
    let rect = rect.normalised();
    let outer = match pen {
        Some(_) => rect,
        None => Rect::new(rect.left, rect.top, rect.right - 1, rect.bottom - 1),
    };
    if outer.is_empty() {
        return;
    }
    let inner = outer.inset(pen.map_or(0, |pen| pen.thickness()));
    let clip = surface.clip();
    for y in outer.top.max(clip.top)..outer.bottom.min(clip.bottom) {
        let Some((left, right)) = ellipse_span(outer, y) else {
            continue;
        };
        let Some(pen) = pen else {
            if let Some(brush) = brush {
                surface.span(y, left, right, brush);
            }
            continue;
        };
        match ellipse_span(inner, y) {
            Some((inner_left, inner_right)) => {
                if let Some(brush) = brush {
                    surface.span(y, inner_left, inner_right, brush);
                }
                surface.span(y, left, inner_left, pen.colour);
                surface.span(y, inner_right, right, pen.colour);
            }
            None => surface.span(y, left, right, pen.colour),
        }
    }
}

/// The pixels `left..right` of row `y` inside the ellipse inscribed in `rect`, if any.
pub fn ellipse_span(rect: Rect, y: i32) -> Option<(i32, i32)> {
    if rect.is_empty() || y < rect.top || y >= rect.bottom {
        return None;
    }
    let width = i128::from(rect.width());
    let height = i128::from(rect.height());
    let across = 2 * i128::from(y) + 1 - i128::from(rect.top) - i128::from(rect.bottom);
    let room = width * width * height * height - across * across * width * width;
    if room < 0 {
        return None;
    }
    let sum = i128::from(rect.left) + i128::from(rect.right);
    let mut reach = isqrt(room / (height * height));
    if (reach - sum - 1).rem_euclid(2) != 0 {
        reach -= 1;
    }
    if reach < 0 {
        return None;
    }
    Some((
        ((sum - 1 - reach) / 2) as i32,
        ((sum - 1 + reach) / 2 + 1) as i32,
    ))
}

fn isqrt(value: i128) -> i128 {
    if value < 2 {
        return value.max(0);
    }
    let mut root = value;
    let mut next = (root + 1) / 2;
    while next < root {
        root = next;
        next = (root + value / root) / 2;
    }
    root
}

/// `LineTo` from `from` to `to`: Bresenham's line without its end point, each point a square
/// of the pen's width.
pub fn line(surface: &mut Surface<'_>, from: (i32, i32), to: (i32, i32), pen: Pen) {
    let (mut x, mut y) = (i64::from(from.0), i64::from(from.1));
    let (end_x, end_y) = (i64::from(to.0), i64::from(to.1));
    let across = (end_x - x).abs();
    let down = -(end_y - y).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = across + down;
    let thickness = pen.thickness();
    let back = (thickness - 1) / 2;
    while (x, y) != (end_x, end_y) {
        let (px, py) = (x as i32, y as i32);
        match thickness {
            1 => surface.pixel(px, py, pen.colour),
            _ => surface.fill(
                Rect::sized(px - back, py - back, thickness, thickness),
                pen.colour,
            ),
        }
        let twice = 2 * error;
        if twice >= down {
            error += down;
            x += step_x;
        }
        if twice <= across {
            error += across;
            y += step_y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u32 = 12;

    fn picture(draw: impl FnOnce(&mut Surface<'_>)) -> Vec<String> {
        let mut pixels = vec![0u32; (WIDTH * WIDTH) as usize];
        let mut surface = Surface::new(&mut pixels, WIDTH, WIDTH).unwrap();
        draw(&mut surface);
        pixels
            .chunks(WIDTH as usize)
            .map(|row| {
                row.iter()
                    .map(|&pixel| match pixel {
                        0 => '.',
                        1 => 'P',
                        2 => 'B',
                        _ => '?',
                    })
                    .collect()
            })
            .collect()
    }

    const PEN: Option<Pen> = Some(Pen {
        colour: 1,
        width: 1,
    });

    #[test]
    fn a_rectangle_leaves_out_its_right_and_bottom_edges() {
        let rows = picture(|s| rectangle(s, Rect::new(1, 1, 5, 4), PEN, Some(2)));
        assert_eq!(rows[0], "............");
        assert_eq!(rows[1], ".PPPP.......");
        assert_eq!(rows[2], ".PBBP.......");
        assert_eq!(rows[3], ".PPPP.......");
        assert_eq!(rows[4], "............");
    }

    #[test]
    fn without_a_pen_a_rectangle_is_one_pixel_smaller_each_way() {
        let rows = picture(|s| rectangle(s, Rect::new(4, 3, 1, 1), None, Some(2)));
        assert_eq!(rows[0], "............");
        assert_eq!(rows[1], ".BB.........");
        assert_eq!(rows[2], "............");
    }

    #[test]
    fn a_wide_pen_outlines_inside_and_a_null_brush_leaves_the_middle() {
        let rows = picture(|s| {
            rectangle(
                s,
                Rect::new(0, 0, 7, 7),
                Some(Pen {
                    colour: 1,
                    width: 2,
                }),
                None,
            )
        });
        assert_eq!(rows[1], "PPPPPPP.....");
        assert_eq!(rows[2], "PP...PP.....");
        assert_eq!(rows[5], "PPPPPPP.....");
        assert_eq!(rows[7], "............");
    }

    #[test]
    fn a_circle_is_symmetric_with_its_outline_around_its_fill() {
        let rows = picture(|s| ellipse(s, Rect::new(0, 0, 8, 8), PEN, Some(2)));
        assert_eq!(rows[0], "..PPPP......");
        assert_eq!(rows[1], ".PBBBBP.....");
        assert_eq!(rows[2], "PBBBBBBP....");
        assert_eq!(rows[5], "PBBBBBBP....");
        assert_eq!(rows[6], ".PBBBBP.....");
        assert_eq!(rows[7], "..PPPP......");
        assert_eq!(rows[8], "............");
    }

    #[test]
    fn ellipse_spans_follow_pixel_centres() {
        assert_eq!(ellipse_span(Rect::new(0, 0, 4, 4), 0), Some((1, 3)));
        assert_eq!(ellipse_span(Rect::new(0, 0, 4, 4), 1), Some((0, 4)));
        assert_eq!(ellipse_span(Rect::new(10, 0, 13, 3), 0), Some((10, 13)));
        assert_eq!(ellipse_span(Rect::new(0, 0, 9, 3), 0), Some((1, 8)));
        assert_eq!(ellipse_span(Rect::new(0, 0, 4, 4), 4), None);
        assert_eq!(
            ellipse_span(Rect::new(-40_000, -40_000, 40_000, 40_000), 0),
            Some((-40_000, 40_000))
        );
    }

    #[test]
    fn a_line_leaves_out_its_end_point() {
        let rows = picture(|s| {
            line(
                s,
                (0, 0),
                (4, 2),
                Pen {
                    colour: 1,
                    width: 1,
                },
            )
        });
        assert_eq!(rows[0], "P...........");
        assert_eq!(rows[1], ".PP.........");
        assert_eq!(rows[2], "...P........");
        let rows = picture(|s| {
            line(
                s,
                (3, 5),
                (3, 1),
                Pen {
                    colour: 1,
                    width: 3,
                },
            )
        });
        assert_eq!(rows[0], "............");
        assert_eq!(rows[1], "..PPP.......");
        assert_eq!(rows[6], "..PPP.......");
        assert_eq!(rows[7], "............");
    }
}

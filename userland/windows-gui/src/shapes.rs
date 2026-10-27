//! GDI's shapes with Win32's rules for which pixels each covers: a shape's bounding rectangle
//! excludes its right and bottom edges; with no pen (`NULL_PEN`) a rectangle or an ellipse is
//! one pixel smaller each way; the pen outlines inside the bounding rectangle, `width` pixels
//! thick, and the brush fills what is inside the outline; a line leaves out its end point.
//! Arithmetic on the program's coordinates saturates, a pen is at most as wide as the surface's
//! width and height together, and a line costs no more than the stretch of it that can reach
//! the surface.

use crate::surface::{Rect, Surface};

/// A pen that draws: its colour (a pixel) and width in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pen {
    pub colour: u32,
    pub width: u32,
}

impl Pen {
    fn thickness(&self, surface: &Surface<'_>) -> i32 {
        let widest = surface.width().saturating_add(surface.height()).max(1);
        self.width.clamp(1, widest.min(i32::MAX as u32)) as i32
    }
}

/// `Rectangle`: outlined with `pen` and filled with `brush`, either of which may be absent.
pub fn rectangle(surface: &mut Surface<'_>, rect: Rect, pen: Option<Pen>, brush: Option<u32>) {
    let rect = rect.normalised();
    let Some(pen) = pen else {
        if let Some(brush) = brush {
            surface.fill(
                Rect::new(
                    rect.left,
                    rect.top,
                    rect.right.saturating_sub(1),
                    rect.bottom.saturating_sub(1),
                ),
                brush,
            );
        }
        return;
    };
    let thickness = pen.thickness(surface);
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
        None => Rect::new(
            rect.left,
            rect.top,
            rect.right.saturating_sub(1),
            rect.bottom.saturating_sub(1),
        ),
    };
    if outer.is_empty() {
        return;
    }
    let inner = outer.inset(pen.map_or(0, |pen| pen.thickness(surface)));
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

/// The pixels `left..right` of row `y` inside the ellipse inscribed in `rect`, if any. The
/// reach from the centre is found by bisection on 128-bit products, with no 128-bit division.
pub fn ellipse_span(rect: Rect, y: i32) -> Option<(i32, i32)> {
    if rect.is_empty() || y < rect.top || y >= rect.bottom {
        return None;
    }
    let width = i64::from(rect.width());
    let height = i128::from(rect.height());
    let across = i128::from(2 * i64::from(y) + 1 - i64::from(rect.top) - i64::from(rect.bottom));
    let room = i128::from(width) * i128::from(width) * (height * height - across * across);
    if room < 0 {
        return None;
    }
    let fits = |reach: i64| i128::from(reach) * i128::from(reach) * height * height <= room;
    let (mut low, mut high) = (0i64, width + 1);
    while high - low > 1 {
        let middle = low + ((high - low) >> 1);
        match fits(middle) {
            true => low = middle,
            false => high = middle,
        }
    }
    let sum = i64::from(rect.left) + i64::from(rect.right);
    let mut reach = low;
    if (reach - sum - 1) & 1 != 0 {
        reach -= 1;
    }
    if reach < 0 {
        return None;
    }
    Some((
        ((sum - 1 - reach) >> 1) as i32,
        (((sum - 1 + reach) >> 1) + 1) as i32,
    ))
}

/// `LineTo` from `from` to `to`, without its end point: along the longer axis one point per
/// pixel, the other coordinate the nearest to the exact line (halves rounded up), each point a
/// square of the pen's width. Only the points whose square can reach the surface's clip are
/// computed.
pub fn line(surface: &mut Surface<'_>, from: (i32, i32), to: (i32, i32), pen: Pen) {
    let across = i64::from(to.0) - i64::from(from.0);
    let down = i64::from(to.1) - i64::from(from.1);
    let steps = across.unsigned_abs().max(down.unsigned_abs());
    if steps == 0 {
        return;
    }
    let thickness = i64::from(pen.thickness(surface));
    let back = (thickness - 1) / 2;
    let clip = surface.clip();
    let along_x = across.unsigned_abs() >= down.unsigned_abs();
    let (major, major_step, minor, minor_step, run, low, high) = match along_x {
        true => (
            i64::from(from.0),
            across.signum(),
            i64::from(from.1),
            down.signum(),
            down.unsigned_abs(),
            clip.left,
            clip.right,
        ),
        false => (
            i64::from(from.1),
            down.signum(),
            i64::from(from.0),
            across.signum(),
            across.unsigned_abs(),
            clip.top,
            clip.bottom,
        ),
    };
    let first_reaching = i64::from(low) - (thickness - 1 - back);
    let last_reaching = i64::from(high) - 1 + back;
    let (from_step, to_step) = match major_step > 0 {
        true => (first_reaching - major, last_reaching - major),
        false => (major - last_reaching, major - first_reaching),
    };
    let last_step = steps as i64 - 1;
    if to_step < 0 || from_step > last_step {
        return;
    }
    let (first, last) = (from_step.max(0) as u64, to_step.min(last_step) as u64);
    let divisor = 2 * steps;
    let numerator = 2 * u128::from(first) * u128::from(run) + u128::from(steps);
    let (quotient, mut remainder) = divide(numerator, divisor);
    let mut offset = quotient as i64;
    for step in first..=last {
        let major_at = major + major_step * step as i64;
        let minor_at = minor + minor_step * offset;
        let (x, y) = match along_x {
            true => (clamp(major_at), clamp(minor_at)),
            false => (clamp(minor_at), clamp(major_at)),
        };
        match thickness {
            1 => surface.pixel(x, y, pen.colour),
            _ => surface.fill(
                Rect::sized(
                    x.saturating_sub(back as i32),
                    y.saturating_sub(back as i32),
                    thickness as i32,
                    thickness as i32,
                ),
                pen.colour,
            ),
        }
        remainder += 2 * run;
        if remainder >= divisor {
            remainder -= divisor;
            offset += 1;
        }
    }
}

fn clamp(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn divide(numerator: u128, divisor: u64) -> (u128, u64) {
    let mut quotient = 0u128;
    let mut remainder = 0u64;
    for digit in (0..8).rev() {
        let part = (remainder << 16) | ((numerator >> (16 * digit)) & 0xffff) as u64;
        quotient = (quotient << 16) | u128::from(part / divisor);
        remainder = part % divisor;
    }
    (quotient, remainder)
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
    fn long_division_in_16_bit_digits_matches_division() {
        for (numerator, divisor) in [
            (0u128, 1u64),
            (7, 2),
            (1 << 65, 3),
            ((1 << 66) - 1, (1 << 34) - 1),
            (u128::from(u64::MAX) * 5 + 4, 1 << 40),
        ] {
            assert_eq!(
                divide(numerator, divisor),
                (
                    numerator / u128::from(divisor),
                    (numerator % u128::from(divisor)) as u64
                )
            );
        }
    }

    #[test]
    fn a_huge_line_costs_only_its_visible_part_and_matches_its_short_piece() {
        let pen = Pen {
            colour: 1,
            width: 1,
        };
        let far = picture(|s| line(s, (i32::MIN, 5), (i32::MAX, 5), pen));
        assert_eq!(far[5], "PPPPPPPPPPPP");
        assert_eq!(far[4], "............");
        let diagonal = picture(|s| {
            line(
                s,
                (-1_000_000_000, -1_000_000_000),
                (i32::MAX, i32::MAX),
                pen,
            )
        });
        let short = picture(|s| line(s, (-5, -5), (20, 20), pen));
        assert_eq!(diagonal, short);
        let away = picture(|s| line(s, (-100, -100), (-50, 3), pen));
        assert!(away.iter().all(|row| !row.contains('P')));
        let shallow = picture(|s| line(s, (-1_000_000, 0), (1_000_000, 1), pen));
        assert_eq!(shallow[1], "PPPPPPPPPPPP");
        assert_eq!(shallow[0], "............");
    }

    #[test]
    fn a_pen_wider_than_the_surface_draws_no_wider_and_ends() {
        let huge = Pen {
            colour: 1,
            width: u32::MAX,
        };
        let surface_wide = Pen {
            colour: 1,
            width: 2 * WIDTH,
        };
        let far = (-1_000_000, 3);
        let lines = picture(|s| line(s, far, (1_000_000, 3), huge));
        assert_eq!(
            lines,
            picture(|s| line(s, far, (1_000_000, 3), surface_wide))
        );
        assert!(lines.iter().all(|row| row == "PPPPPPPPPPPP"));
        let ring = picture(|s| ellipse(s, Rect::new(0, 0, 12, 12), Some(huge), Some(2)));
        assert!(ring.iter().all(|row| !row.contains('B')));
    }

    #[test]
    fn shapes_take_extreme_coordinates_without_overflow() {
        let pen = Some(Pen {
            colour: 1,
            width: 1,
        });
        let extreme = Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        let rows = picture(|s| {
            rectangle(s, extreme, None, Some(2));
            ellipse(s, extreme, None, Some(2));
            rectangle(s, extreme, pen, None);
            line(
                s,
                (0, 0),
                (3, 3),
                Pen {
                    colour: 1,
                    width: i32::MAX as u32,
                },
            );
        });
        assert_eq!(rows.len(), 12);
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

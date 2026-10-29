//! AppKit's drawing on a window's pixels (`docs/ux/macos-gui-v0.3.md` sections 0 to 3): rects,
//! frames, lines and ovals 1 px wide with no anti-aliasing, text in Egg Sans with the point at
//! its line box's bottom-left, and the push button, text field and label as the toolkit draws
//! them inside the frame the app gives. Every function takes frames and points in the window's
//! coordinates and the content's height, and draws inside the canvas's clip.

use crate::colour::Colour;
use crate::geometry::{Frame, Point, pixel_of_point, pixel_rect, round};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text::{self as toolkit_text, Text};
use eggos_toolkit::widget::{self, Align, CONTROL_HEIGHT, Field, Kind, Look};

/// The height of one line of text at size 1.
pub const LINE_HEIGHT: i32 = 16;
const SQUARE_ROOT_STEPS: usize = 24;
const LINE_CLIP_MARGIN: i64 = 2;

/// The glyph size a font of `points` draws at: 1 below 28, 2 from 28.
pub fn font_scale(points: f64) -> i32 {
    if points >= 28.0 { 2 } else { 1 }
}

/// Fills `frame` (`NSRectFill`).
pub fn fill(canvas: &mut Canvas<'_>, height: i32, frame: Frame, colour: Colour) {
    if colour.visible {
        canvas.fill(pixel_rect(frame, height), colour.pixel);
    }
}

/// Draws the 1 px line along the inside of `frame` (`NSFrameRect`).
pub fn outline(canvas: &mut Canvas<'_>, height: i32, frame: Frame, colour: Colour) {
    if !colour.visible {
        return;
    }
    let rect = pixel_rect(frame, height);
    if rect.is_empty() {
        return;
    }
    let ink = colour.pixel;
    canvas.fill(Rect::new(rect.x, rect.y, rect.width, 1), ink);
    canvas.fill(Rect::new(rect.x, rect.bottom() - 1, rect.width, 1), ink);
    canvas.fill(Rect::new(rect.x, rect.y, 1, rect.height), ink);
    canvas.fill(Rect::new(rect.right() - 1, rect.y, 1, rect.height), ink);
}

/// Draws the 1 px line from `from` to `to`, both ends included. A line reaching outside the
/// canvas's clip is cut to the clip (with a margin) first, so far-away ends cost nothing.
pub fn line(canvas: &mut Canvas<'_>, height: i32, from: Point, to: Point, colour: Colour) {
    if !colour.visible {
        return;
    }
    let from = pixel_of_point(from, height);
    let to = pixel_of_point(to, height);
    let Some(((mut x, mut y), (end_x, end_y))) = clipped_line(from, to, canvas.clip()) else {
        return;
    };
    let dx = (end_x - x).abs();
    let dy = -(end_y - y).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        canvas.pixel(x as i32, y as i32, colour.pixel);
        if x == end_x && y == end_y {
            return;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += step_x;
        }
        if doubled <= dx {
            error += dx;
            y += step_y;
        }
    }
}

fn clipped_line(from: (i32, i32), to: (i32, i32), clip: Rect) -> Option<((i64, i64), (i64, i64))> {
    let left = i64::from(clip.x) - LINE_CLIP_MARGIN;
    let top = i64::from(clip.y) - LINE_CLIP_MARGIN;
    let right = i64::from(clip.right()) + LINE_CLIP_MARGIN;
    let bottom = i64::from(clip.bottom()) + LINE_CLIP_MARGIN;
    let widen = |(x, y): (i32, i32)| (i64::from(x), i64::from(y));
    let (from, to) = (widen(from), widen(to));
    let inside = |(x, y): (i64, i64)| (left..=right).contains(&x) && (top..=bottom).contains(&y);
    if inside(from) && inside(to) {
        return Some((from, to));
    }
    let (start_x, start_y) = (from.0 as f64, from.1 as f64);
    let (delta_x, delta_y) = ((to.0 - from.0) as f64, (to.1 - from.1) as f64);
    let mut enter = 0.0;
    let mut leave = 1.0;
    for (direction, room) in [
        (-delta_x, start_x - left as f64),
        (delta_x, right as f64 - start_x),
        (-delta_y, start_y - top as f64),
        (delta_y, bottom as f64 - start_y),
    ] {
        if direction == 0.0 {
            if room < 0.0 {
                return None;
            }
            continue;
        }
        let at = room / direction;
        if direction < 0.0 {
            enter = f64::max(enter, at);
        } else {
            leave = f64::min(leave, at);
        }
        if enter > leave {
            return None;
        }
    }
    let point = |along: f64| {
        (
            i64::from(round(start_x + along * delta_x)).clamp(left, right),
            i64::from(round(start_y + along * delta_y)).clamp(top, bottom),
        )
    };
    Some((point(enter), point(leave)))
}

fn square_root(value: f64) -> f64 {
    if value.is_nan() || value <= 0.0 {
        return 0.0;
    }
    let mut root = if value > 1.0 { value } else { 1.0 };
    for _ in 0..SQUARE_ROOT_STEPS {
        root = 0.5 * (root + value / root);
    }
    root
}

fn oval_span(rect: Rect, row: i32) -> Option<(i32, i32)> {
    if rect.is_empty() || row < rect.y || row >= rect.bottom() {
        return None;
    }
    let radius_x = f64::from(rect.width) / 2.0;
    let radius_y = f64::from(rect.height) / 2.0;
    let centre_x = f64::from(rect.x) + radius_x;
    let centre_y = f64::from(rect.y) + radius_y;
    let across = (f64::from(row) + 0.5 - centre_y) / radius_y;
    let half = radius_x * square_root(1.0 - across * across);
    let (left, right) = (round(centre_x - half), round(centre_x + half));
    (left < right).then_some((left, right))
}

/// Fills (`filled`) or strokes 1 px wide the oval inside `frame`
/// (`bezierPathWithOvalInRect:`).
pub fn oval(canvas: &mut Canvas<'_>, height: i32, frame: Frame, colour: Colour, filled: bool) {
    if !colour.visible {
        return;
    }
    let outer = pixel_rect(frame, height);
    let inner = outer.inset(1);
    let clip = canvas.clip();
    for row in outer.y.max(clip.y)..outer.bottom().min(clip.bottom()) {
        let Some((left, right)) = oval_span(outer, row) else {
            continue;
        };
        match (filled, oval_span(inner, row)) {
            (false, Some((inner_left, inner_right))) => {
                canvas.fill(Rect::new(left, row, inner_left - left, 1), colour.pixel);
                canvas.fill(
                    Rect::new(inner_right, row, right - inner_right, 1),
                    colour.pixel,
                );
            }
            _ => canvas.fill(Rect::new(left, row, right - left, 1), colour.pixel),
        }
    }
}

/// The width of `words` at glyph scale `scale`.
pub fn text_width(words: &str, scale: i32) -> i32 {
    toolkit_text::width(words) * scale
}

/// Draws `words` with the bottom-left of its line box at `at`
/// (`drawAtPoint:withAttributes:`); returns the x after it.
pub fn text(
    canvas: &mut Canvas<'_>,
    height: i32,
    at: Point,
    words: &str,
    colour: Colour,
    scale: i32,
) -> i32 {
    let x = round(at.x);
    let top = height - round(at.y) - LINE_HEIGHT * scale;
    if !colour.visible {
        return x + text_width(words, scale);
    }
    canvas.text(x, top, words, colour.pixel, scale)
}

/// Where a control (push button or text field) at `frame` is drawn: the frame's full width,
/// [`CONTROL_HEIGHT`] tall, centred vertically in it with an odd pixel below.
pub fn control_rect(frame: Frame, height: i32) -> Rect {
    let rect = pixel_rect(frame, height);
    Rect::new(
        rect.x,
        rect.y + (rect.height - CONTROL_HEIGHT).div_euclid(2),
        rect.width,
        CONTROL_HEIGHT,
    )
}

/// Draws a push button titled `title` at `frame` (`NSButton`), the window's default button
/// when `primary`.
pub fn button(
    canvas: &mut Canvas<'_>,
    height: i32,
    frame: Frame,
    title: &str,
    primary: bool,
    look: Look,
) {
    widget::paint(
        canvas,
        &Kind::button(title, primary),
        control_rect(frame, height),
        look,
    );
}

/// Draws an editable text field at `frame` (`NSTextField`).
pub fn text_field(canvas: &mut Canvas<'_>, height: i32, frame: Frame, field: &Field, look: Look) {
    widget::paint(
        canvas,
        &Kind::TextField(*field),
        control_rect(frame, height),
        look,
    );
}

/// Draws a label (`labelWithString:`): one line in `colour`, centred vertically in `frame` and
/// aligned by `align` across it, cut with `...` at its width.
pub fn label(
    canvas: &mut Canvas<'_>,
    height: i32,
    frame: Frame,
    words: &str,
    colour: Colour,
    align: Align,
) {
    if !colour.visible {
        return;
    }
    widget::paint(
        canvas,
        &Kind::Label {
            text: Text::from(words),
            colour: colour.pixel,
            size: 1,
            align,
        },
        pixel_rect(frame, height),
        Look::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u32 = 64;
    const HEIGHT: i32 = 48;
    const INK: Colour = Colour::opaque(0x00ff_0000);

    fn canvas_pixels() -> Vec<u32> {
        vec![0; (WIDTH * HEIGHT as u32) as usize]
    }

    fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
        pixels[(y * WIDTH as i32 + x) as usize]
    }

    #[test]
    fn rect_fill_counts_rows_from_the_bottom_and_clear_draws_nothing() {
        let mut pixels = canvas_pixels();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        fill(&mut canvas, HEIGHT, Frame::new(2.0, 0.0, 4.0, 3.0), INK);
        fill(
            &mut canvas,
            HEIGHT,
            Frame::new(0.0, 0.0, 64.0, 48.0),
            Colour::CLEAR,
        );
        assert_eq!(at(&pixels, 2, 47), INK.pixel);
        assert_eq!(at(&pixels, 5, 45), INK.pixel);
        assert_eq!(at(&pixels, 6, 47), 0);
        assert_eq!(at(&pixels, 2, 44), 0);
    }

    #[test]
    fn outlines_lines_and_ovals_are_one_pixel_wide() {
        let mut pixels = canvas_pixels();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        outline(&mut canvas, HEIGHT, Frame::new(10.0, 10.0, 10.0, 10.0), INK);
        line(
            &mut canvas,
            HEIGHT,
            Point::new(30.0, 40.0),
            Point::new(40.0, 40.0),
            INK,
        );
        oval(
            &mut canvas,
            HEIGHT,
            Frame::new(40.0, 0.0, 20.0, 20.0),
            INK,
            false,
        );
        assert_eq!(at(&pixels, 10, 28), INK.pixel);
        assert_eq!(at(&pixels, 19, 37), INK.pixel);
        assert_eq!(at(&pixels, 15, 33), 0);
        assert_eq!(at(&pixels, 30, 7), INK.pixel);
        assert_eq!(at(&pixels, 40, 7), INK.pixel);
        assert_eq!(at(&pixels, 41, 7), 0);
        assert_eq!(at(&pixels, 50, 37), 0);
        assert_eq!(at(&pixels, 40, 37), INK.pixel);
        let mut filled = canvas_pixels();
        let mut canvas = Canvas::new(&mut filled, WIDTH, HEIGHT as u32, 1).unwrap();
        oval(
            &mut canvas,
            HEIGHT,
            Frame::new(40.0, 0.0, 20.0, 20.0),
            INK,
            true,
        );
        assert_eq!(at(&filled, 50, 37), INK.pixel);
        assert_eq!(at(&filled, 40, 28), 0);
    }

    #[test]
    fn far_away_lines_and_ovals_are_cut_to_the_canvas_and_draw_what_shows() {
        let mut pixels = canvas_pixels();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        line(
            &mut canvas,
            HEIGHT,
            Point::new(-1.0e12, 10.0),
            Point::new(1.0e12, 10.0),
            INK,
        );
        line(
            &mut canvas,
            HEIGHT,
            Point::new(1.0e9, 1.0e9),
            Point::new(2.0e9, 1.0e9),
            INK,
        );
        assert!((0..WIDTH as i32).all(|x| at(&pixels, x, 37) == INK.pixel));
        assert_eq!(
            pixels.iter().filter(|&&pixel| pixel == INK.pixel).count(),
            64
        );
        let mut diagonal = canvas_pixels();
        let mut canvas = Canvas::new(&mut diagonal, WIDTH, HEIGHT as u32, 1).unwrap();
        line(
            &mut canvas,
            HEIGHT,
            Point::new(-1.0e12, -1.0e12),
            Point::new(1.0e12, 1.0e12),
            INK,
        );
        assert!(
            (0..HEIGHT).all(|row| (0..WIDTH as i32).any(|x| at(&diagonal, x, row) == INK.pixel))
        );
        let mut covered = canvas_pixels();
        let mut canvas = Canvas::new(&mut covered, WIDTH, HEIGHT as u32, 1).unwrap();
        oval(
            &mut canvas,
            HEIGHT,
            Frame::new(-1.0e12, -1.0e12, 2.0e12, 2.0e12),
            INK,
            true,
        );
        assert!(covered.iter().all(|&pixel| pixel == INK.pixel));
    }

    #[test]
    fn a_line_inside_the_canvas_is_not_cut_and_one_missing_it_draws_nothing() {
        let clip = Rect::new(0, 0, 64, 48);
        assert_eq!(
            clipped_line((3, 4), (60, 40), clip),
            Some(((3, 4), (60, 40)))
        );
        assert_eq!(
            clipped_line((-5, 10), (70, 10), clip),
            Some(((-2, 10), (66, 10)))
        );
        assert_eq!(clipped_line((-100, -100), (-50, -3), clip), None);
        assert_eq!(clipped_line((100, 0), (100, 47), clip), None);
        let mut pixels = canvas_pixels();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        line(
            &mut canvas,
            HEIGHT,
            Point::new(-1.0e9, -10.0),
            Point::new(1.0e9, -10.0),
            INK,
        );
        assert!(pixels.iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn text_sits_on_its_line_box_bottom_left() {
        let mut pixels = canvas_pixels();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT as u32, 1).unwrap();
        let after = text(&mut canvas, HEIGHT, Point::new(4.0, 4.0), "H", INK, 1);
        assert_eq!(after, 4 + text_width("H", 1));
        let inked_rows: Vec<i32> = (0..HEIGHT)
            .filter(|&y| (0..WIDTH as i32).any(|x| at(&pixels, x, y) == INK.pixel))
            .collect();
        assert!(inked_rows.iter().all(|&y| (28..44).contains(&y)));
        assert!(!inked_rows.is_empty());
        assert_eq!(font_scale(13.0), 1);
        assert_eq!(font_scale(28.0), 2);
    }

    #[test]
    fn controls_are_28_tall_centred_in_their_frame() {
        assert_eq!(
            control_rect(Frame::new(304.0, 8.0, 88.0, 32.0), 300),
            Rect::new(304, 262, 88, 28)
        );
        assert_eq!(
            control_rect(Frame::new(0.0, 0.0, 80.0, 22.0), 100),
            Rect::new(0, 75, 80, 28)
        );
        let mut pixels = vec![0u32; 400 * 300];
        let mut canvas = Canvas::new(&mut pixels, 400, 300, 1).unwrap();
        let look = Look {
            enabled: true,
            ..Look::default()
        };
        button(
            &mut canvas,
            300,
            Frame::new(304.0, 8.0, 88.0, 32.0),
            "Info",
            true,
            look,
        );
        assert_eq!(
            pixels[(276 * 400 + 306) as usize],
            eggos_toolkit::colour::BUTTON_PRIMARY
        );
    }
}

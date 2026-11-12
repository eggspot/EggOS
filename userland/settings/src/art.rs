//! The drawings in Settings (docs/ux/desktop-v0.3-gui.md section 10): each profile card's
//! miniature of its layout, and the egg of the About pane.

use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;

/// Where a card's miniature goes in the card, and its size.
pub const MINIATURE: Rect = Rect::new(16, 8, 96, 56);

const MINI_WINDOW: (i32, i32) = (40, 26);
const TITLE_STRIP: i32 = 5;
const DOT: i32 = 2;
const DOT_STEP: i32 = 3;
const DOT_INSET: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Windows,
    Macos,
    Linux,
}

impl Layout {
    fn of_card(index: usize) -> Self {
        match index {
            0 => Self::Windows,
            1 => Self::Macos,
            _ => Self::Linux,
        }
    }
}

/// Draws card `index`'s miniature into the card at `card`.
pub fn profile_miniature(canvas: &mut Canvas<'_>, index: usize, card: Rect) {
    let layout = Layout::of_card(index);
    let screen = MINIATURE.offset(card.x, card.y);
    canvas.fill(screen, colour::WALLPAPER_TOP);
    canvas.outline(screen, &[], 1, colour::BORDER_ACTIVE);
    let inside = screen.inset(1);
    let free = match layout {
        Layout::Windows => {
            let bar = Rect::new(inside.x, inside.bottom() - 6, inside.width, 6);
            canvas.fill(bar, colour::BAR);
            Rect::new(inside.x, inside.y, inside.width, inside.height - 6)
        }
        Layout::Macos => {
            canvas.fill(Rect::new(inside.x, inside.y, inside.width, 4), colour::BAR);
            let dock = Rect::new(
                inside.x + (inside.width - 40) / 2,
                inside.bottom() - 2 - 8,
                40,
                8,
            );
            canvas.fill(dock, colour::DOCK);
            Rect::new(inside.x, inside.y + 4, inside.width, dock.y - inside.y - 4)
        }
        Layout::Linux => {
            canvas.fill(Rect::new(inside.x, inside.y, inside.width, 4), colour::BAR);
            Rect::new(inside.x, inside.y + 4, inside.width, inside.height - 4)
        }
    };
    let window = Rect::new(
        free.x + (free.width - MINI_WINDOW.0) / 2,
        free.y + (free.height - MINI_WINDOW.1) / 2,
        MINI_WINDOW.0,
        MINI_WINDOW.1,
    );
    canvas.fill(window, colour::WINDOW_BACKGROUND);
    canvas.fill(
        Rect::new(window.x, window.y, window.width, TITLE_STRIP),
        colour::TITLE_ACTIVE,
    );
    canvas.outline(window, &[], 1, colour::BORDER_ACTIVE);
    let dot_y = window.y + (TITLE_STRIP - DOT) / 2;
    let dots: [(i32, u32); 3] = match layout {
        Layout::Macos => [
            (window.x + DOT_INSET, colour::BUTTON_CLOSE),
            (window.x + DOT_INSET + DOT_STEP, colour::BUTTON_MINIMISE),
            (window.x + DOT_INSET + 2 * DOT_STEP, colour::BUTTON_MAXIMISE),
        ],
        Layout::Windows | Layout::Linux => {
            let right = window.right() - DOT_INSET - DOT;
            [
                (right, colour::BUTTON_CLOSE),
                (right - DOT_STEP, colour::BUTTON_MAXIMISE),
                (right - 2 * DOT_STEP, colour::BUTTON_MINIMISE),
            ]
        }
    };
    for (x, dot_colour) in dots {
        canvas.fill(Rect::new(x, dot_y, DOT, DOT), dot_colour);
    }
}

fn isqrt(value: i32) -> i32 {
    let (mut low, mut high) = (0i32, value.clamp(0, 46_340) + 1);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if middle * middle <= value {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// Draws the boot screen's egg and yolk (docs/ux/desktop-v0.3.md sections 1 and 3) as tall
/// as `rect`, at its left.
pub fn egg(canvas: &mut Canvas<'_>, rect: Rect, _: u32) {
    let height = rect.height;
    let width = (72 * height + 50) / 100;
    let equator = ((58 * height + 50) / 100).max(1);
    let centre = rect.x + width / 2;
    for row in 0..height {
        let t = match row < equator {
            true => (equator - row) * 256 / equator,
            false => (row + 1 - equator) * 256 / (height - equator).max(1),
        }
        .min(256);
        let half = (width / 2) * isqrt(65_536 - t * t) / 256;
        canvas.fill(
            Rect::new(centre - half, rect.y + row, 2 * half, 1),
            colour::SHELL,
        );
    }
    let radius = (9 * height + 48) / 96;
    let yolk_y = rect.y + (62 * height + 50) / 100;
    for dy in -radius..radius {
        let half = isqrt(radius * radius - dy * dy);
        canvas.fill(
            Rect::new(centre - half, yolk_y + dy, 2 * half, 1),
            colour::ACCENT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(index: usize) -> Vec<u32> {
        let mut pixels = vec![0u32; 128 * 112];
        let mut canvas = Canvas::new(&mut pixels, 128, 112, 1).unwrap();
        profile_miniature(&mut canvas, index, Rect::new(0, 0, 128, 112));
        pixels
    }

    #[test]
    fn each_miniature_puts_the_panels_and_the_controls_where_its_profile_does() {
        let at = |pixels: &[u32], x: i32, y: i32| pixels[(y * 128 + x) as usize];
        let windows = draw(0);
        let macos = draw(1);
        let linux = draw(2);
        assert_eq!(at(&windows, 50, 8 + 54), colour::BAR);
        assert_eq!(at(&windows, 50, 9), colour::WALLPAPER_TOP);
        assert_eq!(at(&macos, 50, 9), colour::BAR);
        assert_eq!(at(&macos, 64, 8 + 56 - 1 - 2 - 4), colour::DOCK);
        assert_eq!(at(&linux, 50, 9), colour::BAR);
        assert_eq!(at(&linux, 64, 8 + 56 - 1 - 2 - 4), colour::WALLPAPER_TOP);
        let close_at = |pixels: &[u32]| {
            (0..128)
                .find(|&x| (0..112).any(|y| at(pixels, x, y) == colour::BUTTON_CLOSE))
                .unwrap()
        };
        assert!(close_at(&macos) < 64);
        assert!(close_at(&windows) > 64);
        assert!(close_at(&linux) > 64);
        assert_ne!(windows, linux);
    }

    #[test]
    fn the_egg_is_taller_above_its_equator_with_the_yolk_inside() {
        let mut pixels = vec![0u32; 48 * 48];
        let mut canvas = Canvas::new(&mut pixels, 48, 48, 1).unwrap();
        egg(&mut canvas, Rect::new(0, 0, 48, 48), 0);
        let row_width = |y: usize| {
            pixels[y * 48..y * 48 + 48]
                .iter()
                .filter(|&&p| p != 0)
                .count()
        };
        assert!(row_width(28) > row_width(5));
        assert!(row_width(28) <= 36);
        assert_eq!(pixels[30 * 48 + 17], colour::ACCENT);
        assert_eq!(pixels[10 * 48 + 17], colour::SHELL);
    }
}

const PANE_ROW: i32 = 56;
const STROKE: i32 = 2;

fn frame(canvas: &mut Canvas<'_>, rect: Rect, ink: u32) {
    canvas.fill(Rect::new(rect.x, rect.y, rect.width, STROKE), ink);
    canvas.fill(Rect::new(rect.x, rect.bottom() - STROKE, rect.width, STROKE), ink);
    canvas.fill(Rect::new(rect.x, rect.y, STROKE, rect.height), ink);
    canvas.fill(Rect::new(rect.right() - STROKE, rect.y, STROKE, rect.height), ink);
}

/// Pane `pane`'s 24 px line icon (docs/ux/settings-forms-v0.5.md 1.1), its top left at `at`.
pub fn pane_icon(canvas: &mut Canvas<'_>, pane: usize, at: (i32, i32), ink: u32) {
    let (x, y) = at;
    let r = |dx: i32, dy: i32, w: i32, h: i32| Rect::new(x + dx, y + dy, w, h);
    match pane {
        0 => {
            frame(canvas, r(4, 6, 16, 12), ink);
            canvas.fill(r(4, 6, 16, 3), ink);
        }
        1 => {
            for (inset, width) in [(2, 20), (6, 12), (10, 4)] {
                canvas.fill(r(inset, 4 + inset, width, STROKE), ink);
            }
            canvas.fill(r(11, 18, 2, 2), ink);
        }
        2 => {
            frame(canvas, r(3, 4, 18, 12), ink);
            canvas.fill(r(11, 16, 2, 4), ink);
            canvas.fill(r(8, 20, 8, 2), ink);
        }
        3 => {
            frame(canvas, r(2, 6, 20, 12), ink);
            for column in 0..4 {
                canvas.fill(r(5 + 4 * column, 9, 2, 2), ink);
                canvas.fill(r(5 + 4 * column, 13, 2, 2), ink);
            }
        }
        4 => {
            frame(canvas, r(7, 4, 10, 16), ink);
            canvas.fill(r(7, 10, 10, 1), ink);
        }
        5 => {
            frame(canvas, r(9, 2, 6, 6), ink);
            canvas.fill(r(11, 8, 2, 8), ink);
            canvas.fill(r(4, 10, 16, 2), ink);
            canvas.fill(r(8, 16, 2, 6), ink);
            canvas.fill(r(14, 16, 2, 6), ink);
        }
        6 => {
            frame(canvas, r(3, 5, 18, 8), ink);
            frame(canvas, r(3, 13, 18, 8), ink);
        }
        7 => {
            frame(canvas, r(4, 4, 16, 16), ink);
            canvas.fill(r(11, 7, 2, 6), ink);
            canvas.fill(r(11, 11, 5, 2), ink);
        }
        8 => {
            egg(canvas, r(7, 3, 11, 16), 0);
            canvas.fill(r(16, 16, 4, 4), ink);
        }
        9 => {
            for (dx, dy) in [(4, 4), (13, 4), (4, 13), (13, 13)] {
                canvas.fill(r(dx, dy, 7, 7), ink);
            }
        }
        10 => {
            frame(canvas, r(8, 4, 12, 12), ink);
            frame(canvas, r(4, 8, 12, 12), ink);
        }
        _ => {
            frame(canvas, r(4, 4, 16, 16), ink);
            canvas.fill(r(11, 7, 2, 2), ink);
            canvas.fill(r(11, 10, 2, 7), ink);
        }
    }
}

/// The phone's pane list marks (docs/ux/settings-forms-v0.5.md 1.1): each shown pane's icon at
/// its row's left and a `>` at its right, for the panes whose bits `shown` sets, in order, one
/// 56 px row each from `rect`'s top.
pub fn pane_list_marks(canvas: &mut Canvas<'_>, rect: Rect, shown: u32) {
    let palette = canvas.palette();
    let mut row = 0;
    for pane in (0..32).filter(|pane| shown & 1 << pane != 0) {
        let top = rect.y + row * PANE_ROW;
        pane_icon(canvas, pane, (rect.x + 16, top + 16), palette.text);
        canvas.text(rect.right() - 32, top + 20, ">", palette.text_dim, 1);
        row += 1;
    }
}

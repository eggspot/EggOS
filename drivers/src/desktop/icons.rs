//! The shell's icon set (docs/ux/desktop-v0.3-gui.md, section 7, and 6.4.1): Terminal, Files,
//! Settings, Host share and the generic app tile, at 48x48 and 32x32, each a table of shapes
//! drawn with the desktop's primitives, no bitmaps. An icon draws only its own pixels.

use super::colour;
use super::paint::{self, Canvas};
use crate::display::Rect;
use crate::terminal::TERM_BACKGROUND;

pub use eggos_app_manifest::Source;

/// An icon's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// 48x48: the dock, the desktop and the Linux launcher's dash.
    Large,
    /// 32x32: the taskbar, the launcher's rows and the switcher.
    Small,
}

impl IconSize {
    /// Its side in pixels.
    pub const fn pixels(self) -> u32 {
        match self {
            Self::Large => 48,
            Self::Small => 32,
        }
    }
}

/// What an icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// The terminal's prompt on a dark tile.
    Terminal,
    /// A yellow folder.
    Files,
    /// A grey gear.
    Settings,
    /// A folder with a grey front: the host's shared folder.
    HostShare,
    /// A tile with an upper-case letter and, for an installed app, its source's badge.
    App {
        /// The letter shown: `A`-`Z`, `0`-`9` or `?`.
        letter: u8,
        /// The source whose badge it wears, none for a window the catalogue cannot name.
        badge: Option<Source>,
    },
}

impl Icon {
    /// The generic tile for something called `name`, with `badge`: its first ASCII letter or
    /// digit, upper-cased, else `?`.
    pub fn named(name: &str, badge: Option<Source>) -> Self {
        let letter = match name.as_bytes().first() {
            Some(&first) if first.is_ascii_alphanumeric() => first.to_ascii_uppercase(),
            _ => b'?',
        };
        Self::App { letter, badge }
    }
}

#[derive(Debug, Clone, Copy)]
enum Corners {
    R4,
    R6,
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Fill(Rect, u32),
    Rounded(Rect, Corners, u32),
    Tile(Rect, Corners, u32, u32),
    Circle((u32, u32), u32, u32),
    Glyph(u8, (u32, u32), bool, u32),
}

const fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect::new(x, y, width, height)
}

const TERMINAL_LARGE: [Shape; 3] = [
    Shape::Tile(
        rect(0, 0, 48, 48),
        Corners::R6,
        TERM_BACKGROUND,
        colour::ICON_TILE_BORDER,
    ),
    Shape::Glyph(b'>', (8, 8), true, colour::ICON_PROMPT),
    Shape::Glyph(b'_', (24, 8), true, colour::TEXT),
];

const TERMINAL_SMALL: [Shape; 3] = [
    Shape::Tile(
        rect(0, 0, 32, 32),
        Corners::R4,
        TERM_BACKGROUND,
        colour::ICON_TILE_BORDER,
    ),
    Shape::Glyph(b'>', (8, 8), false, colour::ICON_PROMPT),
    Shape::Glyph(b'_', (16, 8), false, colour::TEXT),
];

const fn folder_large(front: u32) -> [Shape; 4] {
    [
        Shape::Fill(rect(6, 8, 18, 8), colour::ICON_FOLDER_BACK),
        Shape::Rounded(rect(6, 12, 36, 28), Corners::R4, colour::ICON_FOLDER_BACK),
        Shape::Rounded(rect(6, 20, 36, 20), Corners::R4, front),
        Shape::Fill(rect(8, 20, 32, 1), colour::ICON_FOLDER_BACK),
    ]
}

const fn folder_small(front: u32) -> [Shape; 4] {
    [
        Shape::Fill(rect(4, 5, 12, 5), colour::ICON_FOLDER_BACK),
        Shape::Rounded(rect(4, 8, 24, 19), Corners::R4, colour::ICON_FOLDER_BACK),
        Shape::Rounded(rect(4, 13, 24, 14), Corners::R4, front),
        Shape::Fill(rect(6, 13, 20, 1), colour::ICON_FOLDER_BACK),
    ]
}

const FILES_LARGE: [Shape; 4] = folder_large(colour::ICON_FOLDER);
const FILES_SMALL: [Shape; 4] = folder_small(colour::ICON_FOLDER);
const HOST_SHARE_LARGE: [Shape; 4] = folder_large(colour::ICON_GEAR);
const HOST_SHARE_SMALL: [Shape; 4] = folder_small(colour::ICON_GEAR);

const SETTINGS_LARGE: [Shape; 8] = [
    Shape::Circle((24, 24), 14, colour::ICON_GEAR),
    Shape::Fill(rect(20, 2, 8, 44), colour::ICON_GEAR),
    Shape::Fill(rect(2, 20, 44, 8), colour::ICON_GEAR),
    Shape::Fill(rect(8, 8, 8, 8), colour::ICON_GEAR),
    Shape::Fill(rect(32, 8, 8, 8), colour::ICON_GEAR),
    Shape::Fill(rect(8, 32, 8, 8), colour::ICON_GEAR),
    Shape::Fill(rect(32, 32, 8, 8), colour::ICON_GEAR),
    Shape::Circle((24, 24), 6, colour::ICON_GEAR_HOLE),
];

const SETTINGS_SMALL: [Shape; 8] = [
    Shape::Circle((16, 16), 9, colour::ICON_GEAR),
    Shape::Fill(rect(13, 2, 6, 28), colour::ICON_GEAR),
    Shape::Fill(rect(2, 13, 28, 6), colour::ICON_GEAR),
    Shape::Fill(rect(5, 5, 6, 6), colour::ICON_GEAR),
    Shape::Fill(rect(21, 5, 6, 6), colour::ICON_GEAR),
    Shape::Fill(rect(5, 21, 6, 6), colour::ICON_GEAR),
    Shape::Fill(rect(21, 21, 6, 6), colour::ICON_GEAR),
    Shape::Circle((16, 16), 4, colour::ICON_GEAR_HOLE),
];

fn badge_colours(source: Source) -> (u32, u8, u32) {
    match source {
        Source::Windows => (colour::ICON_WINDOWS, b'W', colour::TEXT),
        Source::Macos => (colour::ICON_MAC, b'M', colour::ICON_DARK),
        Source::Linux => (colour::ICON_LINUX, b'L', colour::ICON_DARK),
    }
}

fn app_shapes(letter: u8, badge: Option<Source>, size: IconSize) -> ([Shape; 4], usize) {
    let none = Shape::Fill(rect(0, 0, 0, 0), 0);
    let mut shapes = [none; 4];
    let length = match size {
        IconSize::Large => {
            shapes[0] = Shape::Tile(
                rect(0, 0, 48, 48),
                Corners::R6,
                colour::ICON_TILE,
                colour::ICON_TILE_BORDER,
            );
            shapes[1] = Shape::Glyph(letter, (8, 6), true, colour::TEXT);
            match badge.map(badge_colours) {
                Some((fill, mark, ink)) => {
                    shapes[2] = Shape::Rounded(rect(30, 26, 16, 20), Corners::R4, fill);
                    shapes[3] = Shape::Glyph(mark, (34, 28), false, ink);
                    4
                }
                None => 2,
            }
        }
        IconSize::Small => {
            shapes[0] = Shape::Tile(
                rect(0, 0, 32, 32),
                Corners::R4,
                colour::ICON_TILE,
                colour::ICON_TILE_BORDER,
            );
            shapes[1] = Shape::Glyph(letter, (6, 6), false, colour::TEXT);
            match badge.map(badge_colours) {
                Some((fill, _, _)) => {
                    shapes[2] = Shape::Rounded(rect(20, 20, 10, 10), Corners::R4, fill);
                    3
                }
                None => 2,
            }
        }
    };
    (shapes, length)
}

fn insets(corners: Corners) -> (&'static [u32], &'static [u32]) {
    match corners {
        Corners::R4 => (&paint::INSET_R4, &paint::INSET_INSIDE_R4),
        Corners::R6 => (&paint::INSET_R6, &paint::INSET_INSIDE_R6),
    }
}

fn draw_shape(canvas: &mut Canvas<'_>, shape: Shape, x: u32, y: u32) {
    let moved = |area: Rect| Rect::new(x + area.x, y + area.y, area.width, area.height);
    match shape {
        Shape::Fill(area, colour) => canvas.fill(moved(area), colour),
        Shape::Rounded(area, corners, colour) => {
            canvas.rounded(moved(area), insets(corners).0, true, true, colour)
        }
        Shape::Tile(area, corners, fill, border) => {
            let (outside, inside) = insets(corners);
            let area = moved(area);
            canvas.rounded(area, outside, true, true, border);
            canvas.rounded(
                Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2),
                inside,
                true,
                true,
                fill,
            );
        }
        Shape::Circle((centre_x, centre_y), radius, colour) => {
            canvas.circle(x + centre_x, y + centre_y, radius, colour)
        }
        Shape::Glyph(character, (left, top), doubled, colour) => {
            let id = crate::font::glyph_id(u32::from(character));
            match doubled {
                true => canvas.glyph_doubled(x + left, y + top, id, colour),
                false => canvas.glyph_at(x + left, y + top, id, colour),
            }
        }
    }
}

/// Draws `icon` at `size` with its top-left corner at (`x`, `y`).
pub(super) fn draw_icon(canvas: &mut Canvas<'_>, icon: Icon, size: IconSize, x: u32, y: u32) {
    let area = Rect::new(x, y, size.pixels(), size.pixels());
    if !area.overlaps(&canvas.clip()) {
        return;
    }
    let generic;
    let shapes: &[Shape] = match (icon, size) {
        (Icon::Terminal, IconSize::Large) => &TERMINAL_LARGE,
        (Icon::Terminal, IconSize::Small) => &TERMINAL_SMALL,
        (Icon::Files, IconSize::Large) => &FILES_LARGE,
        (Icon::Files, IconSize::Small) => &FILES_SMALL,
        (Icon::HostShare, IconSize::Large) => &HOST_SHARE_LARGE,
        (Icon::HostShare, IconSize::Small) => &HOST_SHARE_SMALL,
        (Icon::Settings, IconSize::Large) => &SETTINGS_LARGE,
        (Icon::Settings, IconSize::Small) => &SETTINGS_SMALL,
        (Icon::App { letter, badge }, size) => {
            generic = app_shapes(letter, badge, size);
            &generic.0[..generic.1]
        }
    };
    for &shape in shapes {
        draw_shape(canvas, shape, x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKGROUND: u32 = 0x00ff_00ff;

    fn pixels(icon: Icon, size: IconSize) -> Vec<u32> {
        let side = size.pixels();
        let mut pixels = vec![BACKGROUND; (side * side) as usize];
        let mut canvas =
            Canvas::new(&mut pixels, side as usize, Rect::new(0, 0, side, side)).unwrap();
        draw_icon(&mut canvas, icon, size, 0, 0);
        pixels
    }

    fn hash(pixels: &[u32]) -> u64 {
        pixels
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
    }

    const ICONS: [(&str, Icon); 8] = [
        ("terminal", Icon::Terminal),
        ("files", Icon::Files),
        ("settings", Icon::Settings),
        ("host share", Icon::HostShare),
        (
            "windows app",
            Icon::App {
                letter: b'P',
                badge: Some(Source::Windows),
            },
        ),
        (
            "macos app",
            Icon::App {
                letter: b'T',
                badge: Some(Source::Macos),
            },
        ),
        (
            "linux app",
            Icon::App {
                letter: b'H',
                badge: Some(Source::Linux),
            },
        ),
        (
            "unnamed window",
            Icon::App {
                letter: b'W',
                badge: None,
            },
        ),
    ];

    const HASHES: [(u64, u64); 8] = [
        (0xbe3f_6501_e98e_9c05, 0xa4ef_8e1b_e8b1_94dd),
        (0x3c45_dcd0_382f_91b3, 0x43d6_fa50_8b2e_fa6b),
        (0x469c_345a_7d45_0035, 0xed8a_a48c_d789_b9c9),
        (0x6e1c_c1d8_a65e_0613, 0xb7bd_dd54_63f6_1bd3),
        (0x3ad4_f94b_1f29_1ec9, 0xeb1b_7a0c_7b9e_c284),
        (0x262b_ed77_87b7_1e85, 0xf9bd_51be_6fce_ad5c),
        (0x088d_2393_d826_6241, 0x11c6_81e7_c78e_f015),
        (0x66f2_5e36_4088_91e5, 0xf13f_e985_5afa_53b5),
    ];

    #[test]
    fn every_icon_s_pixels_are_pinned_at_both_sizes() {
        let mut found = Vec::new();
        for (name, icon) in ICONS {
            let large = hash(&pixels(icon, IconSize::Large));
            let small = hash(&pixels(icon, IconSize::Small));
            found.push(format!("{name}: ({large:#x}, {small:#x})"));
        }
        let pinned: Vec<String> = ICONS
            .iter()
            .zip(HASHES)
            .map(|((name, _), (large, small))| format!("{name}: ({large:#x}, {small:#x})"))
            .collect();
        assert_eq!(found, pinned);
    }

    #[test]
    fn icons_draw_only_inside_their_square_and_leave_corners_transparent() {
        for (name, icon) in ICONS {
            for size in [IconSize::Large, IconSize::Small] {
                let side = size.pixels();
                let mut pixels = vec![BACKGROUND; (3 * side * 3 * side) as usize];
                let screen = Rect::new(0, 0, 3 * side, 3 * side);
                let mut canvas = Canvas::new(&mut pixels, 3 * side as usize, screen).unwrap();
                draw_icon(&mut canvas, icon, size, side, side);
                for y in 0..3 * side {
                    for x in 0..3 * side {
                        let inside = (side..2 * side).contains(&x) && (side..2 * side).contains(&y);
                        if !inside {
                            assert_eq!(pixels[(y * 3 * side + x) as usize], BACKGROUND, "{name}");
                        }
                    }
                }
                assert_eq!(
                    pixels[(side * 3 * side + side) as usize],
                    BACKGROUND,
                    "{name}: the top-left corner is transparent"
                );
            }
        }
    }

    #[test]
    fn the_terminal_shows_its_prompt_and_the_gear_its_hole() {
        let terminal = pixels(Icon::Terminal, IconSize::Large);
        assert!(terminal.contains(&colour::ICON_PROMPT));
        assert_eq!(terminal[24 * 48 + 2], TERM_BACKGROUND);
        let gear = pixels(Icon::Settings, IconSize::Large);
        assert_eq!(gear[24 * 48 + 24], colour::ICON_GEAR_HOLE);
        assert_eq!(gear[24 * 48 + 3], colour::ICON_GEAR);
        let share = pixels(Icon::HostShare, IconSize::Small);
        assert!(share.contains(&colour::ICON_GEAR));
        assert!(!share.contains(&colour::ICON_FOLDER));
    }

    #[test]
    fn a_name_gives_its_first_letter_or_a_question_mark() {
        assert_eq!(
            Icon::named("paint", Some(Source::Windows)),
            Icon::App {
                letter: b'P',
                badge: Some(Source::Windows)
            }
        );
        assert_eq!(
            Icon::named("7zip", None),
            Icon::App {
                letter: b'7',
                badge: None
            }
        );
        for name in ["", " x", "émoji", "-dash"] {
            assert_eq!(
                Icon::named(name, None),
                Icon::App {
                    letter: b'?',
                    badge: None
                }
            );
        }
    }
}

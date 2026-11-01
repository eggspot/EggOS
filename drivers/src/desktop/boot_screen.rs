//! The boot screen (docs/ux/desktop-v0.3.md section 1, docs/ux/desktop-polish-v0.3.md section 3,
//! D186): the egg, `EggOS`, the version and four progress segments on the wallpaper's top
//! colour, in one of three tiers chosen from the display's size. Every row is computed when it is
//! drawn ([`BootScreen::draw_span`]), so the screen is a few numbers and the kernel can hash it a
//! row at a time.

use super::colour;
use super::paint::{egg_half_width, isqrt};
use crate::display::Rect;
use crate::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

/// The stages the progress shows: the display, the input devices, init, the desktop.
pub const STAGES: u8 = 4;
/// The logo's egg.
pub const EGG: u32 = 0x00f6_f1e7;
/// A progress segment not lit yet.
pub const UNLIT: u32 = 0x0026_303f;
/// What the screen says under the name.
pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));

const NAME: &str = "EggOS";
const SEGMENT_GAP: u32 = 4;
const SEGMENT_HEIGHT: u32 = 4;
const FULL_LEAST: (u32, u32) = (188, 212);
const COMPACT_LEAST: (u32, u32) = (124, 104);
const MINIMAL_LEAST: (u32, u32) = (32, 8);
const FULL_EGG: u32 = 96;
const FULL_YOLK: (u32, u32) = (60, 9);
const FULL_NAME_GAP: u32 = 16;
const FULL_VERSION_GAP: u32 = 8;
const FULL_PROGRESS_GAP: u32 = 24;
const FULL_SEGMENT: u32 = 40;
const COMPACT_EGG: u32 = 48;
const COMPACT_YOLK: (u32, u32) = (30, 5);
const COMPACT_NAME_GAP: u32 = 8;
const COMPACT_PROGRESS_GAP: u32 = 12;
const COMPACT_SEGMENT: u32 = 24;
const COMPACT_HEIGHT: u32 =
    COMPACT_EGG + COMPACT_NAME_GAP + GLYPH_HEIGHT as u32 + COMPACT_PROGRESS_GAP + SEGMENT_HEIGHT;
const MINIMAL_MARGINS: u32 = 2 * 8 + 3 * SEGMENT_GAP;

/// How much of the design a display has room for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The egg, the name at 2x, the version and the 172 px progress.
    Full,
    /// A smaller egg, the name at 1x and a 108 px progress.
    Compact,
    /// The progress alone, across the display.
    Minimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Logo {
    top: u32,
    centre: u32,
    height: u32,
    yolk_centre: u32,
    yolk_radius: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line {
    x: u32,
    y: u32,
    scale: u32,
    text: &'static str,
    colour: u32,
}

impl Line {
    fn centred(centre: u32, y: u32, scale: u32, text: &'static str, colour: u32) -> Self {
        let width = text.len() as u32 * GLYPH_WIDTH as u32 * scale;
        Self {
            x: centre.saturating_sub(width / 2),
            y,
            scale,
            text,
            colour,
        }
    }

    fn rect(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.text.len() as u32 * GLYPH_WIDTH as u32 * self.scale,
            GLYPH_HEIGHT as u32 * self.scale,
        )
    }
}

/// The boot screen of a display of one size, with how many stages its progress shows lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootScreen {
    width: u32,
    height: u32,
    tier: Tier,
    logo: Option<Logo>,
    name: Option<Line>,
    version: Option<Line>,
    progress: Rect,
    segment_width: u32,
    lit: u8,
}

impl BootScreen {
    /// The boot screen of a `width` by `height` display with its first stage, the display, lit;
    /// `None` below 32x8, where there is no room for the progress.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let centre = width / 2;
        let fits = |least: (u32, u32)| width >= least.0 && height >= least.1;
        let (tier, logo, name, version, progress_top, segment_width) = if fits(FULL_LEAST) {
            let top = height / 2 - FULL_EGG;
            let name_top = top + FULL_EGG + FULL_NAME_GAP;
            let version_top = name_top + 2 * GLYPH_HEIGHT as u32 + FULL_VERSION_GAP;
            (
                Tier::Full,
                Some(Logo {
                    top,
                    centre,
                    height: FULL_EGG,
                    yolk_centre: top + FULL_YOLK.0,
                    yolk_radius: FULL_YOLK.1,
                }),
                Some(Line::centred(centre, name_top, 2, NAME, colour::TEXT)),
                Some(Line::centred(
                    centre,
                    version_top,
                    1,
                    VERSION,
                    colour::TEXT_DIM,
                )),
                version_top + GLYPH_HEIGHT as u32 + FULL_PROGRESS_GAP,
                FULL_SEGMENT,
            )
        } else if fits(COMPACT_LEAST) {
            let top = (height - COMPACT_HEIGHT) / 2;
            let name_top = top + COMPACT_EGG + COMPACT_NAME_GAP;
            (
                Tier::Compact,
                Some(Logo {
                    top,
                    centre,
                    height: COMPACT_EGG,
                    yolk_centre: top + COMPACT_YOLK.0,
                    yolk_radius: COMPACT_YOLK.1,
                }),
                Some(Line::centred(centre, name_top, 1, NAME, colour::TEXT)),
                None,
                name_top + GLYPH_HEIGHT as u32 + COMPACT_PROGRESS_GAP,
                COMPACT_SEGMENT,
            )
        } else if fits(MINIMAL_LEAST) {
            (
                Tier::Minimal,
                None,
                None,
                None,
                (height - SEGMENT_HEIGHT) / 2,
                (width - MINIMAL_MARGINS) / 4,
            )
        } else {
            return None;
        };
        let bar_width = 4 * segment_width + 3 * SEGMENT_GAP;
        Some(Self {
            width,
            height,
            tier,
            logo,
            name,
            version,
            progress: Rect::new(
                (width - bar_width) / 2,
                progress_top,
                bar_width,
                SEGMENT_HEIGHT,
            ),
            segment_width,
            lit: 1,
        })
    }

    /// How much of the design it shows.
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// How many stages its progress shows lit, 1 to [`STAGES`].
    pub fn lit(&self) -> u8 {
        self.lit
    }

    /// The whole progress bar.
    pub fn progress(&self) -> Rect {
        self.progress
    }

    /// Progress segment `index`, from 0.
    pub fn segment(&self, index: u8) -> Rect {
        Rect::new(
            self.progress.x + u32::from(index) * (self.segment_width + SEGMENT_GAP),
            self.progress.y,
            self.segment_width,
            SEGMENT_HEIGHT,
        )
    }

    /// The egg's bounding box, when the tier has one.
    pub fn logo(&self) -> Option<Rect> {
        self.logo.map(|logo| {
            let half = egg_half_width(logo.height, logo.height * 58 / 100);
            Rect::new(
                logo.centre.saturating_sub(half),
                logo.top,
                2 * half,
                logo.height,
            )
        })
    }

    /// The name's rectangle, when the tier shows it.
    pub fn name(&self) -> Option<Rect> {
        self.name.map(|line| line.rect())
    }

    /// The version's rectangle, when the tier shows it.
    pub fn version(&self) -> Option<Rect> {
        self.version.map(|line| line.rect())
    }

    /// Lights the first `lit` stages (at most [`STAGES`]); returns the segments that changed,
    /// what a display then redraws and shows, or `None` when nothing did: the progress never
    /// goes back.
    pub fn light(&mut self, lit: u8) -> Option<Rect> {
        let lit = lit.min(STAGES);
        if lit <= self.lit {
            return None;
        }
        let changed = (self.lit..lit).fold(Rect::default(), |changed, index| {
            changed.union(self.segment(index))
        });
        self.lit = lit;
        Some(changed)
    }

    /// Writes pixels `left..left + out.len()` of row `y` into `out`.
    pub fn draw_span(&self, y: u32, left: u32, out: &mut [u32]) {
        out.fill(colour::WALLPAPER_TOP);
        if y >= self.height {
            return;
        }
        let right = left.saturating_add(out.len() as u32);
        let mut fill = |from: u32, to: u32, colour: u32| {
            let (from, to) = (from.max(left), to.min(right));
            if from < to {
                out[(from - left) as usize..(to - left) as usize].fill(colour);
            }
        };
        if let Some(logo) = self.logo {
            if let Some(row) = y.checked_sub(logo.top).filter(|&row| row < logo.height) {
                let half = egg_half_width(logo.height, row);
                fill(logo.centre.saturating_sub(half), logo.centre + half, EGG);
            }
            let distance = y.abs_diff(logo.yolk_centre);
            if distance < logo.yolk_radius {
                let radius = logo.yolk_radius;
                let half = isqrt(radius * radius - distance * distance);
                fill(
                    logo.centre.saturating_sub(half),
                    logo.centre + half,
                    colour::ACCENT,
                );
            }
        }
        for line in [self.name, self.version].into_iter().flatten() {
            let Some(row) = y
                .checked_sub(line.y)
                .filter(|&row| row < GLYPH_HEIGHT as u32 * line.scale)
            else {
                continue;
            };
            let glyph_row = (row / line.scale) as usize;
            for (index, character) in (0u32..).zip(line.text.chars()) {
                let bits = font::rows(font::glyph_id(u32::from(character)))[glyph_row];
                for column in 0..GLYPH_WIDTH as u32 {
                    if bits & (0x80 >> column) != 0 {
                        let x = line.x + (index * GLYPH_WIDTH as u32 + column) * line.scale;
                        fill(x, x + line.scale, line.colour);
                    }
                }
            }
        }
        if y >= self.progress.y && y < self.progress.bottom() {
            for index in 0..STAGES {
                let segment = self.segment(index);
                let colour = match index < self.lit {
                    true => colour::ACCENT,
                    false => UNLIT,
                };
                fill(segment.x, segment.right(), colour);
            }
        }
    }

    /// Draws the part of the screen inside `clip` into `pixels`, rows `stride` pixels apart;
    /// nothing outside the display or `pixels`.
    pub fn draw(&self, pixels: &mut [u32], stride: usize, clip: Rect) {
        let clip = clip.clipped(self.width, self.height);
        if clip.right() as usize > stride {
            return;
        }
        for y in clip.y..clip.bottom() {
            let start = y as usize * stride + clip.x as usize;
            let Some(out) = pixels.get_mut(start..start + clip.width as usize) else {
                return;
            };
            self.draw_span(y, clip.x, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(screen: &BootScreen) -> Vec<u32> {
        let (width, height) = (screen.width as usize, screen.height as usize);
        let mut pixels = vec![0; width * height];
        screen.draw(
            &mut pixels,
            width,
            Rect::new(0, 0, screen.width, screen.height),
        );
        pixels
    }

    fn at(screen: &BootScreen, pixels: &[u32], x: u32, y: u32) -> u32 {
        pixels[(y * screen.width + x) as usize]
    }

    #[test]
    fn at_1280x800_the_whole_design_stacks_around_the_middle() {
        let screen = BootScreen::new(1280, 800).unwrap();
        assert_eq!(screen.tier(), Tier::Full);
        assert_eq!(screen.lit(), 1);
        assert_eq!(screen.name(), Some(Rect::new(600, 416, 80, 32)));
        assert_eq!(
            screen.version(),
            Some(Rect::new(
                640 - VERSION.len() as u32 * 4,
                456,
                VERSION.len() as u32 * 8,
                16
            ))
        );
        assert_eq!(screen.progress(), Rect::new(554, 496, 172, 4));
        assert_eq!(screen.segment(1), Rect::new(598, 496, 40, 4));
        let logo = screen.logo().unwrap();
        assert_eq!((logo.y, logo.height), (304, 96));
        assert!(logo.width >= 66 && logo.width <= 69, "{logo:?}");
        let pixels = drawn(&screen);
        assert_eq!(at(&screen, &pixels, 0, 0), colour::WALLPAPER_TOP);
        assert_eq!(at(&screen, &pixels, 640, 304), colour::WALLPAPER_TOP);
        assert_eq!(at(&screen, &pixels, 640, 344), EGG);
        assert_eq!(at(&screen, &pixels, 640, 364), colour::ACCENT);
        assert_eq!(at(&screen, &pixels, 640, 399), colour::WALLPAPER_TOP);
        assert_eq!(at(&screen, &pixels, 554, 496), colour::ACCENT);
        assert_eq!(at(&screen, &pixels, 593, 499), colour::ACCENT);
        assert_eq!(at(&screen, &pixels, 594, 496), colour::WALLPAPER_TOP);
        assert_eq!(at(&screen, &pixels, 598, 496), UNLIT);
        assert_eq!(at(&screen, &pixels, 725, 499), UNLIT);
        assert_eq!(at(&screen, &pixels, 726, 499), colour::WALLPAPER_TOP);
        let text = |colour: u32, rect: Rect| {
            (rect.y..rect.bottom())
                .flat_map(|y| (rect.x..rect.right()).map(move |x| (x, y)))
                .filter(|&(x, y)| at(&screen, &pixels, x, y) == colour)
                .count()
        };
        let name = screen.name().unwrap();
        assert!(text(colour::TEXT, name) > 0);
        assert_eq!(text(colour::TEXT, screen.version().unwrap()), 0);
        assert!(text(colour::TEXT_DIM, screen.version().unwrap()) > 0);
        assert_eq!(
            text(colour::TEXT, Rect::new(0, 0, 1280, 800)),
            text(colour::TEXT, name)
        );
        for (row, &bits) in font::rows(b'E').iter().enumerate() {
            for column in 0..8 {
                let ink = bits & (0x80 >> column) != 0;
                let (x, y) = (600 + 2 * column, 416 + 2 * row as u32);
                let expected = match ink {
                    true => colour::TEXT,
                    false => colour::WALLPAPER_TOP,
                };
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    assert_eq!(at(&screen, &pixels, x + dx, y + dy), expected, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn lighting_a_stage_redraws_only_the_segments_that_changed_and_never_goes_back() {
        let mut screen = BootScreen::new(1280, 800).unwrap();
        assert_eq!(screen.light(1), None);
        assert_eq!(screen.light(3), Some(Rect::new(598, 496, 84, 4)));
        assert_eq!(screen.lit(), 3);
        let pixels = drawn(&screen);
        assert_eq!(at(&screen, &pixels, 642, 497), colour::ACCENT);
        assert_eq!(at(&screen, &pixels, 686, 497), UNLIT);
        assert_eq!(screen.light(2), None);
        assert_eq!(screen.light(9), Some(screen.segment(3)));
        assert_eq!(screen.lit(), STAGES);
        assert_eq!(screen.light(STAGES), None);
    }

    #[test]
    fn smaller_displays_drop_the_version_then_shrink_then_keep_only_the_progress() {
        let full = BootScreen::new(320, 240).unwrap();
        assert_eq!(full.tier(), Tier::Full);
        assert_eq!(full.logo().unwrap().y, 24);
        assert_eq!(full.progress().bottom(), 220);
        assert_eq!(BootScreen::new(188, 212).unwrap().tier(), Tier::Full);
        let compact = BootScreen::new(160, 128).unwrap();
        assert_eq!(compact.tier(), Tier::Compact);
        assert_eq!(compact.version(), None);
        assert_eq!(compact.name(), Some(Rect::new(60, 76, 40, 16)));
        assert_eq!(compact.progress(), Rect::new(26, 104, 108, 4));
        assert_eq!(compact.logo().unwrap().y, 20);
        assert_eq!(BootScreen::new(187, 212).unwrap().tier(), Tier::Compact);
        let minimal = BootScreen::new(100, 50).unwrap();
        assert_eq!(minimal.tier(), Tier::Minimal);
        assert_eq!((minimal.logo(), minimal.name()), (None, None));
        assert_eq!(minimal.progress(), Rect::new(8, 23, 84, 4));
        assert_eq!(minimal.segment(3), Rect::new(74, 23, 18, 4));
        assert_eq!(
            BootScreen::new(32, 8).unwrap().progress(),
            Rect::new(8, 2, 16, 4)
        );
        assert!(BootScreen::new(31, 8).is_none());
        assert!(BootScreen::new(32, 7).is_none());
        assert!(BootScreen::new(0, 0).is_none());
    }

    #[test]
    fn drawing_in_pieces_and_a_row_at_a_time_gives_the_whole_picture() {
        for (width, height) in [(1280, 800), (1024, 768), (160, 128), (40, 9)] {
            let screen = BootScreen::new(width, height).unwrap();
            let whole = drawn(&screen);
            let mut pieces = vec![0; whole.len()];
            for (x, y) in [
                (0, 0),
                (width / 3, 0),
                (0, height / 2),
                (width / 3, height / 2),
            ] {
                let rect = Rect::new(x, y, width - x, height - y);
                screen.draw(&mut pieces, width as usize, rect);
            }
            assert!(pieces == whole, "{width}x{height}");
            let mut row = vec![0; width as usize];
            for y in 0..height {
                screen.draw_span(y, 0, &mut row);
                let start = (y * width) as usize;
                assert!(row[..] == whole[start..start + width as usize], "row {y}");
            }
        }
    }
}

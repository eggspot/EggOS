//! The standard About panel (`docs/ux/macos-gui-v0.3.md` section 8, D175): what
//! `orderFrontStandardAboutPanel:` shows, drawn by the layer like the alert. Its lines (the
//! app's name at 2x, the version line and the copyright's first line at 1x) are centred on its
//! 288 pixel width, each cut with `...` at 256; its icon is the installed app's own 48 pixel
//! image or, without one, the generic tile with the macOS badge.

use eggos_toolkit::canvas::{Canvas, INSET_R4, INSET_R6};
use eggos_toolkit::colour as token;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::text;

/// The content's width.
pub const WIDTH: i32 = 288;
/// The content's height with no body line.
pub const BASE_HEIGHT: i32 = 140;
/// The distance between body lines.
pub const LINE_PITCH: i32 = 20;
/// The width every line is cut at.
pub const TEXT_SPACE: i32 = 256;
/// Where the icon is drawn.
pub const ICON: Rect = Rect {
    x: 120,
    y: 16,
    width: 48,
    height: 48,
};
/// The icon's side in pixels.
pub const ICON_SIDE: usize = 48;
/// The pixels of the icon.
pub const ICON_PIXELS: usize = ICON_SIDE * ICON_SIDE;
/// The exact size of an installed app's icon file (`app.icon`: the 48 and 32 pixel images).
pub const ICON_FILE_BYTES: usize = (ICON_PIXELS + 32 * 32) * 4;
/// The window server's border and title bar around a window's content: width and height.
pub const CHROME: (i32, i32) = (2, 30);
/// The longest version line kept, in bytes.
pub const VERSION_BYTES: usize = 128;

const NAME_TOP: i32 = 80;
const BODY_TOP: i32 = 128;
const NAME_SCALE: i32 = 2;
const LINE_HEIGHT: i32 = 16;
const TILE_LETTER: (i32, i32) = (8, 6);
const BADGE: Rect = Rect {
    x: 30,
    y: 26,
    width: 16,
    height: 20,
};
const BADGE_LETTER: (i32, i32) = (34, 28);

/// `Version <short> (<build>)`, or `Version <short>` when the two are equal or only one is
/// given (then that one); `None` when neither is.
pub fn version_line<'a>(
    short: &str,
    build: &str,
    out: &'a mut [u8; VERSION_BYTES],
) -> Option<&'a str> {
    let mut length = 0;
    let mut push = |piece: &str| {
        for &byte in piece.as_bytes() {
            if length < out.len() {
                out[length] = byte;
                length += 1;
            }
        }
    };
    match (short.is_empty(), build.is_empty()) {
        (true, true) => return None,
        (false, false) if short != build => {
            push("Version ");
            push(short);
            push(" (");
            push(build);
            push(")");
        }
        (false, _) => {
            push("Version ");
            push(short);
        }
        (true, false) => {
            push("Version ");
            push(build);
        }
    }
    let mut end = length;
    while core::str::from_utf8(&out[..end]).is_err() {
        end -= 1;
    }
    core::str::from_utf8(&out[..end]).ok()
}

/// `text` up to its first line break.
pub fn first_line(text: &str) -> &str {
    text.split(['\n', '\r']).next().unwrap_or("")
}

/// The letter the generic tile shows: the name's first ASCII letter or digit, upper-cased, else
/// `?`.
pub fn initial(name: &str) -> char {
    name.chars()
        .find(char::is_ascii_alphanumeric)
        .map_or('?', |letter| letter.to_ascii_uppercase())
}

/// The installed app's 48 pixel icon from its `app.icon` file, each channel clamped to the
/// pixel's alpha; false for a file of another size.
pub fn icon_pixels(file: &[u8], out: &mut [u32; ICON_PIXELS]) -> bool {
    if file.len() != ICON_FILE_BYTES {
        return false;
    }
    for (pixel, bytes) in out.iter_mut().zip(file.chunks_exact(4)) {
        let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let alpha = value >> 24;
        let clamp = |shift: u32| (value >> shift & 0xff).min(alpha) << shift;
        *pixel = alpha << 24 | clamp(16) | clamp(8) | clamp(0);
    }
    true
}

fn blend(source: u32, under: u32) -> u32 {
    let alpha = source >> 24;
    match alpha {
        255 => source & 0x00ff_ffff,
        0 => under & 0x00ff_ffff,
        _ => {
            let mix = |shift: u32| {
                let (from, below) = (source >> shift & 0xff, under >> shift & 0xff);
                (from + (below * (255 - alpha) + 127) / 255).min(255)
            };
            mix(16) << 16 | mix(8) << 8 | mix(0)
        }
    }
}

/// One line as drawn: its cell's rectangle, how many bytes of its text are shown and whether
/// `...` follows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub rect: Rect,
    pub shown: usize,
    pub ellipsis: bool,
}

fn place(words: &str, top: i32, scale: i32) -> Placed {
    let (shown, ellipsis) = text::fit(words, TEXT_SPACE / scale);
    let extra = if ellipsis {
        text::width(text::ELLIPSIS)
    } else {
        0
    };
    let width = (text::width(&words[..shown]) + extra) * scale;
    Placed {
        rect: Rect::new((WIDTH - width) / 2, top, width, LINE_HEIGHT * scale),
        shown,
        ellipsis,
    }
}

/// The panel's text: the app's name, its version line and its copyright's first line (each
/// empty when there is none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panel<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub copyright: &'a str,
}

impl Panel<'_> {
    fn body_lines(&self) -> i32 {
        i32::from(!self.version.is_empty()) + i32::from(!self.copyright.is_empty())
    }

    /// The content's height.
    pub fn height(&self) -> i32 {
        BASE_HEIGHT + LINE_PITCH * self.body_lines()
    }

    /// Where the name is drawn.
    pub fn name_line(&self) -> Placed {
        place(self.name, NAME_TOP, NAME_SCALE)
    }

    /// Where the version line is drawn, when there is one.
    pub fn version_line(&self) -> Option<Placed> {
        (!self.version.is_empty()).then(|| place(self.version, BODY_TOP, 1))
    }

    /// Where the copyright line is drawn, when there is one.
    pub fn copyright_line(&self) -> Option<Placed> {
        let top = BODY_TOP + LINE_PITCH * i32::from(!self.version.is_empty());
        (!self.copyright.is_empty()).then(|| place(self.copyright, top, 1))
    }

    /// Draws the panel filling `canvas` (`WIDTH` by [`Panel::height`]), its icon from
    /// `icon` (the 48 pixel image, premultiplied) or the generic tile.
    pub fn paint(&self, canvas: &mut Canvas<'_>, icon: Option<&[u32; ICON_PIXELS]>) {
        let content = Rect::new(0, 0, WIDTH, self.height());
        canvas.fill(content, token::WINDOW_BACKGROUND);
        canvas.rounded(content, &INSET_R6, token::MENU);
        canvas.outline(content, &INSET_R6, 1, token::MENU_BORDER);
        match icon {
            Some(pixels) => {
                for (index, &pixel) in pixels.iter().enumerate() {
                    let x = ICON.x + (index % ICON_SIDE) as i32;
                    let y = ICON.y + (index / ICON_SIDE) as i32;
                    canvas.pixel(x, y, blend(pixel, token::MENU));
                }
            }
            None => self.paint_tile(canvas),
        }
        let lines = [
            (self.name, Some(self.name_line()), NAME_SCALE, token::TEXT),
            (self.version, self.version_line(), 1, token::TEXT_DIM),
            (self.copyright, self.copyright_line(), 1, token::TEXT_DIM),
        ];
        for (words, placed, scale, colour) in lines {
            let Some(placed) = placed else {
                continue;
            };
            let after = canvas.text(
                placed.rect.x,
                placed.rect.y,
                &words[..placed.shown],
                colour,
                scale,
            );
            if placed.ellipsis {
                canvas.text(after, placed.rect.y, text::ELLIPSIS, colour, scale);
            }
        }
    }

    fn paint_tile(&self, canvas: &mut Canvas<'_>) {
        canvas.rounded(ICON, &INSET_R6, token::ICON_TILE);
        canvas.outline(ICON, &INSET_R6, 1, token::ICON_TILE_BORDER);
        let mut letter = [0u8; 4];
        let letter = initial(self.name).encode_utf8(&mut letter);
        canvas.text(
            ICON.x + TILE_LETTER.0,
            ICON.y + TILE_LETTER.1,
            letter,
            token::TEXT,
            NAME_SCALE,
        );
        let badge = Rect::new(
            ICON.x + BADGE.x,
            ICON.y + BADGE.y,
            BADGE.width,
            BADGE.height,
        );
        canvas.rounded(badge, &INSET_R4, token::ICON_MAC);
        canvas.text(
            ICON.x + BADGE_LETTER.0,
            ICON.y + BADGE_LETTER.1,
            "M",
            token::ICON_DARK,
            1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(short: &str, build: &str) -> Option<String> {
        let mut out = [0u8; VERSION_BYTES];
        version_line(short, build, &mut out).map(str::to_owned)
    }

    #[test]
    fn the_version_line_follows_apple_s_rule() {
        assert_eq!(version("1.0.0", "1").as_deref(), Some("Version 1.0.0 (1)"));
        assert_eq!(version("0.3.0", "0.3.0").as_deref(), Some("Version 0.3.0"));
        assert_eq!(version("1.0", "").as_deref(), Some("Version 1.0"));
        assert_eq!(version("", "7").as_deref(), Some("Version 7"));
        assert_eq!(version("", ""), None);
    }

    #[test]
    fn the_copyright_is_its_first_line_and_the_tile_its_first_letter() {
        assert_eq!(
            first_line("\u{a9} 2026 EggOS\nAll rights"),
            "\u{a9} 2026 EggOS"
        );
        assert_eq!(first_line("one\rtwo"), "one");
        assert_eq!(initial("cocoaGame"), 'C');
        assert_eq!(initial("2048"), '2');
        assert_eq!(initial("\u{e9}t\u{e9}"), 'T');
        assert_eq!(initial("..."), '?');
    }

    #[test]
    fn the_height_counts_the_body_lines() {
        let none = Panel {
            name: "A",
            version: "",
            copyright: "",
        };
        assert_eq!(none.height(), 140);
        let copyright = Panel {
            copyright: "c",
            ..none
        };
        assert_eq!(copyright.height(), 160);
        assert_eq!(copyright.copyright_line().unwrap().rect.y, 128);
        let both = Panel {
            version: "Version 1",
            ..copyright
        };
        assert_eq!(both.height(), 180);
        assert_eq!(both.copyright_line().unwrap().rect.y, 148);
    }

    #[test]
    fn worked_example_a_cocoagame_is_laid_out_as_designed() {
        let panel = Panel {
            name: "CocoaGame",
            version: "Version 1.0.0 (1)",
            copyright: "",
        };
        assert_eq!(panel.height(), 160);
        assert_eq!(panel.name_line().rect, Rect::new(88, 80, 112, 32));
        assert_eq!(
            panel.version_line().unwrap().rect,
            Rect::new(103, 128, 81, 16)
        );
        assert_eq!(panel.copyright_line(), None);
    }

    #[test]
    fn worked_example_b_mac_paint_is_laid_out_as_designed() {
        let panel = Panel {
            name: "mac-paint",
            version: "Version 0.3.0",
            copyright: "\u{a9} 2026 EggOS",
        };
        assert_eq!(panel.height(), 180);
        assert_eq!(panel.name_line().rect, Rect::new(96, 80, 96, 32));
        assert_eq!(
            panel.version_line().unwrap().rect,
            Rect::new(111, 128, 65, 16)
        );
        assert_eq!(
            panel.copyright_line().unwrap().rect,
            Rect::new(108, 148, 71, 16)
        );
    }

    #[test]
    fn a_long_name_is_cut_with_an_ellipsis_within_the_space() {
        let panel = Panel {
            name: "AnApplicationWithAVeryLongName",
            version: "",
            copyright: "",
        };
        let name = panel.name_line();
        assert!(name.ellipsis);
        assert!(name.rect.width <= TEXT_SPACE);
        assert!(name.shown < panel.name.len());
    }

    #[test]
    fn the_icon_file_must_be_exact_and_its_channels_are_clamped_to_alpha() {
        let mut out = [0u32; ICON_PIXELS];
        assert!(!icon_pixels(&[0u8; ICON_FILE_BYTES - 1], &mut out));
        let mut file = vec![0u8; ICON_FILE_BYTES];
        file[..4].copy_from_slice(&0x80ff_2010u32.to_le_bytes());
        assert!(icon_pixels(&file, &mut out));
        assert_eq!(out[0], 0x8080_2010);
    }

    fn render(panel: &Panel<'_>, icon: Option<&[u32; ICON_PIXELS]>) -> Vec<u32> {
        let height = panel.height();
        let mut pixels = vec![0u32; (WIDTH * height) as usize];
        let mut canvas = Canvas::new(&mut pixels, WIDTH as u32, height as u32, 1).unwrap();
        panel.paint(&mut canvas, icon);
        pixels
    }

    fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
        pixels[(y * WIDTH + x) as usize]
    }

    #[test]
    fn example_a_draws_the_surface_and_the_generic_tile() {
        let panel = Panel {
            name: "CocoaGame",
            version: "Version 1.0.0 (1)",
            copyright: "",
        };
        let pixels = render(&panel, None);
        assert_eq!(at(&pixels, 0, 0), token::WINDOW_BACKGROUND);
        assert_eq!(at(&pixels, 4, 0), token::MENU_BORDER);
        assert_eq!(at(&pixels, 144, 70), token::MENU);
        assert_eq!(at(&pixels, 121, 40), token::ICON_TILE);
        assert_eq!(at(&pixels, 152, 44), token::ICON_MAC);
        let text = (80..112)
            .flat_map(|y| (88..200).map(move |x| (x, y)))
            .filter(|&(x, y)| at(&pixels, x, y) == token::TEXT)
            .count();
        assert!(text > 0);
    }

    #[test]
    fn an_own_icon_is_blended_over_the_surface() {
        let panel = Panel {
            name: "mac-paint",
            version: "Version 0.3.0",
            copyright: "\u{a9} 2026 EggOS",
        };
        let mut icon = [0u32; ICON_PIXELS];
        icon[0] = 0xff00_ff00;
        icon[1] = 0x0000_0000;
        let pixels = render(&panel, Some(&icon));
        assert_eq!(at(&pixels, 120, 16), 0x0000_ff00);
        assert_eq!(at(&pixels, 121, 16), token::MENU);
        assert_eq!(at(&pixels, 4, 179), token::MENU_BORDER);
    }
}

/// The terminal's background, `term.bg` (ANSI 0 as well).
pub const TERM_BACKGROUND: u32 = 0x000b_0e13;
/// The terminal's default foreground, `term.fg`.
pub const TERM_FOREGROUND: u32 = 0x00d7_dee8;
/// The block cursor, `term.cursor`.
pub const TERM_CURSOR: u32 = 0x00f2_b33d;
/// The design's `accent`: the scrollback mark.
pub const ACCENT: u32 = 0x00f2_b33d;
/// The fill of selected cells, `term.selection`.
pub const TERM_SELECTION: u32 = 0x002e_4a6b;
/// The ANSI colour drawn as [`TERM_FOREGROUND`] on [`TERM_SELECTION`]: bright black, too dim
/// on the fill.
pub const DIM_ON_SELECTION: u8 = 8;

/// The colour index that means the terminal's own foreground or background.
pub const DEFAULT_COLOUR: u8 = 16;

/// ANSI colours 0-7, then their bright variants 8-15.
pub const ANSI_COLOURS: [u32; 16] = [
    0x000b_0e13,
    0x00e0_565b,
    0x005f_b865,
    0x00e2_b341,
    0x004f_8fe0,
    0x00c4_6bd2,
    0x0044_b9c9,
    0x00c9_d1db,
    0x0070_7c8e,
    0x00f0_7a7e,
    0x0084_d48a,
    0x00f2_cc6d,
    0x007f_b0f0,
    0x00da_95e4,
    0x0072_d3e0,
    0x00f2_f5f9,
];

/// A terminal's colours in one palette (D610, docs/ux/self-drawn-themes-v0.1.md section 3):
/// `term.*`, the scrollback mark and the sixteen ANSI colours. A cell keeps its ANSI index, so
/// another scheme recolours the whole grid, scrollback included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scheme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    pub accent: u32,
    pub ansi: [u32; 16],
}

/// The dark scheme: the constants above.
pub const DARK_SCHEME: Scheme = Scheme {
    background: TERM_BACKGROUND,
    foreground: TERM_FOREGROUND,
    cursor: TERM_CURSOR,
    selection: TERM_SELECTION,
    accent: ACCENT,
    ansi: ANSI_COLOURS,
};

impl Scheme {
    /// The scheme of palette index `index` (`theme + 2 x high_contrast`).
    pub const fn of(index: u32) -> Self {
        let palette = eggos_palette::palette_at(index);
        let ansi = eggos_palette::ansi_at(index);
        let mut colours = [0u32; 16];
        let mut at = 0;
        while at < 8 {
            colours[at] = ansi.normal[at];
            colours[at + 8] = ansi.bright[at];
            at += 1;
        }
        Self {
            background: palette.term_background,
            foreground: palette.term_foreground,
            cursor: palette.term_cursor,
            selection: palette.term_selection,
            accent: palette.accent,
            ansi: colours,
        }
    }

    pub(super) fn pixel(&self, index: u8, default: u32) -> u32 {
        self.ansi.get(usize::from(index)).copied().unwrap_or(default)
    }
}

const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
const BRIGHT_LEVEL: u8 = 128;
const RED: u8 = 1;
const GREEN: u8 = 2;
const BLUE: u8 = 4;
const BRIGHT: u8 = 8;
const WHITE: u8 = 7;

fn from_levels(red: u8, green: u8, blue: u8) -> u8 {
    let channels = [(red, RED), (green, GREEN), (blue, BLUE)];
    let colour = channels
        .iter()
        .filter(|(level, _)| *level > 0)
        .fold(0, |colour, (_, bit)| colour | bit);
    let bright = channels.iter().any(|(level, _)| *level >= BRIGHT_LEVEL);
    if bright { colour | BRIGHT } else { colour }
}

pub(super) fn from_256(index: u16) -> u8 {
    match index {
        0..=15 => index as u8,
        16..=231 => {
            let cube = index - 16;
            let level = |step: u16| CUBE_LEVELS[usize::from(step % 6)];
            from_levels(level(cube / 36), level(cube / 6), level(cube))
        }
        232..=237 => 0,
        238..=243 => BRIGHT,
        244..=249 => WHITE,
        250..=255 => WHITE | BRIGHT,
        _ => DEFAULT_COLOUR,
    }
}

fn cube_level(channel: u16) -> u8 {
    let step = match channel.min(255) {
        0..48 => 0,
        48..115 => 1,
        value => (value - 35) / 40,
    };
    CUBE_LEVELS[usize::from(step.min(5))]
}

pub(super) fn from_rgb(red: u16, green: u16, blue: u16) -> u8 {
    from_levels(cube_level(red), cube_level(green), cube_level(blue))
}

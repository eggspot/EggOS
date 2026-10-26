/// The terminal's background, `term.bg` (ANSI 0 as well).
pub const TERM_BACKGROUND: u32 = 0x000b_0e13;
/// The terminal's default foreground, `term.fg`.
pub const TERM_FOREGROUND: u32 = 0x00d7_dee8;
/// The block cursor, `term.cursor`.
pub const TERM_CURSOR: u32 = 0x00f2_b33d;
/// The design's `accent`: the scrollback mark.
pub const ACCENT: u32 = 0x00f2_b33d;

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

pub(super) fn pixel(index: u8, default: u32) -> u32 {
    ANSI_COLOURS
        .get(usize::from(index))
        .copied()
        .unwrap_or(default)
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

/// The nearest of the 16 colours to colour `index` of the 256: itself below 16, the 6x6x6
/// cube's channels on when not 0 and bright when one reaches 128, the grey ramp in four bands
/// of six (black, bright black, white, bright white).
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

/// The nearest of the 16 colours to (`red`, `green`, `blue`): each channel taken to the cube's
/// nearest level, then as [`from_256`].
pub(super) fn from_rgb(red: u16, green: u16, blue: u16) -> u8 {
    from_levels(cube_level(red), cube_level(green), cube_level(blue))
}

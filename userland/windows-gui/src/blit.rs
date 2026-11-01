//! `BitBlt`'s arithmetic (D174): which part of a copy lands, clipped to the destination's clip
//! and the source's pixels, and the ternary raster operations that combine the source, the
//! destination and the brush, every one of the 256 by its truth table.

use crate::surface::Rect;

/// The raster operations programs name most.
pub mod operation {
    pub const SRCCOPY: u32 = 0x00cc_0020;
    pub const SRCPAINT: u32 = 0x00ee_0086;
    pub const SRCAND: u32 = 0x0088_00c6;
    pub const SRCINVERT: u32 = 0x0066_0046;
    pub const NOTSRCCOPY: u32 = 0x0033_0008;
    pub const DSTINVERT: u32 = 0x0055_0009;
    pub const PATCOPY: u32 = 0x00f0_0021;
    pub const PATINVERT: u32 = 0x005a_0049;
    pub const BLACKNESS: u32 = 0x0000_0042;
    pub const WHITENESS: u32 = 0x00ff_0062;
}

const PATTERN_BITS: u8 = 0xf0;
const SOURCE_BITS: u8 = 0xcc;
const COLOUR_BITS: u32 = 0x00ff_ffff;

fn ternary(operation: u32) -> u8 {
    (operation >> 16) as u8
}

/// Whether `operation` reads the source.
pub fn uses_source(operation: u32) -> bool {
    let code = ternary(operation);
    ((code >> 2) ^ code) & (SOURCE_BITS >> 2) != 0
}

/// Whether `operation` reads the brush.
pub fn uses_pattern(operation: u32) -> bool {
    let code = ternary(operation);
    ((code >> 4) ^ code) & (PATTERN_BITS >> 4) != 0
}

/// The pixel `operation` makes of `pattern`, `source` and `destination`: bit by bit, the bit
/// of the operation's code at position `pattern * 4 + source * 2 + destination`.
pub fn combine(operation: u32, pattern: u32, source: u32, destination: u32) -> u32 {
    let code = ternary(operation);
    match code {
        0xcc => return source & COLOUR_BITS,
        0xf0 => return pattern & COLOUR_BITS,
        0xaa => return destination & COLOUR_BITS,
        _ => {}
    }
    let mut result = 0u32;
    for index in 0..8u32 {
        if (code >> index) & 1 == 0 {
            continue;
        }
        let pick = |bit: u32, value: u32| match (index >> bit) & 1 {
            1 => value,
            _ => !value,
        };
        result |= pick(2, pattern) & pick(1, source) & pick(0, destination);
    }
    result & COLOUR_BITS
}

/// The part of a `width` by `height` copy to (`x`, `y`) from (`source_x`, `source_y`) that
/// lands: inside the destination's `clip` and taken from inside `source_bounds` (a copy that
/// reads no source passes `None`); the destination rectangle and the source point of its
/// top-left, or `None` when nothing lands.
pub fn landing(
    (x, y, width, height): (i32, i32, i32, i32),
    clip: Rect,
    source: Option<((i32, i32), Rect)>,
) -> Option<(Rect, (i32, i32))> {
    let wanted = Rect::sized(x, y, width.max(0), height.max(0));
    let mut landed = wanted.intersection(&clip);
    if let Some(((source_x, source_y), bounds)) = source {
        let shift = (source_x.saturating_sub(x), source_y.saturating_sub(y));
        let readable = bounds.offset(-shift.0, -shift.1);
        landed = landed.intersection(&readable);
        if landed.is_empty() {
            return None;
        }
        return Some((
            landed,
            (
                landed.left.saturating_add(shift.0),
                landed.top.saturating_add(shift.1),
            ),
        ));
    }
    (!landed.is_empty()).then_some((landed, (landed.left, landed.top)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_named_operations_follow_their_truth_tables() {
        let (p, s, d) = (0x00ff_0000, 0x00f0_f0f0, 0x0012_3456);
        assert_eq!(combine(operation::SRCCOPY, p, s, d), s);
        assert_eq!(combine(operation::PATCOPY, p, s, d), p);
        assert_eq!(combine(operation::SRCPAINT, p, s, d), s | d);
        assert_eq!(combine(operation::SRCAND, p, s, d), s & d);
        assert_eq!(combine(operation::SRCINVERT, p, s, d), s ^ d);
        assert_eq!(combine(operation::NOTSRCCOPY, p, s, d), !s & COLOUR_BITS);
        assert_eq!(combine(operation::DSTINVERT, p, s, d), !d & COLOUR_BITS);
        assert_eq!(combine(operation::PATINVERT, p, s, d), p ^ d);
        assert_eq!(combine(operation::BLACKNESS, p, s, d), 0);
        assert_eq!(combine(operation::WHITENESS, p, s, d), COLOUR_BITS);
        assert!(uses_source(operation::SRCCOPY) && !uses_source(operation::PATCOPY));
        assert!(!uses_source(operation::BLACKNESS) && !uses_source(operation::DSTINVERT));
        assert!(uses_pattern(operation::PATINVERT) && !uses_pattern(operation::SRCAND));
    }

    #[test]
    fn a_copy_lands_inside_the_clip_and_reads_only_the_source() {
        let clip = Rect::new(0, 0, 100, 100);
        let source = Rect::new(0, 0, 40, 30);
        assert_eq!(
            landing((10, 10, 40, 40), clip, Some(((0, 0), source))),
            Some((Rect::new(10, 10, 50, 40), (0, 0)))
        );
        assert_eq!(
            landing((-5, 90, 20, 20), clip, Some(((5, 5), source))),
            Some((Rect::new(0, 90, 15, 100), (10, 5)))
        );
        assert_eq!(landing((10, 10, 5, 5), clip, Some(((40, 0), source))), None);
        assert_eq!(
            landing((90, 90, 30, 30), clip, None),
            Some((Rect::new(90, 90, 100, 100), (90, 90)))
        );
        assert_eq!(landing((10, 10, -3, 5), clip, None), None);
    }
}

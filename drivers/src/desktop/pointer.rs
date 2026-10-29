//! The pointer's shapes (docs/ux/desktop-v0.3-gui.md section 2, `desktop-v0.3.md` for the
//! arrow), drawn in software over everything else: `X` black, `o` white, `.` transparent.

use super::protocol::PointerShape;
use crate::display::Rect;

const ARROW: [&[u8]; 19] = [
    b"X...........",
    b"XX..........",
    b"XoX.........",
    b"XooX........",
    b"XoooX.......",
    b"XooooX......",
    b"XoooooX.....",
    b"XooooooX....",
    b"XoooooooX...",
    b"XooooooooX..",
    b"XoooooooooX.",
    b"XooooooXXXXX",
    b"XoooXooX....",
    b"XooX.XooX...",
    b"XoX...XooX..",
    b"XX....XooX..",
    b"X......XooX.",
    b"........XX..",
    b"............",
];

const RESIZE_NORTH_SOUTH: [&[u8]; 19] = [
    b"....X....",
    b"...XoX...",
    b"..XoooX..",
    b".XoooooX.",
    b"XXXXoXXXX",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"XXXXoXXXX",
    b".XoooooX.",
    b"..XoooX..",
    b"...XoX...",
    b"....X....",
];

const RESIZE_NORTH_WEST: [&[u8]; 16] = [
    b"XXXXXXXX........",
    b"XoooooX.........",
    b"XooooX..........",
    b"XoooX...........",
    b"XooXoX..........",
    b"XoX.XoX.........",
    b"XX...XoX........",
    b"X.....XoX.......",
    b".......XoX.....X",
    b"........XoX...XX",
    b".........XoX.XoX",
    b"..........XoXooX",
    b"...........XoooX",
    b"..........XooooX",
    b".........XoooooX",
    b"........XXXXXXXX",
];

const I_BEAM: [&[u8]; 16] = [
    b"XXXXXXXXX",
    b"XoooXoooX",
    b"XXXXoXXXX",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"...XoX...",
    b"XXXXoXXXX",
    b"XoooXoooX",
    b"XXXXXXXXX",
];

const BLACK: u32 = 0x0000_0000;
const WHITE: u32 = 0x00ff_ffff;

#[derive(Clone, Copy)]
enum Transform {
    None,
    Transposed,
    Mirrored,
}

struct Bitmap {
    rows: &'static [&'static [u8]],
    transform: Transform,
}

fn bitmap(shape: PointerShape) -> Bitmap {
    let (rows, transform): (&'static [&'static [u8]], _) = match shape {
        PointerShape::Arrow => (&ARROW, Transform::None),
        PointerShape::IBeam => (&I_BEAM, Transform::None),
        PointerShape::ResizeNorthSouth => (&RESIZE_NORTH_SOUTH, Transform::None),
        PointerShape::ResizeEastWest => (&RESIZE_NORTH_SOUTH, Transform::Transposed),
        PointerShape::ResizeNorthWestSouthEast => (&RESIZE_NORTH_WEST, Transform::None),
        PointerShape::ResizeNorthEastSouthWest => (&RESIZE_NORTH_WEST, Transform::Mirrored),
    };
    Bitmap { rows, transform }
}

impl Bitmap {
    fn size(&self) -> (u32, u32) {
        let (width, height) = (self.rows[0].len() as u32, self.rows.len() as u32);
        match self.transform {
            Transform::Transposed => (height, width),
            Transform::None | Transform::Mirrored => (width, height),
        }
    }

    fn pixel(&self, column: u32, row: u32) -> Option<u32> {
        let (column, row) = (column as usize, row as usize);
        let cell = match self.transform {
            Transform::None => self.rows[row][column],
            Transform::Transposed => self.rows[column][row],
            Transform::Mirrored => {
                let line = self.rows[row];
                line[line.len() - 1 - column]
            }
        };
        match cell {
            b'X' => Some(BLACK),
            b'o' => Some(WHITE),
            _ => None,
        }
    }
}

/// The size of `shape`'s bitmap and its hotspot, the pixel at the pointer's position.
pub const fn size_and_hotspot(shape: PointerShape) -> ((u32, u32), (u32, u32)) {
    match shape {
        PointerShape::Arrow => ((12, 19), (0, 0)),
        PointerShape::IBeam => ((9, 16), (4, 8)),
        PointerShape::ResizeNorthSouth => ((9, 19), (4, 9)),
        PointerShape::ResizeEastWest => ((19, 9), (9, 4)),
        PointerShape::ResizeNorthWestSouthEast => ((16, 16), (7, 7)),
        PointerShape::ResizeNorthEastSouthWest => ((16, 16), (8, 7)),
    }
}

/// The rectangle `shape` covers with its hotspot at (`x`, `y`), cut at the screen's top and
/// left edges.
pub fn area(shape: PointerShape, x: u32, y: u32) -> Rect {
    let ((width, height), (hot_x, hot_y)) = size_and_hotspot(shape);
    let left = x.saturating_sub(hot_x);
    let top = y.saturating_sub(hot_y);
    Rect::new(
        left,
        top,
        (x + width - hot_x).saturating_sub(left),
        (y + height - hot_y).saturating_sub(top),
    )
}

/// The colour `shape` puts at (`x`, `y`) with its hotspot at `at`, or `None` where it is
/// transparent or does not reach.
pub fn pixel_at(shape: PointerShape, at: (u32, u32), x: u32, y: u32) -> Option<u32> {
    let bitmap = bitmap(shape);
    let (width, height) = bitmap.size();
    let (_, hotspot) = size_and_hotspot(shape);
    let column = i64::from(x) - i64::from(at.0) + i64::from(hotspot.0);
    let row = i64::from(y) - i64::from(at.1) + i64::from(hotspot.1);
    if column < 0 || row < 0 || column >= i64::from(width) || row >= i64::from(height) {
        return None;
    }
    bitmap.pixel(column as u32, row as u32)
}

/// As [`pixel_at`] for the arrow, in the agent's colours (docs/ux/desktop-v0.3-gui.md 14.1):
/// its white pixels in the accent, its outline black.
pub fn agent_arrow_pixel_at(at: (u32, u32), x: u32, y: u32) -> Option<u32> {
    pixel_at(PointerShape::Arrow, at, x, y).map(|pixel| match pixel {
        WHITE => super::colour::ACCENT,
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_s_arrow_is_the_arrow_in_the_accent_with_its_black_outline() {
        let at = (100, 50);
        assert_eq!(agent_arrow_pixel_at(at, 100, 50), Some(BLACK));
        assert_eq!(
            agent_arrow_pixel_at(at, 101, 52),
            Some(super::super::colour::ACCENT)
        );
        assert_eq!(pixel_at(PointerShape::Arrow, at, 101, 52), Some(WHITE));
        assert_eq!(agent_arrow_pixel_at(at, 111, 50), None);
    }

    #[test]
    fn each_shape_s_listed_size_is_its_bitmap_s() {
        for shape in [
            PointerShape::Arrow,
            PointerShape::IBeam,
            PointerShape::ResizeNorthSouth,
            PointerShape::ResizeEastWest,
            PointerShape::ResizeNorthWestSouthEast,
            PointerShape::ResizeNorthEastSouthWest,
        ] {
            let bitmap = bitmap(shape);
            assert_eq!(bitmap.size(), size_and_hotspot(shape).0, "{shape:?}");
            assert!(
                bitmap
                    .rows
                    .iter()
                    .all(|row| row.len() == bitmap.rows[0].len())
            );
        }
    }
}

//! What `hello-window` draws: a checkerboard of [`CELL`] px cells in two colours its title
//! picks, so each window of the test shows its own.

/// The side of one checkerboard cell.
pub const CELL: u32 = 16;

/// The colours of the window titled `first` (the design's `accent` and `term.bg`) and of any
/// other (`icon.windows` and `text`).
pub fn colours(title: &[u8]) -> [u32; 2] {
    match title {
        b"first" => [0x00f2_b33d, 0x000b_0e13],
        _ => [0x004f_8fe0, 0x00e6_eaf0],
    }
}

/// The pixel at (`x`, `y`) of the checkerboard in `colours`.
pub fn pixel(colours: [u32; 2], x: u32, y: u32) -> u32 {
    colours[((x / CELL + y / CELL) % 2) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_alternate_every_16_pixels_in_the_title_s_colours() {
        let first = colours(b"first");
        assert_eq!(pixel(first, 0, 0), 0x00f2_b33d);
        assert_eq!(pixel(first, 15, 15), 0x00f2_b33d);
        assert_eq!(pixel(first, 16, 0), 0x000b_0e13);
        assert_eq!(pixel(first, 16, 16), 0x00f2_b33d);
        assert_ne!(colours(b"second"), first);
    }
}

//! Colours: Win32's `COLORREF` (`0x00BBGGRR`) against the window's pixels (`0x00RRGGBB`), the
//! Win32 system colours (Windows' default light values, `docs/ux/windows-gui-v0.3.md` section 2)
//! and the design tokens the message box is painted with.

/// The pixel a `COLORREF` paints; its top byte (the palette flags) is ignored.
pub const fn pixel(colour_ref: u32) -> u32 {
    ((colour_ref & 0xff) << 16) | (colour_ref & 0xff00) | ((colour_ref >> 16) & 0xff)
}

/// The `COLORREF` of a pixel.
pub const fn colour_ref(pixel: u32) -> u32 {
    ((pixel & 0xff) << 16) | (pixel & 0xff00) | ((pixel >> 16) & 0xff)
}

/// The system colours `COLOR_SCROLLBAR` (0) to `COLOR_MENUBAR` (30), as pixels.
pub const SYSTEM_COLOURS: [u32; 31] = [
    0x00c8_c8c8,
    0x0000_0000,
    0x0099_b4d1,
    0x00bf_cddb,
    0x00f0_f0f0,
    0x00ff_ffff,
    0x0064_6464,
    0x0000_0000,
    0x0000_0000,
    0x0000_0000,
    0x00b4_b4b4,
    0x00f4_f7fc,
    0x00ab_abab,
    0x0000_78d7,
    0x00ff_ffff,
    0x00f0_f0f0,
    0x00a0_a0a0,
    0x006d_6d6d,
    0x0000_0000,
    0x0000_0000,
    0x00ff_ffff,
    0x0069_6969,
    0x00e3_e3e3,
    0x0000_0000,
    0x00ff_ffe1,
    0x0000_0000,
    0x0000_66cc,
    0x00b9_d1ea,
    0x00d7_e4f2,
    0x0000_78d7,
    0x00f0_f0f0,
];

/// The pixel of system colour `index`, if there is one.
pub fn system_colour(index: u32) -> Option<u32> {
    SYSTEM_COLOURS.get(index as usize).copied()
}

/// The design tokens the layer paints with (`docs/ux/desktop-v0.3.md` section 2,
/// `desktop-v0.3-gui.md` section 1), the same values as the desktop's.
pub mod token {
    pub const MENU: u32 = 0x001c_2430;
    pub const TEXT: u32 = 0x00e6_eaf0;
    pub const TEXT_DIM: u32 = 0x008a_94a6;
    pub const BUTTON: u32 = 0x002a_3440;
    pub const BUTTON_BORDER: u32 = 0x003a_4454;
    pub const BUTTON_PRIMARY: u32 = 0x00f2_b33d;
    pub const BUTTON_CLOSE: u32 = 0x00e5_534b;
    pub const BUTTON_MINIMISE: u32 = 0x00e8_b33c;
    pub const ICON_WINDOWS: u32 = 0x004f_8fe0;
    pub const ICON_DARK: u32 = 0x000b_0e13;
    pub const WINDOW_BACKGROUND: u32 = 0x0012_161d;
}

/// `pixel` a quarter darker, the desktop's pressed look.
pub const fn quarter(pixel: u32) -> u32 {
    pixel - ((pixel >> 2) & 0x003f_3f3f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_refs_swap_red_and_blue_and_drop_the_flags() {
        assert_eq!(pixel(0x0000_00ff), 0x00ff_0000);
        assert_eq!(pixel(0x0012_3456), 0x0056_3412);
        assert_eq!(pixel(0x0200_ff00), 0x0000_ff00);
        assert_eq!(colour_ref(pixel(0x00ab_cdef)), 0x00ab_cdef);
    }

    #[test]
    fn system_colours_are_windows_light_defaults() {
        assert_eq!(system_colour(5), Some(0x00ff_ffff));
        assert_eq!(system_colour(8), Some(0));
        assert_eq!(system_colour(15), Some(0x00f0_f0f0));
        assert_eq!(system_colour(13), Some(0x0000_78d7));
        assert_eq!(colour_ref(system_colour(13).unwrap()), 0x00d7_7800);
        assert_eq!(system_colour(31), None);
    }

    #[test]
    fn the_pressed_look_darkens_by_a_quarter() {
        assert_eq!(quarter(token::BUTTON), 0x0020_2730);
        assert_eq!(quarter(token::BUTTON_PRIMARY), 0x00b6_872e);
    }
}

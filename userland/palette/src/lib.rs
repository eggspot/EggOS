//! EggOS's colours (docs/ux/toolkit-v0.5.md section 8, docs/ux/self-drawn-themes-v0.1.md): every
//! token's value in the four palettes, dark, light and each with high contrast, at index
//! `theme + 2 x high_contrast`; the terminal's sixteen colours and the Text Editor's syntax
//! colours of each. The toolkit, the shell, the terminal and the apps all read these.

#![no_std]

/// The palette at index `index` (`theme + 2 x high_contrast`); the dark one for an index past
/// the four.
pub const fn palette_at(index: u32) -> &'static Palette {
    match index {
        1 => &LIGHT,
        2 => &DARK_HIGH_CONTRAST,
        3 => &LIGHT_HIGH_CONTRAST,
        _ => &DARK,
    }
}

/// One colour token of the design (docs/ux/toolkit-v0.5.md section 8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    WallpaperTop,
    WallpaperBottom,
    WallpaperEgg,
    Accent,
    Shell,
    Bar,
    BarEdge,
    BarHover,
    BarPressed,
    Menu,
    MenuBorder,
    MenuHover,
    Text,
    TextDim,
    TitleActive,
    TitleInactive,
    BorderActive,
    BorderInactive,
    ButtonClose,
    ButtonMinimise,
    ButtonMaximise,
    ButtonInactive,
    TermBackground,
    TermForeground,
    TermCursor,
    TermSelection,
    WindowBackground,
    Selection,
    FocusRing,
    Field,
    Button,
    ButtonBorder,
    ButtonPrimary,
    ButtonHover,
    ButtonPressed,
    ButtonPrimaryHover,
    ButtonPrimaryPressed,
    ProgressTrack,
    Dock,
    IconDark,
    IconTile,
    IconTileBorder,
    IconPrompt,
    IconFolder,
    IconFolderBack,
    IconGear,
    IconGearHole,
    IconWindows,
    IconBadge,
    IconMac,
    IconLinux,
    IconAndroid,
    IconPage,
    IconPageEdge,
    IconPageLine,
}

/// Every token's value in one theme, as framebuffer pixels `0x00RRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub wallpaper_top: u32,
    pub wallpaper_bottom: u32,
    pub wallpaper_egg: u32,
    pub accent: u32,
    pub shell: u32,
    pub bar: u32,
    pub bar_edge: u32,
    pub bar_hover: u32,
    pub bar_pressed: u32,
    pub menu: u32,
    pub menu_border: u32,
    pub menu_hover: u32,
    pub text: u32,
    pub text_dim: u32,
    pub title_active: u32,
    pub title_inactive: u32,
    pub border_active: u32,
    pub border_inactive: u32,
    pub button_close: u32,
    pub button_minimise: u32,
    pub button_maximise: u32,
    pub button_inactive: u32,
    pub term_background: u32,
    pub term_foreground: u32,
    pub term_cursor: u32,
    pub term_selection: u32,
    pub window_background: u32,
    pub selection: u32,
    pub focus_ring: u32,
    pub field: u32,
    pub button: u32,
    pub button_border: u32,
    pub button_primary: u32,
    pub button_hover: u32,
    pub button_pressed: u32,
    pub button_primary_hover: u32,
    pub button_primary_pressed: u32,
    pub progress_track: u32,
    pub dock: u32,
    pub icon_dark: u32,
    pub icon_tile: u32,
    pub icon_tile_border: u32,
    pub icon_prompt: u32,
    pub icon_folder: u32,
    pub icon_folder_back: u32,
    pub icon_gear: u32,
    pub icon_gear_hole: u32,
    pub icon_windows: u32,
    pub icon_badge: u32,
    pub icon_mac: u32,
    pub icon_linux: u32,
    pub icon_android: u32,
    pub icon_page: u32,
    pub icon_page_edge: u32,
    pub icon_page_line: u32,
}

impl Palette {
    /// The value of `token`.
    pub const fn get(&self, token: Token) -> u32 {
        match token {
            Token::WallpaperTop => self.wallpaper_top,
            Token::WallpaperBottom => self.wallpaper_bottom,
            Token::WallpaperEgg => self.wallpaper_egg,
            Token::Accent => self.accent,
            Token::Shell => self.shell,
            Token::Bar => self.bar,
            Token::BarEdge => self.bar_edge,
            Token::BarHover => self.bar_hover,
            Token::BarPressed => self.bar_pressed,
            Token::Menu => self.menu,
            Token::MenuBorder => self.menu_border,
            Token::MenuHover => self.menu_hover,
            Token::Text => self.text,
            Token::TextDim => self.text_dim,
            Token::TitleActive => self.title_active,
            Token::TitleInactive => self.title_inactive,
            Token::BorderActive => self.border_active,
            Token::BorderInactive => self.border_inactive,
            Token::ButtonClose => self.button_close,
            Token::ButtonMinimise => self.button_minimise,
            Token::ButtonMaximise => self.button_maximise,
            Token::ButtonInactive => self.button_inactive,
            Token::TermBackground => self.term_background,
            Token::TermForeground => self.term_foreground,
            Token::TermCursor => self.term_cursor,
            Token::TermSelection => self.term_selection,
            Token::WindowBackground => self.window_background,
            Token::Selection => self.selection,
            Token::FocusRing => self.focus_ring,
            Token::Field => self.field,
            Token::Button => self.button,
            Token::ButtonBorder => self.button_border,
            Token::ButtonPrimary => self.button_primary,
            Token::ButtonHover => self.button_hover,
            Token::ButtonPressed => self.button_pressed,
            Token::ButtonPrimaryHover => self.button_primary_hover,
            Token::ButtonPrimaryPressed => self.button_primary_pressed,
            Token::ProgressTrack => self.progress_track,
            Token::Dock => self.dock,
            Token::IconDark => self.icon_dark,
            Token::IconTile => self.icon_tile,
            Token::IconTileBorder => self.icon_tile_border,
            Token::IconPrompt => self.icon_prompt,
            Token::IconFolder => self.icon_folder,
            Token::IconFolderBack => self.icon_folder_back,
            Token::IconGear => self.icon_gear,
            Token::IconGearHole => self.icon_gear_hole,
            Token::IconWindows => self.icon_windows,
            Token::IconBadge => self.icon_badge,
            Token::IconMac => self.icon_mac,
            Token::IconLinux => self.icon_linux,
            Token::IconAndroid => self.icon_android,
            Token::IconPage => self.icon_page,
            Token::IconPageEdge => self.icon_page_edge,
            Token::IconPageLine => self.icon_page_line,
        }
    }
}

/// The dark theme.
pub const DARK: Palette = Palette {
    wallpaper_top: 0x000e_1726,
    wallpaper_bottom: 0x001b_3a5c,
    wallpaper_egg: 0x001f_3f63,
    accent: 0x00f2_b33d,
    shell: 0x00f6_f1e7,
    bar: 0x0014_1b26,
    bar_edge: 0x0020_2938,
    bar_hover: 0x0026_303f,
    bar_pressed: 0x0033_4052,
    menu: 0x001c_2430,
    menu_border: 0x002a_3340,
    menu_hover: 0x002a_3440,
    text: 0x00e6_eaf0,
    text_dim: 0x008a_94a6,
    title_active: 0x002a_3340,
    title_inactive: 0x001f_2630,
    border_active: 0x003a_4454,
    border_inactive: 0x002a_3340,
    button_close: 0x00e5_534b,
    button_minimise: 0x00e8_b33c,
    button_maximise: 0x003d_be5c,
    button_inactive: 0x004a_5466,
    term_background: 0x000b_0e13,
    term_foreground: 0x00d7_dee8,
    term_cursor: 0x00f2_b33d,
    term_selection: 0x002e_4a6b,
    window_background: 0x0012_161d,
    selection: 0x002e_4a6b,
    focus_ring: 0x00f2_b33d,
    field: 0x000b_0e13,
    button: 0x002a_3440,
    button_border: 0x003a_4454,
    button_primary: 0x00f2_b33d,
    button_hover: 0x0034_4050,
    button_pressed: 0x0020_2833,
    button_primary_hover: 0x00f6_c25c,
    button_primary_pressed: 0x00d9_9e2e,
    progress_track: 0x0026_303f,
    dock: 0x001c_2430,
    icon_dark: 0x000b_0e13,
    icon_tile: 0x0026_303f,
    icon_tile_border: 0x003a_4454,
    icon_prompt: 0x005f_b865,
    icon_folder: 0x00e2_b341,
    icon_folder_back: 0x00b8_8a24,
    icon_gear: 0x008a_94a6,
    icon_gear_hole: 0x001f_2630,
    icon_windows: 0x004f_8fe0,
    icon_badge: 0x00e6_eaf0,
    icon_mac: 0x00c9_d1db,
    icon_linux: 0x00e2_b341,
    icon_android: 0x003d_dc84,
    icon_page: 0x00f6_f1e7,
    icon_page_edge: 0x00c9_d1db,
    icon_page_line: 0x004a_5466,
};

/// The light theme: a cool white, the yolk kept for the primary button and a deep amber for
/// the marks and the ring.
pub const LIGHT: Palette = Palette {
    wallpaper_top: 0x00dc_e4ee,
    wallpaper_bottom: 0x00b9_cbe0,
    wallpaper_egg: 0x00c7_d6e6,
    accent: 0x009a_6200,
    shell: DARK.shell,
    bar: 0x00e9_edf2,
    bar_edge: 0x00c9_d0da,
    bar_hover: 0x00dc_e2ea,
    bar_pressed: 0x00d3_dae3,
    menu: 0x00f4_f6f9,
    menu_border: 0x00c9_d0da,
    menu_hover: 0x00e3_e8ef,
    text: 0x0016_1b22,
    text_dim: 0x005b_6675,
    title_active: 0x00dc_e2ea,
    title_inactive: 0x00ec_eff3,
    border_active: 0x009a_a5b4,
    border_inactive: 0x00c9_d0da,
    button_close: DARK.button_close,
    button_minimise: DARK.button_minimise,
    button_maximise: DARK.button_maximise,
    button_inactive: 0x00b0_b9c6,
    term_background: 0x00fb_fcfd,
    term_foreground: 0x001e_2630,
    term_cursor: 0x009a_6200,
    term_selection: 0x00bb_d3f0,
    window_background: 0x00ff_ffff,
    selection: 0x00cf_e0f5,
    focus_ring: 0x009a_6200,
    field: 0x00ff_ffff,
    button: 0x00ff_ffff,
    button_border: 0x00b8_c1cc,
    button_primary: DARK.button_primary,
    button_hover: 0x00f0_f3f7,
    button_pressed: 0x00dc_e2ea,
    button_primary_hover: DARK.button_primary_hover,
    button_primary_pressed: DARK.button_primary_pressed,
    progress_track: 0x00dc_e2ea,
    dock: 0x00f4_f6f9,
    icon_dark: DARK.icon_dark,
    icon_tile: 0x00dc_e2ea,
    icon_tile_border: 0x00b8_c1cc,
    icon_prompt: 0x002e_7d32,
    icon_folder: DARK.icon_folder,
    icon_folder_back: DARK.icon_folder_back,
    icon_gear: 0x005b_6675,
    icon_gear_hole: 0x00dc_e2ea,
    icon_windows: 0x002f_6fd0,
    icon_badge: 0x00ff_ffff,
    icon_mac: DARK.icon_mac,
    icon_linux: DARK.icon_linux,
    icon_android: 0x000f_6b3a,
    icon_page: 0x00ff_ffff,
    icon_page_edge: 0x009a_a5b4,
    icon_page_line: 0x005b_6675,
};

/// The dark theme lifted to its extremes (docs/ux/first-run-accessibility-v0.3.md section 2.4).
pub const DARK_HIGH_CONTRAST: Palette = Palette {
    wallpaper_top: 0x0000_0000,
    wallpaper_bottom: 0x0000_0000,
    wallpaper_egg: 0x0026_2626,
    accent: 0x00ff_d24d,
    shell: DARK.shell,
    bar: 0x0000_0000,
    bar_edge: 0x00ff_ffff,
    bar_hover: 0x0026_2626,
    bar_pressed: 0x0040_4040,
    menu: 0x0000_0000,
    menu_border: 0x00ff_ffff,
    menu_hover: 0x0026_2626,
    text: 0x00ff_ffff,
    text_dim: 0x00b8_c0cc,
    title_active: 0x0026_2626,
    title_inactive: 0x0000_0000,
    border_active: 0x00ff_ffff,
    border_inactive: 0x008a_94a6,
    button_close: DARK.button_close,
    button_minimise: DARK.button_minimise,
    button_maximise: DARK.button_maximise,
    button_inactive: 0x008a_94a6,
    term_background: 0x0000_0000,
    term_foreground: 0x00ff_ffff,
    term_cursor: 0x00ff_d24d,
    term_selection: 0x0015_3a6b,
    window_background: 0x0000_0000,
    selection: 0x0015_3a6b,
    focus_ring: 0x00ff_d24d,
    field: 0x0000_0000,
    button: 0x001f_1f1f,
    button_border: 0x00ff_ffff,
    button_primary: 0x00ff_d24d,
    button_hover: 0x0033_3333,
    button_pressed: 0x004d_4d4d,
    button_primary_hover: 0x00ff_e080,
    button_primary_pressed: 0x00e6_b800,
    progress_track: 0x004d_4d4d,
    dock: 0x0000_0000,
    icon_dark: 0x0000_0000,
    icon_tile: 0x0000_0000,
    icon_tile_border: 0x00ff_ffff,
    icon_prompt: 0x0084_d48a,
    icon_folder: 0x00ff_d24d,
    icon_folder_back: 0x00b3_8600,
    icon_gear: 0x00ff_ffff,
    icon_gear_hole: 0x0000_0000,
    icon_windows: 0x000a_3d8f,
    icon_badge: 0x00ff_ffff,
    icon_mac: 0x00ff_ffff,
    icon_linux: 0x00ff_d24d,
    icon_android: 0x003d_dc84,
    icon_page: 0x00ff_ffff,
    icon_page_edge: 0x00b8_c0cc,
    icon_page_line: 0x008a_94a6,
};

/// The light theme at its extremes: black on white, every edge black.
pub const LIGHT_HIGH_CONTRAST: Palette = Palette {
    wallpaper_top: 0x00ff_ffff,
    wallpaper_bottom: 0x00ff_ffff,
    wallpaper_egg: 0x00e6_e6e6,
    accent: 0x007a_4e00,
    shell: DARK.shell,
    bar: 0x00ff_ffff,
    bar_edge: 0x0000_0000,
    bar_hover: 0x00e6_e6e6,
    bar_pressed: 0x00d9_d9d9,
    menu: 0x00ff_ffff,
    menu_border: 0x0000_0000,
    menu_hover: 0x00e6_e6e6,
    text: 0x0000_0000,
    text_dim: 0x003d_4650,
    title_active: 0x00e6_e6e6,
    title_inactive: 0x00ff_ffff,
    border_active: 0x0000_0000,
    border_inactive: 0x006b_6b6b,
    button_close: DARK.button_close,
    button_minimise: DARK.button_minimise,
    button_maximise: DARK.button_maximise,
    button_inactive: 0x006b_6b6b,
    term_background: 0x00ff_ffff,
    term_foreground: 0x0000_0000,
    term_cursor: 0x007a_4e00,
    term_selection: 0x00cc_e0ff,
    window_background: 0x00ff_ffff,
    selection: 0x00cc_e0ff,
    focus_ring: 0x007a_4e00,
    field: 0x00ff_ffff,
    button: 0x00ff_ffff,
    button_border: 0x0000_0000,
    button_primary: DARK.button_primary,
    button_hover: 0x00e6_e6e6,
    button_pressed: 0x00d9_d9d9,
    button_primary_hover: DARK.button_primary_hover,
    button_primary_pressed: DARK.button_primary_pressed,
    progress_track: 0x00d9_d9d9,
    dock: 0x00ff_ffff,
    icon_dark: 0x0000_0000,
    icon_tile: 0x00e6_e6e6,
    icon_tile_border: 0x0000_0000,
    icon_prompt: 0x001b_5e20,
    icon_folder: DARK.icon_folder,
    icon_folder_back: DARK.icon_folder_back,
    icon_gear: 0x0000_0000,
    icon_gear_hole: 0x00e6_e6e6,
    icon_windows: 0x001f_4fa0,
    icon_badge: 0x00ff_ffff,
    icon_mac: DARK.icon_mac,
    icon_linux: DARK.icon_linux,
    icon_android: 0x000f_6b3a,
    icon_page: 0x00ff_ffff,
    icon_page_edge: 0x0000_0000,
    icon_page_line: 0x0000_0000,
};

/// A terminal's sixteen colours (SGR 30 to 37 and 90 to 97; bold selects the bright row), in
/// one palette (docs/ux/self-drawn-themes-v0.1.md section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ansi {
    pub normal: [u32; 8],
    pub bright: [u32; 8],
}

impl Ansi {
    /// Colour `index`, 0 to 15: 0 to 7 the normal row, 8 to 15 the bright one.
    pub const fn get(&self, index: u8) -> u32 {
        match index {
            0..=7 => self.normal[index as usize],
            _ => self.bright[(index & 7) as usize],
        }
    }
}

/// The dark terminal: ANSI 0 is the terminal's own background.
pub const DARK_ANSI: Ansi = Ansi {
    normal: [
        0x000b_0e13, 0x00e0_565b, 0x005f_b865, 0x00e2_b341, 0x004f_8fe0, 0x00c4_6bd2, 0x0044_b9c9,
        0x00c9_d1db,
    ],
    bright: [
        0x0070_7c8e, 0x00f0_7a7e, 0x0084_d48a, 0x00f2_cc6d, 0x007f_b0f0, 0x00da_95e4, 0x0072_d3e0,
        0x00f2_f5f9,
    ],
};

/// The dark high-contrast terminal.
pub const DARK_HIGH_CONTRAST_ANSI: Ansi = Ansi {
    normal: [
        0x0000_0000, 0x00ff_6e6e, 0x0084_d48a, 0x00f2_cc6d, 0x007f_b0f0, 0x00da_95e4, 0x0072_d3e0,
        0x00f2_f5f9,
    ],
    bright: [
        0x00b8_c0cc, 0x00ff_9a9a, 0x00a8_f0ad, 0x00ff_e08a, 0x00a8_ccff, 0x00ea_b8f0, 0x00a0_e8f0,
        0x00ff_ffff,
    ],
};

/// The light terminal: black is black, and bright white is the faintest colour.
pub const LIGHT_ANSI: Ansi = Ansi {
    normal: [
        0x001e_2630, 0x00b3_261e, 0x002e_7d32, 0x008a_5a00, 0x001f_5fbf, 0x008e_2ea0, 0x000b_7285,
        0x0065_707f,
    ],
    bright: [
        0x004a_5566, 0x00c7_342b, 0x002f_8f3a, 0x009a_6200, 0x002f_6fd0, 0x009b_3dae, 0x0015_7f95,
        0x008a_94a6,
    ],
};

/// The light high-contrast terminal: a darkened pair of rows.
pub const LIGHT_HIGH_CONTRAST_ANSI: Ansi = Ansi {
    normal: [
        0x0000_0000, 0x008b_1a14, 0x001b_5e20, 0x005c_3c00, 0x0015_3f8a, 0x005e_1c6b, 0x0006_4c5a,
        0x003d_4650,
    ],
    bright: [
        0x001e_2630, 0x009e_1f18, 0x001c_6021, 0x006e_4700, 0x001a_4fa3, 0x0073_258a, 0x0008_5e6e,
        0x005b_6675,
    ],
};

/// The terminal colours of palette index `index`; the dark ones past the four.
pub const fn ansi_at(index: u32) -> &'static Ansi {
    match index {
        1 => &LIGHT_ANSI,
        2 => &DARK_HIGH_CONTRAST_ANSI,
        3 => &LIGHT_HIGH_CONTRAST_ANSI,
        _ => &DARK_ANSI,
    }
}

/// The Text Editor's syntax colours in one palette (docs/ux/self-drawn-themes-v0.1.md section
/// 4): the dark columns take the terminal's bright row, the light ones its normal row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Syntax {
    pub plain: u32,
    pub function: u32,
    pub type_name: u32,
    pub string: u32,
    pub number: u32,
    pub keyword: u32,
    pub comment: u32,
}

/// The syntax colours of palette index `index`; the dark ones past the four.
pub const fn syntax_at(index: u32) -> Syntax {
    let palette = palette_at(index);
    let ansi = ansi_at(index);
    let row = match index % 2 {
        1 => ansi.normal,
        _ => ansi.bright,
    };
    Syntax {
        plain: palette.text,
        function: row[3],
        type_name: row[6],
        string: row[2],
        number: row[5],
        keyword: row[4],
        comment: palette.text_dim,
    }
}

/// The index of `palette` among the four, 0 for one that is none of them.
pub fn index_of(palette: &Palette) -> u32 {
    (0..4).find(|&index| palette_at(index) == palette).unwrap_or(0)
}

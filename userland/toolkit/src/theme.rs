//! The two themes and their palettes (docs/ux/toolkit-v0.5.md section 8): every colour token
//! has a dark, a light and a high-contrast value of each, and the toolkit draws through the
//! palette it was given, so a window repaints in the other theme without being rebuilt.

/// Dark or light: what `appearance.theme` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Both themes, each at the index that is its number.
    pub const ALL: [Self; 2] = [Self::Dark, Self::Light];

    /// Its name in the settings file and the `settings` command.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// The theme named `name`.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.name() == name)
    }

    /// Its number: 0 dark, 1 light.
    pub const fn number(self) -> u32 {
        self as u32
    }

    /// The theme numbered `number`.
    pub fn from_number(number: u32) -> Option<Self> {
        Self::ALL.get(number as usize).copied()
    }
}

/// The palette index `/dev/window`'s `GET_PALETTE` and `SET_PALETTE` carry for `theme` with
/// or without high contrast: `theme + 2 * high_contrast`.
pub const fn palette_index(theme: Theme, high_contrast: bool) -> u32 {
    theme.number() + 2 * high_contrast as u32
}

/// The theme of palette index `index`.
pub const fn theme_of(index: u32) -> Theme {
    match index % 2 {
        1 => Theme::Light,
        _ => Theme::Dark,
    }
}

pub use eggos_palette::{
    DARK, DARK_HIGH_CONTRAST, LIGHT, LIGHT_HIGH_CONTRAST, Palette, Syntax, Token, index_of,
    palette_at, syntax_at,
};

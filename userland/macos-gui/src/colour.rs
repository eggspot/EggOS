//! `NSColor` as pixels (`docs/ux/macos-gui-v0.3.md` section 1): the fixed colours at their exact
//! values, the semantic ones as the desktop's dark tokens, alpha ignored except that a colour
//! with none (`clearColor`) draws nothing.

use eggos_toolkit::colour as token;

/// A colour as the layer draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// `0x00RRGGBB`.
    pub pixel: u32,
    /// Whether drawing with it changes anything.
    pub visible: bool,
}

impl Colour {
    pub const fn opaque(pixel: u32) -> Self {
        Self {
            pixel,
            visible: true,
        }
    }

    pub const CLEAR: Self = Self {
        pixel: 0,
        visible: false,
    };
}

fn component(value: f64) -> u32 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * 255.0 + 0.5) as u32
}

/// The colour of `red`, `green` and `blue` from 0 to 1, as `colorWithRed:green:blue:alpha:`
/// makes it.
pub fn rgb(red: f64, green: f64, blue: f64, alpha: f64) -> Colour {
    Colour {
        pixel: (component(red) << 16) | (component(green) << 8) | component(blue),
        visible: alpha > 0.0,
    }
}

/// The grey `white` from 0 to 1, as `colorWithWhite:alpha:` makes it.
pub fn grey(white: f64, alpha: f64) -> Colour {
    rgb(white, white, white, alpha)
}

/// The text colour AppKit draws with when none is given: `labelColor`.
pub const DEFAULT_TEXT: Colour = Colour::opaque(token::TEXT);
/// A window's background: `windowBackgroundColor`.
pub const WINDOW_BACKGROUND: Colour = Colour::opaque(token::WINDOW_BACKGROUND);

const SYSTEM_RED: u32 = 0x00e0_565b;
const SYSTEM_GREEN: u32 = 0x005f_b865;
const SYSTEM_YELLOW: u32 = 0x00e2_b341;
const SYSTEM_PURPLE: u32 = 0x00c4_6bd2;
const SYSTEM_TEAL: u32 = 0x0044_b9c9;

/// The colour `NSColor`'s class method `name` (its selector) returns, if it is one.
pub fn named(name: &[u8]) -> Option<Colour> {
    let third = 1.0 / 3.0;
    let fixed = match name {
        b"blackColor" => grey(0.0, 1.0),
        b"whiteColor" => grey(1.0, 1.0),
        b"grayColor" => grey(0.5, 1.0),
        b"darkGrayColor" => grey(third, 1.0),
        b"lightGrayColor" => grey(2.0 * third, 1.0),
        b"redColor" => rgb(1.0, 0.0, 0.0, 1.0),
        b"greenColor" => rgb(0.0, 1.0, 0.0, 1.0),
        b"blueColor" => rgb(0.0, 0.0, 1.0, 1.0),
        b"cyanColor" => rgb(0.0, 1.0, 1.0, 1.0),
        b"yellowColor" => rgb(1.0, 1.0, 0.0, 1.0),
        b"magentaColor" => rgb(1.0, 0.0, 1.0, 1.0),
        b"orangeColor" => rgb(1.0, 0.5, 0.0, 1.0),
        b"purpleColor" => rgb(0.5, 0.0, 0.5, 1.0),
        b"brownColor" => rgb(0.6, 0.4, 0.2, 1.0),
        b"clearColor" => Colour::CLEAR,
        _ => return semantic(name).map(Colour::opaque),
    };
    Some(fixed)
}

fn semantic(name: &[u8]) -> Option<u32> {
    Some(match name {
        b"windowBackgroundColor" => token::WINDOW_BACKGROUND,
        b"controlBackgroundColor" | b"textBackgroundColor" => token::FIELD,
        b"labelColor"
        | b"textColor"
        | b"controlTextColor"
        | b"headerTextColor"
        | b"selectedTextColor"
        | b"selectedMenuItemTextColor"
        | b"alternateSelectedControlTextColor" => token::TEXT,
        b"secondaryLabelColor"
        | b"tertiaryLabelColor"
        | b"quaternaryLabelColor"
        | b"placeholderTextColor"
        | b"disabledControlTextColor"
        | b"systemGrayColor" => token::TEXT_DIM,
        b"controlColor" => token::BUTTON,
        b"selectedControlColor"
        | b"selectedContentBackgroundColor"
        | b"selectedTextBackgroundColor" => token::SELECTION,
        b"controlAccentColor" | b"keyboardFocusIndicatorColor" | b"systemOrangeColor" => {
            token::ACCENT
        }
        b"separatorColor" | b"gridColor" => token::MENU_BORDER,
        b"linkColor" | b"systemBlueColor" => token::ICON_WINDOWS,
        b"systemRedColor" => SYSTEM_RED,
        b"systemGreenColor" => SYSTEM_GREEN,
        b"systemYellowColor" => SYSTEM_YELLOW,
        b"systemPurpleColor" => SYSTEM_PURPLE,
        b"systemTealColor" => SYSTEM_TEAL,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_round_to_the_nearest_byte() {
        assert_eq!(rgb(1.0, 0.5, 0.0, 1.0).pixel, 0x00ff_8000);
        assert_eq!(grey(1.0 / 3.0, 1.0).pixel, 0x0055_5555);
        assert_eq!(rgb(2.0, -1.0, f64::NAN, 1.0).pixel, 0x00ff_0000);
        assert!(!rgb(1.0, 1.0, 1.0, 0.0).visible);
    }

    #[test]
    fn semantic_colours_are_the_dark_tokens() {
        assert_eq!(named(b"labelColor"), Some(Colour::opaque(token::TEXT)));
        assert_eq!(
            named(b"textBackgroundColor"),
            Some(Colour::opaque(token::FIELD))
        );
        assert_eq!(
            named(b"systemBlueColor").map(|colour| colour.pixel),
            Some(0x004f_8fe0)
        );
        assert_eq!(named(b"clearColor"), Some(Colour::CLEAR));
        assert_eq!(named(b"redColor").map(|colour| colour.pixel), Some(0x00ff_0000));
        assert_eq!(named(b"colorWithRed:green:blue:alpha:"), None);
    }
}

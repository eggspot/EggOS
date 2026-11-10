//! A tooltip: a tile under a widget naming it and its shortcut (docs/ux/toolkit-v0.5.md
//! section 2), shown after the pointer or the keyboard focus rests on its owner.

use crate::canvas::{Canvas, INSET_R4};
use crate::geometry::Rect;
use crate::text::{self, Text};
use crate::widget::{
    TOOLTIP_GAP, TOOLTIP_HEIGHT, TOOLTIP_PADDING, TOOLTIP_SHORTCUT_GAP, TOOLTIP_TEXT_TOP,
};

/// What a tooltip says: its owner's name and, if any, its shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tooltip {
    pub text: Text<48>,
    /// Empty for none.
    pub shortcut: Text<16>,
}

impl Tooltip {
    /// Its tile's width: the padding, the text, and the shortcut after its gap.
    pub fn width(&self) -> i32 {
        let shortcut = match self.shortcut.is_empty() {
            true => 0,
            false => TOOLTIP_SHORTCUT_GAP + text::width(self.shortcut.as_str()),
        };
        2 * TOOLTIP_PADDING + text::width(self.text.as_str()) + shortcut
    }
}

/// Where the tooltip of an owner at `owner` (its visible part) shows in a window `window` big:
/// below the owner and centred on it, clamped 8 px inside the window, or above when below does
/// not fit.
pub fn tooltip_rect(tooltip: &Tooltip, owner: Rect, window: (i32, i32)) -> Rect {
    let width = tooltip.width();
    let centred = owner.x + (owner.width - width) / 2;
    let x = match window.0 < width + 2 * TOOLTIP_PADDING {
        true => 0,
        false => centred.clamp(TOOLTIP_PADDING, window.0 - width - TOOLTIP_PADDING),
    };
    let below = owner.bottom() + TOOLTIP_GAP;
    let y = match below + TOOLTIP_HEIGHT > window.1 {
        true => owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT,
        false => below,
    };
    Rect::new(x, y, width, TOOLTIP_HEIGHT)
}

/// Draws a tile of `words` at `rect`, with `shortcut` after them in `text.dim`: a tooltip, or
/// a drag's ghost with no shortcut.
pub fn paint_tile(canvas: &mut Canvas<'_>, rect: Rect, words: &str, shortcut: &str) {
    let palette = canvas.palette();
    canvas.rounded(rect, &INSET_R4, palette.menu);
    canvas.outline(rect, &INSET_R4, 1, palette.menu_border);
    let y = rect.y + TOOLTIP_TEXT_TOP;
    let after = canvas.text(rect.x + TOOLTIP_PADDING, y, words, palette.text, 1);
    if !shortcut.is_empty() {
        canvas.text(
            after + TOOLTIP_SHORTCUT_GAP,
            y,
            shortcut,
            palette.text_dim,
            1,
        );
    }
}

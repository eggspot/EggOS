//! The bar's `Updates` chip (docs/ux/components-v0.5.md section 7, D300): drawn while the
//! last stored update check found at least one component update, left of the `Agent` chip
//! (or where it would be); a click opens Settings on Components. It updates nothing.

use super::paint::{self, Canvas};
use super::{CHIP_HEIGHT, CHIP_PADDING, CHIP_Y, Desktop, colour};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;
use alloc::format;
use alloc::string::String;

/// The chip's label.
pub const UPDATES_LABEL: &str = "Updates";

const UPDATES_CHIP_GAP: u32 = 8;
const UPDATES_CHIP_FROM_POWER: u32 = 8;
const UPDATES_CHIP_BEFORE_CLOCK: u32 = 16;
const UPDATES_LABEL_Y: u32 = 2;
const KIBIBYTE: u64 = 1024;

/// What the desktop knows of the stored check: how many updates and their bytes, and the
/// tooltip that says so.
#[derive(Debug, Clone, Default)]
pub(super) struct UpdatesState {
    count: usize,
    bytes: u64,
    tooltip: String,
}

fn size_text(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10) / (KIBIBYTE * KIBIBYTE);
    match bytes < KIBIBYTE * KIBIBYTE {
        true => format!("{} KB", bytes.div_ceil(KIBIBYTE)),
        false => format!("{}.{} MB", tenths / 10, tenths % 10),
    }
}

impl Desktop {
    /// How many component updates the last stored check found, and their bytes (`0` hides the
    /// chip).
    pub fn set_updates(&mut self, count: usize, bytes: u64, damage: &mut Damage) {
        if count == self.updates.count && bytes == self.updates.bytes {
            return;
        }
        if let Some(chip) = self.updates_chip() {
            damage.add(chip);
        }
        let noun = if count == 1 { "update" } else { "updates" };
        self.updates = UpdatesState {
            count,
            bytes,
            tooltip: format!(
                "{count} component {noun}, {}. Click to open Components.",
                size_text(bytes)
            ),
        };
        for panel in self.layout.panels() {
            damage.add(panel);
        }
        self.refresh_panel_hover(damage);
    }

    /// How many updates the chip shows, as an agent reads its value.
    pub fn updates_count(&self) -> usize {
        self.updates.count
    }

    pub(super) fn updates_tooltip(&self) -> &str {
        &self.updates.tooltip
    }

    /// The chip's rectangle while it shows: 8 left of the `Agent` chip, or where that chip
    /// would stand when no agent is connected.
    pub fn updates_chip(&self) -> Option<Rect> {
        if self.updates.count == 0 {
            return None;
        }
        let width = UPDATES_LABEL.len() as u32 * GLYPH_WIDTH as u32 + 2 * CHIP_PADDING;
        if let Some(agent) = self.agent_chip() {
            return Some(Rect::new(
                agent.x.saturating_sub(UPDATES_CHIP_GAP + width),
                agent.y,
                width,
                CHIP_HEIGHT,
            ));
        }
        let indicator = self
            .tray_run()
            .or(self.keyboard_layout_indicator())
            .or(self.network_indicator());
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                match indicator {
                    Some(indicator) => indicator.x.saturating_sub(UPDATES_CHIP_FROM_POWER + width),
                    None => self
                        .clock_rect()
                        .x
                        .saturating_sub(UPDATES_CHIP_BEFORE_CLOCK + width),
                },
                bar.y + (bar.height - CHIP_HEIGHT) / 2,
                width,
                CHIP_HEIGHT,
            )),
            (None, Some(_)) => Some(Rect::new(
                indicator
                    .unwrap_or(self.power_button())
                    .x
                    .saturating_sub(UPDATES_CHIP_FROM_POWER + width),
                CHIP_Y,
                width,
                CHIP_HEIGHT,
            )),
            (None, None) => None,
        }
    }

    pub(super) fn draw_updates_chip(&self, canvas: &mut Canvas<'_>, hovered: bool) {
        let Some(chip) = self.updates_chip() else {
            return;
        };
        let fill = match hovered {
            true => colour::BAR_PRESSED,
            false => colour::BAR_HOVER,
        };
        canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
        canvas.text(
            chip.x + CHIP_PADDING,
            chip.y + UPDATES_LABEL_Y,
            UPDATES_LABEL,
            colour::TEXT_DIM,
        );
    }
}

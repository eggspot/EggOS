//! What the desktop shows of an AI agent (docs/ux/desktop-v0.3-gui.md section 14.1, D156):
//! the `Agent` chip on the panel while an MCP client is connected, filled with the accent for
//! [`AGENT_ACTING_MS`] after each capture, input step or menu choice it makes; the
//! ` (agent)` suffix on the title of the window its input went to; and a second, accent arrow
//! where its pointer input went, which never moves the person's pointer.

use super::injection::InjectedInput;
use super::paint::{self, Canvas};
use super::protocol::PointerShape;
use super::windows::WindowId;
use super::{CHIP_HEIGHT, CHIP_PADDING, CHIP_Y, Desktop, colour, pointer};
use crate::display::{Damage, Rect};
use crate::font::GLYPH_WIDTH;

/// How long the chip, the title and the arrow say the agent is acting after its last action.
pub const AGENT_ACTING_MS: u32 = 1000;
/// The chip's label.
pub const AGENT_LABEL: &str = "Agent";
/// The chip's tooltip.
pub const AGENT_TOOLTIP: &str = "An AI agent is connected. Log: agentlog";
/// What a driven window's title says after its title, in the accent.
pub const AGENT_SUFFIX: &str = " (agent)";
/// What a click on the chip types in the terminal: Ctrl-U, the command and Enter.
pub const AGENT_LOG_LINE: &[u8] = b"\x15agentlog\r";

const AGENT_CHIP_FROM_POWER: u32 = 8;
const AGENT_CHIP_BEFORE_CLOCK: u32 = 16;
const AGENT_LABEL_Y: u32 = 2;

/// The desktop's view of the agent: whether one is connected, and when and where it last
/// acted.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AgentState {
    connected: bool,
    acted_ms: Option<u32>,
    driven: Option<(WindowId, u32)>,
    pointer: Option<(u32, u32, u32)>,
}

impl Desktop {
    /// Whether an MCP client is connected, so the chip shows.
    pub fn agent_connected(&self) -> bool {
        self.agent.connected
    }

    /// Shows the chip while an MCP client is connected, or hides it.
    pub fn set_agent_connected(&mut self, connected: bool, damage: &mut Damage) {
        if connected == self.agent.connected {
            return;
        }
        self.damage_panels(damage);
        self.agent.connected = connected;
        if !connected {
            self.agent.acted_ms = None;
        }
        self.damage_panels(damage);
        self.refresh_panel_hover(damage);
    }

    /// Whether the agent acted in the last [`AGENT_ACTING_MS`].
    pub fn agent_acting(&self) -> bool {
        self.agent
            .acted_ms
            .is_some_and(|at| self.now_ms.wrapping_sub(at) < AGENT_ACTING_MS)
    }

    /// The chip's rectangle while it shows: left of the top bar's power button, or of the
    /// taskbar's clock.
    pub fn agent_chip(&self) -> Option<Rect> {
        if !self.agent.connected {
            return None;
        }
        let width = AGENT_LABEL.len() as u32 * GLYPH_WIDTH as u32 + 2 * CHIP_PADDING;
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                self.clock_rect()
                    .x
                    .saturating_sub(AGENT_CHIP_BEFORE_CLOCK + width),
                bar.y + (bar.height - CHIP_HEIGHT) / 2,
                width,
                CHIP_HEIGHT,
            )),
            (None, Some(_)) => Some(Rect::new(
                self.power_button()
                    .x
                    .saturating_sub(AGENT_CHIP_FROM_POWER + width),
                CHIP_Y,
                width,
                CHIP_HEIGHT,
            )),
            (None, None) => None,
        }
    }

    /// The agent captured a window or chose a menu item: the chip shows it acting.
    pub fn note_agent_action(&mut self, damage: &mut Damage) {
        self.agent.acted_ms = Some(self.now_ms);
        if let Some(chip) = self.agent_chip() {
            damage.add(chip);
        }
    }

    /// The agent's `input` reached window `window`: the chip shows it acting, the window's
    /// title says ` (agent)`, and pointer input draws the agent's arrow at its point.
    pub fn note_agent_input(&mut self, window: u32, input: InjectedInput, damage: &mut Damage) {
        self.note_agent_action(damage);
        let Ok(slot) = self.any_window(window) else {
            return;
        };
        self.damage_driven(damage);
        let target = *self.window_at(slot);
        self.agent.driven = Some((target.id, self.now_ms));
        self.damage_driven(damage);
        let point = match input {
            InjectedInput::PointerMove { x, y }
            | InjectedInput::Click { x, y, .. }
            | InjectedInput::Wheel { x, y, .. } => Some((x, y)),
            InjectedInput::Key { .. } | InjectedInput::Character(_) => None,
        };
        if let Some((x, y)) = point
            && self.is_visible(slot)
        {
            self.damage_agent_pointer(damage);
            let content = target.content();
            self.agent.pointer = Some((content.x + x, content.y + y, self.now_ms));
            self.damage_agent_pointer(damage);
        }
    }

    /// Whether window `slot`'s title says ` (agent)`: the agent's input went to it in the
    /// last [`AGENT_ACTING_MS`] and it is answering.
    pub(super) fn agent_drives(&self, slot: usize) -> bool {
        let window = self.window_at(slot);
        !window.not_responding
            && self.agent.driven.is_some_and(|(id, at)| {
                id == window.id && self.now_ms.wrapping_sub(at) < AGENT_ACTING_MS
            })
    }

    /// The clock moved on: what the agent did more than [`AGENT_ACTING_MS`] ago stops showing.
    pub(super) fn tick_agent(&mut self, damage: &mut Damage) {
        let expired = |at: u32, now: u32| now.wrapping_sub(at) >= AGENT_ACTING_MS;
        if self
            .agent
            .acted_ms
            .is_some_and(|at| expired(at, self.now_ms))
        {
            self.agent.acted_ms = None;
            if let Some(chip) = self.agent_chip() {
                damage.add(chip);
            }
        }
        if self
            .agent
            .driven
            .is_some_and(|(_, at)| expired(at, self.now_ms))
        {
            self.damage_driven(damage);
            self.agent.driven = None;
        }
        if self
            .agent
            .pointer
            .is_some_and(|(_, _, at)| expired(at, self.now_ms))
        {
            self.damage_agent_pointer(damage);
            self.agent.pointer = None;
        }
    }

    fn damage_panels(&self, damage: &mut Damage) {
        for panel in self.layout.panels() {
            damage.add(panel);
        }
    }

    fn damage_driven(&self, damage: &mut Damage) {
        if let Some((id, _)) = self.agent.driven
            && let Some(slot) = self.find(id)
            && self.is_visible(slot)
        {
            damage.add(self.window_at(slot).title_bar());
        }
    }

    fn damage_agent_pointer(&self, damage: &mut Damage) {
        if let Some((x, y, _)) = self.agent.pointer {
            damage.add(pointer::area(PointerShape::Arrow, x, y));
        }
    }

    pub(super) fn draw_agent_chip(&self, canvas: &mut Canvas<'_>, hovered: bool) {
        let Some(chip) = self.agent_chip() else {
            return;
        };
        let (fill, ink) = match (self.agent_acting(), hovered) {
            (true, _) => (colour::ACCENT, colour::ICON_DARK),
            (false, true) => (colour::BAR_PRESSED, colour::TEXT_DIM),
            (false, false) => (colour::BAR_HOVER, colour::TEXT_DIM),
        };
        canvas.rounded(chip, &paint::INSET_R4, true, true, fill);
        canvas.text(
            chip.x + CHIP_PADDING,
            chip.y + AGENT_LABEL_Y,
            AGENT_LABEL,
            ink,
        );
    }

    pub(super) fn draw_agent_pointer(&self, canvas: &mut Canvas<'_>) {
        let Some((x, y, _)) = self.agent.pointer else {
            return;
        };
        let area = pointer::area(PointerShape::Arrow, x, y);
        let clip = canvas.clip();
        if !area.overlaps(&clip) {
            return;
        }
        for row in area.y.max(clip.y)..area.bottom().min(clip.bottom()) {
            for column in area.x.max(clip.x)..area.right().min(clip.right()) {
                if let Some(pixel) = pointer::agent_arrow_pixel_at((x, y), column, row) {
                    canvas.pixel(column, row, pixel);
                }
            }
        }
    }
}

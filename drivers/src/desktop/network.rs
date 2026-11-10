//! The bar's network indicator (docs/ux/network-v0.3.md section 4, D225): a 16x16 glyph of the
//! wired adapter's state drawn from a table of rectangles, directly left of the top bar's power
//! button or 8 px left of the taskbar's clock, its tooltip in the words of section 4.4, which is
//! also its accessible name, and a click that opens Settings on the Network pane. A machine
//! without a network adapter shows nothing and has no hit target there.

use super::layout::{TASKBAR_HEIGHT, TOP_BAR_HEIGHT};
use super::paint::Canvas;
use super::shell::PanelItem;
use super::{Desktop, colour};
use crate::display::{Damage, Rect};

/// The glyph's side at scale 1.
pub const NETWORK_GLYPH_SIDE: u32 = 16;
/// The tooltip while the cable is unplugged.
pub const CABLE_UNPLUGGED_TOOLTIP: &str = "Ethernet: cable unplugged";
/// The tooltip while DHCP is asking for an address.
pub const GETTING_ADDRESS_TOOLTIP: &str = "Ethernet: getting an address...";
/// The tooltip while no DHCP server answered.
pub const NO_ADDRESS_TOOLTIP: &str = "Ethernet: no address. EggOS keeps trying.";
/// The connected tooltip's words before the address.
pub const CONNECTED_TOOLTIP_PREFIX: &str = "Ethernet: connected, ";

const INDICATOR_BEFORE_CLOCK: u32 = 8;
const TOOLTIP_CAPACITY: usize = 48;
const LARGEST_SCALE: u32 = 2;

/// The state of the machine's wired adapter, as the indicator shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    /// The adapter's link is down.
    CableUnplugged,
    /// The link is up and DHCP is asking for an address.
    GettingAddress,
    /// The link is up and no DHCP server answered; the kernel keeps asking.
    NoAddress,
    /// The link is up and DHCP gave this IPv4 address.
    Connected([u8; 4]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Text,
    Dim,
    Accent,
}

impl Ink {
    const fn pixel(self) -> u32 {
        match self {
            Self::Text => colour::TEXT,
            Self::Dim => colour::TEXT_DIM,
            Self::Accent => colour::ACCENT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlyphRect {
    x: u8,
    y: u8,
    width: u8,
    height: u8,
    ink: Ink,
}

impl GlyphRect {
    fn covers(&self, column: u32, row: u32) -> bool {
        let (x, y) = (u32::from(self.x), u32::from(self.y));
        (x..x + u32::from(self.width)).contains(&column)
            && (y..y + u32::from(self.height)).contains(&row)
    }
}

const fn part(x: u8, y: u8, width: u8, height: u8, ink: Ink) -> GlyphRect {
    GlyphRect {
        x,
        y,
        width,
        height,
        ink,
    }
}

const fn outlined_box(x: u8, y: u8, ink: Ink) -> [GlyphRect; 4] {
    [
        part(x, y, 9, 1, ink),
        part(x, y + 5, 9, 1, ink),
        part(x, y + 1, 1, 4, ink),
        part(x + 8, y + 1, 1, 4, ink),
    ]
}

const MACHINE: [GlyphRect; 4] = outlined_box(0, 0, Ink::Text);
const NETWORK_REACHED: [GlyphRect; 4] = outlined_box(7, 10, Ink::Text);
const NETWORK_NOT_REACHED: [GlyphRect; 4] = outlined_box(7, 10, Ink::Dim);
const CABLE: [GlyphRect; 3] = [
    part(4, 6, 1, 2, Ink::Text),
    part(4, 8, 8, 1, Ink::Text),
    part(11, 9, 1, 1, Ink::Text),
];
const CUT_CABLE: [GlyphRect; 2] = [part(4, 6, 1, 3, Ink::Text), part(5, 8, 1, 1, Ink::Text)];
const ASKING_DOTS: [GlyphRect; 3] = [
    part(9, 12, 1, 1, Ink::Accent),
    part(11, 12, 1, 1, Ink::Accent),
    part(13, 12, 1, 1, Ink::Accent),
];
const NO_ANSWER_MARK: [GlyphRect; 2] = [
    part(11, 11, 1, 2, Ink::Accent),
    part(11, 14, 1, 1, Ink::Accent),
];

impl NetworkState {
    fn glyph(self) -> [&'static [GlyphRect]; 4] {
        match self {
            Self::Connected(_) => [&MACHINE, &CABLE, &NETWORK_REACHED, &[]],
            Self::GettingAddress => [&MACHINE, &CABLE, &NETWORK_NOT_REACHED, &ASKING_DOTS],
            Self::NoAddress => [&MACHINE, &CABLE, &NETWORK_NOT_REACHED, &NO_ANSWER_MARK],
            Self::CableUnplugged => [&MACHINE, &CUT_CABLE, &NETWORK_NOT_REACHED, &[]],
        }
    }

    /// The colour of the glyph's pixel at (`column`, `row`) of its 16x16 box at scale 1, or
    /// `None` where the bar shows through.
    pub fn glyph_pixel(self, column: u32, row: u32) -> Option<u32> {
        self.glyph()
            .into_iter()
            .flatten()
            .find(|rect| rect.covers(column, row))
            .map(|rect| rect.ink.pixel())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NetworkIndicator {
    state: Option<NetworkState>,
    tooltip: [u8; TOOLTIP_CAPACITY],
    tooltip_length: usize,
}

impl Default for NetworkIndicator {
    fn default() -> Self {
        Self {
            state: None,
            tooltip: [0; TOOLTIP_CAPACITY],
            tooltip_length: 0,
        }
    }
}

impl NetworkIndicator {
    fn showing(state: Option<NetworkState>) -> Self {
        let mut indicator = Self {
            state,
            ..Self::default()
        };
        let mut length = 0;
        let mut push = |bytes: &[u8]| {
            let end = (length + bytes.len()).min(TOOLTIP_CAPACITY);
            indicator.tooltip[length..end].copy_from_slice(&bytes[..end - length]);
            length = end;
        };
        match state {
            None => {}
            Some(NetworkState::CableUnplugged) => push(CABLE_UNPLUGGED_TOOLTIP.as_bytes()),
            Some(NetworkState::GettingAddress) => push(GETTING_ADDRESS_TOOLTIP.as_bytes()),
            Some(NetworkState::NoAddress) => push(NO_ADDRESS_TOOLTIP.as_bytes()),
            Some(NetworkState::Connected(address)) => {
                push(CONNECTED_TOOLTIP_PREFIX.as_bytes());
                for (index, octet) in address.into_iter().enumerate() {
                    if index > 0 {
                        push(b".");
                    }
                    let digits =
                        [octet / 100, octet / 10 % 10, octet % 10].map(|digit| b'0' + digit);
                    let first = match octet {
                        100.. => 0,
                        10.. => 1,
                        _ => 2,
                    };
                    push(&digits[first..]);
                }
            }
        }
        indicator.tooltip_length = length;
        indicator
    }
}

impl Desktop {
    /// Shows the wired adapter's state `state` on the bar, `None` for a machine without a
    /// network adapter (D225). The kernel calls it on each change; a change of state redraws
    /// the glyph and re-places a tooltip of the indicator that shows, and the indicator
    /// appearing or going re-lays the bar's right-hand items in the same frame.
    pub fn set_network(&mut self, state: Option<NetworkState>, damage: &mut Damage) {
        if state == self.network.state {
            return;
        }
        let comes_or_goes = state.is_some() != self.network.state.is_some();
        if comes_or_goes {
            if self.panels.tooltip == Some(PanelItem::Network) {
                self.hide_tooltip(damage);
            }
            for panel in self.layout.panels() {
                damage.add(panel);
            }
        } else if let Some(indicator) = self.network_indicator() {
            damage.add(indicator);
        }
        self.network = NetworkIndicator::showing(state);
        if comes_or_goes {
            for panel in self.layout.panels() {
                damage.add(panel);
            }
            self.panels.moved = true;
            self.refresh_panel_hover(damage);
        } else {
            self.tooltip_text_changed(PanelItem::Network, damage);
        }
    }

    /// The state the indicator shows, `None` without a network adapter.
    pub fn network_state(&self) -> Option<NetworkState> {
        self.network.state
    }

    /// The indicator's hit target while the machine has a network adapter: a bar-high square
    /// directly left of the top bar's power button, or a taskbar item 8 px left of the
    /// taskbar's clock.
    pub fn network_indicator(&self) -> Option<Rect> {
        self.network.state?;
        match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => Some(Rect::new(
                self.clock_rect()
                    .x
                    .checked_sub(INDICATOR_BEFORE_CLOCK * self.bar_scale() + bar.height)?,
                bar.y,
                bar.height,
                bar.height,
            )),
            (None, Some(bar)) => Some(Rect::new(
                self.power_button().x.checked_sub(bar.height)?,
                bar.y,
                bar.height,
                bar.height,
            )),
            (None, None) => None,
        }
    }

    /// Where the glyph's top-left pixel is: the glyph centred in the hit target.
    pub fn network_glyph_at(&self) -> Option<(u32, u32)> {
        let indicator = self.network_indicator()?;
        let side = NETWORK_GLYPH_SIDE * self.bar_scale();
        Some((
            indicator.x + indicator.width.saturating_sub(side) / 2,
            indicator.y + indicator.height.saturating_sub(side) / 2,
        ))
    }

    /// The indicator's tooltip text, which is also its accessible name, while it shows.
    pub fn network_indicator_name(&self) -> Option<&str> {
        self.network.state?;
        core::str::from_utf8(&self.network.tooltip[..self.network.tooltip_length]).ok()
    }

    fn bar_scale(&self) -> u32 {
        let (bar, height) = match (self.layout.taskbar, self.layout.top_bar) {
            (Some(bar), _) => (bar, TASKBAR_HEIGHT),
            (None, Some(bar)) => (bar, TOP_BAR_HEIGHT),
            (None, None) => return 1,
        };
        (bar.height / height).clamp(1, LARGEST_SCALE)
    }

    pub(super) fn draw_network_item(&self, canvas: &mut Canvas<'_>) {
        let (Some(state), Some(indicator), Some((x, y))) = (
            self.network.state,
            self.network_indicator(),
            self.network_glyph_at(),
        ) else {
            return;
        };
        if !indicator.overlaps(&canvas.clip()) {
            return;
        }
        let fill = match self.layout.taskbar {
            Some(_) => self.item_fill(PanelItem::Network, false).map(|fill| {
                (
                    fill,
                    Rect::new(
                        indicator.x,
                        indicator.y + 1,
                        indicator.width,
                        indicator.height - 1,
                    ),
                )
            }),
            None => (self.panels.pressed == Some(PanelItem::Network)).then_some((
                colour::BAR_PRESSED,
                Rect::new(
                    indicator.x,
                    indicator.y,
                    indicator.width,
                    indicator.height - 1,
                ),
            )),
        };
        if let Some((fill, area)) = fill {
            canvas.fill(area, fill);
        }
        let scale = self.bar_scale();
        for rect in state.glyph().into_iter().flatten() {
            canvas.fill(
                Rect::new(
                    x + u32::from(rect.x) * scale,
                    y + u32::from(rect.y) * scale,
                    u32::from(rect.width) * scale,
                    u32::from(rect.height) * scale,
                ),
                rect.ink.pixel(),
            );
        }
    }
}

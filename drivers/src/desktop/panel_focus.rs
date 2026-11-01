//! The keyboard in the panel (docs/ux/first-run-accessibility-v0.3.md section 2.8, D223):
//! `Ctrl+Alt+Tab`, `Super+T` or `Ctrl+F3` moves the keys to the bar's items, a second press to
//! the dock's icons where there is a dock, and the next back to the window; Left, Right, Tab
//! and Shift+Tab move the ring, Enter or Space clicks the ringed item and Esc or a click ends it.

use super::paint::Canvas;
use super::protocol::EventSink;
use super::shell::PanelItem;
use super::shortcuts::{Shortcut, shortcut};
use super::{Action, Desktop, colour};
use crate::display::{Damage, Rect};
use crate::input::key;
use crate::input::keymap::Modifiers;
use alloc::vec::Vec;

const PANEL_RING_INSET: u32 = 2;
const PANEL_RING_WIDTH: u32 = 2;

/// Where the panel's keyboard focus is: the bar's items or the dock's icons, and which one,
/// counted left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PanelFocus {
    dock: bool,
    index: usize,
}

impl Desktop {
    /// Whether the keys go to the panel's items rather than the focused window.
    pub fn panel_has_focus(&self) -> bool {
        self.panel_focus.is_some()
    }

    /// The ring around the item that has the panel's keyboard focus: its hit target inset by
    /// 2 px.
    pub fn panel_focus_ring(&self) -> Option<Rect> {
        let focus = self.panel_focus?;
        let items = self.panel_focus_items(focus.dock);
        let (_, rect) = items.get(focus.index)?;
        Some(Rect::new(
            rect.x + PANEL_RING_INSET,
            rect.y + PANEL_RING_INSET,
            rect.width.saturating_sub(2 * PANEL_RING_INSET),
            rect.height.saturating_sub(2 * PANEL_RING_INSET),
        ))
    }

    fn panel_focus_items(&self, dock: bool) -> Vec<(PanelItem, Rect)> {
        let mut items = Vec::new();
        self.for_each_panel_item(|item, rect| {
            let in_dock = matches!(item, PanelItem::DockApp(_) | PanelItem::DockWindow(_));
            if in_dock == dock {
                items.push((item, rect));
            }
        });
        items.sort_by_key(|(_, rect)| rect.x);
        items
    }

    /// [`Shortcut::FocusPanel`]: the bar's first item, then the dock's first icon where there
    /// is a dock, then the window again.
    pub(super) fn focus_panel(&mut self, damage: &mut Damage) {
        let next = match self.panel_focus {
            None => Some(PanelFocus {
                dock: false,
                index: 0,
            }),
            Some(PanelFocus { dock: false, .. }) if !self.panel_focus_items(true).is_empty() => {
                Some(PanelFocus {
                    dock: true,
                    index: 0,
                })
            }
            Some(_) => None,
        };
        let next = next.filter(|focus| !self.panel_focus_items(focus.dock).is_empty());
        self.set_panel_focus(next, damage);
    }

    pub(super) fn end_panel_focus(&mut self, damage: &mut Damage) {
        if self.panel_focus.is_some() {
            self.set_panel_focus(None, damage);
        }
    }

    fn set_panel_focus(&mut self, focus: Option<PanelFocus>, damage: &mut Damage) {
        if let Some(ring) = self.panel_focus_ring() {
            damage.add(ring);
        }
        self.panel_focus = focus;
        if let Some(ring) = self.panel_focus_ring() {
            damage.add(ring);
        }
    }

    /// Key `code` pressed with `modifiers` while the panel has the keys.
    pub(super) fn panel_focus_key(
        &mut self,
        code: u16,
        modifiers: Modifiers,
        damage: &mut Damage,
        events: &mut dyn EventSink,
    ) -> Action {
        let Some(focus) = self.panel_focus else {
            return Action::None;
        };
        match shortcut(code, modifiers, self.profile, false) {
            Some(Shortcut::FocusPanel) => {
                self.focus_panel(damage);
                return Action::None;
            }
            Some(other) => {
                self.end_panel_focus(damage);
                return self.run_shortcut(other, damage, events);
            }
            None => {}
        }
        let items = self.panel_focus_items(focus.dock);
        if items.is_empty() {
            self.end_panel_focus(damage);
            return Action::None;
        }
        let count = items.len();
        let index = focus.index.min(count - 1);
        let back = modifiers.contains(Modifiers::SHIFT);
        let moved = match code {
            key::LEFT => Some((index + count - 1) % count),
            key::TAB if back => Some((index + count - 1) % count),
            key::RIGHT | key::TAB => Some((index + 1) % count),
            _ => None,
        };
        if let Some(index) = moved {
            self.set_panel_focus(
                Some(PanelFocus {
                    dock: focus.dock,
                    index,
                }),
                damage,
            );
            return Action::None;
        }
        match code {
            key::ENTER | key::KEYPAD_ENTER | key::SPACE => {
                self.end_panel_focus(damage);
                let action = self.panel_press(items[index].0, damage, events);
                self.panel_release(damage);
                action
            }
            key::ESC => {
                self.end_panel_focus(damage);
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(super) fn draw_panel_focus_ring(&self, canvas: &mut Canvas<'_>) {
        let Some(ring) = self.panel_focus_ring() else {
            return;
        };
        if ring.width < 2 * PANEL_RING_WIDTH || ring.height < 2 * PANEL_RING_WIDTH {
            return;
        }
        let inner_height = ring.height - 2 * PANEL_RING_WIDTH;
        for edge in [
            Rect::new(ring.x, ring.y, ring.width, PANEL_RING_WIDTH),
            Rect::new(
                ring.x,
                ring.bottom() - PANEL_RING_WIDTH,
                ring.width,
                PANEL_RING_WIDTH,
            ),
            Rect::new(
                ring.x,
                ring.y + PANEL_RING_WIDTH,
                PANEL_RING_WIDTH,
                inner_height,
            ),
            Rect::new(
                ring.right() - PANEL_RING_WIDTH,
                ring.y + PANEL_RING_WIDTH,
                PANEL_RING_WIDTH,
                inner_height,
            ),
        ] {
            canvas.fill(edge, colour::FOCUS_RING);
        }
    }
}

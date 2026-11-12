//! When a widget's tooltip shows and hides (docs/ux/toolkit-v0.5.md section 2.3 and 2.4).

use super::{Kind, WidgetId, WidgetTree};
use crate::canvas::Canvas;
use crate::geometry::Rect;
use crate::widget::{
    FormFactor, LONG_PRESS_MS, TOOLTIP_DELAY_MS, TOOLTIP_HOP_MS, TOOLTIP_TOUCH_MS,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct Tip {
    owner: Option<WidgetId>,
    hovered: Option<WidgetId>,
    since: u32,
    shown: bool,
    spent: bool,
    hidden_at: Option<u32>,
    until: Option<u32>,
    long_press: Option<(WidgetId, u32)>,
    press_spent: bool,
}

impl Tip {
    pub(super) const NONE: Self = Self {
        owner: None,
        hovered: None,
        since: 0,
        shown: false,
        spent: false,
        hidden_at: None,
        until: None,
        long_press: None,
        press_spent: false,
    };

    pub(super) fn spent_on_press(&mut self) -> bool {
        self.long_press = None;
        core::mem::take(&mut self.press_spent)
    }
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    fn tooltip_of(&self, owner: WidgetId) -> Option<WidgetId> {
        self.ids().find(|&id| {
            self.node(id).is_some_and(|node| {
                node.parent == Some(owner) && node.shown && matches!(node.kind, Kind::Tooltip(_))
            })
        })
    }

    fn tooltip_blocked(&self) -> bool {
        self.popup.is_some()
            || self.menu_open()
            || self.bar_has_keyboard()
            || self.pick.is_some()
            || self.column.is_some()
            || self.form == FormFactor::Wearable
    }

    fn damage_tooltip(&mut self) {
        if let Some(owner) = self.tip.owner
            && let Some(tooltip) = self.tooltip_of(owner)
            && let Some(rect) = self.rect(tooltip)
        {
            self.damage.add(rect);
        }
    }

    fn show_tooltip(&mut self, now_ms: u32) {
        if self.tip.shown || self.tooltip_blocked() {
            return;
        }
        let Some(owner) = self.tip.owner else {
            return;
        };
        if !self.is_shown(owner) || !self.window_focused && self.form != FormFactor::Phone {
            return;
        }
        self.tip.shown = true;
        self.tip.until = match self.form {
            FormFactor::Phone => Some(now_ms.wrapping_add(TOOLTIP_TOUCH_MS)),
            _ => None,
        };
        self.damage_tooltip();
    }

    pub(super) fn tip_owned_by(&self, id: WidgetId) -> bool {
        self.tip
            .owner
            .is_some_and(|owner| self.is_within(owner, id))
    }

    /// Hides the tooltip showing, if any.
    pub(super) fn hide_tooltip(&mut self) {
        if self.tip.shown {
            self.damage_tooltip();
            self.tip.shown = false;
            self.tip.until = None;
        }
    }

    fn set_tooltip_owner(&mut self, owner: Option<WidgetId>, now_ms: u32) {
        if self.tip.owner == owner {
            return;
        }
        let was_shown = self.tip.shown;
        self.hide_tooltip();
        if was_shown {
            self.tip.hidden_at = Some(now_ms);
        }
        self.tip.owner = owner;
        self.tip.since = now_ms;
        self.tip.spent = false;
        let hop = self
            .tip
            .hidden_at
            .is_some_and(|hidden| now_ms.wrapping_sub(hidden) <= TOOLTIP_HOP_MS);
        if owner.is_some() && hop {
            self.show_tooltip(now_ms);
        }
    }

    /// The pointer moved: the widget under it with a tooltip becomes the owner.
    pub(super) fn tooltip_hover(&mut self, now_ms: u32) {
        let hovered = self
            .hover
            .map(|target| target.widget)
            .filter(|&widget| self.tooltip_of(widget).is_some());
        if hovered == self.tip.hovered {
            return;
        }
        let leaving = self.tip.hovered;
        self.tip.hovered = hovered;
        if hovered.is_some() || self.tip.owner == leaving {
            self.set_tooltip_owner(hovered, now_ms);
        }
    }

    /// The keyboard focus moved to `id` with the window focused.
    pub(super) fn tooltip_focus(&mut self, id: WidgetId, now_ms: u32) {
        let owner = self.tooltip_of(id).map(|_| id);
        self.set_tooltip_owner(owner, now_ms);
    }

    /// A button was pressed: the tooltip hides and stays away until the pointer leaves.
    pub(super) fn tooltip_spent(&mut self) {
        self.hide_tooltip();
        self.tip.spent = true;
    }

    /// A key went down: the tooltip hides and stays away.
    pub(super) fn tooltip_key(&mut self) {
        self.tooltip_spent();
    }

    /// A press on `id` on a touch screen: held still for a long-press, it shows `id`'s tooltip.
    pub(super) fn tooltip_long_press(&mut self, id: WidgetId, now_ms: u32) {
        if self.tooltip_of(id).is_some() {
            self.tip.long_press = Some((id, now_ms));
        }
    }

    pub(super) fn tooltip_tick(&mut self, now_ms: u32) {
        if self.tooltip_blocked() {
            self.hide_tooltip();
            return;
        }
        if let Some((id, pressed)) = self.tip.long_press
            && now_ms.wrapping_sub(pressed) >= LONG_PRESS_MS
        {
            self.tip.long_press = None;
            self.tip.press_spent = true;
            self.tip.owner = Some(id);
            self.tip.spent = false;
            self.show_tooltip(now_ms);
            return;
        }
        if self.tip.shown {
            if let Some(until) = self.tip.until
                && now_ms.wrapping_sub(until) as i32 >= 0
            {
                self.hide_tooltip();
                self.tip.spent = true;
            }
            return;
        }
        let due = now_ms.wrapping_sub(self.tip.since) >= TOOLTIP_DELAY_MS;
        if self.tip.owner.is_some() && !self.tip.spent && due {
            self.show_tooltip(now_ms);
        }
    }

    pub(super) fn tooltip_deadline(&self) -> Option<u32> {
        if let Some((_, pressed)) = self.tip.long_press {
            return Some(pressed.wrapping_add(LONG_PRESS_MS));
        }
        match (self.tip.shown, self.tip.until) {
            (true, until) => until,
            (false, _)
                if self.tip.owner.is_some() && !self.tip.spent && !self.tooltip_blocked() =>
            {
                Some(self.tip.since.wrapping_add(TOOLTIP_DELAY_MS))
            }
            _ => None,
        }
    }

    /// The tooltip showing now, if any: its widget.
    pub fn shown_tooltip(&self) -> Option<WidgetId> {
        let owner = self.tip.owner.filter(|_| self.tip.shown)?;
        self.tooltip_of(owner)
    }

    pub(super) fn paint_tooltip(&self, canvas: &mut Canvas<'_>, clip: Rect) {
        if self.tooltip_blocked() {
            return;
        }
        let Some(id) = self.shown_tooltip() else {
            return;
        };
        let (Some(Kind::Tooltip(tooltip)), Some(rect)) = (self.kind(id), self.rect(id)) else {
            return;
        };
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let previous = canvas.narrow(rect.intersection(&window).intersection(&clip));
        crate::tooltip::paint_tile(
            canvas,
            rect,
            tooltip.text.as_str(),
            tooltip.shortcut.as_str(),
        );
        canvas.restore(previous);
    }
}

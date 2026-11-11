//! Drag and drop inside one window (docs/ux/toolkit-v0.5.md section 7): picking up a row of a
//! list or table or a whole custom widget, the ghost under the pointer, where it would land,
//! scrolling a target near its edges, and what the drop says.

use super::{Kind, Response, Target, WidgetId, WidgetTree};
use crate::canvas::Canvas;
use crate::drag::{DropMode, Landing};
use crate::geometry::Rect;
use crate::table::ROW_HIT;
use crate::text::{self, Text};
use crate::widget::{
    DRAG_EDGE, DRAG_GHOST_OFFSET, DRAG_SCROLL_MS, DRAG_THRESHOLD, FormFactor, LONG_PRESS_MS,
    TABLE_ROW_HEIGHT, TOOLTIP_HEIGHT, TOOLTIP_PADDING,
};

const TOUCH_GHOST_RISE: i32 = 36;

#[derive(Debug, Clone, Copy)]
pub(super) struct Pick {
    source: WidgetId,
    part: usize,
    from: (i32, i32),
    at: (i32, i32),
    pub(super) started: bool,
    double: bool,
    pressed_ms: u32,
    scrolled_ms: u32,
    landing: Option<(WidgetId, Landing)>,
}

/// A drag from another window over this one (D394): where it would land.
#[derive(Debug, Clone, Copy)]
pub(super) struct Foreign {
    landing: Option<(WidgetId, Landing)>,
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    /// Lets a drag from `id` leave the window: crossing the window's edge then answers
    /// [`Response::DragLeftWindow`] and ends the drag here (D394).
    pub fn set_drag_out(&mut self, id: WidgetId, out: bool) {
        if let Some(node) = self.node_mut(id) {
            node.drag_out = out;
        }
    }

    /// The words a drag of `part` of `source` shows, for the server's ghost.
    pub fn drag_words(&self, source: WidgetId, part: usize) -> Text<64> {
        self.ghost_text(source, part)
    }

    pub(super) fn foreign_over(&mut self, x: i32, y: i32) {
        self.damage_foreign();
        let landing = self.landing_for(None, x, y);
        self.foreign = Some(Foreign { landing });
        self.damage_foreign();
    }

    pub(super) fn foreign_left(&mut self) {
        self.damage_foreign();
        self.foreign = None;
    }

    pub(super) fn foreign_dropped(&mut self, x: i32, y: i32, kind: u32, serial: u32) -> Response {
        self.damage_foreign();
        self.foreign = None;
        match self.landing_for(None, x, y) {
            Some((target, landing)) => Response::ForeignDropped {
                target,
                at: match landing {
                    Landing::Onto { row, .. } => row,
                    Landing::Between { at, .. } => at,
                },
                kind,
                serial,
            },
            None => Response::Nothing,
        }
    }

    fn damage_foreign(&mut self) {
        if let Some(Foreign {
            landing: Some((target, landing)),
            ..
        }) = self.foreign
        {
            self.damage.add(landing.rect().inset(-2));
            self.damage_widget(target);
        }
    }

    /// Lets the rows of list or table `id`, or the whole of custom widget `id`, be picked up
    /// and dragged.
    pub fn set_drag_source(&mut self, id: WidgetId, source: bool) {
        if let Some(node) = self.node_mut(id) {
            node.drag_source = source;
        }
    }

    /// Lets `id` take drops as `mode` says, or none for `None`.
    pub fn set_drop_target(&mut self, id: WidgetId, mode: Option<DropMode>) {
        if let Some(node) = self.node_mut(id) {
            node.drop = mode;
        }
    }

    /// The words a drag of custom widget `id` shows in its ghost.
    pub fn set_drag_text(&mut self, id: WidgetId, words: &str) {
        if let Some(node) = self.node_mut(id) {
            node.drag_text = Text::from(words);
        }
    }

    /// Refuses the drag [`Response::DragStarted`] just announced: the ghost never shows and the
    /// release does nothing.
    pub fn cancel_drag(&mut self) {
        self.damage_pick();
        self.pick = None;
    }

    pub(super) fn is_drag_source(&self, id: WidgetId) -> bool {
        self.form != FormFactor::Wearable && self.node(id).is_some_and(|node| node.drag_source)
    }

    pub(super) fn start_pick(
        &mut self,
        id: WidgetId,
        part: usize,
        x: i32,
        y: i32,
        time_ms: u32,
        double: bool,
    ) {
        self.pick = Some(Pick {
            source: id,
            part,
            from: (x, y),
            at: (x, y),
            started: false,
            double,
            pressed_ms: time_ms,
            scrolled_ms: time_ms,
            landing: None,
        });
        self.damage_widget(id);
    }

    fn ghost_text(&self, source: WidgetId, part: usize) -> Text<64> {
        match self.kind(source) {
            Some(Kind::List(list)) => Text::from(list.items.get(part).copied().unwrap_or("")),
            Some(Kind::Table(table)) => table.cell_text(part, 0),
            _ => self
                .node(source)
                .map_or_else(Text::new, |node| Text::from(node.drag_text.as_str())),
        }
    }

    fn ghost_rect(&self, pick: &Pick) -> Rect {
        let words = self.ghost_text(pick.source, pick.part);
        let rise = match self.form {
            FormFactor::Phone => -TOUCH_GHOST_RISE,
            _ => DRAG_GHOST_OFFSET,
        };
        Rect::new(
            pick.at.0 + DRAG_GHOST_OFFSET,
            pick.at.1 + rise,
            2 * TOOLTIP_PADDING + text::width(words.as_str()),
            TOOLTIP_HEIGHT,
        )
    }

    fn damage_pick(&mut self) {
        let Some(pick) = self.pick else {
            return;
        };
        if pick.started {
            self.damage.add(self.ghost_rect(&pick));
        }
        if let Some((target, landing)) = pick.landing {
            self.damage.add(landing.rect().inset(-2));
            self.damage_widget(target);
        }
        self.damage_widget(pick.source);
    }

    fn landing_at(&self, pick: &Pick, x: i32, y: i32) -> Option<(WidgetId, Landing)> {
        self.landing_for(Some((pick.source, pick.part)), x, y)
    }

    fn landing_for(
        &self,
        source: Option<(WidgetId, usize)>,
        x: i32,
        y: i32,
    ) -> Option<(WidgetId, Landing)> {
        let id = self.ids().rev().find(|&id| {
            self.node(id).is_some_and(|node| {
                node.drop.is_some()
                    && node.enabled
                    && node.rect.contains(x, y)
                    && node.visible.contains(x, y)
            }) && self.is_shown(id)
        })?;
        let node = self.node(id)?;
        let mode = node.drop?;
        let own = source.is_some_and(|(source, _)| source == id);
        let part = source.map_or(usize::MAX, |(_, part)| part);
        let rect = node.rect;
        let (rows, first_top, row_height, view) = match &node.kind {
            Kind::List(list) => (
                list.shown_items().count(),
                rect.y - list.offset,
                list.row_height,
                widget_row_view(rect, list.content_height(), list.offset),
            ),
            Kind::Table(table) => {
                let view = table.view(rect);
                (table.rows, view.y - table.offset_y, TABLE_ROW_HEIGHT, view)
            }
            _ => {
                return match (mode, own) {
                    (DropMode::Onto, false) => Some((id, Landing::Onto { row: 0, rect })),
                    _ => None,
                };
            }
        };
        if !view.contains(x, y) {
            return None;
        }
        let item_of = |row: usize| match &node.kind {
            Kind::List(list) => list.item_at_row(row),
            _ => Some(row),
        };
        match mode {
            DropMode::Onto => {
                let row = ((y - first_top) / row_height.max(1)).max(0) as usize;
                if row >= rows {
                    return None;
                }
                let item = item_of(row)?;
                if own && item == part {
                    return None;
                }
                let rect = Rect::new(
                    view.x,
                    first_top + row as i32 * row_height,
                    view.width,
                    row_height,
                );
                Some((id, Landing::Onto { row: item, rect }))
            }
            DropMode::Between => {
                let landing = Landing::between(view, rows, first_top, row_height, y);
                let Landing::Between { at, .. } = landing else {
                    return None;
                };
                let source_row = match &node.kind {
                    Kind::List(list) => list.row_of(part),
                    _ => Some(part),
                };
                if own && source_row.is_some_and(|row| at == row || at == row + 1) {
                    return None;
                }
                Some((id, landing))
            }
        }
    }

    fn begin_drag(&mut self, mut pick: Pick) -> Response {
        pick.started = true;
        self.hide_tooltip();
        self.close_popup();
        self.close_menu();
        self.pressed = None;
        pick.landing = self.landing_at(&pick, pick.at.0, pick.at.1);
        self.pick = Some(pick);
        self.damage_pick();
        Response::DragStarted(pick.source, pick.part)
    }

    pub(super) fn pick_moved(&mut self, x: i32, y: i32, _time_ms: u32) -> Response {
        let Some(mut pick) = self.pick else {
            return Response::Nothing;
        };
        pick.at = (x, y);
        if !pick.started {
            self.pick = Some(pick);
            let moved = (x - pick.from.0).abs() + (y - pick.from.1).abs();
            if moved >= DRAG_THRESHOLD {
                return self.begin_drag(pick);
            }
            return Response::Nothing;
        }
        let outside = x < 0 || y < 0 || x >= self.size.0 || y >= self.size.1;
        if outside && self.node(pick.source).is_some_and(|node| node.drag_out) {
            self.cancel_drag();
            return Response::DragLeftWindow(pick.source, pick.part);
        }
        self.damage_pick();
        pick.landing = self.landing_at(&pick, x, y);
        self.pick = Some(pick);
        self.damage_pick();
        Response::Nothing
    }

    pub(super) fn pick_released(&mut self, x: i32, y: i32, time_ms: u32) -> Response {
        let Some(pick) = self.pick else {
            return Response::Nothing;
        };
        self.damage_pick();
        self.pick = None;
        self.set_hover(self.hit(x, y));
        if pick.started {
            return match pick.landing {
                Some((target, landing)) => Response::Dropped {
                    source: pick.source,
                    part: pick.part,
                    target,
                    at: match landing {
                        Landing::Onto { row, .. } => row,
                        Landing::Between { at, .. } => at,
                    },
                },
                None => Response::DragCancelled(pick.source),
            };
        }
        let _ = time_ms;
        match self.kind(pick.source).copied() {
            Some(Kind::List(list)) => {
                if pick.double {
                    return Response::Opened(pick.source, pick.part);
                }
                if list.selected != pick.part {
                    self.set_selected(pick.source, pick.part);
                    return Response::Chosen(pick.source, pick.part);
                }
                Response::Nothing
            }
            Some(Kind::Table(table)) => {
                if pick.double && table.selected == pick.part {
                    return Response::Opened(pick.source, pick.part);
                }
                self.select_row(pick.source, pick.part)
            }
            _ => Response::Nothing,
        }
    }

    /// Ends a drag without a drop: [`Response::DragCancelled`] when one was on.
    pub(super) fn cancel_drag_response(&mut self) -> Response {
        let Some(pick) = self.pick else {
            return Response::Nothing;
        };
        self.cancel_drag();
        match pick.started {
            true => Response::DragCancelled(pick.source),
            false => Response::Nothing,
        }
    }

    pub(super) fn cancel_pick_on_press(&mut self) -> Option<Response> {
        let pick = self.pick?;
        self.cancel_drag();
        pick.started.then_some(Response::DragCancelled(pick.source))
    }

    pub(super) fn pick_tick(&mut self, now_ms: u32) -> Response {
        let Some(mut pick) = self.pick else {
            return Response::Nothing;
        };
        if !pick.started {
            let long = now_ms.wrapping_sub(pick.pressed_ms) >= LONG_PRESS_MS;
            if self.form == FormFactor::Phone && long {
                return self.begin_drag(pick);
            }
            return Response::Nothing;
        }
        if now_ms.wrapping_sub(pick.scrolled_ms) < DRAG_SCROLL_MS {
            return Response::Nothing;
        }
        let Some((target, _)) = self.edge_target(&pick) else {
            return Response::Nothing;
        };
        pick.scrolled_ms = now_ms;
        self.pick = Some(pick);
        let Some(rect) = self.rect(target) else {
            return Response::Nothing;
        };
        let (row_height, offset) = match self.kind(target) {
            Some(Kind::List(list)) => (list.row_height, list.offset),
            Some(Kind::Table(table)) => (TABLE_ROW_HEIGHT, table.offset_y),
            _ => return Response::Nothing,
        };
        let up = pick.at.1 < rect.y + DRAG_EDGE + row_height_of_header(self.kind(target));
        let wanted = match up {
            true => offset - row_height,
            false => offset + row_height,
        };
        self.damage_pick();
        self.scroll_axis_to(target, false, wanted);
        let at = pick.at;
        pick.landing = self.landing_at(&pick, at.0, at.1);
        self.pick = Some(pick);
        self.damage_pick();
        Response::Nothing
    }

    fn edge_target(&self, pick: &Pick) -> Option<(WidgetId, Rect)> {
        let (x, y) = pick.at;
        let id = self.ids().rev().find(|&id| {
            self.node(id).is_some_and(|node| {
                node.drop.is_some()
                    && matches!(node.kind, Kind::List(_) | Kind::Table(_))
                    && node.rect.contains(x, y)
            }) && self.is_shown(id)
        })?;
        let rect = self.rect(id)?;
        let top = rect.y + row_height_of_header(self.kind(id));
        let (offset, most) = self.scroll_range(id, false)?;
        let near_top = y < top + DRAG_EDGE && offset > 0;
        let near_bottom = y >= rect.bottom() - DRAG_EDGE && offset < most;
        (near_top || near_bottom).then_some((id, rect))
    }

    pub(super) fn pick_deadline(&self) -> Option<u32> {
        let pick = self.pick?;
        match pick.started {
            false if self.form == FormFactor::Phone => {
                Some(pick.pressed_ms.wrapping_add(LONG_PRESS_MS))
            }
            false => None,
            true => self
                .edge_target(&pick)
                .map(|_| pick.scrolled_ms.wrapping_add(DRAG_SCROLL_MS)),
        }
    }

    /// The row of `id` a drag would land on, drawn hovered under the highlight.
    pub(super) fn landing_hover(&self, id: WidgetId) -> Option<usize> {
        let landing = match (self.pick.filter(|pick| pick.started), self.foreign) {
            (Some(pick), _) => pick.landing,
            (None, foreign) => foreign?.landing,
        };
        match landing? {
            (target, Landing::Onto { row, .. }) if target == id => match self.kind(id)? {
                Kind::List(_) => Some(row),
                Kind::Table(_) => Some(ROW_HIT + row),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn paint_landing(&self, canvas: &mut Canvas<'_>, clip: Rect) {
        let landing = match (self.pick.filter(|pick| pick.started), self.foreign) {
            (Some(pick), _) => pick.landing,
            (None, Some(foreign)) => foreign.landing,
            (None, None) => return,
        };
        if let Some((target, landing)) = landing
            && let Some(node) = self.node(target)
        {
            let previous = canvas.narrow(node.visible.intersection(&clip));
            landing.paint(canvas);
            canvas.restore(previous);
        }
    }

    pub(super) fn paint_ghost(&self, canvas: &mut Canvas<'_>, clip: Rect) {
        let Some(pick) = self.pick.filter(|pick| pick.started) else {
            return;
        };
        let rect = self.ghost_rect(&pick);
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let previous = canvas.narrow(rect.intersection(&window).intersection(&clip));
        let words = self.ghost_text(pick.source, pick.part);
        crate::tooltip::paint_tile(canvas, rect, words.as_str(), "");
        canvas.restore(previous);
    }

    /// Widget `id` was hidden: its tooltip, its menu and a drag from it end.
    pub(super) fn widget_went(&mut self, id: WidgetId) {
        self.widget_disabled(id);
    }

    /// Widget `id` was disabled or hidden.
    pub(super) fn widget_disabled(&mut self, id: WidgetId) {
        if self.tip_owned_by(id) {
            self.hide_tooltip();
        }
        if self.menu.is_some_and(|menu| self.is_within(menu, id)) {
            self.close_menu();
        }
        if self.popup.is_some_and(|popup| self.is_within(popup, id)) {
            self.close_popup();
        }
        if self
            .pick
            .is_some_and(|pick| self.is_within(pick.source, id))
        {
            self.cancel_drag();
        }
        if self
            .column
            .is_some_and(|column| self.is_within(column.table, id))
        {
            self.end_column_drag(true);
        }
        if self
            .hover
            .is_some_and(|hover: Target| self.is_within(hover.widget, id))
        {
            self.set_hover(None);
        }
    }
}

fn widget_row_view(rect: Rect, content: i32, offset: i32) -> Rect {
    match crate::widget::scroll_bar(rect, content, offset) {
        Some(_) => Rect::new(
            rect.x,
            rect.y,
            rect.width - crate::widget::SCROLL_BAR_WIDTH,
            rect.height,
        ),
        None => rect,
    }
}

fn row_height_of_header(kind: Option<&Kind>) -> i32 {
    match kind {
        Some(Kind::Table(_)) => crate::widget::TABLE_HEADER_HEIGHT,
        _ => 0,
    }
}

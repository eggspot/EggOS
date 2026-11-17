//! A table's rows, sorting and column resizing by pointer and keys (docs/ux/toolkit-v0.5.md
//! section 4.4), and the program's calls that change it.

use super::{Kind, Response, Target, WidgetId, WidgetTree};
use crate::input::key;
use crate::table::{HANDLE_PART, ROW_HIT, RowSelected, SelectionChange, Table};
use crate::table::ColumnMove;
use crate::widget::{
    DRAG_EDGE, DRAG_SCROLL_MS, DRAG_THRESHOLD, MAX_COLUMNS, SortOrder, TABLE_KEY_SCROLL,
    TABLE_ROW_HEIGHT,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnDrag {
    pub(super) table: WidgetId,
    pub(super) column: usize,
    start_width: i32,
    grab_x: i32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HeaderMove {
    table: WidgetId,
    column: usize,
    start_x: i32,
    grab: i32,
    pub(super) started: bool,
    scrolled_ms: Option<u32>,
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    /// Draws table `id`'s columns in `order` (visual position to column index, every column
    /// once); an order that is not one is ignored. Without a response.
    pub fn set_column_order(&mut self, id: WidgetId, order: &[u8]) {
        self.change_table(id, |table| {
            let count = table.column_count;
            let mut seen = [false; MAX_COLUMNS];
            let whole = order.len() == count
                && order.iter().all(|&column| {
                    let column = usize::from(column);
                    column < count && !core::mem::replace(&mut seen[column], true)
                });
            if whole {
                table.order[..count].copy_from_slice(order);
            }
        });
    }

    pub(super) fn move_column_by_agent(
        &mut self,
        id: WidgetId,
        column: usize,
        position: usize,
    ) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
        let from = table.position_of(column);
        if from == position {
            return Response::Nothing;
        }
        let slot = match position > from {
            true => position + 1,
            false => position,
        };
        let mut to = from;
        self.change_table(id, |table| to = table.move_column(from, slot));
        Response::ColumnMoved(id, from, to)
    }

    /// Table `id`'s columns as drawn, by index, left to right.
    pub fn column_order(&self, id: WidgetId) -> Option<[u8; MAX_COLUMNS]> {
        self.table(id).map(|table| table.order)
    }

    pub(super) fn header_moved(&mut self, moving: HeaderMove, x: i32, now_ms: u32) -> Response {
        let (Some(table), Some(rect)) = (self.table(moving.table), self.rect(moving.table)) else {
            self.header_move = None;
            return Response::Nothing;
        };
        let started = moving.started || (x - moving.start_x).abs() >= DRAG_THRESHOLD;
        if !started {
            return Response::Nothing;
        }
        let view = table.view(rect);
        let step = match () {
            () if table.content_width() <= view.width => 0,
            () if x < view.x + DRAG_EDGE => -DRAG_EDGE,
            () if x >= view.right() - DRAG_EDGE => DRAG_EDGE,
            () => 0,
        };
        let due = moving
            .scrolled_ms
            .is_none_or(|at| now_ms.wrapping_sub(at) >= DRAG_SCROLL_MS);
        let scrolled_ms = match (step != 0, due) {
            (true, true) => {
                self.scroll_axis_to(moving.table, true, table.offset_x + step);
                Some(now_ms)
            }
            (true, false) => moving.scrolled_ms,
            (false, _) => None,
        };
        self.header_move = Some(HeaderMove {
            started: true,
            scrolled_ms,
            ..moving
        });
        let Some(table) = self.table(moving.table) else {
            return Response::Nothing;
        };
        let from = table.position_of(moving.column);
        let slot = Some(table.slot_at(rect, x)).filter(|&slot| slot != from && slot != from + 1);
        let ghost_x = x - moving.grab;
        self.change_table(moving.table, |table| {
            table.moving = Some(ColumnMove {
                column: moving.column,
                ghost_x,
                slot,
            });
        });
        Response::Nothing
    }

    /// Ends a header's drag: `cancel` leaves the order, else a release over a slot moves the
    /// column and says [`Response::ColumnMoved`].
    pub(super) fn end_header_move(&mut self, cancel: bool) -> Response {
        let Some(moving) = self.header_move.take() else {
            return Response::Nothing;
        };
        let Some(table) = self.table(moving.table) else {
            return Response::Nothing;
        };
        let slot = table.moving.and_then(|shown| shown.slot);
        let from = table.position_of(moving.column);
        let mut to = from;
        self.change_table(moving.table, |table| {
            table.moving = None;
            if let (false, Some(slot)) = (cancel, slot) {
                to = table.move_column(from, slot);
            }
        });
        match to == from {
            true => Response::Nothing,
            false => Response::ColumnMoved(moving.table, from, to),
        }
    }

    fn table(&self, id: WidgetId) -> Option<Table> {
        match self.kind(id)? {
            Kind::Table(table) => Some(*table),
            _ => None,
        }
    }

    fn change_table(&mut self, id: WidgetId, change: impl FnOnce(&mut Table)) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let rect = node.rect;
        if let Kind::Table(table) = &mut node.kind {
            change(table);
            table.clamp_offsets(rect);
        }
        self.damage_widget(id);
    }

    /// Gives table or outline `id` `rows` rows (after the program sorted, filtered, opened or
    /// closed what its rows read), keeping the selection in range; without a response.
    pub fn set_rows(&mut self, id: WidgetId, rows: usize) {
        if self.outline(id).is_some() {
            self.change_outline(id, |outline| {
                outline.rows = rows;
                outline.selected = outline.selected.min(rows.saturating_sub(1));
            });
            return;
        }
        self.change_table(id, |table| {
            table.rows = rows;
            table.selected = table.selected.min(rows.saturating_sub(1));
        });
    }

    /// Makes `data` what table or outline `id` gives its row function from now on; without
    /// a response.
    pub fn set_cell_data(&mut self, id: WidgetId, data: u32) {
        if self.outline(id).is_some() {
            self.change_outline(id, |outline| outline.data = data);
            return;
        }
        self.change_table(id, |table| table.data = data);
    }

    /// Makes `text` what table or outline `id` draws while it has no rows; without a
    /// response.
    pub fn set_empty_text(&mut self, id: WidgetId, text: &'static str) {
        if self.outline(id).is_some() {
            self.change_outline(id, |outline| outline.empty_text = text);
            return;
        }
        self.change_table(id, |table| table.empty_text = text);
    }

    /// Draws table `id` sorted by `sort`, or with no arrow for `None`; without a response.
    pub fn set_sort(&mut self, id: WidgetId, sort: Option<(usize, SortOrder)>) {
        self.change_table(id, |table| {
            table.sort = sort.filter(|(column, _)| *column < table.column_count);
        });
    }

    /// Makes column `column` of table `id` `width` wide, at least its narrowest; without a
    /// response.
    pub fn set_column_width(&mut self, id: WidgetId, column: usize, width: i32) {
        self.change_table(id, |table| {
            if column < table.column_count {
                let held = &mut table.columns[column];
                held.width = width.max(held.min_width);
            }
        });
    }

    /// Turns table `id`'s multi-selection on with `is_selected` telling which rows the
    /// program's set holds, or off with None; without a response.
    pub fn set_row_selection(&mut self, id: WidgetId, is_selected: Option<RowSelected>) {
        self.change_table(id, |table| {
            table.selection = is_selected;
            table.anchor = table.selected;
        });
    }

    /// Says how many rows table `id`'s set holds now, which agents read; without a response.
    pub fn set_selection_count(&mut self, id: WidgetId, count: usize) {
        self.change_table(id, |table| table.selected_count = count);
    }

    /// Turns table `id`'s cell focus on or off; without a response.
    pub fn set_cell_focus(&mut self, id: WidgetId, on: bool) {
        self.change_table(id, |table| {
            table.cell_focus = on;
            table.column = table.column.min(table.column_count.saturating_sub(1));
        });
    }

    /// Table `id`'s focused cell: its row and column.
    pub fn focused_cell(&self, id: WidgetId) -> Option<(usize, usize)> {
        self.table(id).map(|table| (table.selected, table.column))
    }

    fn selection_change(
        &mut self,
        id: WidgetId,
        current: usize,
        keep_anchor: bool,
        change: SelectionChange,
    ) -> Response {
        self.change_table(id, |table| {
            table.selected = current.min(table.rows.saturating_sub(1));
            if !keep_anchor {
                table.anchor = table.selected;
            }
        });
        self.keep_row_visible(id);
        Response::Selection(id, change)
    }

    fn focus_cell(&mut self, id: WidgetId, column: usize) -> Response {
        let (Some(table), Some(rect)) = (self.table(id), self.rect(id)) else {
            return Response::Nothing;
        };
        let column = column.min(table.column_count.saturating_sub(1));
        self.change_table(id, |table| table.column = column);
        let offset = table.offset_showing_column(rect, column);
        self.scroll_axis_to(id, true, offset);
        Response::CellFocused(id, table.selected, column)
    }

    pub(super) fn focus_cell_by_agent(&mut self, id: WidgetId, column: usize) -> Response {
        self.focus_cell(id, column)
    }

    pub(super) fn selection_by_agent(&mut self, id: WidgetId, change: SelectionChange) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
        let (current, keep_anchor) = match change {
            SelectionChange::Only(row) | SelectionChange::Toggle(row) => (row, false),
            SelectionChange::Range(_, last) | SelectionChange::AddRange(_, last) => (last, true),
            SelectionChange::All | SelectionChange::Clear => (table.selected, true),
        };
        self.selection_change(id, current, keep_anchor, change)
    }

    pub(super) fn select_row(&mut self, id: WidgetId, row: usize) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
        if table.selection.is_some() && row < table.rows {
            return self.selection_change(id, row, false, SelectionChange::Only(row));
        }
        if table.cell_focus && row < table.rows && row != table.selected {
            self.change_table(id, |table| table.selected = row);
            self.keep_row_visible(id);
            return Response::CellFocused(id, row, table.column);
        }
        if row >= table.rows || row == table.selected {
            self.keep_row_visible(id);
            return Response::Nothing;
        }
        self.change_table(id, |table| table.selected = row);
        self.keep_row_visible(id);
        Response::Chosen(id, row)
    }

    pub(super) fn keep_row_visible(&mut self, id: WidgetId) {
        let (Some(table), Some(rect)) = (self.table(id), self.rect(id)) else {
            return;
        };
        let offset = table.offset_showing(rect, table.selected);
        self.scroll_axis_to(id, false, offset);
    }

    pub(super) fn table_pressed(
        &mut self,
        id: WidgetId,
        part: usize,
        x: i32,
        y: i32,
        time_ms: u32,
        double: bool,
    ) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
        if part >= ROW_HIT {
            let row = part - ROW_HIT;
            if table.cell_focus
                && let Some(rect) = self.rect(id)
                && let Some(column) = table.column_at_x(rect, x)
                && column != table.column
            {
                self.change_table(id, |table| table.column = column);
                if table.selection.is_none() && row == table.selected && !double {
                    return Response::CellFocused(id, row, column);
                }
            }
            let (mod_held, shift) = (self.shortcut_held, self.shift_held);
            if table.selection.is_some() && (mod_held || shift) {
                let change = match (mod_held, shift) {
                    (true, true) => SelectionChange::AddRange(table.anchor, row),
                    (false, true) => SelectionChange::Range(table.anchor, row),
                    _ => SelectionChange::Toggle(row),
                };
                return self.selection_change(id, row, shift, change);
            }
            if self.is_drag_source(id) {
                self.start_pick(id, row, x, y, time_ms, double);
                return Response::Nothing;
            }
            if double && row == table.selected {
                return Response::Opened(id, row);
            }
            return self.select_row(id, row);
        }
        if part >= HANDLE_PART {
            let column = part - HANDLE_PART;
            if double {
                let Some(rect) = self.rect(id) else {
                    return Response::Nothing;
                };
                let before = table.columns[column].width;
                let width = table.fit_width(rect, column);
                self.set_column_width(id, column, width);
                return match width == before {
                    true => Response::Nothing,
                    false => Response::ColumnResized(id, column, width),
                };
            }
            self.column = Some(ColumnDrag {
                table: id,
                column,
                start_width: table.columns[column].width,
                grab_x: x,
            });
            self.damage_widget(id);
            return Response::Nothing;
        }
        if part < MAX_COLUMNS && table.column_count > 1 {
            let grab = self.rect(id).map_or(0, |rect| x - table.column_x(rect, part));
            self.header_move = Some(HeaderMove {
                table: id,
                column: part,
                start_x: x,
                grab,
                started: false,
                scrolled_ms: None,
            });
        }
        if part < MAX_COLUMNS && table.columns[part].sortable {
            self.pressed = Some(Target { widget: id, part });
            self.pressed_inside = true;
            self.damage_widget(id);
        }
        Response::Nothing
    }

    /// A click on header `column` of table `id`: it sorts by that column, ascending first,
    /// then flipping.
    pub(super) fn header_clicked(&mut self, id: WidgetId, column: usize) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
        if column >= table.column_count || !table.columns[column].sortable {
            return Response::Nothing;
        }
        let order = match table.sort {
            Some((sorted, order)) if sorted == column => order.flipped(),
            _ => SortOrder::Ascending,
        };
        self.set_sort(id, Some((column, order)));
        Response::Sorted(id, column, order)
    }

    pub(super) fn column_moved(&mut self, drag: ColumnDrag, x: i32) -> Response {
        self.set_column_width(drag.table, drag.column, drag.start_width + x - drag.grab_x);
        Response::Nothing
    }

    /// Ends a column's resize: `cancel` puts its width back and says nothing, else
    /// [`Response::ColumnResized`] when the width changed.
    pub(super) fn end_column_drag(&mut self, cancel: bool) -> Response {
        let Some(drag) = self.column.take() else {
            return Response::Nothing;
        };
        self.damage_widget(drag.table);
        if cancel {
            self.set_column_width(drag.table, drag.column, drag.start_width);
            return Response::Nothing;
        }
        let width = self
            .table(drag.table)
            .map_or(drag.start_width, |table| table.columns[drag.column].width);
        match width == drag.start_width {
            true => Response::Nothing,
            false => Response::ColumnResized(drag.table, drag.column, width),
        }
    }

    pub(super) fn table_key(
        &mut self,
        id: WidgetId,
        table: &Table,
        code: u16,
        unused: Response,
    ) -> Response {
        let Some(rect) = self.rect(id) else {
            return unused;
        };
        let last = table.rows.saturating_sub(1);
        let shown = (table.view(rect).height + TABLE_ROW_HEIGHT - 1) / TABLE_ROW_HEIGHT;
        let page = (shown - 1).max(1) as usize;
        let here = table.selected;
        let held = self.held_keys;
        let mod_held = held.control || held.command;
        if table.selection.is_some() && held.command && !held.control
            && matches!(code, key::UP | key::DOWN)
        {
            return unused;
        }
        if table.selection.is_some() && mod_held && code == key::A {
            let change = match self.shift_held {
                true => SelectionChange::Clear,
                false => SelectionChange::All,
            };
            return self.selection_change(id, here, true, change);
        }
        if table.selection.is_some() && held.control && code == key::SPACE && table.rows > 0 {
            return self.selection_change(id, here, false, SelectionChange::Toggle(here));
        }
        if table.cell_focus && matches!(code, key::LEFT | key::RIGHT) {
            let last_column = table.column_count.saturating_sub(1);
            let column = match (code, mod_held) {
                (key::LEFT, true) => 0,
                (key::LEFT, false) => table.column.saturating_sub(1),
                (_, true) => last_column,
                _ => (table.column + 1).min(last_column),
            };
            if column == table.column {
                return Response::Nothing;
            }
            return self.focus_cell(id, column);
        }
        let next = match code {
            key::UP => here.saturating_sub(1),
            key::DOWN => (here + 1).min(last),
            key::HOME => 0,
            key::END => last,
            key::PAGE_UP => here.saturating_sub(page),
            key::PAGE_DOWN => (here + page).min(last),
            key::ENTER if table.rows > 0 => return Response::Opened(id, here),
            key::SPACE => return Response::Nothing,
            key::LEFT | key::RIGHT => {
                let step = match code {
                    key::LEFT => -TABLE_KEY_SCROLL,
                    _ => TABLE_KEY_SCROLL,
                };
                self.scroll_axis_to(id, true, table.offset_x + step);
                return Response::Nothing;
            }
            _ => return unused,
        };
        if table.rows == 0 {
            return Response::Nothing;
        }
        if table.selection.is_some() && held.control && matches!(code, key::UP | key::DOWN) {
            self.change_table(id, |table| table.selected = next);
            self.keep_row_visible(id);
            return Response::Chosen(id, next);
        }
        if table.selection.is_some() && self.shift_held {
            return self.selection_change(
                id,
                next,
                true,
                SelectionChange::Range(table.anchor, next),
            );
        }
        self.select_row(id, next)
    }
}

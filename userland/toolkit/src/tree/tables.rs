//! A table's rows, sorting and column resizing by pointer and keys (docs/ux/toolkit-v0.5.md
//! section 4.4), and the program's calls that change it.

use super::{Kind, Response, Target, WidgetId, WidgetTree};
use crate::input::key;
use crate::table::{HANDLE_PART, ROW_HIT, Table};
use crate::widget::{MAX_COLUMNS, SortOrder, TABLE_KEY_SCROLL, TABLE_ROW_HEIGHT};

#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnDrag {
    pub(super) table: WidgetId,
    pub(super) column: usize,
    start_width: i32,
    grab_x: i32,
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
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

    pub(super) fn select_row(&mut self, id: WidgetId, row: usize) -> Response {
        let Some(table) = self.table(id) else {
            return Response::Nothing;
        };
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
            self.column = Some(ColumnDrag {
                table: id,
                column,
                start_width: table.columns[column].width,
                grab_x: x,
            });
            self.damage_widget(id);
            return Response::Nothing;
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
        self.select_row(id, next)
    }
}

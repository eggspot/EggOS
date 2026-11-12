//! An outline's rows by pointer and keys (docs/ux/windows-registry-v0.3.md section 4, D401),
//! and the program's calls that change it.

use super::{Kind, Response, WidgetId, WidgetTree};
use crate::outline::{GLYPH_HIT, Outline, ROW_HIT, RowPath, Step};
use crate::widget::FormFactor;

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    pub(super) fn outline(&self, id: WidgetId) -> Option<Outline> {
        match self.kind(id)? {
            Kind::Outline(outline) => Some(*outline),
            _ => None,
        }
    }

    pub(super) fn change_outline(&mut self, id: WidgetId, change: impl FnOnce(&mut Outline)) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let rect = node.rect;
        if let Kind::Outline(outline) = &mut node.kind {
            change(outline);
            outline.clamp_offsets(rect);
        }
        self.damage_widget(id);
    }

    /// Makes `path` what outline `id` asks for its rows' values for agents; without a
    /// response.
    pub fn set_row_path(&mut self, id: WidgetId, path: RowPath) {
        self.change_outline(id, |outline| outline.path = Some(path));
    }

    /// Makes outline `id` browse one level a page (`true`), as a phone does, or show its
    /// levels; without a response.
    pub fn set_drill_down(&mut self, id: WidgetId, on: bool) {
        self.change_outline(id, |outline| outline.drill_down = on);
    }

    pub(super) fn select_outline_row(&mut self, id: WidgetId, row: usize) -> Response {
        let Some(outline) = self.outline(id) else {
            return Response::Nothing;
        };
        if row >= outline.rows || row == outline.selected {
            self.keep_outline_row_visible(id);
            return Response::Nothing;
        }
        self.change_outline(id, |outline| outline.selected = row);
        self.keep_outline_row_visible(id);
        Response::Chosen(id, row)
    }

    pub(super) fn keep_outline_row_visible(&mut self, id: WidgetId) {
        let (Some(outline), Some(rect)) = (self.outline(id), self.rect(id)) else {
            return;
        };
        let offset = outline.offset_showing(rect, outline.selected);
        self.scroll_axis_to(id, false, offset);
    }

    /// Asks the program to open or close row `row` of outline `id`: it changes its rows and
    /// calls `set_rows`.
    pub(super) fn toggle_outline_row(&mut self, id: WidgetId, row: usize) -> Response {
        let Some(outline) = self.outline(id) else {
            return Response::Nothing;
        };
        if row >= outline.rows {
            return Response::Nothing;
        }
        let shown = outline.row_of(row);
        if outline.drill_down {
            self.damage_widget(id);
            return Response::RowToggled(id, row, true);
        }
        if !shown.expandable {
            return Response::Nothing;
        }
        self.damage_widget(id);
        Response::RowToggled(id, row, !shown.expanded)
    }

    pub(super) fn outline_pressed(&mut self, id: WidgetId, part: usize, double: bool) -> Response {
        let Some(outline) = self.outline(id) else {
            return Response::Nothing;
        };
        if part >= GLYPH_HIT {
            return self.toggle_outline_row(id, part - GLYPH_HIT);
        }
        if part >= ROW_HIT {
            let row = part - ROW_HIT;
            if outline.drill_down {
                self.select_outline_row(id, row);
                return self.toggle_outline_row(id, row);
            }
            if double && row == outline.selected {
                return self.toggle_outline_row(id, row);
            }
            return self.select_outline_row(id, row);
        }
        Response::Nothing
    }

    pub(super) fn outline_key(
        &mut self,
        id: WidgetId,
        outline: &Outline,
        code: u16,
        character: Option<char>,
        unused: Response,
    ) -> Response {
        let Some(rect) = self.rect(id) else {
            return unused;
        };
        if let Some(typed) = character.filter(|typed| !typed.is_control() && *typed != ' ') {
            return match outline.row_starting_with(typed) {
                Some(row) => self.select_outline_row(id, row),
                None => Response::Nothing,
            };
        }
        let tv = self.form == FormFactor::Tv;
        match outline.step_for(code, outline.page(rect), tv) {
            Step::Select(row) => self.select_outline_row(id, row),
            Step::Toggle(row) => self.toggle_outline_row(id, row),
            Step::Stay => Response::Nothing,
            Step::Leave | Step::Unused => unused,
        }
    }
}

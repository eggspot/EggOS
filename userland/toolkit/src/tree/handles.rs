//! The touch handles of a text field or a text area (docs/ux/on-screen-keyboard-v0.5.md section
//! 7, D317, D329): in the touch form, a tap in one shows a caret handle under the caret, which a
//! drag moves; a long press on a word selects it and shows a handle at each end, each of which
//! a drag moves, across a text area's lines too. A typed key, or a tap elsewhere, hides them. A
//! masked field shows the caret handle and selects no word.

use super::text_areas::word_around;
use super::{Kind, WidgetId, WidgetTree};
use crate::canvas::Canvas;
use crate::geometry::Rect;
use crate::text;
use crate::text_area;
use crate::widget::{self, FormFactor, LINE_HEIGHT, TEXT_AREA_LINE_PITCH};

const LONG_PRESS_MS: u32 = 500;
const REST_SLOP: i32 = 8;
const HANDLE_WIDTH: i32 = 12;
const HANDLE_POINT_ROWS: i32 = 6;
const HANDLE_HEIGHT: i32 = 14;
const HANDLE_TARGET: i32 = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Caret,
    Anchor,
}

/// The handles a field shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Handles {
    field: WidgetId,
    selection: bool,
    /// The end being dragged, and where the finger grabbed it from its point.
    dragging: Option<(End, i32, i32)>,
    rest: Option<(i32, u32)>,
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    fn touch_form(&self) -> bool {
        self.form == FormFactor::Phone
    }

    fn handle_point(&self, field: WidgetId, end: End) -> Option<(i32, i32)> {
        if let (Some(Kind::TextArea(area)), Some(rect)) = (self.kind(field), self.rect(field)) {
            let store = self.stores.get(area.store)?;
            let at = match end {
                End::Caret => area.caret,
                End::Anchor => area.anchor,
            };
            let inner = text_area::inner(rect);
            let line = store.line_of(at);
            let x = inner.x + store.x_of(line, at);
            let bottom = inner.y + line as i32 * TEXT_AREA_LINE_PITCH - area.scroll + LINE_HEIGHT;
            return Some((x, bottom));
        }
        let (Some(Kind::TextField(edited)), Some(rect)) = (self.kind(field), self.rect(field))
        else {
            return None;
        };
        let inner = widget::field_inner(rect);
        let at = match end {
            End::Caret => edited.caret,
            End::Anchor => edited.anchor,
        };
        let text = edited.text.as_str();
        let x = inner.x - edited.scroll + text::width(&text[..at.min(text.len())]);
        let bottom = rect.y + (rect.height + LINE_HEIGHT) / 2;
        Some((x, bottom))
    }

    fn handle_rect(&self, field: WidgetId, end: End) -> Option<Rect> {
        let (x, bottom) = self.handle_point(field, end)?;
        Some(Rect::new(
            x - HANDLE_WIDTH / 2,
            bottom,
            HANDLE_WIDTH,
            HANDLE_HEIGHT,
        ))
    }

    fn damage_handles(&mut self) {
        let Some(handles) = self.handles else {
            return;
        };
        for end in [End::Caret, End::Anchor] {
            if let Some(rect) = self.handle_rect(handles.field, end) {
                self.damage.add(rect.inset(-1));
            }
        }
    }

    /// A press: grabs a shown handle when it lands in its target; `true` then.
    pub(super) fn handles_pressed(&mut self, x: i32, y: i32) -> bool {
        let Some(handles) = self.handles else {
            return false;
        };
        let ends: &[End] = match handles.selection {
            true => &[End::Caret, End::Anchor],
            false => &[End::Caret],
        };
        for &end in ends {
            let Some((handle_x, bottom)) = self.handle_point(handles.field, end) else {
                continue;
            };
            let target = Rect::new(
                handle_x - HANDLE_TARGET / 2,
                bottom + HANDLE_HEIGHT / 2 - HANDLE_TARGET / 2,
                HANDLE_TARGET,
                HANDLE_TARGET,
            );
            if target.contains(x, y) {
                self.handles = Some(Handles {
                    dragging: Some((end, x - handle_x, y - bottom)),
                    ..handles
                });
                return true;
            }
        }
        self.damage_handles();
        self.handles = None;
        false
    }

    /// A tap that placed the caret in field or text area `id`: the caret handle shows there,
    /// and a press that rests becomes a word selection.
    pub(super) fn field_tapped(&mut self, id: WidgetId, x: i32, time_ms: u32) {
        if !self.touch_form() {
            return;
        }
        self.damage_handles();
        self.handles = Some(Handles {
            field: id,
            selection: false,
            dragging: None,
            rest: Some((x, time_ms)),
        });
        self.damage_handles();
    }

    /// The finger moved to (`x`, `y`): a grabbed handle follows it, by the offset it was
    /// grabbed at, along a field's line or across a text area's lines; a resting press that
    /// moves is no long press. `true` when a handle took the move.
    pub(super) fn handles_moved(&mut self, x: i32, y: i32, time_ms: u32) -> bool {
        let Some(mut handles) = self.handles else {
            return false;
        };
        if let Some((rest_x, _)) = handles.rest
            && (x - rest_x).abs() > REST_SLOP
        {
            handles.rest = None;
            self.handles = Some(handles);
        }
        let Some((end, grab_x, grab_y)) = handles.dragging else {
            return false;
        };
        if let Some(Kind::TextArea(_)) = self.kind(handles.field) {
            let line_y = y - grab_y - LINE_HEIGHT / 2;
            let Some((_, at)) = self.area_point(handles.field, x - grab_x, line_y) else {
                return false;
            };
            self.damage_handles();
            self.edit_area(handles.field, time_ms, |area, _| {
                match end {
                    End::Caret => area.caret = at,
                    End::Anchor => area.anchor = at,
                }
                area.wanted_x = None;
                false
            });
            self.damage_handles();
            return true;
        }
        let grab = grab_x;
        let (Some(Kind::TextField(field)), Some(rect)) =
            (self.kind(handles.field).copied(), self.rect(handles.field))
        else {
            return false;
        };
        let at = widget::caret_at(&field, rect, x - grab);
        self.damage_handles();
        self.edit_field(handles.field, time_ms, |edited| {
            match end {
                End::Caret => edited.caret = at,
                End::Anchor => edited.anchor = at,
            }
            false
        });
        self.damage_handles();
        true
    }

    /// The finger lifted: a dragged handle stays where it was left. The caret and the anchor
    /// swap when the caret's handle passed the anchor's, so the selection reads forwards.
    pub(super) fn handles_released(&mut self) {
        if let Some(handles) = &mut self.handles {
            handles.dragging = None;
            handles.rest = None;
        }
    }

    /// The time moved on: a press resting [`LONG_PRESS_MS`] on a field or a text area selects
    /// the word under it (not in a masked field) and shows both handles.
    pub(super) fn handles_tick(&mut self, now_ms: u32) {
        let Some(handles) = self.handles else {
            return;
        };
        let Some((_, since)) = handles.rest else {
            return;
        };
        if now_ms.wrapping_sub(since) < LONG_PRESS_MS {
            return;
        }
        if let Some(Kind::TextArea(_)) = self.kind(handles.field) {
            self.damage_handles();
            self.edit_area(handles.field, now_ms, |area, store| {
                (area.anchor, area.caret) = word_around(store.text(), area.caret);
                false
            });
            self.selecting = None;
            self.handles = Some(Handles {
                selection: true,
                rest: None,
                ..handles
            });
            self.damage_handles();
            return;
        }
        let Some(Kind::TextField(field)) = self.kind(handles.field).copied() else {
            return;
        };
        self.handles = Some(Handles {
            rest: None,
            ..handles
        });
        if field.masked {
            return;
        }
        self.damage_handles();
        self.edit_field(handles.field, now_ms, |edited| {
            (edited.anchor, edited.caret) = edited.word_at(edited.caret);
            false
        });
        self.selecting = None;
        self.handles = Some(Handles {
            selection: true,
            rest: None,
            ..handles
        });
        self.damage_handles();
    }

    /// When a resting press becomes a long press, if one rests.
    pub(super) fn handles_deadline(&self) -> Option<u32> {
        let (_, since) = self.handles?.rest?;
        Some(since.wrapping_add(LONG_PRESS_MS))
    }

    /// A key, or the field losing the focus: the handles go.
    pub(super) fn hide_handles(&mut self) {
        self.damage_handles();
        self.handles = None;
    }

    /// Draws the shown handles in `accent`, clipped to `clip`.
    pub(super) fn paint_handles(&self, canvas: &mut Canvas<'_>, clip: Rect) {
        let Some(handles) = self.handles else {
            return;
        };
        if self.focus != Some(handles.field) {
            return;
        }
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let accent = canvas.palette().accent;
        let ends: &[End] = match handles.selection {
            true => &[End::Caret, End::Anchor],
            false => &[End::Caret],
        };
        for &end in ends {
            let Some((x, bottom)) = self.handle_point(handles.field, end) else {
                continue;
            };
            let previous = canvas.narrow(window.intersection(&clip));
            for row in 0..HANDLE_POINT_ROWS {
                let half = row + 1;
                canvas.fill(Rect::new(x - half, bottom + row, 2 * half, 1), accent);
            }
            canvas.fill(
                Rect::new(
                    x - HANDLE_WIDTH / 2,
                    bottom + HANDLE_POINT_ROWS,
                    HANDLE_WIDTH,
                    HANDLE_HEIGHT - HANDLE_POINT_ROWS,
                ),
                accent,
            );
            canvas.restore(previous);
        }
    }
}

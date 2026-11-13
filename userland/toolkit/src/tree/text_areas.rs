//! Editing a text area: the caret and the selection by keys and pointer, typing, the
//! clipboard and the wrap pass after each edit (docs/ux/toolkit-v0.5.md section 5.4).

use super::{Kind, Response, WidgetId, WidgetTree};
use crate::input::key;
use crate::text::Text;
use crate::text_area::{self, Store, TextArea};
use crate::widget::TEXT_AREA_LINE_PITCH;

/// The word of `text` around byte `at`, as a field's double click finds it, within the line
/// and 255 bytes around `at`: its start and end.
pub(super) fn word_around(text: &str, at: usize) -> (usize, usize) {
    let mut field = crate::widget::Field {
        text: Text::new(),
        caret: 0,
        anchor: 0,
        scroll: 0,
        placeholder: "",
        escape: crate::widget::Escape::Clear,
        masked: false,
    };
    let line_start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |index| at + index);
    let mut start = at.saturating_sub(127).max(line_start);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut stop = (start + 255).min(line_end);
    while !text.is_char_boundary(stop) {
        stop -= 1;
    }
    field.text.set(&text[start..stop]);
    let (word_start, word_end) = field.word_at(at.saturating_sub(start));
    (start + word_start, start + word_end)
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    /// Applies `edit` to text area `id` and its store; when it says the text changed, the
    /// lines are wrapped again from the edit's first byte, `from`. Then the caret is kept in
    /// view and blinks again. Whether the text changed.
    pub(super) fn edit_area(
        &mut self,
        id: WidgetId,
        now_ms: u32,
        edit: impl FnOnce(&mut TextArea, &mut Store) -> bool,
    ) -> bool {
        let Some(node) = self.nodes.get_mut(id.index()).and_then(Option::as_mut) else {
            return false;
        };
        let rect = node.rect;
        let Kind::TextArea(area) = &mut node.kind else {
            return false;
        };
        let Some(store) = self.stores.get_mut(area.store) else {
            return false;
        };
        let from = area.caret.min(area.anchor);
        let changed = edit(area, store);
        if changed {
            store.wrap(from.min(area.caret), text_area::inner(rect).width);
            area.lines = store.line_count();
            area.wanted_x = None;
        }
        area.caret = area.caret.min(store.len());
        area.anchor = area.anchor.min(store.len());
        self.damage_widget(id);
        self.keep_area_caret_visible(id);
        self.restart_caret(now_ms);
        changed
    }

    /// Wraps text area `id`'s whole text again for its width.
    pub(super) fn rewrap(&mut self, id: WidgetId) {
        let Some(node) = self.nodes.get_mut(id.index()).and_then(Option::as_mut) else {
            return;
        };
        let rect = node.rect;
        if let Kind::TextArea(area) = &mut node.kind
            && let Some(store) = self.stores.get_mut(area.store)
        {
            store.wrap_all(text_area::inner(rect).width);
            area.lines = store.line_count();
        }
        self.damage_widget(id);
        self.keep_area_caret_visible(id);
    }

    fn keep_area_caret_visible(&mut self, id: WidgetId) {
        let Some(node) = self.nodes.get_mut(id.index()).and_then(Option::as_mut) else {
            return;
        };
        let rect = node.rect;
        let Kind::TextArea(area) = &mut node.kind else {
            return;
        };
        let Some(store) = self.stores.get(area.store) else {
            return;
        };
        let line = store.line_of(area.caret) as i32;
        let shown = text_area::visible_lines(rect) as i32;
        let first = area.scroll / TEXT_AREA_LINE_PITCH;
        let first = match () {
            () if line < first => line,
            () if line >= first + shown => line - shown + 1,
            () => first,
        };
        let most = (store.line_count() as i32 - shown).max(0);
        area.scroll = first.clamp(0, most) * TEXT_AREA_LINE_PITCH;
    }

    pub(super) fn area_point(&self, id: WidgetId, x: i32, y: i32) -> Option<(usize, usize)> {
        let rect = self.rect(id)?;
        let Some(Kind::TextArea(area)) = self.kind(id) else {
            return None;
        };
        let store = self.stores.get(area.store)?;
        let inner = text_area::inner(rect);
        let line = ((y - inner.y + area.scroll).max(0) / TEXT_AREA_LINE_PITCH) as usize;
        let line = line.min(store.line_count() - 1);
        Some((line, store.byte_at_x(line, x - inner.x)))
    }

    pub(super) fn area_pressed(&mut self, id: WidgetId, x: i32, y: i32, now_ms: u32) {
        let Some((_, at)) = self.area_point(id, x, y) else {
            return;
        };
        let clicks = self.clicks;
        let extend = self.shift_held;
        self.edit_area(id, now_ms, |area, store| {
            let text = store.text();
            match clicks {
                2 => (area.anchor, area.caret) = word_around(text, at),
                3 => {
                    area.anchor = text[..at].rfind('\n').map_or(0, |index| index + 1);
                    area.caret = text[at..].find('\n').map_or(text.len(), |index| at + index);
                }
                _ if extend => area.caret = at,
                _ => {
                    area.caret = at;
                    area.anchor = at;
                }
            }
            area.wanted_x = None;
            false
        });
    }

    pub(super) fn area_select_to(&mut self, id: WidgetId, x: i32, y: i32, now_ms: u32) {
        let (Some(rect), Some(Kind::TextArea(area))) = (self.rect(id), self.kind(id).copied())
        else {
            return;
        };
        let inner = text_area::inner(rect);
        let Some(store) = self.stores.get(area.store) else {
            return;
        };
        let here = store.line_of(area.caret);
        let caret = match () {
            () if y < inner.y => {
                let line = here.saturating_sub(1);
                store.byte_at_x(line, x - inner.x)
            }
            () if y >= inner.bottom() => {
                let line = (here + 1).min(store.line_count() - 1);
                store.byte_at_x(line, x - inner.x)
            }
            () => match self.area_point(id, x, y) {
                Some((_, at)) => at,
                None => return,
            },
        };
        if caret != area.caret {
            self.edit_area(id, now_ms, |area, _| {
                area.caret = caret;
                false
            });
        }
    }

    fn vertical_move(store: &Store, area: &mut TextArea, lines: isize) {
        let here = store.line_of(area.caret);
        let x = area
            .wanted_x
            .unwrap_or_else(|| store.x_of(here, area.caret));
        area.wanted_x = Some(x);
        let target = here as isize + lines;
        area.caret = match () {
            () if target < 0 => 0,
            () if target >= store.line_count() as isize => store.len(),
            () => store.byte_at_x(target as usize, x),
        };
    }

    fn replace_selection(area: &mut TextArea, store: &mut Store, words: &str) -> bool {
        let (start, end) = area.selection();
        let mut changed = start != end;
        store.remove(start, end);
        area.caret = start;
        for character in words.chars() {
            let character = match character {
                '\r' => continue,
                '\n' | '\t' => character,
                character if character.is_control() => continue,
                character => character,
            };
            if store.line_count() >= crate::widget::TEXT_AREA_LINES {
                break;
            }
            let mut bytes = [0; 4];
            let encoded = character.encode_utf8(&mut bytes);
            if store.insert(area.caret, encoded) < encoded.len() {
                break;
            }
            area.caret += encoded.len();
            changed = true;
        }
        area.anchor = area.caret;
        changed
    }

    /// Puts `text` in text area `id` in place of its selection, as [`Response::PasteWanted`]
    /// asked.
    pub(super) fn area_paste(&mut self, id: WidgetId, text: &str, now_ms: u32) -> Response {
        let changed = self.edit_area(id, now_ms, |area, store| {
            Self::replace_selection(area, store, text)
        });
        match changed {
            true => Response::Edited(id),
            false => Response::Nothing,
        }
    }

    pub(super) fn area_key(
        &mut self,
        id: WidgetId,
        code: u16,
        character: u32,
        command: bool,
        now_ms: u32,
        unused: Response,
    ) -> Response {
        let shortcut = self.shortcut_held;
        let extend = self.shift_held;
        let Some(rect) = self.rect(id) else {
            return unused;
        };
        let page = (text_area::visible_lines(rect) as isize - 1).max(1);
        let Some(Kind::TextArea(area)) = self.kind(id).copied() else {
            return unused;
        };
        let (start, end) = area.selection();
        if shortcut {
            match code {
                key::A => {
                    self.edit_area(id, now_ms, |area, store| {
                        area.anchor = 0;
                        area.caret = store.len();
                        false
                    });
                    return Response::Nothing;
                }
                key::C | key::X if start != end => {
                    let copied = self
                        .stores
                        .get(area.store)
                        .and_then(|store| store.text().get(start..end))
                        .unwrap_or("");
                    self.copied = Text::from(copied);
                    if code == key::X {
                        self.edit_area(id, now_ms, |area, store| {
                            Self::replace_selection(area, store, "")
                        });
                        return Response::Cut(id);
                    }
                    return Response::Copied(id);
                }
                key::C | key::X => return Response::Nothing,
                key::V => return Response::PasteWanted(id),
                key::ENTER => return unused,
                _ => {}
            }
        }
        let moves = matches!(
            code,
            key::LEFT
                | key::RIGHT
                | key::UP
                | key::DOWN
                | key::HOME
                | key::END
                | key::PAGE_UP
                | key::PAGE_DOWN
        );
        if moves {
            self.edit_area(id, now_ms, |area, store| {
                let collapse = start != end && !extend;
                match code {
                    key::LEFT if collapse => area.caret = start,
                    key::RIGHT if collapse => area.caret = end,
                    key::LEFT => area.caret = store.previous_boundary(area.caret),
                    key::RIGHT => area.caret = store.next_boundary(area.caret),
                    key::UP => Self::vertical_move(store, area, -1),
                    key::DOWN => Self::vertical_move(store, area, 1),
                    key::PAGE_UP => Self::vertical_move(store, area, -page),
                    key::PAGE_DOWN => Self::vertical_move(store, area, page),
                    key::HOME if shortcut => area.caret = 0,
                    key::END if shortcut => area.caret = store.len(),
                    key::HOME => area.caret = store.line_range(store.line_of(area.caret)).0,
                    _ => area.caret = store.end_of_line(store.line_of(area.caret)),
                }
                if !matches!(code, key::UP | key::DOWN | key::PAGE_UP | key::PAGE_DOWN) {
                    area.wanted_x = None;
                }
                if !extend {
                    area.anchor = area.caret;
                }
                false
            });
            return Response::Nothing;
        }
        let edited = match code {
            key::ENTER => self.edit_area(id, now_ms, |area, store| {
                Self::replace_selection(area, store, "\n")
            }),
            key::BACKSPACE | key::DELETE => self.edit_area(id, now_ms, |area, store| {
                if area.caret == area.anchor {
                    area.anchor = match code {
                        key::BACKSPACE => store.previous_boundary(area.caret),
                        _ => store.next_boundary(area.caret),
                    };
                }
                Self::replace_selection(area, store, "")
            }),
            key::TAB | key::ESC => return unused,
            _ => {
                let typed = char::from_u32(character)
                    .filter(|typed| !command && eggos_font::egg_sans::covers(*typed));
                let Some(typed) = typed else {
                    return unused;
                };
                let mut bytes = [0; 4];
                let typed = typed.encode_utf8(&mut bytes);
                self.edit_area(id, now_ms, |area, store| {
                    Self::replace_selection(area, store, typed)
                })
            }
        };
        match edited {
            true => Response::Edited(id),
            false => Response::Nothing,
        }
    }
}

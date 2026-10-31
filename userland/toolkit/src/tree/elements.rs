//! A widget tree as an element tree for an agent (D161, docs/windows.md "Element trees"): each
//! shown widget is one element, and each option, card and shown list row one more below it.
//! Widget `i` is element `(i + 1) * 100`, its part `p` element `(i + 1) * 100 + p + 1`, so an id
//! stays the same while the window keeps its widgets. Acting on an element does what a click
//! or the keyboard does to it, and answers with the same [`Response`] a person's input gives.

use super::{ROOT_ELEMENT, Response, WidgetId, WidgetTree};
use crate::geometry::Rect;
use crate::widget::{Kind, card_rect, radio_rect};
use eggos_element_tree::{Action, Bounds, Element, Outcome, Role, TreeWriter, flags};

const PER_WIDGET: u32 = 100;

fn bounds(rect: Rect) -> Bounds {
    Bounds::new(rect.x, rect.y, rect.width, rect.height)
}

fn decimal(value: u32, digits: &mut [u8; 10]) -> &[u8] {
    let mut at = digits.len();
    let mut rest = value;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            return &digits[at..];
        }
    }
}

/// The element id of widget `id`.
pub fn element_of(id: WidgetId) -> u32 {
    (id.index() as u32 + 1) * PER_WIDGET
}

fn part_element(id: WidgetId, part: usize) -> u32 {
    element_of(id) + part as u32 + 1
}

impl<const N: usize> WidgetTree<N> {
    fn widget_of(&self, element: u32) -> Option<(WidgetId, Option<usize>)> {
        let index = (element / PER_WIDGET).checked_sub(1)? as usize;
        if index >= self.count {
            return None;
        }
        let part = (element % PER_WIDGET)
            .checked_sub(1)
            .map(|part| part as usize);
        Some((WidgetId(index as u16), part))
    }

    fn flags_of(&self, id: WidgetId) -> u16 {
        let mut flags = 0;
        if self.focusable(id) {
            flags |= flags::FOCUSABLE;
        }
        if self.focus == Some(id) && self.window_focused {
            flags |= flags::FOCUSED;
        }
        if self.node(id).is_some_and(|node| !node.enabled) {
            flags |= flags::DISABLED;
        }
        if self.is_person_only(id) {
            flags |= flags::PERSON_ONLY;
        }
        flags
    }

    /// Writes every shown widget, with its options, cards and shown rows, below element
    /// `ROOT_ELEMENT`; stops when `tree` is full.
    pub fn describe(&self, tree: &mut TreeWriter<'_>) {
        for id in self.ids() {
            let Some(node) = self.node(id) else {
                continue;
            };
            if !self.is_shown(id) {
                continue;
            }
            let parent = node.parent.map_or(ROOT_ELEMENT, element_of);
            let here = Element::new(element_of(id), parent, Role::Group, bounds(node.rect))
                .flagged(self.flags_of(id));
            let mut digits = [0u8; 10];
            let mut range = crate::text::Text::<48>::new();
            let element = match &node.kind {
                Kind::Panel { .. } | Kind::Column { .. } | Kind::ScrollView { .. } => here,
                Kind::Label { text, .. } => Element {
                    role: Role::Label,
                    ..here.named(text.as_str().as_bytes())
                },
                Kind::Button { text, primary } => {
                    let default = *primary || self.default_button == Some(id);
                    Element {
                        role: Role::Button,
                        ..here.named(text.as_str().as_bytes())
                    }
                    .flagged(if default { flags::DEFAULT } else { 0 })
                }
                Kind::Checkbox { text, checked } => Element {
                    role: Role::Checkbox,
                    ..here.named(text.as_str().as_bytes())
                }
                .flagged(if *checked { flags::CHECKED } else { 0 }),
                Kind::Radios { .. } | Kind::Cards { .. } => Element {
                    role: Role::RadioGroup,
                    ..here
                },
                Kind::List(_) => Element {
                    role: Role::List,
                    ..here
                },
                Kind::TextField(field) => Element {
                    role: Role::TextField,
                    ..here
                        .named(field.placeholder.as_bytes())
                        .valued(field.text.as_str().as_bytes())
                },
                Kind::Progress { permille } => Element {
                    role: Role::Progress,
                    ..here.valued(decimal(u32::from(*permille), &mut digits))
                },
                Kind::Slider(slider) => {
                    let _ = core::fmt::Write::write_fmt(
                        &mut range,
                        format_args!(
                            "{} {}..{} by {}",
                            slider.value, slider.min, slider.max, slider.step
                        ),
                    );
                    Element {
                        role: Role::Slider,
                        ..here
                            .named(slider.name.as_bytes())
                            .valued(range.as_str().as_bytes())
                    }
                }
                Kind::Custom { .. } => Element {
                    role: Role::Graphic,
                    ..here
                },
            };
            if !tree.push(&element) || !self.describe_parts(id, tree) {
                return;
            }
        }
    }

    fn describe_parts(&self, id: WidgetId, tree: &mut TreeWriter<'_>) -> bool {
        let Some(node) = self.node(id) else {
            return true;
        };
        let disabled = match (node.enabled, node.person_only) {
            (true, false) => 0,
            (false, false) => flags::DISABLED,
            (true, true) => flags::PERSON_ONLY,
            (false, true) => flags::DISABLED | flags::PERSON_ONLY,
        };
        let part = |part: usize, role: Role, rect: Rect, name: &'static str, selected: bool| {
            Element::new(part_element(id, part), element_of(id), role, bounds(rect))
                .named(name.as_bytes())
                .flagged(disabled | if selected { flags::SELECTED } else { 0 })
        };
        match &node.kind {
            Kind::Radios {
                options,
                count,
                selected,
                ..
            } => (0..*count).all(|index| {
                let rect = radio_rect(&node.kind, node.rect, index);
                tree.push(&part(
                    index,
                    Role::Radio,
                    rect,
                    options[index],
                    index == *selected,
                ))
            }),
            Kind::Cards {
                labels,
                count,
                selected,
                ..
            } => (0..*count).all(|index| {
                let rect = card_rect(&node.kind, node.rect, index);
                tree.push(&part(
                    index,
                    Role::Radio,
                    rect,
                    labels[index],
                    index == *selected,
                ))
            }),
            Kind::List(list) => list.shown_items().enumerate().all(|(row, item)| {
                let rect = Rect::new(
                    node.rect.x,
                    node.rect.y + row as i32 * list.row_height - list.offset,
                    node.rect.width,
                    list.row_height,
                );
                let name = list.items[item];
                tree.push(&part(
                    item,
                    Role::ListItem,
                    rect,
                    name,
                    item == list.selected,
                ))
            }),
            _ => true,
        }
    }

    /// Does `action` to `element` for an agent, as a person's click or keys would, with `text`
    /// for [`Action::SetText`]; returns the outcome and the response the program acts on.
    pub fn act(
        &mut self,
        element: u32,
        action: Action,
        text: &str,
        now_ms: u32,
    ) -> (Outcome, Response) {
        let Some((id, part)) = self.widget_of(element) else {
            return (Outcome::NoSuchElement, Response::Nothing);
        };
        let Some(node) = self.node(id) else {
            return (Outcome::NoSuchElement, Response::Nothing);
        };
        let kind = node.kind;
        let parts = match &kind {
            Kind::Radios { count, .. } | Kind::Cards { count, .. } => *count,
            Kind::List(list) => list.items.len(),
            _ => 0,
        };
        let shown_part = match (&kind, part) {
            (_, None) => true,
            (Kind::List(list), Some(item)) => list.row_of(item).is_some(),
            (_, Some(part)) => part < parts,
        };
        if !self.is_shown(id) || !shown_part {
            return (Outcome::NoSuchElement, Response::Nothing);
        }
        if node.person_only {
            return (Outcome::NotSupported, Response::Nothing);
        }
        if !node.enabled {
            return (Outcome::Disabled, Response::Nothing);
        }
        match (action, &kind, part) {
            (Action::Focus, _, None) if self.focusable(id) => {
                self.set_focus(Some(id), now_ms);
                (Outcome::Done, Response::Nothing)
            }
            (Action::Invoke, Kind::Button { .. }, None) => (Outcome::Done, Response::Activated(id)),
            (Action::Invoke | Action::Select, Kind::Checkbox { .. }, None) => {
                (Outcome::Done, self.activate(id, 0))
            }
            (
                Action::Invoke | Action::Select,
                Kind::Radios { .. } | Kind::Cards { .. },
                Some(p),
            ) => (Outcome::Done, self.activate(id, p)),
            (Action::Select, Kind::List(list), Some(item)) => {
                if list.selected == item {
                    return (Outcome::Done, Response::Nothing);
                }
                self.set_selected(id, item);
                (Outcome::Done, Response::Chosen(id, item))
            }
            (Action::Invoke, Kind::List(_), Some(item)) => {
                self.set_selected(id, item);
                (Outcome::Done, Response::Opened(id, item))
            }
            (Action::SetText, Kind::Slider(_), None) => match text.trim().parse::<i32>() {
                Ok(wanted) => (Outcome::Done, self.adjust(id, wanted)),
                Err(_) => (Outcome::NotSupported, Response::Nothing),
            },
            (Action::SetText, Kind::TextField(_), None) => {
                self.set_text(id, text);
                (Outcome::Done, Response::Edited(id))
            }
            _ => (Outcome::NotSupported, Response::Nothing),
        }
    }
}

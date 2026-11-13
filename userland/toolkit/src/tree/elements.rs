//! A widget tree as an element tree for an agent (D161, docs/windows.md "Element trees"): each
//! shown widget is one element, and each option, card and shown list row one more below it.
//! Widget `i` is element `(i + 1) * 100`, its part `p` element `(i + 1) * 100 + p + 1`, so an id
//! stays the same while the window keeps its widgets. Acting on an element does what a click
//! or the keyboard does to it, and answers with the same [`Response`] a person's input gives.

use super::{ROOT_ELEMENT, Response, WidgetId, WidgetTree};
use crate::geometry::Rect;
use crate::menu::{Entry, Menu, plain_label};
use crate::table::ROW_PART;
use crate::text::Text;
use crate::widget::{Kind, MAX_COLUMNS, MAX_MENUS, card_rect, radio_rect};
use eggos_element_tree::{Action, Bounds, Element, Outcome, Role, TreeWriter, flags};

const PER_WIDGET: u32 = 100;
const LAST_PART: usize = PER_WIDGET as usize - 2;
const MENU_ENTRY_PART: usize = MAX_MENUS;
const MENU_ENTRIES: usize = crate::menu::MAX_ENTRIES;
const VALUE_BYTES: usize = eggos_element_tree::MAX_LABEL_BYTES;
const CUT_VALUE: usize = 252;

fn bar_entry_part(title: usize, entry: usize) -> usize {
    MENU_ENTRY_PART + title * MENU_ENTRIES + entry
}

fn bar_entry_of(part: usize) -> Option<(usize, usize)> {
    let entry = part.checked_sub(MENU_ENTRY_PART)?;
    Some((entry / MENU_ENTRIES, entry % MENU_ENTRIES))
}

fn cut_value(text: &str) -> Text<VALUE_BYTES> {
    if text.len() <= VALUE_BYTES {
        return Text::from(text);
    }
    let mut end = CUT_VALUE;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = Text::from(&text[..end]);
    value.push_str("...");
    value
}

fn describe_entries(
    menu: &Menu,
    parent: u32,
    element: impl Fn(usize) -> Option<u32>,
    disabled: u16,
    tree: &mut TreeWriter<'_>,
) -> bool {
    menu.entries().iter().enumerate().all(|(index, entry)| {
        let Entry::Item {
            label,
            key,
            enabled,
            ..
        } = entry
        else {
            return true;
        };
        let Some(id) = element(index) else {
            return true;
        };
        let name = plain_label(label);
        let flags = disabled | if *enabled { 0 } else { flags::DISABLED };
        tree.push(
            &Element::new(id, parent, Role::MenuItem, bounds(menu.entry_rect(index)))
                .named(name.as_str().as_bytes())
                .valued(key.as_bytes())
                .flagged(flags),
        )
    })
}

fn row_value<const N: usize>(table: &crate::table::Table, row: usize, out: &mut Text<N>) {
    for column in 0..table.column_count {
        if column > 0 {
            out.push_str("\t");
        }
        out.push_str(table.cell_text(row, column).as_str());
    }
}

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

fn names_a_control(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Button { .. } | Kind::MenuButton(_) | Kind::Checkbox { .. }
    )
}

fn for_agents(person_only: bool, text: &[u8]) -> &[u8] {
    match person_only {
        true => b"",
        false => text,
    }
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
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
            let mut value = Text::<VALUE_BYTES>::new();
            let element = match &node.kind {
                Kind::Panel { .. } | Kind::Column { .. } | Kind::ScrollView { .. } => here,
                Kind::Label { text, .. } => Element {
                    role: Role::Label,
                    ..here.named(for_agents(node.person_only, text.as_str().as_bytes()))
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
                        .valued(for_agents(node.person_only, field.text.as_str().as_bytes()))
                },
                Kind::Progress { permille } => Element {
                    role: Role::Progress,
                    ..here.valued(for_agents(
                        node.person_only,
                        decimal(u32::from(*permille), &mut digits),
                    ))
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
                            .valued(for_agents(node.person_only, range.as_str().as_bytes()))
                    }
                }
                Kind::Dropdown(dropdown) => Element {
                    role: Role::ComboBox,
                    ..here.named(dropdown.name.as_bytes()).valued(for_agents(
                        node.person_only,
                        dropdown
                            .options
                            .get(dropdown.selected)
                            .map_or(&b""[..], |option| option.as_bytes()),
                    ))
                },
                Kind::Custom { .. } => Element {
                    role: Role::Graphic,
                    ..here
                },
                Kind::Tooltip(tooltip) => Element {
                    role: Role::Tooltip,
                    ..here
                        .named(tooltip.text.as_str().as_bytes())
                        .valued(tooltip.shortcut.as_str().as_bytes())
                },
                Kind::MenuBar(bar) => {
                    if let Some(open) = bar.open {
                        value = Text::from(plain_label(bar.titles[open]).as_str());
                    }
                    Element {
                        role: Role::MenuBar,
                        ..here.named(b"Menu").valued(value.as_str().as_bytes())
                    }
                    .flagged(if bar.open.is_some() {
                        flags::EXPANDED
                    } else {
                        0
                    })
                }
                Kind::MenuButton(button) => Element {
                    role: Role::Button,
                    ..here.named(button.text.as_str().as_bytes())
                }
                .flagged(if button.open { flags::EXPANDED } else { 0 }),
                Kind::Table(table) => {
                    let _ = match table.rows {
                        0 => core::fmt::Write::write_str(&mut range, "0 rows"),
                        rows => core::fmt::Write::write_fmt(
                            &mut range,
                            format_args!("row {} of {}", table.selected + 1, rows),
                        ),
                    };
                    Element {
                        role: Role::Table,
                        ..here
                            .named(table.name.as_bytes())
                            .valued(range.as_str().as_bytes())
                    }
                }
                Kind::Outline(outline) => {
                    let _ = match outline.rows {
                        0 => core::fmt::Write::write_str(&mut range, "0 rows"),
                        rows => core::fmt::Write::write_fmt(
                            &mut range,
                            format_args!("row {} of {}", outline.selected + 1, rows),
                        ),
                    };
                    Element {
                        role: Role::Tree,
                        ..here
                            .named(outline.name.as_bytes())
                            .valued(range.as_str().as_bytes())
                    }
                }
                Kind::TextArea(area) => {
                    let text = self.stores.get(area.store).map_or("", |store| store.text());
                    value = cut_value(text);
                    Element {
                        role: Role::TextArea,
                        ..here
                            .named(area.placeholder.as_bytes())
                            .valued(for_agents(node.person_only, value.as_str().as_bytes()))
                    }
                }
            };
            let element = match node.accessible_name.is_empty() {
                true => element,
                false => element.named(match names_a_control(&node.kind) {
                    true => node.accessible_name.as_str().as_bytes(),
                    false => for_agents(node.person_only, node.accessible_name.as_str().as_bytes()),
                }),
            };
            let element = match node.accessible_value.is_empty() {
                true => element,
                false => element.valued(for_agents(
                    node.person_only,
                    node.accessible_value.as_str().as_bytes(),
                )),
            };
            if !tree.push(&element) || !self.describe_parts(id, tree) {
                return;
            }
        }
    }

    fn describe_bar(
        &self,
        id: WidgetId,
        bar: &crate::menu::Bar,
        rect: Rect,
        disabled: u16,
        tree: &mut TreeWriter<'_>,
    ) -> bool {
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        (0..bar.count).all(|title| {
            let name = plain_label(bar.titles[title]);
            let open = bar.open == Some(title);
            let title_rect = bar.title_rect(rect, title);
            let element = Element::new(
                part_element(id, title),
                element_of(id),
                Role::MenuItem,
                bounds(title_rect),
            )
            .named(name.as_str().as_bytes())
            .flagged(disabled | if open { flags::EXPANDED } else { 0 });
            let menu = Menu::new(bar.menus[title], title_rect.x, rect.bottom() + 1, window);
            let parent = part_element(id, title);
            tree.push(&element)
                && describe_entries(
                    &menu,
                    parent,
                    |entry| {
                        let part = bar_entry_part(title, entry);
                        (part <= LAST_PART).then(|| part_element(id, part))
                    },
                    disabled,
                    tree,
                )
        })
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
            Kind::MenuBar(bar) => self.describe_bar(id, bar, node.rect, disabled, tree),
            Kind::MenuButton(button) => {
                let window = Rect::new(0, 0, self.size.0, self.size.1);
                let menu = button.menu(node.rect, window);
                describe_entries(
                    &menu,
                    element_of(id),
                    |entry| Some(part_element(id, entry)),
                    disabled,
                    tree,
                )
            }
            Kind::Table(table) => {
                let columns = (0..table.column_count).all(|column| {
                    let held = table.columns[column];
                    let left = table.column_x(node.rect, column);
                    let sorted = table.sort.filter(|(sorted, _)| *sorted == column);
                    let rect = Rect::new(
                        left,
                        node.rect.y,
                        held.width,
                        crate::widget::TABLE_HEADER_HEIGHT,
                    );
                    tree.push(
                        &part(
                            column,
                            Role::ColumnHeader,
                            rect,
                            held.title,
                            sorted.is_some(),
                        )
                        .valued(sorted.map_or(&b""[..], |(_, order)| order.name().as_bytes())),
                    )
                });
                let shown = table.shown_rows(node.rect);
                let first = shown.start;
                columns
                    && shown.take(LAST_PART + 1 - ROW_PART).all(|row| {
                        let mut name = Text::<64>::new();
                        name.push_str(table.cell_text(row, 0).as_str());
                        let mut cells = Text::<VALUE_BYTES>::new();
                        row_value(table, row, &mut cells);
                        let rect = table.row_rect(node.rect, row);
                        let element = Element::new(
                            part_element(id, ROW_PART + row - first),
                            element_of(id),
                            Role::Row,
                            bounds(rect),
                        )
                        .named(for_agents(node.person_only, name.as_str().as_bytes()))
                        .valued(for_agents(node.person_only, cells.as_str().as_bytes()))
                        .flagged(
                            disabled
                                | if row == table.selected {
                                    flags::SELECTED
                                } else {
                                    0
                                },
                        );
                        tree.push(&element)
                    })
            }
            Kind::Outline(outline) => {
                let shown = outline.shown_rows(node.rect);
                let first = shown.start;
                shown.take(LAST_PART + 1).all(|row| {
                    let held = outline.row_of(row);
                    let path = outline.path_of(row);
                    let mut state = disabled;
                    if row == outline.selected {
                        state |= flags::SELECTED;
                    }
                    if held.expandable {
                        state |= flags::EXPANDABLE;
                    }
                    if held.expanded {
                        state |= flags::EXPANDED;
                    }
                    let element = Element::new(
                        part_element(id, row - first),
                        element_of(id),
                        Role::TreeItem,
                        bounds(outline.row_rect(node.rect, row)),
                    )
                    .named(for_agents(node.person_only, held.text.as_str().as_bytes()))
                    .valued(for_agents(node.person_only, path.as_str().as_bytes()))
                    .flagged(state);
                    tree.push(&element)
                })
            }
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

    fn invoke_by_agent(&mut self, id: WidgetId, menu: usize, entry: usize) -> (Outcome, Response) {
        match self.invoke_entry(id, menu, entry) {
            Response::Nothing => (Outcome::Disabled, Response::Nothing),
            response => (Outcome::Done, response),
        }
    }

    fn table_column_width(&self, id: WidgetId, column: usize) -> Option<i32> {
        match self.kind(id)? {
            Kind::Table(table) => table.columns.get(column).map(|held| held.width),
            _ => None,
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
            (Kind::MenuBar(bar), Some(part)) => match bar_entry_of(part) {
                None => part < bar.count,
                Some((title, entry)) => title < bar.count && entry < bar.menus[title].len(),
            },
            (Kind::MenuButton(button), Some(part)) => part < button.entries.len(),
            (Kind::Outline(outline), Some(part)) => self
                .rect(id)
                .is_some_and(|rect| part < outline.shown_rows(rect).len()),
            (Kind::Table(table), Some(part)) => match part.checked_sub(ROW_PART) {
                None => part < table.column_count.min(MAX_COLUMNS),
                Some(row) => self
                    .rect(id)
                    .is_some_and(|rect| row < table.shown_rows(rect).len()),
            },
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
            (Action::SetText, Kind::Dropdown(dropdown), None) => {
                match dropdown.options.iter().position(|option| *option == text) {
                    Some(option) => (Outcome::Done, self.choose_option(id, option)),
                    None => (Outcome::NotSupported, Response::Nothing),
                }
            }
            (Action::SetText, Kind::Slider(_), None) => match text.trim().parse::<i32>() {
                Ok(wanted) => (Outcome::Done, self.adjust(id, wanted)),
                Err(_) => (Outcome::NotSupported, Response::Nothing),
            },
            (Action::SetText, Kind::TextField(_) | Kind::TextArea(_), None) => {
                self.set_text(id, text);
                (Outcome::Done, Response::Edited(id))
            }
            (Action::Invoke, Kind::MenuBar(_), Some(part)) => match bar_entry_of(part) {
                None => {
                    self.toggle_menu(id, part);
                    (Outcome::Done, Response::Nothing)
                }
                Some((title, entry)) => self.invoke_by_agent(id, title, entry),
            },
            (Action::Invoke, Kind::MenuButton(_), None) => {
                self.toggle_menu(id, 0);
                (Outcome::Done, Response::Nothing)
            }
            (Action::Invoke, Kind::MenuButton(_), Some(entry)) => {
                self.invoke_by_agent(id, 0, entry)
            }
            (Action::SetText, Kind::Outline(outline), None) => match text.trim().parse::<usize>() {
                Ok(row) if row >= 1 && row <= outline.rows => {
                    (Outcome::Done, self.select_outline_row(id, row - 1))
                }
                Ok(_) => (Outcome::Done, Response::Nothing),
                Err(_) => (Outcome::NotSupported, Response::Nothing),
            },
            (action @ (Action::Select | Action::Invoke), Kind::Outline(outline), Some(part)) => {
                let Some(rect) = self.rect(id) else {
                    return (Outcome::NoSuchElement, Response::Nothing);
                };
                let row = outline.shown_rows(rect).start + part;
                match action {
                    Action::Select => (Outcome::Done, self.select_outline_row(id, row)),
                    _ => (Outcome::Done, self.toggle_outline_row(id, row)),
                }
            }
            (Action::SetText, Kind::Table(table), None) => match text.trim().parse::<usize>() {
                Ok(row) if row >= 1 && row <= table.rows => {
                    (Outcome::Done, self.select_row(id, row - 1))
                }
                Ok(_) => (Outcome::Done, Response::Nothing),
                Err(_) => (Outcome::NotSupported, Response::Nothing),
            },
            (action, Kind::Table(table), Some(part)) => {
                let Some(rect) = self.rect(id) else {
                    return (Outcome::NoSuchElement, Response::Nothing);
                };
                match (action, part.checked_sub(ROW_PART)) {
                    (Action::Invoke, None) if table.columns[part].sortable => {
                        (Outcome::Done, self.header_clicked(id, part))
                    }
                    (Action::SetText, None) => match text.trim().parse::<i32>() {
                        Ok(width) => {
                            let before = table.columns[part].width;
                            self.set_column_width(id, part, width);
                            let after = self.table_column_width(id, part).unwrap_or(before);
                            match after == before {
                                true => (Outcome::Done, Response::Nothing),
                                false => (Outcome::Done, Response::ColumnResized(id, part, after)),
                            }
                        }
                        Err(_) => (Outcome::NotSupported, Response::Nothing),
                    },
                    (Action::Select, Some(shown)) => {
                        let row = table.shown_rows(rect).start + shown;
                        (Outcome::Done, self.select_row(id, row))
                    }
                    (Action::Invoke, Some(shown)) => {
                        let row = table.shown_rows(rect).start + shown;
                        self.select_row(id, row);
                        (Outcome::Done, Response::Opened(id, row))
                    }
                    _ => (Outcome::NotSupported, Response::Nothing),
                }
            }
            _ => (Outcome::NotSupported, Response::Nothing),
        }
    }
}

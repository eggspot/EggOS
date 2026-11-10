//! The widgets (docs/ux/toolkit-v0.3.md sections 4 to 8): what each holds, the size it asks
//! for, where its parts are, and how it is drawn in each state.

use crate::canvas::{Canvas, INSET_R2, INSET_R3, INSET_R4, INSET_R6, INSET_R8, SQUARE};
use crate::geometry::Rect;
use crate::input::key;
use crate::menu;
use crate::text::{self, Text};
use crate::theme::{Palette, Token};

/// Height of push buttons, text fields, checkboxes and radio rows.
pub const CONTROL_HEIGHT: i32 = 28;
/// Rows of a glyph cell.
pub const LINE_HEIGHT: i32 = 16;
/// The narrowest push button.
pub const BUTTON_MIN_WIDTH: i32 = 96;
/// Text padding inside a push button, each side.
pub const BUTTON_PADDING: i32 = 16;
/// Side of a checkbox's box and a radio's circle.
pub const MARK_SIZE: i32 = 16;
/// From a mark's left edge to its label's cell.
pub const LABEL_OFFSET: i32 = 24;
/// From a horizontal radio's label end to the next radio.
pub const RADIO_GAP: i32 = 24;
/// Text inset of a text field, each side.
pub const FIELD_PADDING: i32 = 8;
/// Width of a scroll bar.
pub const SCROLL_BAR_WIDTH: i32 = 8;
/// Shortest scroll bar thumb.
pub const MIN_THUMB: i32 = 24;
/// How far one wheel notch scrolls.
pub const WHEEL_STEP: i32 = 72;
/// From a card's label's cell top to the card's bottom (section 10: 76 in a 112 card).
pub const CARD_LABEL_FROM_BOTTOM: i32 = 36;
/// The most options a radio group or card picker holds.
pub const MAX_OPTIONS: usize = 4;
/// The most rows a list holds.
pub const MAX_ROWS: usize = 32;
/// Height of a progress bar.
pub const PROGRESS_HEIGHT: i32 = 8;
/// A progress bar's value when it is all done.
pub const PROGRESS_DONE: u16 = 1000;
/// A slider's track bar's height (docs/ux/toolkit-v0.4.md section 2).
pub const SLIDER_TRACK_HEIGHT: i32 = 4;
/// A slider's thumb's diameter.
pub const SLIDER_THUMB: i32 = 16;
/// The shortest track a slider takes by its content.
pub const SLIDER_MIN_TRACK: i32 = 128;
/// The width of a slider's value, drawn at its right.
pub const SLIDER_READOUT: i32 = 48;
/// The gap between a slider's track and its value.
pub const SLIDER_GAP: i32 = 8;
/// How many steps PageUp and PageDown move a slider.
pub const SLIDER_PAGE_STEPS: i32 = 10;
/// Where a dropdown's chosen option starts, from its left edge (docs/ux/toolkit-v0.4.md
/// section 3).
pub const DROPDOWN_TEXT_X: i32 = 12;
/// The room a dropdown keeps at its right for its chevron.
pub const DROPDOWN_ARROW_ROOM: i32 = 32;
/// A dropdown popup's row height.
pub const POPUP_ROW_HEIGHT: i32 = 32;
/// The most rows a dropdown's popup shows at once.
pub const POPUP_ROWS: usize = 8;
/// The gap between a dropdown and its popup.
pub const POPUP_GAP: i32 = 4;
/// How far a popup reaches past its dropdown on each side.
pub const POPUP_OUTSET: i32 = 12;
/// Where a popup row's `>` goes, from the popup's left edge.
pub const POPUP_MARK_X: i32 = 8;
/// Where a popup row's text goes, from the popup's left edge.
pub const POPUP_TEXT_X: i32 = 24;
/// A tooltip's height (docs/ux/toolkit-v0.5.md section 2).
pub const TOOLTIP_HEIGHT: i32 = 24;
/// A tooltip's text inset, each side.
pub const TOOLTIP_PADDING: i32 = 8;
/// From a tooltip's top to its text's cell.
pub const TOOLTIP_TEXT_TOP: i32 = 4;
/// The gap between a tooltip and its owner.
pub const TOOLTIP_GAP: i32 = 4;
/// The gap between a tooltip's text and its shortcut.
pub const TOOLTIP_SHORTCUT_GAP: i32 = 12;
/// How long the pointer or the focus rests on an owner before its tooltip shows.
pub const TOOLTIP_DELAY_MS: u32 = 600;
/// How soon after a tooltip hid another owner's shows at once.
pub const TOOLTIP_HOP_MS: u32 = 300;
/// How long a long-press's tooltip stays on a touch screen.
pub const TOOLTIP_TOUCH_MS: u32 = 1500;
/// A menu bar's height (docs/ux/toolkit-v0.5.md section 3).
pub const MENU_BAR_HEIGHT: i32 = 28;
/// A menu bar title's text inset, each side.
pub const MENU_TITLE_PADDING: i32 = 12;
/// The gap between a menu item's label and its key.
pub const MENU_KEY_GAP: i32 = 24;
/// The most menus a menu bar holds: what `SET_MENU` takes.
pub const MAX_MENUS: usize = 6;
/// A table's header height (docs/ux/toolkit-v0.5.md section 4).
pub const TABLE_HEADER_HEIGHT: i32 = 28;
/// A table row's height.
pub const TABLE_ROW_HEIGHT: i32 = 24;
/// A table cell's text inset, each side.
pub const TABLE_CELL_PADDING: i32 = 8;
/// Where the selected row's `>` goes, from the first column's left edge.
pub const TABLE_MARK_X: i32 = 8;
/// Where the first column's text goes, from its left edge.
pub const TABLE_FIRST_TEXT_X: i32 = 24;
/// The width of a column's resize zone, centred on its right edge.
pub const TABLE_HANDLE: i32 = 7;
/// The narrowest column.
pub const TABLE_MIN_COLUMN: i32 = 40;
/// The room the sorted column keeps for its arrow.
pub const TABLE_ARROW_ROOM: i32 = 20;
/// How far Left and Right scroll a wide table.
pub const TABLE_KEY_SCROLL: i32 = 48;
/// The most columns a table holds.
pub const MAX_COLUMNS: usize = 8;
/// A text area's left text inset (docs/ux/toolkit-v0.5.md section 5).
pub const TEXT_AREA_PADDING: i32 = 8;
/// From a text area's top to its first line's cell.
pub const TEXT_AREA_TOP: i32 = 6;
/// From one line of a text area to the next.
pub const TEXT_AREA_LINE_PITCH: i32 = 20;
/// The lines one wheel notch scrolls a text area.
pub const TEXT_AREA_WHEEL_LINES: i32 = 3;
/// A tab's width in a text area.
pub const TEXT_AREA_TAB: i32 = 16;
/// The most bytes a text area holds.
pub const TEXT_AREA_CAPACITY: usize = 65536;
/// The most visual lines a text area holds.
pub const TEXT_AREA_LINES: usize = 4096;
/// How far a press moves before it becomes a drag (docs/ux/toolkit-v0.5.md section 7).
pub const DRAG_THRESHOLD: i32 = 6;
/// From the pointer to a drag's ghost, each way.
pub const DRAG_GHOST_OFFSET: i32 = 12;
/// The band at a scrollable target's top and bottom that scrolls it during a drag.
pub const DRAG_EDGE: i32 = 24;
/// How often a drag in that band scrolls the target one row.
pub const DRAG_SCROLL_MS: u32 = 100;
/// How long a press held still is a long-press on a touch screen.
pub const LONG_PRESS_MS: u32 = 500;

/// The kind of device a window is on, for the rules that differ by it
/// (docs/ux/toolkit-v0.5.md section 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormFactor {
    /// A pointer and a keyboard.
    #[default]
    Desktop,
    /// Touch: long-presses show tooltips and pick up rows.
    Phone,
    /// A D-pad: the focus rule alone shows tooltips.
    Tv,
    /// One small screen: no tooltips, no drag.
    Wearable,
}

/// The device class of a screen `width` by `height` at `scale` with or without touch, as the
/// shell decides it (docs/ux/toolkit-v0.5.md section 11).
pub fn form_factor(width: i32, height: i32, scale: i32, touch: bool) -> FormFactor {
    let (width, height) = (width / scale.max(1), height / scale.max(1));
    match () {
        () if width.max(height) <= 320 => FormFactor::Wearable,
        () if touch && width.min(height) <= 480 => FormFactor::Phone,
        _ => FormFactor::Desktop,
    }
}

pub use crate::drag::DropMode;
pub use crate::table::{CellText, Column, SortOrder, Table};
pub use crate::text_area::TextArea;
pub use crate::tooltip::{Tooltip, tooltip_rect};

/// Where a widget's left edge goes in its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Horizontal {
    /// This far right of the parent's left edge.
    Left(i32),
    /// Its right edge this far left of the parent's right edge.
    Right(i32),
    /// Centred.
    Centre,
}

/// A widget's width or height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// Exactly this many pixels.
    Px(i32),
    /// What the parent has left after the widget's offset, less this margin at the far end.
    Fill(i32),
    /// What the widget's content needs.
    Content,
}

/// Where a widget goes in its parent and how big it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub horizontal: Horizontal,
    /// Below the parent's top; in a [`Kind::Column`], below the previous child instead.
    pub top: i32,
    pub width: Length,
    pub height: Length,
}

impl Place {
    /// At (`x`, `y`) in the parent, `width` by `height`.
    pub const fn at(x: i32, y: i32, width: Length, height: Length) -> Self {
        Self {
            horizontal: Horizontal::Left(x),
            top: y,
            width,
            height,
        }
    }

    /// The parent's whole area.
    pub const fn fill() -> Self {
        Self::at(0, 0, Length::Fill(0), Length::Fill(0))
    }

    /// At `top`, its right edge `margin` left of the parent's, as big as its content.
    pub const fn right(margin: i32, top: i32) -> Self {
        Self {
            horizontal: Horizontal::Right(margin),
            top,
            width: Length::Content,
            height: Length::Content,
        }
    }
}

/// How a label sits in its rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Centre,
    End,
}

/// What Esc does in a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// Empties the text; when already empty Esc goes on to the window.
    Clear,
    /// Puts back the text the field had when it got the focus.
    Revert,
}

/// Draws a card's art into its rectangle: the card picker's index of it, then where.
pub type CardArt = fn(&mut Canvas<'_>, usize, Rect);

/// Draws a custom widget into its rectangle, given the widget's data word.
pub type Painter = fn(&mut Canvas<'_>, Rect, u32);

/// A view's vertical scroll bar, then its horizontal one, each as its track and thumb when shown.
pub type ScrollBars = (Option<(Rect, Rect)>, Option<(Rect, Rect)>);

/// A text field's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub text: Text<255>,
    /// The caret's byte in `text`.
    pub caret: usize,
    /// The selection's other end: the text between it and the caret is selected, none when
    /// they are equal (docs/ux/toolkit-v0.4.md section 4).
    pub anchor: usize,
    /// How far the text is scrolled left, in pixels.
    pub scroll: i32,
    pub placeholder: &'static str,
    pub escape: Escape,
}

/// A vertical list of rows in its own scroll view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct List {
    pub items: &'static [&'static str],
    /// Bit `i` set: row `i` is shown.
    pub shown: u32,
    pub selected: usize,
    pub offset: i32,
    pub row_height: i32,
    /// Where the `>` mark goes, from the list's left edge.
    pub mark_x: i32,
    /// Where the text goes.
    pub text_x: i32,
    pub empty_text: &'static str,
}

impl List {
    /// The shown items' indexes, in order.
    pub fn shown_items(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.items.len().min(MAX_ROWS)).filter(|&index| self.shown & (1 << index) != 0)
    }

    /// Where item `index` is among the shown rows.
    pub fn row_of(&self, index: usize) -> Option<usize> {
        self.shown_items().position(|item| item == index)
    }

    /// The item shown at row `row`.
    pub fn item_at_row(&self, row: usize) -> Option<usize> {
        self.shown_items().nth(row)
    }

    /// The height of every shown row together.
    pub fn content_height(&self) -> i32 {
        self.shown_items().count() as i32 * self.row_height
    }
}

/// What a widget is and what it holds.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    /// A container, filled with `fill` if any, with a 1 px line of `border_right` along its
    /// right edge if any.
    Panel {
        fill: Option<Token>,
        border_right: Option<Token>,
    },
    /// A container that stacks its children top to bottom, `gap` apart.
    Column { gap: i32 },
    /// A container whose children scroll under its rectangle, vertically by `offset` and
    /// horizontally by `offset_x` (docs/ux/toolkit-v0.5.md section 6).
    ScrollView {
        offset: i32,
        content_height: i32,
        offset_x: i32,
        content_width: i32,
    },
    /// One line of text, `size` 1 (body) or 2 (heading).
    Label {
        text: Text<64>,
        colour: Token,
        size: i32,
        align: Align,
    },
    /// A push button; `primary` for the window's default button.
    Button { text: Text<32>, primary: bool },
    /// A checkbox and its label.
    Checkbox { text: Text<32>, checked: bool },
    /// A group of radio buttons, one always selected, side by side when `horizontal`.
    Radios {
        options: [&'static str; MAX_OPTIONS],
        count: usize,
        selected: usize,
        horizontal: bool,
    },
    /// A radio group drawn as cards (Settings' profile picker): `count` cards of `card`
    /// size, `gap` apart, with their labels and art; `ring` is the card the focus ring is on.
    Cards {
        labels: [&'static str; MAX_OPTIONS],
        count: usize,
        selected: usize,
        ring: usize,
        card: (i32, i32),
        gap: i32,
        art: CardArt,
    },
    /// A list of rows.
    List(List),
    /// A single-line text field.
    TextField(Field),
    /// A progress bar, `permille` of it done (docs/ux/desktop-v0.3-gui.md section 9).
    Progress { permille: u16 },
    /// A slider setting a number in a range (docs/ux/toolkit-v0.4.md section 2).
    Slider(Slider),
    /// A button showing one of its options that opens a list of them (docs/ux/toolkit-v0.4.md
    /// section 3).
    Dropdown(Dropdown),
    /// Something the program draws itself, with a word of its own data.
    Custom { painter: Painter, data: u32 },
    /// The tile naming its parent widget (docs/ux/toolkit-v0.5.md section 2).
    Tooltip(Tooltip),
    /// A window's menu bar (docs/ux/toolkit-v0.5.md section 3).
    MenuBar(menu::Bar),
    /// A button that opens a short menu.
    MenuButton(menu::Button),
    /// Rows of cells under a header of resizable, sortable columns (docs/ux/toolkit-v0.5.md
    /// section 4).
    Table(Table),
    /// Multi-line text that wraps at words (docs/ux/toolkit-v0.5.md section 5).
    TextArea(TextArea),
}

impl Kind {
    /// A panel filled with `fill`.
    pub const fn panel(fill: Token) -> Self {
        Self::Panel {
            fill: Some(fill),
            border_right: None,
        }
    }

    /// A scroll view at its start, its content measured when it is laid out.
    pub const fn scroll_view() -> Self {
        Self::ScrollView {
            offset: 0,
            content_height: 0,
            offset_x: 0,
            content_width: 0,
        }
    }

    /// A body label in `colour`.
    pub fn label(text: &str, colour: Token) -> Self {
        Self::Label {
            text: Text::from(text),
            colour,
            size: 1,
            align: Align::Start,
        }
    }

    /// A heading: a label at size 2.
    pub fn heading(text: &str) -> Self {
        Self::Label {
            text: Text::from(text),
            colour: Token::Text,
            size: 2,
            align: Align::Start,
        }
    }

    /// A push button.
    pub fn button(text: &str, primary: bool) -> Self {
        Self::Button {
            text: Text::from(text),
            primary,
        }
    }

    /// A checkbox.
    pub fn checkbox(text: &str, checked: bool) -> Self {
        Self::Checkbox {
            text: Text::from(text),
            checked,
        }
    }

    /// A radio group of `options` (at most [`MAX_OPTIONS`]).
    pub fn radios(options: &[&'static str], selected: usize, horizontal: bool) -> Self {
        let mut held = [""; MAX_OPTIONS];
        let count = options.len().min(MAX_OPTIONS);
        held[..count].copy_from_slice(&options[..count]);
        Self::Radios {
            options: held,
            count,
            selected: selected.min(count.saturating_sub(1)),
            horizontal,
        }
    }

    /// A card picker of `labels` (at most [`MAX_OPTIONS`]).
    pub fn cards(
        labels: &[&'static str],
        selected: usize,
        card: (i32, i32),
        gap: i32,
        art: CardArt,
    ) -> Self {
        let mut held = [""; MAX_OPTIONS];
        let count = labels.len().min(MAX_OPTIONS);
        held[..count].copy_from_slice(&labels[..count]);
        let selected = selected.min(count.saturating_sub(1));
        Self::Cards {
            labels: held,
            count,
            selected,
            ring: selected,
            card,
            gap,
            art,
        }
    }

    /// A list of `items` (at most [`MAX_ROWS`]), every one shown, the first selected.
    pub fn list(items: &'static [&'static str], row_height: i32, mark_x: i32, text_x: i32) -> Self {
        let count = items.len().min(MAX_ROWS);
        Self::List(List {
            items,
            shown: if count == 32 {
                u32::MAX
            } else {
                (1 << count) - 1
            },
            selected: 0,
            offset: 0,
            row_height,
            mark_x,
            text_x,
            empty_text: "No matches",
        })
    }

    /// An empty text field showing `placeholder`.
    pub const fn text_field(placeholder: &'static str, escape: Escape) -> Self {
        Self::TextField(Field {
            text: Text::new(),
            caret: 0,
            anchor: 0,
            scroll: 0,
            placeholder,
            escape,
        })
    }

    /// A slider named `name` for agents, from `min` to `max` by `step`, at `value`, its value
    /// drawn with `unit` after it; `max` lowered to the last value a step reaches and `value`
    /// snapped to one.
    pub fn slider(
        name: &'static str,
        min: i32,
        max: i32,
        step: i32,
        value: i32,
        unit: &'static str,
    ) -> Self {
        let step = step.max(1);
        let max = min + (max.max(min) - min) / step * step;
        let mut slider = Slider {
            name,
            value: min,
            min,
            max,
            step,
            unit,
            readout: true,
        };
        slider.value = slider.snapped(value);
        Self::Slider(slider)
    }

    /// A dropdown named `name` for agents, choosing among `options` (at most [`MAX_ROWS`]),
    /// `selected` chosen.
    pub fn dropdown(name: &'static str, options: &'static [&'static str], selected: usize) -> Self {
        let options = &options[..options.len().min(MAX_ROWS)];
        let selected = selected.min(options.len().saturating_sub(1));
        Self::Dropdown(Dropdown {
            name,
            options,
            selected,
            open: false,
            highlighted: selected,
            offset: 0,
        })
    }

    /// A tooltip saying `text` and `shortcut` (`""` for none), for the widget it is added under.
    pub fn tooltip(text: &str, shortcut: &str) -> Self {
        Self::Tooltip(Tooltip {
            text: Text::from(text),
            shortcut: Text::from(shortcut),
        })
    }

    /// A menu bar of `menus` (at most [`MAX_MENUS`]), each a title and its entries.
    pub fn menu_bar(menus: &[(&'static str, &'static [menu::Entry])]) -> Self {
        Self::MenuBar(menu::Bar::new(menus))
    }

    /// A menu button showing `text` that opens `entries`.
    pub fn menu_button(text: &str, entries: &'static [menu::Entry]) -> Self {
        Self::MenuButton(menu::Button {
            text: Text::from(text),
            entries,
            open: false,
            highlighted: None,
        })
    }

    /// A table named `name` for agents, of `columns` (at most [`MAX_COLUMNS`]) and `rows` rows
    /// whose cells `cell` writes from the program's `data`.
    pub fn table(
        name: &'static str,
        columns: &[Column],
        rows: usize,
        cell: CellText,
        data: u32,
    ) -> Self {
        Self::Table(Table::new(name, columns, rows, cell, data))
    }

    /// A text area keeping its text in the tree's text store `store`, showing `placeholder`
    /// while empty.
    pub const fn text_area(store: usize, placeholder: &'static str) -> Self {
        Self::TextArea(TextArea::new(store, placeholder))
    }

    /// Whether text is typed into it: a text field or a text area.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::TextField(_) | Self::TextArea(_))
    }

    /// Whether Tab stops on it.
    pub fn is_focusable(&self) -> bool {
        matches!(
            self,
            Self::Button { .. }
                | Self::Slider(_)
                | Self::Dropdown(_)
                | Self::Checkbox { .. }
                | Self::Radios { .. }
                | Self::Cards { .. }
                | Self::List(_)
                | Self::TextField(_)
                | Self::MenuButton(_)
                | Self::Table(_)
                | Self::TextArea(_)
        )
    }

    /// Whether the pointer acts on it.
    pub fn is_interactive(&self) -> bool {
        self.is_focusable() || matches!(self, Self::ScrollView { .. } | Self::MenuBar(_))
    }

    /// Whether it holds other widgets.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            Self::Panel { .. } | Self::Column { .. } | Self::ScrollView { .. }
        )
    }

    /// The size its content needs.
    pub fn content_size(&self) -> (i32, i32) {
        match self {
            Self::Label { text, size, .. } => {
                (text::width(text.as_str()) * size, LINE_HEIGHT * size)
            }
            Self::Button { text, .. } => (
                (text::width(text.as_str()) + 2 * BUTTON_PADDING).max(BUTTON_MIN_WIDTH),
                CONTROL_HEIGHT,
            ),
            Self::Checkbox { text, .. } => {
                (LABEL_OFFSET + text::width(text.as_str()), CONTROL_HEIGHT)
            }
            Self::Radios {
                options,
                count,
                horizontal,
                ..
            } => {
                let widths = options[..*count]
                    .iter()
                    .map(|option| LABEL_OFFSET + text::width(option));
                match horizontal {
                    true => (
                        widths.sum::<i32>() + RADIO_GAP * (*count as i32 - 1).max(0),
                        CONTROL_HEIGHT,
                    ),
                    false => (widths.max().unwrap_or(0), CONTROL_HEIGHT * *count as i32),
                }
            }
            Self::Cards {
                count, card, gap, ..
            } => (
                card.0 * *count as i32 + gap * (*count as i32 - 1).max(0),
                card.1,
            ),
            Self::List(list) => (0, list.content_height()),
            Self::TextField(_) => (BUTTON_MIN_WIDTH, CONTROL_HEIGHT),
            Self::Progress { .. } => (BUTTON_MIN_WIDTH, PROGRESS_HEIGHT),
            Self::Slider(slider) => match slider.readout {
                true => (
                    SLIDER_MIN_TRACK + SLIDER_GAP + SLIDER_READOUT,
                    CONTROL_HEIGHT,
                ),
                false => (SLIDER_MIN_TRACK, CONTROL_HEIGHT),
            },
            Self::Dropdown(dropdown) => {
                let widest = dropdown
                    .options
                    .iter()
                    .map(|option| text::width(option))
                    .max()
                    .unwrap_or(0);
                (
                    (DROPDOWN_TEXT_X + widest + DROPDOWN_ARROW_ROOM).max(BUTTON_MIN_WIDTH),
                    CONTROL_HEIGHT,
                )
            }
            Self::Tooltip(tooltip) => (tooltip.width(), TOOLTIP_HEIGHT),
            Self::MenuBar(bar) => (bar.content_width(), MENU_BAR_HEIGHT),
            Self::MenuButton(button) => (
                (BUTTON_PADDING + text::width(button.text.as_str()) + DROPDOWN_ARROW_ROOM)
                    .max(BUTTON_MIN_WIDTH),
                CONTROL_HEIGHT,
            ),
            Self::Table(table) => table.content_size(),
            Self::TextArea(_) => (240, 2 * TEXT_AREA_TOP + 4 * TEXT_AREA_LINE_PITCH),
            Self::Panel { .. }
            | Self::Column { .. }
            | Self::ScrollView { .. }
            | Self::Custom { .. } => (0, 0),
        }
    }
}

/// The rectangle of option `index` of a radio group at `rect`: its mark and label, 28 tall.
pub fn radio_rect(kind: &Kind, rect: Rect, index: usize) -> Rect {
    let Kind::Radios {
        options,
        horizontal,
        ..
    } = kind
    else {
        return Rect::default();
    };
    let width_of = |option: &str| LABEL_OFFSET + text::width(option);
    match horizontal {
        true => {
            let x = options[..index]
                .iter()
                .map(|option| width_of(option) + RADIO_GAP)
                .sum::<i32>();
            Rect::new(rect.x + x, rect.y, width_of(options[index]), CONTROL_HEIGHT)
        }
        false => Rect::new(
            rect.x,
            rect.y + CONTROL_HEIGHT * index as i32,
            width_of(options[index]),
            CONTROL_HEIGHT,
        ),
    }
}

/// The rectangle of card `index` of a card picker at `rect`.
pub fn card_rect(kind: &Kind, rect: Rect, index: usize) -> Rect {
    let Kind::Cards { card, gap, .. } = kind else {
        return Rect::default();
    };
    Rect::new(
        rect.x + (card.0 + gap) * index as i32,
        rect.y,
        card.0,
        card.1,
    )
}

/// Where a scroll bar goes in a view `view` tall over content `content` tall scrolled by
/// `offset`: the track and the thumb; `None` when the content fits.
pub fn scroll_bar(rect: Rect, content: i32, offset: i32) -> Option<(Rect, Rect)> {
    if content <= rect.height || rect.height <= 0 {
        return None;
    }
    let track = Rect::new(
        rect.right() - SCROLL_BAR_WIDTH,
        rect.y,
        SCROLL_BAR_WIDTH,
        rect.height,
    );
    let height = i64::from(rect.height);
    let length = (height * height / i64::from(content))
        .max(i64::from(MIN_THUMB))
        .min(height);
    let travel = height - length;
    let position = i64::from(offset) * travel / (i64::from(content) - height);
    let (length, position) = (length as i32, position.clamp(0, travel) as i32);
    Some((
        track,
        Rect::new(track.x, rect.y + position, SCROLL_BAR_WIDTH, length),
    ))
}

/// Where a horizontal scroll bar goes along the bottom of `rect` over content `content` wide
/// scrolled by `offset`: the track and the thumb; `None` when the content fits
/// (docs/ux/toolkit-v0.5.md section 6).
pub fn horizontal_bar(rect: Rect, content: i32, offset: i32) -> Option<(Rect, Rect)> {
    if content <= rect.width || rect.width <= 0 {
        return None;
    }
    let track = Rect::new(
        rect.x,
        rect.bottom() - SCROLL_BAR_WIDTH,
        rect.width,
        SCROLL_BAR_WIDTH,
    );
    let width = i64::from(rect.width);
    let length = (width * width / i64::from(content))
        .max(i64::from(MIN_THUMB))
        .min(width);
    let travel = width - length;
    let position = i64::from(offset) * travel / (i64::from(content) - width);
    let (length, position) = (length as i32, position.clamp(0, travel) as i32);
    Some((
        track,
        Rect::new(rect.x + position, track.y, length, SCROLL_BAR_WIDTH),
    ))
}

/// Which of a view's two bars show for content `content` (width, height) in `rect`: the
/// vertical one, then the horizontal one; each strip is reserved only when its bar shows.
pub fn bars_shown(rect: Rect, content: (i32, i32)) -> (bool, bool) {
    let vertical_alone = content.1 > rect.height;
    let horizontal = content.0 > rect.width - if vertical_alone { SCROLL_BAR_WIDTH } else { 0 };
    let vertical = content.1 > rect.height - if horizontal { SCROLL_BAR_WIDTH } else { 0 };
    (vertical, horizontal)
}

/// The part of a view at `rect` its content shows through: `rect` less the strips of the bars
/// that show for content `content`.
pub fn viewport(rect: Rect, content: (i32, i32)) -> Rect {
    let (vertical, horizontal) = bars_shown(rect, content);
    Rect::new(
        rect.x,
        rect.y,
        rect.width - if vertical { SCROLL_BAR_WIDTH } else { 0 },
        rect.height - if horizontal { SCROLL_BAR_WIDTH } else { 0 },
    )
}

/// Where both bars of a view at `rect` go over content `content` (width, height) scrolled by
/// `offset` (x, y): the vertical bar's track and thumb, then the horizontal one's; when both
/// show, the 8x8 corner where they meet is neither's.
pub fn scroll_bars(rect: Rect, content: (i32, i32), offset: (i32, i32)) -> ScrollBars {
    let (vertical, horizontal) = bars_shown(rect, content);
    let inner = viewport(rect, content);
    let vertical = match vertical {
        true => scroll_bar(
            Rect::new(rect.x, rect.y, rect.width, inner.height),
            content.1,
            offset.1,
        ),
        false => None,
    };
    let horizontal = match horizontal {
        true => horizontal_bar(
            Rect::new(rect.x, rect.y, inner.width, rect.height),
            content.0,
            offset.0,
        ),
        false => None,
    };
    (vertical, horizontal)
}

/// The rectangle of shown row `row` of a list at `rect`.
pub fn row_rect(list: &List, rect: Rect, row: usize) -> Rect {
    let bar = match scroll_bar(rect, list.content_height(), list.offset) {
        Some(_) => SCROLL_BAR_WIDTH,
        None => 0,
    };
    Rect::new(
        rect.x,
        rect.y + row as i32 * list.row_height - list.offset,
        rect.width - bar,
        list.row_height,
    )
}

/// The inner rectangle of a text field at `rect`, where its text goes.
pub fn field_inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x + FIELD_PADDING,
        rect.y,
        rect.width - 2 * FIELD_PADDING,
        rect.height,
    )
}

/// The caret's rectangle in a text field at `rect`.
pub fn caret_rect(field: &Field, rect: Rect) -> Rect {
    let inner = field_inner(rect);
    let before = text::width(&field.text.as_str()[..field.caret]);
    Rect::new(
        inner.x + before - field.scroll - 1,
        rect.y + (rect.height - LINE_HEIGHT) / 2,
        1,
        LINE_HEIGHT,
    )
}

/// The narrowest cell a waiting dead key's accent is drawn in at the caret (D221).
pub const DEAD_KEY_MIN_WIDTH: i32 = 4;

/// The cell a waiting dead key's `accent` takes at a caret whose left edge is `x` on a line
/// whose top is `y` (D221): as wide as the accent's advance, at least [`DEAD_KEY_MIN_WIDTH`],
/// and a line tall.
pub fn dead_key_cell(x: i32, y: i32, accent: char) -> Rect {
    let mut bytes = [0; 4];
    let width = text::width(accent.encode_utf8(&mut bytes)).max(DEAD_KEY_MIN_WIDTH);
    Rect::new(x, y, width, LINE_HEIGHT)
}

/// Draws a cell for a waiting dead key's `accent` at `x` on the line at `y` and moves `after`
/// right by its width (D221): the line's background from `x` to `right`, the cell in the
/// accent token with the accent in `icon_dark`, then `after` in the text token.
pub fn paint_dead_key_cell(
    canvas: &mut Canvas<'_>,
    (x, y): (i32, i32),
    right: i32,
    accent: char,
    after: &str,
) {
    let palette = canvas.palette();
    let cell = dead_key_cell(x, y, accent);
    canvas.fill(Rect::new(x, y, right - x, LINE_HEIGHT), palette.field);
    canvas.fill(cell, palette.accent);
    let mut bytes = [0; 4];
    canvas.text(x, y, accent.encode_utf8(&mut bytes), palette.icon_dark, 1);
    canvas.text(cell.right(), y, after, palette.text, 1);
}

/// Draws a waiting dead key's `accent` at the caret of `field` at `rect` in place of the
/// caret (D221), the text after the caret moved right by the cell's width.
pub fn paint_dead_key(canvas: &mut Canvas<'_>, field: &Field, rect: Rect, accent: char) {
    let inner = field_inner(rect);
    let previous = canvas.narrow(inner);
    let text = field.text.as_str();
    let x = inner.x - field.scroll + text::width(&text[..field.caret]);
    let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
    paint_dead_key_cell(canvas, (x, y), inner.right(), accent, &text[field.caret..]);
    canvas.restore(previous);
}

/// The caret's byte in `field` at `rect` nearest to x `x`, as a click there places it.
pub fn caret_at(field: &Field, rect: Rect, x: i32) -> usize {
    let inner = field_inner(rect);
    let wanted = x - inner.x + field.scroll;
    let mut caret = 0;
    let mut used = 0;
    for (at, character) in field.text.as_str().char_indices() {
        let advance = text::width(character.encode_utf8(&mut [0; 4]));
        if used + advance / 2 > wanted {
            break;
        }
        used += advance;
        caret = at + character.len_utf8();
    }
    caret
}

/// Scrolls `field` at `rect` so its caret is inside it, and no further than its text needs.
pub fn keep_caret_visible(field: &mut Field, rect: Rect) {
    let inner = field_inner(rect);
    let before = text::width(&field.text.as_str()[..field.caret]);
    let total = text::width(field.text.as_str());
    if before - field.scroll > inner.width {
        field.scroll = before - inner.width;
    }
    if before - field.scroll < 0 {
        field.scroll = before;
    }
    if field.scroll > 0 && total - field.scroll < inner.width {
        field.scroll = (total - inner.width).max(0);
    }
}

/// What a key did to a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKey {
    /// The field does not use the key.
    Unused,
    /// The caret moved, or the key did nothing visible.
    Moved,
    /// The text changed.
    Changed,
    /// The selection was copied ([`Field::selected`] holds it).
    Copied,
    /// The selection was copied, then removed.
    Cut,
    /// The clipboard's text is wanted at the caret.
    PasteWanted,
}

impl Field {
    /// The selection's first and last byte, both within the text and on character boundaries
    /// however `caret` and `anchor` were set.
    pub fn selection(&self) -> (usize, usize) {
        let text = self.text.as_str();
        let bounded = |at: usize| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        let (caret, anchor) = (bounded(self.caret), bounded(self.anchor));
        (caret.min(anchor), caret.max(anchor))
    }

    /// Puts the caret at byte `at` (at most the text's end), nothing selected.
    pub fn place_caret(&mut self, at: usize) {
        self.caret = at.min(self.text.len());
        self.anchor = self.caret;
    }

    /// Replaces its text, the caret at the end and nothing selected.
    pub fn set_text(&mut self, text: &str) {
        self.text.set(text);
        self.place_caret(self.text.len());
    }

    /// The selected text.
    pub fn selected(&self) -> &str {
        let (start, end) = self.selection();
        self.text.as_str().get(start..end).unwrap_or("")
    }

    /// Removes the selected text, the caret where it was; whether there was any.
    pub fn remove_selection(&mut self) -> bool {
        let (start, end) = self.selection();
        if start == end {
            return false;
        }
        let mut kept = Text::<255>::new();
        let words = self.text.as_str();
        kept.push_str(&words[..start]);
        kept.push_str(&words[end..]);
        self.text = kept;
        self.caret = start;
        self.anchor = start;
        true
    }

    /// Selects all of its text, the caret at the end.
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// The word around byte `at` as a double-click selects it: a run of ASCII letters,
    /// digits and `_`, or of characters beyond ASCII; a run of spaces; or one other character.
    pub fn word_at(&self, at: usize) -> (usize, usize) {
        let words = self.text.as_str();
        let class = |character: char| match character {
            ' ' => 0,
            character if character.is_ascii_alphanumeric() || character == '_' => 1,
            character if !character.is_ascii() => 1,
            _ => 2,
        };
        let Some(here) = words[at.min(words.len())..]
            .chars()
            .next()
            .or_else(|| words[..at.min(words.len())].chars().next_back())
        else {
            return (0, 0);
        };
        let start_at = match words[at.min(words.len())..].chars().next() {
            Some(_) => at.min(words.len()),
            None => at.min(words.len()) - here.len_utf8(),
        };
        let kind = class(here);
        if kind == 2 {
            return (start_at, start_at + here.len_utf8());
        }
        let start = words[..start_at]
            .char_indices()
            .rev()
            .take_while(|&(_, character)| class(character) == kind)
            .last()
            .map_or(start_at, |(index, _)| index);
        let end = words[start_at..]
            .char_indices()
            .take_while(|&(_, character)| class(character) == kind)
            .last()
            .map_or(start_at, |(index, character)| {
                start_at + index + character.len_utf8()
            });
        (start, end)
    }
}

/// Applies key `code` (typing `character`, with Ctrl, Alt or Super held when `command`) to
/// `field` as section 7 of docs/ux/toolkit-v0.3.md says; Esc in a [`Escape::Revert`] field
/// puts back `before`.
pub fn field_key(
    field: &mut Field,
    code: u16,
    character: u32,
    command: bool,
    before: &Text<255>,
) -> FieldKey {
    field_key_with(field, code, character, command, false, false, before)
}

/// [`field_key`] with Shift (`extend`) and Ctrl or Super (`shortcut`) told apart: Shift with a
/// move extends the selection, and the shortcut modifier with A, C, X and V selects all,
/// copies, cuts and asks for a paste (docs/ux/toolkit-v0.4.md section 4.3).
pub fn field_key_with(
    field: &mut Field,
    code: u16,
    character: u32,
    command: bool,
    extend: bool,
    shortcut: bool,
    before: &Text<255>,
) -> FieldKey {
    let selecting = field.anchor != field.caret;
    if shortcut {
        match code {
            key::A => {
                field.select_all();
                return FieldKey::Moved;
            }
            key::C if selecting => return FieldKey::Copied,
            key::X if selecting => return FieldKey::Cut,
            key::V => return FieldKey::PasteWanted,
            key::C | key::X => return FieldKey::Moved,
            _ => {}
        }
    }
    let moves = matches!(code, key::LEFT | key::RIGHT | key::HOME | key::END);
    if moves && selecting && !extend && matches!(code, key::LEFT | key::RIGHT) {
        let (start, end) = field.selection();
        field.caret = if code == key::LEFT { start } else { end };
        field.anchor = field.caret;
        return FieldKey::Moved;
    }
    if selecting && matches!(code, key::BACKSPACE | key::DELETE) {
        field.remove_selection();
        return FieldKey::Changed;
    }
    let anchor = field.anchor;
    let typing = !moves && !matches!(code, key::BACKSPACE | key::DELETE | key::ESC);
    let typed_over = typing
        && selecting
        && char::from_u32(character)
            .is_some_and(|typed| !command && eggos_font::egg_sans::covers(typed));
    if typed_over {
        field.remove_selection();
    }
    let result = field_key_alone(field, code, character, command, before);
    field.anchor = match (moves && extend, result) {
        (true, _) => anchor,
        (false, FieldKey::Unused) => anchor,
        _ => field.caret,
    };
    match (typed_over, result) {
        (true, FieldKey::Unused | FieldKey::Moved) => FieldKey::Changed,
        _ => result,
    }
}

fn field_key_alone(
    field: &mut Field,
    code: u16,
    character: u32,
    command: bool,
    before: &Text<255>,
) -> FieldKey {
    let changed = match code {
        key::BACKSPACE => {
            let at = field.text.previous_boundary(field.caret);
            let changed = at != field.caret;
            if changed {
                field.text.remove(at);
                field.caret = at;
            }
            changed
        }
        key::DELETE => field.text.remove(field.caret) > 0,
        key::LEFT => {
            field.caret = field.text.previous_boundary(field.caret);
            false
        }
        key::RIGHT => {
            field.caret = field.text.next_boundary(field.caret);
            false
        }
        key::HOME => {
            field.caret = 0;
            false
        }
        key::END => {
            field.caret = field.text.len();
            false
        }
        key::ESC => match field.escape {
            Escape::Clear if field.text.is_empty() => return FieldKey::Unused,
            Escape::Clear => {
                field.text.set("");
                field.caret = 0;
                true
            }
            Escape::Revert => {
                let changed = field.text != *before;
                field.text = *before;
                field.caret = field.text.len();
                changed
            }
        },
        _ => {
            let typed = char::from_u32(character)
                .filter(|typed| !command && eggos_font::egg_sans::covers(*typed));
            let Some(typed) = typed else {
                return FieldKey::Unused;
            };
            let inserted = field.text.insert(field.caret, typed);
            if inserted {
                field.caret += typed.len_utf8();
            }
            inserted
        }
    };
    match changed {
        true => FieldKey::Changed,
        false => FieldKey::Moved,
    }
}

/// How a widget looks now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    /// The part under the pointer, if the pointer is over it.
    pub hovered: Option<usize>,
    /// The part pressed with the pointer inside it, or the key held on it.
    pub pressed: Option<usize>,
    /// Whether it has the focus in a window that has the keyboard.
    pub focused: bool,
    pub enabled: bool,
    /// Whether a focused text field shows its caret now.
    pub caret_shown: bool,
}

/// The part number of a scroll bar's thumb.
pub const THUMB: usize = usize::MAX - 1;
/// The part number of a horizontal scroll bar's thumb.
pub const THUMB_X: usize = usize::MAX - 2;

fn ring(canvas: &mut Canvas<'_>, rect: Rect, inset: i32, colour: u32) {
    canvas.outline(rect.inset(inset), &INSET_R4, 2, colour);
}

/// Draws the 2 px focus ring `inset` inside `rect`.
pub fn focus_ring(canvas: &mut Canvas<'_>, rect: Rect, inset: i32) {
    let palette = canvas.palette();
    ring(canvas, rect, inset, palette.focus_ring);
}

/// Draws `words` as a label of `size` in `rect`, aligned by `align`, in the exact colour
/// `colour` (a pixel, not a token: what a foreign app's own label colour needs).
pub fn label_text(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    words: &str,
    colour: u32,
    size: i32,
    align: Align,
) {
    let width = text::width(words) * size;
    let x = match align {
        Align::Start => rect.x,
        Align::Centre => rect.x + (rect.width - width) / 2,
        Align::End => rect.right() - width,
    }
    .max(rect.x);
    let y = rect.y + (rect.height - LINE_HEIGHT * size) / 2;
    line(canvas, x, y, rect.right() - x, words, colour, size);
}

/// Draws `words` from (`x`, `y`) in at most `space` pixels, cut with `...` when they do not fit.
pub fn clipped_text(canvas: &mut Canvas<'_>, x: i32, y: i32, space: i32, words: &str, colour: u32) {
    line(canvas, x, y, space, words, colour, 1);
}

fn centred_text(canvas: &mut Canvas<'_>, rect: Rect, words: &str, colour: u32) {
    let (end, ellipsis) = text::fit(words, rect.width);
    let shown = &words[..end];
    let width = text::width(shown)
        + if ellipsis {
            text::width(text::ELLIPSIS)
        } else {
            0
        };
    let x = rect.x + (rect.width - width) / 2;
    let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
    let after = canvas.text(x, y, shown, colour, 1);
    if ellipsis {
        canvas.text(after, y, text::ELLIPSIS, colour, 1);
    }
}

fn line(canvas: &mut Canvas<'_>, x: i32, y: i32, space: i32, words: &str, colour: u32, size: i32) {
    let (end, ellipsis) = text::fit(words, space / size.max(1));
    let after = canvas.text(x, y, &words[..end], colour, size);
    if ellipsis {
        canvas.text(after, y, text::ELLIPSIS, colour, size);
    }
}

fn scroll_bar_look(canvas: &mut Canvas<'_>, rect: Rect, content: i32, offset: i32, look: Look) {
    let palette = canvas.palette();
    if let Some((track, thumb)) = scroll_bar(rect, content, offset) {
        canvas.fill(track, palette.progress_track);
        let active = look.hovered == Some(THUMB) || look.pressed == Some(THUMB);
        let colour = if active {
            palette.button_inactive
        } else {
            palette.bar_pressed
        };
        canvas.rounded(thumb, &INSET_R4, colour);
    }
}

fn scroll_bars_look(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    content: (i32, i32),
    offset: (i32, i32),
    look: Look,
) {
    let palette = canvas.palette();
    let (vertical, horizontal) = scroll_bars(rect, content, offset);
    for (bar, part) in [(vertical, THUMB), (horizontal, THUMB_X)] {
        let Some((track, thumb)) = bar else {
            continue;
        };
        canvas.fill(track, palette.progress_track);
        let active = look.hovered == Some(part) || look.pressed == Some(part);
        let colour = match active {
            true => palette.button_inactive,
            false => palette.bar_pressed,
        };
        canvas.rounded(thumb, &INSET_R4, colour);
    }
}

fn mark_fill(palette: &Palette, checked: bool, look: Look, part: usize) -> u32 {
    let pressed = look.pressed == Some(part);
    let hovered = look.hovered == Some(part);
    match (checked, look.enabled) {
        (true, false) => palette.button,
        (true, true) if pressed => palette.button_primary_pressed,
        (true, true) if hovered => palette.button_primary_hover,
        (true, true) => palette.button_primary,
        (false, true) if pressed => palette.button_pressed,
        (false, _) => palette.field,
    }
}

fn mark_border(palette: &Palette, look: Look, part: usize) -> u32 {
    match look.enabled && look.hovered == Some(part) && look.pressed.is_none() {
        true => palette.text_dim,
        false => palette.button_border,
    }
}

const TICK: [(i32, i32); 9] = [
    (3, 8),
    (4, 9),
    (5, 10),
    (6, 9),
    (7, 8),
    (8, 7),
    (9, 6),
    (10, 5),
    (11, 4),
];

fn paint_mark(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    label: &str,
    round: bool,
    checked: bool,
    look: Look,
    part: usize,
) {
    let palette = canvas.palette();
    let mark = Rect::new(
        rect.x,
        rect.y + (rect.height - MARK_SIZE) / 2,
        MARK_SIZE,
        MARK_SIZE,
    );
    let insets: &[i32] = if round { &INSET_R8 } else { &SQUARE };
    canvas.rounded(mark, insets, mark_fill(palette, checked, look, part));
    let ink = if look.enabled {
        palette.icon_dark
    } else {
        palette.text_dim
    };
    if !checked || !look.enabled {
        canvas.outline(mark, insets, 1, mark_border(palette, look, part));
    }
    if checked {
        match round {
            true => canvas.rounded(Rect::new(mark.x + 5, mark.y + 5, 6, 6), &INSET_R3, ink),
            false => {
                for (x, y) in TICK {
                    canvas.fill(Rect::new(mark.x + x, mark.y + y, 2, 2), ink);
                }
            }
        }
    }
    let text_colour = if look.enabled {
        palette.text
    } else {
        palette.text_dim
    };
    line(
        canvas,
        rect.x + LABEL_OFFSET,
        rect.y + (rect.height - LINE_HEIGHT) / 2,
        rect.width - LABEL_OFFSET,
        label,
        text_colour,
        1,
    );
}

fn paint_button(canvas: &mut Canvas<'_>, rect: Rect, words: &str, primary: bool, look: Look) {
    let palette = canvas.palette();
    let hovered = look.enabled && look.hovered.is_some();
    let pressed = look.enabled && look.pressed.is_some();
    let (fill, ink) = match (primary && look.enabled, pressed, hovered) {
        (true, true, _) => (palette.button_primary_pressed, palette.icon_dark),
        (true, false, true) => (palette.button_primary_hover, palette.icon_dark),
        (true, false, false) => (palette.button_primary, palette.icon_dark),
        (false, true, _) => (palette.button_pressed, palette.text),
        (false, false, true) => (palette.button_hover, palette.text),
        (false, false, false) => (palette.button, palette.text),
    };
    let ink = if look.enabled { ink } else { palette.text_dim };
    canvas.rounded(rect, &INSET_R4, fill);
    if !primary || !look.enabled {
        canvas.outline(rect, &INSET_R4, 1, palette.button_border);
    }
    centred_text(
        canvas,
        Rect::new(
            rect.x + BUTTON_PADDING,
            rect.y,
            rect.width - 2 * BUTTON_PADDING,
            rect.height,
        ),
        words,
        ink,
    );
    if look.focused {
        let ring_colour = if primary && look.enabled {
            palette.icon_dark
        } else {
            palette.focus_ring
        };
        ring(canvas, rect, 2, ring_colour);
    }
}

fn paint_cards(canvas: &mut Canvas<'_>, kind: &Kind, rect: Rect, look: Look) {
    let palette = canvas.palette();
    let Kind::Cards {
        labels,
        count,
        selected,
        ring: ringed,
        art,
        ..
    } = kind
    else {
        return;
    };
    for (index, label) in labels[..*count].iter().enumerate() {
        let card = card_rect(kind, rect, index);
        let chosen = index == *selected;
        let fill = match () {
            () if chosen => palette.selection,
            () if look.enabled && look.pressed == Some(index) => palette.button_pressed,
            () if look.enabled && look.hovered == Some(index) && look.pressed.is_none() => {
                palette.button_hover
            }
            () => palette.button,
        };
        canvas.rounded(card, &INSET_R6, fill);
        match chosen {
            true => canvas.outline(card, &INSET_R6, 2, palette.accent),
            false => canvas.outline(card, &INSET_R6, 1, palette.button_border),
        }
        art(canvas, index, card);
        let label_width = text::width(label);
        canvas.text(
            card.x + (card.width - label_width) / 2,
            card.bottom() - CARD_LABEL_FROM_BOTTOM,
            label,
            if look.enabled {
                palette.text
            } else {
                palette.text_dim
            },
            1,
        );
        if look.focused && index == *ringed {
            ring(canvas, card, if chosen { 4 } else { 2 }, palette.focus_ring);
        }
    }
}

fn paint_list(canvas: &mut Canvas<'_>, list: &List, rect: Rect, look: Look) {
    let palette = canvas.palette();
    let previous = canvas.narrow(rect);
    let mark_y = (list.row_height - LINE_HEIGHT) / 2;
    let mut any = false;
    for (row, item) in list.shown_items().enumerate() {
        any = true;
        let row_rect = row_rect(list, rect, row);
        if row_rect.intersection(&canvas.clip()).is_empty() {
            continue;
        }
        let chosen = item == list.selected;
        if chosen {
            canvas.fill(row_rect, palette.selection);
        } else if look.hovered == Some(item) {
            canvas.fill(row_rect, palette.menu_hover);
        }
        if chosen {
            canvas.text(
                row_rect.x + list.mark_x,
                row_rect.y + mark_y,
                ">",
                palette.accent,
                1,
            );
        }
        line(
            canvas,
            row_rect.x + list.text_x,
            row_rect.y + mark_y,
            row_rect.width - list.text_x - 8,
            list.items[item],
            palette.text,
            1,
        );
        if chosen && look.focused {
            ring(canvas, row_rect, 0, palette.focus_ring);
        }
    }
    if !any {
        canvas.text(
            rect.x + 16,
            rect.y + mark_y,
            list.empty_text,
            palette.text_dim,
            1,
        );
    }
    scroll_bar_look(canvas, rect, list.content_height(), list.offset, look);
    canvas.restore(previous);
}

fn paint_field(canvas: &mut Canvas<'_>, field: &Field, rect: Rect, look: Look) {
    let palette = canvas.palette();
    canvas.rounded(rect, &INSET_R4, palette.field);
    let border = match look.enabled && look.hovered.is_some() && !look.focused {
        true => palette.text_dim,
        false => palette.button_border,
    };
    canvas.outline(rect, &INSET_R4, 1, border);
    let inner = field_inner(rect);
    let previous = canvas.narrow(inner);
    let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
    let (words, ink) = match field.text.is_empty() {
        true => (field.placeholder, palette.text_dim),
        false => (field.text.as_str(), palette.text),
    };
    let ink = if look.enabled { ink } else { palette.text_dim };
    let (start, end) = field.selection();
    if look.focused && start != end && !field.text.is_empty() {
        let text = field.text.as_str();
        let left = inner.x - field.scroll + text::width(&text[..start]);
        let right = inner.x - field.scroll + text::width(&text[..end]);
        canvas.fill(
            Rect::new(left, y, right - left, LINE_HEIGHT),
            palette.selection,
        );
    }
    canvas.text(inner.x - field.scroll, y, words, ink, 1);
    canvas.restore(previous);
    if look.focused && look.caret_shown && look.enabled {
        let caret = caret_rect(field, rect).intersection(&rect.inset(1));
        canvas.fill(caret, palette.text);
    }
    if look.focused {
        ring(canvas, rect, 2, palette.focus_ring);
    }
}

/// A dropdown's options and its popup (docs/ux/toolkit-v0.4.md section 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dropdown {
    /// Its name for agents: the words of the label before it.
    pub name: &'static str,
    pub options: &'static [&'static str],
    pub selected: usize,
    /// Whether its popup is open.
    pub open: bool,
    /// The popup's row the keys and the pointer are on.
    pub highlighted: usize,
    /// The popup's first row shown.
    pub offset: usize,
}

impl Dropdown {
    /// The popup's rectangle for a dropdown at `rect` in a window `window` big, and how many
    /// rows it shows: below the dropdown when it fits, else above, else below and cut.
    pub fn popup(&self, rect: Rect, window: (i32, i32)) -> (Rect, usize) {
        let mut rows = self.options.len().min(POPUP_ROWS);
        let height = |rows: usize| 2 + rows as i32 * POPUP_ROW_HEIGHT;
        let width = (rect.width + 2 * POPUP_OUTSET).min(window.0);
        let x = (rect.x - POPUP_OUTSET).clamp(0, (window.0 - width).max(0));
        let below = rect.bottom() + POPUP_GAP;
        let y = if below + height(rows) <= window.1 {
            below
        } else if rect.y - POPUP_GAP - height(rows) >= 0 {
            rect.y - POPUP_GAP - height(rows)
        } else {
            rows = ((window.1 - rect.bottom() - 6) / POPUP_ROW_HEIGHT).max(1) as usize;
            below
        };
        (Rect::new(x, y, width, height(rows)), rows)
    }

    /// The option at popup row `row` from the top of the rows shown.
    pub fn option_at_row(&self, row: usize) -> Option<usize> {
        let option = self.offset + row;
        (option < self.options.len()).then_some(option)
    }

    /// The scroll that keeps `option` among `rows` rows shown.
    pub fn offset_showing(&self, option: usize, rows: usize) -> usize {
        match option {
            option if option < self.offset => option,
            option if option >= self.offset + rows => option + 1 - rows,
            _ => self.offset,
        }
    }

    /// The next option after `from` whose text starts with `letter`, ignoring case, cycling.
    pub fn next_starting_with(&self, from: usize, letter: char) -> Option<usize> {
        let count = self.options.len();
        (1..=count)
            .map(|step| (from + step) % count)
            .find(|&index| {
                self.options[index]
                    .chars()
                    .next()
                    .is_some_and(|first| first.eq_ignore_ascii_case(&letter))
            })
    }
}

fn chevron(canvas: &mut Canvas<'_>, left: i32, top: i32, down: bool, colour: u32) {
    for index in 0..9 {
        let from_tip = 4 - (index - 4i32).abs();
        let y = match down {
            true => top + from_tip,
            false => top + 4 - from_tip,
        };
        canvas.fill(Rect::new(left + index, y, 2, 2), colour);
    }
}

fn paint_menu_button(canvas: &mut Canvas<'_>, button: &menu::Button, rect: Rect, look: Look) {
    let palette = canvas.palette();
    let fill = match (
        look.enabled,
        button.open || look.pressed.is_some(),
        look.hovered,
    ) {
        (false, _, _) => palette.button,
        (true, true, _) => palette.button_pressed,
        (true, false, Some(_)) => palette.button_hover,
        (true, false, None) => palette.button,
    };
    let ink = match look.enabled {
        true => palette.text,
        false => palette.text_dim,
    };
    canvas.rounded(rect, &INSET_R4, fill);
    canvas.outline(rect, &INSET_R4, 1, palette.button_border);
    let x = rect.x + BUTTON_PADDING;
    line(
        canvas,
        x,
        rect.y + (rect.height - LINE_HEIGHT) / 2,
        rect.right() - DROPDOWN_ARROW_ROOM - x,
        button.text.as_str(),
        ink,
        1,
    );
    chevron(canvas, rect.right() - 22, rect.y + 11, true, ink);
    if look.focused {
        ring(canvas, rect, 2, palette.focus_ring);
    }
}

/// Draws the sort arrow of a table column, 10 by 6 with its top-left at (`left`, `top`):
/// pointing up for ascending.
pub fn sort_arrow(canvas: &mut Canvas<'_>, left: i32, top: i32, ascending: bool, colour: u32) {
    chevron(canvas, left, top, !ascending, colour);
}

fn paint_dropdown(canvas: &mut Canvas<'_>, dropdown: &Dropdown, rect: Rect, look: Look) {
    let palette = canvas.palette();
    let fill = match (
        look.enabled,
        dropdown.open || look.pressed.is_some(),
        look.hovered,
    ) {
        (false, _, _) => palette.button,
        (true, true, _) => palette.button_pressed,
        (true, false, Some(_)) => palette.button_hover,
        (true, false, None) => palette.button,
    };
    let ink = if look.enabled {
        palette.text
    } else {
        palette.text_dim
    };
    canvas.rounded(rect, &INSET_R4, fill);
    canvas.outline(rect, &INSET_R4, 1, palette.button_border);
    let chosen = dropdown
        .options
        .get(dropdown.selected)
        .copied()
        .unwrap_or("");
    let x = rect.x + DROPDOWN_TEXT_X;
    let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
    line(
        canvas,
        x,
        y,
        rect.right() - DROPDOWN_ARROW_ROOM - x,
        chosen,
        ink,
        1,
    );
    chevron(canvas, rect.right() - 22, rect.y + 11, true, ink);
    if look.focused {
        ring(canvas, rect, 2, palette.focus_ring);
    }
}

/// Draws the open popup of `dropdown`, at `popup` showing `rows` rows, over everything.
pub fn paint_popup(canvas: &mut Canvas<'_>, dropdown: &Dropdown, popup: Rect, rows: usize) {
    let palette = canvas.palette();
    canvas.rounded(popup, &INSET_R6, palette.menu);
    canvas.outline(popup, &INSET_R6, 1, palette.menu_border);
    for row in 0..rows {
        let Some(option) = dropdown.option_at_row(row) else {
            break;
        };
        let row_rect = Rect::new(
            popup.x + 1,
            popup.y + 1 + row as i32 * POPUP_ROW_HEIGHT,
            popup.width - 2,
            POPUP_ROW_HEIGHT,
        );
        if option == dropdown.highlighted {
            canvas.fill(row_rect, palette.menu_hover);
        }
        if option == dropdown.selected {
            canvas.text(
                popup.x + POPUP_MARK_X,
                row_rect.y + 8,
                ">",
                palette.accent,
                1,
            );
        }
        let x = popup.x + POPUP_TEXT_X;
        line(
            canvas,
            x,
            row_rect.y + 8,
            popup.right() - 9 - x,
            dropdown.options[option],
            palette.text,
            1,
        );
    }
}

/// A slider's number and range (docs/ux/toolkit-v0.4.md section 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slider {
    /// Its name for agents: the words of the label before it.
    pub name: &'static str,
    pub value: i32,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    /// Drawn after the number: `""`, `" ms"`, `"/s"`.
    pub unit: &'static str,
    /// Whether the number is drawn at the right.
    pub readout: bool,
}

impl Slider {
    /// `value` clamped to the range and moved to the nearest step, a half rounding up.
    pub fn snapped(&self, value: i32) -> i32 {
        let value = value.clamp(self.min, self.max);
        let steps = (value - self.min + self.step / 2) / self.step;
        (self.min + steps * self.step).min(self.max)
    }

    /// Where its track is in a control at `rect`: all of it but the value and its gap.
    pub fn track(&self, rect: Rect) -> Rect {
        let width = match self.readout {
            true => rect.width - SLIDER_GAP - SLIDER_READOUT,
            false => rect.width,
        };
        Rect::new(rect.x, rect.y, width.max(SLIDER_THUMB), rect.height)
    }

    /// The thumb's centre's x in a control at `rect`.
    pub fn thumb_x(&self, rect: Rect) -> i32 {
        let track = self.track(rect);
        let travel = track.width - SLIDER_THUMB;
        let span = self.max - self.min;
        let moved = match span {
            0 => 0,
            span => (i64::from(self.value - self.min) * i64::from(travel) / i64::from(span)) as i32,
        };
        track.x + SLIDER_THUMB / 2 + moved
    }

    /// The value the thumb's centre at `x` stands for, in a control at `rect`.
    pub fn value_at(&self, rect: Rect, x: i32) -> i32 {
        let track = self.track(rect);
        let travel = i64::from((track.width - SLIDER_THUMB).max(1));
        let moved = i64::from((x - track.x - SLIDER_THUMB / 2).clamp(0, travel as i32));
        let span = i64::from(self.max - self.min);
        let raw = (moved * span * 2 + travel) / (2 * travel);
        self.snapped(self.min + raw as i32)
    }
}

fn paint_slider(canvas: &mut Canvas<'_>, slider: &Slider, rect: Rect, look: Look) {
    let palette = canvas.palette();
    let track = slider.track(rect);
    let bar = Rect::new(
        track.x,
        rect.y + (rect.height - SLIDER_TRACK_HEIGHT) / 2,
        track.width,
        SLIDER_TRACK_HEIGHT,
    );
    canvas.rounded(bar, &INSET_R2, palette.progress_track);
    let thumb_x = slider.thumb_x(rect);
    if look.enabled && thumb_x > bar.x {
        let previous = canvas.narrow(Rect::new(bar.x, bar.y, thumb_x - bar.x, bar.height));
        canvas.rounded(bar, &INSET_R2, palette.accent);
        canvas.restore(previous);
    }
    let thumb = Rect::new(
        thumb_x - SLIDER_THUMB / 2,
        rect.y + (rect.height - SLIDER_THUMB) / 2,
        SLIDER_THUMB,
        SLIDER_THUMB,
    );
    match look.enabled {
        true => {
            let fill = match (look.pressed, look.hovered) {
                (Some(_), _) => palette.button_primary_pressed,
                (None, Some(_)) => palette.button_primary_hover,
                (None, None) => palette.button_primary,
            };
            canvas.rounded(thumb, &INSET_R8, fill);
        }
        false => {
            canvas.rounded(thumb, &INSET_R8, palette.button);
            canvas.outline(thumb, &INSET_R8, 1, palette.text_dim);
        }
    }
    if look.focused {
        ring(
            canvas,
            Rect::new(thumb_x - 13, rect.y + (rect.height - 26) / 2, 26, 26),
            2,
            palette.focus_ring,
        );
    }
    if slider.readout {
        let mut words = crate::text::Text::<32>::new();
        let _ = core::fmt::Write::write_fmt(
            &mut words,
            format_args!("{}{}", slider.value, slider.unit),
        );
        let ink = match look.enabled {
            true => palette.text,
            false => palette.text_dim,
        };
        let width = text::width(words.as_str());
        let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
        line(
            canvas,
            rect.right() - width,
            y,
            width,
            words.as_str(),
            ink,
            1,
        );
    }
}

fn paint_progress(canvas: &mut Canvas<'_>, rect: Rect, permille: u16) {
    let palette = canvas.palette();
    canvas.rounded(rect, &INSET_R4, palette.progress_track);
    let permille = i32::from(permille.min(PROGRESS_DONE));
    let done = rect.width * permille / i32::from(PROGRESS_DONE);
    if done > 0 {
        let previous = canvas.narrow(Rect::new(rect.x, rect.y, done, rect.height));
        canvas.rounded(rect, &INSET_R4, palette.accent);
        canvas.restore(previous);
    }
}

/// Draws widget `kind` at `rect` as `look` says.
pub fn paint(canvas: &mut Canvas<'_>, kind: &Kind, rect: Rect, look: Look) {
    let palette = canvas.palette();
    match kind {
        Kind::Panel { fill, border_right } => {
            if let Some(fill) = fill {
                canvas.fill(rect, palette.get(*fill));
            }
            if let Some(border) = border_right {
                canvas.fill(
                    Rect::new(rect.right() - 1, rect.y, 1, rect.height),
                    palette.get(*border),
                );
            }
        }
        Kind::Column { .. } => {}
        Kind::ScrollView {
            offset,
            content_height,
            offset_x,
            content_width,
        } => scroll_bars_look(
            canvas,
            rect,
            (*content_width, *content_height),
            (*offset_x, *offset),
            look,
        ),
        Kind::Label {
            text: words,
            colour: ink,
            size,
            align,
        } => label_text(
            canvas,
            rect,
            words.as_str(),
            palette.get(*ink),
            *size,
            *align,
        ),
        Kind::Button {
            text: words,
            primary,
        } => paint_button(canvas, rect, words.as_str(), *primary, look),
        Kind::Checkbox {
            text: words,
            checked,
        } => {
            paint_mark(canvas, rect, words.as_str(), false, *checked, look, 0);
            if look.focused {
                let mark_y = rect.y + (rect.height - MARK_SIZE) / 2;
                ring(
                    canvas,
                    Rect::new(rect.x - 5, mark_y - 5, 26, 26),
                    2,
                    palette.focus_ring,
                );
            }
        }
        Kind::Radios {
            options,
            count,
            selected,
            ..
        } => {
            for (index, option) in options[..*count].iter().enumerate() {
                let option_rect = radio_rect(kind, rect, index);
                paint_mark(
                    canvas,
                    option_rect,
                    option,
                    true,
                    index == *selected,
                    look,
                    index,
                );
                if look.focused && index == *selected {
                    let mark_y = option_rect.y + (option_rect.height - MARK_SIZE) / 2;
                    ring(
                        canvas,
                        Rect::new(option_rect.x - 5, mark_y - 5, 26, 26),
                        2,
                        palette.focus_ring,
                    );
                }
            }
        }
        Kind::Cards { .. } => paint_cards(canvas, kind, rect, look),
        Kind::List(list) => paint_list(canvas, list, rect, look),
        Kind::TextField(field) => paint_field(canvas, field, rect, look),
        Kind::Progress { permille } => paint_progress(canvas, rect, *permille),
        Kind::Slider(slider) => paint_slider(canvas, slider, rect, look),
        Kind::Dropdown(dropdown) => paint_dropdown(canvas, dropdown, rect, look),
        Kind::Custom { painter, data } => painter(canvas, rect, *data),
        Kind::Tooltip(_) | Kind::TextArea(_) => {}
        Kind::MenuBar(bar) => bar.paint(canvas, rect, look),
        Kind::MenuButton(button) => paint_menu_button(canvas, button, rect, look),
        Kind::Table(table) => table.paint(canvas, rect, look),
    }
}

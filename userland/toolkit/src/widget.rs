//! The widgets (docs/ux/toolkit-v0.3.md sections 4 to 8): what each holds, the size it asks
//! for, where its parts are, and how it is drawn in each state.

use crate::canvas::{Canvas, INSET_R3, INSET_R4, INSET_R6, INSET_R8, SQUARE};
use crate::colour;
use crate::geometry::Rect;
use crate::input::key;
use crate::text::{self, Text};

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

/// A text field's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub text: Text<255>,
    /// The caret's byte in `text`.
    pub caret: usize,
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
        fill: Option<u32>,
        border_right: Option<u32>,
    },
    /// A container that stacks its children top to bottom, `gap` apart.
    Column { gap: i32 },
    /// A container whose children scroll vertically under its rectangle.
    ScrollView { offset: i32, content_height: i32 },
    /// One line of text, `size` 1 (body) or 2 (heading).
    Label {
        text: Text<64>,
        colour: u32,
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
    /// Something the program draws itself, with a word of its own data.
    Custom { painter: Painter, data: u32 },
}

impl Kind {
    /// A panel filled with `fill`.
    pub const fn panel(fill: u32) -> Self {
        Self::Panel {
            fill: Some(fill),
            border_right: None,
        }
    }

    /// A body label in `colour`.
    pub fn label(text: &str, colour: u32) -> Self {
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
            colour: colour::TEXT,
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
            scroll: 0,
            placeholder,
            escape,
        })
    }

    /// Whether Tab stops on it.
    pub fn is_focusable(&self) -> bool {
        matches!(
            self,
            Self::Button { .. }
                | Self::Checkbox { .. }
                | Self::Radios { .. }
                | Self::Cards { .. }
                | Self::List(_)
                | Self::TextField(_)
        )
    }

    /// Whether the pointer acts on it.
    pub fn is_interactive(&self) -> bool {
        self.is_focusable() || matches!(self, Self::ScrollView { .. })
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

fn ring(canvas: &mut Canvas<'_>, rect: Rect, inset: i32, colour: u32) {
    canvas.outline(rect.inset(inset), &INSET_R4, 2, colour);
}

/// Draws the 2 px focus ring `inset` inside `rect`.
pub fn focus_ring(canvas: &mut Canvas<'_>, rect: Rect, inset: i32) {
    ring(canvas, rect, inset, colour::FOCUS_RING);
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
    if let Some((track, thumb)) = scroll_bar(rect, content, offset) {
        canvas.fill(track, colour::PROGRESS_TRACK);
        let active = look.hovered == Some(THUMB) || look.pressed == Some(THUMB);
        let colour = if active {
            colour::BUTTON_INACTIVE
        } else {
            colour::BAR_PRESSED
        };
        canvas.rounded(thumb, &INSET_R4, colour);
    }
}

fn mark_fill(checked: bool, look: Look, part: usize) -> u32 {
    let pressed = look.pressed == Some(part);
    let hovered = look.hovered == Some(part);
    match (checked, look.enabled) {
        (true, false) => colour::BUTTON,
        (true, true) if pressed => colour::BUTTON_PRIMARY_PRESSED,
        (true, true) if hovered => colour::BUTTON_PRIMARY_HOVER,
        (true, true) => colour::BUTTON_PRIMARY,
        (false, true) if pressed => colour::BUTTON_PRESSED,
        (false, _) => colour::FIELD,
    }
}

fn mark_border(look: Look, part: usize) -> u32 {
    match look.enabled && look.hovered == Some(part) && look.pressed.is_none() {
        true => colour::TEXT_DIM,
        false => colour::BUTTON_BORDER,
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
    let mark = Rect::new(
        rect.x,
        rect.y + (rect.height - MARK_SIZE) / 2,
        MARK_SIZE,
        MARK_SIZE,
    );
    let insets: &[i32] = if round { &INSET_R8 } else { &SQUARE };
    canvas.rounded(mark, insets, mark_fill(checked, look, part));
    let ink = if look.enabled {
        colour::ICON_DARK
    } else {
        colour::TEXT_DIM
    };
    if !checked || !look.enabled {
        canvas.outline(mark, insets, 1, mark_border(look, part));
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
        colour::TEXT
    } else {
        colour::TEXT_DIM
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
    let hovered = look.enabled && look.hovered.is_some();
    let pressed = look.enabled && look.pressed.is_some();
    let (fill, ink) = match (primary && look.enabled, pressed, hovered) {
        (true, true, _) => (colour::BUTTON_PRIMARY_PRESSED, colour::ICON_DARK),
        (true, false, true) => (colour::BUTTON_PRIMARY_HOVER, colour::ICON_DARK),
        (true, false, false) => (colour::BUTTON_PRIMARY, colour::ICON_DARK),
        (false, true, _) => (colour::BUTTON_PRESSED, colour::TEXT),
        (false, false, true) => (colour::BUTTON_HOVER, colour::TEXT),
        (false, false, false) => (colour::BUTTON, colour::TEXT),
    };
    let ink = if look.enabled { ink } else { colour::TEXT_DIM };
    canvas.rounded(rect, &INSET_R4, fill);
    if !primary || !look.enabled {
        canvas.outline(rect, &INSET_R4, 1, colour::BUTTON_BORDER);
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
            colour::ICON_DARK
        } else {
            colour::FOCUS_RING
        };
        ring(canvas, rect, 2, ring_colour);
    }
}

fn paint_cards(canvas: &mut Canvas<'_>, kind: &Kind, rect: Rect, look: Look) {
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
            () if chosen => colour::SELECTION,
            () if look.enabled && look.pressed == Some(index) => colour::BUTTON_PRESSED,
            () if look.enabled && look.hovered == Some(index) && look.pressed.is_none() => {
                colour::BUTTON_HOVER
            }
            () => colour::BUTTON,
        };
        canvas.rounded(card, &INSET_R6, fill);
        match chosen {
            true => canvas.outline(card, &INSET_R6, 2, colour::ACCENT),
            false => canvas.outline(card, &INSET_R6, 1, colour::BUTTON_BORDER),
        }
        art(canvas, index, card);
        let label_width = text::width(label);
        canvas.text(
            card.x + (card.width - label_width) / 2,
            card.bottom() - CARD_LABEL_FROM_BOTTOM,
            label,
            if look.enabled {
                colour::TEXT
            } else {
                colour::TEXT_DIM
            },
            1,
        );
        if look.focused && index == *ringed {
            ring(canvas, card, if chosen { 4 } else { 2 }, colour::FOCUS_RING);
        }
    }
}

fn paint_list(canvas: &mut Canvas<'_>, list: &List, rect: Rect, look: Look) {
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
            canvas.fill(row_rect, colour::SELECTION);
        } else if look.hovered == Some(item) {
            canvas.fill(row_rect, colour::MENU_HOVER);
        }
        if chosen {
            canvas.text(
                row_rect.x + list.mark_x,
                row_rect.y + mark_y,
                ">",
                colour::ACCENT,
                1,
            );
        }
        line(
            canvas,
            row_rect.x + list.text_x,
            row_rect.y + mark_y,
            row_rect.width - list.text_x - 8,
            list.items[item],
            colour::TEXT,
            1,
        );
        if chosen && look.focused {
            ring(canvas, row_rect, 0, colour::FOCUS_RING);
        }
    }
    if !any {
        canvas.text(
            rect.x + 16,
            rect.y + mark_y,
            list.empty_text,
            colour::TEXT_DIM,
            1,
        );
    }
    scroll_bar_look(canvas, rect, list.content_height(), list.offset, look);
    canvas.restore(previous);
}

fn paint_field(canvas: &mut Canvas<'_>, field: &Field, rect: Rect, look: Look) {
    canvas.rounded(rect, &INSET_R4, colour::FIELD);
    let border = match look.enabled && look.hovered.is_some() && !look.focused {
        true => colour::TEXT_DIM,
        false => colour::BUTTON_BORDER,
    };
    canvas.outline(rect, &INSET_R4, 1, border);
    let inner = field_inner(rect);
    let previous = canvas.narrow(inner);
    let y = rect.y + (rect.height - LINE_HEIGHT) / 2;
    let (words, ink) = match field.text.is_empty() {
        true => (field.placeholder, colour::TEXT_DIM),
        false => (field.text.as_str(), colour::TEXT),
    };
    let ink = if look.enabled { ink } else { colour::TEXT_DIM };
    canvas.text(inner.x - field.scroll, y, words, ink, 1);
    canvas.restore(previous);
    if look.focused && look.caret_shown && look.enabled {
        let caret = caret_rect(field, rect).intersection(&rect.inset(1));
        canvas.fill(caret, colour::TEXT);
    }
    if look.focused {
        ring(canvas, rect, 2, colour::FOCUS_RING);
    }
}

fn paint_progress(canvas: &mut Canvas<'_>, rect: Rect, permille: u16) {
    canvas.rounded(rect, &INSET_R4, colour::PROGRESS_TRACK);
    let permille = i32::from(permille.min(PROGRESS_DONE));
    let done = rect.width * permille / i32::from(PROGRESS_DONE);
    if done > 0 {
        let previous = canvas.narrow(Rect::new(rect.x, rect.y, done, rect.height));
        canvas.rounded(rect, &INSET_R4, colour::ACCENT);
        canvas.restore(previous);
    }
}

/// Draws widget `kind` at `rect` as `look` says.
pub fn paint(canvas: &mut Canvas<'_>, kind: &Kind, rect: Rect, look: Look) {
    match kind {
        Kind::Panel { fill, border_right } => {
            if let Some(fill) = fill {
                canvas.fill(rect, *fill);
            }
            if let Some(border) = border_right {
                canvas.fill(Rect::new(rect.right() - 1, rect.y, 1, rect.height), *border);
            }
        }
        Kind::Column { .. } => {}
        Kind::ScrollView {
            offset,
            content_height,
        } => scroll_bar_look(canvas, rect, *content_height, *offset, look),
        Kind::Label {
            text: words,
            colour: ink,
            size,
            align,
        } => {
            let words = words.as_str();
            let width = text::width(words) * size;
            let x = match align {
                Align::Start => rect.x,
                Align::Centre => rect.x + (rect.width - width) / 2,
                Align::End => rect.right() - width,
            }
            .max(rect.x);
            let y = rect.y + (rect.height - LINE_HEIGHT * size) / 2;
            line(canvas, x, y, rect.right() - x, words, *ink, *size);
        }
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
                    colour::FOCUS_RING,
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
                        colour::FOCUS_RING,
                    );
                }
            }
        }
        Kind::Cards { .. } => paint_cards(canvas, kind, rect, look),
        Kind::List(list) => paint_list(canvas, list, rect, look),
        Kind::TextField(field) => paint_field(canvas, field, rect, look),
        Kind::Progress { permille } => paint_progress(canvas, rect, *permille),
        Kind::Custom { painter, data } => painter(canvas, rect, *data),
    }
}

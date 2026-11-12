//! The retained widget tree of one window: its widgets in a fixed array (parents before their
//! children, so nothing is allocated), their layout, which one the pointer is over, which has
//! the focus, what input does to them (docs/ux/toolkit-v0.3.md section 3), and what changed
//! since the window was last drawn.

use crate::canvas::Canvas;
use crate::damage::Damage;
use crate::geometry::Rect;
use crate::input::{Input, Modifiers, MouseButton, key};
use crate::text::Text;
use crate::text_area::Store;
use crate::theme::{self, Palette};
use crate::widget::{
    self, DropMode, Field, FieldKey, FormFactor, Horizontal, Kind, Length, List, Look, Place,
    SortOrder, THUMB, THUMB_X, WHEEL_STEP, card_rect, caret_rect, radio_rect, scroll_bar,
};

/// A widget's place in its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WidgetId(u16);

impl WidgetId {
    /// Its index in the tree.
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// What an input did that the program may act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    /// Nothing the program needs to know.
    Nothing,
    /// A push button was activated.
    Activated(WidgetId),
    /// A checkbox changed to this.
    Toggled(WidgetId, bool),
    /// A radio group, card picker or list selected this option, card or item.
    Chosen(WidgetId, usize),
    /// A list's item was opened: a double-click or Enter.
    Opened(WidgetId, usize),
    /// An outline's row asks to open (`true`) or close: change the rows and call
    /// [`WidgetTree::set_rows`] (D401).
    RowToggled(WidgetId, usize, bool),
    /// A text field's text changed.
    Edited(WidgetId),
    /// A slider's value changed to this, by a key, a click or a step of a drag (D199).
    Adjusted(WidgetId, i32),
    /// A text field's selection was copied: give [`WidgetTree::copied`] to the clipboard.
    Copied(WidgetId),
    /// A text field's selection was copied, then removed: give [`WidgetTree::copied`] to the
    /// clipboard, and treat it as [`Response::Edited`].
    Cut(WidgetId),
    /// A text field wants the clipboard's text: answer with [`WidgetTree::paste`].
    PasteWanted(WidgetId),
    /// A key no widget used, for the window: its code and modifiers.
    Unused { code: u16, shift: bool },
    /// A menu bar's or menu button's item of this id was chosen (D219).
    Invoked(WidgetId, u8),
    /// A table's header was clicked: it is now sorted by this column this way; reorder the
    /// rows and call [`WidgetTree::set_rows`], or refuse with [`WidgetTree::set_sort`].
    Sorted(WidgetId, usize, SortOrder),
    /// A table's column was resized to this width by a drag that just ended.
    ColumnResized(WidgetId, usize, i32),
    /// A drag of this source's part (a row, or 0 for a whole widget) started: call
    /// [`WidgetTree::cancel_drag`] to refuse it.
    DragStarted(WidgetId, usize),
    /// A drag of this source's part left the window, and the source lets drags leave it
    /// ([`WidgetTree::set_drag_out`]): hand it to the server with
    /// [`crate::window::WindowServer::start_drag`] (D394).
    DragLeftWindow(WidgetId, usize),
    /// A drag from another window was let go on `target` at `at` (as [`Response::Dropped`]):
    /// read its payload of `kind` with [`crate::window::WindowServer::drag_payload`] (D394).
    ForeignDropped {
        target: WidgetId,
        at: usize,
        kind: u32,
        serial: u32,
    },
    /// The window's drag that left it ended, `landed` on a window or not.
    CarriedEnded { landed: bool },
    /// The drag of `source`'s `part` was let go over `target` at `at`: the row for
    /// [`DropMode::Onto`] (0 for a whole widget), the insertion index for
    /// [`DropMode::Between`].
    Dropped {
        source: WidgetId,
        part: usize,
        target: WidgetId,
        at: usize,
    },
    /// The drag of this source ended without a drop.
    DragCancelled(WidgetId),
}

/// The pointer shape a window shows over its content (docs/windows.md, `SET_POINTER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Arrow,
    IBeam,
    /// Over a table's column handle (the server's shape 3).
    ResizeEW,
    /// While a drag is in progress (the server's shape 7).
    Hand,
}

mod drags;
mod elements;
mod handles;
mod menus;
mod outlines;
mod tables;
mod text_areas;
mod tooltips;

pub use elements::element_of;

/// The element id of a window's content: the root of its element tree (D161).
pub const ROOT_ELEMENT: u32 = 1;

const CARET_BLINK_MS: u32 = 500;
const DOUBLE_CLICK_MS: u32 = 400;
const LEFT_SHIFT: u16 = key::LEFT_SHIFT;
const RIGHT_SHIFT: u16 = key::RIGHT_SHIFT;
const DAMAGE_OUTSET: i32 = 5;

#[derive(Debug, Clone, Copy)]
struct Node {
    kind: Kind,
    parent: Option<WidgetId>,
    place: Place,
    rect: Rect,
    visible: Rect,
    shown: bool,
    enabled: bool,
    person_only: bool,
    drag_source: bool,
    drag_out: bool,
    drop: Option<DropMode>,
    drag_text: Text<48>,
    accessible_name: Text<64>,
    accessible_value: Text<64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    widget: WidgetId,
    part: usize,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    widget: WidgetId,
    horizontal: bool,
    grab: i32,
    start_offset: i32,
}

/// The widgets of one window, at most `N`, and `A` text stores for its text areas.
#[derive(Debug, Clone)]
pub struct WidgetTree<const N: usize, const A: usize = 0> {
    nodes: [Option<Node>; N],
    count: usize,
    size: (i32, i32),
    focus: Option<WidgetId>,
    hover: Option<Target>,
    pressed: Option<Target>,
    pressed_inside: bool,
    key_pressed: Option<WidgetId>,
    drag: Option<Drag>,
    slide: Option<(WidgetId, i32)>,
    selecting: Option<WidgetId>,
    handles: Option<handles::Handles>,
    popup: Option<WidgetId>,
    popup_pressed: bool,
    clicks: u8,
    shift_held: bool,
    shortcut_held: bool,
    alt_held: bool,
    copied: Text<COPIED_BYTES>,
    window_focused: bool,
    caret_epoch_ms: u32,
    caret_shown: bool,
    dead_key: Option<char>,
    last_click: Option<(Target, u32)>,
    default_button: Option<WidgetId>,
    text_before_focus: Text<255>,
    damage: Damage,
    palette: &'static Palette,
    reduce_motion: bool,
    form: FormFactor,
    global_menu: bool,
    menu: Option<WidgetId>,
    menu_pressed: bool,
    alt_alone: bool,
    tip: tooltips::Tip,
    column: Option<tables::ColumnDrag>,
    pick: Option<drags::Pick>,
    foreign: Option<drags::Foreign>,
    stores: [Store; A],
}

/// The most bytes a copy or a cut hands to the clipboard.
pub const COPIED_BYTES: usize = 4096;

impl<const N: usize, const A: usize> Default for WidgetTree<N, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const A: usize> WidgetTree<N, A> {
    /// An empty tree for a window that has the keyboard focus.
    pub const fn new() -> Self {
        Self {
            nodes: [None; N],
            count: 0,
            size: (0, 0),
            focus: None,
            hover: None,
            pressed: None,
            pressed_inside: false,
            key_pressed: None,
            drag: None,
            slide: None,
            selecting: None,
            handles: None,
            popup: None,
            popup_pressed: false,
            clicks: 0,
            shift_held: false,
            shortcut_held: false,
            alt_held: false,
            copied: Text::new(),
            window_focused: true,
            caret_epoch_ms: 0,
            caret_shown: true,
            dead_key: None,
            last_click: None,
            default_button: None,
            text_before_focus: Text::new(),
            damage: Damage::new(),
            palette: &theme::DARK,
            reduce_motion: false,
            form: FormFactor::Desktop,
            global_menu: false,
            menu: None,
            menu_pressed: false,
            alt_alone: false,
            tip: tooltips::Tip::NONE,
            column: None,
            pick: None,
            foreign: None,
            stores: [Store::EMPTY; A],
        }
    }

    /// The palette it draws with.
    pub fn palette(&self) -> &'static Palette {
        self.palette
    }

    /// Draws with `palette` from now on: the whole window is drawn again.
    pub fn set_palette(&mut self, palette: &'static Palette) {
        if !core::ptr::eq(self.palette, palette) {
            self.palette = palette;
            self.damage_all();
        }
    }

    /// Whether nothing blinks (`accessibility.reduce_motion`, D224): the caret is drawn steady.
    pub fn set_reduce_motion(&mut self, reduce_motion: bool) {
        self.reduce_motion = reduce_motion;
        if reduce_motion && !self.caret_shown {
            self.caret_shown = true;
            self.damage_caret();
        }
    }

    /// Tells it the device class it is on (docs/ux/toolkit-v0.5.md section 11).
    pub fn set_form_factor(&mut self, form: FormFactor) {
        self.form = form;
    }

    /// Adds `kind` under `parent` (a container added before it, or the window when `None`; a
    /// tooltip under the widget it names, replacing one it had) at `place`; `None` when the
    /// tree is full, `parent` holds no widgets, or a text area's store is missing or taken.
    pub fn add(&mut self, parent: Option<WidgetId>, place: Place, kind: Kind) -> Option<WidgetId> {
        if self.count >= N || self.count > usize::from(u16::MAX) {
            return None;
        }
        let tooltip = matches!(kind, Kind::Tooltip(_));
        if let Some(parent) = parent
            && !self.node(parent)?.kind.is_container()
            && !tooltip
        {
            return None;
        }
        if tooltip && parent.is_none() {
            return None;
        }
        if let Kind::TextArea(area) = &kind {
            let shared = self.ids().any(|id| {
                matches!(self.kind(id), Some(Kind::TextArea(other)) if other.store == area.store)
            });
            if area.store >= A || shared {
                return None;
            }
        }
        if tooltip && let Some(parent) = parent {
            let previous = self.ids().find(|&id| {
                self.node(id).is_some_and(|node| {
                    node.parent == Some(parent) && matches!(node.kind, Kind::Tooltip(_))
                })
            });
            if let Some(previous) = previous {
                if let Some(node) = self.node_mut(previous) {
                    node.kind = kind;
                }
                self.layout();
                return Some(previous);
            }
        }
        let id = WidgetId(self.count as u16);
        self.nodes[self.count] = Some(Node {
            kind,
            parent,
            place,
            rect: Rect::default(),
            visible: Rect::default(),
            shown: true,
            enabled: true,
            person_only: false,
            drag_source: false,
            drop: None,
            drag_text: Text::new(),
            drag_out: false,
            accessible_name: Text::new(),
            accessible_value: Text::new(),
        });
        self.count += 1;
        if let Kind::TextArea(area) = kind {
            self.stores[area.store] = Store::EMPTY;
        }
        Some(id)
    }

    fn node(&self, id: WidgetId) -> Option<&Node> {
        self.nodes.get(id.index())?.as_ref()
    }

    fn node_mut(&mut self, id: WidgetId) -> Option<&mut Node> {
        self.nodes.get_mut(id.index())?.as_mut()
    }

    fn ids(&self) -> impl DoubleEndedIterator<Item = WidgetId> + use<N, A> {
        (0..self.count as u16).map(WidgetId)
    }

    /// What widget `id` is and holds.
    pub fn kind(&self, id: WidgetId) -> Option<&Kind> {
        self.node(id).map(|node| &node.kind)
    }

    /// Changes what widget `id` holds with `change`, then lays the tree out again and draws it
    /// again where it is.
    pub fn change(&mut self, id: WidgetId, change: impl FnOnce(&mut Kind)) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        change(&mut node.kind);
        let before = node.rect;
        self.damage_widget(id);
        self.layout();
        if self.node(id).is_some_and(|node| node.rect != before) {
            self.damage.add(before.inset(-DAMAGE_OUTSET));
            self.damage_widget(id);
        }
    }

    /// Replaces the text of label, button, checkbox, tooltip, text field or text area `id`.
    pub fn set_text(&mut self, id: WidgetId, words: &str) {
        if self.text_of(id) == Some(words) {
            return;
        }
        if let Some(Kind::TextArea(area)) = self.kind(id).copied() {
            self.stores[area.store].set(words);
            let length = self.stores[area.store].len();
            self.edit_area(id, 0, |area, _| {
                area.caret = length;
                area.anchor = length;
                area.wanted_x = None;
                false
            });
            self.rewrap(id);
            return;
        }
        self.change(id, |kind| match kind {
            Kind::Label { text, .. } => text.set(words),
            Kind::Button { text, .. } | Kind::Checkbox { text, .. } => text.set(words),
            Kind::Tooltip(tip) => tip.text.set(words),
            Kind::TextField(field) => {
                field.text.set(words);
                field.caret = field.text.len();
                field.anchor = field.caret;
                field.scroll = 0;
            }
            _ => {}
        });
        self.keep_caret_visible(id);
    }

    /// Stands cards `id` in a column `gap` apart, each the widget's width (or back in a row
    /// `gap` apart), as a narrow page asks.
    pub fn set_cards_column(&mut self, id: WidgetId, column: bool, gap: i32) {
        self.change(id, |kind| {
            if let Kind::Cards {
                column: standing,
                gap: apart,
                ..
            } = kind
            {
                *standing = column;
                *apart = gap;
            }
        });
        self.layout();
    }

    /// Gives list `id` rows `height` tall, as a window that changes its form asks.
    pub fn set_row_height(&mut self, id: WidgetId, height: i32) {
        if matches!(self.kind(id), Some(Kind::List(list)) if list.row_height == height) {
            return;
        }
        self.change(id, |kind| {
            if let Kind::List(list) = kind {
                list.row_height = height;
            }
        });
        self.layout();
    }

    /// Shows progress bar `id` with `permille` of it done.
    pub fn set_progress(&mut self, id: WidgetId, permille: u16) {
        if matches!(self.kind(id), Some(Kind::Progress { permille: shown }) if *shown == permille) {
            return;
        }
        self.change(id, |kind| {
            if let Kind::Progress { permille: shown } = kind {
                *shown = permille;
            }
        });
    }

    /// The text of label, button, checkbox or text field `id`.
    pub fn text_of(&self, id: WidgetId) -> Option<&str> {
        match self.kind(id)? {
            Kind::Label { text, .. } => Some(text.as_str()),
            Kind::Button { text, .. } | Kind::Checkbox { text, .. } => Some(text.as_str()),
            Kind::Tooltip(tip) => Some(tip.text.as_str()),
            Kind::TextField(field) => Some(field.text.as_str()),
            Kind::TextArea(area) => self.stores.get(area.store).map(Store::text),
            _ => None,
        }
    }

    /// The selected option, card or item of radio group, card picker or list `id`, or whether
    /// checkbox `id` is checked (1) or not (0).
    pub fn selected(&self, id: WidgetId) -> Option<usize> {
        match self.kind(id)? {
            Kind::Radios { selected, .. } | Kind::Cards { selected, .. } => Some(*selected),
            Kind::List(list) => Some(list.selected),
            Kind::Dropdown(dropdown) => Some(dropdown.selected),
            Kind::Table(table) => Some(table.selected),
            Kind::Outline(outline) => Some(outline.selected),
            Kind::Checkbox { checked, .. } => Some(usize::from(*checked)),
            _ => None,
        }
    }

    /// Selects option, card or item `index` of `id` (a list's item must be shown), or checks
    /// checkbox `id` when `index` is not 0; without a response.
    pub fn set_selected(&mut self, id: WidgetId, index: usize) {
        if self.selected(id) == Some(index) {
            return;
        }
        self.change(id, |kind| match kind {
            Kind::Radios {
                selected, count, ..
            } if index < *count => *selected = index,
            Kind::Cards {
                selected,
                ring,
                count,
                ..
            } if index < *count => {
                *selected = index;
                *ring = index;
            }
            Kind::List(list) if list.row_of(index).is_some() => list.selected = index,
            Kind::Dropdown(dropdown) if index < dropdown.options.len() => {
                dropdown.selected = index;
                dropdown.highlighted = index;
            }
            Kind::Checkbox { checked, .. } => *checked = index != 0,
            Kind::Table(table) if index < table.rows => table.selected = index,
            Kind::Outline(outline) if index < outline.rows => outline.selected = index,
            _ => {}
        });
        self.keep_selection_visible(id);
        match self.kind(id) {
            Some(Kind::Table(_)) => self.keep_row_visible(id),
            Some(Kind::Outline(_)) => self.keep_outline_row_visible(id),
            _ => {}
        }
    }

    /// Shows only the items of list `id` whose bit is set in `shown`; when the selected one
    /// is hidden, the first shown becomes selected. Whether the selection changed.
    pub fn set_shown_items(&mut self, id: WidgetId, shown: u32) -> bool {
        let mut changed = false;
        self.change(id, |kind| {
            if let Kind::List(list) = kind {
                list.shown = shown;
                list.offset = 0;
                let first = list.shown_items().next();
                if list.row_of(list.selected).is_none()
                    && let Some(first) = first
                {
                    list.selected = first;
                    changed = true;
                }
            }
        });
        changed
    }

    /// Shows or hides widget `id` and what it holds.
    pub fn set_shown(&mut self, id: WidgetId, shown: bool) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        if node.shown == shown {
            return;
        }
        node.shown = shown;
        let rect = node.rect;
        self.damage.add(rect.inset(-DAMAGE_OUTSET));
        if !shown {
            self.widget_went(id);
        }
        if !shown && self.focus.is_some_and(|focus| self.is_within(focus, id)) {
            self.focus = None;
        }
        self.layout();
    }

    /// Moves widget `id` (and what it holds) to `place` in its parent and lays the tree out
    /// again, for a window whose widgets keep to its bottom or change places with its size.
    pub fn set_place(&mut self, id: WidgetId, place: Place) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        if node.place == place {
            return;
        }
        node.place = place;
        let before = node.rect;
        self.damage.add(before.inset(-DAMAGE_OUTSET));
        self.layout();
        self.damage_widget(id);
    }

    /// Enables or disables widget `id`: a disabled one ignores the pointer and Tab.
    pub fn set_enabled(&mut self, id: WidgetId, enabled: bool) {
        if let Some(node) = self.node_mut(id)
            && node.enabled != enabled
        {
            node.enabled = enabled;
            self.damage_widget(id);
            if !enabled {
                self.widget_disabled(id);
            }
        }
    }

    /// Whether widget `id` is enabled.
    pub fn is_enabled(&self, id: WidgetId) -> bool {
        self.node(id).is_some_and(|node| node.enabled)
    }

    /// Marks widget `id` as one only a person operates (D193): [`Self::act`] refuses every
    /// agent action on it and `wm show` lists it `person-only`, without its value or, for a
    /// label, its text; pointer and keys still work.
    pub fn set_person_only(&mut self, id: WidgetId, person_only: bool) {
        if let Some(node) = self.node_mut(id) {
            node.person_only = person_only;
        }
    }

    /// Gives widget `id` the name an agent and a screen reader read for it instead of its
    /// drawn text (docs/ux/components-v0.3.md section 11): `Install Windows apps` for a button
    /// that draws `Install`; at most 64 bytes, and `""` for its drawn text again.
    pub fn set_accessible_name(&mut self, id: WidgetId, name: &str) {
        if let Some(node) = self.node_mut(id) {
            node.accessible_name = Text::from(name);
        }
    }

    /// Gives widget `id` the value an agent and a screen reader read for it instead of the
    /// one its kind gives; at most 64 bytes, and `""` for its kind's value again.
    pub fn set_accessible_value(&mut self, id: WidgetId, value: &str) {
        if let Some(node) = self.node_mut(id) {
            node.accessible_value = Text::from(value);
        }
    }

    /// Whether widget `id` is one only a person operates.
    pub fn is_person_only(&self, id: WidgetId) -> bool {
        self.node(id).is_some_and(|node| node.person_only)
    }

    /// Makes push button `id` the window's default: Enter reaches it from widgets that do not
    /// use Enter.
    pub fn set_default_button(&mut self, id: WidgetId) {
        self.default_button = Some(id);
    }

    fn is_within(&self, id: WidgetId, ancestor: WidgetId) -> bool {
        let mut at = Some(id);
        while let Some(current) = at {
            if current == ancestor {
                return true;
            }
            at = self.node(current).and_then(|node| node.parent);
        }
        false
    }

    /// Whether widget `id` and every widget holding it are shown.
    pub fn is_shown(&self, id: WidgetId) -> bool {
        let mut at = Some(id);
        while let Some(current) = at {
            match self.node(current) {
                Some(node) if node.shown => at = node.parent,
                _ => return false,
            }
        }
        true
    }

    /// Where widget `id` is in the window.
    pub fn rect(&self, id: WidgetId) -> Option<Rect> {
        self.node(id).map(|node| node.rect)
    }

    /// The window's size at scale 1.
    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// Lays the widgets out in a window `width` by `height` at scale 1 and draws it all again.
    pub fn resize(&mut self, width: i32, height: i32) {
        let resized = self.size != (width, height);
        self.size = (width, height);
        self.layout();
        if resized {
            self.hide_tooltip();
            self.close_menu();
        }
        for id in self.ids() {
            if matches!(self.kind(id), Some(Kind::TextArea(_))) {
                self.rewrap(id);
            }
        }
        self.damage.clear();
        self.damage.add(Rect::new(0, 0, width, height));
    }

    fn layout(&mut self) {
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let mut cursors = [0i32; N];
        let mut content_bottoms = [i32::MIN; N];
        let mut content_rights = [i32::MIN; N];
        for index in 0..self.count {
            let Some(node) = self.nodes[index] else {
                continue;
            };
            let (parent_rect, parent_visible, parent_kind) = match node.parent {
                Some(parent) => match self.node(parent) {
                    Some(parent) => (parent.rect, parent.visible, Some(parent.kind)),
                    None => (window, window, None),
                },
                None => (window, window, None),
            };
            if let Kind::Tooltip(tooltip) = &node.kind {
                let owner = parent_rect.intersection(&parent_visible);
                let rect = widget::tooltip_rect(tooltip, owner, self.size);
                if let Some(node) = self.nodes[index].as_mut() {
                    node.rect = rect;
                    node.visible = window;
                }
                continue;
            }
            let (content_width, content_height) = match node.kind {
                Kind::MenuBar(_) if self.global_menu => (0, 0),
                kind => kind.content_size(),
            };
            let offset_x = match node.place.horizontal {
                Horizontal::Left(x) => x,
                Horizontal::Right(margin) => margin,
                Horizontal::Centre => 0,
            };
            let width = match node.place.width {
                Length::Px(width) => width,
                Length::Fill(margin) => parent_rect.width - offset_x - margin,
                Length::Content => content_width,
            }
            .max(0);
            let scrolled_x = match parent_kind {
                Some(Kind::ScrollView { offset_x, .. }) => offset_x,
                _ => 0,
            };
            let x = match node.place.horizontal {
                Horizontal::Left(x) => parent_rect.x + x,
                Horizontal::Right(margin) => parent_rect.right() - margin - width,
                Horizontal::Centre => parent_rect.x + (parent_rect.width - width) / 2,
            } - scrolled_x;
            let column_gap = match parent_kind {
                Some(Kind::Column { gap }) => Some(gap),
                _ => None,
            };
            let top = match (node.parent, parent_kind) {
                (Some(parent), Some(Kind::Column { .. })) => {
                    cursors[parent.index()] + node.place.top
                }
                (_, Some(Kind::ScrollView { offset, .. })) => node.place.top - offset,
                _ => node.place.top,
            };
            let height = match (node.kind, node.place.height) {
                (Kind::MenuBar(_), _) if self.global_menu => 0,
                (_, Length::Px(height)) => height,
                (_, Length::Fill(margin)) => parent_rect.height - top - margin,
                (_, Length::Content) => content_height,
            }
            .max(0);
            if let (Some(parent), Some(gap)) = (node.parent, column_gap)
                && node.shown
            {
                cursors[parent.index()] = top + height + gap;
            }
            let rect = Rect::new(x, parent_rect.y + top, width, height);
            if let Some(parent) = node.parent
                && node.shown
            {
                let bottom = rect.bottom() - parent_rect.y;
                let held = &mut content_bottoms[parent.index()];
                *held = (*held).max(bottom);
                let right = match node.place.width {
                    Length::Fill(_) => 0,
                    _ => rect.right() - parent_rect.x,
                };
                let held = &mut content_rights[parent.index()];
                *held = (*held).max(right);
            }
            let visible = match node.kind {
                Kind::ScrollView { .. }
                | Kind::List(_)
                | Kind::Table(_)
                | Kind::Outline(_)
                | Kind::TextArea(_) => rect.intersection(&parent_visible),
                _ => parent_visible,
            };
            if let Some(node) = self.nodes[index].as_mut() {
                node.rect = rect;
                node.visible = visible;
            }
        }
        for ((node, bottom), right) in self
            .nodes
            .iter_mut()
            .zip(content_bottoms)
            .zip(content_rights)
            .take(self.count)
        {
            if let Some(node) = node.as_mut()
                && let Kind::ScrollView {
                    offset,
                    content_height,
                    offset_x,
                    content_width,
                } = &mut node.kind
            {
                *content_height = match bottom {
                    i32::MIN => 0,
                    bottom => bottom + *offset,
                };
                *content_width = match right {
                    i32::MIN => 0,
                    right => right + *offset_x,
                };
            }
        }
        self.clamp_scrolls();
    }

    fn clamp_scrolls(&mut self) {
        let mut moved = false;
        for index in 0..self.count {
            if let Some(node) = self.nodes[index].as_mut() {
                let rect = node.rect;
                match &mut node.kind {
                    Kind::ScrollView {
                        offset,
                        content_height,
                        offset_x,
                        content_width,
                    } => {
                        let view = widget::viewport(rect, (*content_width, *content_height));
                        let clamped = (*offset).clamp(0, (*content_height - view.height).max(0));
                        let clamped_x = (*offset_x).clamp(0, (*content_width - view.width).max(0));
                        moved |= clamped != *offset || clamped_x != *offset_x;
                        *offset = clamped;
                        *offset_x = clamped_x;
                    }
                    Kind::List(list) => {
                        list.offset = list
                            .offset
                            .clamp(0, (list.content_height() - rect.height).max(0));
                    }
                    Kind::Table(table) => table.clamp_offsets(rect),
                    Kind::Outline(outline) => outline.clamp_offsets(rect),
                    _ => {}
                }
            }
        }
        if moved {
            self.layout();
        }
    }

    fn shows_dead_key(&self, id: WidgetId) -> bool {
        self.dead_key.is_some()
            && self.focus == Some(id)
            && self.window_focused
            && self.menu_keyboard().is_none()
            && self
                .node(id)
                .is_some_and(|node| node.enabled && node.kind.is_text())
    }

    fn show_dead_key(&mut self, accent: Option<char>) {
        let shown = accent.filter(|_| {
            self.focus
                .and_then(|id| self.node(id))
                .is_some_and(|node| node.kind.is_text())
        });
        if shown == self.dead_key {
            return;
        }
        self.dead_key = shown;
        if let Some(id) = self.focus {
            self.damage_widget(id);
        }
    }

    fn resolve_dead_key(&mut self, code: u16) {
        if self.dead_key.is_some() && !key::is_modifier_or_lock(code) {
            self.show_dead_key(None);
        }
    }

    /// The accent a dead key left waiting, which the focused text field or text area shows at
    /// its caret until the next key resolves it (D221).
    pub fn dead_key(&self) -> Option<char> {
        self.dead_key
    }

    fn damage_widget(&mut self, id: WidgetId) {
        if let Some(node) = self.node(id) {
            let rect = node
                .rect
                .inset(-DAMAGE_OUTSET)
                .intersection(&node.visible.inset(-DAMAGE_OUTSET));
            self.damage.add(rect);
        }
    }

    /// What changed since the last call, at scale 1: draw it with [`Self::paint`] and commit
    /// it.
    pub fn take_damage(&mut self) -> Damage {
        self.damage.take()
    }

    /// Asks for the whole window to be drawn again.
    pub fn damage_all(&mut self) {
        self.damage.add(Rect::new(0, 0, self.size.0, self.size.1));
    }

    fn look(&self, id: WidgetId) -> Look {
        let Some(node) = self.node(id) else {
            return Look::default();
        };
        let hovered = self
            .hover
            .filter(|target| {
                target.widget == id && self.pressed.is_none_or(|pressed| pressed.widget == id)
            })
            .map(|target| target.part);
        let pressed = match (self.pressed, self.key_pressed) {
            (Some(target), _) if target.widget == id && self.pressed_inside => Some(target.part),
            (_, Some(widget)) if widget == id => Some(0),
            _ => None,
        };
        let thumb = self
            .drag
            .filter(|drag| drag.widget == id)
            .map(|drag| if drag.horizontal { THUMB_X } else { THUMB })
            .or(self
                .slide
                .filter(|(widget, _)| *widget == id)
                .map(|_| THUMB));
        let column = self
            .column
            .filter(|column| column.table == id)
            .map(|column| crate::table::HANDLE_PART + column.column);
        Look {
            hovered: self.landing_hover(id).or(hovered),
            pressed: column.or(thumb).or(pressed),
            focused: self.window_focused
                && self.focus == Some(id)
                && self.menu_keyboard().is_none(),
            enabled: node.enabled,
            caret_shown: self.caret_shown && !self.shows_dead_key(id),
        }
    }

    /// Draws every shown widget that meets the canvas's clip, parents first, then what floats
    /// over them: a drop's highlight, a dropdown's popup, an open menu, a tooltip and a drag's
    /// ghost, in the tree's palette.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
        let outer = canvas.palette();
        canvas.set_palette(self.palette);
        let clip = canvas.clip();
        for id in self.ids() {
            let Some(node) = self.node(id) else {
                continue;
            };
            if !self.is_shown(id) {
                continue;
            }
            let visible = node.visible.intersection(&clip);
            let reach = node.rect.inset(-DAMAGE_OUTSET).intersection(&visible);
            if reach.is_empty() {
                continue;
            }
            let previous = canvas.narrow(visible);
            match &node.kind {
                Kind::TextArea(area) => {
                    if let Some(store) = self.stores.get(area.store) {
                        crate::text_area::paint(canvas, area, store, node.rect, self.look(id));
                    }
                }
                kind => widget::paint(canvas, kind, node.rect, self.look(id)),
            }
            if let Some(accent) = self.dead_key.filter(|_| self.shows_dead_key(id)) {
                match &node.kind {
                    Kind::TextField(field) => {
                        widget::paint_dead_key(canvas, field, node.rect, accent)
                    }
                    Kind::TextArea(area) => {
                        if let Some(store) = self.stores.get(area.store) {
                            crate::text_area::paint_dead_key(
                                canvas, area, store, node.rect, accent,
                            );
                        }
                    }
                    _ => {}
                }
            }
            canvas.restore(previous);
        }
        self.paint_landing(canvas, clip);
        if let Some((_, dropdown, popup, rows)) = self.popup_of() {
            let previous = canvas.narrow(popup.intersection(&clip));
            widget::paint_popup(canvas, &dropdown, popup, rows);
            canvas.restore(previous);
        }
        self.paint_handles(canvas, clip);
        self.paint_menu(canvas, clip);
        self.paint_tooltip(canvas, clip);
        self.paint_ghost(canvas, clip);
        canvas.set_palette(outer);
    }

    fn popup_of(&self) -> Option<(WidgetId, widget::Dropdown, Rect, usize)> {
        let id = self.popup?;
        let Some(Kind::Dropdown(dropdown)) = self.kind(id).copied() else {
            return None;
        };
        let (popup, rows) = dropdown.popup(self.rect(id)?, self.size);
        Some((id, dropdown, popup, rows))
    }

    fn open_popup(&mut self, id: WidgetId) {
        let Some(rect) = self.rect(id) else {
            return;
        };
        let size = self.size;
        self.change(id, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                let (_, rows) = dropdown.popup(rect, size);
                dropdown.open = true;
                dropdown.highlighted = dropdown.selected;
                dropdown.offset = dropdown.offset_showing(dropdown.selected, rows);
            }
        });
        self.popup = Some(id);
        if let Some((_, _, popup, _)) = self.popup_of() {
            self.damage.add(popup);
        }
    }

    fn close_popup(&mut self) {
        let Some((id, _, popup, _)) = self.popup_of() else {
            self.popup = None;
            return;
        };
        self.damage.add(popup);
        self.popup = None;
        self.popup_pressed = false;
        self.change(id, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.open = false;
            }
        });
    }

    pub(crate) fn choose_option(&mut self, id: WidgetId, option: usize) -> Response {
        self.close_popup();
        let Some(Kind::Dropdown(dropdown)) = self.kind(id).copied() else {
            return Response::Nothing;
        };
        if option == dropdown.selected || option >= dropdown.options.len() {
            return Response::Nothing;
        }
        self.change(id, |kind| {
            if let Kind::Dropdown(dropdown) = kind {
                dropdown.selected = option;
                dropdown.highlighted = option;
            }
        });
        Response::Chosen(id, option)
    }

    fn highlight(&mut self, option: usize) {
        let Some((id, dropdown, popup, rows)) = self.popup_of() else {
            return;
        };
        let option = option.min(dropdown.options.len().saturating_sub(1));
        if option == dropdown.highlighted {
            return;
        }
        if let Some(node) = self.node_mut(id)
            && let Kind::Dropdown(held) = &mut node.kind
        {
            held.highlighted = option;
            held.offset = held.offset_showing(option, rows);
        }
        self.damage.add(popup);
    }

    fn popup_option_at(&self, x: i32, y: i32) -> Option<usize> {
        let (_, dropdown, popup, rows) = self.popup_of()?;
        if !popup.contains(x, y) {
            return None;
        }
        let row = ((y - popup.y - 1) / widget::POPUP_ROW_HEIGHT).max(0) as usize;
        (row < rows).then(|| dropdown.option_at_row(row)).flatten()
    }

    fn popup_key(&mut self, id: WidgetId, dropdown: widget::Dropdown, code: u16, character: u32) {
        let last = dropdown.options.len().saturating_sub(1);
        let page = self
            .popup_of()
            .map_or(1, |(_, _, _, rows)| rows.saturating_sub(1).max(1));
        let here = dropdown.highlighted;
        let next = match code {
            key::UP => here.saturating_sub(1),
            key::DOWN => (here + 1).min(last),
            key::HOME => 0,
            key::END => last,
            key::PAGE_UP => here.saturating_sub(page),
            key::PAGE_DOWN => (here + page).min(last),
            _ => match char::from_u32(character).filter(char::is_ascii_alphanumeric) {
                Some(letter) => dropdown.next_starting_with(here, letter).unwrap_or(here),
                None => return,
            },
        };
        let _ = id;
        self.highlight(next);
    }

    /// Draws the whole window into `pixels`, `width` by `height` at `scale`: what a test
    /// compares with a screen.
    pub fn render(&self, pixels: &mut [u32], width: u32, height: u32, scale: u32) {
        if let Some(mut canvas) = Canvas::new(pixels, width, height, scale) {
            self.paint(&mut canvas);
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Target> {
        let id = self.ids().rev().find(|&id| {
            self.node(id).is_some_and(|node| {
                node.kind.is_interactive()
                    && node.enabled
                    && node.rect.contains(x, y)
                    && node.visible.contains(x, y)
            }) && self.is_shown(id)
        })?;
        let node = self.node(id)?;
        let part = match &node.kind {
            Kind::Radios { count, .. } => (0..*count)
                .find(|&index| radio_rect(&node.kind, node.rect, index).contains(x, y))?,
            Kind::Cards { count, .. } => {
                (0..*count).find(|&index| card_rect(&node.kind, node.rect, index).contains(x, y))?
            }
            Kind::List(list) => match scroll_bar(node.rect, list.content_height(), list.offset) {
                Some((track, _)) if track.contains(x, y) => THUMB,
                _ => {
                    let row = (y - node.rect.y + list.offset) / list.row_height;
                    list.item_at_row(row.max(0) as usize)?
                }
            },
            Kind::ScrollView {
                offset,
                content_height,
                offset_x,
                content_width,
            } => match widget::scroll_bars(
                node.rect,
                (*content_width, *content_height),
                (*offset_x, *offset),
            ) {
                (Some((track, _)), _) if track.contains(x, y) => THUMB,
                (_, Some((track, _))) if track.contains(x, y) => THUMB_X,
                _ => return None,
            },
            Kind::Table(table) => table.part_at(node.rect, x, y)?,
            Kind::Outline(outline) => outline.part_at(node.rect, x, y)?,
            Kind::MenuBar(bar) => bar.title_at(node.rect, x, y)?,
            Kind::TextArea(_) => match self.bar_of(id, false) {
                Some((track, _)) if track.contains(x, y) => THUMB,
                _ => 0,
            },
            _ => 0,
        };
        Some(Target { widget: id, part })
    }

    /// The pointer shape the window should show where the pointer is.
    pub fn pointer_shape(&self) -> PointerShape {
        if self.pick.is_some_and(|pick| pick.started) {
            return PointerShape::Hand;
        }
        if self.column.is_some() {
            return PointerShape::ResizeEW;
        }
        let hover = self.hover.filter(|_| self.pressed.is_none());
        match hover.and_then(|target| Some((target, self.kind(target.widget)?))) {
            Some((_, Kind::TextField(_))) => PointerShape::IBeam,
            Some((target, Kind::TextArea(_))) if target.part != THUMB => PointerShape::IBeam,
            Some((target, Kind::Table(_)))
                if (crate::table::HANDLE_PART..crate::table::ROW_HIT).contains(&target.part) =>
            {
                PointerShape::ResizeEW
            }
            _ => PointerShape::Arrow,
        }
    }

    /// The widget with the focus.
    pub fn focus(&self) -> Option<WidgetId> {
        self.focus
    }

    /// Gives widget `id` the focus.
    pub fn set_focus(&mut self, id: Option<WidgetId>, now_ms: u32) {
        if self.focus == id {
            return;
        }
        if let Some(old) = self.focus {
            self.damage_widget(old);
        }
        self.focus = id;
        self.dead_key = None;
        if let Some(new) = id {
            if let Some(Kind::TextField(field)) = self.kind(new) {
                self.text_before_focus = field.text;
            }
            self.damage_widget(new);
        }
        self.restart_caret(now_ms);
    }

    fn focusable(&self, id: WidgetId) -> bool {
        self.node(id)
            .is_some_and(|node| node.kind.is_focusable() && node.enabled)
            && self.is_shown(id)
    }

    /// Moves the focus to the next focusable widget in the order they were added, or the
    /// previous one when `back`, wrapping at the ends.
    pub fn move_focus(&mut self, back: bool, now_ms: u32) {
        let count = self.count;
        if count == 0 {
            return;
        }
        let start = self
            .focus
            .map_or(if back { 0 } else { count - 1 }, WidgetId::index);
        for step in 1..=count {
            let index = match back {
                false => (start + step) % count,
                true => (start + count * 2 - step) % count,
            };
            let id = WidgetId(index as u16);
            if self.focusable(id) {
                self.set_focus(Some(id), now_ms);
                if self.window_focused {
                    self.tooltip_focus(id, now_ms);
                }
                if matches!(self.kind(id), Some(Kind::TextField(_))) {
                    self.edit_field(id, now_ms, |field| {
                        field.select_all();
                        false
                    });
                }
                return;
            }
        }
    }

    fn restart_caret(&mut self, now_ms: u32) {
        self.caret_epoch_ms = now_ms;
        if !self.caret_shown {
            self.caret_shown = true;
            self.damage_caret();
        }
    }

    fn damage_caret(&mut self) {
        let Some(id) = self.focus else {
            return;
        };
        match self.node(id).map(|node| (node.kind, node.rect)) {
            Some((Kind::TextField(field), rect)) => {
                let caret = caret_rect(&field, rect);
                self.damage.add(caret.inset(-1));
            }
            Some((Kind::TextArea(area), rect)) => {
                if let Some(store) = self.stores.get(area.store) {
                    let caret = crate::text_area::caret_rect(&area, store, rect);
                    self.damage.add(caret.inset(-1));
                }
            }
            _ => {}
        }
    }

    fn focused_field(&self) -> bool {
        self.window_focused
            && self.focus.is_some_and(|id| {
                matches!(self.kind(id), Some(Kind::TextField(_) | Kind::TextArea(_)))
            })
    }

    /// Blinks the caret, shows and hides tooltips, picks up a long-pressed row and scrolls a
    /// drag's target: call at the time [`Self::next_deadline`] gave, or any time after; what
    /// the program should act on (a long-press's [`Response::DragStarted`]).
    pub fn tick(&mut self, now_ms: u32) -> Response {
        if self.focused_field() {
            let elapsed = now_ms.wrapping_sub(self.caret_epoch_ms);
            let shown = self.reduce_motion || (elapsed / CARET_BLINK_MS).is_multiple_of(2);
            if shown != self.caret_shown {
                self.caret_shown = shown;
                self.damage_caret();
            }
        }
        self.tooltip_tick(now_ms);
        self.handles_tick(now_ms);
        self.pick_tick(now_ms)
    }

    /// When [`Self::tick`] next has something to do, if ever.
    pub fn next_deadline(&self, now_ms: u32) -> Option<u32> {
        let caret = (self.focused_field() && !self.reduce_motion).then(|| {
            let elapsed = now_ms.wrapping_sub(self.caret_epoch_ms);
            now_ms.wrapping_add(CARET_BLINK_MS - elapsed % CARET_BLINK_MS)
        });
        [caret, self.tooltip_deadline(), self.pick_deadline(), self.handles_deadline()]
            .into_iter()
            .flatten()
            .min_by_key(|deadline| deadline.wrapping_sub(now_ms) as i32)
    }

    fn set_hover(&mut self, hover: Option<Target>) {
        if self.hover == hover {
            return;
        }
        if let Some(old) = self.hover {
            self.damage_widget(old.widget);
        }
        self.hover = hover;
        if let Some(new) = hover {
            self.damage_widget(new.widget);
        }
    }

    /// Hands the tree `input`, which happened at `time_ms`: what the program should act on.
    pub fn handle(&mut self, input: Input, time_ms: u32) -> Response {
        match input {
            Input::DragOver { x, y, .. } => {
                self.foreign_over(x, y);
                Response::Nothing
            }
            Input::DragLeave => {
                self.foreign_left();
                Response::Nothing
            }
            Input::Drop { x, y, kind, serial } => self.foreign_dropped(x, y, kind, serial),
            Input::DragEnded { landed } => Response::CarriedEnded { landed },
            Input::PointerMove { x, y } => {
                if let Some(column) = self.column {
                    return self.column_moved(column, x);
                }
                if self.pick.is_some() {
                    return self.pick_moved(x, y, time_ms);
                }
                if self.menu_open() {
                    self.menu_pointer_moved(x, y);
                    return Response::Nothing;
                }
                if self.popup.is_some() {
                    if let Some(option) = self.popup_option_at(x, y) {
                        self.highlight(option);
                    }
                    return Response::Nothing;
                }
                if let Some((id, grab)) = self.slide {
                    return self.slide_to(id, x - grab);
                }
                if self.handles_moved(x, y, time_ms) {
                    return Response::Nothing;
                }
                if let Some(id) = self.selecting {
                    self.select_to(id, x, y, time_ms);
                    return Response::Nothing;
                }
                self.pointer_moved(x, y);
                self.tooltip_hover(time_ms);
                Response::Nothing
            }
            Input::PointerLeave => {
                self.set_hover(None);
                self.tooltip_hover(time_ms);
                if let Some(pressed) = self.pressed
                    && self.pressed_inside
                {
                    self.pressed_inside = false;
                    self.damage_widget(pressed.widget);
                }
                Response::Nothing
            }
            Input::Button {
                x,
                y,
                button: MouseButton::Left,
                pressed,
            } => match pressed {
                true => {
                    self.tooltip_spent();
                    if let Some(cancelled) = self.cancel_pick_on_press() {
                        return cancelled;
                    }
                    self.left_pressed(x, y, time_ms)
                }
                false => self.left_released(x, y, time_ms),
            },
            Input::Button { pressed: true, .. } => {
                self.tooltip_spent();
                match self.cancel_pick_on_press() {
                    Some(cancelled) => cancelled,
                    None => Response::Nothing,
                }
            }
            Input::Button { .. } => Response::Nothing,
            Input::Wheel { x, y, notches } => {
                if self.menu_open() {
                    return Response::Nothing;
                }
                if let Some((id, dropdown, popup, rows)) = self.popup_of() {
                    if popup.contains(x, y) {
                        let most = dropdown.options.len().saturating_sub(rows);
                        let offset = (dropdown.offset as i32 - notches * 2).clamp(0, most as i32);
                        if let Some(node) = self.node_mut(id)
                            && let Kind::Dropdown(held) = &mut node.kind
                        {
                            held.offset = offset as usize;
                        }
                        self.damage.add(popup);
                    } else {
                        self.close_popup();
                    }
                    return Response::Nothing;
                }
                match self.shift_held {
                    true => self.wheel(x, y, -notches, true),
                    false => self.wheel(x, y, notches, false),
                }
                Response::Nothing
            }
            Input::SidewaysWheel { x, y, notches } => {
                if self.popup.is_none() && !self.menu_open() {
                    self.wheel(x, y, notches, true);
                }
                Response::Nothing
            }
            Input::Key {
                code,
                pressed,
                modifiers,
                character,
            } => match pressed {
                true => {
                    self.hide_handles();
                    self.resolve_dead_key(code);
                    self.shift_held = modifiers.shift() || matches!(code, LEFT_SHIFT | RIGHT_SHIFT);
                    self.shortcut_held = modifiers.control_or_super();
                    self.alt_held = modifiers.0 & Modifiers::ALT != 0
                        || matches!(code, key::LEFT_ALT | key::RIGHT_ALT);
                    self.tooltip_key();
                    self.key_down(
                        code,
                        modifiers.shift(),
                        modifiers.command(),
                        character,
                        time_ms,
                    )
                }
                false => {
                    if matches!(code, LEFT_SHIFT | RIGHT_SHIFT) {
                        self.shift_held = false;
                    }
                    if matches!(code, key::LEFT_ALT | key::RIGHT_ALT) {
                        self.alt_held = false;
                        if core::mem::take(&mut self.alt_alone) {
                            self.arm_menu_bar();
                            return Response::Nothing;
                        }
                    }
                    self.key_up(code)
                }
            },
            Input::DeadKey { accent } => {
                self.show_dead_key(accent);
                Response::Nothing
            }
            Input::FocusIn | Input::FocusOut => {
                let focused = input == Input::FocusIn;
                let mut response = Response::Nothing;
                if !focused {
                    self.show_dead_key(None);
                    self.close_popup();
                    self.close_menu();
                    self.hide_tooltip();
                    self.alt_alone = false;
                    response = self.cancel_drag_response();
                    self.end_column_drag(true);
                }
                if self.window_focused != focused {
                    self.window_focused = focused;
                    if let Some(id) = self.focus {
                        self.damage_widget(id);
                    }
                    self.restart_caret(time_ms);
                }
                response
            }
            Input::CloseRequest
            | Input::Resize { .. }
            | Input::StateChanged { .. }
            | Input::Overflow => Response::Nothing,
        }
    }

    fn pointer_moved(&mut self, x: i32, y: i32) {
        if let Some(drag) = self.drag {
            self.drag_thumb(drag, if drag.horizontal { x } else { y });
            return;
        }
        let target = self.hit(x, y);
        if let Some(pressed) = self.pressed {
            let inside = target == Some(pressed);
            if inside != self.pressed_inside {
                self.pressed_inside = inside;
                self.damage_widget(pressed.widget);
            }
        }
        self.set_hover(target);
    }

    fn bar_of(&self, id: WidgetId, horizontal: bool) -> Option<(Rect, Rect)> {
        let node = self.node(id)?;
        match (&node.kind, horizontal) {
            (Kind::List(list), false) => scroll_bar(node.rect, list.content_height(), list.offset),
            (
                Kind::ScrollView {
                    offset,
                    content_height,
                    offset_x,
                    content_width,
                },
                _,
            ) => {
                let bars = widget::scroll_bars(
                    node.rect,
                    (*content_width, *content_height),
                    (*offset_x, *offset),
                );
                if horizontal { bars.1 } else { bars.0 }
            }
            (Kind::Table(table), _) => {
                let bars = table.bars(node.rect);
                if horizontal { bars.1 } else { bars.0 }
            }
            (Kind::Outline(outline), false) => outline.bars(node.rect).0,
            (Kind::TextArea(area), false) => {
                let store = self.stores.get(area.store)?;
                let rect = node.rect;
                let content = store.line_count() as i32 * widget::TEXT_AREA_LINE_PITCH
                    + 2 * widget::TEXT_AREA_TOP;
                scroll_bar(
                    Rect::new(
                        rect.right() - 1 - widget::SCROLL_BAR_WIDTH,
                        rect.y + 1,
                        widget::SCROLL_BAR_WIDTH,
                        rect.height - 2,
                    ),
                    content,
                    area.scroll,
                )
            }
            _ => None,
        }
    }

    fn scroll_range(&self, id: WidgetId, horizontal: bool) -> Option<(i32, i32)> {
        let node = self.node(id)?;
        let rect = node.rect;
        match (&node.kind, horizontal) {
            (Kind::List(list), false) => {
                Some((list.offset, (list.content_height() - rect.height).max(0)))
            }
            (
                Kind::ScrollView {
                    offset,
                    content_height,
                    offset_x,
                    content_width,
                },
                _,
            ) => {
                let view = widget::viewport(rect, (*content_width, *content_height));
                Some(match horizontal {
                    true => (*offset_x, (*content_width - view.width).max(0)),
                    false => (*offset, (*content_height - view.height).max(0)),
                })
            }
            (Kind::Outline(outline), false) => {
                let view = outline.view(rect);
                Some((
                    outline.offset_y,
                    (outline.content_height() - view.height).max(0),
                ))
            }
            (Kind::Table(table), _) => {
                let view = table.view(rect);
                Some(match horizontal {
                    true => (table.offset_x, (table.content_width() - view.width).max(0)),
                    false => (
                        table.offset_y,
                        (table.content_height() - view.height).max(0),
                    ),
                })
            }
            (Kind::TextArea(area), false) => {
                let store = self.stores.get(area.store)?;
                let hidden = store
                    .line_count()
                    .saturating_sub(crate::text_area::visible_lines(rect));
                Some((area.scroll, hidden as i32 * widget::TEXT_AREA_LINE_PITCH))
            }
            _ => None,
        }
    }

    fn scroll_axis_to(&mut self, id: WidgetId, horizontal: bool, offset: i32) {
        let Some((old, most)) = self.scroll_range(id, horizontal) else {
            return;
        };
        let mut offset = offset.clamp(0, most);
        if matches!(self.kind(id), Some(Kind::TextArea(_))) {
            offset = offset / widget::TEXT_AREA_LINE_PITCH * widget::TEXT_AREA_LINE_PITCH;
        }
        if offset == old {
            return;
        }
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let rect = node.rect;
        let is_view = matches!(node.kind, Kind::ScrollView { .. });
        match (&mut node.kind, horizontal) {
            (Kind::List(list), false) => list.offset = offset,
            (Kind::ScrollView { offset: held, .. }, false) => *held = offset,
            (Kind::ScrollView { offset_x: held, .. }, true) => *held = offset,
            (Kind::Table(table), false) => table.offset_y = offset,
            (Kind::Table(table), true) => table.offset_x = offset,
            (Kind::Outline(outline), false) => outline.offset_y = offset,
            (Kind::TextArea(area), false) => area.scroll = offset,
            _ => {}
        }
        if is_view {
            self.layout();
        }
        self.damage.add(rect);
    }

    /// Scrolls list, scroll view, table or text area `id` to `offset_x` across and `offset_y`
    /// down, each clamped to what its content allows (a text area by whole lines).
    pub fn set_scroll(&mut self, id: WidgetId, offset_x: i32, offset_y: i32) {
        self.scroll_axis_to(id, true, offset_x);
        self.scroll_axis_to(id, false, offset_y);
    }

    /// How far list, scroll view, table or text area `id` is scrolled: across, then down.
    pub fn scroll(&self, id: WidgetId) -> Option<(i32, i32)> {
        let down = self.scroll_range(id, false).map_or(0, |(offset, _)| offset);
        let across = self.scroll_range(id, true).map_or(0, |(offset, _)| offset);
        self.node(id).map(|_| (across, down))
    }

    fn scroll_to(&mut self, id: WidgetId, offset: i32) {
        self.scroll_axis_to(id, false, offset);
    }

    fn drag_thumb(&mut self, drag: Drag, at: i32) {
        let Some((track, thumb)) = self.bar_of(drag.widget, drag.horizontal) else {
            return;
        };
        let Some((_, most)) = self.scroll_range(drag.widget, drag.horizontal) else {
            return;
        };
        let (track_length, thumb_length) = match drag.horizontal {
            true => (track.width, thumb.width),
            false => (track.height, thumb.height),
        };
        let travel = i64::from((track_length - thumb_length).max(1));
        let moved = i64::from(at - drag.grab) * i64::from(most) / travel;
        let offset = (i64::from(drag.start_offset) + moved).clamp(0, i64::from(most.max(0)));
        self.scroll_axis_to(drag.widget, drag.horizontal, offset as i32);
    }

    fn wheel(&mut self, x: i32, y: i32, notches: i32, horizontal: bool) {
        let scrollable = self.ids().rev().find(|&id| {
            self.is_shown(id)
                && self.node(id).is_some_and(|node| {
                    matches!(
                        node.kind,
                        Kind::List(_)
                            | Kind::ScrollView { .. }
                            | Kind::Table(_)
                            | Kind::Outline(_)
                            | Kind::TextArea(_)
                    ) && node.visible.contains(x, y)
                        && node.rect.contains(x, y)
                })
                && self
                    .scroll_range(id, horizontal)
                    .is_some_and(|(_, most)| most > 0)
        });
        let Some(id) = scrollable else {
            return;
        };
        let Some((offset, _)) = self.scroll_range(id, horizontal) else {
            return;
        };
        let step = match self.kind(id) {
            Some(Kind::TextArea(_)) => widget::TEXT_AREA_WHEEL_LINES * widget::TEXT_AREA_LINE_PITCH,
            _ => WHEEL_STEP,
        };
        let wanted = match horizontal {
            true => offset + notches * step,
            false => offset - notches * step,
        };
        let wanted = match (self.kind(id), horizontal) {
            (Some(Kind::Table(_)), false) => {
                wanted / widget::TABLE_ROW_HEIGHT * widget::TABLE_ROW_HEIGHT
            }
            (Some(Kind::Outline(_)), false) => {
                wanted / crate::outline::OUTLINE_ROW_HEIGHT * crate::outline::OUTLINE_ROW_HEIGHT
            }
            _ => wanted,
        };
        self.scroll_axis_to(id, horizontal, wanted);
    }

    fn left_pressed(&mut self, x: i32, y: i32, time_ms: u32) -> Response {
        if self.menu_open() {
            return self.menu_pressed_at(x, y);
        }
        if self.popup.is_some() {
            match self.popup_option_at(x, y) {
                Some(option) => {
                    self.highlight(option);
                    self.popup_pressed = true;
                }
                None => self.close_popup(),
            }
            return Response::Nothing;
        }
        if self.handles_pressed(x, y) {
            return Response::Nothing;
        }
        let Some(target) = self.hit(x, y) else {
            return Response::Nothing;
        };
        let id = target.widget;
        match self.kind(id).copied() {
            Some(Kind::Dropdown(_)) => {
                if self.focusable(id) {
                    self.set_focus(Some(id), time_ms);
                }
                self.open_popup(id);
                self.popup_pressed = true;
                return Response::Nothing;
            }
            Some(Kind::MenuBar(_)) => return self.bar_title_pressed(id, target.part),
            Some(Kind::MenuButton(_)) => {
                if self.focusable(id) {
                    self.set_focus(Some(id), time_ms);
                }
                self.open_button_menu(id);
                self.menu_pressed = true;
                return Response::Nothing;
            }
            _ => {}
        }
        if self.focusable(id) {
            self.set_focus(Some(id), time_ms);
        }
        if target.part == THUMB || target.part == THUMB_X {
            return self.thumb_pressed(id, target.part == THUMB_X, x, y);
        }
        if let (Some(Kind::Slider(slider)), Some(rect)) = (self.kind(id).copied(), self.rect(id)) {
            let thumb_x = slider.thumb_x(rect);
            let grab = match (x - thumb_x).abs() <= widget::SLIDER_THUMB / 2 {
                true => x - thumb_x,
                false => 0,
            };
            self.slide = Some((id, grab));
            self.damage_widget(id);
            return self.slide_to(id, x - grab);
        }
        let double = self.last_click.is_some_and(|(last, at)| {
            last == target && time_ms.wrapping_sub(at) <= DOUBLE_CLICK_MS
        });
        self.clicks = match double {
            true => self.clicks % 3 + 1,
            false => 1,
        };
        self.last_click = Some((target, time_ms));
        match self.kind(id).copied() {
            Some(Kind::List(list)) => {
                let item = target.part;
                if self.is_drag_source(id) {
                    self.start_pick(id, item, x, y, time_ms, double);
                    self.set_hover(Some(target));
                    return Response::Nothing;
                }
                if double {
                    return Response::Opened(id, item);
                }
                if list.selected != item {
                    self.set_selected(id, item);
                    return Response::Chosen(id, item);
                }
                Response::Nothing
            }
            Some(Kind::Table(_)) => self.table_pressed(id, target.part, x, y, time_ms, double),
            Some(Kind::Outline(_)) => self.outline_pressed(id, target.part, double),
            Some(Kind::TextArea(_)) => {
                self.area_pressed(id, x, y, time_ms);
                self.selecting = Some(id);
                if self.clicks == 1 && !self.shift_held {
                    self.field_tapped(id, x, time_ms);
                }
                Response::Nothing
            }
            Some(Kind::Custom { .. }) if self.is_drag_source(id) => {
                self.start_pick(id, 0, x, y, time_ms, false);
                Response::Nothing
            }
            Some(Kind::TextField(field)) => {
                let clicks = self.clicks;
                let extend = self.shift_held;
                self.place_caret(id, &field, x, time_ms);
                self.edit_field(id, time_ms, |placed| {
                    match clicks {
                        2 => (placed.anchor, placed.caret) = placed.word_at(placed.caret),
                        3 => placed.select_all(),
                        _ if extend => placed.anchor = field.anchor,
                        _ => placed.anchor = placed.caret,
                    }
                    false
                });
                self.selecting = Some(id);
                self.field_tapped(id, x, time_ms);
                Response::Nothing
            }
            _ => {
                self.pressed = Some(target);
                self.pressed_inside = true;
                self.damage_widget(id);
                if self.form == FormFactor::Phone {
                    self.tooltip_long_press(id, time_ms);
                }
                Response::Nothing
            }
        }
    }

    fn thumb_pressed(&mut self, id: WidgetId, horizontal: bool, x: i32, y: i32) -> Response {
        let Some((track, thumb)) = self.bar_of(id, horizontal) else {
            return Response::Nothing;
        };
        let Some((offset, _)) = self.scroll_range(id, horizontal) else {
            return Response::Nothing;
        };
        let (at, thumb_start, thumb_length, track_length) = match horizontal {
            true => (x, thumb.x, thumb.width, track.width),
            false => (y, thumb.y, thumb.height, track.height),
        };
        if (thumb_start..thumb_start + thumb_length).contains(&at) {
            self.drag = Some(Drag {
                widget: id,
                horizontal,
                grab: at,
                start_offset: offset,
            });
            self.damage_widget(id);
            return Response::Nothing;
        }
        let page = track_length - 24;
        let offset = match at < thumb_start {
            true => offset - page,
            false => offset + page,
        };
        self.scroll_axis_to(id, horizontal, offset);
        Response::Nothing
    }

    fn place_caret(&mut self, id: WidgetId, field: &Field, x: i32, now_ms: u32) {
        let Some(rect) = self.rect(id) else {
            return;
        };
        let caret = widget::caret_at(field, rect, x);
        self.edit_field(id, now_ms, |field| {
            field.caret = caret;
            false
        });
    }

    fn select_to(&mut self, id: WidgetId, x: i32, y: i32, now_ms: u32) {
        if matches!(self.kind(id), Some(Kind::TextArea(_))) {
            self.area_select_to(id, x, y, now_ms);
            return;
        }
        let (Some(Kind::TextField(field)), Some(rect)) = (self.kind(id).copied(), self.rect(id))
        else {
            return;
        };
        let caret = widget::caret_at(&field, rect, x);
        if caret != field.caret {
            self.edit_field(id, now_ms, |selected| {
                selected.caret = caret;
                false
            });
        }
    }

    /// The text the last [`Response::Copied`] or [`Response::Cut`] took.
    pub fn copied(&self) -> &str {
        self.copied.as_str()
    }

    /// Puts `text`, the clipboard's, in text field or text area `id` in place of its selection,
    /// as [`Response::PasteWanted`] asked: in a field line breaks and tabs become spaces (an
    /// area keeps them), other control
    /// characters go, and what does not fit is left out. [`Response::Edited`] when the text
    /// changed.
    pub fn paste(&mut self, id: WidgetId, text: &str, now_ms: u32) -> Response {
        if matches!(self.kind(id), Some(Kind::TextArea(_))) {
            return self.area_paste(id, text, now_ms);
        }
        let changed = self.edit_field(id, now_ms, |field| {
            let mut changed = field.remove_selection();
            for character in text.chars() {
                let character = match character {
                    '\n' | '\r' | '\t' => ' ',
                    character if character.is_control() => continue,
                    character => character,
                };
                if !field.text.insert(field.caret, character) {
                    break;
                }
                field.caret += character.len_utf8();
                changed = true;
            }
            field.anchor = field.caret;
            changed
        });
        match changed {
            true => Response::Edited(id),
            false => Response::Nothing,
        }
    }

    fn slide_to(&mut self, id: WidgetId, thumb_x: i32) -> Response {
        let (Some(Kind::Slider(slider)), Some(rect)) = (self.kind(id).copied(), self.rect(id))
        else {
            return Response::Nothing;
        };
        self.adjust(id, slider.value_at(rect, thumb_x))
    }

    pub(crate) fn adjust(&mut self, id: WidgetId, wanted: i32) -> Response {
        let Some(Kind::Slider(slider)) = self.kind(id).copied() else {
            return Response::Nothing;
        };
        let value = slider.snapped(wanted);
        if value == slider.value {
            return Response::Nothing;
        }
        self.set_value(id, value);
        Response::Adjusted(id, value)
    }

    /// A slider's value.
    pub fn value(&self, id: WidgetId) -> Option<i32> {
        match self.kind(id)? {
            Kind::Slider(slider) => Some(slider.value),
            _ => None,
        }
    }

    /// Sets slider `id` to `value`, snapped to its steps and clamped to its range, without a
    /// response.
    pub fn set_value(&mut self, id: WidgetId, value: i32) {
        if !matches!(self.kind(id), Some(Kind::Slider(_))) {
            return;
        }
        self.change(id, |kind| {
            if let Kind::Slider(slider) = kind {
                slider.value = slider.snapped(value);
            }
        });
    }

    fn left_released(&mut self, x: i32, y: i32, time_ms: u32) -> Response {
        self.selecting = None;
        self.handles_released();
        if self.column.is_some() {
            return self.end_column_drag(false);
        }
        if self.pick.is_some() {
            return self.pick_released(x, y, time_ms);
        }
        if self.menu_open() {
            return self.menu_released_at(x, y);
        }
        if let Some(id) = self.popup {
            let pressed = core::mem::take(&mut self.popup_pressed);
            return match (pressed, self.popup_option_at(x, y)) {
                (true, Some(option)) => self.choose_option(id, option),
                _ => Response::Nothing,
            };
        }
        if let Some((id, _)) = self.slide.take() {
            self.damage_widget(id);
            self.set_hover(self.hit(x, y));
            return Response::Nothing;
        }
        if let Some(drag) = self.drag.take() {
            self.damage_widget(drag.widget);
            self.set_hover(self.hit(x, y));
            return Response::Nothing;
        }
        let Some(pressed) = self.pressed.take() else {
            return Response::Nothing;
        };
        if self.tip.spent_on_press() {
            self.pressed_inside = false;
            self.damage_widget(pressed.widget);
            return Response::Nothing;
        }
        let inside = self.pressed_inside && self.hit(x, y) == Some(pressed);
        self.pressed_inside = false;
        self.damage_widget(pressed.widget);
        self.set_hover(self.hit(x, y));
        match (inside, self.kind(pressed.widget)) {
            (true, Some(Kind::Table(_))) => self.header_clicked(pressed.widget, pressed.part),
            (true, _) => self.activate(pressed.widget, pressed.part),
            (false, _) => Response::Nothing,
        }
    }

    fn activate(&mut self, id: WidgetId, part: usize) -> Response {
        match self.kind(id).copied() {
            Some(Kind::Button { .. }) => Response::Activated(id),
            Some(Kind::Checkbox { checked, .. }) => {
                self.set_selected(id, usize::from(!checked));
                Response::Toggled(id, !checked)
            }
            Some(Kind::Radios { selected, .. }) | Some(Kind::Cards { selected, .. }) => {
                if let Some(Kind::Cards { ring, .. }) = self.node_mut(id).map(|node| &mut node.kind)
                {
                    *ring = part;
                }
                if selected == part {
                    self.damage_widget(id);
                    return Response::Nothing;
                }
                self.set_selected(id, part);
                Response::Chosen(id, part)
            }
            _ => Response::Nothing,
        }
    }

    fn key_up(&mut self, code: u16) -> Response {
        if code != key::SPACE {
            return Response::Nothing;
        }
        let Some(id) = self.key_pressed.take() else {
            return Response::Nothing;
        };
        self.damage_widget(id);
        match self.focus == Some(id) {
            true => Response::Activated(id),
            false => Response::Nothing,
        }
    }

    fn key_down(
        &mut self,
        code: u16,
        shift: bool,
        command: bool,
        character: u32,
        time_ms: u32,
    ) -> Response {
        self.alt_alone =
            matches!(code, key::LEFT_ALT | key::RIGHT_ALT) && self.menu_bar().is_some();
        if code == key::ESC && self.column.is_some() {
            return self.end_column_drag(true);
        }
        if code == key::ESC && self.pick.is_some() {
            return self.cancel_drag_response();
        }
        if let Some(response) = self.menu_key(code, character, time_ms) {
            return response;
        }
        if code == key::TAB && !command {
            self.close_popup();
            self.move_focus(shift, time_ms);
            return Response::Nothing;
        }
        if let Some((id, dropdown, _, _)) = self.popup_of() {
            match code {
                key::ESC => self.close_popup(),
                key::ENTER | key::SPACE => return self.choose_option(id, dropdown.highlighted),
                _ => self.popup_key(id, dropdown, code, character),
            }
            return Response::Nothing;
        }
        let unused = Response::Unused { code, shift };
        let Some(id) = self.focus.filter(|&id| self.focusable(id)) else {
            return self.default_or(code, unused);
        };
        let Some(kind) = self.kind(id).copied() else {
            return unused;
        };
        let response = match kind {
            Kind::Button { .. } => match code {
                key::SPACE => {
                    self.key_pressed = Some(id);
                    self.damage_widget(id);
                    Response::Nothing
                }
                key::ENTER => Response::Activated(id),
                _ => unused,
            },
            Kind::Checkbox { .. } if code == key::SPACE => self.activate(id, 0),
            Kind::Dropdown(dropdown) => {
                let last = dropdown.options.len().saturating_sub(1);
                let here = dropdown.selected;
                let next = match code {
                    key::SPACE | key::ENTER => {
                        self.open_popup(id);
                        return Response::Nothing;
                    }
                    key::DOWN | key::UP if self.alt_held => {
                        self.open_popup(id);
                        return Response::Nothing;
                    }
                    key::UP | key::LEFT => here.saturating_sub(1),
                    key::DOWN | key::RIGHT => (here + 1).min(last),
                    key::HOME => 0,
                    key::END => last,
                    _ => match char::from_u32(character).filter(char::is_ascii_alphanumeric) {
                        Some(letter) if !command => {
                            dropdown.next_starting_with(here, letter).unwrap_or(here)
                        }
                        _ => return self.default_or(code, unused),
                    },
                };
                self.choose_option(id, next)
            }
            Kind::Slider(slider) => {
                let page = slider.step * widget::SLIDER_PAGE_STEPS;
                let wanted = match code {
                    key::RIGHT | key::UP => slider.value.saturating_add(slider.step),
                    key::LEFT | key::DOWN => slider.value.saturating_sub(slider.step),
                    key::HOME => slider.min,
                    key::END => slider.max,
                    key::PAGE_UP => slider.value.saturating_add(page),
                    key::PAGE_DOWN => slider.value.saturating_sub(page),
                    _ => return self.default_or(code, unused),
                };
                self.adjust(id, wanted)
            }
            Kind::Radios {
                count, selected, ..
            } => {
                let next = match code {
                    key::UP | key::LEFT => selected.checked_sub(1),
                    key::DOWN | key::RIGHT => Some(selected + 1).filter(|&next| next < count),
                    key::SPACE => None,
                    _ => return self.default_or(code, unused),
                };
                match next {
                    Some(next) => {
                        self.set_selected(id, next);
                        Response::Chosen(id, next)
                    }
                    None => Response::Nothing,
                }
            }
            Kind::Cards { count, ring, .. } => match code {
                key::UP | key::LEFT | key::DOWN | key::RIGHT => {
                    let next = match code {
                        key::UP | key::LEFT => ring.saturating_sub(1),
                        _ => (ring + 1).min(count.saturating_sub(1)),
                    };
                    if let Some(Kind::Cards { ring, .. }) =
                        self.node_mut(id).map(|node| &mut node.kind)
                    {
                        *ring = next;
                    }
                    self.damage_widget(id);
                    Response::Nothing
                }
                key::ENTER | key::SPACE => self.activate(id, ring),
                _ => unused,
            },
            Kind::List(list) => self.list_key(id, &list, code, unused),
            Kind::MenuButton(_) => self.menu_button_key(id, code),
            Kind::Table(table) => self.table_key(id, &table, code, unused),
            Kind::Outline(outline) => self.outline_key(id, &outline, code, character, unused),
            Kind::TextArea(_) => self.area_key(id, code, character, command, time_ms, unused),
            Kind::TextField(_) => self.field_key(id, code, character, command, time_ms, unused),
            _ => unused,
        };
        match response {
            Response::Unused { .. } => self.default_or(code, response),
            other => other,
        }
    }

    fn default_or(&mut self, code: u16, unused: Response) -> Response {
        match (code, self.default_button) {
            (key::ENTER, Some(button)) if self.focusable(button) => Response::Activated(button),
            _ => unused,
        }
    }

    fn list_key(&mut self, id: WidgetId, list: &List, code: u16, unused: Response) -> Response {
        let rows = list.shown_items().count();
        let Some(row) = list.row_of(list.selected) else {
            return unused;
        };
        let page =
            (self.rect(id).map_or(0, |rect| rect.height) / list.row_height - 1).max(1) as usize;
        let last = rows.saturating_sub(1);
        let next = match code {
            key::UP => row.saturating_sub(1),
            key::DOWN => (row + 1).min(last),
            key::HOME => 0,
            key::END => last,
            key::PAGE_UP => row.saturating_sub(page),
            key::PAGE_DOWN => (row + page).min(last),
            key::ENTER => return Response::Opened(id, list.selected),
            _ => return unused,
        };
        let Some(item) = list.item_at_row(next) else {
            return Response::Nothing;
        };
        if item == list.selected {
            return Response::Nothing;
        }
        self.set_selected(id, item);
        Response::Chosen(id, item)
    }

    fn keep_selection_visible(&mut self, id: WidgetId) {
        let Some(node) = self.node(id) else {
            return;
        };
        let Kind::List(list) = &node.kind else {
            return;
        };
        let Some(row) = list.row_of(list.selected) else {
            return;
        };
        let top = row as i32 * list.row_height;
        let bottom = top + list.row_height;
        let height = node.rect.height;
        let offset = match () {
            () if top < list.offset => top,
            () if bottom > list.offset + height => bottom - height,
            () => return,
        };
        self.scroll_to(id, offset);
    }

    fn edit_field(
        &mut self,
        id: WidgetId,
        now_ms: u32,
        edit: impl FnOnce(&mut Field) -> bool,
    ) -> bool {
        let Some(Kind::TextField(field)) = self.node_mut(id).map(|node| &mut node.kind) else {
            return false;
        };
        let changed = edit(field);
        self.damage_widget(id);
        self.keep_caret_visible(id);
        self.restart_caret(now_ms);
        changed
    }

    fn keep_caret_visible(&mut self, id: WidgetId) {
        let Some(rect) = self.rect(id) else {
            return;
        };
        if let Some(Kind::TextField(field)) = self.node_mut(id).map(|node| &mut node.kind) {
            widget::keep_caret_visible(field, rect);
        }
    }

    fn field_key(
        &mut self,
        id: WidgetId,
        code: u16,
        character: u32,
        command: bool,
        now_ms: u32,
        unused: Response,
    ) -> Response {
        let Some(Kind::TextField(mut edited)) = self.kind(id).copied() else {
            return unused;
        };
        let before = self.text_before_focus;
        let selected = Text::<255>::from(edited.selected());
        let key = widget::field_key_with(
            &mut edited,
            code,
            character,
            command,
            self.shift_held,
            self.shortcut_held,
            &before,
        );
        if key == FieldKey::Unused {
            return unused;
        }
        if edited.masked && matches!(key, FieldKey::Copied | FieldKey::Cut) {
            return Response::Nothing;
        }
        if key == FieldKey::Cut {
            edited.remove_selection();
        }
        self.edit_field(id, now_ms, |field| {
            *field = edited;
            false
        });
        match key {
            FieldKey::Changed => Response::Edited(id),
            FieldKey::Copied => {
                self.copied = Text::from(selected.as_str());
                Response::Copied(id)
            }
            FieldKey::Cut => {
                self.copied = Text::from(selected.as_str());
                Response::Cut(id)
            }
            FieldKey::PasteWanted => Response::PasteWanted(id),
            _ => Response::Nothing,
        }
    }
}

//! The retained widget tree of one window: its widgets in a fixed array (parents before their
//! children, so nothing is allocated), their layout, which one the pointer is over, which has
//! the focus, what input does to them (docs/ux/toolkit-v0.3.md section 3), and what changed
//! since the window was last drawn.

use crate::canvas::Canvas;
use crate::damage::Damage;
use crate::geometry::Rect;
use crate::input::{Input, MouseButton, key};
use crate::text::{self, Text};
use crate::widget::{
    self, Escape, Field, Horizontal, Kind, Length, List, Look, Place, THUMB, WHEEL_STEP, card_rect,
    caret_rect, field_inner, radio_rect, scroll_bar,
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
    /// A text field's text changed.
    Edited(WidgetId),
    /// A key no widget used, for the window: its code and modifiers.
    Unused { code: u16, shift: bool },
}

/// The pointer shape a window shows over its content (docs/windows.md, `SET_POINTER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Arrow,
    IBeam,
}

const CARET_BLINK_MS: u32 = 500;
const DOUBLE_CLICK_MS: u32 = 400;
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    widget: WidgetId,
    part: usize,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    widget: WidgetId,
    grab_y: i32,
    start_offset: i32,
}

/// The widgets of one window, at most `N`.
#[derive(Debug, Clone)]
pub struct WidgetTree<const N: usize> {
    nodes: [Option<Node>; N],
    count: usize,
    size: (i32, i32),
    focus: Option<WidgetId>,
    hover: Option<Target>,
    pressed: Option<Target>,
    pressed_inside: bool,
    key_pressed: Option<WidgetId>,
    drag: Option<Drag>,
    window_focused: bool,
    caret_epoch_ms: u32,
    caret_shown: bool,
    last_click: Option<(Target, u32)>,
    default_button: Option<WidgetId>,
    text_before_focus: Text<255>,
    damage: Damage,
}

impl<const N: usize> Default for WidgetTree<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> WidgetTree<N> {
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
            window_focused: true,
            caret_epoch_ms: 0,
            caret_shown: true,
            last_click: None,
            default_button: None,
            text_before_focus: Text::new(),
            damage: Damage::new(),
        }
    }

    /// Adds `kind` under `parent` (a container added before it, or the window when `None`) at
    /// `place`; `None` when the tree is full or `parent` holds no widgets.
    pub fn add(&mut self, parent: Option<WidgetId>, place: Place, kind: Kind) -> Option<WidgetId> {
        if self.count >= N || self.count > usize::from(u16::MAX) {
            return None;
        }
        if let Some(parent) = parent
            && !self.node(parent)?.kind.is_container()
        {
            return None;
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
        });
        self.count += 1;
        Some(id)
    }

    fn node(&self, id: WidgetId) -> Option<&Node> {
        self.nodes.get(id.index())?.as_ref()
    }

    fn node_mut(&mut self, id: WidgetId) -> Option<&mut Node> {
        self.nodes.get_mut(id.index())?.as_mut()
    }

    fn ids(&self) -> impl DoubleEndedIterator<Item = WidgetId> + use<N> {
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

    /// Replaces the text of label, button or checkbox `id`.
    pub fn set_text(&mut self, id: WidgetId, words: &str) {
        if self.text_of(id) == Some(words) {
            return;
        }
        self.change(id, |kind| match kind {
            Kind::Label { text, .. } => text.set(words),
            Kind::Button { text, .. } | Kind::Checkbox { text, .. } => text.set(words),
            Kind::TextField(field) => {
                field.text.set(words);
                field.caret = field.text.len();
                field.scroll = 0;
            }
            _ => {}
        });
        self.keep_caret_visible(id);
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
            Kind::TextField(field) => Some(field.text.as_str()),
            _ => None,
        }
    }

    /// The selected option, card or item of radio group, card picker or list `id`, or whether
    /// checkbox `id` is checked (1) or not (0).
    pub fn selected(&self, id: WidgetId) -> Option<usize> {
        match self.kind(id)? {
            Kind::Radios { selected, .. } | Kind::Cards { selected, .. } => Some(*selected),
            Kind::List(list) => Some(list.selected),
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
            Kind::Checkbox { checked, .. } => *checked = index != 0,
            _ => {}
        });
        self.keep_selection_visible(id);
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
        if !shown && self.focus.is_some_and(|focus| self.is_within(focus, id)) {
            self.focus = None;
        }
        self.layout();
    }

    /// Enables or disables widget `id`: a disabled one ignores the pointer and Tab.
    pub fn set_enabled(&mut self, id: WidgetId, enabled: bool) {
        if let Some(node) = self.node_mut(id)
            && node.enabled != enabled
        {
            node.enabled = enabled;
            self.damage_widget(id);
        }
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

    fn is_shown(&self, id: WidgetId) -> bool {
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
        self.size = (width, height);
        self.layout();
        self.damage.clear();
        self.damage.add(Rect::new(0, 0, width, height));
    }

    fn layout(&mut self) {
        let window = Rect::new(0, 0, self.size.0, self.size.1);
        let mut cursors = [0i32; N];
        let mut content_bottoms = [i32::MIN; N];
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
            let (content_width, content_height) = node.kind.content_size();
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
            let x = match node.place.horizontal {
                Horizontal::Left(x) => parent_rect.x + x,
                Horizontal::Right(margin) => parent_rect.right() - margin - width,
                Horizontal::Centre => parent_rect.x + (parent_rect.width - width) / 2,
            };
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
            let height = match node.place.height {
                Length::Px(height) => height,
                Length::Fill(margin) => parent_rect.height - top - margin,
                Length::Content => content_height,
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
            }
            let visible = match node.kind {
                Kind::ScrollView { .. } | Kind::List(_) => rect.intersection(&parent_visible),
                _ => parent_visible,
            };
            if let Some(node) = self.nodes[index].as_mut() {
                node.rect = rect;
                node.visible = visible;
            }
        }
        for (node, bottom) in self.nodes.iter_mut().zip(content_bottoms).take(self.count) {
            if let Some(node) = node.as_mut()
                && let Kind::ScrollView {
                    offset,
                    content_height,
                } = &mut node.kind
            {
                *content_height = if bottom == i32::MIN {
                    0
                } else {
                    bottom + *offset
                };
            }
        }
        self.clamp_scrolls();
    }

    fn clamp_scrolls(&mut self) {
        let mut moved = false;
        for index in 0..self.count {
            if let Some(node) = self.nodes[index].as_mut() {
                let height = node.rect.height;
                match &mut node.kind {
                    Kind::ScrollView {
                        offset,
                        content_height,
                    } => {
                        let clamped = (*offset).clamp(0, (*content_height - height).max(0));
                        moved |= clamped != *offset;
                        *offset = clamped;
                    }
                    Kind::List(list) => {
                        list.offset = list
                            .offset
                            .clamp(0, (list.content_height() - height).max(0));
                    }
                    _ => {}
                }
            }
        }
        if moved {
            self.layout();
        }
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
        let drag = self.drag.is_some_and(|drag| drag.widget == id);
        Look {
            hovered,
            pressed: if drag { Some(THUMB) } else { pressed },
            focused: self.window_focused && self.focus == Some(id),
            enabled: node.enabled,
            caret_shown: self.caret_shown,
        }
    }

    /// Draws every shown widget that meets the canvas's clip, parents first.
    pub fn paint(&self, canvas: &mut Canvas<'_>) {
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
            widget::paint(canvas, &node.kind, node.rect, self.look(id));
            canvas.restore(previous);
        }
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
            } => match scroll_bar(node.rect, *content_height, *offset) {
                Some((track, _)) if track.contains(x, y) => THUMB,
                _ => return None,
            },
            _ => 0,
        };
        Some(Target { widget: id, part })
    }

    /// The pointer shape the window should show where the pointer is.
    pub fn pointer_shape(&self) -> PointerShape {
        match self
            .hover
            .and_then(|target| self.kind(target.widget))
            .filter(|_| self.pressed.is_none())
        {
            Some(Kind::TextField(_)) => PointerShape::IBeam,
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
        if let Some(id) = self.focus
            && let Some(node) = self.node(id)
            && let Kind::TextField(field) = &node.kind
        {
            let caret = caret_rect(field, node.rect);
            self.damage.add(caret.inset(-1));
        }
    }

    fn focused_field(&self) -> bool {
        self.window_focused
            && self
                .focus
                .is_some_and(|id| matches!(self.kind(id), Some(Kind::TextField(_))))
    }

    /// Blinks the caret: call at the time [`Self::next_deadline`] gave, or any time after.
    pub fn tick(&mut self, now_ms: u32) {
        if !self.focused_field() {
            return;
        }
        let shown = (now_ms.wrapping_sub(self.caret_epoch_ms) / CARET_BLINK_MS).is_multiple_of(2);
        if shown != self.caret_shown {
            self.caret_shown = shown;
            self.damage_caret();
        }
    }

    /// When [`Self::tick`] next has something to do, if ever.
    pub fn next_deadline(&self, now_ms: u32) -> Option<u32> {
        if !self.focused_field() {
            return None;
        }
        let elapsed = now_ms.wrapping_sub(self.caret_epoch_ms);
        Some(now_ms.wrapping_add(CARET_BLINK_MS - elapsed % CARET_BLINK_MS))
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
            Input::PointerMove { x, y } => {
                self.pointer_moved(x, y);
                Response::Nothing
            }
            Input::PointerLeave => {
                self.set_hover(None);
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
                true => self.left_pressed(x, y, time_ms),
                false => self.left_released(x, y),
            },
            Input::Button { .. } => Response::Nothing,
            Input::Wheel { x, y, notches } => {
                self.wheel(x, y, notches);
                Response::Nothing
            }
            Input::Key {
                code,
                pressed,
                modifiers,
                character,
            } => match pressed {
                true => self.key_down(
                    code,
                    modifiers.shift(),
                    modifiers.command(),
                    character,
                    time_ms,
                ),
                false => self.key_up(code),
            },
            Input::FocusIn | Input::FocusOut => {
                let focused = input == Input::FocusIn;
                if self.window_focused != focused {
                    self.window_focused = focused;
                    if let Some(id) = self.focus {
                        self.damage_widget(id);
                    }
                    self.restart_caret(time_ms);
                }
                Response::Nothing
            }
            Input::CloseRequest
            | Input::Resize { .. }
            | Input::StateChanged { .. }
            | Input::Overflow => Response::Nothing,
        }
    }

    fn pointer_moved(&mut self, x: i32, y: i32) {
        if let Some(drag) = self.drag {
            self.drag_thumb(drag, y);
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

    fn scroll_extent(&self, id: WidgetId) -> Option<(Rect, i32, i32)> {
        let node = self.node(id)?;
        match &node.kind {
            Kind::List(list) => Some((node.rect, list.content_height(), list.offset)),
            Kind::ScrollView {
                offset,
                content_height,
            } => Some((node.rect, *content_height, *offset)),
            _ => None,
        }
    }

    fn scroll_to(&mut self, id: WidgetId, offset: i32) {
        let Some((rect, content, old)) = self.scroll_extent(id) else {
            return;
        };
        let offset = offset.clamp(0, (content - rect.height).max(0));
        if offset == old {
            return;
        }
        let is_view = matches!(self.kind(id), Some(Kind::ScrollView { .. }));
        if let Some(node) = self.node_mut(id) {
            match &mut node.kind {
                Kind::List(list) => list.offset = offset,
                Kind::ScrollView { offset: held, .. } => *held = offset,
                _ => {}
            }
        }
        if is_view {
            self.layout();
        }
        self.damage.add(rect);
    }

    fn drag_thumb(&mut self, drag: Drag, y: i32) {
        let Some((rect, content, _)) = self.scroll_extent(drag.widget) else {
            return;
        };
        let Some((_, thumb)) = scroll_bar(rect, content, drag.start_offset) else {
            return;
        };
        let travel = (rect.height - thumb.height).max(1);
        let moved = (y - drag.grab_y) * (content - rect.height) / travel;
        self.scroll_to(drag.widget, drag.start_offset + moved);
    }

    fn wheel(&mut self, x: i32, y: i32, notches: i32) {
        let scrollable = self.ids().rev().find(|&id| {
            self.is_shown(id)
                && self.node(id).is_some_and(|node| {
                    matches!(node.kind, Kind::List(_) | Kind::ScrollView { .. })
                        && node.visible.contains(x, y)
                        && node.rect.contains(x, y)
                })
                && self
                    .scroll_extent(id)
                    .is_some_and(|(rect, content, _)| content > rect.height)
        });
        if let Some(id) = scrollable
            && let Some((_, _, offset)) = self.scroll_extent(id)
        {
            self.scroll_to(id, offset - notches * WHEEL_STEP);
        }
    }

    fn left_pressed(&mut self, x: i32, y: i32, time_ms: u32) -> Response {
        let Some(target) = self.hit(x, y) else {
            return Response::Nothing;
        };
        let id = target.widget;
        if self.focusable(id) {
            self.set_focus(Some(id), time_ms);
        }
        if target.part == THUMB {
            return self.thumb_pressed(id, y);
        }
        let double = self.last_click.is_some_and(|(last, at)| {
            last == target && time_ms.wrapping_sub(at) <= DOUBLE_CLICK_MS
        });
        self.last_click = Some((target, time_ms));
        match self.kind(id).copied() {
            Some(Kind::List(list)) => {
                let item = target.part;
                if double {
                    return Response::Opened(id, item);
                }
                if list.selected != item {
                    self.set_selected(id, item);
                    return Response::Chosen(id, item);
                }
                Response::Nothing
            }
            Some(Kind::TextField(field)) => {
                self.place_caret(id, &field, x, time_ms);
                Response::Nothing
            }
            _ => {
                self.pressed = Some(target);
                self.pressed_inside = true;
                self.damage_widget(id);
                Response::Nothing
            }
        }
    }

    fn thumb_pressed(&mut self, id: WidgetId, y: i32) -> Response {
        let Some((rect, content, offset)) = self.scroll_extent(id) else {
            return Response::Nothing;
        };
        let Some((_, thumb)) = scroll_bar(rect, content, offset) else {
            return Response::Nothing;
        };
        if thumb.contains(thumb.x, y) {
            self.drag = Some(Drag {
                widget: id,
                grab_y: y,
                start_offset: offset,
            });
            self.damage_widget(id);
            return Response::Nothing;
        }
        let page = rect.height - 24;
        let offset = match y < thumb.y {
            true => offset - page,
            false => offset + page,
        };
        self.scroll_to(id, offset);
        Response::Nothing
    }

    fn place_caret(&mut self, id: WidgetId, field: &Field, x: i32, now_ms: u32) {
        let Some(rect) = self.rect(id) else {
            return;
        };
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
        self.edit_field(id, now_ms, |field| {
            field.caret = caret;
            false
        });
    }

    fn left_released(&mut self, x: i32, y: i32) -> Response {
        if let Some(drag) = self.drag.take() {
            self.damage_widget(drag.widget);
            self.set_hover(self.hit(x, y));
            return Response::Nothing;
        }
        let Some(pressed) = self.pressed.take() else {
            return Response::Nothing;
        };
        let inside = self.pressed_inside && self.hit(x, y) == Some(pressed);
        self.pressed_inside = false;
        self.damage_widget(pressed.widget);
        self.set_hover(self.hit(x, y));
        match inside {
            true => self.activate(pressed.widget, pressed.part),
            false => Response::Nothing,
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
        if code == key::TAB && !command {
            self.move_focus(shift, time_ms);
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
        let inner = field_inner(rect);
        if let Some(Kind::TextField(field)) = self.node_mut(id).map(|node| &mut node.kind) {
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
        let Some(Kind::TextField(field)) = self.kind(id).copied() else {
            return unused;
        };
        let changed = match code {
            key::BACKSPACE => self.edit_field(id, now_ms, |field| {
                let at = field.text.previous_boundary(field.caret);
                if at == field.caret {
                    return false;
                }
                field.text.remove(at);
                field.caret = at;
                true
            }),
            key::DELETE => self.edit_field(id, now_ms, |field| field.text.remove(field.caret) > 0),
            key::LEFT => self.edit_field(id, now_ms, |field| {
                field.caret = field.text.previous_boundary(field.caret);
                false
            }),
            key::RIGHT => self.edit_field(id, now_ms, |field| {
                field.caret = field.text.next_boundary(field.caret);
                false
            }),
            key::HOME => self.edit_field(id, now_ms, |field| {
                field.caret = 0;
                false
            }),
            key::END => self.edit_field(id, now_ms, |field| {
                field.caret = field.text.len();
                false
            }),
            key::ESC => match field.escape {
                Escape::Clear if field.text.is_empty() => return unused,
                Escape::Clear => self.edit_field(id, now_ms, |field| {
                    field.text.set("");
                    field.caret = 0;
                    true
                }),
                Escape::Revert => {
                    let before = self.text_before_focus;
                    self.edit_field(id, now_ms, |field| {
                        let changed = field.text != before;
                        field.text = before;
                        field.caret = field.text.len();
                        changed
                    })
                }
            },
            _ => {
                let typed = char::from_u32(character)
                    .filter(|typed| !command && !typed.is_control() && (*typed as u32) < 0x100);
                match typed {
                    Some(typed) => self.edit_field(id, now_ms, |field| {
                        let inserted = field.text.insert(field.caret, typed);
                        if inserted {
                            field.caret += typed.len_utf8();
                        }
                        inserted
                    }),
                    None => return unused,
                }
            }
        };
        match changed {
            true => Response::Edited(id),
            false => Response::Nothing,
        }
    }
}

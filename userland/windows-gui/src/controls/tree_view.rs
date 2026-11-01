//! comctl32's tree view (section 25 of the design, D178): the [`TreeView`] model (up to
//! [`TREE_CAPACITY`] items in one arena, linked to their parent and siblings, their labels in one
//! text store, each named by a handle that carries a generation so a deleted item's handle never
//! names the item that takes its slot), the visible items in depth-first order, the geometry of
//! an item's row (indent, `+`/`-` box, check box, image, label area), hit testing, and the
//! painting with its dotted lines.

use super::common::{CommonPalette, Images, TextArena, draw_plain, plain_width_cells};
use super::list_box::compare_names;
use super::list_view::{
    CHECK_BOX_SIZE, draw_check_box, horizontal_part_at, paint_scroll_bars, stable_sort,
};
use super::scroll_bar::{self, Part, ScrollInfo, WIDTH as BAR};
use super::{Palette, dotted_outline, fit_cells, outline};
use crate::surface::{Rect, Surface};
use crate::text::CELL_WIDTH;
use core::cmp::Ordering;

/// The most items a tree holds.
pub const TREE_CAPACITY: usize = 512;
/// The UTF-16 units all of a tree's labels hold together.
pub const TREE_UNITS: usize = 16 * 1024;
/// An item's height.
pub const ITEM_HEIGHT: i32 = 20;
/// The indent until `TVM_SETINDENT` sets one.
pub const DEFAULT_INDENT: i32 = 20;
/// The smallest indent `TVM_SETINDENT` gives.
pub const SMALLEST_INDENT: i32 = 12;
/// What a click on a scroll arrow moves a tree by horizontally, in pixels.
pub const HORIZONTAL_LINE: i32 = 8;
const NONE: u16 = u16::MAX;
const GENERATIONS: u16 = 0x7fff;
const INDEX_BITS: u32 = 16;
const CONTENT_LEFT: i32 = 2;
const BOX_SIZE: i32 = 9;
const BOX_TOP: i32 = 5;
const BOX_MIDDLE: i32 = 4;
const BOX_MARK_FROM: i32 = 2;
const BOX_MARK_LENGTH: i32 = 5;
const LINE_ROW: i32 = 9;
const PREFIX_STEP: i32 = 20;
const CHECK_BOX_LEFT: i32 = 3;
const CHECK_BOX_TOP: i32 = 3;
const IMAGE_TOP: i32 = 2;
const IMAGE_SIZE: i32 = 16;
const LABEL_GAP: i32 = 4;
const LABEL_TOP: i32 = 2;
const AREA_MARGIN: i32 = 2;
const AREA_TOP: i32 = 1;
const AREA_HEIGHT: i32 = 18;
const ELLIPSIS_CELLS: usize = 3;
const UNLIMITED_ROOM: i32 = i32::MAX / 4;

/// The tree view's styles this layer reads (`TVS_*`).
pub mod style {
    pub const HAS_BUTTONS: u32 = 0x1;
    pub const HAS_LINES: u32 = 0x2;
    pub const LINES_AT_ROOT: u32 = 0x4;
    pub const SHOW_SELECTION_ALWAYS: u32 = 0x20;
    pub const CHECK_BOXES: u32 = 0x100;
    pub const FULL_ROW_SELECT: u32 = 0x1000;
    pub const NO_SCROLL: u32 = 0x2000;
    pub const NO_HORIZONTAL_SCROLL: u32 = 0x8000;
}

/// An item's state bits (`TVIS_*`); the state image is the list view's.
pub mod state {
    pub const SELECTED: u32 = 0x2;
    pub const EXPANDED: u32 = 0x20;
    pub const EXPANDED_ONCE: u32 = 0x40;
    pub const STATE_IMAGE_MASK: u32 = 0xf000;
}

/// The keys a tree reads, as Windows' virtual keys.
pub mod key {
    pub const BACK: u32 = 0x08;
    pub const RETURN: u32 = 0x0d;
    pub const PAGE_UP: u32 = 0x21;
    pub const PAGE_DOWN: u32 = 0x22;
    pub const END: u32 = 0x23;
    pub const HOME: u32 = 0x24;
    pub const LEFT: u32 = 0x25;
    pub const UP: u32 = 0x26;
    pub const RIGHT: u32 = 0x27;
    pub const DOWN: u32 = 0x28;
    pub const MULTIPLY: u32 = 0x6a;
    pub const ADD: u32 = 0x6b;
    pub const SUBTRACT: u32 = 0x6d;
    pub const PLUS: u32 = 0xbb;
    pub const MINUS: u32 = 0xbd;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Node {
    used: bool,
    generation: u16,
    parent: u16,
    first: u16,
    last: u16,
    next: u16,
    previous: u16,
    state: u32,
    image: i32,
    selected_image: i32,
    declared: Option<bool>,
    lparam: usize,
}

const FREE: Node = Node {
    used: false,
    generation: 0,
    parent: NONE,
    first: NONE,
    last: NONE,
    next: NONE,
    previous: NONE,
    state: 0,
    image: 0,
    selected_image: 0,
    declared: None,
    lparam: 0,
};

/// Where a new item goes among its siblings (`TVI_FIRST`, `TVI_LAST`, `TVI_SORT`, or after a
/// sibling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    First,
    Last,
    Sorted,
    After(usize),
}

/// What a new item starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewItem<'a> {
    pub text: &'a [u16],
    pub image: i32,
    pub selected_image: i32,
    pub state: u32,
    pub lparam: usize,
    /// `cChildren`: whether it shows a box whatever its children (`None`: by its children).
    pub children: Option<bool>,
}

/// Where a tree's parts are inside its rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLayout {
    pub inner: Rect,
    /// Where items are drawn: left of the vertical bar, above the horizontal.
    pub rows: Rect,
    /// `V`: the whole rows shown (at least 1).
    pub visible: usize,
    pub vertical: Option<Rect>,
    pub horizontal: Option<Rect>,
    /// How far the widest visible item's label area reaches from the inner left edge.
    pub widest: i32,
}

/// Where the parts of one item's row are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemGeometry {
    pub row: Rect,
    pub depth: usize,
    /// The content x: where the item's own column starts.
    pub content_x: i32,
    pub button: Option<Rect>,
    pub check_box: Option<Rect>,
    pub image: Option<Rect>,
    pub label_x: i32,
    pub label_area: Rect,
    /// The room the label has, in pixels.
    pub room: i32,
}

/// The part of an item's row a point is on (`TVHT_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreePart {
    Button,
    CheckBox,
    Icon,
    Label,
    Indent,
    Right,
}

/// What is at a point of a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeHit {
    Item {
        index: usize,
        part: TreePart,
    },
    /// The rows area below the last item.
    Below,
    Vertical(Part),
    Horizontal(Part),
    /// The border, the size box, or outside.
    Nowhere,
}

/// A tree's items, selection and scroll.
#[derive(Debug, Clone)]
pub struct TreeView {
    labels: TextArena<TREE_UNITS, TREE_CAPACITY>,
    nodes: [Node; TREE_CAPACITY],
    count: usize,
    first_root: u16,
    last_root: u16,
    selected: u16,
    top: usize,
    indent: i32,
    pub hpos: i32,
    pub images: usize,
    pub state_images: usize,
    pub background: Option<u32>,
    pub text_colour: Option<u32>,
    pub line_colour: Option<u32>,
}

impl Default for TreeView {
    fn default() -> Self {
        Self::new()
    }
}

fn fold(unit: u32) -> u32 {
    match unit {
        0x41..=0x5a => unit + 0x20,
        unit => unit,
    }
}

fn raw(index: Option<usize>) -> u16 {
    index.map_or(NONE, |index| index as u16)
}

fn real(index: u16) -> Option<usize> {
    (index != NONE).then_some(usize::from(index))
}

fn shown_cells(units: &[u16], room: i32) -> usize {
    let (shown, ellipsis) = fit_cells(plain_width_cells(units), room);
    match ellipsis {
        true => shown + ELLIPSIS_CELLS,
        false => shown,
    }
}

fn dot(surface: &mut Surface<'_>, x: i32, y: i32, colour: u32) {
    let (ox, oy) = surface.origin();
    if (x + ox + y + oy).rem_euclid(2) == 0 {
        surface.pixel(x, y, colour);
    }
}

fn dotted_column(
    surface: &mut Surface<'_>,
    x: i32,
    from: i32,
    to: i32,
    skip: Option<(i32, i32)>,
    colour: u32,
) {
    for y in from..=to {
        if skip.is_some_and(|(top, bottom)| y >= top && y <= bottom) {
            continue;
        }
        dot(surface, x, y, colour);
    }
}

impl TreeView {
    pub const fn new() -> Self {
        Self {
            labels: TextArena::new(),
            nodes: [FREE; TREE_CAPACITY],
            count: 0,
            first_root: NONE,
            last_root: NONE,
            selected: NONE,
            top: 0,
            indent: DEFAULT_INDENT,
            hpos: 0,
            images: 0,
            state_images: 0,
            background: None,
            text_colour: None,
            line_colour: None,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn indent(&self) -> i32 {
        self.indent
    }

    /// `TVM_SETINDENT`: at least [`SMALLEST_INDENT`].
    pub fn set_indent(&mut self, indent: i32) {
        self.indent = indent.max(SMALLEST_INDENT);
    }

    /// The first visible item shown, by its visible index.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The handle (`HTREEITEM`) of item `index`: its generation and its slot.
    pub fn handle(&self, index: usize) -> usize {
        match self.nodes.get(index) {
            Some(node) if node.used => (usize::from(node.generation) << INDEX_BITS) | (index + 1),
            _ => 0,
        }
    }

    /// The item `handle` names, while it exists.
    pub fn index_of(&self, handle: usize) -> Option<usize> {
        let index = (handle & ((1 << INDEX_BITS) - 1)).checked_sub(1)?;
        let node = self.nodes.get(index)?;
        let generation = handle >> INDEX_BITS;
        (node.used && generation == usize::from(node.generation)).then_some(index)
    }

    fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index).filter(|node| node.used)
    }

    pub fn exists(&self, index: usize) -> bool {
        self.node(index).is_some()
    }

    pub fn text(&self, index: usize) -> &[u16] {
        match self.exists(index) {
            true => self.labels.get(index),
            false => &[],
        }
    }

    pub fn state(&self, index: usize) -> u32 {
        self.node(index).map_or(0, |node| node.state)
    }

    pub fn image(&self, index: usize) -> i32 {
        self.node(index).map_or(0, |node| node.image)
    }

    pub fn selected_image(&self, index: usize) -> i32 {
        self.node(index).map_or(0, |node| node.selected_image)
    }

    pub fn lparam(&self, index: usize) -> usize {
        self.node(index).map_or(0, |node| node.lparam)
    }

    /// `cChildren` as `TVM_GETITEMW` answers it: 1 when it shows a box, else 0.
    pub fn declared_children(&self, index: usize) -> bool {
        self.has_children(index)
    }

    pub fn parent(&self, index: usize) -> Option<usize> {
        real(self.node(index)?.parent)
    }

    pub fn first_child(&self, index: usize) -> Option<usize> {
        real(self.node(index)?.first)
    }

    pub fn next_sibling(&self, index: usize) -> Option<usize> {
        real(self.node(index)?.next)
    }

    pub fn previous_sibling(&self, index: usize) -> Option<usize> {
        real(self.node(index)?.previous)
    }

    pub fn first_root(&self) -> Option<usize> {
        real(self.first_root)
    }

    /// The first child of `parent`, or the first root when `None`.
    pub fn first_of(&self, parent: Option<usize>) -> Option<usize> {
        match parent {
            Some(parent) => self.first_child(parent),
            None => self.first_root(),
        }
    }

    pub fn is_expanded(&self, index: usize) -> bool {
        self.state(index) & state::EXPANDED != 0
    }

    /// Whether item `index` shows a box: its `cChildren`, else whether it has children.
    pub fn has_children(&self, index: usize) -> bool {
        self.node(index)
            .is_some_and(|node| node.declared.unwrap_or(node.first != NONE))
    }

    pub fn selected(&self) -> Option<usize> {
        real(self.selected)
    }

    /// Makes `index` the selected item (the caret), or none.
    pub fn set_selected(&mut self, index: Option<usize>) {
        if let Some(old) = self.selected() {
            self.nodes[old].state &= !state::SELECTED;
        }
        self.selected = raw(index.filter(|&index| self.exists(index)));
        if let Some(new) = self.selected() {
            self.nodes[new].state |= state::SELECTED;
        }
    }

    pub fn set_text(&mut self, index: usize, text: &[u16]) -> bool {
        self.exists(index) && self.labels.set(index, text)
    }

    pub fn set_image(&mut self, index: usize, image: i32) {
        if self.exists(index) {
            self.nodes[index].image = image;
        }
    }

    pub fn set_selected_image(&mut self, index: usize, image: i32) {
        if self.exists(index) {
            self.nodes[index].selected_image = image;
        }
    }

    pub fn set_lparam(&mut self, index: usize, lparam: usize) {
        if self.exists(index) {
            self.nodes[index].lparam = lparam;
        }
    }

    pub fn set_declared_children(&mut self, index: usize, children: Option<bool>) {
        if self.exists(index) {
            self.nodes[index].declared = children;
        }
    }

    /// Sets the bits of `mask` (but the selection, which only [`TreeView::set_selected`] moves)
    /// to `bits`.
    pub fn set_state(&mut self, index: usize, bits: u32, mask: u32) {
        if self.exists(index) {
            let mask = mask & !state::SELECTED;
            let node = &mut self.nodes[index];
            node.state = (node.state & !mask) | (bits & mask);
        }
    }

    /// Expands or collapses item `index`; whether its state changed.
    pub fn set_expanded(&mut self, index: usize, expanded: bool) -> bool {
        if !self.exists(index) || self.is_expanded(index) == expanded {
            return false;
        }
        let node = &mut self.nodes[index];
        node.state = match expanded {
            true => node.state | state::EXPANDED | state::EXPANDED_ONCE,
            false => node.state & !state::EXPANDED,
        };
        true
    }

    fn first_link(&self, parent: u16) -> u16 {
        match parent {
            NONE => self.first_root,
            parent => self.nodes[usize::from(parent)].first,
        }
    }

    fn set_first_link(&mut self, parent: u16, value: u16) {
        match parent {
            NONE => self.first_root = value,
            parent => self.nodes[usize::from(parent)].first = value,
        }
    }

    fn set_last_link(&mut self, parent: u16, value: u16) {
        match parent {
            NONE => self.last_root = value,
            parent => self.nodes[usize::from(parent)].last = value,
        }
    }

    fn last_link(&self, parent: u16) -> u16 {
        match parent {
            NONE => self.last_root,
            parent => self.nodes[usize::from(parent)].last,
        }
    }

    fn link(&mut self, index: usize, parent: u16, after: Option<usize>) {
        let next = match after {
            Some(after) => self.nodes[after].next,
            None => self.first_link(parent),
        };
        let node = &mut self.nodes[index];
        node.parent = parent;
        node.previous = raw(after);
        node.next = next;
        match after {
            Some(after) => self.nodes[after].next = index as u16,
            None => self.set_first_link(parent, index as u16),
        }
        match next {
            NONE => self.set_last_link(parent, index as u16),
            next => self.nodes[usize::from(next)].previous = index as u16,
        }
    }

    fn unlink(&mut self, index: usize) {
        let Node {
            parent,
            previous,
            next,
            ..
        } = self.nodes[index];
        match previous {
            NONE => self.set_first_link(parent, next),
            previous => self.nodes[usize::from(previous)].next = next,
        }
        match next {
            NONE => self.set_last_link(parent, previous),
            next => self.nodes[usize::from(next)].previous = previous,
        }
        let node = &mut self.nodes[index];
        node.previous = NONE;
        node.next = NONE;
    }

    /// Inserts an item under `parent` (a root when `None`) at `place`; its index, `None` when
    /// the parent is gone, the tree is full or its label does not fit.
    pub fn insert(
        &mut self,
        parent: Option<usize>,
        place: Place,
        item: &NewItem<'_>,
    ) -> Option<usize> {
        if parent.is_some_and(|parent| !self.exists(parent)) {
            return None;
        }
        let index = self.nodes.iter().position(|node| !node.used)?;
        if !self.labels.set(index, item.text) {
            return None;
        }
        let parent_link = raw(parent);
        let after = match place {
            Place::First => None,
            Place::Last => real(self.last_link(parent_link)),
            Place::Sorted => {
                let mut after = None;
                let mut at = real(self.first_link(parent_link));
                while let Some(sibling) = at {
                    if compare_names(item.text, self.labels.get(sibling)) == Ordering::Less {
                        break;
                    }
                    after = Some(sibling);
                    at = real(self.nodes[sibling].next);
                }
                after
            }
            Place::After(sibling)
                if self
                    .node(sibling)
                    .is_some_and(|node| node.parent == parent_link) =>
            {
                Some(sibling)
            }
            Place::After(_) => real(self.last_link(parent_link)),
        };
        let generation = self.nodes[index].generation % GENERATIONS + 1;
        self.nodes[index] = Node {
            used: true,
            generation,
            parent: parent_link,
            first: NONE,
            last: NONE,
            next: NONE,
            previous: NONE,
            state: item.state & !state::SELECTED,
            image: item.image,
            selected_image: item.selected_image,
            declared: item.children,
            lparam: item.lparam,
        };
        self.link(index, parent_link, after);
        self.count += 1;
        Some(index)
    }

    fn deepest_first(&self, mut at: usize) -> usize {
        while let Some(first) = real(self.nodes[at].first) {
            at = first;
        }
        at
    }

    /// Item `index` and its descendants, children before their parent (the order
    /// `TVN_DELETEITEMW` goes in), into `out`; how many.
    pub fn postorder(&self, index: usize, out: &mut [u16]) -> usize {
        if !self.exists(index) {
            return 0;
        }
        let mut count = 0;
        let mut at = self.deepest_first(index);
        while count < out.len() {
            out[count] = at as u16;
            count += 1;
            if at == index {
                break;
            }
            at = match real(self.nodes[at].next) {
                Some(next) => self.deepest_first(next),
                None => usize::from(self.nodes[at].parent),
            };
        }
        count
    }

    /// Deletes item `index` and its descendants (the selection goes with them); whether it
    /// existed.
    pub fn remove(&mut self, index: usize) -> bool {
        if !self.exists(index) {
            return false;
        }
        let mut doomed = [0u16; TREE_CAPACITY];
        let count = self.postorder(index, &mut doomed);
        self.unlink(index);
        for &at in &doomed[..count] {
            let at = usize::from(at);
            if self.selected == at as u16 {
                self.selected = NONE;
            }
            self.labels.set(at, &[]);
            let generation = self.nodes[at].generation;
            self.nodes[at] = Node { generation, ..FREE };
            self.count -= 1;
        }
        true
    }

    /// Whether every ancestor of item `index` is expanded.
    pub fn is_shown(&self, index: usize) -> bool {
        let mut at = self.parent(index);
        while let Some(parent) = at {
            if !self.is_expanded(parent) {
                return false;
            }
            at = self.parent(parent);
        }
        self.exists(index)
    }

    /// Item `index`'s depth: 0 for a root.
    pub fn depth(&self, index: usize) -> usize {
        let mut depth = 0;
        let mut at = self.parent(index);
        while let Some(parent) = at {
            depth += 1;
            at = self.parent(parent);
        }
        depth
    }

    /// The visible item after `index` in depth-first order.
    pub fn next_visible(&self, index: usize) -> Option<usize> {
        if self.is_expanded(index)
            && let Some(first) = self.first_child(index)
        {
            return Some(first);
        }
        let mut at = index;
        loop {
            if let Some(next) = self.next_sibling(at) {
                return Some(next);
            }
            at = self.parent(at)?;
        }
    }

    /// The visible item before `index`.
    pub fn previous_visible(&self, index: usize) -> Option<usize> {
        match self.previous_sibling(index) {
            Some(previous) => Some(self.last_visible_in(previous)),
            None => self.parent(index),
        }
    }

    fn last_visible_in(&self, mut at: usize) -> usize {
        while self.is_expanded(at)
            && let Some(last) = real(self.nodes[at].last)
        {
            at = last;
        }
        at
    }

    /// The last visible item.
    pub fn last_visible(&self) -> Option<usize> {
        real(self.last_root).map(|last| self.last_visible_in(last))
    }

    /// How many items are visible (every ancestor expanded).
    pub fn visible_count(&self) -> usize {
        let mut count = 0;
        let mut at = self.first_root();
        while let Some(index) = at {
            count += 1;
            at = self.next_visible(index);
        }
        count
    }

    /// Item `index`'s place among the visible items, if it is one.
    pub fn visible_index(&self, index: usize) -> Option<usize> {
        if !self.is_shown(index) {
            return None;
        }
        let mut count = 0;
        let mut at = self.first_root();
        while let Some(item) = at {
            if item == index {
                return Some(count);
            }
            count += 1;
            at = self.next_visible(item);
        }
        None
    }

    /// The visible item at place `place`.
    pub fn visible_at(&self, place: usize) -> Option<usize> {
        let mut at = self.first_root();
        for _ in 0..place {
            at = self.next_visible(at?);
        }
        at
    }

    /// Scrolls so the visible item at `place` shows whole, `visible` rows showing.
    pub fn ensure_visible(&mut self, place: usize, visible: usize) {
        let visible = visible.max(1);
        if place < self.top {
            self.top = place;
        }
        if place >= self.top + visible {
            self.top = place + 1 - visible;
        }
    }

    /// Makes `top` the first visible item shown, as far as the items go.
    pub fn set_top(&mut self, top: usize, visible: usize) {
        self.top = top.min(self.visible_count().saturating_sub(visible.max(1)));
    }

    /// Keeps the scroll within the items and the selected item in view after a change.
    pub fn settle(&mut self, visible: usize) {
        self.set_top(self.top, visible);
        if let Some(place) = self
            .selected()
            .and_then(|selected| self.visible_index(selected))
        {
            self.ensure_visible(place, visible);
        }
    }

    /// Sets the horizontal scroll, kept within what `layout` leaves.
    pub fn set_hpos(&mut self, hpos: i32, layout: &TreeLayout) {
        let most = (layout.widest - layout.rows.width()).max(0);
        self.hpos = hpos.clamp(0, most);
    }

    /// The visible item an arrow, Home, End, PgUp or PgDn selects, `visible` rows showing.
    pub fn step(&self, virtual_key: u32, visible: usize) -> Option<usize> {
        let page = visible.max(2) - 1;
        let selected = self.selected();
        match (virtual_key, selected) {
            (key::UP, Some(selected)) => self.previous_visible(selected),
            (key::DOWN, Some(selected)) => self.next_visible(selected),
            (key::UP | key::DOWN | key::HOME, _) => self.first_root(),
            (key::END, _) => self.last_visible(),
            (key::PAGE_UP, Some(selected)) => {
                let place = self.visible_index(selected)?;
                self.visible_at(place.saturating_sub(page))
            }
            (key::PAGE_DOWN, Some(selected)) => {
                let place = self.visible_index(selected)?;
                let last = self.visible_count().saturating_sub(1);
                self.visible_at((place + page).min(last))
            }
            (key::PAGE_UP | key::PAGE_DOWN, None) => self.first_root(),
            _ => None,
        }
    }

    /// The next visible item after the selected one, wrapping, whose label starts with
    /// `character` (ASCII letters folded).
    pub fn find_by_first_glyph(&self, character: u32) -> Option<usize> {
        let wanted = fold(character);
        let matches = |index: usize| {
            self.labels
                .get(index)
                .first()
                .is_some_and(|&unit| fold(u32::from(unit)) == wanted)
        };
        let start = self.selected();
        let mut at = match start {
            Some(selected) => self.next_visible(selected).or_else(|| self.first_root()),
            None => self.first_root(),
        };
        for _ in 0..=self.count {
            let index = at?;
            if matches(index) {
                return Some(index);
            }
            if Some(index) == start {
                return None;
            }
            at = self.next_visible(index).or_else(|| self.first_root());
        }
        None
    }

    /// The children of `parent` (the roots when `None`) in order, into `out`; how many.
    pub fn children(&self, parent: Option<usize>, out: &mut [u16]) -> usize {
        let mut count = 0;
        let mut at = self.first_of(parent);
        while let Some(index) = at {
            if count == out.len() {
                break;
            }
            out[count] = index as u16;
            count += 1;
            at = self.next_sibling(index);
        }
        count
    }

    /// Puts the children of `parent` in the order `order` gives; false, nothing moved, when it
    /// is not an order of exactly those children.
    pub fn reorder_children(&mut self, parent: Option<usize>, order: &[u16]) -> bool {
        let mut current = [0u16; TREE_CAPACITY];
        let count = self.children(parent, &mut current);
        if order.len() != count {
            return false;
        }
        let mut seen = [false; TREE_CAPACITY];
        for &index in order {
            let index = usize::from(index);
            if index >= TREE_CAPACITY || seen[index] || !current[..count].contains(&(index as u16))
            {
                return false;
            }
            seen[index] = true;
        }
        let parent_link = raw(parent);
        for &index in order {
            self.unlink(usize::from(index));
        }
        let mut after = None;
        for &index in order {
            self.link(usize::from(index), parent_link, after);
            after = Some(usize::from(index));
        }
        true
    }

    /// `TVM_SORTCHILDREN`: the children of `parent` by label, ignoring ASCII case, and their
    /// descendants' too with `recurse`.
    pub fn sort_children(&mut self, parent: Option<usize>, recurse: bool) {
        let mut pending = [0u16; TREE_CAPACITY + 1];
        let mut waiting = 1;
        pending[0] = raw(parent);
        while waiting > 0 {
            waiting -= 1;
            let at = real(pending[waiting]);
            let mut order = [0u16; TREE_CAPACITY];
            let count = self.children(at, &mut order);
            let mut scratch = [0u16; TREE_CAPACITY];
            stable_sort(
                &mut order[..count],
                &mut scratch[..count],
                &mut |a: u16, b: u16| {
                    compare_names(
                        self.labels.get(usize::from(a)),
                        self.labels.get(usize::from(b)),
                    )
                },
            );
            self.reorder_children(at, &order[..count]);
            if recurse {
                for &child in &order[..count] {
                    if waiting < pending.len() {
                        pending[waiting] = child;
                        waiting += 1;
                    }
                }
            }
        }
    }

    /// Where the parts of a tree of `style` drawn over `rect` are.
    pub fn layout(&self, rect: Rect, style: u32) -> TreeLayout {
        let inner = rect.inset(1);
        let scrolls = style & style::NO_SCROLL == 0;
        let horizontal_allowed = scrolls && style & style::NO_HORIZONTAL_SCROLL == 0;
        let visible_count = self.visible_count();
        let widest = match horizontal_allowed {
            true => self.widest(style, inner),
            false => 0,
        };
        let size = |vertical: bool, horizontal: bool| {
            let width = inner.width() - if vertical { BAR } else { 0 };
            let height = inner.height() - if horizontal { BAR } else { 0 };
            (width, height, ((height / ITEM_HEIGHT).max(1)) as usize)
        };
        let (mut vertical, mut horizontal) = (false, false);
        for _ in 0..3 {
            let (width, _, visible) = size(vertical, horizontal);
            vertical = scrolls && visible_count > visible;
            horizontal = horizontal_allowed && widest > width;
        }
        let (width, height, visible) = size(vertical, horizontal);
        let rows = Rect::sized(inner.left, inner.top, width.max(0), height.max(0));
        let below_bar = match horizontal {
            true => BAR,
            false => 0,
        };
        TreeLayout {
            inner,
            rows,
            visible,
            vertical: vertical.then_some(Rect::new(
                inner.right - BAR,
                inner.top,
                inner.right,
                inner.bottom - below_bar,
            )),
            horizontal: horizontal.then_some(Rect::new(
                inner.left,
                inner.bottom - BAR,
                rows.right,
                inner.bottom,
            )),
            widest,
        }
    }

    fn widest(&self, style: u32, inner: Rect) -> i32 {
        let mut widest = 0;
        let mut at = self.first_root();
        while let Some(index) = at {
            let label_x = self.label_x(style, inner.left, index, 0);
            let cells = plain_width_cells(self.labels.get(index)) as i32;
            widest = widest
                .max(label_x - AREA_MARGIN + cells * CELL_WIDTH + 2 * AREA_MARGIN - inner.left);
            at = self.next_visible(index);
        }
        widest
    }

    fn lead(style: u32) -> i32 {
        let lines_or_buttons = style & (style::HAS_LINES | style::HAS_BUTTONS) != 0;
        i32::from(style & style::LINES_AT_ROOT != 0 && lines_or_buttons)
    }

    fn content_x(&self, style: u32, inner_left: i32, index: usize, hpos: i32) -> i32 {
        let steps = self.depth(index) as i32 + Self::lead(style);
        inner_left + CONTENT_LEFT + self.indent * steps - hpos
    }

    fn label_x(&self, style: u32, inner_left: i32, index: usize, hpos: i32) -> i32 {
        let mut x = self.content_x(style, inner_left, index, hpos);
        if style & style::CHECK_BOXES != 0 {
            x += PREFIX_STEP;
        }
        match self.images != 0 {
            true => x + PREFIX_STEP,
            false => x + LABEL_GAP,
        }
    }

    /// The row of the visible item at `place`, shown or not.
    pub fn row_rect(&self, layout: &TreeLayout, place: usize) -> Rect {
        let y = layout.rows.top + ITEM_HEIGHT * (place as i32 - self.top as i32);
        Rect::new(layout.rows.left, y, layout.rows.right, y + ITEM_HEIGHT)
    }

    /// Where the parts of item `index`'s row are, the item being the visible one at `place`.
    pub fn geometry(
        &self,
        layout: &TreeLayout,
        style: u32,
        index: usize,
        place: usize,
    ) -> ItemGeometry {
        let row = self.row_rect(layout, place);
        let depth = self.depth(index);
        let content_x = self.content_x(style, layout.inner.left, index, self.hpos);
        let has_lines_column = depth as i32 + Self::lead(style) >= 1;
        let button =
            (style & style::HAS_BUTTONS != 0 && has_lines_column && self.has_children(index))
                .then_some(Rect::sized(
                    content_x - self.indent + (self.indent - BOX_SIZE) / 2,
                    row.top + BOX_TOP,
                    BOX_SIZE,
                    BOX_SIZE,
                ));
        let mut x = content_x;
        let check_box = (style & style::CHECK_BOXES != 0).then(|| {
            let rect = Rect::sized(
                x + CHECK_BOX_LEFT,
                row.top + CHECK_BOX_TOP,
                CHECK_BOX_SIZE,
                CHECK_BOX_SIZE,
            );
            x += PREFIX_STEP;
            rect
        });
        let image = (self.images != 0).then_some(Rect::sized(
            x,
            row.top + IMAGE_TOP,
            IMAGE_SIZE,
            IMAGE_SIZE,
        ));
        let label_x = match image {
            Some(_) => x + PREFIX_STEP,
            None => x + LABEL_GAP,
        };
        let room = match layout.horizontal.is_some()
            || style & (style::NO_SCROLL | style::NO_HORIZONTAL_SCROLL) == 0
        {
            true => UNLIMITED_ROOM,
            false => layout.rows.right - label_x,
        };
        let cells = shown_cells(self.labels.get(index), room) as i32;
        let label_area = Rect::sized(
            label_x - AREA_MARGIN,
            row.top + AREA_TOP,
            cells * CELL_WIDTH + 2 * AREA_MARGIN,
            AREA_HEIGHT,
        );
        ItemGeometry {
            row,
            depth,
            content_x,
            button,
            check_box,
            image,
            label_x,
            label_area,
            room,
        }
    }

    /// The area a selected item fills: its label area, or its row with `TVS_FULLROWSELECT`.
    pub fn selection_area(geometry: &ItemGeometry, style: u32) -> Rect {
        match style & style::FULL_ROW_SELECT != 0 {
            true => geometry.row,
            false => geometry.label_area,
        }
    }

    /// Item `index`'s bounds as agents see them: its row; an item out of view (scrolled, or
    /// under a collapsed parent) as wide as the rows area and 0 high at its top or bottom.
    pub fn item_bounds(&self, layout: &TreeLayout, index: usize) -> Rect {
        let rows = layout.rows;
        let above = Rect::new(rows.left, rows.top, rows.right, rows.top);
        let below = Rect::new(rows.left, rows.bottom, rows.right, rows.bottom);
        let mut shown = index;
        while !self.is_shown(shown) {
            match self.parent(shown) {
                Some(parent) => shown = parent,
                None => return below,
            }
        }
        let Some(place) = self.visible_index(shown) else {
            return below;
        };
        if shown != index {
            return match place < self.top {
                true => above,
                false => below,
            };
        }
        let row = self.row_rect(layout, place);
        match () {
            () if row.bottom <= rows.top => above,
            () if row.top >= rows.bottom => below,
            () => row.intersection(&rows),
        }
    }

    /// The vertical bar's `SCROLLINFO`: the visible items, `V` and the first shown.
    pub fn vertical_info(&self, layout: &TreeLayout) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: self.visible_count() as i32 - 1,
            page: layout.visible as u32,
            position: self.top as i32,
        }
    }

    /// The horizontal bar's `SCROLLINFO`, in pixels.
    pub fn horizontal_info(&self, layout: &TreeLayout) -> ScrollInfo {
        ScrollInfo {
            minimum: 0,
            maximum: layout.widest - 1,
            page: layout.rows.width().max(0) as u32,
            position: self.hpos,
        }
    }

    /// What is at (`x`, `y`) of a tree of `style`.
    pub fn hit(&self, layout: &TreeLayout, style: u32, x: i32, y: i32) -> TreeHit {
        if let Some(bar) = layout.vertical
            && bar.contains(x, y)
        {
            return scroll_bar::part_at(bar, &self.vertical_info(layout), y)
                .map_or(TreeHit::Nowhere, TreeHit::Vertical);
        }
        if let Some(bar) = layout.horizontal
            && bar.contains(x, y)
        {
            return horizontal_part_at(bar, &self.horizontal_info(layout), x)
                .map_or(TreeHit::Nowhere, TreeHit::Horizontal);
        }
        if !layout.rows.contains(x, y) {
            return TreeHit::Nowhere;
        }
        let place = self.top + ((y - layout.rows.top) / ITEM_HEIGHT) as usize;
        let Some(index) = self.visible_at(place) else {
            return TreeHit::Below;
        };
        let geometry = self.geometry(layout, style, index, place);
        let on = |rect: Option<Rect>| rect.is_some_and(|rect| rect.contains(x, y));
        let part = match () {
            () if on(geometry.button) => TreePart::Button,
            () if on(geometry.check_box) => TreePart::CheckBox,
            () if on(geometry.image) => TreePart::Icon,
            () if geometry.label_area.contains(x, y) => TreePart::Label,
            () if x < geometry.label_area.left => TreePart::Indent,
            () => TreePart::Right,
        };
        TreeHit::Item { index, part }
    }
}

/// How a tree looks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLook {
    /// The window's style (`TVS_*`).
    pub style: u32,
    pub focused: bool,
    pub enabled: bool,
}

/// Paints `tree` over the whole of `rect`: the frame, the visible items with their lines,
/// boxes, check boxes, images and labels, and the scroll bars.
pub fn paint(
    surface: &mut Surface<'_>,
    rect: Rect,
    tree: &TreeView,
    look: TreeLook,
    images: &dyn Images,
    palette: &Palette,
    common: &CommonPalette,
) {
    let saved = surface.clip();
    surface.clip_to(rect);
    let layout = tree.layout(rect, look.style);
    outline(surface, rect, palette.field_border);
    surface.fill(layout.inner, tree.background.unwrap_or(palette.field_fill));
    let rows_clip = surface.clip().intersection(&layout.rows);
    surface.set_clip(rows_clip);
    let mut place = tree.top;
    let mut at = tree.visible_at(place);
    while let Some(index) = at {
        if tree.row_rect(&layout, place).top >= layout.rows.bottom {
            break;
        }
        paint_item(
            surface, tree, &layout, index, place, look, images, palette, common,
        );
        surface.set_clip(rows_clip);
        at = tree.next_visible(index);
        place += 1;
    }
    surface.set_clip(saved.intersection(&rect));
    paint_scroll_bars(
        surface,
        layout
            .vertical
            .map(|bar| (bar, tree.vertical_info(&layout))),
        layout
            .horizontal
            .map(|bar| (bar, tree.horizontal_info(&layout))),
        look.enabled,
        palette,
    );
    surface.set_clip(saved);
}

#[allow(clippy::too_many_arguments)]
fn paint_item(
    surface: &mut Surface<'_>,
    tree: &TreeView,
    layout: &TreeLayout,
    index: usize,
    place: usize,
    look: TreeLook,
    images: &dyn Images,
    palette: &Palette,
    common: &CommonPalette,
) {
    let geometry = tree.geometry(layout, look.style, index, place);
    let row = geometry.row;
    let line = tree.line_colour.unwrap_or(common.tree_line);
    let text = match look.enabled {
        true => tree.text_colour.unwrap_or(palette.text),
        false => palette.text_disabled,
    };
    let indent = tree.indent;
    let lead = TreeView::lead(look.style);
    if look.style & style::HAS_LINES != 0 && geometry.depth as i32 + lead >= 1 {
        let column = geometry.content_x - indent + indent / 2;
        let from = geometry.button.map_or(column, |button| button.right);
        for x in from..geometry.content_x {
            dot(surface, x, row.top + LINE_ROW, line);
        }
        let top = match tree.previous_sibling(index) {
            Some(_) => row.top,
            None => row.top + LINE_ROW,
        };
        let bottom = match tree.next_sibling(index) {
            Some(_) => row.bottom - 1,
            None => row.top + LINE_ROW,
        };
        let skip = geometry
            .button
            .map(|button| (button.top, button.bottom - 1));
        dotted_column(surface, column, top, bottom, skip, line);
        let mut ancestor = tree.parent(index);
        let mut depth = geometry.depth as i32 - 1;
        while let Some(at) = ancestor {
            let ancestor_x = geometry.content_x - indent * (geometry.depth as i32 - depth);
            if depth + lead >= 1 && tree.next_sibling(at).is_some() {
                let column = ancestor_x - indent + indent / 2;
                dotted_column(surface, column, row.top, row.bottom - 1, None, line);
            }
            ancestor = tree.parent(at);
            depth -= 1;
        }
    }
    if let Some(button) = geometry.button {
        surface.fill(button, tree.background.unwrap_or(palette.field_fill));
        outline(surface, button, line);
        surface.fill(
            Rect::sized(
                button.left + BOX_MARK_FROM,
                button.top + BOX_MIDDLE,
                BOX_MARK_LENGTH,
                1,
            ),
            text,
        );
        if !tree.is_expanded(index) {
            surface.fill(
                Rect::sized(
                    button.left + BOX_MIDDLE,
                    button.top + BOX_MARK_FROM,
                    1,
                    BOX_MARK_LENGTH,
                ),
                text,
            );
        }
    }
    let item_state = tree.state(index);
    if let Some(check_box) = geometry.check_box {
        let checked =
            super::list_view::state::image_of(item_state) == super::list_view::state::CHECKED;
        draw_check_box(
            surface,
            check_box.left,
            check_box.top,
            checked,
            look.enabled,
            palette,
        );
    }
    let selected = item_state & state::SELECTED != 0;
    if let Some(image) = geometry.image {
        let shown = match selected {
            true => tree.selected_image(index),
            false => tree.image(index),
        };
        images.draw(surface, tree.images, shown, image.left, image.top, false);
    }
    let area = TreeView::selection_area(&geometry, look.style);
    let shows_inactive = look.style & style::SHOW_SELECTION_ALWAYS != 0;
    let (fill, colour) = match () {
        () if !selected || !look.enabled => (None, text),
        () if look.focused => (Some(palette.highlight), palette.highlight_text),
        () if shows_inactive => (
            Some(common.selection_inactive),
            common.selection_inactive_text,
        ),
        () => (None, text),
    };
    if let Some(fill) = fill {
        surface.fill(area, fill);
    }
    draw_plain(
        surface,
        geometry.label_x,
        row.top + LABEL_TOP,
        tree.labels.get(index),
        geometry.room,
        colour,
    );
    if look.focused && selected && look.enabled {
        dotted_outline(surface, area, palette.highlight_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::LIGHT;
    use crate::controls::common::{COMMON_LIGHT, NoImages};

    const WIDTH: u32 = 480;
    const HEIGHT: u32 = 300;
    const AHK_STYLE: u32 =
        style::HAS_BUTTONS | style::HAS_LINES | style::LINES_AT_ROOT | style::SHOW_SELECTION_ALWAYS;
    const LINE: u32 = 0x00a0_a0a0;
    const WHITE: u32 = 0x00ff_ffff;
    const BLUE: u32 = 0x0000_78d7;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn item(text: &[u16]) -> NewItem<'_> {
        NewItem {
            text,
            image: 0,
            selected_image: 0,
            state: 0,
            lparam: 0,
            children: None,
        }
    }

    fn add(tree: &mut TreeView, parent: Option<usize>, text: &str) -> usize {
        let text = units(text);
        tree.insert(parent, Place::Last, &item(&text)).unwrap()
    }

    fn example() -> (TreeView, usize, usize) {
        let mut tree = TreeView::new();
        let fruit = add(&mut tree, None, "Fruit");
        for name in ["Apples", "Bananas", "Cherries", "Dates", "Elderberries"] {
            add(&mut tree, Some(fruit), name);
        }
        let vegetables = add(&mut tree, None, "Vegetables");
        add(&mut tree, Some(vegetables), "Carrots");
        add(&mut tree, Some(vegetables), "Leeks");
        tree.set_selected(Some(fruit));
        (tree, fruit, vegetables)
    }

    fn render(tree: &TreeView, look: TreeLook, images: &dyn Images) -> Vec<u32> {
        let mut pixels = vec![0x00f0_f0f0u32; (WIDTH * HEIGHT) as usize];
        let mut surface = Surface::new(&mut pixels, WIDTH, HEIGHT).unwrap();
        paint(
            &mut surface,
            Rect::sized(12, 64, 456, 208),
            tree,
            look,
            images,
            &LIGHT,
            &COMMON_LIGHT,
        );
        pixels
    }

    fn at(pixels: &[u32], x: i32, y: i32) -> u32 {
        pixels[y as usize * WIDTH as usize + x as usize]
    }

    fn cell_has(pixels: &[u32], x: i32, y: i32, colour: u32) -> bool {
        (x..x + 8).any(|px| (y..y + 16).any(|py| at(pixels, px, py) == colour))
    }

    fn look(focused: bool) -> TreeLook {
        TreeLook {
            style: AHK_STYLE,
            focused,
            enabled: true,
        }
    }

    #[test]
    fn the_collapsed_example_has_boxes_lines_and_an_inactive_selection() {
        let (tree, _, _) = example();
        let pixels = render(&tree, look(false), &NoImages);
        assert_eq!(at(&pixels, 12, 64), 0x007a_7a7a);
        assert_eq!(at(&pixels, 20, 70), LINE);
        assert_eq!(at(&pixels, 28, 78), LINE);
        assert_eq!(at(&pixels, 21, 71), WHITE);
        for x in 22..27 {
            assert_eq!(at(&pixels, x, 74), 0);
        }
        for y in 72..77 {
            assert_eq!(at(&pixels, 24, y), 0);
        }
        assert_eq!(at(&pixels, 29, 74), WHITE);
        for x in [30, 32, 34] {
            assert_eq!(at(&pixels, x, 74), LINE);
        }
        assert_eq!(at(&pixels, 31, 74), WHITE);
        assert_eq!(at(&pixels, 35, 74), WHITE);
        for y in [79, 81, 83, 85, 87, 89] {
            assert_eq!(at(&pixels, 25, y), LINE);
        }
        for y in [80, 82, 84, 91, 93, 95, 97, 99, 101] {
            assert_ne!(at(&pixels, 25, y), LINE);
        }
        assert_eq!(at(&pixels, 25, 69), WHITE);
        assert_eq!(at(&pixels, 37, 66), COMMON_LIGHT.selection_inactive);
        assert_eq!(at(&pixels, 80, 83), COMMON_LIGHT.selection_inactive);
        assert_eq!(at(&pixels, 81, 66), WHITE);
        assert!(cell_has(&pixels, 39, 67, 0));
        assert_eq!(at(&pixels, 20, 90), LINE);
        assert!(cell_has(&pixels, 39, 87, 0));
        assert_eq!(at(&pixels, 24, 93), 0);
    }

    #[test]
    fn the_expanded_example_draws_the_children_and_the_long_column() {
        let (mut tree, fruit, vegetables) = example();
        assert!(tree.set_expanded(fruit, true));
        let pixels = render(&tree, look(true), &NoImages);
        assert_eq!(at(&pixels, 24, 72), WHITE);
        assert_eq!(at(&pixels, 24, 74), 0);
        assert_eq!(at(&pixels, 38, 67), BLUE);
        assert_eq!(at(&pixels, 38, 66), WHITE);
        assert_eq!(at(&pixels, 39, 66), BLUE);
        assert!(cell_has(&pixels, 39, 67, WHITE));
        for (row, name) in [(85, "Apples"), (105, "Bananas"), (165, "Elderberries")] {
            assert!(cell_has(&pixels, 59, row + 2, 0), "{name}");
            assert!(!cell_has(&pixels, 51, row + 2, 0));
            assert_eq!(at(&pixels, 46, row + 9), LINE);
            assert_eq!(at(&pixels, 54, row + 9), LINE);
            assert_eq!(at(&pixels, 55, row + 9), WHITE);
        }
        assert_eq!(at(&pixels, 45, 92), WHITE);
        assert_eq!(at(&pixels, 45, 94), WHITE);
        assert_eq!(at(&pixels, 45, 95), LINE);
        assert_eq!(at(&pixels, 45, 173), LINE);
        assert_eq!(at(&pixels, 45, 175), WHITE);
        for y in (79..=189).filter(|y| y % 2 == 1) {
            assert_eq!(at(&pixels, 25, y), LINE, "{y}");
        }
        assert_eq!(at(&pixels, 20, 190), LINE);
        assert_eq!(at(&pixels, 24, 196), 0);
        let layout = tree.layout(Rect::sized(12, 64, 456, 208), AHK_STYLE);
        let place = tree.visible_index(vegetables).unwrap();
        assert_eq!(place, 6);
        assert_eq!(
            tree.geometry(&layout, AHK_STYLE, vegetables, place).button,
            Some(Rect::sized(20, 190, 9, 9))
        );
        assert_eq!(
            tree.hit(&layout, AHK_STYLE, 24, 74),
            TreeHit::Item {
                index: fruit,
                part: TreePart::Button
            }
        );
        let apples = tree.first_child(fruit).unwrap();
        tree.set_selected(tree.step(key::DOWN, layout.visible));
        assert_eq!(tree.selected(), Some(apples));
        let pixels = render(&tree, look(true), &NoImages);
        assert_eq!(at(&pixels, 58, 87), BLUE);
        assert_eq!(at(&pixels, 108, 103), BLUE);
        assert_eq!(at(&pixels, 109, 90), WHITE);
        assert!(cell_has(&pixels, 59, 87, WHITE));
        assert_eq!(at(&pixels, 38, 67), WHITE);
    }

    #[test]
    fn hit_testing_names_every_part() {
        let (mut tree, fruit, _) = example();
        tree.set_expanded(fruit, true);
        let layout = tree.layout(Rect::sized(12, 64, 456, 208), AHK_STYLE);
        let hit = |x, y| tree.hit(&layout, AHK_STYLE, x, y);
        assert_eq!(
            hit(40, 70),
            TreeHit::Item {
                index: fruit,
                part: TreePart::Label
            }
        );
        assert_eq!(
            hit(16, 70),
            TreeHit::Item {
                index: fruit,
                part: TreePart::Indent
            }
        );
        assert_eq!(
            hit(200, 70),
            TreeHit::Item {
                index: fruit,
                part: TreePart::Right
            }
        );
        assert_eq!(hit(200, 250), TreeHit::Below);
        assert_eq!(hit(12, 64), TreeHit::Nowhere);
    }

    #[test]
    fn without_lines_at_root_roots_have_no_box_and_a_custom_indent_moves_everything() {
        let mut tree = TreeView::new();
        let root = add(&mut tree, None, "Root");
        let child = add(&mut tree, Some(root), "Child");
        let grandchild = add(&mut tree, Some(child), "Grand");
        tree.set_expanded(root, true);
        tree.set_expanded(child, true);
        let style = style::HAS_BUTTONS | style::HAS_LINES;
        let layout = tree.layout(Rect::sized(0, 0, 300, 200), style);
        let root_geometry = tree.geometry(&layout, style, root, 0);
        assert_eq!(root_geometry.content_x, 3);
        assert_eq!(root_geometry.button, None);
        assert_eq!(root_geometry.label_x, 7);
        let child_geometry = tree.geometry(&layout, style, child, 1);
        assert_eq!(child_geometry.content_x, 23);
        assert_eq!(child_geometry.button, Some(Rect::sized(8, 26, 9, 9)));
        let grand_geometry = tree.geometry(&layout, style, grandchild, 2);
        assert_eq!(grand_geometry.content_x, 43);
        tree.set_indent(24);
        let grand_geometry = tree.geometry(&layout, style, grandchild, 2);
        assert_eq!(grand_geometry.content_x, 51);
        let child_geometry = tree.geometry(&layout, style, child, 1);
        assert_eq!(child_geometry.button, Some(Rect::sized(10, 26, 9, 9)));
        tree.set_indent(4);
        assert_eq!(tree.indent(), SMALLEST_INDENT);
        let empty = units("");
        let blank = tree.insert(None, Place::Last, &item(&empty)).unwrap();
        let place = tree.visible_index(blank).unwrap();
        assert_eq!(
            tree.geometry(&layout, style, blank, place)
                .label_area
                .width(),
            4
        );
    }

    struct Squares;

    impl Images for Squares {
        fn size(&self, _list: usize) -> Option<(i32, i32)> {
            Some((16, 16))
        }

        fn draw(&self, surface: &mut Surface<'_>, _: usize, index: i32, x: i32, y: i32, _: bool) {
            surface.fill(Rect::sized(x, y, 16, 16), 0x0010_0000 + index as u32);
        }
    }

    #[test]
    fn check_boxes_and_images_move_the_label() {
        let (mut tree, fruit, vegetables) = example();
        tree.images = 5;
        tree.set_selected_image(fruit, 3);
        tree.set_state(vegetables, 2 << 12, state::STATE_IMAGE_MASK);
        let style = AHK_STYLE | style::CHECK_BOXES;
        let layout = tree.layout(Rect::sized(12, 64, 456, 208), style);
        let geometry = tree.geometry(&layout, style, fruit, 0);
        assert_eq!(geometry.check_box, Some(Rect::sized(38, 68, 14, 14)));
        assert_eq!(geometry.image, Some(Rect::sized(55, 67, 16, 16)));
        assert_eq!(geometry.label_x, 75);
        let look = TreeLook {
            style,
            focused: true,
            enabled: true,
        };
        let pixels = render(&tree, look, &Squares);
        assert_eq!(at(&pixels, 60, 70), 0x0010_0003);
        assert_eq!(at(&pixels, 60, 90), 0x0010_0000);
        assert_eq!(at(&pixels, 41, 95), LIGHT.box_mark);
        assert!(cell_has(&pixels, 75, 67, WHITE));
    }

    #[test]
    fn keys_walk_the_visible_items() {
        let (mut tree, fruit, vegetables) = example();
        assert_eq!(tree.step(key::DOWN, 9), Some(vegetables));
        tree.set_expanded(fruit, true);
        let apples = tree.first_child(fruit).unwrap();
        assert_eq!(tree.step(key::DOWN, 9), Some(apples));
        tree.set_selected(Some(vegetables));
        let elderberries = tree.previous_visible(vegetables).unwrap();
        assert_eq!(tree.text(elderberries), units("Elderberries"));
        assert_eq!(tree.step(key::HOME, 9), Some(fruit));
        assert_eq!(tree.step(key::END, 9), Some(vegetables));
        tree.set_expanded(vegetables, true);
        assert_eq!(tree.text(tree.step(key::END, 9).unwrap()), units("Leeks"));
        assert_eq!(
            tree.step(key::PAGE_UP, 3)
                .map(|index| tree.text(index).to_vec()),
            Some(units("Dates"))
        );
        assert_eq!(tree.visible_count(), 9);
        assert_eq!(
            tree.find_by_first_glyph(u32::from(b'c')),
            tree.first_child(vegetables)
        );
        assert_eq!(tree.find_by_first_glyph(u32::from(b'F')), Some(fruit));
        assert_eq!(tree.find_by_first_glyph(u32::from(b'z')), None);
    }

    #[test]
    fn handles_outlive_nothing_and_deletion_goes_children_first() {
        let (mut tree, fruit, vegetables) = example();
        let handle = tree.handle(fruit);
        assert_eq!(tree.index_of(handle), Some(fruit));
        let mut order = [0u16; TREE_CAPACITY];
        let count = tree.postorder(fruit, &mut order);
        assert_eq!(count, 6);
        assert_eq!(usize::from(order[5]), fruit);
        assert!(tree.remove(fruit));
        assert_eq!(tree.count(), 3);
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.index_of(handle), None);
        assert_eq!(tree.first_root(), Some(vegetables));
        let again = add(&mut tree, None, "Again");
        assert_eq!(again, fruit);
        assert_ne!(tree.handle(again), handle);
        assert_eq!(tree.index_of(handle), None);
    }

    #[test]
    fn sorted_inserts_and_sorting_order_labels_ignoring_case() {
        let mut tree = TreeView::new();
        for name in ["pear", "Apple", "fig"] {
            let text = units(name);
            tree.insert(None, Place::Sorted, &item(&text));
        }
        let mut roots = [0u16; 8];
        let count = tree.children(None, &mut roots);
        let names: Vec<Vec<u16>> = roots[..count]
            .iter()
            .map(|&index| tree.text(usize::from(index)).to_vec())
            .collect();
        assert_eq!(names, [units("Apple"), units("fig"), units("pear")]);
        let mut tree = TreeView::new();
        for name in ["c", "B", "a"] {
            add(&mut tree, None, name);
        }
        tree.sort_children(None, false);
        let count = tree.children(None, &mut roots);
        let names: Vec<Vec<u16>> = roots[..count]
            .iter()
            .map(|&index| tree.text(usize::from(index)).to_vec())
            .collect();
        assert_eq!(names, [units("a"), units("B"), units("c")]);
    }

    #[test]
    fn a_long_label_brings_the_horizontal_bar_and_collapsed_items_have_no_height() {
        let (mut tree, fruit, _) = example();
        let long = units(&"x".repeat(80));
        tree.set_text(fruit, &long);
        let rect = Rect::sized(12, 64, 456, 208);
        let layout = tree.layout(rect, AHK_STYLE);
        assert!(layout.horizontal.is_some());
        assert_eq!(layout.widest, 39 - 2 + 640 + 4 - 13);
        tree.set_hpos(10_000, &layout);
        assert_eq!(tree.hpos, layout.widest - 454);
        let narrow = tree.layout(rect, AHK_STYLE | style::NO_HORIZONTAL_SCROLL);
        assert!(narrow.horizontal.is_none());
        let apples = tree.first_child(fruit).unwrap();
        let hidden = tree.item_bounds(&layout, apples);
        assert_eq!(hidden.height(), 0);
        assert_eq!(hidden.top, layout.rows.bottom);
    }
}

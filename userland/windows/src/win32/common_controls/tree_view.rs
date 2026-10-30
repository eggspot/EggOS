//! `SysTreeView32` (D178, `docs/ux/windows-controls-v0.3.md` section 25): the tree view's
//! procedure over `eggos_windows_gui`'s [`TreeView`] model. Selecting and expanding by the user
//! (and `TVM_ENSUREVISIBLE`'s expanding) are refusable `TVN_SELCHANGINGW` and
//! `TVN_ITEMEXPANDINGW` notifications followed by their `-ED` ones, sent with the GUI lock let
//! go; `TVM_EXPAND` changes the state silently. Items are named by handles that stay valid until
//! the item is deleted. Its items are elements of agents' tree, nested as the items are.

use super::super::controls::point;
use super::super::dialog::{has_focus, release_capture, set_capture, set_focus};
use super::super::gui::Gui;
use super::super::user32::now_ms;
use super::super::{call_program, window_text};
use super::list_view::{self, text_at, write_rect};
use super::{
    Model, NotifyHeader, allocate, code, default_procedure, image_list, invalidate, item_element,
    notify_parent, paint_with, read_state, utf8, with_state,
};
use crate::gui::message;
use crate::gui::windows::{self, CommonKind, Window};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, Outcome, Role, TreeWriter, flags,
};
use eggos_windows_gui::controls::LIGHT;
use eggos_windows_gui::controls::common::COMMON_LIGHT;
use eggos_windows_gui::controls::list_view::{stable_sort, state as state_image, thumb_position};
use eggos_windows_gui::controls::scroll_bar::{self, Part};
use eggos_windows_gui::controls::tree_view::{
    self as model, HORIZONTAL_LINE, ITEM_HEIGHT, NewItem, Place, TREE_CAPACITY, TreeHit,
    TreeLayout, TreeLook, TreePart, TreeView, key, state, style,
};

const TVM_FIRST: u32 = 0x1100;
const TVM_LAST: u32 = 0x11ff;
const TVM_DELETEITEM: u32 = 0x1101;
const TVM_EXPAND: u32 = 0x1102;
const TVM_GETITEMRECT: u32 = 0x1104;
const TVM_GETCOUNT: u32 = 0x1105;
const TVM_GETINDENT: u32 = 0x1106;
const TVM_SETINDENT: u32 = 0x1107;
const TVM_GETIMAGELIST: u32 = 0x1108;
const TVM_SETIMAGELIST: u32 = 0x1109;
const TVM_GETNEXTITEM: u32 = 0x110a;
const TVM_SELECTITEM: u32 = 0x110b;
const TVM_GETEDITCONTROL: u32 = 0x110f;
const TVM_GETVISIBLECOUNT: u32 = 0x1110;
const TVM_HITTEST: u32 = 0x1111;
const TVM_SORTCHILDREN: u32 = 0x1113;
const TVM_ENSUREVISIBLE: u32 = 0x1114;
const TVM_SORTCHILDRENCB: u32 = 0x1115;
const TVM_SETITEMHEIGHT: u32 = 0x111b;
const TVM_GETITEMHEIGHT: u32 = 0x111c;
const TVM_SETBKCOLOR: u32 = 0x111d;
const TVM_SETTEXTCOLOR: u32 = 0x111e;
const TVM_GETBKCOLOR: u32 = 0x111f;
const TVM_GETTEXTCOLOR: u32 = 0x1120;
const TVM_GETITEMSTATE: u32 = 0x1127;
const TVM_SETLINECOLOR: u32 = 0x1128;
const TVM_GETLINECOLOR: u32 = 0x1129;
const TVM_SETEXTENDEDSTYLE: u32 = 0x112c;
const TVM_GETEXTENDEDSTYLE: u32 = 0x112d;
const TVM_INSERTITEMW: u32 = 0x1132;
const TVM_GETITEMW: u32 = 0x113e;
const TVM_SETITEMW: u32 = 0x113f;
const CCM_GETUNICODEFORMAT: u32 = 0x2006;
const WM_HSCROLL: u32 = 0x0114;
const WM_VSCROLL: u32 = 0x0115;
const ACT_ON_ITEM: u32 = 0x03f2;

const TVN_KEYDOWN: i32 = -412;
const TVN_ITEMCHANGINGW: i32 = -417;
const TVN_ITEMCHANGEDW: i32 = -419;
const TVN_SELCHANGINGW: i32 = -450;
const TVN_SELCHANGEDW: i32 = -451;
const TVN_ITEMEXPANDINGW: i32 = -454;
const TVN_ITEMEXPANDEDW: i32 = -455;
const TVN_DELETEITEMW: i32 = -458;

const TVIF_TEXT: u32 = 0x1;
const TVIF_IMAGE: u32 = 0x2;
const TVIF_PARAM: u32 = 0x4;
const TVIF_STATE: u32 = 0x8;
const TVIF_HANDLE: u32 = 0x10;
const TVIF_SELECTEDIMAGE: u32 = 0x20;
const TVIF_CHILDREN: u32 = 0x40;
const TVI_ROOT: isize = -0x10000;
const TVI_FIRST: isize = -0xffff;
const TVI_LAST: isize = -0xfffe;
const TVI_SORT: isize = -0xfffd;
const TVGN_ROOT: usize = 0x0;
const TVGN_NEXT: usize = 0x1;
const TVGN_PREVIOUS: usize = 0x2;
const TVGN_PARENT: usize = 0x3;
const TVGN_CHILD: usize = 0x4;
const TVGN_FIRSTVISIBLE: usize = 0x5;
const TVGN_NEXTVISIBLE: usize = 0x6;
const TVGN_PREVIOUSVISIBLE: usize = 0x7;
const TVGN_CARET: usize = 0x9;
const TVGN_LASTVISIBLE: usize = 0xa;
const TVE_COLLAPSE: u32 = 0x1;
const TVE_EXPAND: u32 = 0x2;
const TVE_TOGGLE: u32 = 0x3;
const TVE_ACTION_MASK: u32 = 0x3;
const TVE_COLLAPSERESET: u32 = 0x8000;
const TVC_UNKNOWN: u32 = 0x0;
const TVC_BYMOUSE: u32 = 0x1;
const TVC_BYKEYBOARD: u32 = 0x2;
const TVHT_NOWHERE: u32 = 0x1;
const TVHT_ONITEMICON: u32 = 0x2;
const TVHT_ONITEMLABEL: u32 = 0x4;
const TVHT_ONITEMINDENT: u32 = 0x8;
const TVHT_ONITEMBUTTON: u32 = 0x10;
const TVHT_ONITEMRIGHT: u32 = 0x20;
const TVHT_ONITEMSTATEICON: u32 = 0x40;
const TVHT_ABOVE: u32 = 0x100;
const TVHT_BELOW: u32 = 0x200;
const TVHT_TORIGHT: u32 = 0x400;
const TVHT_TOLEFT: u32 = 0x800;
const TVSIL_NORMAL: usize = 0;
const TVSIL_STATE: usize = 2;
const CLR_DEFAULT: u32 = 0xff00_0000;
const CLR_NONE: u32 = 0xffff_ffff;
const NO_COLOUR: usize = usize::MAX;
const ALL_STATE_BITS: u32 = 0xffff;
const SB_LINEUP: usize = 0;
const SB_LINEDOWN: usize = 1;
const SB_PAGEUP: usize = 2;
const SB_PAGEDOWN: usize = 3;
const SB_THUMBPOSITION: usize = 4;
const SB_THUMBTRACK: usize = 5;
const SB_TOP: usize = 6;
const SB_BOTTOM: usize = 7;
const DOUBLE_CLICK_MS: u32 = 400;
const DOUBLE_CLICK_DISTANCE: i32 = 4;
const LAST_PRINTABLE_CONTROL: u32 = 0x20;
const DELETE_CHARACTER: u32 = 0x7f;
const KEY_CHARACTERS: [u32; 4] = [b'+' as u32, b'-' as u32, b'*' as u32, b'=' as u32];

/// Win32's `TVITEMW`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct TreeItem {
    mask: u32,
    item: usize,
    state: u32,
    state_mask: u32,
    text: usize,
    text_max: i32,
    image: i32,
    selected_image: i32,
    children: i32,
    lparam: usize,
}

/// Win32's `TVINSERTSTRUCTW`, its item read as a `TVITEMW`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct InsertRequest {
    parent: usize,
    after: usize,
    item: TreeItem,
}

/// Win32's `NMTREEVIEWW`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NmTreeView {
    header: NotifyHeader,
    action: u32,
    old: TreeItem,
    new: TreeItem,
    x: i32,
    y: i32,
}

/// Win32's `NMTVKEYDOWN`, packed to one byte as commctrl.h declares it.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct NmTreeKeyDown {
    header: NotifyHeader,
    key: u16,
    flags: u32,
}

/// Win32's `NMTVITEMCHANGE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NmTreeItemChange {
    header: NotifyHeader,
    changed: u32,
    item: usize,
    new_state: u32,
    old_state: u32,
    lparam: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct HitTestInfo {
    x: i32,
    y: i32,
    flags: u32,
    item: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SortRequest {
    parent: usize,
    compare: usize,
    lparam: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drag {
    vertical: bool,
    grab: i32,
}

struct TreeState {
    tree: TreeView,
    extended: u32,
    last_down: Option<(u32, i32, i32, usize)>,
    drag: Option<Drag>,
}

impl Model for TreeState {
    const KIND: CommonKind = CommonKind::TreeView;
}

/// A new tree view's state; `None` without memory.
pub(super) fn new_state(_style: u32) -> Option<usize> {
    allocate(TreeState {
        tree: TreeView::new(),
        extended: 0,
        last_down: None,
        drag: None,
    })
}

/// A tree view's role in agents' tree: a focusable list.
pub(super) fn role(_window: &Window) -> (Role, u16) {
    (Role::List, flags::FOCUSABLE)
}

fn layout_of(entry: &Window, tree: &TreeView) -> TreeLayout {
    tree.layout(entry.client(), entry.style)
}

fn swap_red_blue(colour: u32) -> u32 {
    ((colour & 0xff) << 16) | (colour & 0xff00) | ((colour >> 16) & 0xff)
}

fn colour_parameter(value: usize) -> Option<u32> {
    match value as u32 {
        CLR_NONE | CLR_DEFAULT => None,
        colour => Some(swap_red_blue(colour)),
    }
}

/// Keeps the scroll within the items, and the selected item in view with `follow`, then
/// repaints.
fn refresh(window: usize, follow: bool) {
    with_state::<TreeState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.tree);
        match follow {
            true => control.tree.settle(layout.visible),
            false => {
                let top = control.tree.top();
                control.tree.set_top(top, layout.visible);
            }
        }
        let layout = layout_of(entry, &control.tree);
        let hpos = control.tree.hpos;
        control.tree.set_hpos(hpos, &layout);
    });
    invalidate(window);
}

fn item_info(tree: &TreeView, index: usize) -> TreeItem {
    TreeItem {
        mask: TVIF_HANDLE | TVIF_STATE | TVIF_PARAM,
        item: tree.handle(index),
        state: tree.state(index),
        state_mask: ALL_STATE_BITS,
        lparam: tree.lparam(index),
        ..TreeItem::default()
    }
}

fn info_of(window: usize, index: Option<usize>) -> TreeItem {
    let Some(index) = index else {
        return TreeItem::default();
    };
    with_state::<TreeState, _>(window, |_, control| {
        control
            .tree
            .exists(index)
            .then(|| item_info(&control.tree, index))
    })
    .flatten()
    .unwrap_or_default()
}

fn send_tree(window: usize, code: i32, action: u32, old: TreeItem, new: TreeItem) -> usize {
    notify_parent(window, code, |header| NmTreeView {
        header,
        action,
        old,
        new,
        x: 0,
        y: 0,
    })
}

fn is_ancestor(tree: &TreeView, ancestor: usize, index: usize) -> bool {
    let mut at = tree.parent(index);
    while let Some(parent) = at {
        if parent == ancestor {
            return true;
        }
        at = tree.parent(parent);
    }
    false
}

fn reveal(tree: &mut TreeView, index: usize) {
    let mut at = tree.parent(index);
    while let Some(parent) = at {
        tree.set_expanded(parent, true);
        at = tree.parent(parent);
    }
}

/// Selects `target` (none when `None`) for `cause` (`TVC_*`): `TVN_SELCHANGINGW`, which the
/// program may refuse, the change (its collapsed ancestors opened silently), then
/// `TVN_SELCHANGEDW`; whether it changed.
fn select(window: usize, target: Option<usize>, cause: u32) -> bool {
    let found = with_state::<TreeState, _>(window, |_, control| {
        let old = control.tree.selected();
        let valid = target.is_none_or(|target| control.tree.exists(target));
        (valid && old != target).then_some(old)
    })
    .flatten();
    let Some(old) = found else {
        return false;
    };
    let (old_info, new_info) = (info_of(window, old), info_of(window, target));
    if send_tree(window, TVN_SELCHANGINGW, cause, old_info, new_info) != 0 {
        return false;
    }
    let changed = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        let target =
            target.filter(|_| new_info.item == 0 || tree.index_of(new_info.item).is_some());
        if let Some(target) = target {
            reveal(tree, target);
        }
        tree.set_selected(target);
        true
    })
    .unwrap_or(false);
    refresh(window, true);
    if changed {
        let (old_info, new_info) = (info_of(window, old), info_of(window, target));
        send_tree(window, TVN_SELCHANGEDW, cause, old_info, new_info);
    }
    changed
}

/// Expands, collapses or toggles item `index` (`TVE_*`), with `TVN_ITEMEXPANDINGW` (which the
/// program may refuse) and `TVN_ITEMEXPANDEDW` when `notify`; collapsing an ancestor of the
/// selected item selects it. Whether the item can expand at all.
fn expand(window: usize, index: usize, action: u32, notify: bool) -> bool {
    let found = with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        if !tree.exists(index) {
            return None;
        }
        let expanded = tree.is_expanded(index);
        let wanted = match action & TVE_ACTION_MASK {
            TVE_EXPAND => true,
            TVE_COLLAPSE => false,
            TVE_TOGGLE => !expanded,
            _ => return None,
        };
        let expandable = tree.has_children(index) || tree.first_child(index).is_some();
        Some((wanted, expanded, expandable))
    })
    .flatten();
    let Some((wanted, expanded, expandable)) = found else {
        return false;
    };
    if !expandable {
        return false;
    }
    let resets = action & TVE_COLLAPSERESET != 0 && !wanted;
    if wanted == expanded && !resets {
        return true;
    }
    let notified_action = match wanted {
        true => TVE_EXPAND,
        false => TVE_COLLAPSE,
    };
    let empty = TreeItem::default();
    if notify
        && send_tree(
            window,
            TVN_ITEMEXPANDINGW,
            notified_action,
            empty,
            info_of(window, Some(index)),
        ) != 0
    {
        return false;
    }
    let hidden_selection = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        tree.set_expanded(index, wanted);
        !wanted
            && tree
                .selected()
                .is_some_and(|selected| is_ancestor(tree, index, selected))
    })
    .unwrap_or(false);
    if resets {
        let mut children = [0u16; TREE_CAPACITY];
        let count = with_state::<TreeState, _>(window, |_, control| {
            control.tree.set_state(index, 0, state::EXPANDED_ONCE);
            control.tree.children(Some(index), &mut children)
        })
        .unwrap_or(0);
        for &child in &children[..count] {
            delete_item(window, usize::from(child));
        }
    }
    if notify {
        send_tree(
            window,
            TVN_ITEMEXPANDEDW,
            notified_action,
            empty,
            info_of(window, Some(index)),
        );
    }
    if hidden_selection {
        select(window, Some(index), TVC_UNKNOWN);
    }
    refresh(window, false);
    true
}

/// Deletes item `index` and its descendants, each announced with `TVN_DELETEITEMW` children
/// first; the selection inside it moves to its parent first. Whether it existed.
fn delete_item(window: usize, index: usize) -> bool {
    let found = with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        if !tree.exists(index) {
            return None;
        }
        let selected_inside = tree
            .selected()
            .is_some_and(|selected| selected == index || is_ancestor(tree, index, selected));
        Some((tree.handle(index), selected_inside, tree.parent(index)))
    })
    .flatten();
    let Some((handle, selected_inside, parent)) = found else {
        return false;
    };
    if selected_inside {
        select(window, parent, TVC_UNKNOWN);
    }
    let mut doomed = [0u16; TREE_CAPACITY];
    let count = with_state::<TreeState, _>(window, |_, control| {
        control.tree.postorder(index, &mut doomed)
    })
    .unwrap_or(0);
    for &item in &doomed[..count] {
        let old = info_of(window, Some(usize::from(item)));
        if old.item != 0 {
            send_tree(window, TVN_DELETEITEMW, 0, old, TreeItem::default());
        }
    }
    let removed = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        tree.index_of(handle)
            .is_some_and(|index| tree.remove(index))
    })
    .unwrap_or(false);
    refresh(window, false);
    removed
}

fn delete_all(window: usize) -> bool {
    let mut deleted = false;
    while let Some(root) =
        with_state::<TreeState, _>(window, |_, control| control.tree.first_root()).flatten()
    {
        if !delete_item(window, root) {
            break;
        }
        deleted = true;
    }
    deleted
        || with_state::<TreeState, _>(window, |_, control| control.tree.count() == 0)
            .unwrap_or(false)
}

fn toggle_check(window: usize, index: usize) {
    let found = with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        tree.exists(index)
            .then(|| (tree.handle(index), tree.state(index), tree.lparam(index)))
    })
    .flatten();
    let Some((handle, old, lparam)) = found else {
        return;
    };
    let image = match state_image::image_of(old) {
        state_image::CHECKED => state_image::UNCHECKED,
        _ => state_image::CHECKED,
    };
    let new = (old & !state::STATE_IMAGE_MASK) | state_image::image_bits(image);
    let change = |code| {
        notify_parent(window, code, |header| NmTreeItemChange {
            header,
            changed: TVIF_STATE,
            item: handle,
            new_state: new,
            old_state: old,
            lparam,
        })
    };
    if change(TVN_ITEMCHANGINGW) != 0 {
        return;
    }
    with_state::<TreeState, _>(window, |_, control| {
        if control.tree.index_of(handle) == Some(index) {
            control.tree.set_state(index, new, state::STATE_IMAGE_MASK);
        }
    });
    invalidate(window);
    change(TVN_ITEMCHANGEDW);
}

fn act_on_key(window: usize, virtual_key: u32) {
    let found = with_state::<TreeState, _>(window, |entry, control| {
        let tree = &control.tree;
        let visible = layout_of(entry, tree).visible;
        let selected = tree.selected();
        let open = selected
            .is_some_and(|selected| tree.is_expanded(selected) && tree.has_children(selected));
        let closed = selected
            .is_some_and(|selected| !tree.is_expanded(selected) && tree.has_children(selected));
        (
            selected,
            tree.step(virtual_key, visible),
            open,
            closed,
            selected.and_then(|selected| tree.parent(selected)),
            selected.and_then(|selected| tree.first_child(selected)),
        )
    });
    let Some((selected, step, open, closed, parent, child)) = found else {
        return;
    };
    match virtual_key {
        key::RETURN => {
            notify_parent(window, code::NM_RETURN, |header| header);
        }
        key::UP | key::DOWN | key::HOME | key::END | key::PAGE_UP | key::PAGE_DOWN => {
            if let Some(target) = step {
                select(window, Some(target), TVC_BYKEYBOARD);
            }
        }
        key::LEFT => match (selected, open, parent) {
            (Some(selected), true, _) => {
                expand(window, selected, TVE_COLLAPSE, true);
            }
            (Some(_), false, Some(parent)) => {
                select(window, Some(parent), TVC_BYKEYBOARD);
            }
            _ => {}
        },
        key::RIGHT => match (selected, closed, child) {
            (Some(selected), true, _) => {
                expand(window, selected, TVE_EXPAND, true);
            }
            (Some(_), false, Some(child)) => {
                select(window, Some(child), TVC_BYKEYBOARD);
            }
            _ => {}
        },
        key::ADD | key::PLUS => {
            if let Some(selected) = selected {
                expand(window, selected, TVE_EXPAND, true);
            }
        }
        key::SUBTRACT | key::MINUS => {
            if let Some(selected) = selected {
                expand(window, selected, TVE_COLLAPSE, true);
            }
        }
        key::MULTIPLY => {
            if let Some(selected) = selected {
                expand_all(window, selected);
            }
        }
        key::BACK => {
            if let Some(parent) = parent {
                select(window, Some(parent), TVC_BYKEYBOARD);
            }
        }
        _ => {}
    }
}

fn expand_all(window: usize, index: usize) {
    let mut items = [0u16; TREE_CAPACITY];
    let count = with_state::<TreeState, _>(window, |_, control| {
        control.tree.postorder(index, &mut items)
    })
    .unwrap_or(0);
    for &item in items[..count].iter().rev() {
        expand(window, usize::from(item), TVE_EXPAND, true);
    }
}

fn key_down(window: usize, virtual_key: u32) {
    notify_parent(window, TVN_KEYDOWN, |header| NmTreeKeyDown {
        header,
        key: virtual_key as u16,
        flags: 0,
    });
    act_on_key(window, virtual_key);
}

fn character(window: usize, character: u32) {
    if character <= LAST_PRINTABLE_CONTROL
        || character == DELETE_CHARACTER
        || KEY_CHARACTERS.contains(&character)
    {
        return;
    }
    let target = with_state::<TreeState, _>(window, |_, control| {
        control.tree.find_by_first_glyph(character)
    })
    .flatten();
    if let Some(target) = target {
        select(window, Some(target), TVC_BYKEYBOARD);
    }
}

fn focus_changed(window: usize, gained: bool) {
    invalidate(window);
    let notification = match gained {
        true => code::NM_SETFOCUS,
        false => code::NM_KILLFOCUS,
    };
    notify_parent(window, notification, |header| header);
}

fn scroll_part(tree: &mut TreeView, layout: &TreeLayout, vertical: bool, part: Part) {
    let visible = layout.visible;
    let width = layout.rows.width();
    let top = tree.top();
    match (vertical, part) {
        (_, Part::Thumb) => {}
        (true, Part::LineUp) => tree.set_top(top.saturating_sub(1), visible),
        (true, Part::LineDown) => tree.set_top(top + 1, visible),
        (true, Part::PageUp) => tree.set_top(top.saturating_sub(visible), visible),
        (true, Part::PageDown) => tree.set_top(top + visible, visible),
        (false, Part::LineUp) => tree.set_hpos(tree.hpos - HORIZONTAL_LINE, layout),
        (false, Part::LineDown) => tree.set_hpos(tree.hpos + HORIZONTAL_LINE, layout),
        (false, Part::PageUp) => tree.set_hpos(tree.hpos - width, layout),
        (false, Part::PageDown) => tree.set_hpos(tree.hpos + width, layout),
    }
}

fn bar_down(window: usize, vertical: bool, part: Part, along: i32) {
    let dragging = with_state::<TreeState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.tree);
        if part != Part::Thumb {
            scroll_part(&mut control.tree, &layout, vertical, part);
            return Some(false);
        }
        let (bar, info) = match vertical {
            true => (layout.vertical?, control.tree.vertical_info(&layout)),
            false => (layout.horizontal?, control.tree.horizontal_info(&layout)),
        };
        let (start, length) = match vertical {
            true => (bar.top, bar.height()),
            false => (bar.left, bar.width()),
        };
        let (thumb, _) = scroll_bar::thumb(length, &info)?;
        control.drag = Some(Drag {
            vertical,
            grab: along - start - thumb,
        });
        Some(true)
    })
    .flatten();
    if dragging == Some(true) {
        set_capture(window);
    }
    invalidate(window);
}

fn button_down(window: usize, (x, y): (i32, i32), double_message: bool) {
    set_focus(window);
    let now = now_ms();
    let found = with_state::<TreeState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.tree);
        let hit = control.tree.hit(&layout, entry.style, x, y);
        let on_label = match hit {
            TreeHit::Item {
                index,
                part: TreePart::Label | TreePart::Icon,
            } => Some(index),
            _ => None,
        };
        let repeated = control
            .last_down
            .is_some_and(|(time, last_x, last_y, last)| {
                Some(last) == on_label
                    && now.wrapping_sub(time) <= DOUBLE_CLICK_MS
                    && (x - last_x).abs() <= DOUBLE_CLICK_DISTANCE
                    && (y - last_y).abs() <= DOUBLE_CLICK_DISTANCE
            });
        let double = on_label.is_some() && (double_message || repeated);
        control.last_down = match (double, on_label) {
            (false, Some(index)) => Some((now, x, y, index)),
            _ => None,
        };
        (hit, double, entry.style & style::FULL_ROW_SELECT != 0)
    });
    let Some((hit, double, full_row)) = found else {
        return;
    };
    match hit {
        TreeHit::Vertical(part) => bar_down(window, true, part, y),
        TreeHit::Horizontal(part) => bar_down(window, false, part, x),
        TreeHit::Item {
            index,
            part: TreePart::Button,
        } => {
            expand(window, index, TVE_TOGGLE, true);
        }
        TreeHit::Item {
            index,
            part: TreePart::CheckBox,
        } => toggle_check(window, index),
        TreeHit::Item { index, part }
            if full_row || matches!(part, TreePart::Label | TreePart::Icon) =>
        {
            if double {
                if notify_parent(window, code::NM_DBLCLK, |header| header) == 0 {
                    expand(window, index, TVE_TOGGLE, true);
                }
            } else if notify_parent(window, code::NM_CLICK, |header| header) == 0 {
                select(window, Some(index), TVC_BYMOUSE);
            }
        }
        TreeHit::Item { .. } | TreeHit::Below => {
            notify_parent(window, code::NM_CLICK, |header| header);
        }
        TreeHit::Nowhere => {}
    }
}

fn button_up(window: usize) {
    let dragged = with_state::<TreeState, _>(window, |_, control| control.drag.take()).flatten();
    if dragged.is_some() {
        release_capture();
    }
}

fn pointer_moved(window: usize, (x, y): (i32, i32)) {
    let moved = with_state::<TreeState, _>(window, |entry, control| {
        let drag = control.drag?;
        let layout = layout_of(entry, &control.tree);
        match drag.vertical {
            true => {
                let bar = layout.vertical?;
                let info = control.tree.vertical_info(&layout);
                let top = thumb_position(bar.height(), &info, y - bar.top - drag.grab);
                control.tree.set_top(top.max(0) as usize, layout.visible);
            }
            false => {
                let bar = layout.horizontal?;
                let info = control.tree.horizontal_info(&layout);
                let hpos = thumb_position(bar.width(), &info, x - bar.left - drag.grab);
                control.tree.set_hpos(hpos, &layout);
            }
        }
        Some(())
    })
    .flatten();
    if moved.is_some() {
        invalidate(window);
    }
}

fn scroll_message(window: usize, vertical: bool, wparam: usize) {
    let request = wparam & 0xffff;
    let position = ((wparam >> 16) & 0xffff) as i32;
    with_state::<TreeState, _>(window, |entry, control| {
        let layout = layout_of(entry, &control.tree);
        let tree = &mut control.tree;
        let part = match request {
            SB_LINEUP => Some(Part::LineUp),
            SB_LINEDOWN => Some(Part::LineDown),
            SB_PAGEUP => Some(Part::PageUp),
            SB_PAGEDOWN => Some(Part::PageDown),
            _ => None,
        };
        if let Some(part) = part {
            scroll_part(tree, &layout, vertical, part);
            return;
        }
        let target = match request {
            SB_THUMBPOSITION | SB_THUMBTRACK => position,
            SB_TOP => 0,
            SB_BOTTOM => i32::MAX,
            _ => return,
        };
        match vertical {
            true => tree.set_top(target.max(0) as usize, layout.visible),
            false => tree.set_hpos(target, &layout),
        }
    });
    invalidate(window);
}

fn paint(window: usize) {
    paint_with::<TreeState>(window, |surface, rect, entry, control, gui| {
        let look = TreeLook {
            style: entry.style,
            focused: has_focus(gui, window),
            enabled: windows::enabled_in_tree(&gui.windows, window),
        };
        model::paint(
            surface,
            rect,
            &control.tree,
            look,
            &image_list::LayerImages,
            &LIGHT,
            &COMMON_LIGHT,
        );
    });
}

fn handle_parameter(tree: &TreeView, value: usize) -> Option<usize> {
    tree.index_of(value)
}

fn insert_item(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `TVM_INSERTITEMW`'s `lParam` is the program's `TVINSERTSTRUCTW`, whose item is at
    // least a `TVITEMW`.
    let request = unsafe { (lparam as *const InsertRequest).read_unaligned() };
    let item = request.item;
    let text = match item.mask & TVIF_TEXT {
        0 => &[][..],
        // SAFETY: with `TVIF_TEXT`, `pszText` is a NUL-terminated string or the callback value.
        _ => unsafe { text_at(item.text) },
    };
    let new = NewItem {
        text,
        image: match item.mask & TVIF_IMAGE {
            0 => 0,
            _ => item.image,
        },
        selected_image: match item.mask & TVIF_SELECTEDIMAGE {
            0 => 0,
            _ => item.selected_image,
        },
        state: match item.mask & TVIF_STATE {
            0 => 0,
            _ => item.state & item.state_mask,
        },
        lparam: match item.mask & TVIF_PARAM {
            0 => 0,
            _ => item.lparam,
        },
        children: match (item.mask & TVIF_CHILDREN, item.children) {
            (0, _) => None,
            (_, 0) => Some(false),
            (_, 1) => Some(true),
            _ => None,
        },
    };
    let handle = with_state::<TreeState, _>(window, |entry, control| {
        let tree = &mut control.tree;
        let parent = match request.parent as isize {
            0 | TVI_ROOT => None,
            handle => Some(handle_parameter(tree, handle as usize)?),
        };
        let place = match request.after as isize {
            TVI_FIRST => Place::First,
            TVI_SORT => Place::Sorted,
            0 | TVI_LAST | TVI_ROOT => Place::Last,
            handle => handle_parameter(tree, handle as usize).map_or(Place::Last, Place::After),
        };
        let index = tree.insert(parent, place, &new)?;
        let checks = entry.style & style::CHECK_BOXES != 0;
        if checks && state_image::image_of(new.state) == 0 {
            let bits = state_image::image_bits(state_image::UNCHECKED);
            tree.set_state(index, bits, state::STATE_IMAGE_MASK);
        }
        Some(tree.handle(index))
    })
    .flatten();
    refresh(window, false);
    handle.unwrap_or(0)
}

fn get_item(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `TVM_GETITEMW`'s `lParam` is the program's `TVITEMW`.
    let item = unsafe { (lparam as *const TreeItem).read_unaligned() };
    let target = lparam as *mut TreeItem;
    with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        let Some(index) = tree.index_of(item.item) else {
            return 0;
        };
        // SAFETY: `target` is the program's `TVITEMW`, as read above; `pszText` holds
        // `cchTextMax` writable units when `TVIF_TEXT` asks for the label.
        unsafe {
            if item.mask & TVIF_TEXT != 0 {
                window_text::copy_out(tree.text(index), item.text, item.text_max.max(0) as usize);
            }
            if item.mask & TVIF_IMAGE != 0 {
                (&raw mut (*target).image).write_unaligned(tree.image(index));
            }
            if item.mask & TVIF_SELECTEDIMAGE != 0 {
                (&raw mut (*target).selected_image).write_unaligned(tree.selected_image(index));
            }
            if item.mask & TVIF_STATE != 0 {
                let bits = tree.state(index) & item.state_mask;
                (&raw mut (*target).state).write_unaligned(bits);
            }
            if item.mask & TVIF_PARAM != 0 {
                (&raw mut (*target).lparam).write_unaligned(tree.lparam(index));
            }
            if item.mask & TVIF_CHILDREN != 0 {
                let children = i32::from(tree.declared_children(index));
                (&raw mut (*target).children).write_unaligned(children);
            }
        }
        1
    })
    .unwrap_or(0)
}

fn set_item(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `TVM_SETITEMW`'s `lParam` is the program's `TVITEMW`.
    let item = unsafe { (lparam as *const TreeItem).read_unaligned() };
    let text = match item.mask & TVIF_TEXT {
        0 => &[][..],
        // SAFETY: with `TVIF_TEXT`, `pszText` is a NUL-terminated string or the callback value.
        _ => unsafe { text_at(item.text) },
    };
    let done = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        let Some(index) = tree.index_of(item.item) else {
            return false;
        };
        let mut done = true;
        if item.mask & TVIF_TEXT != 0 {
            done &= tree.set_text(index, text);
        }
        if item.mask & TVIF_IMAGE != 0 {
            tree.set_image(index, item.image);
        }
        if item.mask & TVIF_SELECTEDIMAGE != 0 {
            tree.set_selected_image(index, item.selected_image);
        }
        if item.mask & TVIF_STATE != 0 {
            tree.set_state(index, item.state, item.state_mask);
        }
        if item.mask & TVIF_PARAM != 0 {
            tree.set_lparam(index, item.lparam);
        }
        if item.mask & TVIF_CHILDREN != 0 {
            let declared = match item.children {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            };
            tree.set_declared_children(index, declared);
        }
        done
    })
    .unwrap_or(false);
    refresh(window, false);
    usize::from(done)
}

fn next_item(window: usize, relation: usize, handle: usize) -> usize {
    with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        let item = tree.index_of(handle);
        let found = match relation {
            TVGN_ROOT => tree.first_root(),
            TVGN_NEXT => item.and_then(|item| tree.next_sibling(item)),
            TVGN_PREVIOUS => item.and_then(|item| tree.previous_sibling(item)),
            TVGN_PARENT => item.and_then(|item| tree.parent(item)),
            TVGN_CHILD => match handle as isize {
                0 | TVI_ROOT => tree.first_root(),
                _ => item.and_then(|item| tree.first_child(item)),
            },
            TVGN_FIRSTVISIBLE => tree.visible_at(tree.top()),
            TVGN_NEXTVISIBLE => item
                .filter(|&item| tree.is_shown(item))
                .and_then(|item| tree.next_visible(item)),
            TVGN_PREVIOUSVISIBLE => item
                .filter(|&item| tree.is_shown(item))
                .and_then(|item| tree.previous_visible(item)),
            TVGN_CARET => tree.selected(),
            TVGN_LASTVISIBLE => tree.last_visible(),
            _ => None,
        };
        found.map_or(0, |found| tree.handle(found))
    })
    .unwrap_or(0)
}

fn select_item(window: usize, relation: usize, handle: usize) -> usize {
    let index =
        with_state::<TreeState, _>(window, |_, control| control.tree.index_of(handle)).flatten();
    match relation {
        TVGN_CARET => {
            if handle != 0 && index.is_none() {
                return 0;
            }
            select(window, index, TVC_UNKNOWN);
            1
        }
        TVGN_FIRSTVISIBLE => {
            let Some(index) = index else {
                return 0;
            };
            with_state::<TreeState, _>(window, |entry, control| {
                let visible = layout_of(entry, &control.tree).visible;
                if let Some(place) = control.tree.visible_index(index) {
                    control.tree.set_top(place, visible);
                }
            });
            refresh(window, false);
            1
        }
        _ => usize::from(index.is_some() || handle == 0),
    }
}

fn ensure_visible(window: usize, handle: usize) -> usize {
    let mut chain = [0u16; TREE_CAPACITY];
    let found = with_state::<TreeState, _>(window, |_, control| {
        let tree = &control.tree;
        let index = tree.index_of(handle)?;
        let mut count = 0;
        let mut at = tree.parent(index);
        while let Some(parent) = at {
            if count < chain.len() {
                chain[count] = parent as u16;
                count += 1;
            }
            at = tree.parent(parent);
        }
        Some((index, count))
    })
    .flatten();
    let Some((index, count)) = found else {
        return 0;
    };
    let mut expanded_any = false;
    for &ancestor in chain[..count].iter().rev() {
        let ancestor = usize::from(ancestor);
        let closed =
            with_state::<TreeState, _>(window, |_, control| !control.tree.is_expanded(ancestor))
                .unwrap_or(false);
        if closed {
            expand(window, ancestor, TVE_EXPAND, true);
            expanded_any = true;
        }
    }
    with_state::<TreeState, _>(window, |entry, control| {
        let visible = layout_of(entry, &control.tree).visible;
        if let Some(place) = control.tree.visible_index(index) {
            control.tree.ensure_visible(place, visible);
        }
    });
    refresh(window, false);
    usize::from(!expanded_any)
}

fn item_rect(window: usize, label_only: bool, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    // SAFETY: `TVM_GETITEMRECT`'s `lParam` is a `RECT` whose first bytes hold the item's
    // `HTREEITEM` on the way in.
    let handle = unsafe { (lparam as *const usize).read_unaligned() };
    let rect = with_state::<TreeState, _>(window, |entry, control| {
        let tree = &control.tree;
        let index = tree.index_of(handle)?;
        let place = tree.visible_index(index)?;
        let layout = layout_of(entry, tree);
        let geometry = tree.geometry(&layout, entry.style, index, place);
        Some(match label_only {
            true => geometry.label_area,
            false => geometry.row,
        })
    })
    .flatten();
    match rect {
        Some(rect) => {
            // SAFETY: as above: the program's `RECT`.
            unsafe { write_rect(lparam, rect) };
            1
        }
        None => 0,
    }
}

fn hit_test(window: usize, lparam: usize) -> usize {
    if lparam == 0 {
        return 0;
    }
    let info = lparam as *mut HitTestInfo;
    // SAFETY: `TVM_HITTEST`'s `lParam` is the program's `TVHITTESTINFO`, its point set.
    let (x, y) = unsafe {
        (
            (&raw const (*info).x).read_unaligned(),
            (&raw const (*info).y).read_unaligned(),
        )
    };
    let answer = with_state::<TreeState, _>(window, |entry, control| {
        let tree = &control.tree;
        let client = entry.client();
        let outside = match () {
            () if y < client.top => TVHT_ABOVE,
            () if y >= client.bottom => TVHT_BELOW,
            () if x < client.left => TVHT_TOLEFT,
            () if x >= client.right => TVHT_TORIGHT,
            () => 0,
        };
        if outside != 0 {
            return (outside, 0);
        }
        let layout = layout_of(entry, tree);
        match tree.hit(&layout, entry.style, x, y) {
            TreeHit::Item { index, part } => {
                let found = match part {
                    TreePart::Button => TVHT_ONITEMBUTTON,
                    TreePart::CheckBox => TVHT_ONITEMSTATEICON,
                    TreePart::Icon => TVHT_ONITEMICON,
                    TreePart::Label => TVHT_ONITEMLABEL,
                    TreePart::Indent => TVHT_ONITEMINDENT,
                    TreePart::Right => TVHT_ONITEMRIGHT,
                };
                (found, tree.handle(index))
            }
            _ => (TVHT_NOWHERE, 0),
        }
    });
    let (found, item) = answer.unwrap_or((TVHT_NOWHERE, 0));
    // SAFETY: as above.
    unsafe {
        (&raw mut (*info).flags).write_unaligned(found);
        (&raw mut (*info).item).write_unaligned(item);
    }
    item
}

fn sort_children(
    window: usize,
    handle: usize,
    recurse: bool,
    callback: Option<(usize, usize)>,
) -> usize {
    let parent = match handle as isize {
        0 | TVI_ROOT => Ok(None),
        _ => with_state::<TreeState, _>(window, |_, control| control.tree.index_of(handle))
            .flatten()
            .map(Some)
            .ok_or(()),
    };
    let Ok(parent) = parent else {
        return 0;
    };
    let Some((compare, extra)) = callback else {
        with_state::<TreeState, _>(window, |_, control| {
            control.tree.sort_children(parent, recurse)
        });
        refresh(window, false);
        return 1;
    };
    let mut order = [0u16; TREE_CAPACITY];
    let mut keys = [0usize; TREE_CAPACITY];
    let count = with_state::<TreeState, _>(window, |_, control| {
        let count = control.tree.children(parent, &mut order);
        for (key, &index) in keys.iter_mut().zip(&order[..count]) {
            *key = control.tree.lparam(usize::from(index));
        }
        count
    })
    .unwrap_or(0);
    let mut positions = [0u16; TREE_CAPACITY];
    for (index, slot) in positions[..count].iter_mut().enumerate() {
        *slot = index as u16;
    }
    let mut scratch = [0u16; TREE_CAPACITY];
    stable_sort(
        &mut positions[..count],
        &mut scratch[..count],
        &mut |a: u16, b: u16| {
            let (first, second) = (keys[usize::from(a)], keys[usize::from(b)]);
            // SAFETY: the program's `PFNTVCOMPARE`, `TVM_SORTCHILDRENCB`'s contract: it takes two
            // items' data and `lParam`, and returns an `int`; the GUI lock is not held.
            let answer = unsafe { call_program(compare, [first, second, extra, 0]) } as u32 as i32;
            answer.cmp(&0)
        },
    );
    let mut sorted = [0u16; TREE_CAPACITY];
    for (slot, &position) in sorted[..count].iter_mut().zip(&positions[..count]) {
        *slot = order[usize::from(position)];
    }
    let done = with_state::<TreeState, _>(window, |_, control| {
        control.tree.reorder_children(parent, &sorted[..count])
    })
    .unwrap_or(false);
    refresh(window, false);
    usize::from(done)
}

fn colour_message(window: usize, number: u32, lparam: usize) -> usize {
    let answer = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        let slot = match number {
            TVM_SETBKCOLOR | TVM_GETBKCOLOR => &mut tree.background,
            TVM_SETTEXTCOLOR | TVM_GETTEXTCOLOR => &mut tree.text_colour,
            _ => &mut tree.line_colour,
        };
        let previous = match number {
            TVM_SETLINECOLOR | TVM_GETLINECOLOR => slot.map_or(CLR_DEFAULT as usize, |colour| {
                swap_red_blue(colour) as usize
            }),
            _ => slot.map_or(NO_COLOUR, |colour| swap_red_blue(colour) as usize),
        };
        if matches!(number, TVM_SETBKCOLOR | TVM_SETTEXTCOLOR | TVM_SETLINECOLOR) {
            *slot = colour_parameter(lparam);
        }
        previous
    })
    .unwrap_or(NO_COLOUR);
    invalidate(window);
    answer
}

fn image_list_message(window: usize, set: bool, which: usize, list: usize) -> usize {
    let answer = with_state::<TreeState, _>(window, |_, control| {
        let tree = &mut control.tree;
        let slot = match which {
            TVSIL_NORMAL => &mut tree.images,
            TVSIL_STATE => &mut tree.state_images,
            _ => return 0,
        };
        match set {
            true => core::mem::replace(slot, list),
            false => *slot,
        }
    })
    .unwrap_or(0);
    if set {
        refresh(window, false);
    }
    answer
}

fn tree_message(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        TVM_INSERTITEMW => insert_item(window, lparam),
        TVM_DELETEITEM => match lparam as isize {
            0 | TVI_ROOT => usize::from(delete_all(window)),
            _ => {
                let index =
                    with_state::<TreeState, _>(window, |_, control| control.tree.index_of(lparam))
                        .flatten();
                index.map_or(0, |index| usize::from(delete_item(window, index)))
            }
        },
        TVM_EXPAND => {
            let index =
                with_state::<TreeState, _>(window, |_, control| control.tree.index_of(lparam))
                    .flatten();
            index.map_or(0, |index| {
                usize::from(expand(window, index, wparam as u32, false))
            })
        }
        TVM_GETITEMW => get_item(window, lparam),
        TVM_SETITEMW => set_item(window, lparam),
        TVM_GETNEXTITEM => next_item(window, wparam, lparam),
        TVM_SELECTITEM => select_item(window, wparam, lparam),
        TVM_ENSUREVISIBLE => ensure_visible(window, lparam),
        TVM_GETITEMRECT => item_rect(window, wparam != 0, lparam),
        TVM_HITTEST => hit_test(window, lparam),
        TVM_GETCOUNT => {
            with_state::<TreeState, _>(window, |_, control| control.tree.count()).unwrap_or(0)
        }
        TVM_GETVISIBLECOUNT => with_state::<TreeState, _>(window, |entry, control| {
            layout_of(entry, &control.tree).visible
        })
        .unwrap_or(0),
        TVM_GETINDENT => {
            with_state::<TreeState, _>(window, |_, control| control.tree.indent() as usize)
                .unwrap_or(0)
        }
        TVM_SETINDENT => {
            with_state::<TreeState, _>(window, |_, control| control.tree.set_indent(wparam as i32));
            refresh(window, false);
            0
        }
        TVM_GETITEMHEIGHT | TVM_SETITEMHEIGHT => ITEM_HEIGHT as usize,
        TVM_GETITEMSTATE => with_state::<TreeState, _>(window, |_, control| {
            let tree = &control.tree;
            tree.index_of(wparam)
                .map_or(0, |index| (tree.state(index) & lparam as u32) as usize)
        })
        .unwrap_or(0),
        TVM_SETIMAGELIST => image_list_message(window, true, wparam, lparam),
        TVM_GETIMAGELIST => image_list_message(window, false, wparam, 0),
        TVM_SETBKCOLOR | TVM_GETBKCOLOR | TVM_SETTEXTCOLOR | TVM_GETTEXTCOLOR
        | TVM_SETLINECOLOR | TVM_GETLINECOLOR => colour_message(window, number, lparam),
        TVM_SORTCHILDREN => sort_children(window, lparam, wparam != 0, None),
        TVM_SORTCHILDRENCB => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: `TVM_SORTCHILDRENCB`'s `lParam` is the program's `TVSORTCB`.
            let request = unsafe { (lparam as *const SortRequest).read_unaligned() };
            if request.compare == 0 {
                return 0;
            }
            let callback = Some((request.compare, request.lparam));
            sort_children(window, request.parent, wparam != 0, callback)
        }
        TVM_SETEXTENDEDSTYLE => {
            let mask = match wparam as u32 {
                0 => u32::MAX,
                mask => mask,
            };
            with_state::<TreeState, _>(window, |_, control| {
                control.extended = (control.extended & !mask) | (lparam as u32 & mask);
            });
            0
        }
        TVM_GETEXTENDEDSTYLE => {
            with_state::<TreeState, _>(window, |_, control| control.extended as usize).unwrap_or(0)
        }
        TVM_GETEDITCONTROL => 0,
        _ => 0,
    }
}

fn act_here(window: usize, item: usize, action: Action) {
    let exists =
        with_state::<TreeState, _>(window, |_, control| control.tree.exists(item)).unwrap_or(false);
    if !exists {
        return;
    }
    match action {
        Action::Select => {
            select(window, Some(item), TVC_UNKNOWN);
        }
        Action::Focus => {
            set_focus(window);
            select(window, Some(item), TVC_UNKNOWN);
        }
        Action::Invoke => {
            set_focus(window);
            if notify_parent(window, code::NM_DBLCLK, |header| header) == 0 {
                expand(window, item, TVE_TOGGLE, true);
            }
        }
        Action::SetText => {}
    }
}

/// Does an agent's `action` on item `item` (section 29): `select` selects it, `invoke` is a
/// double click on its label, `focus` focuses the tree and selects it.
pub(super) fn act(window: usize, item: Option<usize>, action: Action, ours: bool) -> Outcome {
    let Some(item) = item else {
        return Outcome::NotSupported;
    };
    if !matches!(action, Action::Select | Action::Invoke | Action::Focus) {
        return Outcome::NotSupported;
    }
    let exists =
        with_state::<TreeState, _>(window, |_, control| control.tree.exists(item)).unwrap_or(false);
    if !exists {
        return Outcome::NoSuchElement;
    }
    match ours {
        true => {
            act_here(window, item, action);
            Outcome::Done
        }
        false => list_view::hand_over(window, ACT_ON_ITEM, item, action),
    }
}

fn next_in_order(tree: &TreeView, index: usize) -> Option<usize> {
    if let Some(child) = tree.first_child(index) {
        return Some(child);
    }
    let mut at = index;
    loop {
        if let Some(next) = tree.next_sibling(at) {
            return Some(next);
        }
        at = tree.parent(at)?;
    }
}

/// Pushes tree `window`'s items (section 29) under its element `element`, each `ListItem`
/// under its parent item's, collapsed ones' children included at height 0.
pub(super) fn push_items(
    gui: &Gui,
    window: usize,
    element: u32,
    origin: (i32, i32),
    tree: &mut TreeWriter<'_>,
) -> bool {
    let focused = has_focus(gui, window);
    read_state::<TreeState, bool>(gui, window, |entry, control| {
        let items = &control.tree;
        let layout = layout_of(entry, items);
        let checks = entry.style & style::CHECK_BOXES != 0;
        let mut at = items.first_root();
        while let Some(index) = at {
            let rect = items.item_bounds(&layout, index);
            let bounds = Bounds::new(
                origin.0 + rect.left,
                origin.1 + rect.top,
                rect.width(),
                rect.height(),
            );
            let parent = items
                .parent(index)
                .map_or(element, |parent| item_element(window, parent));
            let mut name = [0u8; MAX_LABEL_BYTES];
            let name = utf8(items.text(index), &mut name);
            let item_state = items.state(index);
            let mut bits = 0;
            if item_state & state::SELECTED != 0 {
                bits |= flags::SELECTED;
                if focused {
                    bits |= flags::FOCUSED;
                }
            }
            if item_state & state::EXPANDED != 0 {
                bits |= flags::EXPANDED;
            }
            if checks && state_image::image_of(item_state) == state_image::CHECKED {
                bits |= flags::CHECKED;
            }
            let row = Element::new(item_element(window, index), parent, Role::ListItem, bounds)
                .named(name)
                .flagged(bits);
            if !tree.push(&row) {
                return false;
            }
            at = next_in_order(items, index);
        }
        true
    })
    .unwrap_or(true)
}

/// The window procedure of every tree view.
pub(super) fn procedure(window: usize, number: u32, wparam: usize, lparam: usize) -> usize {
    match number {
        message::NC_CREATE => 1,
        message::CREATE => 0,
        message::PAINT => {
            paint(window);
            0
        }
        message::SIZE | message::ENABLE => {
            refresh(window, false);
            0
        }
        message::SET_FOCUS | message::KILL_FOCUS => {
            focus_changed(window, number == message::SET_FOCUS);
            0
        }
        message::LEFT_BUTTON_DOWN | message::LEFT_BUTTON_DOUBLE_CLICK => {
            let double = number == message::LEFT_BUTTON_DOUBLE_CLICK;
            button_down(window, point(lparam), double);
            0
        }
        message::LEFT_BUTTON_UP => {
            button_up(window);
            0
        }
        message::MOUSE_MOVE => {
            pointer_moved(window, point(lparam));
            0
        }
        message::KEY_DOWN => {
            key_down(window, wparam as u32);
            0
        }
        message::CHAR => {
            character(window, wparam as u32);
            0
        }
        WM_VSCROLL | WM_HSCROLL => {
            scroll_message(window, number == WM_VSCROLL, wparam);
            0
        }
        ACT_ON_ITEM => {
            if let Some(action) = Action::from_value(lparam as u32) {
                act_here(window, wparam, action);
            }
            0
        }
        CCM_GETUNICODEFORMAT => 1,
        number if (TVM_FIRST..=TVM_LAST).contains(&number) => {
            tree_message(window, number, wparam, lparam)
        }
        _ => default_procedure(window, number, wparam, lparam),
    }
}

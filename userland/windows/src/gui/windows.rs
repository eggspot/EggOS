//! The program's windows (`HWND`s): what each was created with, its server window and pixel
//! buffer (a top-level window's) or its rectangle in its parent (a child window's, D145, which
//! draws on its top-level window's buffer), the region waiting to be painted, and the values
//! `GetWindowLongPtrW` and `SetWindowLongPtrW` read and write.

use super::slots::Slots;
use eggos_windows_gui::surface::Rect;

/// The most windows a program has at once, child windows included (the window server's limit
/// of 16 per client bounds the top-level ones).
pub const MAX_WINDOWS: usize = 64;
/// The deepest a tree of child windows goes.
pub const MAX_DEPTH: usize = 8;
/// The most extra bytes a window's class may ask for (`cbWndExtra`).
pub const MAX_EXTRA: usize = 256;
const HANDLE_BASE: usize = 0x1_0000;

/// `GetWindowLongPtrW`'s indices below 0.
pub mod index {
    pub const PROCEDURE: i32 = -4;
    pub const INSTANCE: i32 = -6;
    pub const PARENT: i32 = -8;
    pub const ID: i32 = -12;
    pub const STYLE: i32 = -16;
    pub const EX_STYLE: i32 = -20;
    pub const USER_DATA: i32 = -21;
}

/// The system's control classes (D145).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Button,
    Edit,
    Static,
    ListBox,
}

/// Who answers a window's messages: the program's window procedure, the layer's message box
/// (its state's address), one of the system's controls (its state's address, 0 for a static)
/// or the layer's file picker (its state's address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Procedure {
    Program(usize),
    MessageBox(usize),
    Control(ControlKind, usize),
    FilePicker(usize),
}

impl Procedure {
    /// Whether it paints through `WM_PAINT` (the message box paints itself).
    pub fn paints_on_request(&self) -> bool {
        !matches!(self, Procedure::MessageBox(_))
    }
}

/// One window.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub class_atom: u16,
    pub procedure: Procedure,
    pub style: u32,
    pub ex_style: u32,
    pub instance: usize,
    pub id: usize,
    pub owner: usize,
    pub user_data: usize,
    pub extra: [u8; MAX_EXTRA],
    pub extra_length: usize,
    pub background: usize,
    pub class_style: u32,
    pub server: u32,
    pub pixels: usize,
    pub buffer_width: u32,
    pub buffer_height: u32,
    pub width: u32,
    pub height: u32,
    pub parent: usize,
    pub bounds: Rect,
    pub menu: usize,
    pub menu_height: i32,
    pub focus: usize,
    pub capture: usize,
    pub enabled: bool,
    pub modal_disabled: bool,
    pub text: usize,
    pub creation_sequence: u32,
    pub pending: Rect,
    pub paint_sent: bool,
    pub shown: bool,
    pub committed: bool,
    pub size_kind: usize,
    pub update: Rect,
    pub erase: bool,
    pub destroying: bool,
    pub thread: u32,
}

/// Where a window draws: its top-level window, the point of that window's buffer its client
/// area's (0, 0) is at, and the part of the buffer it may draw on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub root: usize,
    pub origin: (i32, i32),
    pub clip: Rect,
}

/// A window's pixel buffer: the address it is mapped at and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixels {
    pub address: usize,
    pub width: u32,
    pub height: u32,
}

impl Window {
    /// A top-level window of a class's procedure and background, on server window `server`
    /// with a `width` by `height` buffer at `pixels`, all of it client area.
    pub fn new(procedure: Procedure, server: u32, pixels: usize, width: u32, height: u32) -> Self {
        Window {
            class_atom: 0,
            procedure,
            style: 0,
            ex_style: 0,
            instance: 0,
            id: 0,
            owner: 0,
            user_data: 0,
            extra: [0; MAX_EXTRA],
            extra_length: 0,
            background: 0,
            class_style: 0,
            server,
            pixels,
            buffer_width: width,
            buffer_height: height,
            width,
            height,
            parent: 0,
            bounds: Rect::default(),
            menu: 0,
            menu_height: 0,
            focus: 0,
            capture: 0,
            enabled: true,
            modal_disabled: false,
            text: 0,
            creation_sequence: 0,
            pending: Rect::default(),
            paint_sent: false,
            shown: false,
            committed: false,
            size_kind: 0,
            update: Rect::default(),
            erase: false,
            destroying: false,
            thread: 0,
        }
    }

    /// A child window of `parent` at `bounds` in its parent's client area.
    pub fn child(procedure: Procedure, parent: usize, bounds: Rect) -> Self {
        let bounds = bounds.normalised();
        let mut window = Window::new(
            procedure,
            0,
            0,
            bounds.width().max(0) as u32,
            bounds.height().max(0) as u32,
        );
        window.parent = parent;
        window.bounds = bounds;
        window.buffer_width = 0;
        window.buffer_height = 0;
        window
    }

    pub fn is_child(&self) -> bool {
        self.parent != 0
    }

    /// The buffer of a top-level window.
    pub fn pixels(&self) -> Pixels {
        Pixels {
            address: self.pixels,
            width: self.buffer_width,
            height: self.buffer_height,
        }
    }

    /// Sets the menu bar's height and the client area under it.
    pub fn set_menu_height(&mut self, height: i32) {
        self.menu_height = height.clamp(0, self.buffer_height as i32);
        self.width = self.buffer_width;
        self.height = self.buffer_height - self.menu_height as u32;
    }

    /// The client area, `(0, 0)` to its size.
    pub fn client(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Adds `rect` (the whole client area when `None`) to the region waiting to be painted.
    pub fn invalidate(&mut self, rect: Option<Rect>, erase: bool) {
        let client = self.client();
        let rect = rect.map_or(client, |rect| rect.normalised().intersection(&client));
        if rect.is_empty() {
            return;
        }
        self.update = self.update.union(&rect);
        self.erase |= erase;
    }

    /// Whether a `WM_PAINT` waits for it, its ancestors aside: it paints on request, is
    /// shown, not being destroyed, and has a region to paint (the layer paints its message
    /// boxes itself).
    pub fn needs_paint(&self) -> bool {
        self.procedure.paints_on_request()
            && self.shown
            && !self.update.is_empty()
            && !self.destroying
    }

    /// The value at `index`: one of [`index`]'s, or `size` bytes of the extra bytes at that
    /// offset.
    pub fn long(&self, index: i32, size: usize) -> Option<usize> {
        let value = match index {
            index::PROCEDURE => match self.procedure {
                Procedure::Program(address) => address,
                _ => return None,
            },
            index::INSTANCE => self.instance,
            index::PARENT if self.parent != 0 => self.parent,
            index::PARENT => self.owner,
            index::ID => self.id,
            index::STYLE => self.style as usize,
            index::EX_STYLE => self.ex_style as usize,
            index::USER_DATA => self.user_data,
            offset => {
                let range = self.extra_range(offset, size)?;
                let mut bytes = [0u8; 8];
                bytes[..size].copy_from_slice(&self.extra[range]);
                usize::from_le_bytes(bytes)
            }
        };
        Some(match size {
            4 => value as u32 as usize,
            _ => value,
        })
    }

    /// Sets the value at `index` (as [`Self::long`]) and returns the one it had.
    pub fn set_long(&mut self, index: i32, size: usize, value: usize) -> Option<usize> {
        let previous = self.long(index, size)?;
        match index {
            index::PROCEDURE => match self.procedure {
                Procedure::Program(_) => self.procedure = Procedure::Program(value),
                _ => return None,
            },
            index::PARENT if self.parent != 0 => return None,
            index::INSTANCE => self.instance = value,
            index::PARENT => self.owner = value,
            index::ID => self.id = value,
            index::STYLE => self.style = value as u32,
            index::EX_STYLE => self.ex_style = value as u32,
            index::USER_DATA => self.user_data = value,
            offset => {
                let range = self.extra_range(offset, size)?;
                self.extra[range].copy_from_slice(&value.to_le_bytes()[..size]);
            }
        }
        Some(previous)
    }

    fn extra_range(&self, offset: i32, size: usize) -> Option<core::ops::Range<usize>> {
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(size)?;
        (end <= self.extra_length && matches!(size, 4 | 8)).then_some(start..end)
    }
}

/// The program's windows by handle.
pub type WindowTable = Slots<Window, MAX_WINDOWS>;

pub const fn new_table() -> WindowTable {
    Slots::new(HANDLE_BASE)
}

/// The top-level window of server window `server`, if the program has it.
pub fn by_server(table: &WindowTable, server: u32) -> Option<usize> {
    table
        .iter()
        .find(|(_, window)| !window.is_child() && window.server == server)
        .map(|(handle, _)| handle)
}

/// The windows from `handle` up to its top-level window, `handle` first; `None` when one is
/// gone or the chain is deeper than [`MAX_DEPTH`].
pub fn ancestry(table: &WindowTable, handle: usize) -> Option<([usize; MAX_DEPTH], usize)> {
    let mut chain = [0; MAX_DEPTH];
    let mut at = handle;
    for (depth, slot) in chain.iter_mut().enumerate() {
        let window = table.get(at)?;
        *slot = at;
        if !window.is_child() {
            return Some((chain, depth + 1));
        }
        at = window.parent;
    }
    None
}

/// The top-level window `handle` belongs to (itself when it is one).
pub fn root_of(table: &WindowTable, handle: usize) -> Option<usize> {
    let (chain, length) = ancestry(table, handle)?;
    Some(chain[length - 1])
}

/// Whether `handle` and every window above it are enabled.
pub fn enabled_in_tree(table: &WindowTable, handle: usize) -> bool {
    ancestry(table, handle).is_some_and(|(chain, length)| {
        chain[..length]
            .iter()
            .all(|&at| table.get(at).is_some_and(|window| window.enabled))
    })
}

/// Whether `handle` is `ancestor` or one of its descendants.
pub fn is_in_tree(table: &WindowTable, ancestor: usize, handle: usize) -> bool {
    ancestry(table, handle).is_some_and(|(chain, length)| chain[..length].contains(&ancestor))
}

/// Where `handle` draws: its top-level window's buffer, from its client area's origin, clipped
/// to its client area and its ancestors'; nothing for a window hidden or under a hidden one.
pub fn client_placement(table: &WindowTable, handle: usize) -> Option<Placement> {
    let (chain, length) = ancestry(table, handle)?;
    let root = chain[length - 1];
    let top = table.get(root)?;
    let mut origin: (i32, i32) = (0, top.menu_height);
    let mut clip = Rect::new(
        0,
        top.menu_height,
        top.buffer_width as i32,
        top.buffer_height as i32,
    );
    for &at in chain[..length - 1].iter().rev() {
        let window = table.get(at)?;
        origin = (
            origin.0.saturating_add(window.bounds.left),
            origin.1.saturating_add(window.bounds.top),
        );
        let own = Rect::sized(
            origin.0,
            origin.1,
            window.width as i32,
            window.height as i32,
        );
        clip = match window.shown {
            true => clip.intersection(&own),
            false => Rect::default(),
        };
    }
    Some(Placement { root, origin, clip })
}

/// The handles of `parent`'s children, in creation order.
pub fn children(table: &WindowTable, parent: usize) -> ([usize; MAX_WINDOWS], usize) {
    let mut found = [(0u32, 0usize); MAX_WINDOWS];
    let mut count = 0;
    for (handle, window) in table.iter() {
        if window.parent == parent && parent != 0 {
            found[count] = (window.creation_sequence, handle);
            count += 1;
        }
    }
    found[..count].sort_unstable();
    let mut handles = [0; MAX_WINDOWS];
    for (slot, (_, handle)) in handles.iter_mut().zip(&found[..count]) {
        *slot = *handle;
    }
    (handles, count)
}

/// The handles of `handle`'s tree, `handle` first and every window before its children.
pub fn tree(table: &WindowTable, handle: usize) -> ([usize; MAX_WINDOWS], usize) {
    let mut found = [0; MAX_WINDOWS];
    if table.get(handle).is_none() {
        return (found, 0);
    }
    found[0] = handle;
    let (mut count, mut at) = (1, 0);
    while at < count {
        let (kids, length) = children(table, found[at]);
        for &kid in &kids[..length] {
            if count < MAX_WINDOWS {
                found[count] = kid;
                count += 1;
            }
        }
        at += 1;
    }
    (found, count)
}

/// The deepest shown child of `parent` at (`x`, `y`) of its client area, the last created on
/// top, with the point in that child's client coordinates; `parent` itself when no child is
/// there.
pub fn child_at(table: &WindowTable, parent: usize, x: i32, y: i32) -> (usize, (i32, i32)) {
    let (kids, length) = children(table, parent);
    for &kid in kids[..length].iter().rev() {
        let Some(window) = table.get(kid) else {
            continue;
        };
        if window.shown && !window.destroying && window.bounds.contains(x, y) {
            return child_at(table, kid, x - window.bounds.left, y - window.bounds.top);
        }
    }
    (parent, (x, y))
}

/// Invalidates `rect` of `handle` (its client area when `None`) and, as Windows does for a
/// window without `WS_CLIPCHILDREN`, the part of each shown child it covers.
pub fn invalidate_tree(table: &mut WindowTable, handle: usize, rect: Option<Rect>, erase: bool) {
    let Some(window) = table.get_mut(handle) else {
        return;
    };
    window.invalidate(rect, erase);
    let area = rect.unwrap_or(window.client());
    let (kids, length) = children(table, handle);
    for &kid in &kids[..length] {
        let Some(bounds) = table
            .get(kid)
            .filter(|window| window.shown)
            .map(|window| window.bounds)
        else {
            continue;
        };
        let covered = area.normalised().intersection(&bounds);
        if !covered.is_empty() {
            let local = covered.offset(-bounds.left, -bounds.top);
            invalidate_tree(table, kid, Some(local), erase);
        }
    }
}

fn ancestors_allow_paint(table: &WindowTable, handle: usize) -> bool {
    let Some((chain, length)) = ancestry(table, handle) else {
        return false;
    };
    chain[1..length].iter().all(|&at| {
        table
            .get(at)
            .is_some_and(|window| window.shown && !window.destroying && !window.needs_paint())
    })
}

/// The first window of `filter`'s tree (any when 0) and of `thread` (any when 0) with a region
/// to paint, a parent always before its children.
pub fn needing_paint(table: &WindowTable, filter: usize, thread: u32) -> Option<usize> {
    table
        .iter()
        .find(|(handle, window)| {
            (filter == 0 || is_in_tree(table, filter, *handle))
                && (thread == 0 || thread == window.thread)
                && window.needs_paint()
                && ancestors_allow_paint(table, *handle)
        })
        .map(|(handle, _)| handle)
}

/// Whether a paint of `thread`'s that was not handed out since the last commit still waits in
/// `root`'s tree, so a commit of what is drawn would show a half-painted window.
pub fn paint_outstanding(table: &WindowTable, root: usize, thread: u32) -> bool {
    let (handles, count) = tree(table, root);
    handles[..count].iter().any(|&handle| {
        table.get(handle).is_some_and(|window| {
            window.thread == thread
                && !window.paint_sent
                && window.needs_paint()
                && ancestors_allow_paint(table, handle)
        })
    })
}

/// Clears the marks of paints handed out in `root`'s tree, after a commit.
pub fn forget_sent_paints(table: &mut WindowTable, root: usize) {
    let (handles, count) = tree(table, root);
    for &handle in &handles[..count] {
        if let Some(window) = table.get_mut(handle) {
            window.paint_sent = false;
        }
    }
}

/// The next window of `root`'s tree after `from` (before it when `backwards`) that takes the
/// focus by Tab: a shown, enabled child with `WS_TABSTOP`, wrapping, in creation order.
pub fn next_tab_stop(
    table: &WindowTable,
    root: usize,
    from: usize,
    backwards: bool,
) -> Option<usize> {
    const WS_TABSTOP: u32 = 0x0001_0000;
    let (handles, count) = tree(table, root);
    let stops = |handle: &&usize| {
        table.get(**handle).is_some_and(|window| {
            window.is_child() && window.shown && window.enabled && window.style & WS_TABSTOP != 0
        })
    };
    let order = &handles[1..count];
    let at = order.iter().position(|&handle| handle == from);
    let after = |index: usize| (index + 1) % order.len().max(1);
    let before = |index: usize| (index + order.len() - 1) % order.len().max(1);
    let mut index = match (at, backwards) {
        (Some(at), false) => after(at),
        (Some(at), true) => before(at),
        (None, false) => 0,
        (None, true) => order.len().checked_sub(1)?,
    };
    for _ in 0..order.len() {
        if stops(&&order[index]) {
            return Some(order[index]);
        }
        index = match backwards {
            false => after(index),
            true => before(index),
        };
    }
    None
}

/// Gives `handle` the new buffer `new`, all of its tree to paint, and returns the buffer
/// nothing draws on any more, for the caller to unmap after the table's lock: the window's old
/// one, or `new` itself when the window is gone. The table never holds an unmapped buffer.
pub fn replace_pixels(table: &mut WindowTable, handle: usize, new: Pixels) -> Pixels {
    let Some(window) = table.get_mut(handle) else {
        return new;
    };
    let old = window.pixels();
    window.pixels = new.address;
    window.buffer_width = new.width;
    window.buffer_height = new.height;
    let menu_height = window.menu_height;
    window.set_menu_height(menu_height);
    window.update = Rect::default();
    window.pending = Rect::new(0, 0, new.width as i32, new.height as i32);
    invalidate_tree(table, handle, None, true);
    old
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        let mut window = Window::new(Procedure::Program(0x4000), 7, 0, 400, 300);
        window.extra_length = 16;
        window.shown = true;
        window
    }

    #[test]
    fn invalidation_unites_rectangles_inside_the_client_area() {
        let mut window = window();
        assert!(!window.needs_paint());
        window.invalidate(Some(Rect::new(10, 10, 20, 20)), false);
        window.invalidate(Some(Rect::new(390, 290, 500, 500)), true);
        assert_eq!(window.update, Rect::new(10, 10, 400, 300));
        assert!(window.erase && window.needs_paint());
        window.update = Rect::default();
        window.invalidate(Some(Rect::new(500, 500, 600, 600)), false);
        assert!(!window.needs_paint());
        window.invalidate(None, false);
        assert_eq!(window.update, window.client());
    }

    #[test]
    fn window_longs_read_and_write_fields_and_extra_bytes() {
        let mut window = window();
        assert_eq!(
            window.set_long(index::USER_DATA, 8, 0xdead_beef_0000),
            Some(0)
        );
        assert_eq!(window.long(index::USER_DATA, 8), Some(0xdead_beef_0000));
        assert_eq!(window.long(index::USER_DATA, 4), Some(0xbeef_0000));
        assert_eq!(window.set_long(index::PROCEDURE, 8, 0x5000), Some(0x4000));
        assert_eq!(window.procedure, Procedure::Program(0x5000));
        assert_eq!(window.set_long(8, 8, usize::MAX), Some(0));
        assert_eq!(window.long(8, 4), Some(0xffff_ffff));
        assert_eq!(window.long(12, 4), Some(0xffff_ffff));
        assert_eq!(window.long(0, 8), Some(0));
        assert_eq!(window.long(12, 8), None);
        assert_eq!(window.long(16, 4), None);
        assert_eq!(window.long(-100, 8), None);
        window.procedure = Procedure::MessageBox(1);
        assert_eq!(window.long(index::PROCEDURE, 8), None);
    }

    #[test]
    fn a_new_buffer_replaces_the_old_one_which_is_given_back_to_unmap() {
        let mut table = new_table();
        let handle = table.insert(window()).unwrap();
        let new = Pixels {
            address: 0x9000,
            width: 500,
            height: 360,
        };
        let old = replace_pixels(&mut table, handle, new);
        assert_eq!(
            old,
            Pixels {
                address: 0,
                width: 400,
                height: 300
            }
        );
        let window = table.get(handle).unwrap();
        assert_eq!(window.pixels(), new);
        assert_eq!((window.width, window.height), (500, 360));
        assert_eq!(window.update, Rect::new(0, 0, 500, 360));
        assert!(window.erase);
        table.remove(handle);
        let newer = Pixels {
            address: 0xa000,
            ..new
        };
        assert_eq!(replace_pixels(&mut table, handle, newer), newer);
    }

    #[test]
    fn paint_waits_only_for_the_windows_thread() {
        let mut table = new_table();
        let mut other = window();
        other.thread = 7;
        other.invalidate(None, false);
        let other = table.insert(other).unwrap();
        assert_eq!(needing_paint(&table, 0, 8), None);
        assert_eq!(needing_paint(&table, 0, 7), Some(other));
        assert_eq!(needing_paint(&table, 0, 0), Some(other));
    }

    #[test]
    fn windows_are_found_by_server_id_and_by_paint() {
        let mut table = new_table();
        let first = table.insert(window()).unwrap();
        let mut second = window();
        second.server = 9;
        let second = table.insert(second).unwrap();
        assert_eq!(by_server(&table, 9), Some(second));
        assert_eq!(by_server(&table, 8), None);
        assert_eq!(needing_paint(&table, 0, 0), None);
        table.get_mut(second).unwrap().invalidate(None, true);
        assert_eq!(needing_paint(&table, 0, 0), Some(second));
        assert_eq!(needing_paint(&table, first, 0), None);
        table.get_mut(second).unwrap().destroying = true;
        assert_eq!(needing_paint(&table, 0, 0), None);
        let mut hidden = window();
        hidden.shown = false;
        hidden.invalidate(None, false);
        let hidden = table.insert(hidden).unwrap();
        assert_eq!(needing_paint(&table, hidden, 0), None);
        assert_eq!(needing_paint(&table, usize::MAX, 0), None);
        let mut message_box = window();
        message_box.procedure = Procedure::MessageBox(0x1000);
        message_box.invalidate(None, true);
        let message_box = table.insert(message_box).unwrap();
        assert_eq!(needing_paint(&table, message_box, 0), None);
    }

    fn child(parent: usize, bounds: Rect) -> Window {
        let mut window = Window::child(Procedure::Program(0x4000), parent, bounds);
        window.shown = true;
        window
    }

    #[test]
    fn children_draw_inside_their_parents_below_the_menu_bar() {
        let mut table = new_table();
        let mut top = window();
        top.buffer_height = 324;
        top.set_menu_height(24);
        assert_eq!((top.width, top.height), (400, 300));
        let root = table.insert(top).unwrap();
        let group = table
            .insert(child(root, Rect::sized(10, 20, 100, 50)))
            .unwrap();
        let button = table
            .insert(child(group, Rect::sized(90, 5, 40, 20)))
            .unwrap();
        assert_eq!(
            client_placement(&table, root),
            Some(Placement {
                root,
                origin: (0, 24),
                clip: Rect::new(0, 24, 400, 324)
            })
        );
        assert_eq!(
            client_placement(&table, button),
            Some(Placement {
                root,
                origin: (100, 49),
                clip: Rect::new(100, 49, 110, 69)
            })
        );
        assert_eq!(root_of(&table, button), Some(root));
        assert!(is_in_tree(&table, root, button) && !is_in_tree(&table, button, group));
        assert_eq!(child_at(&table, root, 105, 30), (button, (5, 5)));
        assert_eq!(child_at(&table, root, 15, 30), (group, (5, 10)));
        assert_eq!(child_at(&table, root, 300, 200), (root, (300, 200)));
        table.get_mut(group).unwrap().shown = false;
        assert!(client_placement(&table, button).unwrap().clip.is_empty());
        assert_eq!(child_at(&table, root, 105, 30), (root, (105, 30)));
        let (handles, count) = tree(&table, root);
        assert_eq!(&handles[..count], &[root, group, button]);
    }

    #[test]
    fn invalidating_a_parent_reaches_the_children_it_covers_and_parents_paint_first() {
        let mut table = new_table();
        let root = table.insert(window()).unwrap();
        let left = table
            .insert(child(root, Rect::sized(10, 10, 50, 20)))
            .unwrap();
        let right = table
            .insert(child(root, Rect::sized(300, 10, 50, 20)))
            .unwrap();
        invalidate_tree(&mut table, root, Some(Rect::new(0, 0, 100, 100)), true);
        assert_eq!(table.get(left).unwrap().update, Rect::new(0, 0, 50, 20));
        assert!(table.get(right).unwrap().update.is_empty());
        assert_eq!(needing_paint(&table, 0, 0), Some(root));
        table.get_mut(root).unwrap().update = Rect::default();
        assert_eq!(needing_paint(&table, 0, 0), Some(left));
        table.get_mut(root).unwrap().shown = false;
        assert_eq!(needing_paint(&table, 0, 0), None);
    }

    #[test]
    fn a_commit_waits_only_for_paints_not_yet_handed_out_on_the_same_thread() {
        let mut table = new_table();
        let root = table.insert(window()).unwrap();
        let kid = table
            .insert(child(root, Rect::sized(10, 10, 50, 20)))
            .unwrap();
        table.get_mut(kid).unwrap().invalidate(None, true);
        assert!(paint_outstanding(&table, root, 0));
        table.get_mut(kid).unwrap().paint_sent = true;
        assert!(!paint_outstanding(&table, root, 0));
        forget_sent_paints(&mut table, root);
        assert!(paint_outstanding(&table, root, 0));
        assert!(!paint_outstanding(&table, root, 5));
    }

    #[test]
    fn tab_stops_wrap_in_creation_order_and_skip_disabled_children() {
        let mut table = new_table();
        let root = table.insert(window()).unwrap();
        let mut stop = |enabled: bool| {
            let mut window = child(root, Rect::sized(0, 0, 10, 10));
            window.style = 0x0001_0000;
            window.enabled = enabled;
            table.insert(window).unwrap()
        };
        let first = stop(true);
        let skipped = stop(false);
        let last = stop(true);
        let _ = skipped;
        assert_eq!(next_tab_stop(&table, root, first, false), Some(last));
        assert_eq!(next_tab_stop(&table, root, last, false), Some(first));
        assert_eq!(next_tab_stop(&table, root, first, true), Some(last));
        assert_eq!(next_tab_stop(&table, root, root, false), Some(first));
    }

    #[test]
    fn a_controls_procedure_and_a_childs_parent_cannot_be_replaced() {
        let mut window = Window::child(
            Procedure::Control(ControlKind::Edit, 0x100),
            0x1_0040,
            Rect::sized(0, 0, 9, 9),
        );
        assert_eq!(window.long(index::PROCEDURE, 8), None);
        assert_eq!(window.set_long(index::PROCEDURE, 8, 5), None);
        assert_eq!(window.long(index::PARENT, 8), Some(0x1_0040));
        assert_eq!(window.set_long(index::PARENT, 8, 5), None);
    }

    #[test]
    fn children_keep_creation_order_when_slots_are_reused() {
        let mut table = new_table();
        let root = table.insert(window()).unwrap();
        let mut make = |creation_sequence: u32| {
            let mut kid = child(root, Rect::sized(0, 0, 10, 10));
            kid.creation_sequence = creation_sequence;
            kid
        };
        let first = table.insert(make(1)).unwrap();
        let second = table.insert(make(2)).unwrap();
        table.remove(first);
        let third = table.insert(make(3)).unwrap();
        let (kids, count) = children(&table, root);
        assert_eq!(&kids[..count], &[second, third]);
        assert!(enabled_in_tree(&table, third));
        table.get_mut(root).unwrap().enabled = false;
        assert!(!enabled_in_tree(&table, third));
        table.get_mut(root).unwrap().enabled = true;
        assert_eq!(child_at(&table, root, 5, 5).0, third);
        assert_eq!(needing_paint(&table, root, 0), None);
        table.get_mut(third).unwrap().invalidate(None, true);
        assert_eq!(needing_paint(&table, root, 0), Some(third));
        assert_eq!(needing_paint(&table, second, 0), None);
    }
}

//! The program's windows (`HWND`s): what each was created with, its server window and pixel
//! buffer, the region waiting to be painted, and the values `GetWindowLongPtrW` and
//! `SetWindowLongPtrW` read and write.

use super::slots::Slots;
use eggos_windows_gui::surface::Rect;

/// The most windows a program has at once: the window server's limit per client.
pub const MAX_WINDOWS: usize = 16;
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

/// Who answers a window's messages: the program's window procedure, or the layer's message
/// box (its state's address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Procedure {
    Program(usize),
    MessageBox(usize),
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
    pub server: u32,
    pub pixels: usize,
    pub width: u32,
    pub height: u32,
    pub shown: bool,
    pub committed: bool,
    pub size_kind: usize,
    pub update: Rect,
    pub erase: bool,
    pub destroying: bool,
}

impl Window {
    /// A window of a class's procedure and background, on server window `server` with a
    /// `width` by `height` buffer at `pixels`.
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
            server,
            pixels,
            width,
            height,
            shown: false,
            committed: false,
            size_kind: 0,
            update: Rect::default(),
            erase: false,
            destroying: false,
        }
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

    /// Whether a `WM_PAINT` waits for it: it is shown, not being destroyed, and has a region
    /// to paint.
    pub fn needs_paint(&self) -> bool {
        self.shown && !self.update.is_empty() && !self.destroying
    }

    /// The value at `index`: one of [`index`]'s, or `size` bytes of the extra bytes at that
    /// offset.
    pub fn long(&self, index: i32, size: usize) -> Option<usize> {
        let value = match index {
            index::PROCEDURE => match self.procedure {
                Procedure::Program(address) => address,
                Procedure::MessageBox(_) => return None,
            },
            index::INSTANCE => self.instance,
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
            index::PROCEDURE => self.procedure = Procedure::Program(value),
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

/// The window of server window `server`, if the program has it.
pub fn by_server(table: &WindowTable, server: u32) -> Option<usize> {
    table
        .iter()
        .find(|(_, window)| window.server == server)
        .map(|(handle, _)| handle)
}

/// The first window of `filter` (any when 0) with a region to paint.
pub fn needing_paint(table: &WindowTable, filter: usize) -> Option<usize> {
    table
        .iter()
        .find(|(handle, window)| (filter == 0 || filter == *handle) && window.needs_paint())
        .map(|(handle, _)| handle)
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
    fn windows_are_found_by_server_id_and_by_paint() {
        let mut table = new_table();
        let first = table.insert(window()).unwrap();
        let mut second = window();
        second.server = 9;
        let second = table.insert(second).unwrap();
        assert_eq!(by_server(&table, 9), Some(second));
        assert_eq!(by_server(&table, 8), None);
        assert_eq!(needing_paint(&table, 0), None);
        table.get_mut(second).unwrap().invalidate(None, true);
        assert_eq!(needing_paint(&table, 0), Some(second));
        assert_eq!(needing_paint(&table, first), None);
        table.get_mut(second).unwrap().destroying = true;
        assert_eq!(needing_paint(&table, 0), None);
        let mut hidden = window();
        hidden.shown = false;
        hidden.invalidate(None, false);
        let hidden = table.insert(hidden).unwrap();
        assert_eq!(needing_paint(&table, hidden), None);
        assert_eq!(needing_paint(&table, usize::MAX), None);
    }
}

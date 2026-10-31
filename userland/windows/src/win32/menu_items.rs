//! Menu items as `MENUITEMINFOW` describes them (D178, `docs/ux/windows-controls-v0.3.md`
//! section 12): `InsertMenuItemW`, `SetMenuItemInfoW`, `GetMenuItemInfoW`, `RemoveMenu`,
//! `GetMenuStringW`, `SetMenuDefaultItem`, `SetMenuInfo` and `GetMenuInfo` over the layer's
//! menu table (a menu's `MNS_` style is kept, not acted on), and
//! `GetSystemMenu`'s window menu. Bitmaps, check-mark bitmaps, a menu's background and its
//! largest height are kept by no item and drawn by no menu (design section 12), and nothing on
//! EggOS opens a window menu: the window server draws the window's frame.

use super::gui::try_with;
use super::menu::{destroy, table, table_mut};
use super::{Bool, FALSE, TRUE, error, fail};
use crate::gui::menus::{self, DEEPEST_SEARCH, Menu, MenuItem, MenuTable, flag};
use crate::library::Export;

const MF_BYPOSITION: u32 = 0x400;
const MIIM_STATE: u32 = 0x1;
const MIIM_ID: u32 = 0x2;
const MIIM_SUBMENU: u32 = 0x4;
const MIIM_CHECKMARKS: u32 = 0x8;
const MIIM_TYPE: u32 = 0x10;
const MIIM_DATA: u32 = 0x20;
const MIIM_STRING: u32 = 0x40;
const MIIM_BITMAP: u32 = 0x80;
const MIIM_FTYPE: u32 = 0x100;
const MFT_BITMAP: u32 = 0x4;
const TYPE_BITS: u32 =
    MFT_BITMAP | 0x20 | 0x40 | flag::OWNER_DRAW | 0x200 | flag::SEPARATOR | 0x6000;
const STATE_BITS: u32 = flag::GRAYED | flag::DISABLED | flag::CHECKED | 0x80 | flag::DEFAULT;
const NOT_TEXT: u32 = MFT_BITMAP | flag::OWNER_DRAW | flag::SEPARATOR;
const MIM_MAXHEIGHT: u32 = 0x1;
const MIM_BACKGROUND: u32 = 0x2;
const MIM_HELPID: u32 = 0x4;
const MIM_STYLE: u32 = 0x10;
const MIM_MENUDATA: u32 = 0x8;
const MIM_APPLYTOSUBMENUS: u32 = 0x8000_0000;
const NO_DEFAULT: u32 = u32::MAX;
const SC_CLOSE: usize = 0xf060;
const CLOSE_LABEL: &str = "&Close\tAlt+F4";

#[repr(C)]
struct MenuItemInfo {
    size: u32,
    mask: u32,
    kind: u32,
    state: u32,
    id: u32,
    submenu: usize,
    checked_bitmap: usize,
    unchecked_bitmap: usize,
    data: usize,
    text: *mut u16,
    length: u32,
    bitmap: usize,
}

const SMALLEST_INFO: usize = core::mem::offset_of!(MenuItemInfo, bitmap);

#[repr(C)]
struct MenuInfo {
    size: u32,
    mask: u32,
    style: u32,
    largest_height: u32,
    background: usize,
    help_id: u32,
    data: usize,
}

fn info_of<'a>(info: *mut MenuItemInfo) -> Result<&'a mut MenuItemInfo, u32> {
    if info.is_null() {
        return Err(error::INVALID_PARAMETER);
    }
    // SAFETY: every caller passes the program's readable and writable `MENUITEMINFOW`; only
    // the fields its size covers are reached (`hbmpItem` only when it is there).
    let info = unsafe { &mut *info };
    match info.size as usize >= SMALLEST_INFO {
        true => Ok(info),
        false => Err(error::INVALID_PARAMETER),
    }
}

fn takes_text(info: &MenuItemInfo) -> bool {
    info.mask & MIIM_STRING != 0 || (info.mask & MIIM_TYPE != 0 && info.kind & NOT_TEXT == 0)
}

fn change(item: &mut MenuItem, info: &MenuItemInfo) {
    if info.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        item.flags = (item.flags & !TYPE_BITS) | (info.kind & TYPE_BITS);
    }
    if info.mask & MIIM_STATE != 0 {
        item.flags = (item.flags & !STATE_BITS) | (info.state & STATE_BITS);
    }
    if info.mask & MIIM_ID != 0 {
        item.id = info.id as usize;
    }
    if info.mask & MIIM_SUBMENU != 0 {
        item.submenu = info.submenu;
        item.flags = match info.submenu {
            0 => item.flags & !flag::POPUP,
            _ => item.flags | flag::POPUP,
        };
    }
    if info.mask & MIIM_DATA != 0 {
        item.data = info.data;
    }
    if takes_text(info) {
        let label = match info.text.is_null() {
            true => &[][..],
            // SAFETY: a text item's `dwTypeData` is the program's NUL-terminated string.
            false => unsafe { super::file::wide_string(info.text) },
        };
        item.set_label(label);
    }
}

fn describe(item: &MenuItem, info: &mut MenuItemInfo) {
    if info.mask & (MIIM_FTYPE | MIIM_TYPE) != 0 {
        info.kind = item.flags & TYPE_BITS;
    }
    if info.mask & MIIM_STATE != 0 {
        info.state = item.flags & STATE_BITS;
    }
    if info.mask & MIIM_ID != 0 {
        info.id = item.id as u32;
    }
    if info.mask & MIIM_SUBMENU != 0 {
        info.submenu = item.popup().unwrap_or(0);
    }
    if info.mask & MIIM_CHECKMARKS != 0 {
        info.checked_bitmap = 0;
        info.unchecked_bitmap = 0;
    }
    if info.mask & MIIM_DATA != 0 {
        info.data = item.data;
    }
    if info.mask & MIIM_BITMAP != 0 && info.size as usize >= core::mem::size_of::<MenuItemInfo>() {
        info.bitmap = 0;
    }
    if takes_text(info) || (info.mask & MIIM_TYPE != 0 && item.flags & NOT_TEXT == 0) {
        let label = item.label();
        if info.text.is_null() || info.length == 0 {
            info.length = label.len() as u32;
        } else {
            // SAFETY: the program's `dwTypeData` holds `cch` writable units.
            let copied = unsafe {
                super::window_text::copy_out(label, info.text as usize, info.length as usize)
            };
            info.length = copied as u32;
        }
    }
}

fn locate(
    table: &MenuTable,
    menu: usize,
    item: u32,
    by_position: bool,
) -> Result<(usize, usize), u32> {
    table.get(menu).ok_or(error::INVALID_MENU_HANDLE)?;
    menus::locate(table, menu, item as usize, by_position).ok_or(error::MENU_ITEM_NOT_FOUND)
}

unsafe extern "efiapi" fn insert_menu_item_w(
    menu: usize,
    item: u32,
    by_position: Bool,
    info: *mut MenuItemInfo,
) -> Bool {
    let inserted = try_with(|gui| {
        let info = info_of(info)?;
        let table = table_mut(gui).ok_or(error::NOT_ENOUGH_MEMORY)?;
        let (owner, index) = match by_position != 0 {
            true => {
                let count = table.get(menu).ok_or(error::INVALID_MENU_HANDLE)?.count;
                (menu, (item as usize).min(count))
            }
            false => locate(table, menu, item, false)?,
        };
        let mut new = MenuItem::new(0, 0, 0, &[]);
        change(&mut new, info);
        match table
            .get_mut(owner)
            .ok_or(error::INVALID_MENU_HANDLE)?
            .insert(index, new)
        {
            true => Ok(()),
            false => Err(error::NOT_ENOUGH_MEMORY),
        }
    });
    match inserted {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn set_menu_item_info_w(
    menu: usize,
    item: u32,
    by_position: Bool,
    info: *mut MenuItemInfo,
) -> Bool {
    let changed = try_with(|gui| {
        let info = info_of(info)?;
        let table = table_mut(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        let (owner, index) = locate(table, menu, item, by_position != 0)?;
        let entry = table
            .get_mut(owner)
            .and_then(|menu| menu.item_mut(index))
            .ok_or(error::MENU_ITEM_NOT_FOUND)?;
        change(entry, info);
        Ok(())
    });
    match changed {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_menu_item_info_w(
    menu: usize,
    item: u32,
    by_position: Bool,
    info: *mut MenuItemInfo,
) -> Bool {
    let found = try_with(|gui| {
        let info = info_of(info)?;
        let table = table(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        let (owner, index) = locate(table, menu, item, by_position != 0)?;
        let entry = table
            .get(owner)
            .and_then(|menu| menu.items().get(index))
            .ok_or(error::MENU_ITEM_NOT_FOUND)?;
        describe(entry, info);
        Ok(())
    });
    match found {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

extern "efiapi" fn remove_menu(menu: usize, item: u32, flags: u32) -> Bool {
    let removed = try_with(|gui| {
        let table = table_mut(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        let (owner, index) = locate(table, menu, item, flags & MF_BYPOSITION != 0)?;
        table
            .get_mut(owner)
            .and_then(|menu| menu.remove(index))
            .map(|_| ())
            .ok_or(error::MENU_ITEM_NOT_FOUND)
    });
    match removed {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_menu_string_w(
    menu: usize,
    item: u32,
    buffer: *mut u16,
    capacity: i32,
    flags: u32,
) -> i32 {
    let length = try_with(|gui| {
        let table = table(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        let (owner, index) = locate(table, menu, item, flags & MF_BYPOSITION != 0)?;
        let entry = table
            .get(owner)
            .and_then(|menu| menu.items().get(index))
            .ok_or(error::MENU_ITEM_NOT_FOUND)?;
        let label = entry.label();
        Ok(match buffer.is_null() || capacity <= 0 {
            true => label.len(),
            // SAFETY: `GetMenuStringW`'s contract: `buffer` holds `capacity` writable units.
            false => unsafe {
                super::window_text::copy_out(label, buffer as usize, capacity as usize)
            },
        })
    });
    match length {
        Ok(length) => length as i32,
        Err(code) => fail(code, 0),
    }
}

extern "efiapi" fn set_menu_default_item(menu: usize, item: u32, by_position: u32) -> Bool {
    let set = try_with(|gui| {
        let table = table_mut(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        let index = match item {
            NO_DEFAULT => None,
            item => {
                let (owner, index) = locate(table, menu, item, by_position != 0)?;
                if owner != menu {
                    return Err(error::MENU_ITEM_NOT_FOUND);
                }
                Some(index)
            }
        };
        let entry = table.get_mut(menu).ok_or(error::INVALID_MENU_HANDLE)?;
        match entry.set_default(index) {
            true => Ok(()),
            false => Err(error::MENU_ITEM_NOT_FOUND),
        }
    });
    match set {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

fn apply_info(table: &mut MenuTable, menu: usize, info: &MenuInfo, depth: usize) {
    let Some(entry) = table.get_mut(menu) else {
        return;
    };
    if info.mask & MIM_STYLE != 0 {
        entry.style = info.style;
    }
    if info.mask & MIM_MENUDATA != 0 {
        entry.data = info.data;
    }
    if info.mask & MIM_APPLYTOSUBMENUS == 0 || depth == 0 {
        return;
    }
    let mut popups = [0; menus::MAX_ITEMS];
    let mut count = 0;
    for popup in entry.items().iter().filter_map(MenuItem::popup) {
        popups[count] = popup;
        count += 1;
    }
    for &popup in &popups[..count] {
        apply_info(table, popup, info, depth - 1);
    }
}

unsafe extern "efiapi" fn set_menu_info(menu: usize, info: *const MenuInfo) -> Bool {
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable `MENUINFO`.
    let info = unsafe { &*info };
    if info.size as usize != core::mem::size_of::<MenuInfo>() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let set = try_with(|gui| {
        let table = table_mut(gui).ok_or(error::INVALID_MENU_HANDLE)?;
        table.get(menu).ok_or(error::INVALID_MENU_HANDLE)?;
        apply_info(table, menu, info, DEEPEST_SEARCH);
        Ok(())
    });
    match set {
        Ok(()) => TRUE,
        Err(code) => fail(code, FALSE),
    }
}

unsafe extern "efiapi" fn get_menu_info(menu: usize, info: *mut MenuInfo) -> Bool {
    if info.is_null() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller passes a readable and writable `MENUINFO`.
    let info = unsafe { &mut *info };
    if info.size as usize != core::mem::size_of::<MenuInfo>() {
        return fail(error::INVALID_PARAMETER, FALSE);
    }
    let found = try_with(|gui| {
        let entry = table(gui)
            .and_then(|table| table.get(menu))
            .ok_or(error::INVALID_MENU_HANDLE)?;
        Ok((entry.style, entry.data))
    });
    let (style, data) = match found {
        Ok(found) => found,
        Err(code) => return fail(code, FALSE),
    };
    if info.mask & MIM_STYLE != 0 {
        info.style = style;
    }
    if info.mask & MIM_MENUDATA != 0 {
        info.data = data;
    }
    if info.mask & MIM_MAXHEIGHT != 0 {
        info.largest_height = 0;
    }
    if info.mask & MIM_BACKGROUND != 0 {
        info.background = 0;
    }
    if info.mask & MIM_HELPID != 0 {
        info.help_id = 0;
    }
    TRUE
}

extern "efiapi" fn get_system_menu(window: usize, revert: Bool) -> usize {
    let mut label = [0u16; CLOSE_LABEL.len()];
    for (slot, byte) in label.iter_mut().zip(CLOSE_LABEL.bytes()) {
        *slot = u16::from(byte);
    }
    let menu = try_with(|gui| {
        let entry = gui.window(window)?;
        if entry.is_child() {
            return Err(error::INVALID_WINDOW_HANDLE);
        }
        let existing = entry.system_menu;
        if revert != 0 {
            if existing != 0 {
                destroy(gui, existing);
                gui.window_mut(window)?.system_menu = 0;
            }
            return Ok(0);
        }
        if existing != 0 {
            return Ok(existing);
        }
        let mut made = Menu::new();
        made.append(0, SC_CLOSE, &label);
        let menu = table_mut(gui)
            .and_then(|table| table.insert(made))
            .ok_or(error::NOT_ENOUGH_MEMORY)?;
        gui.window_mut(window)?.system_menu = menu;
        Ok(menu)
    });
    match menu {
        Ok(menu) => menu,
        Err(code) => fail(code, 0),
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"InsertMenuItemW" => insert_menu_item_w,
    b"SetMenuItemInfoW" => set_menu_item_info_w,
    b"GetMenuItemInfoW" => get_menu_item_info_w,
    b"RemoveMenu" => remove_menu,
    b"GetMenuStringW" => get_menu_string_w,
    b"SetMenuDefaultItem" => set_menu_default_item,
    b"SetMenuInfo" => set_menu_info,
    b"GetMenuInfo" => get_menu_info,
    b"GetSystemMenu" => get_system_menu,
];

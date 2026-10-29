//! NSMenu and NSMenuItem, and the main menu in the top bar (`docs/ux/macos-gui-v0.3.md` section
//! 5): `setMainMenu:` sends the window server each window's `SET_MENU` pool, the first menu
//! titled with the app's name; a `MenuChosen` record and a key press matching an item's key
//! equivalent both send the item's action.

use super::{
    NSMENU, NSMENU_META, NSMENUITEM, NSMENUITEM_META, application, event, server, store,
    store_copy, string, window, with_text,
};
use crate::foundation::{NSARRAY, autoreleased, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send2};
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_macos_gui::keys::modifier;
use eggos_macos_gui::menu::{MAX_MENUS, Pool, answers, key_label};

static MAIN_MENU: AtomicUsize = AtomicUsize::new(0);
static SHOWN_MENUS: AtomicUsize = AtomicUsize::new(0);
static TOP_ITEMS: [AtomicUsize; MAX_MENUS] = [const { AtomicUsize::new(0) }; MAX_MENUS];

#[repr(C)]
struct MenuFields {
    isa: usize,
    title: Id,
    items: Buffer<Id>,
    supermenu: Id,
}

#[repr(C)]
struct ItemFields {
    isa: usize,
    title: Id,
    action: usize,
    target: Id,
    key_equivalent: Id,
    key_mask: u64,
    submenu: Id,
    menu: Id,
    tag: isize,
    disabled: bool,
    separator: bool,
}

fn menu_fields(menu: Id) -> &'static mut MenuFields {
    // SAFETY: the callers pass NSMenu instances.
    unsafe { &mut *menu.cast::<MenuFields>() }
}

fn item_fields(item: Id) -> &'static mut ItemFields {
    // SAFETY: the callers pass NSMenuItem instances.
    unsafe { &mut *item.cast::<ItemFields>() }
}

/// The main menu, nil for none.
pub fn main_menu() -> Id {
    MAIN_MENU.load(Ordering::Acquire) as Id
}

fn first_character(object: Id) -> u32 {
    with_text(object, |text| text.chars().next().map_or(0, u32::from))
}

fn top_title(item: Id, submenu: Id, first: bool, pool: &mut Pool) -> bool {
    if first {
        return pool.menu(super::app_name());
    }
    let title = match menu_fields(submenu).title {
        none if none.is_null() => item_fields(item).title,
        title => title,
    };
    with_text(title, |text| pool.menu(text))
}

fn add_entry(entry: Id, pool: &mut Pool) -> bool {
    let fields = item_fields(entry);
    if fields.separator {
        return pool.separator();
    }
    let enabled = !fields.disabled && fields.action != 0 && fields.submenu.is_null();
    let label = key_label(first_character(fields.key_equivalent), fields.key_mask);
    with_text(fields.title, |title| {
        pool.item(title, label.as_str(), enabled)
    })
}

fn build_pool() -> Pool {
    let mut pool = Pool::new();
    let main = main_menu();
    let mut shown = 0;
    if !main.is_null() {
        let items = menu_fields(main).items.as_slice();
        for (index, &item) in items.iter().enumerate() {
            let submenu = item_fields(item).submenu;
            if submenu.is_null() {
                continue;
            }
            if shown == MAX_MENUS || !top_title(item, submenu, shown == 0, &mut pool) {
                break;
            }
            TOP_ITEMS[shown].store(index, Ordering::Relaxed);
            shown += 1;
            for &entry in menu_fields(submenu).items.as_slice() {
                if !add_entry(entry, &mut pool) {
                    break;
                }
            }
        }
    }
    SHOWN_MENUS.store(shown, Ordering::Relaxed);
    pool
}

/// Sends the main menu to `window`'s server window.
pub fn send_to(window: Id) {
    let id = window::server_id(window);
    if id == 0 || main_menu().is_null() {
        return;
    }
    let pool = build_pool();
    server::set_menu(id, pool.as_bytes());
}

fn refresh() {
    let mut index = 0;
    while let Some(window) = application::window_at(index) {
        send_to(window);
        index += 1;
    }
}

/// Makes `menu` the main menu and sends it to the app's windows.
pub fn set_main_menu(menu: Id) {
    if !menu.is_null() {
        objc_retain(menu);
    }
    let old = MAIN_MENU.swap(menu as usize, Ordering::AcqRel) as Id;
    if !old.is_null() && old != menu {
        objc_release(old);
    }
    refresh();
}

fn changed(item: Id) {
    if !main_menu().is_null() && !item_fields(item).menu.is_null() {
        refresh();
    }
}

fn perform(item: Id) {
    let fields = item_fields(item);
    if fields.separator || fields.disabled || fields.action == 0 {
        return;
    }
    application::send_action(fields.action as Sel, fields.target, item);
}

/// The menu in the top bar chose entry `entry` of menu `shown` (both as sent).
pub fn chosen(shown: usize, entry: usize) {
    if shown >= SHOWN_MENUS.load(Ordering::Relaxed) || main_menu().is_null() {
        return;
    }
    let top = TOP_ITEMS[shown].load(Ordering::Relaxed);
    let Some(&item) = menu_fields(main_menu()).items.as_slice().get(top) else {
        return;
    };
    let submenu = item_fields(item).submenu;
    if submenu.is_null() {
        return;
    }
    if let Some(&chosen) = menu_fields(submenu).items.as_slice().get(entry) {
        perform(chosen);
    }
}

fn find_key_equivalent(menu: Id, typed: u32, flags: u64) -> Option<Id> {
    for &item in menu_fields(menu).items.as_slice() {
        let fields = item_fields(item);
        if !fields.submenu.is_null() {
            if let Some(found) = find_key_equivalent(fields.submenu, typed, flags) {
                return Some(found);
            }
            continue;
        }
        if fields.separator || fields.disabled || fields.action == 0 {
            continue;
        }
        if answers(
            first_character(fields.key_equivalent),
            fields.key_mask,
            typed,
            flags,
        ) {
            return Some(item);
        }
    }
    None
}

/// Sends the action of the main menu's item whose key equivalent `event` (a key press) is;
/// whether there was one.
pub fn perform_key_equivalent(event: Id) -> bool {
    let main = main_menu();
    if main.is_null() {
        return false;
    }
    let flags = event::fields(event).flags;
    match find_key_equivalent(main, event::typed(event), flags) {
        Some(item) => {
            perform(item);
            true
        }
        None => false,
    }
}

fn new_item(title: Id, action: Sel, key: Id) -> Id {
    let item = new_instance(class_pointer(&NSMENUITEM));
    if item.is_null() {
        return item;
    }
    initialize_item(item, title, action, key);
    item
}

fn initialize_item(item: Id, title: Id, action: Sel, key: Id) {
    let fields = item_fields(item);
    store_copy(&mut fields.title, title);
    store_copy(&mut fields.key_equivalent, key);
    fields.action = action as usize;
    fields.key_mask = modifier::COMMAND;
}

fn add(menu: Id, item: Id) {
    if item.is_null() {
        return;
    }
    objc_retain(item);
    item_fields(item).menu = menu;
    menu_fields(menu).items.push(item);
    changed(item);
}

extern "C" fn menu_init_with_title(this: Id, _: Sel, title: Id) -> Id {
    if !this.is_null() {
        store_copy(&mut menu_fields(this).title, title);
    }
    this
}

extern "C" fn menu_init(this: Id, _: Sel) -> Id {
    let empty = string("");
    menu_init_with_title(this, core::ptr::null(), empty)
}

extern "C" fn menu_title(this: Id, _: Sel) -> Id {
    match menu_fields(this).title {
        none if none.is_null() => string(""),
        title => title,
    }
}

extern "C" fn menu_set_title(this: Id, _: Sel, title: Id) {
    store_copy(&mut menu_fields(this).title, title);
    if !main_menu().is_null() {
        refresh();
    }
}

extern "C" fn add_item(this: Id, _: Sel, item: Id) {
    add(this, item);
}

extern "C" fn add_item_with_title(this: Id, _: Sel, title: Id, action: Sel, key: Id) -> Id {
    let item = new_item(title, action, key);
    add(this, item);
    if item.is_null() {
        return item;
    }
    objc_release(item);
    item
}

extern "C" fn remove_all_items(this: Id, _: Sel) {
    let items = &mut menu_fields(this).items;
    for &item in items.as_slice() {
        item_fields(item).menu = core::ptr::null_mut();
        objc_release(item);
    }
    items.free();
    if !main_menu().is_null() {
        refresh();
    }
}

extern "C" fn item_array(this: Id, _: Sel) -> Id {
    let items = menu_fields(this).items.as_slice();
    send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        items.as_ptr() as usize,
        items.len(),
    )
}

extern "C" fn number_of_items(this: Id, _: Sel) -> isize {
    menu_fields(this).items.len() as isize
}

extern "C" fn item_at_index(this: Id, _: Sel, index: isize) -> Id {
    usize::try_from(index)
        .ok()
        .and_then(|index| menu_fields(this).items.as_slice().get(index).copied())
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn supermenu(this: Id, _: Sel) -> Id {
    menu_fields(this).supermenu
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn yes(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn separator_item(_: Id, _: Sel) -> Id {
    let item = new_item(
        core::ptr::null_mut(),
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    if item.is_null() {
        return item;
    }
    item_fields(item).separator = true;
    autoreleased(item)
}

extern "C" fn item_init_with_title(this: Id, _: Sel, title: Id, action: Sel, key: Id) -> Id {
    if !this.is_null() {
        initialize_item(this, title, action, key);
    }
    this
}

extern "C" fn item_init(this: Id, _: Sel) -> Id {
    let init: extern "C" fn(Id, Sel, Id, Sel, Id) -> Id = message::msg_send_as();
    let empty = string("");
    init(
        this,
        sel!("initWithTitle:action:keyEquivalent:"),
        empty,
        core::ptr::null(),
        empty,
    )
}

extern "C" fn item_title(this: Id, _: Sel) -> Id {
    match item_fields(this).title {
        none if none.is_null() => string(""),
        title => title,
    }
}

extern "C" fn item_set_title(this: Id, _: Sel, title: Id) {
    store_copy(&mut item_fields(this).title, title);
    changed(this);
}

extern "C" fn item_action(this: Id, _: Sel) -> Sel {
    item_fields(this).action as Sel
}

extern "C" fn item_set_action(this: Id, _: Sel, action: Sel) {
    item_fields(this).action = action as usize;
    changed(this);
}

extern "C" fn item_target(this: Id, _: Sel) -> Id {
    item_fields(this).target
}

extern "C" fn item_set_target(this: Id, _: Sel, target: Id) {
    item_fields(this).target = target;
}

extern "C" fn item_key_equivalent(this: Id, _: Sel) -> Id {
    match item_fields(this).key_equivalent {
        none if none.is_null() => string(""),
        key => key,
    }
}

extern "C" fn item_set_key_equivalent(this: Id, _: Sel, key: Id) {
    store_copy(&mut item_fields(this).key_equivalent, key);
    changed(this);
}

extern "C" fn item_key_mask(this: Id, _: Sel) -> u64 {
    item_fields(this).key_mask
}

extern "C" fn item_set_key_mask(this: Id, _: Sel, mask: u64) {
    item_fields(this).key_mask = mask;
    changed(this);
}

extern "C" fn item_submenu(this: Id, _: Sel) -> Id {
    item_fields(this).submenu
}

extern "C" fn item_set_submenu(this: Id, _: Sel, submenu: Id) {
    store(&mut item_fields(this).submenu, submenu);
    if !submenu.is_null() {
        menu_fields(submenu).supermenu = item_fields(this).menu;
    }
    changed(this);
}

extern "C" fn item_has_submenu(this: Id, _: Sel) -> u8 {
    u8::from(!item_fields(this).submenu.is_null())
}

extern "C" fn item_is_enabled(this: Id, _: Sel) -> u8 {
    u8::from(!item_fields(this).disabled)
}

extern "C" fn item_set_enabled(this: Id, _: Sel, enabled: u8) {
    item_fields(this).disabled = enabled == 0;
    changed(this);
}

extern "C" fn item_is_separator(this: Id, _: Sel) -> u8 {
    u8::from(item_fields(this).separator)
}

extern "C" fn item_tag(this: Id, _: Sel) -> isize {
    item_fields(this).tag
}

extern "C" fn item_set_tag(this: Id, _: Sel, tag: isize) {
    item_fields(this).tag = tag;
}

extern "C" fn item_menu(this: Id, _: Sel) -> Id {
    item_fields(this).menu
}

extern "C" fn item_state(_: Id, _: Sel) -> isize {
    0
}

extern "C" fn ignore_object(_: Id, _: Sel, _: Id) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let menu_instance = methods![
        "initWithTitle:" => menu_init_with_title,
        "init" => menu_init,
        "title" => menu_title,
        "setTitle:" => menu_set_title,
        "addItem:" => add_item,
        "addItemWithTitle:action:keyEquivalent:" => add_item_with_title,
        "removeAllItems" => remove_all_items,
        "itemArray" => item_array,
        "numberOfItems" => number_of_items,
        "itemAtIndex:" => item_at_index,
        "supermenu" => supermenu,
        "setAutoenablesItems:" => ignore_word,
        "autoenablesItems" => yes,
    ];
    let item_instance = methods![
        "initWithTitle:action:keyEquivalent:" => item_init_with_title,
        "init" => item_init,
        "title" => item_title,
        "setTitle:" => item_set_title,
        "action" => item_action,
        "setAction:" => item_set_action,
        "target" => item_target,
        "setTarget:" => item_set_target,
        "keyEquivalent" => item_key_equivalent,
        "setKeyEquivalent:" => item_set_key_equivalent,
        "keyEquivalentModifierMask" => item_key_mask,
        "setKeyEquivalentModifierMask:" => item_set_key_mask,
        "submenu" => item_submenu,
        "setSubmenu:" => item_set_submenu,
        "hasSubmenu" => item_has_submenu,
        "isEnabled" => item_is_enabled,
        "setEnabled:" => item_set_enabled,
        "isSeparatorItem" => item_is_separator,
        "tag" => item_tag,
        "setTag:" => item_set_tag,
        "menu" => item_menu,
        "state" => item_state,
        "setState:" => ignore_word,
        "setImage:" => ignore_object,
        "setRepresentedObject:" => ignore_object,
    ];
    let item_class = methods![
        "separatorItem" => separator_item,
    ];
    super::define(
        runtime,
        b"NSMenu\0",
        (&NSMENU, &NSMENU_META),
        &NSOBJECT,
        core::mem::size_of::<MenuFields>(),
        &menu_instance,
        &[],
    ) && super::define(
        runtime,
        b"NSMenuItem\0",
        (&NSMENUITEM, &NSMENUITEM_META),
        &NSOBJECT,
        core::mem::size_of::<ItemFields>(),
        &item_instance,
        &item_class,
    )
}

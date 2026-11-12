//! The shell's namespace (D399) over the file system: item ID lists (`crate::item_lists`) in
//! blocks of the task allocator, so `ILFree` and `CoTaskMemFree` both give them back; the
//! desktop folder and file-system folders as `IShellFolder` objects, which parse Windows paths
//! into lists, list their children through `IEnumIDList`, bind to subfolders, compare, give
//! attributes and display names and rename; `IShellItem` over any list; and the folder picker
//! `SHBrowseForFolderW`, which runs the system picker in `folder` mode. The desktop is the
//! namespace's root; its one child is the file system's root, `C:\`. Shell views, context
//! menus, data objects and icons (`CreateViewObject`, `GetUIObjectOf`) are not supported, and
//! the folder picker calls no `BFFCALLBACK`: it has no dialog window to send messages to.

use super::{Bool, FALSE, TRUE, heap, pick_process};
use crate::com::guid::{self, Guid};
use crate::com::{E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, HResult, S_FALSE, S_OK};
use crate::item_lists::{self, FOLDER, Item};
use crate::library::Export;
use crate::linux::{self, AT_FDCWD};
use crate::text::{self, PATH_CAPACITY};
use core::cmp::Ordering;
use core::sync::atomic::{AtomicU32, Ordering as Atomic};

const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const E_FAIL: HResult = 0x8000_4005_u32 as i32;
const MK_E_NOOBJECT: HResult = 0x8004_01e5_u32 as i32;
const MAX_PATH: usize = 260;
const LONGEST_LIST: usize = 0x1_0000;
const LIST_CAPACITY: usize = PATH_CAPACITY * 2;
const PICK_OUTPUT_BYTES: usize = PATH_CAPACITY + 2;
const TITLE_BYTES: usize = 256;
const DESKTOP_NAME: &str = "Desktop";
const ROOT_NAME: &str = "C:";

const SFGAO_CANCOPY: u32 = 0x0000_0001;
const SFGAO_CANMOVE: u32 = 0x0000_0002;
const SFGAO_CANRENAME: u32 = 0x0000_0010;
const SFGAO_CANDELETE: u32 = 0x0000_0020;
const SFGAO_STREAM: u32 = 0x0040_0000;
const SFGAO_FILESYSANCESTOR: u32 = 0x1000_0000;
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_FILESYSTEM: u32 = 0x4000_0000;
const SFGAO_HASSUBFOLDER: u32 = 0x8000_0000;
const FOLDER_ATTRIBUTES: u32 = SFGAO_FOLDER | SFGAO_FILESYSANCESTOR | SFGAO_HASSUBFOLDER;
const ENTRY_ATTRIBUTES: u32 =
    SFGAO_FILESYSTEM | SFGAO_CANCOPY | SFGAO_CANMOVE | SFGAO_CANRENAME | SFGAO_CANDELETE;

const SHCONTF_FOLDERS: u32 = 0x20;
const SHCONTF_NONFOLDERS: u32 = 0x40;
const SHCONTF_INCLUDEHIDDEN: u32 = 0x80;
const SHGDN_INFOLDER: u32 = 0x1;
const SHGDN_FORPARSING: u32 = 0x8000;
const STRRET_WSTR: u32 = 0;
const STRRET_POINTER_OFFSET: usize = 8;

const SIGDN_DESKTOPABSOLUTEPARSING: u32 = 0x8002_8000;
const SIGDN_DESKTOPABSOLUTEEDITING: u32 = 0x8004_c000;
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
const SIGDN_URL: u32 = 0x8006_8000;
const SICHINT_DISPLAY: u32 = 0;

const BROWSE_OWNER: usize = 0;
const BROWSE_ROOT: usize = 8;
const BROWSE_DISPLAY_NAME: usize = 16;
const BROWSE_TITLE: usize = 24;

const SHELL_FOLDER: Guid = Guid::new(0x0002_14e6, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const ENUM_ID_LIST: Guid = Guid::new(0x0002_14f2, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const SHELL_ITEM: Guid = Guid::new(
    0x4382_6d1e,
    0xe718,
    0x42ee,
    [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe],
);
const BHID_SF_OBJECT: Guid = Guid::new(
    0x3981_e224,
    0xf559,
    0x11d3,
    [0x8e, 0x3a, 0x00, 0xc0, 0x4f, 0x68, 0x37, 0xd5],
);

#[derive(Debug)]
#[repr(transparent)]
struct Table<const N: usize>([*const (); N]);

// SAFETY: the table holds functions' addresses, fixed for the program's life.
unsafe impl<const N: usize> Sync for Table<N> {}

impl<const N: usize> Table<N> {
    fn address(&'static self) -> usize {
        self.0.as_ptr() as usize
    }
}

/// A shell folder or a shell item: its table, its reference count and its absolute list.
#[repr(C)]
#[derive(Debug)]
struct Node {
    table: usize,
    references: AtomicU32,
    list: *mut u8,
}

/// A folder's children, listed when it was made: their one-item lists back to back.
#[repr(C)]
#[derive(Debug)]
struct Children {
    table: usize,
    references: AtomicU32,
    lists: *mut u8,
    length: usize,
    at: usize,
}

/// The list at `list` with its terminator, if it ends within [`LONGEST_LIST`] bytes.
///
/// # Safety
/// `list` must be null or a readable item ID list.
unsafe fn list_bytes<'a>(list: *const u8) -> Option<&'a [u8]> {
    if list.is_null() {
        return None;
    }
    let mut at = 0usize;
    loop {
        // SAFETY: the caller's duty: the list's sizes lead from item to item up to its end.
        let size = unsafe { list.add(at).cast::<[u8; 2]>().read_unaligned() };
        match usize::from(u16::from_le_bytes(size)) {
            0 => break,
            size if size < 2 || at + size > LONGEST_LIST => return None,
            size => at += size,
        }
    }
    // SAFETY: as above: the `at` bytes walked and the two of the terminator.
    Some(unsafe { core::slice::from_raw_parts(list, at + 2) })
}

fn new_list(parts: &[&[u8]]) -> *mut u8 {
    let length: usize = parts.iter().map(|part| part.len()).sum::<usize>() + 2;
    let block = heap::allocate(length);
    if block.is_null() {
        return block;
    }
    let mut at = 0;
    for part in parts {
        // SAFETY: the fresh block holds `length` bytes, the parts' and a zeroed terminator.
        unsafe { block.add(at).copy_from_nonoverlapping(part.as_ptr(), part.len()) };
        at += part.len();
    }
    block
}

fn without_terminator(list: &[u8]) -> &[u8] {
    &list[..list.len().saturating_sub(2)]
}

fn terminated(path: &[u8], out: &mut [u8; PATH_CAPACITY + 1]) -> Option<usize> {
    out.get_mut(..path.len())?.copy_from_slice(path);
    out[path.len()] = 0;
    Some(path.len() + 1)
}

fn kind_of(path: &[u8]) -> Option<bool> {
    let mut nul = [0u8; PATH_CAPACITY + 1];
    let length = terminated(path, &mut nul)?;
    let words = linux::stat_at(AT_FDCWD, &nul[..length]).ok()?;
    Some(linux::file_mode(&words) & linux::FILE_TYPE == linux::DIRECTORY)
}

fn path_of(list: &[u8], out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    item_lists::path(list, out)
}

fn attributes_of(found: &Item<'_>) -> u32 {
    let kind = match found.is_folder() {
        true => FOLDER_ATTRIBUTES,
        false => SFGAO_STREAM,
    };
    match found.is_root() {
        true => FOLDER_ATTRIBUTES | SFGAO_FILESYSTEM,
        false => kind | ENTRY_ATTRIBUTES,
    }
}

fn last_item(list: &[u8]) -> Option<Item<'_>> {
    let body = without_terminator(list);
    let at = item_lists::last_offset(list)?;
    item_lists::item(&body[at..])
}

fn from_win32(code: u32) -> HResult {
    super::shell_folders::from_win32(code)
}

fn wide_block(units: &[u16]) -> *mut u16 {
    let block = heap::allocate((units.len() + 1) * 2).cast::<u16>();
    if !block.is_null() {
        // SAFETY: a fresh, zeroed block of `units.len() + 1` units: the text and its NUL.
        unsafe { block.copy_from_nonoverlapping(units.as_ptr(), units.len()) };
    }
    block
}

fn windows_units(path: &[u8], out: &mut [u16; PATH_CAPACITY]) -> Option<usize> {
    super::directory::write_windows_path(path, out).ok()
}

fn name_units(list: &[u8], out: &mut [u16; PATH_CAPACITY]) -> usize {
    match last_item(list) {
        None => text::write_utf16(DESKTOP_NAME.as_bytes(), out),
        Some(found) if found.is_root() => text::write_utf16(ROOT_NAME.as_bytes(), out),
        Some(found) => text::write_utf16(found.name, out),
    }
}

fn parsing_units(list: &[u8], out: &mut [u16; PATH_CAPACITY]) -> Option<usize> {
    let mut path = [0u8; PATH_CAPACITY];
    match path_of(list, &mut path) {
        Some(length) => windows_units(&path[..length], out),
        None if item_lists::count(list) == 0 => {
            Some(text::write_utf16(DESKTOP_NAME.as_bytes(), out))
        }
        None => None,
    }
}

fn url_units(list: &[u8], out: &mut [u16; PATH_CAPACITY]) -> Option<usize> {
    let mut path = [0u8; PATH_CAPACITY];
    let length = path_of(list, &mut path)?;
    let prefix = b"file:///C:";
    let mut written = text::write_utf16(prefix, out);
    written += text::write_utf16(&path[..length], &mut out[written..]);
    (written < PATH_CAPACITY).then_some(written)
}

fn new_node(table: usize, list: *mut u8) -> Result<*mut Node, HResult> {
    if list.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    let node = crate::com::allocate(core::mem::size_of::<Node>()).cast::<Node>();
    if node.is_null() {
        // SAFETY: the list was made for the node, which was not made.
        unsafe { heap::free(list) };
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: a fresh block, sized and aligned for a node.
    unsafe {
        node.write(Node {
            table,
            references: AtomicU32::new(1),
            list,
        })
    };
    Ok(node)
}

unsafe fn node<'a>(this: *mut Node) -> &'a Node {
    // SAFETY: the caller's duty: a live node.
    unsafe { &*this }
}

unsafe fn node_list<'a>(this: *mut Node) -> &'a [u8] {
    // SAFETY: the caller's duty: a live node, whose list it made and keeps.
    unsafe { list_bytes(node(this).list) }.unwrap_or(&[0, 0])
}

unsafe fn write_out<T>(out: *mut T, value: T) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a writable pointer from the program.
    unsafe { out.write_unaligned(value) };
    S_OK
}

fn hand_out(result: Result<*mut Node, HResult>, wanted: *const Guid, out: *mut usize) -> HResult {
    let made = match result {
        Ok(made) => made,
        Err(failure) => {
            // SAFETY: the caller checked `out`.
            unsafe { out.write(0) };
            return failure;
        }
    };
    // SAFETY: a live node the call just made, which holds one reference.
    let table = unsafe { node(made).table };
    let query: unsafe extern "efiapi" fn(*mut Node, *const Guid, *mut usize) -> HResult =
        match table == FOLDER_TABLE.address() {
            true => folder_query_interface,
            false => item_query_interface,
        };
    // SAFETY: as above; `out` is writable.
    let answer = unsafe { query(made, wanted, out) };
    // SAFETY: the call's own reference, given back after the query took its own.
    unsafe { node_release(made) };
    answer
}

/// A new shell folder over the absolute list `list`, which it takes.
fn new_folder(list: *mut u8) -> Result<*mut Node, HResult> {
    new_node(FOLDER_TABLE.address(), list)
}

/// A new shell item over the absolute list `list`, which it takes.
fn new_item(list: *mut u8) -> Result<*mut Node, HResult> {
    new_node(ITEM_TABLE.address(), list)
}

unsafe extern "efiapi" fn folder_query_interface(
    this: *mut Node,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &node(this).references };
    let interfaces = [guid::UNKNOWN, SHELL_FOLDER];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn item_query_interface(
    this: *mut Node,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &node(this).references };
    let interfaces = [guid::UNKNOWN, SHELL_ITEM];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn node_add_reference(this: *mut Node) -> u32 {
    // SAFETY: the caller's duty: a live node.
    unsafe { node(this) }.references.fetch_add(1, Atomic::Relaxed) + 1
}

unsafe extern "efiapi" fn node_release(this: *mut Node) -> u32 {
    // SAFETY: the caller's duty: a live node it holds a reference to.
    let found = unsafe { node(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    // SAFETY: the node's own list, which nothing uses once it is gone.
    unsafe { heap::free(found.list) };
    // SAFETY: the node's block; no reference is left.
    unsafe { crate::com::free(this.cast()) };
    0
}

fn folder_path(list: &[u8], out: &mut [u8; PATH_CAPACITY]) -> Option<usize> {
    match item_lists::count(list) {
        0 => Some(0),
        _ => path_of(list, out),
    }
}

fn parse_in_folder(folder: &[u8], name: &[u16]) -> Result<(*mut u8, bool), HResult> {
    let mut eggos = [0u8; PATH_CAPACITY];
    let typed = text::eggos_path(text::utf16_chars(name), &mut eggos).ok_or(E_INVALIDARG)? - 1;
    let mut base = [0u8; PATH_CAPACITY];
    let base_length = folder_path(folder, &mut base).ok_or(E_INVALIDARG)?;
    let desktop = base_length == 0;
    if desktop && eggos.first() != Some(&b'/') {
        return Err(E_INVALIDARG);
    }
    let base = match desktop {
        true => &b"/"[..],
        false => &base[..base_length],
    };
    let mut full = [0u8; PATH_CAPACITY];
    let length = text::absolute_path(base, &eggos[..typed], &mut full).ok_or(E_INVALIDARG)?;
    let full = &full[..length];
    let is_folder = kind_of(full).ok_or(from_win32(super::error::FILE_NOT_FOUND))?;
    let mut list = [0u8; LIST_CAPACITY];
    let written = match desktop {
        true => item_lists::write_absolute(full, is_folder, &mut list),
        false => {
            let below = full
                .strip_prefix(base)
                .filter(|rest| base == b"/" || rest.first() == Some(&b'/'))
                .ok_or(E_INVALIDARG)?;
            item_lists::write_relative(below, is_folder, &mut list)
        }
    }
    .ok_or(E_INVALIDARG)?;
    let made = new_list(&[without_terminator(&list[..written])]);
    match made.is_null() {
        true => Err(E_OUTOFMEMORY),
        false => Ok((made, is_folder)),
    }
}

unsafe extern "efiapi" fn folder_parse_display_name(
    this: *mut Node,
    _window: usize,
    _context: usize,
    name: *const u16,
    eaten: *mut u32,
    out: *mut *mut u8,
    attributes: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable list pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    if name.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: a live folder and a NUL-terminated name.
    let (list, name) = unsafe { (node_list(this), super::file::wide_string(name)) };
    let (made, is_folder) = match parse_in_folder(list, name) {
        Ok(made) => made,
        Err(failure) => return failure,
    };
    if !eaten.is_null() {
        // SAFETY: the caller passes a writable count.
        unsafe { eaten.write(name.len() as u32) };
    }
    if !attributes.is_null() {
        let found = Item {
            name: b"x",
            flags: if is_folder { FOLDER } else { 0 },
        };
        // SAFETY: the caller passes a readable and writable mask.
        unsafe { attributes.write(attributes.read() & attributes_of(&found)) };
    }
    // SAFETY: as above.
    unsafe { out.write(made) };
    S_OK
}

fn child_wanted(name: &[u8], is_folder: bool, flags: u32) -> bool {
    let kind = match is_folder {
        true => flags & SHCONTF_FOLDERS != 0,
        false => flags & SHCONTF_NONFOLDERS != 0,
    };
    kind && (flags & SHCONTF_INCLUDEHIDDEN != 0 || !name.starts_with(b"."))
}

fn list_children(list: &[u8], flags: u32) -> Result<(*mut u8, usize), HResult> {
    if item_lists::count(list) == 0 {
        if flags & SHCONTF_FOLDERS == 0 {
            return Ok((core::ptr::null_mut(), 0));
        }
        let mut root = [0u8; LIST_CAPACITY];
        let written = item_lists::write_absolute(b"/", true, &mut root).ok_or(E_FAIL)?;
        let made = new_list(&[without_terminator(&root[..written])]);
        return match made.is_null() {
            true => Err(E_OUTOFMEMORY),
            false => Ok((made, written)),
        };
    }
    let mut path = [0u8; PATH_CAPACITY];
    let length = path_of(list, &mut path).ok_or(E_INVALIDARG)?;
    let mut nul = [0u8; PATH_CAPACITY + 1];
    let length = terminated(&path[..length], &mut nul).ok_or(E_INVALIDARG)?;
    let mut needed = 0usize;
    let readable = super::directory::for_each_entry(&nul[..length], &mut |name, is_folder| {
        if child_wanted(name, is_folder, flags) {
            needed += name.len() + 7;
        }
    });
    if !readable {
        return Err(from_win32(super::error::PATH_NOT_FOUND));
    }
    let block = heap::allocate(needed.max(1));
    if block.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the fresh block of `needed` bytes (at least one), which nothing else uses.
    let target = unsafe { core::slice::from_raw_parts_mut(block, needed.max(1)) };
    let mut filled = 0;
    super::directory::for_each_entry(&nul[..length], &mut |name, is_folder| {
        if !child_wanted(name, is_folder, flags) || name.contains(&b'/') {
            return;
        }
        if let Some(written) = item_lists::write_relative(name, is_folder, &mut target[filled..]) {
            filled += written;
        }
    });
    Ok((block, filled))
}

unsafe extern "efiapi" fn folder_enum_objects(
    this: *mut Node,
    _window: usize,
    flags: u32,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live folder.
    let list = unsafe { node_list(this) };
    let (lists, length) = match list_children(list, flags) {
        Ok(found) => found,
        Err(failure) => {
            // SAFETY: the caller passes a writable interface pointer.
            unsafe { out.write(0) };
            return failure;
        }
    };
    let children = crate::com::allocate(core::mem::size_of::<Children>()).cast::<Children>();
    if children.is_null() {
        // SAFETY: the lists were made for the enumerator, which was not made.
        unsafe { heap::free(lists) };
        // SAFETY: as above.
        unsafe { out.write(0) };
        return E_OUTOFMEMORY;
    }
    // SAFETY: a fresh block, sized and aligned for an enumerator.
    unsafe {
        children.write(Children {
            table: CHILDREN_TABLE.address(),
            references: AtomicU32::new(1),
            lists,
            length,
            at: 0,
        })
    };
    // SAFETY: as above.
    unsafe { out.write(children as usize) };
    match length {
        0 => S_FALSE,
        _ => S_OK,
    }
}

unsafe extern "efiapi" fn folder_bind_to_object(
    this: *mut Node,
    relative: *const u8,
    _context: usize,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live folder and a relative list.
    let (list, relative) = unsafe { (node_list(this), list_bytes(relative)) };
    let Some(relative) = relative.filter(|relative| item_lists::count(relative) > 0) else {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return E_INVALIDARG;
    };
    let made = new_list(&[without_terminator(list), without_terminator(relative)]);
    if made.is_null() {
        // SAFETY: as above.
        unsafe { out.write(0) };
        return E_OUTOFMEMORY;
    }
    // SAFETY: the list just made, whole.
    let full = unsafe { list_bytes(made) }.unwrap_or(&[0, 0]);
    let mut path = [0u8; PATH_CAPACITY];
    let is_folder = path_of(full, &mut path).and_then(|length| kind_of(&path[..length]));
    if is_folder != Some(true) {
        // SAFETY: the list made above, unused.
        unsafe { heap::free(made) };
        // SAFETY: as above.
        unsafe { out.write(0) };
        return from_win32(super::error::PATH_NOT_FOUND);
    }
    hand_out(new_folder(made), interface, out)
}

unsafe extern "efiapi" fn folder_bind_to_storage(
    _this: *mut Node,
    _relative: *const u8,
    _context: usize,
    _interface: *const Guid,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller passes a writable interface pointer, or null.
    let _ = unsafe { write_out(out, 0) };
    E_NOTIMPL
}

fn compare_lists(a: &[u8], b: &[u8]) -> Ordering {
    let mut a_items = item_lists::items(a);
    let mut b_items = item_lists::items(b);
    loop {
        match (a_items.next(), b_items.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) => {
                let a = a.unwrap_or(Item { name: b"", flags: 0 });
                let b = b.unwrap_or(Item { name: b"", flags: 0 });
                let order = item_lists::compare(&a, &b);
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

fn order_result(order: Ordering) -> HResult {
    let code: i16 = match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    i32::from(code as u16)
}

unsafe extern "efiapi" fn folder_compare_ids(
    _this: *mut Node,
    _column: usize,
    a: *const u8,
    b: *const u8,
) -> HResult {
    // SAFETY: the caller's duty: relative lists of the folder.
    let (Some(a), Some(b)) = (unsafe { list_bytes(a) }, unsafe { list_bytes(b) }) else {
        return E_INVALIDARG;
    };
    order_result(compare_lists(a, b))
}

unsafe extern "efiapi" fn folder_create_view_object(
    _this: *mut Node,
    _window: usize,
    _interface: *const Guid,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller passes a writable interface pointer, or null.
    let _ = unsafe { write_out(out, 0) };
    E_NOINTERFACE
}

unsafe extern "efiapi" fn folder_get_attributes_of(
    this: *mut Node,
    count: u32,
    lists: *const *const u8,
    mask: *mut u32,
) -> HResult {
    if mask.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live folder.
    let desktop = item_lists::count(unsafe { node_list(this) }) == 0;
    let mut attributes = u32::MAX;
    if count == 0 {
        attributes = match desktop {
            true => FOLDER_ATTRIBUTES,
            false => FOLDER_ATTRIBUTES | ENTRY_ATTRIBUTES,
        };
    }
    for index in 0..count as usize {
        // SAFETY: the caller's duty: `count` readable list pointers.
        let list = unsafe { lists.add(index).read() };
        // SAFETY: as above: each a relative list of the folder.
        let Some(found) = unsafe { list_bytes(list) }.and_then(last_item) else {
            return E_INVALIDARG;
        };
        attributes &= attributes_of(&found);
    }
    // SAFETY: the caller passes a readable and writable mask.
    unsafe { mask.write(mask.read() & attributes) };
    S_OK
}

unsafe extern "efiapi" fn folder_get_ui_object_of(
    _this: *mut Node,
    _window: usize,
    _count: u32,
    _lists: *const *const u8,
    _interface: *const Guid,
    _reserved: usize,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller passes a writable interface pointer, or null.
    let _ = unsafe { write_out(out, 0) };
    E_NOTIMPL
}

fn display_units(full: &[u8], flags: u32, out: &mut [u16; PATH_CAPACITY]) -> Option<usize> {
    match flags & SHGDN_FORPARSING != 0 && flags & SHGDN_INFOLDER == 0 {
        true => parsing_units(full, out),
        false => Some(name_units(full, out)),
    }
}

unsafe extern "efiapi" fn folder_get_display_name_of(
    this: *mut Node,
    relative: *const u8,
    flags: u32,
    name: *mut u8,
) -> HResult {
    if name.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live folder and a relative list.
    let (list, relative) = unsafe { (node_list(this), list_bytes(relative)) };
    let Some(relative) = relative else {
        return E_INVALIDARG;
    };
    let mut joined = [0u8; LIST_CAPACITY * 2];
    let (head, tail) = (without_terminator(list), without_terminator(relative));
    let Some(target) = joined.get_mut(..head.len() + tail.len() + 2) else {
        return E_INVALIDARG;
    };
    target[..head.len()].copy_from_slice(head);
    target[head.len()..head.len() + tail.len()].copy_from_slice(tail);
    let mut units = [0u16; PATH_CAPACITY];
    let Some(length) = display_units(target, flags, &mut units) else {
        return E_INVALIDARG;
    };
    let block = wide_block(&units[..length]);
    if block.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the caller passes a writable `STRRET`: its kind, then on 64-bit Windows its
    // union at offset 8, which holds the string's pointer for `STRRET_WSTR`.
    unsafe {
        name.cast::<u32>().write_unaligned(STRRET_WSTR);
        name.add(STRRET_POINTER_OFFSET).cast::<usize>().write_unaligned(block as usize);
    }
    S_OK
}

fn rename_in(folder: &[u8], relative: &[u8], new_name: &[u16]) -> Result<*mut u8, HResult> {
    let found = last_item(relative).ok_or(E_INVALIDARG)?;
    let mut base = [0u8; PATH_CAPACITY];
    let base_length = folder_path(folder, &mut base).ok_or(E_INVALIDARG)?;
    let mut from = [0u8; PATH_CAPACITY];
    from[..base_length].copy_from_slice(&base[..base_length]);
    let from_length =
        item_lists::append_path(relative, &mut from, base_length).ok_or(E_INVALIDARG)?;
    let mut name = [0u8; PATH_CAPACITY];
    let (name_length, _) = text::write_utf8(new_name, &mut name);
    let name = &name[..name_length];
    if name.is_empty() || name.contains(&b'/') || name.contains(&b'\\') {
        return Err(E_INVALIDARG);
    }
    let parent_length = from[..from_length]
        .iter()
        .rposition(|&byte| byte == b'/')
        .ok_or(E_INVALIDARG)?;
    let mut to = [0u8; PATH_CAPACITY];
    to[..=parent_length].copy_from_slice(&from[..=parent_length]);
    let to_length = parent_length + 1 + name.len();
    to.get_mut(parent_length + 1..to_length).ok_or(E_INVALIDARG)?.copy_from_slice(name);
    let mut from_nul = [0u8; PATH_CAPACITY + 1];
    let mut to_nul = [0u8; PATH_CAPACITY + 1];
    let from_nul_length = terminated(&from[..from_length], &mut from_nul).ok_or(E_INVALIDARG)?;
    let to_nul_length = terminated(&to[..to_length], &mut to_nul).ok_or(E_INVALIDARG)?;
    linux::rename(&from_nul[..from_nul_length], &to_nul[..to_nul_length], true)
        .map_err(|errno| from_win32(super::error_for(errno)))?;
    let body = without_terminator(relative);
    let at = item_lists::last_offset(relative).ok_or(E_INVALIDARG)?;
    let mut tail = [0u8; LIST_CAPACITY];
    let written =
        item_lists::write_relative(name, found.is_folder(), &mut tail).ok_or(E_INVALIDARG)?;
    let made = new_list(&[&body[..at], without_terminator(&tail[..written])]);
    match made.is_null() {
        true => Err(E_OUTOFMEMORY),
        false => Ok(made),
    }
}

unsafe extern "efiapi" fn folder_set_name_of(
    this: *mut Node,
    _window: usize,
    relative: *const u8,
    name: *const u16,
    _flags: u32,
    out: *mut *mut u8,
) -> HResult {
    if !out.is_null() {
        // SAFETY: the caller passes a writable list pointer, or null.
        unsafe { out.write(core::ptr::null_mut()) };
    }
    if name.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: a live folder, a relative list and a NUL-terminated name.
    let (list, relative, name) =
        unsafe { (node_list(this), list_bytes(relative), super::file::wide_string(name)) };
    let Some(relative) = relative else {
        return E_INVALIDARG;
    };
    let made = match rename_in(list, relative, name) {
        Ok(made) => made,
        Err(failure) => return failure,
    };
    match out.is_null() {
        // SAFETY: the list made above, which nobody asked for.
        true => unsafe { heap::free(made) },
        // SAFETY: as above: a writable list pointer.
        false => unsafe { out.write(made) },
    }
    S_OK
}

unsafe fn children<'a>(this: *mut Children) -> &'a mut Children {
    // SAFETY: the caller's duty: a live enumerator, used by one thread at a time.
    unsafe { &mut *this }
}

unsafe extern "efiapi" fn children_query_interface(
    this: *mut Children,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &children(this).references };
    let interfaces = [guid::UNKNOWN, ENUM_ID_LIST];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn children_add_reference(this: *mut Children) -> u32 {
    // SAFETY: the caller's duty: a live enumerator.
    unsafe { children(this) }.references.fetch_add(1, Atomic::Relaxed) + 1
}

unsafe extern "efiapi" fn children_release(this: *mut Children) -> u32 {
    // SAFETY: the caller's duty: a live enumerator it holds a reference to.
    let found = unsafe { children(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    // SAFETY: the enumerator's own lists, which nothing uses once it is gone.
    unsafe { heap::free(found.lists) };
    // SAFETY: the enumerator's block; no reference is left.
    unsafe { crate::com::free(this.cast()) };
    0
}

fn listed(found: &Children) -> &[u8] {
    match found.lists.is_null() {
        true => &[],
        // SAFETY: the enumerator's own `length` bytes of lists.
        false => unsafe { core::slice::from_raw_parts(found.lists, found.length) },
    }
}

fn next_list(found: &mut Children) -> Option<*mut u8> {
    let lists = listed(found);
    let rest = lists.get(found.at..)?;
    let length = item_lists::length(rest).filter(|&length| length > 2)?;
    found.at += length;
    Some(new_list(&[without_terminator(&rest[..length])]))
}

unsafe extern "efiapi" fn children_next(
    this: *mut Children,
    wanted: u32,
    out: *mut *mut u8,
    fetched: *mut u32,
) -> HResult {
    if out.is_null() || (wanted > 1 && fetched.is_null()) {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live enumerator.
    let found = unsafe { children(this) };
    let mut given = 0u32;
    while given < wanted {
        let Some(list) = next_list(found) else {
            break;
        };
        if list.is_null() {
            break;
        }
        // SAFETY: the caller passes room for `wanted` list pointers.
        unsafe { out.add(given as usize).write(list) };
        given += 1;
    }
    if !fetched.is_null() {
        // SAFETY: the caller passes a writable count.
        unsafe { fetched.write(given) };
    }
    if given == wanted { S_OK } else { S_FALSE }
}

unsafe extern "efiapi" fn children_skip(this: *mut Children, count: u32) -> HResult {
    // SAFETY: the caller's duty: a live enumerator.
    let found = unsafe { children(this) };
    for _ in 0..count {
        let lists = listed(found);
        let Some(length) = lists.get(found.at..).and_then(item_lists::length) else {
            return S_FALSE;
        };
        if length <= 2 {
            return S_FALSE;
        }
        found.at += length;
    }
    S_OK
}

unsafe extern "efiapi" fn children_reset(this: *mut Children) -> HResult {
    // SAFETY: the caller's duty: a live enumerator.
    unsafe { children(this) }.at = 0;
    S_OK
}

unsafe extern "efiapi" fn children_clone(this: *mut Children, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live enumerator.
    let found = unsafe { children(this) };
    let copy = heap::allocate(found.length.max(1));
    let block = crate::com::allocate(core::mem::size_of::<Children>()).cast::<Children>();
    if copy.is_null() || block.is_null() {
        // SAFETY: blocks made here, unused.
        unsafe {
            heap::free(copy);
            crate::com::free(block.cast());
            out.write(0);
        }
        return E_OUTOFMEMORY;
    }
    let lists = listed(found);
    // SAFETY: the fresh copy holds at least `lists.len()` bytes; the block fits an enumerator.
    unsafe {
        copy.copy_from_nonoverlapping(lists.as_ptr(), lists.len());
        block.write(Children {
            table: CHILDREN_TABLE.address(),
            references: AtomicU32::new(1),
            lists: copy,
            length: found.length,
            at: found.at,
        });
        out.write(block as usize);
    }
    S_OK
}

unsafe extern "efiapi" fn item_bind_to_handler(
    this: *mut Node,
    _context: usize,
    handler: *const Guid,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a readable handler id.
    if unsafe { Guid::read(handler) } != Some(BHID_SF_OBJECT) {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return MK_E_NOOBJECT;
    }
    // SAFETY: the caller's duty: a live item.
    let list = unsafe { node_list(this) };
    let mut path = [0u8; PATH_CAPACITY];
    let folder = match path_of(list, &mut path) {
        Some(length) => kind_of(&path[..length]) == Some(true),
        None => item_lists::count(list) == 0,
    };
    if !folder {
        // SAFETY: as above.
        unsafe { out.write(0) };
        return E_INVALIDARG;
    }
    hand_out(new_folder(new_list(&[without_terminator(list)])), interface, out)
}

unsafe extern "efiapi" fn item_get_parent(this: *mut Node, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live item.
    let list = unsafe { node_list(this) };
    let Some(at) = item_lists::last_offset(list) else {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return MK_E_NOOBJECT;
    };
    let made = new_list(&[&list[..at]]);
    match new_item(made) {
        Ok(parent) => {
            // SAFETY: as above.
            unsafe { out.write(parent as usize) };
            S_OK
        }
        Err(failure) => {
            // SAFETY: as above.
            unsafe { out.write(0) };
            failure
        }
    }
}

unsafe extern "efiapi" fn item_get_display_name(
    this: *mut Node,
    form: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable string pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    // SAFETY: the caller's duty: a live item.
    let list = unsafe { node_list(this) };
    let mut units = [0u16; PATH_CAPACITY];
    let length = match form {
        SIGDN_FILESYSPATH => {
            let mut path = [0u8; PATH_CAPACITY];
            path_of(list, &mut path).and_then(|length| windows_units(&path[..length], &mut units))
        }
        SIGDN_DESKTOPABSOLUTEPARSING | SIGDN_DESKTOPABSOLUTEEDITING => {
            parsing_units(list, &mut units)
        }
        SIGDN_URL => url_units(list, &mut units),
        _ => Some(name_units(list, &mut units)),
    };
    let Some(length) = length else {
        return E_INVALIDARG;
    };
    let block = wide_block(&units[..length]);
    if block.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: as above.
    unsafe { out.write(block) };
    S_OK
}

fn item_attributes(list: &[u8]) -> u32 {
    match last_item(list) {
        None => FOLDER_ATTRIBUTES,
        Some(found) => attributes_of(&found),
    }
}

unsafe extern "efiapi" fn item_get_attributes(
    this: *mut Node,
    mask: u32,
    out: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live item.
    let attributes = item_attributes(unsafe { node_list(this) }) & mask;
    // SAFETY: the caller passes a writable mask.
    unsafe { out.write(attributes) };
    if attributes == mask { S_OK } else { S_FALSE }
}

unsafe extern "efiapi" fn item_compare(
    this: *mut Node,
    other: *mut Node,
    hint: u32,
    order: *mut i32,
) -> HResult {
    if order.is_null() || other.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a shell item; only the layer's own items are compared.
    if unsafe { node(other).table } != ITEM_TABLE.address() {
        // SAFETY: the caller passes a writable order.
        unsafe { order.write(1) };
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: two live items.
    let (a, b) = unsafe { (node_list(this), node_list(other)) };
    let found = match hint & 0xffff == SICHINT_DISPLAY {
        true => {
            let mut a_units = [0u16; PATH_CAPACITY];
            let mut b_units = [0u16; PATH_CAPACITY];
            let a_length = name_units(a, &mut a_units);
            let b_length = name_units(b, &mut b_units);
            crate::text::compare_logical(&a_units[..a_length], &b_units[..b_length])
                .then_with(|| compare_lists(a, b))
        }
        false => compare_lists(a, b),
    };
    let result = match found {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    };
    // SAFETY: as above.
    unsafe { order.write(result) };
    if result == 0 { S_OK } else { S_FALSE }
}

static FOLDER_TABLE: Table<13> = Table([
    export_entry!(folder_query_interface),
    export_entry!(node_add_reference),
    export_entry!(node_release),
    export_entry!(folder_parse_display_name),
    export_entry!(folder_enum_objects),
    export_entry!(folder_bind_to_object),
    export_entry!(folder_bind_to_storage),
    export_entry!(folder_compare_ids),
    export_entry!(folder_create_view_object),
    export_entry!(folder_get_attributes_of),
    export_entry!(folder_get_ui_object_of),
    export_entry!(folder_get_display_name_of),
    export_entry!(folder_set_name_of),
]);

static ITEM_TABLE: Table<8> = Table([
    export_entry!(item_query_interface),
    export_entry!(node_add_reference),
    export_entry!(node_release),
    export_entry!(item_bind_to_handler),
    export_entry!(item_get_parent),
    export_entry!(item_get_display_name),
    export_entry!(item_get_attributes),
    export_entry!(item_compare),
]);

static CHILDREN_TABLE: Table<7> = Table([
    export_entry!(children_query_interface),
    export_entry!(children_add_reference),
    export_entry!(children_release),
    export_entry!(children_next),
    export_entry!(children_skip),
    export_entry!(children_reset),
    export_entry!(children_clone),
]);

fn absolute_list_of_path(path: &[u8]) -> Result<*mut u8, HResult> {
    let is_folder = kind_of(path).ok_or(from_win32(super::error::FILE_NOT_FOUND))?;
    let mut list = [0u8; LIST_CAPACITY];
    let written = item_lists::write_absolute(path, is_folder, &mut list).ok_or(E_INVALIDARG)?;
    let made = new_list(&[without_terminator(&list[..written])]);
    match made.is_null() {
        true => Err(E_OUTOFMEMORY),
        false => Ok(made),
    }
}

fn absolute_list_of_windows(name: &[u16]) -> Result<*mut u8, HResult> {
    let desktop = [0u8, 0];
    parse_in_folder(&desktop, name).map(|(made, _)| made)
}

/// The absolute list of the layer's own folder or item object `object`, a copy.
///
/// # Safety
/// `object` must be null or a live COM object.
unsafe fn list_of_object(object: usize) -> Option<*mut u8> {
    if object == 0 {
        return None;
    }
    // SAFETY: the caller's duty: a COM object's first word is its table.
    let table = unsafe { (object as *const usize).read() };
    if table != FOLDER_TABLE.address() && table != ITEM_TABLE.address() {
        return None;
    }
    // SAFETY: one of the layer's nodes, live.
    let list = unsafe { node_list(object as *mut Node) };
    Some(new_list(&[without_terminator(list)]))
}

unsafe extern "efiapi" fn il_free(list: *mut u8) {
    // SAFETY: `ILFree`'s contract: null or a list the shell allocated.
    unsafe { heap::free(list) }
}

unsafe extern "efiapi" fn il_get_size(list: *const u8) -> u32 {
    // SAFETY: `ILGetSize`'s contract: null or a readable list.
    unsafe { list_bytes(list) }.map_or(0, |bytes| bytes.len() as u32)
}

unsafe extern "efiapi" fn il_clone(list: *const u8) -> *mut u8 {
    // SAFETY: `ILClone`'s contract: null or a readable list.
    match unsafe { list_bytes(list) } {
        Some(bytes) => new_list(&[without_terminator(bytes)]),
        None => core::ptr::null_mut(),
    }
}

unsafe extern "efiapi" fn il_clone_first(list: *const u8) -> *mut u8 {
    // SAFETY: `ILCloneFirst`'s contract: null or a readable list.
    let Some(bytes) = (unsafe { list_bytes(list) }) else {
        return core::ptr::null_mut();
    };
    let first = item_lists::offsets(bytes).next().map_or(0, |(_, size)| size);
    new_list(&[&bytes[..first]])
}

unsafe extern "efiapi" fn il_combine(first: *const u8, second: *const u8) -> *mut u8 {
    // SAFETY: `ILCombine`'s contract: null or readable lists.
    let (first, second) = unsafe { (list_bytes(first), list_bytes(second)) };
    match (first, second) {
        (None, None) => core::ptr::null_mut(),
        (first, second) => new_list(&[
            first.map_or(&[][..], without_terminator),
            second.map_or(&[][..], without_terminator),
        ]),
    }
}

unsafe extern "efiapi" fn il_find_last_id(list: *const u8) -> *const u8 {
    // SAFETY: `ILFindLastID`'s contract: null or a readable list.
    match unsafe { list_bytes(list) } {
        Some(bytes) => list.wrapping_add(item_lists::last_offset(bytes).unwrap_or(0)),
        None => core::ptr::null(),
    }
}

unsafe extern "efiapi" fn il_get_next(list: *const u8) -> *const u8 {
    // SAFETY: `ILGetNext`'s contract: null or a readable list.
    match unsafe { list_bytes(list) }.and_then(|bytes| item_lists::offsets(bytes).next()) {
        Some((_, size)) => list.wrapping_add(size),
        None => core::ptr::null(),
    }
}

unsafe extern "efiapi" fn il_remove_last_id(list: *mut u8) -> Bool {
    // SAFETY: `ILRemoveLastID`'s contract: null or a writable list.
    let Some(at) = (unsafe { list_bytes(list) }).and_then(item_lists::last_offset) else {
        return FALSE;
    };
    // SAFETY: the last item's size field lies inside the caller's list; zero ends it there.
    unsafe { list.add(at).cast::<[u8; 2]>().write_unaligned([0, 0]) };
    TRUE
}

unsafe extern "efiapi" fn il_is_equal(a: *const u8, b: *const u8) -> Bool {
    // SAFETY: `ILIsEqual`'s contract: readable lists.
    let (a, b) = unsafe { (list_bytes(a), list_bytes(b)) };
    Bool::from(a.is_some() && a == b)
}

unsafe extern "efiapi" fn il_is_parent(
    parent: *const u8,
    child: *const u8,
    immediate: Bool,
) -> Bool {
    // SAFETY: `ILIsParent`'s contract: readable lists.
    let (Some(parent), Some(child)) = (unsafe { list_bytes(parent) }, unsafe { list_bytes(child) })
    else {
        return FALSE;
    };
    let head = without_terminator(parent);
    let below = without_terminator(child);
    let Some(rest) = below.strip_prefix(head).filter(|rest| !rest.is_empty()) else {
        return FALSE;
    };
    let starts_an_item = item_lists::offsets(below).any(|(at, _)| at == head.len());
    let one_below = item_lists::offsets(rest).count() == 1;
    Bool::from(starts_an_item && (immediate == FALSE || one_below))
}

unsafe extern "efiapi" fn il_create_from_path_w(path: *const u16) -> *mut u8 {
    if path.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `ILCreateFromPathW`'s contract: a NUL-terminated path.
    let name = unsafe { super::file::wide_string(path) };
    absolute_list_of_windows(name).unwrap_or(core::ptr::null_mut())
}

unsafe extern "efiapi" fn sh_parse_display_name(
    name: *const u16,
    _context: usize,
    out: *mut *mut u8,
    wanted: u32,
    attributes: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable list pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    if name.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `SHParseDisplayName`'s contract: a NUL-terminated name.
    let name = unsafe { super::file::wide_string(name) };
    let made = match absolute_list_of_windows(name) {
        Ok(made) => made,
        Err(failure) => return failure,
    };
    if !attributes.is_null() {
        // SAFETY: the list just made, whole.
        let list = unsafe { list_bytes(made) }.unwrap_or(&[0, 0]);
        // SAFETY: the caller passes a writable mask.
        unsafe { attributes.write(item_attributes(list) & wanted) };
    }
    // SAFETY: as above.
    unsafe { out.write(made) };
    S_OK
}

fn windows_path_into(list: &[u8], out: &mut [u16]) -> Option<usize> {
    let mut path = [0u8; PATH_CAPACITY];
    let length = path_of(list, &mut path)?;
    let mut units = [0u16; PATH_CAPACITY];
    let written = windows_units(&path[..length], &mut units)?;
    let target = out.get_mut(..=written)?;
    target[..written].copy_from_slice(&units[..written]);
    target[written] = 0;
    Some(written)
}

unsafe extern "efiapi" fn sh_get_path_from_id_list_w(list: *const u8, out: *mut u16) -> Bool {
    if out.is_null() {
        return FALSE;
    }
    // SAFETY: `SHGetPathFromIDListW`'s contract: a buffer of `MAX_PATH` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(out, MAX_PATH) };
    out[0] = 0;
    // SAFETY: as above: a readable list.
    let found = unsafe { list_bytes(list) }.and_then(|bytes| windows_path_into(bytes, out));
    Bool::from(found.is_some())
}

unsafe extern "efiapi" fn sh_get_path_from_id_list_ex(
    list: *const u8,
    out: *mut u16,
    capacity: u32,
    _flags: u32,
) -> Bool {
    if out.is_null() || capacity == 0 {
        return FALSE;
    }
    // SAFETY: `SHGetPathFromIDListEx`'s contract: a buffer of `capacity` writable units.
    let out = unsafe { core::slice::from_raw_parts_mut(out, capacity as usize) };
    out[0] = 0;
    // SAFETY: as above: a readable list.
    let found = unsafe { list_bytes(list) }.and_then(|bytes| windows_path_into(bytes, out));
    Bool::from(found.is_some())
}

unsafe extern "efiapi" fn sh_get_desktop_folder(out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match new_folder(new_list(&[])) {
        Ok(folder) => {
            // SAFETY: the caller passes a writable `IShellFolder*`.
            unsafe { out.write(folder as usize) };
            S_OK
        }
        Err(failure) => {
            // SAFETY: as above.
            unsafe { out.write(0) };
            failure
        }
    }
}

unsafe extern "efiapi" fn sh_create_item_from_parsing_name(
    name: *const u16,
    _context: usize,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if name.is_null() {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return E_INVALIDARG;
    }
    // SAFETY: `SHCreateItemFromParsingName`'s contract: a NUL-terminated name.
    let name = unsafe { super::file::wide_string(name) };
    hand_out(absolute_list_of_windows(name).and_then(new_item), interface, out)
}

unsafe extern "efiapi" fn sh_create_item_from_id_list(
    list: *const u8,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `SHCreateItemFromIDList`'s contract: a readable absolute list.
    let made = match unsafe { list_bytes(list) } {
        Some(bytes) => new_list(&[without_terminator(bytes)]),
        None => {
            // SAFETY: the caller passes a writable interface pointer.
            unsafe { out.write(0) };
            return E_INVALIDARG;
        }
    };
    hand_out(new_item(made), interface, out)
}

unsafe extern "efiapi" fn sh_get_id_list_from_object(object: usize, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `SHGetIDListFromObject`'s contract: a live COM object, or null.
    let made = unsafe { list_of_object(object) };
    match made {
        Some(list) if !list.is_null() => {
            // SAFETY: the caller passes a writable list pointer.
            unsafe { out.write(list) };
            S_OK
        }
        Some(_) => {
            // SAFETY: as above.
            unsafe { out.write(core::ptr::null_mut()) };
            E_OUTOFMEMORY
        }
        None => {
            // SAFETY: as above.
            unsafe { out.write(core::ptr::null_mut()) };
            E_NOINTERFACE
        }
    }
}

unsafe extern "efiapi" fn sh_bind_to_parent(
    list: *const u8,
    interface: *const Guid,
    out: *mut usize,
    last: *mut *const u8,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if !last.is_null() {
        // SAFETY: the caller passes a writable list pointer, or null.
        unsafe { last.write(core::ptr::null()) };
    }
    // SAFETY: `SHBindToParent`'s contract: a readable absolute list.
    let Some((bytes, at)) = (unsafe { list_bytes(list) })
        .and_then(|bytes| Some((bytes, item_lists::last_offset(bytes)?)))
    else {
        // SAFETY: the caller passes a writable interface pointer.
        unsafe { out.write(0) };
        return E_INVALIDARG;
    };
    let answer = hand_out(new_folder(new_list(&[&bytes[..at]])), interface, out);
    if answer == S_OK && !last.is_null() {
        // SAFETY: as above; the last item lies inside the caller's list.
        unsafe { last.write(list.wrapping_add(at)) };
    }
    answer
}

unsafe extern "efiapi" fn sh_get_special_folder_location(
    _window: usize,
    csidl: u32,
    out: *mut *mut u8,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable list pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    let made = match csidl & 0xff {
        0 => Ok(new_list(&[])),
        _ => {
            let mut path = [0u8; PATH_CAPACITY];
            match super::shell_folders::special_folder(csidl, &mut path) {
                Some(length) => absolute_list_of_path(&path[..length]),
                None => Err(E_INVALIDARG),
            }
        }
    };
    match made {
        Ok(list) if !list.is_null() => {
            // SAFETY: as above.
            unsafe { out.write(list) };
            S_OK
        }
        Ok(_) => E_OUTOFMEMORY,
        Err(failure) => failure,
    }
}

unsafe extern "efiapi" fn sh_get_known_folder_id_list(
    id: *const [u8; 16],
    _flags: u32,
    _token: usize,
    out: *mut *mut u8,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable list pointer.
    unsafe { out.write(core::ptr::null_mut()) };
    if id.is_null() {
        return E_INVALIDARG;
    }
    let mut path = [0u8; PATH_CAPACITY];
    // SAFETY: `SHGetKnownFolderIDList`'s contract: a readable `KNOWNFOLDERID`.
    let id = unsafe { id.read_unaligned() };
    let Some(length) = super::shell_folders::known_folder(&id, &mut path) else {
        return E_INVALIDARG;
    };
    match absolute_list_of_path(&path[..length]) {
        Ok(list) => {
            // SAFETY: as above.
            unsafe { out.write(list) };
            S_OK
        }
        Err(failure) => failure,
    }
}

fn browse_folder_of(root: *const u8, out: &mut [u8]) -> Option<usize> {
    // SAFETY: `BROWSEINFOW.pidlRoot`'s contract: null or a readable absolute list.
    let bytes = unsafe { list_bytes(root) }?;
    let mut path = [0u8; PATH_CAPACITY];
    let length = path_of(bytes, &mut path)?;
    out.get_mut(..length)?.copy_from_slice(&path[..length]);
    Some(length)
}

unsafe fn browse_field<T: Copy>(request: *const u8, offset: usize) -> T {
    // SAFETY: the caller's duty: a readable `BROWSEINFOW`, whose field at `offset` is a `T`.
    unsafe { request.add(offset).cast::<T>().read_unaligned() }
}

unsafe extern "efiapi" fn sh_browse_for_folder_w(request: *const u8) -> *mut u8 {
    if request.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `SHBrowseForFolderW`'s contract: a readable `BROWSEINFOW`.
    let (owner, root, display, title) = unsafe {
        (
            browse_field::<usize>(request, BROWSE_OWNER),
            browse_field::<*const u8>(request, BROWSE_ROOT),
            browse_field::<*mut u16>(request, BROWSE_DISPLAY_NAME),
            browse_field::<*const u16>(request, BROWSE_TITLE),
        )
    };
    let mut arguments = pick_process::Arguments::new(b"folder");
    if !title.is_null() {
        // SAFETY: `lpszTitle` is a NUL-terminated string.
        let units = unsafe { super::file::wide_string(title) };
        let mut bytes = [0u8; TITLE_BYTES];
        let (length, _) = text::write_utf8(units, &mut bytes);
        if length > 0 {
            let _ = arguments.option(b"--title", &bytes[..length]);
        }
    }
    let mut start = [0u8; PATH_CAPACITY];
    if let Some(length) = browse_folder_of(root, &mut start) {
        let _ = arguments.option(b"--folder", &start[..length]);
    }
    let modal = super::file_picker::owner_root(owner);
    super::file_picker::set_modal(modal, true);
    let mut output = [0u8; PICK_OUTPUT_BYTES];
    let picked = pick_process::run(&arguments, &mut output);
    super::file_picker::set_modal(modal, false);
    let Ok(pick_process::Picked::Chosen(length)) = picked else {
        return core::ptr::null_mut();
    };
    let Some(path) = output[..length]
        .split(|&byte| byte == b'\n')
        .next()
        .filter(|line| line.starts_with(b"/") && line.len() < PATH_CAPACITY)
    else {
        return core::ptr::null_mut();
    };
    let Ok(made) = absolute_list_of_path(path) else {
        return core::ptr::null_mut();
    };
    if !display.is_null() {
        // SAFETY: the list just made, whole.
        let list = unsafe { list_bytes(made) }.unwrap_or(&[0, 0]);
        let mut units = [0u16; PATH_CAPACITY];
        let written = name_units(list, &mut units).min(MAX_PATH - 1);
        // SAFETY: `pszDisplayName` holds `MAX_PATH` writable units.
        unsafe {
            display.copy_from_nonoverlapping(units.as_ptr(), written);
            display.add(written).write(0);
        }
    }
    made
}

pub(super) const EXPORTS: &[Export] = exports![
    b"ILFree" => il_free,
    b"ILGetSize" => il_get_size,
    b"ILClone" => il_clone,
    b"ILCloneFirst" => il_clone_first,
    b"ILCombine" => il_combine,
    b"ILFindLastID" => il_find_last_id,
    b"ILGetNext" => il_get_next,
    b"ILRemoveLastID" => il_remove_last_id,
    b"ILIsEqual" => il_is_equal,
    b"ILIsParent" => il_is_parent,
    b"ILCreateFromPathW" => il_create_from_path_w,
    b"SHParseDisplayName" => sh_parse_display_name,
    b"SHGetPathFromIDListW" => sh_get_path_from_id_list_w,
    b"SHGetPathFromIDListEx" => sh_get_path_from_id_list_ex,
    b"SHGetDesktopFolder" => sh_get_desktop_folder,
    b"SHCreateItemFromParsingName" => sh_create_item_from_parsing_name,
    b"SHCreateItemFromIDList" => sh_create_item_from_id_list,
    b"SHGetIDListFromObject" => sh_get_id_list_from_object,
    b"SHBindToParent" => sh_bind_to_parent,
    b"SHGetSpecialFolderLocation" => sh_get_special_folder_location,
    b"SHGetKnownFolderIDList" => sh_get_known_folder_id_list,
    b"SHBrowseForFolderW" => sh_browse_for_folder_w,
];

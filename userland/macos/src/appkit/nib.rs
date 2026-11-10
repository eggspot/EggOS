//! Loading compiled nibs (D232): `NSApplicationMain`'s `NSMainNibFile` and
//! `-[NSBundle loadNibNamed:owner:topLevelObjects:]`. The file is read whole into the layer's
//! heap, indexed by `nib_archive`, and its objects are made through the classes' own
//! initializers and setters: windows from their templates, views and their subviews, text
//! fields and buttons configured from their cells, menus and their items, custom objects of
//! the program's own classes. Only the classes on the allow-list (and the program's own) are
//! made, every object once (a memo table), no deeper than `DEPTH`; then outlets and actions
//! are connected, `awakeFromNib` is sent, the main menu is set and the visible windows shown.

use super::{
    NSAPPLICATION, NSBUTTON, NSMENU, NSMENUITEM, NSTEXTFIELD, NSVIEW, NSWINDOW, application,
    responds, store, view, with_text,
};
use crate::foundation::nib_archive::{
    self, ClassEntry, Connection, NibArchive, NibValue, ObjectEntry, Span, Tables, ValueEntry,
};
use crate::foundation::{NSDATA, NSMUTABLEARRAY, NSMUTABLESET, NSNUMBER, class_pointer};
use crate::libsystem::heap;
use crate::linux;
use crate::objc::abi::{self, ClassObject, Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::selector::name_bytes;
use crate::objc::{class, class_named, message, root, sel, selector_named, send, send1, send2};
use core::sync::atomic::Ordering;
use eggos_macos_gui::geometry::Frame;

const DEPTH: usize = 64;
const LARGEST_NIB: usize = 16 << 20;
const READ_STEP: usize = 64 * 1024;
const LONGEST_PATH: usize = 4096;
const LONGEST_SELECTOR: usize = 256;
const BACKING_BUFFERED: u64 = 2;
const CELL_EDITABLE: i64 = 0x1000_0000;
const CELL_SELECTABLE: i64 = 0x0020_0000;
const CELL_BEZELED: i64 = 0x0040_0000;
const MAIN_MENU_NAME: &[u8] = b"_NSMainMenu";
const VIEW_CLASSES: [&[u8]; 4] = [b"NSView", b"NSControl", b"NSButton", b"NSTextField"];
const OBJECT_CLASSES: [&[u8]; 2] = [b"NSObject", b"NSApplication"];
const WINDOW_CLASSES: [&[u8]; 1] = [b"NSWindow"];

#[derive(Clone, Copy, Default)]
enum Slot {
    #[default]
    Unmade,
    Making,
    Made(Id),
}

struct FileBytes {
    pointer: *mut u8,
    length: usize,
}

impl FileBytes {
    fn as_slice(&self) -> &[u8] {
        if self.pointer.is_null() {
            return &[];
        }
        // SAFETY: the block holds `length` bytes the file filled.
        unsafe { core::slice::from_raw_parts(self.pointer, self.length) }
    }
}

impl Drop for FileBytes {
    fn drop(&mut self) {
        heap::release(self.pointer);
    }
}

fn read_file(path: &[u8]) -> Option<FileBytes> {
    let fd = linux::open(path, linux::O_RDONLY | linux::O_CLOEXEC, 0).ok()?;
    let mut file = FileBytes {
        pointer: core::ptr::null_mut(),
        length: 0,
    };
    let mut capacity = 0;
    let complete = loop {
        if file.length == capacity {
            if capacity >= LARGEST_NIB {
                break false;
            }
            let next = (capacity * 2).clamp(READ_STEP, LARGEST_NIB);
            let grown = heap::reallocate(file.pointer, next);
            if grown.is_null() {
                break false;
            }
            file.pointer = grown;
            capacity = next;
        }
        // SAFETY: the block holds `capacity` bytes; the first `length` are filled.
        let rest = unsafe {
            core::slice::from_raw_parts_mut(
                file.pointer.wrapping_add(file.length),
                capacity - file.length,
            )
        };
        match linux::read(fd, rest) {
            Ok(0) => break true,
            Ok(read) => file.length += read,
            Err(_) => break false,
        }
    };
    let _ = linux::close(fd);
    (complete && file.length > 0).then_some(file)
}

fn joined(parts: &[&[u8]], out: &mut [u8; LONGEST_PATH]) -> Option<usize> {
    let mut length = 0;
    for part in parts {
        let end = length + part.len();
        if end >= LONGEST_PATH {
            return None;
        }
        out[length..end].copy_from_slice(part);
        length = end;
    }
    out[length] = 0;
    Some(length)
}

fn read_nib(bundle: &[u8], name: &[u8]) -> Option<FileBytes> {
    let stem = nib_archive::nib_stem(name);
    let mut path = [0u8; LONGEST_PATH];
    for folder in nib_archive::NIB_FOLDERS {
        for suffix in nib_archive::NIB_SUFFIXES {
            let Some(length) = joined(&[bundle, folder, stem, suffix], &mut path) else {
                continue;
            };
            if let Some(file) = read_file(&path[..=length]) {
                return Some(file);
            }
        }
    }
    None
}

fn filled<T: Copy + Default>(count: usize) -> Option<Buffer<T>> {
    let mut buffer = Buffer::new();
    if !buffer.reserve(count) {
        return None;
    }
    for _ in 0..count {
        buffer.push(T::default());
    }
    Some(buffer)
}

fn is_program_class(class: *mut ClassObject) -> bool {
    class::record_of(class).is_some_and(|record| !record.has(class::BUILTIN))
}

fn descends(class: *mut ClassObject, ancestor: &'static ClassObject) -> bool {
    !class.is_null() && root::descends_from(class, class_pointer(ancestor))
}

fn allowed_class(name: &[u8], layer: &[&[u8]]) -> *mut ClassObject {
    let class = class_named(name);
    if class.is_null() || is_program_class(class) || layer.contains(&name) {
        return class;
    }
    core::ptr::null_mut()
}

fn retained_string(bytes: &[u8]) -> Id {
    objc_retain(super::string(core::str::from_utf8(bytes).unwrap_or("")))
}

fn new_of(class: *mut ClassObject) -> Id {
    send(send(class.cast(), sel!("alloc")), sel!("init"))
}

fn send_flag(receiver: Id, sel: Sel, flag: bool) {
    send1(receiver, sel, usize::from(flag));
}

fn send_string(receiver: Id, sel: Sel, bytes: &[u8]) {
    let text = super::string(core::str::from_utf8(bytes).unwrap_or(""));
    send1(receiver, sel, text as usize);
}

fn frame_from(text: Option<&[u8]>) -> Option<Frame> {
    let [x, y, width, height] = nib_archive::numbers::<4>(text?)?;
    Some(Frame {
        x,
        y,
        width,
        height,
    })
}

fn size_from(text: Option<&[u8]>) -> Option<Frame> {
    let [width, height] = nib_archive::numbers::<2>(text?)?;
    Some(Frame {
        x: 0.0,
        y: 0.0,
        width,
        height,
    })
}

fn selector_of(name: &[u8]) -> Option<Sel> {
    if name.is_empty() || name.len() >= LONGEST_SELECTOR || name.contains(&0) {
        return None;
    }
    selector_named(name)
}

fn outlet_ivar(object: Id, name: &[u8]) -> Option<usize> {
    let mut record = class::record_of(class::class_of(object));
    while let Some(current) = record {
        if !current.has(class::BUILTIN) {
            // SAFETY: a record's class object lives as long as the program.
            let read_only = unsafe { &*current.class }.read_only();
            if !read_only.is_null() {
                // SAFETY: a realized image class's `class_ro_t`, mapped.
                let list = unsafe { (*read_only).ivars };
                // SAFETY: that class's ivar list, mapped (or null).
                let ivars = unsafe { abi::ivars(list) };
                for ivar in ivars {
                    if ivar.offset.is_null() || ivar.name.is_null() || ivar.kind.is_null() {
                        continue;
                    }
                    // SAFETY: an ivar's name is a NUL-terminated string in the mapped image.
                    let ivar_name = unsafe { name_bytes(ivar.name) };
                    // SAFETY: so is its type encoding.
                    let kind = unsafe { name_bytes(ivar.kind) };
                    let named = ivar_name == name || ivar_name.strip_prefix(b"_") == Some(name);
                    if named && kind.first() == Some(&b'@') {
                        // SAFETY: the ivar's offset variable, in the image's data.
                        let offset = unsafe { &*ivar.offset }.load(Ordering::Relaxed);
                        return Some(offset as usize);
                    }
                }
            }
        }
        record = current.superclass();
    }
    None
}

fn set_outlet(source: Id, name: &[u8], destination: Id) {
    let mut setter = [0u8; LONGEST_SELECTOR];
    let length = name.len() + 4;
    if let (Some(first), true) = (name.first(), length < LONGEST_SELECTOR) {
        setter[..3].copy_from_slice(b"set");
        setter[3] = first.to_ascii_uppercase();
        setter[4..length].copy_from_slice(&name[1..]);
        setter[length] = b':';
        if let Some(sel) = selector_of(&setter[..=length])
            && responds(source, sel)
        {
            send1(source, sel, destination as usize);
            return;
        }
    }
    if let Some(offset) = outlet_ivar(source, name) {
        // SAFETY: the object's own strong-object ivar at its slid offset.
        let slot = unsafe { &mut *source.cast::<u8>().wrapping_add(offset).cast::<Id>() };
        store(slot, destination);
    }
}

/// Why a nib was not loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    NotFound,
    Unreadable,
    NoMemory,
}

struct Loader<'b, 't, 'm> {
    archive: NibArchive<'b, 't>,
    memo: &'m mut Buffer<Slot>,
    made: &'m mut Buffer<Id>,
    depth: usize,
    main_menu: Id,
}

impl Loader<'_, '_, '_> {
    fn object(&mut self, index: usize) -> Id {
        match self.memo.as_slice().get(index).copied() {
            Some(Slot::Unmade) if self.depth < DEPTH => {}
            Some(Slot::Made(object)) => return object,
            _ => return core::ptr::null_mut(),
        }
        self.memo.as_mut_slice()[index] = Slot::Making;
        self.depth += 1;
        let object = self.make(index);
        self.depth -= 1;
        self.memo.as_mut_slice()[index] = Slot::Made(object);
        if !object.is_null() && !self.made.push(object) {
            objc_release(object);
            self.memo.as_mut_slice()[index] = Slot::Made(core::ptr::null_mut());
            return core::ptr::null_mut();
        }
        object
    }

    fn reference(&mut self, index: usize, key: &[u8]) -> Id {
        match self.archive.reference(index, key) {
            Some(target) => self.object(target),
            None => core::ptr::null_mut(),
        }
    }

    fn make(&mut self, index: usize) -> Id {
        let Some(class) = self.archive.class_name(index) else {
            return core::ptr::null_mut();
        };
        match class {
            b"NSWindowTemplate" => self.window(index),
            b"NSCustomObject" => self.custom_object(index),
            b"NSMenu" => self.menu(index),
            b"NSMenuItem" => self.menu_item(index),
            b"NSString" | b"NSMutableString" => self
                .archive
                .string_bytes(index)
                .map_or(core::ptr::null_mut(), retained_string),
            b"NSArray" | b"NSMutableArray" => self.collection(index, &NSMUTABLEARRAY),
            b"NSSet" | b"NSMutableSet" => self.collection(index, &NSMUTABLESET),
            b"NSNumber" => match self.archive.integer(index, b"NS.intval") {
                Some(value) => objc_retain(send1(
                    class_pointer(&NSNUMBER).cast(),
                    sel!("numberWithLongLong:"),
                    value as usize,
                )),
                None => core::ptr::null_mut(),
            },
            b"NSData" | b"NSMutableData" => match self.archive.field(index, b"NS.bytes") {
                Some(NibValue::Data(bytes)) => objc_retain(send2(
                    class_pointer(&NSDATA).cast(),
                    sel!("dataWithBytes:length:"),
                    bytes.as_ptr() as usize,
                    bytes.len(),
                )),
                _ => core::ptr::null_mut(),
            },
            _ => self.view(index),
        }
    }

    fn collection(&mut self, index: usize, class: &'static ClassObject) -> Id {
        let collection = new_of(class_pointer(class));
        if collection.is_null() {
            return collection;
        }
        let archive = self.archive;
        for member in archive.members(index) {
            if let NibValue::Object(target) = member {
                let object = self.object(target);
                if !object.is_null() {
                    send1(collection, sel!("addObject:"), object as usize);
                }
            }
        }
        collection
    }

    fn custom_object(&mut self, index: usize) -> Id {
        let Some(name) = self.archive.string(index, b"NSClassName") else {
            return core::ptr::null_mut();
        };
        let class = allowed_class(name, &OBJECT_CLASSES);
        if descends(class, &NSAPPLICATION) {
            let shared = application::application();
            return match shared.is_null() {
                true => objc_retain(send(class.cast(), sel!("sharedApplication"))),
                false => objc_retain(shared),
            };
        }
        if class.is_null() || descends(class, &NSVIEW) || descends(class, &NSWINDOW) {
            return core::ptr::null_mut();
        }
        new_of(class)
    }

    fn view_class(&self, index: usize) -> *mut ClassObject {
        if self.archive.class_name(index) == Some(&b"NSCustomView"[..]) {
            let class = self
                .archive
                .string(index, b"NSClassName")
                .map_or(core::ptr::null_mut(), |name| {
                    allowed_class(name, &VIEW_CLASSES)
                });
            return match descends(class, &NSVIEW) {
                true => class,
                false => core::ptr::null_mut(),
            };
        }
        self.archive
            .class_names(index)
            .map(|name| allowed_class(name, &VIEW_CLASSES))
            .find(|&class| descends(class, &NSVIEW))
            .unwrap_or(core::ptr::null_mut())
    }

    fn view(&mut self, index: usize) -> Id {
        let class = self.view_class(index);
        if class.is_null() {
            return core::ptr::null_mut();
        }
        let frame = frame_from(self.archive.string(index, b"NSFrame"))
            .or_else(|| size_from(self.archive.string(index, b"NSFrameSize")))
            .unwrap_or_default();
        let made = view::new_with_frame_of(class.cast(), frame);
        if made.is_null() {
            return made;
        }
        if descends(class, &NSTEXTFIELD) {
            self.configure_text_field(index, made);
        } else if descends(class, &NSBUTTON) {
            self.configure_button(index, made);
        }
        if let Some(NibValue::Boolean(enabled)) = self.archive.field(index, b"NSEnabled")
            && responds(made, sel!("setEnabled:"))
        {
            send_flag(made, sel!("setEnabled:"), enabled);
        }
        let archive = self.archive;
        if let Some(subviews) = archive.reference(index, b"NSSubviews") {
            for member in archive.members(subviews) {
                if let NibValue::Object(target) = member {
                    let subview = self.object(target);
                    if !subview.is_null() && root::is_kind_of(subview, &NSVIEW) {
                        send1(made, sel!("addSubview:"), subview as usize);
                    }
                }
            }
        }
        made
    }

    fn configure_text_field(&mut self, index: usize, field: Id) {
        let Some(cell) = self.archive.reference(index, b"NSCell") else {
            return;
        };
        let flags = self.archive.integer(cell, b"NSCellFlags").unwrap_or(0);
        send_flag(field, sel!("setEditable:"), flags & CELL_EDITABLE != 0);
        send_flag(field, sel!("setSelectable:"), flags & CELL_SELECTABLE != 0);
        send_flag(field, sel!("setBezeled:"), flags & CELL_BEZELED != 0);
        let draws = self.archive.field(cell, b"NSDrawsBackground") == Some(NibValue::Boolean(true));
        send_flag(field, sel!("setDrawsBackground:"), draws);
        if let Some(text) = self.archive.string(cell, b"NSContents") {
            send_string(field, sel!("setStringValue:"), text);
        }
        if let Some(placeholder) = self.archive.string(cell, b"NSPlaceholderString") {
            send_string(field, sel!("setPlaceholderString:"), placeholder);
        }
    }

    fn configure_button(&mut self, index: usize, button: Id) {
        let Some(cell) = self.archive.reference(index, b"NSCell") else {
            return;
        };
        if let Some(title) = self.archive.string(cell, b"NSContents") {
            send_string(button, sel!("setTitle:"), title);
        }
        if let Some(key) = self.archive.string(cell, b"NSKeyEquivalent") {
            send_string(button, sel!("setKeyEquivalent:"), key);
        }
        if let Some(style) = self.archive.integer(cell, b"NSBezelStyle") {
            send1(button, sel!("setBezelStyle:"), style as usize);
        }
    }

    fn window(&mut self, index: usize) -> Id {
        let class = self
            .archive
            .string(index, b"NSWindowClass")
            .map_or(core::ptr::null_mut(), |name| {
                allowed_class(name, &WINDOW_CLASSES)
            });
        let class = match descends(class, &NSWINDOW) {
            true => class,
            false => class_pointer(&NSWINDOW),
        };
        let frame = frame_from(self.archive.string(index, b"NSWindowRect")).unwrap_or_default();
        let style = self
            .archive
            .integer(index, b"NSWindowStyleMask")
            .unwrap_or(0) as u64;
        let allocated = send(class.cast(), sel!("alloc"));
        if allocated.is_null() {
            return allocated;
        }
        let init: extern "C" fn(Id, Sel, Frame, u64, u64, u8) -> Id = message::msg_send_as();
        let window = init(
            allocated,
            sel!("initWithContentRect:styleMask:backing:defer:"),
            frame,
            style,
            BACKING_BUFFERED,
            0,
        );
        if window.is_null() {
            return window;
        }
        if let Some(title) = self.archive.string(index, b"NSWindowTitle") {
            send_string(window, sel!("setTitle:"), title);
        }
        let content = self.reference(index, b"NSWindowView");
        if !content.is_null() && root::is_kind_of(content, &NSVIEW) {
            send1(window, sel!("setContentView:"), content as usize);
        }
        window
    }

    fn menu(&mut self, index: usize) -> Id {
        let title = self.archive.string(index, b"NSTitle").unwrap_or(b"");
        let title = super::string(core::str::from_utf8(title).unwrap_or(""));
        let allocated = send(class_pointer(&NSMENU).cast(), sel!("alloc"));
        let menu = send1(allocated, sel!("initWithTitle:"), title as usize);
        if menu.is_null() {
            return menu;
        }
        if self.archive.string(index, b"NSName") == Some(MAIN_MENU_NAME) {
            self.main_menu = menu;
        }
        let archive = self.archive;
        if let Some(items) = archive.reference(index, b"NSMenuItems") {
            for member in archive.members(items) {
                if let NibValue::Object(target) = member {
                    let item = self.object(target);
                    if !item.is_null() && root::is_kind_of(item, &NSMENUITEM) {
                        send1(menu, sel!("addItem:"), item as usize);
                    }
                }
            }
        }
        menu
    }

    fn menu_item(&mut self, index: usize) -> Id {
        let class = class_pointer(&NSMENUITEM).cast();
        if self.archive.field(index, b"NSIsSeparator") == Some(NibValue::Boolean(true)) {
            return objc_retain(send(class, sel!("separatorItem")));
        }
        let title = self.archive.string(index, b"NSTitle").unwrap_or(b"");
        let key = self.archive.string(index, b"NSKeyEquiv").unwrap_or(b"");
        let init: extern "C" fn(Id, Sel, Id, Sel, Id) -> Id = message::msg_send_as();
        let item = init(
            send(class, sel!("alloc")),
            sel!("initWithTitle:action:keyEquivalent:"),
            super::string(core::str::from_utf8(title).unwrap_or("")),
            core::ptr::null(),
            super::string(core::str::from_utf8(key).unwrap_or("")),
        );
        if item.is_null() {
            return item;
        }
        if let Some(mask) = self.archive.integer(index, b"NSKeyEquivModMask") {
            send1(item, sel!("setKeyEquivalentModifierMask:"), mask as usize);
        }
        if let Some(tag) = self.archive.integer(index, b"NSTag") {
            send1(item, sel!("setTag:"), tag as usize);
        }
        if self.archive.field(index, b"NSIsDisabled") == Some(NibValue::Boolean(true)) {
            send_flag(item, sel!("setEnabled:"), false);
        }
        let submenu = self.reference(index, b"NSSubmenu");
        if !submenu.is_null() && root::is_kind_of(submenu, &NSMENU) {
            send1(item, sel!("setSubmenu:"), submenu as usize);
            return item;
        }
        if let Some(action) = self
            .archive
            .string(index, b"NSAction")
            .and_then(selector_of)
        {
            send1(item, sel!("setAction:"), action as usize);
            let target = self.reference(index, b"NSTarget");
            send1(item, sel!("setTarget:"), target as usize);
        }
        item
    }

    fn connect(&mut self, connections: usize) {
        let archive = self.archive;
        for member in archive.members(connections) {
            let NibValue::Object(connector) = member else {
                continue;
            };
            match archive.connection(connector) {
                Some(Connection::Outlet {
                    source,
                    destination,
                    name,
                }) => {
                    let source = self.object(source);
                    let destination = self.object(destination);
                    if !source.is_null() {
                        set_outlet(source, name, destination);
                    }
                }
                Some(Connection::Action {
                    source,
                    destination,
                    action,
                }) => {
                    let source = self.object(source);
                    let target = destination.map_or(core::ptr::null_mut(), |at| self.object(at));
                    if let Some(action) = selector_of(action)
                        && responds(source, sel!("setAction:"))
                    {
                        send1(source, sel!("setTarget:"), target as usize);
                        send1(source, sel!("setAction:"), action as usize);
                    }
                }
                None => {}
            }
        }
    }
}

/// What a loaded nib leaves: its top-level objects (counted once each, for the caller) and its
/// main menu, if it has one.
#[derive(Debug)]
pub struct Loaded {
    pub top_level: Buffer<Id>,
    pub main_menu: Id,
}

struct Storage {
    objects: Buffer<ObjectEntry>,
    keys: Buffer<Span>,
    values: Buffer<ValueEntry>,
    classes: Buffer<ClassEntry>,
    memo: Buffer<Slot>,
    made: Buffer<Id>,
}

impl Drop for Storage {
    fn drop(&mut self) {
        self.objects.free();
        self.keys.free();
        self.values.free();
        self.classes.free();
        self.memo.free();
        self.made.free();
    }
}

impl Storage {
    fn new(counts: nib_archive::Counts) -> Option<Self> {
        let mut storage = Self {
            objects: Buffer::new(),
            keys: Buffer::new(),
            values: Buffer::new(),
            classes: Buffer::new(),
            memo: Buffer::new(),
            made: Buffer::new(),
        };
        storage.objects = filled(counts.objects)?;
        storage.keys = filled(counts.keys)?;
        storage.values = filled(counts.values)?;
        storage.classes = filled(counts.classes)?;
        storage.memo = filled(counts.objects)?;
        Some(storage)
    }
}

/// Makes the objects of the nib `bytes` with `owner` as File's Owner, connects them, sends
/// `awakeFromNib` and shows the windows the nib marks visible.
pub fn load(bytes: &[u8], owner: Id) -> Result<Loaded, LoadError> {
    let counts = nib_archive::counts(bytes).map_err(|_| LoadError::Unreadable)?;
    let mut storage = Storage::new(counts).ok_or(LoadError::NoMemory)?;
    let Storage {
        objects,
        keys,
        values,
        classes,
        memo,
        made,
    } = &mut storage;
    let tables = Tables {
        objects: objects.as_mut_slice(),
        keys: keys.as_mut_slice(),
        values: values.as_mut_slice(),
        classes: classes.as_mut_slice(),
    };
    let archive = NibArchive::index(bytes, tables).map_err(|_| LoadError::Unreadable)?;
    let data = archive.object_data().ok_or(LoadError::Unreadable)?;
    if let Some(slot) = memo.as_mut_slice().get_mut(data.root) {
        *slot = Slot::Made(owner);
    }
    let mut loader = Loader {
        archive,
        memo,
        made,
        depth: 0,
        main_menu: core::ptr::null_mut(),
    };
    let mut top_level = Buffer::new();
    for child in archive.top_level_objects(&data) {
        let object = loader.object(child);
        if !object.is_null() && top_level.push(object) {
            objc_retain(object);
        }
    }
    if let Some(connections) = data.connections {
        loader.connect(connections);
    }
    for &object in loader.made.as_slice() {
        if object != owner {
            send(object, sel!("awakeFromNib"));
        }
    }
    if !owner.is_null() {
        send(owner, sel!("awakeFromNib"));
    }
    for member in data
        .visible_windows
        .into_iter()
        .flat_map(|visible| archive.members(visible))
    {
        if let NibValue::Object(target) = member {
            let window = loader.object(target);
            if !window.is_null() && root::is_kind_of(window, &NSWINDOW) {
                send1(window, sel!("makeKeyAndOrderFront:"), 0);
            }
        }
    }
    let main_menu = loader.main_menu;
    for &object in loader.made.as_slice() {
        objc_release(object);
    }
    Ok(Loaded {
        top_level,
        main_menu,
    })
}

fn bundle_path(bundle: Id, out: &mut [u8; LONGEST_PATH]) -> Option<usize> {
    let path = send(bundle, sel!("bundlePath"));
    with_text(path, |text| joined(&[text.as_bytes()], out))
}

/// Reads and loads the nib `name` (without or with `.nib`) from `bundle`'s resources with
/// `owner` as File's Owner.
pub fn load_from_bundle(bundle: Id, name: &[u8], owner: Id) -> Result<Loaded, LoadError> {
    let mut path = [0u8; LONGEST_PATH];
    let length = bundle_path(bundle, &mut path).ok_or(LoadError::NotFound)?;
    let file = read_nib(&path[..length], name).ok_or(LoadError::NotFound)?;
    load(file.as_slice(), owner)
}

/// Loads the app's main nib `name` with the application as File's Owner and sets its main
/// menu; the nib's top-level objects stay alive as long as the app.
pub fn load_main(name: &[u8], application: Id) -> Result<(), LoadError> {
    let bundle = send(
        class_pointer(&crate::foundation::NSBUNDLE).cast(),
        sel!("mainBundle"),
    );
    let mut loaded = load_from_bundle(bundle, name, application)?;
    if !loaded.main_menu.is_null() {
        send1(application, sel!("setMainMenu:"), loaded.main_menu as usize);
    }
    loaded.top_level.free();
    Ok(())
}

extern "C" fn load_nib_named(this: Id, _: Sel, name: Id, owner: Id, top_level: *mut Id) -> u8 {
    let loaded = with_text(name, |text| load_from_bundle(this, text.as_bytes(), owner));
    let Ok(mut loaded) = loaded else {
        return 0;
    };
    if !loaded.main_menu.is_null() && owner == application::application() {
        send1(owner, sel!("setMainMenu:"), loaded.main_menu as usize);
    }
    if !top_level.is_null() {
        let array = send(class_pointer(&NSMUTABLEARRAY).cast(), sel!("array"));
        for &object in loaded.top_level.as_slice() {
            send1(array, sel!("addObject:"), object as usize);
            objc_release(object);
        }
        // SAFETY: the caller's out-parameter for the top-level objects.
        unsafe { top_level.write(array) };
    }
    loaded.top_level.free();
    1
}

extern "C" fn load_nib_named_owner(_: Id, _: Sel, name: Id, owner: Id) -> u8 {
    let bundle = send(
        class_pointer(&crate::foundation::NSBUNDLE).cast(),
        sel!("mainBundle"),
    );
    let sel = sel!("loadNibNamed:owner:topLevelObjects:");
    load_nib_named(bundle, sel, name, owner, core::ptr::null_mut())
}

extern "C" fn awake_from_nib(_: Id, _: Sel) {}

/// Adds AppKit's nib loading to NSBundle and `awakeFromNib` to NSObject.
pub fn extend_bundle(runtime: &mut crate::objc::Runtime) -> bool {
    let instance = crate::objc::methods!["loadNibNamed:owner:topLevelObjects:" => load_nib_named];
    let class_methods = crate::objc::methods!["loadNibNamed:owner:" => load_nib_named_owner];
    let awaking = crate::objc::methods!["awakeFromNib" => awake_from_nib];
    class::extend(
        runtime,
        &crate::foundation::NSBUNDLE,
        &instance,
        &class_methods,
    ) && class::extend(runtime, &root::NSOBJECT, &awaking, &[])
}

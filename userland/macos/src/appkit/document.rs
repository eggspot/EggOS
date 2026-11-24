//! Document-based apps (D750): `NSDocument` (its file URL and type, reading through
//! `readFromData:ofType:error:` or `readFromURL:ofType:error:`, writing through
//! `dataOfType:error:` or `writeToURL:ofType:error:`, its change count, its window controllers
//! and the save, save-as, revert and close commands with the unsaved-changes alert) and
//! `NSDocumentController` (the shared controller: the document types of the app's
//! `Info.plist`, untitled and opened documents, the File menu's New and Open commands, recent
//! documents, and the untitled document a document-based app opens at launch). Documents and
//! their window controllers join the responder chain after the window and its delegate.

use super::{
    NSALERT, NSDOCUMENT, NSDOCUMENT_META, NSDOCUMENTCONTROLLER, NSDOCUMENTCONTROLLER_META,
    NSOPENPANEL, NSSAVEPANEL, NSWINDOWCONTROLLER, responds, store, with_text,
};
use crate::foundation::{NSBUNDLE, NSMUTABLEARRAY, autoreleased, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, message, methods, sel, send, send_bool, send1, send2};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

const CHANGE_DONE: usize = 0;
const CHANGE_UNDONE: usize = 1;
const CHANGE_CLEARED: usize = 2;
const CHANGE_REDONE: usize = 5;
const CHANGE_DISCARDABLE: usize = 256;
const SAVE_OPERATION: usize = 0;
const SAVE_AS_OPERATION: usize = 1;
const SAVE_TO_OPERATION: usize = 2;
const MODAL_OK: isize = 1;
const ALERT_SAVE: isize = 1000;
const ALERT_DONT_SAVE: isize = 1001;
const MOST_RECENT_DOCUMENTS: usize = 10;
const UNTITLED: &str = "Untitled";
const KNOWN_TYPES: [(&str, &[&str]); 14] = [
    ("public.plain-text", &["txt", "text"]),
    ("public.utf8-plain-text", &["txt", "text"]),
    ("public.text", &["txt", "text"]),
    ("public.rtf", &["rtf"]),
    ("public.html", &["html", "htm"]),
    ("public.xml", &["xml"]),
    ("public.json", &["json"]),
    ("public.comma-separated-values-text", &["csv"]),
    ("public.png", &["png"]),
    ("public.jpeg", &["jpg", "jpeg"]),
    ("public.tiff", &["tif", "tiff"]),
    ("com.adobe.pdf", &["pdf"]),
    ("net.daringfireball.markdown", &["md", "markdown"]),
    ("com.apple.property-list", &["plist"]),
];

static SHARED: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Debug)]
struct DocumentFields {
    isa: usize,
    file_url: Id,
    file_type: Id,
    window_controllers: Id,
    change_count: isize,
    untitled_number: usize,
}

#[repr(C)]
#[derive(Debug)]
struct ControllerFields {
    isa: usize,
    documents: Id,
    recent: Id,
    untitled_count: usize,
}

fn document(object: Id) -> &'static mut DocumentFields {
    // SAFETY: the callers pass NSDocument instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<DocumentFields>() }
}

fn controller(object: Id) -> &'static mut ControllerFields {
    // SAFETY: the callers pass NSDocumentController instances.
    unsafe { &mut *object.cast::<ControllerFields>() }
}

fn mutable_array() -> Id {
    send(new_instance(class_pointer(&NSMUTABLEARRAY)), sel!("init"))
}

fn list_of(slot: &mut Id) -> Id {
    if slot.is_null() {
        *slot = mutable_array();
    }
    *slot
}

fn items_of(list: Id) -> Vec<Id> {
    if list.is_null() {
        return Vec::new();
    }
    let count = send(list, sel!("count")) as usize;
    (0..count)
        .map(|index| send1(list, sel!("objectAtIndex:"), index))
        .collect()
}

fn text_of(object: Id) -> Option<String> {
    match object.is_null() {
        true => None,
        false => Some(with_text(object, String::from)),
    }
}

fn value(dictionary: Id, key: &str) -> Id {
    if dictionary.is_null() {
        return dictionary;
    }
    send1(
        dictionary,
        sel!("objectForKey:"),
        super::string(key) as usize,
    )
}

fn info_value(key: &str) -> Id {
    let bundle = send(class_pointer(&NSBUNDLE).cast(), sel!("mainBundle"));
    if bundle.is_null() {
        return bundle;
    }
    send1(
        bundle,
        sel!("objectForInfoDictionaryKey:"),
        super::string(key) as usize,
    )
}

fn texts_of(list: Id) -> Vec<String> {
    items_of(list).into_iter().filter_map(text_of).collect()
}

fn same_text(first: Id, second: Id) -> bool {
    !first.is_null() && !second.is_null() && send_bool(first, sel!("isEqual:"), second as usize)
}

fn invoke_block(block: Id) -> usize {
    crate::objc::block::invoke_address(block)
}

struct DocumentType {
    identifier: String,
    name: String,
    class_name: Option<String>,
    extensions: Vec<String>,
    editor: bool,
}

fn declared_extensions(identifier: &str) -> Vec<String> {
    let declared = ["UTExportedTypeDeclarations", "UTImportedTypeDeclarations"]
        .into_iter()
        .flat_map(|key| items_of(info_value(key)))
        .find(|&entry| text_of(value(entry, "UTTypeIdentifier")).as_deref() == Some(identifier));
    if let Some(entry) = declared {
        let tags = value(entry, "UTTypeTagSpecification");
        let extensions = value(tags, "public.filename-extension");
        let listed = match root::is_kind_of(extensions, &crate::foundation::NSSTRING) {
            true => text_of(extensions).into_iter().collect(),
            false => texts_of(extensions),
        };
        if !listed.is_empty() {
            return listed;
        }
    }
    KNOWN_TYPES
        .iter()
        .find(|(known, _)| *known == identifier)
        .map(|(_, extensions)| extensions.iter().map(|&text| String::from(text)).collect())
        .unwrap_or_default()
}

fn document_types() -> Vec<DocumentType> {
    items_of(info_value("CFBundleDocumentTypes"))
        .into_iter()
        .filter_map(|entry| {
            let content_types = texts_of(value(entry, "LSItemContentTypes"));
            let name = text_of(value(entry, "CFBundleTypeName"));
            let identifier = content_types.first().cloned().or_else(|| name.clone())?;
            let mut extensions = texts_of(value(entry, "CFBundleTypeExtensions"));
            for content_type in &content_types {
                extensions.extend(declared_extensions(content_type));
            }
            let role = text_of(value(entry, "CFBundleTypeRole"));
            Some(DocumentType {
                name: name.unwrap_or_else(|| identifier.clone()),
                identifier,
                class_name: text_of(value(entry, "NSDocumentClass")),
                extensions,
                editor: role.as_deref() != Some("Viewer"),
            })
        })
        .collect()
}

fn type_named(identifier: &str) -> Option<DocumentType> {
    document_types()
        .into_iter()
        .find(|known| known.identifier == identifier || known.name == identifier)
}

fn type_for_extension(extension: &str) -> Option<DocumentType> {
    document_types().into_iter().find(|known| {
        known
            .extensions
            .iter()
            .any(|listed| listed == "*" || listed.eq_ignore_ascii_case(extension))
    })
}

fn class_for_type(identifier: Id) -> Id {
    let named = text_of(identifier)
        .and_then(|text| type_named(&text))
        .and_then(|known| known.class_name);
    let class = named.map_or(core::ptr::null_mut(), |name| {
        crate::objc::class_named(name.as_bytes())
    });
    class.cast()
}

fn types_of_class(class: Id, writable: bool) -> Id {
    let list = mutable_array();
    for known in document_types() {
        let matches = known
            .class_name
            .as_deref()
            .is_some_and(|name| crate::objc::class_named(name.as_bytes()) == class.cast());
        if matches && (!writable || known.editor) {
            send1(
                list,
                sel!("addObject:"),
                super::string(&known.identifier) as usize,
            );
        }
    }
    autoreleased(list)
}

fn extensions_for(identifier: Id) -> Id {
    let list = mutable_array();
    let known = text_of(identifier).and_then(|text| type_named(&text));
    for extension in known.map(|known| known.extensions).unwrap_or_default() {
        if extension != "*" {
            send1(list, sel!("addObject:"), super::string(&extension) as usize);
        }
    }
    autoreleased(list)
}

fn write_error(error: *mut Id) {
    if !error.is_null() {
        // SAFETY: the program passes room for an error object or null, checked above.
        unsafe { error.write(core::ptr::null_mut()) };
    }
}

extern "C" fn document_init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn init_with_type(this: Id, _: Sel, kind: Id, _error: *mut Id) -> Id {
    let made = send(this, sel!("init"));
    if !made.is_null() {
        store(&mut document(made).file_type, kind);
    }
    made
}

extern "C" fn init_with_contents(this: Id, _: Sel, url: Id, kind: Id, error: *mut Id) -> Id {
    init_for_url(this, sel!(""), url, url, kind, error)
}

extern "C" fn init_for_url(
    this: Id,
    _: Sel,
    url: Id,
    contents: Id,
    kind: Id,
    error: *mut Id,
) -> Id {
    let made = send(this, sel!("init"));
    if made.is_null() {
        return made;
    }
    let read: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> u8 = message::msg_send_as();
    if read(made, sel!("readFromURL:ofType:error:"), contents, kind, error) == 0 {
        objc_release(made);
        return core::ptr::null_mut();
    }
    store(&mut document(made).file_url, url);
    store(&mut document(made).file_type, kind);
    made
}

extern "C" fn document_dealloc(this: Id, _: Sel) {
    let fields = document(this);
    for window_controller in items_of(fields.window_controllers) {
        send1(window_controller, sel!("setDocument:"), 0);
    }
    for slot in [
        &mut fields.file_url,
        &mut fields.file_type,
        &mut fields.window_controllers,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn file_url(this: Id, _: Sel) -> Id {
    document(this).file_url
}

extern "C" fn set_file_url(this: Id, _: Sel, url: Id) {
    store(&mut document(this).file_url, url);
    synchronize_titles(this);
}

extern "C" fn file_type(this: Id, _: Sel) -> Id {
    document(this).file_type
}

extern "C" fn set_file_type(this: Id, _: Sel, kind: Id) {
    store(&mut document(this).file_type, kind);
}

extern "C" fn display_name(this: Id, _: Sel) -> Id {
    let fields = document(this);
    if !fields.file_url.is_null() {
        let name = send(fields.file_url, sel!("lastPathComponent"));
        if !name.is_null() {
            return name;
        }
    }
    let text = match fields.untitled_number {
        0 | 1 => String::from(UNTITLED),
        number => alloc::format!("{UNTITLED} {number}"),
    };
    super::string(&text)
}

extern "C" fn set_display_name(_: Id, _: Sel, _: Id) {}

extern "C" fn window_controllers(this: Id, _: Sel) -> Id {
    list_of(&mut document(this).window_controllers)
}

extern "C" fn add_window_controller(this: Id, _: Sel, window_controller: Id) {
    if window_controller.is_null() {
        return;
    }
    let list = list_of(&mut document(this).window_controllers);
    if !send_bool(list, sel!("containsObject:"), window_controller as usize) {
        send1(list, sel!("addObject:"), window_controller as usize);
    }
    if send(window_controller, sel!("document")) != this {
        send1(window_controller, sel!("setDocument:"), this as usize);
    }
}

extern "C" fn remove_window_controller(this: Id, _: Sel, window_controller: Id) {
    let list = document(this).window_controllers;
    if list.is_null() || window_controller.is_null() {
        return;
    }
    objc_retain(window_controller);
    send1(list, sel!("removeObject:"), window_controller as usize);
    if send(window_controller, sel!("document")) == this {
        send1(window_controller, sel!("setDocument:"), 0);
    }
    objc_release(window_controller);
}

extern "C" fn make_window_controllers(this: Id, _: Sel) {
    let name = send(this, sel!("windowNibName"));
    if name.is_null() {
        return;
    }
    let made = new_instance(class_pointer(&NSWINDOWCONTROLLER));
    let made = send2(
        made,
        sel!("initWithWindowNibName:owner:"),
        name as usize,
        this as usize,
    );
    if !made.is_null() {
        add_window_controller(this, sel!(""), made);
        objc_release(made);
    }
}

extern "C" fn nil_answer(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn show_windows(this: Id, _: Sel) {
    for window_controller in items_of(document(this).window_controllers) {
        send1(window_controller, sel!("showWindow:"), this as usize);
    }
}

extern "C" fn set_window(this: Id, _: Sel, window: Id) {
    if let Some(&last) = items_of(document(this).window_controllers).last() {
        send1(last, sel!("setWindow:"), window as usize);
    }
}

extern "C" fn window_for_sheet(this: Id, _: Sel) -> Id {
    items_of(document(this).window_controllers)
        .into_iter()
        .map(|window_controller| send(window_controller, sel!("window")))
        .find(|window| !window.is_null())
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn is_edited(this: Id, _: Sel) -> u8 {
    u8::from(document(this).change_count != 0)
}

extern "C" fn update_change_count(this: Id, _: Sel, change: usize) {
    let fields = document(this);
    match change & !CHANGE_DISCARDABLE {
        CHANGE_DONE | CHANGE_REDONE => fields.change_count += 1,
        CHANGE_UNDONE => fields.change_count -= 1,
        CHANGE_CLEARED => fields.change_count = 0,
        _ => {}
    }
    let edited = usize::from(fields.change_count != 0);
    for window_controller in items_of(fields.window_controllers) {
        let window = send(window_controller, sel!("window"));
        if responds(window, sel!("setDocumentEdited:")) {
            send1(window, sel!("setDocumentEdited:"), edited);
        }
    }
}

extern "C" fn read_from_data(_: Id, _: Sel, _: Id, _: Id, error: *mut Id) -> u8 {
    write_error(error);
    0
}

extern "C" fn read_from_url(this: Id, _: Sel, url: Id, kind: Id, error: *mut Id) -> u8 {
    let data = send1(
        class_pointer(&crate::foundation::NSDATA).cast(),
        sel!("dataWithContentsOfURL:"),
        url as usize,
    );
    if data.is_null() {
        write_error(error);
        return 0;
    }
    let read: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> u8 = message::msg_send_as();
    read(this, sel!("readFromData:ofType:error:"), data, kind, error)
}

extern "C" fn data_of_type(_: Id, _: Sel, _: Id, error: *mut Id) -> Id {
    write_error(error);
    core::ptr::null_mut()
}

extern "C" fn write_to_url(this: Id, _: Sel, url: Id, kind: Id, error: *mut Id) -> u8 {
    let data_of: extern "C" fn(Id, Sel, Id, *mut Id) -> Id = message::msg_send_as();
    let data = data_of(this, sel!("dataOfType:error:"), kind, error);
    if data.is_null() {
        return 0;
    }
    let write: extern "C" fn(Id, Sel, Id, u8) -> u8 = message::msg_send_as();
    let written = write(data, sel!("writeToURL:atomically:"), url, 1);
    if written == 0 {
        write_error(error);
    }
    written
}

fn save_to(this: Id, url: Id, kind: Id, operation: usize) -> bool {
    let kind = match kind.is_null() {
        true => document(this).file_type,
        false => kind,
    };
    let write: extern "C" fn(Id, Sel, Id, Id, usize, *mut Id) -> u8 = message::msg_send_as();
    let mut error = core::ptr::null_mut();
    let selector = sel!("writeSafelyToURL:ofType:forSaveOperation:error:");
    if url.is_null() || write(this, selector, url, kind, operation, &mut error) == 0 {
        return false;
    }
    if operation != SAVE_TO_OPERATION {
        store(&mut document(this).file_url, url);
        store(&mut document(this).file_type, kind);
        update_change_count(this, sel!(""), CHANGE_CLEARED);
        synchronize_titles(this);
        note_recent(url);
    }
    true
}

extern "C" fn write_safely(
    this: Id,
    _: Sel,
    url: Id,
    kind: Id,
    _operation: usize,
    error: *mut Id,
) -> u8 {
    let write: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> u8 = message::msg_send_as();
    write(this, sel!("writeToURL:ofType:error:"), url, kind, error)
}

extern "C" fn save_to_url(
    this: Id,
    _: Sel,
    url: Id,
    kind: Id,
    operation: usize,
    handler: Id,
) {
    save_to(this, url, kind, operation);
    if !handler.is_null() {
        // SAFETY: a save completion handler is a block taking itself and an error.
        let invoke: extern "C" fn(Id, Id) = unsafe { core::mem::transmute(invoke_block(handler)) };
        invoke(handler, core::ptr::null_mut());
    }
}

fn ask_for_url(this: Id) -> Id {
    let panel = send(class_pointer(&NSSAVEPANEL).cast(), sel!("savePanel"));
    if panel.is_null() {
        return panel;
    }
    let name = display_name(this, sel!(""));
    send1(panel, sel!("setNameFieldStringValue:"), name as usize);
    let extensions = extensions_for(document(this).file_type);
    if send(extensions, sel!("count")) as usize > 0 {
        send1(panel, sel!("setAllowedFileTypes:"), extensions as usize);
    }
    if responds(this, sel!("prepareSavePanel:"))
        && !send_bool(this, sel!("prepareSavePanel:"), panel as usize)
    {
        return core::ptr::null_mut();
    }
    match send(panel, sel!("runModal")) as isize {
        MODAL_OK => send(panel, sel!("URL")),
        _ => core::ptr::null_mut(),
    }
}

extern "C" fn save_document(this: Id, _: Sel, _sender: Id) {
    let url = document(this).file_url;
    match url.is_null() {
        true => save_document_as(this, sel!(""), core::ptr::null_mut()),
        false => {
            save_to(this, url, core::ptr::null_mut(), SAVE_OPERATION);
        }
    }
}

extern "C" fn save_document_as(this: Id, _: Sel, _sender: Id) {
    let url = ask_for_url(this);
    if !url.is_null() {
        save_to(this, url, core::ptr::null_mut(), SAVE_AS_OPERATION);
    }
}

extern "C" fn save_document_to(this: Id, _: Sel, _sender: Id) {
    let url = ask_for_url(this);
    if !url.is_null() {
        save_to(this, url, core::ptr::null_mut(), SAVE_TO_OPERATION);
    }
}

extern "C" fn revert_to_contents(this: Id, _: Sel, url: Id, kind: Id, error: *mut Id) -> u8 {
    let read: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> u8 = message::msg_send_as();
    if read(this, sel!("readFromURL:ofType:error:"), url, kind, error) == 0 {
        return 0;
    }
    update_change_count(this, sel!(""), CHANGE_CLEARED);
    1
}

extern "C" fn revert_document(this: Id, _: Sel, _sender: Id) {
    let fields = document(this);
    if !fields.file_url.is_null() {
        let (url, kind) = (fields.file_url, fields.file_type);
        revert_to_contents(this, sel!(""), url, kind, core::ptr::null_mut());
    }
}

fn ask_to_save(this: Id) -> isize {
    let alert = send(new_instance(class_pointer(&NSALERT)), sel!("init"));
    if alert.is_null() {
        return ALERT_DONT_SAVE;
    }
    let name = text_of(display_name(this, sel!(""))).unwrap_or_default();
    let message = alloc::format!(
        "Do you want to save the changes made to the document \u{201c}{name}\u{201d}?"
    );
    send1(alert, sel!("setMessageText:"), super::string(&message) as usize);
    send1(
        alert,
        sel!("setInformativeText:"),
        super::string("Your changes will be lost if you don\u{2019}t save them.") as usize,
    );
    for title in ["Save", "Don\u{2019}t Save", "Cancel"] {
        send1(alert, sel!("addButtonWithTitle:"), super::string(title) as usize);
    }
    let answer = send(alert, sel!("runModal")) as isize;
    objc_release(alert);
    answer
}

fn may_close(this: Id) -> bool {
    if document(this).change_count == 0 {
        return true;
    }
    match ask_to_save(this) {
        ALERT_SAVE => {
            save_document(this, sel!(""), core::ptr::null_mut());
            document(this).change_count == 0
        }
        ALERT_DONT_SAVE => true,
        _ => false,
    }
}

extern "C" fn can_close(this: Id, _: Sel, delegate: Id, selector: Sel, context: usize) {
    let answer = may_close(this);
    if !delegate.is_null() && !selector.is_null() {
        let tell: extern "C" fn(Id, Sel, Id, u8, usize) = message::msg_send_as();
        tell(delegate, selector, this, u8::from(answer), context);
    }
}

extern "C" fn should_close_window_controller(
    this: Id,
    _: Sel,
    window_controller: Id,
    delegate: Id,
    selector: Sel,
    context: usize,
) {
    let others = items_of(document(this).window_controllers)
        .into_iter()
        .any(|other| other != window_controller);
    let answer = others || may_close(this);
    if !delegate.is_null() && !selector.is_null() {
        let tell: extern "C" fn(Id, Sel, Id, u8, usize) = message::msg_send_as();
        tell(delegate, selector, this, u8::from(answer), context);
    }
}

extern "C" fn close_document(this: Id, _: Sel) {
    objc_retain(this);
    for window_controller in items_of(document(this).window_controllers) {
        send(window_controller, sel!("close"));
        remove_window_controller(this, sel!(""), window_controller);
    }
    let shared = shared_controller();
    if !shared.is_null() {
        remove_document(shared, sel!(""), this);
    }
    objc_release(this);
}

extern "C" fn perform_close(this: Id, _: Sel, _sender: Id) {
    if may_close(this) {
        close_document(this, sel!(""));
    }
}

extern "C" fn readable_types(class: Id, _: Sel) -> Id {
    types_of_class(class, false)
}

extern "C" fn writable_types(this: Id, _: Sel) -> Id {
    types_of_class(crate::objc::class::class_of(this).cast(), true)
}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn yes_for(_: Id, _: Sel, _: Id) -> u8 {
    1
}

extern "C" fn extension_for_type(_: Id, _: Sel, kind: Id, _operation: usize) -> Id {
    let extensions = extensions_for(kind);
    match send(extensions, sel!("count")) as usize {
        0 => core::ptr::null_mut(),
        _ => send1(extensions, sel!("objectAtIndex:"), 0),
    }
}

extern "C" fn title_for_display_name(_: Id, _: Sel, name: Id) -> Id {
    name
}

fn synchronize_titles(this: Id) {
    for window_controller in items_of(document(this).window_controllers) {
        synchronize_window_title(window_controller);
    }
}

/// Gives `window_controller`'s window its document's name as its title, as AppKit does when a
/// window controller has a document.
pub fn synchronize_window_title(window_controller: Id) {
    let owner = send(window_controller, sel!("document"));
    if owner.is_null() || !send_bool(window_controller, sel!("isWindowLoaded"), 0) {
        return;
    }
    let window = send(window_controller, sel!("window"));
    let name = send(owner, sel!("displayName"));
    let title = send1(
        window_controller,
        sel!("windowTitleForDocumentDisplayName:"),
        name as usize,
    );
    if !window.is_null() && !title.is_null() {
        send1(window, sel!("setTitle:"), title as usize);
    }
}

fn shared_controller() -> Id {
    SHARED.load(Ordering::Acquire) as Id
}

extern "C" fn shared(class: Id, _: Sel) -> Id {
    let existing = shared_controller();
    if !existing.is_null() {
        return existing;
    }
    send(send(class, sel!("alloc")), sel!("init"))
}

extern "C" fn controller_init(this: Id, _: Sel) -> Id {
    if this.is_null() {
        return this;
    }
    let _ = SHARED.compare_exchange(0, this as usize, Ordering::AcqRel, Ordering::Acquire);
    this
}

extern "C" fn documents(this: Id, _: Sel) -> Id {
    list_of(&mut controller(this).documents)
}

extern "C" fn add_document(this: Id, _: Sel, added: Id) {
    if added.is_null() {
        return;
    }
    let list = list_of(&mut controller(this).documents);
    if send_bool(list, sel!("containsObject:"), added as usize) {
        return;
    }
    if root::is_kind_of(added, &NSDOCUMENT) && document(added).file_url.is_null() {
        controller(this).untitled_count += 1;
        document(added).untitled_number = controller(this).untitled_count;
    }
    send1(list, sel!("addObject:"), added as usize);
}

extern "C" fn remove_document(this: Id, _: Sel, removed: Id) {
    let list = controller(this).documents;
    if !list.is_null() && !removed.is_null() {
        send1(list, sel!("removeObject:"), removed as usize);
    }
}

/// The document whose window controller owns `window`, and that window controller.
pub fn owner_of_window(window: Id) -> Option<(Id, Id)> {
    let shared = shared_controller();
    if shared.is_null() || window.is_null() {
        return None;
    }
    items_of(controller(shared).documents)
        .into_iter()
        .find_map(|owner| {
            items_of(document(owner).window_controllers)
                .into_iter()
                .find(|&window_controller| {
                    send_bool(window_controller, sel!("isWindowLoaded"), 0)
                        && send(window_controller, sel!("window")) == window
                })
                .map(|window_controller| (owner, window_controller))
        })
}

extern "C" fn document_for_window(_: Id, _: Sel, window: Id) -> Id {
    owner_of_window(window).map_or(core::ptr::null_mut(), |(owner, _)| owner)
}

extern "C" fn current_document(this: Id, sel: Sel) -> Id {
    document_for_window(this, sel, super::application::key_window())
}

extern "C" fn document_for_url(this: Id, _: Sel, url: Id) -> Id {
    items_of(controller(this).documents)
        .into_iter()
        .find(|&owner| same_text(document(owner).file_url, url))
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn has_edited_documents(this: Id, _: Sel) -> u8 {
    let edited = items_of(controller(this).documents)
        .into_iter()
        .any(|owner| send_bool(owner, sel!("isDocumentEdited"), 0));
    u8::from(edited)
}

extern "C" fn document_class_names(_: Id, _: Sel) -> Id {
    let list = mutable_array();
    let mut seen: Vec<String> = Vec::new();
    for known in document_types() {
        if let Some(name) = known.class_name
            && !seen.contains(&name)
        {
            send1(list, sel!("addObject:"), super::string(&name) as usize);
            seen.push(name);
        }
    }
    autoreleased(list)
}

extern "C" fn default_type(_: Id, _: Sel) -> Id {
    document_types()
        .into_iter()
        .find(|known| known.editor && known.class_name.is_some())
        .map_or(core::ptr::null_mut(), |known| {
            super::string(&known.identifier)
        })
}

extern "C" fn document_class_for_type(_: Id, _: Sel, kind: Id) -> Id {
    class_for_type(kind)
}

extern "C" fn display_name_for_type(_: Id, _: Sel, kind: Id) -> Id {
    text_of(kind)
        .and_then(|text| type_named(&text))
        .map_or(kind, |known| super::string(&known.name))
}

extern "C" fn type_for_url(_: Id, _: Sel, url: Id, error: *mut Id) -> Id {
    let extension = match url.is_null() {
        true => None,
        false => text_of(send(url, sel!("pathExtension"))),
    };
    match extension.and_then(|extension| type_for_extension(&extension)) {
        Some(known) => super::string(&known.identifier),
        None => {
            write_error(error);
            core::ptr::null_mut()
        }
    }
}

extern "C" fn make_untitled(this: Id, _: Sel, kind: Id, error: *mut Id) -> Id {
    let class = document_class_for_type(this, sel!(""), kind);
    if class.is_null() {
        write_error(error);
        return class;
    }
    let made = send(class, sel!("alloc"));
    let initialise: extern "C" fn(Id, Sel, Id, *mut Id) -> Id = message::msg_send_as();
    let made = initialise(made, sel!("initWithType:error:"), kind, error);
    match made.is_null() {
        true => made,
        false => autoreleased(made),
    }
}

extern "C" fn make_with_contents(this: Id, _: Sel, url: Id, kind: Id, error: *mut Id) -> Id {
    let class = document_class_for_type(this, sel!(""), kind);
    if class.is_null() {
        write_error(error);
        return class;
    }
    let made = send(class, sel!("alloc"));
    let initialise: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> Id = message::msg_send_as();
    let made = initialise(
        made,
        sel!("initWithContentsOfURL:ofType:error:"),
        url,
        kind,
        error,
    );
    match made.is_null() {
        true => made,
        false => autoreleased(made),
    }
}

fn display(made: Id) {
    send(made, sel!("makeWindowControllers"));
    send(made, sel!("showWindows"));
}

extern "C" fn open_untitled(this: Id, _: Sel, show: u8, error: *mut Id) -> Id {
    let kind = default_type(this, sel!(""));
    let make: extern "C" fn(Id, Sel, Id, *mut Id) -> Id = message::msg_send_as();
    let made = make(this, sel!("makeUntitledDocumentOfType:error:"), kind, error);
    if made.is_null() {
        return made;
    }
    send1(this, sel!("addDocument:"), made as usize);
    if show != 0 {
        display(made);
    }
    made
}

fn open_url(this: Id, url: Id, show: bool) -> (Id, bool) {
    let existing = document_for_url(this, sel!(""), url);
    if !existing.is_null() {
        if show {
            send(existing, sel!("showWindows"));
        }
        return (existing, true);
    }
    let mut error = core::ptr::null_mut();
    let kind_of: extern "C" fn(Id, Sel, Id, *mut Id) -> Id = message::msg_send_as();
    let kind = kind_of(this, sel!("typeForContentsOfURL:error:"), url, &mut error);
    if kind.is_null() {
        return (core::ptr::null_mut(), false);
    }
    let make: extern "C" fn(Id, Sel, Id, Id, *mut Id) -> Id = message::msg_send_as();
    let selector = sel!("makeDocumentWithContentsOfURL:ofType:error:");
    let made = make(this, selector, url, kind, &mut error);
    if made.is_null() {
        return (made, false);
    }
    send1(this, sel!("addDocument:"), made as usize);
    note_recent(url);
    if show {
        display(made);
    }
    (made, false)
}

extern "C" fn open_with_contents(this: Id, _: Sel, url: Id, show: u8, handler: Id) {
    let (opened, already) = open_url(this, url, show != 0);
    if !handler.is_null() {
        // SAFETY: an open completion handler is a block taking the document, a flag and an error.
        let invoke: extern "C" fn(Id, Id, u8, Id) =
            unsafe { core::mem::transmute(invoke_block(handler)) };
        invoke(handler, opened, u8::from(already), core::ptr::null_mut());
    }
}

extern "C" fn new_document(this: Id, _: Sel, _sender: Id) {
    open_untitled(this, sel!(""), 1, core::ptr::null_mut());
}

extern "C" fn open_document(this: Id, _: Sel, _sender: Id) {
    let panel = send(class_pointer(&NSOPENPANEL).cast(), sel!("openPanel"));
    if panel.is_null() {
        return;
    }
    send1(panel, sel!("setAllowsMultipleSelection:"), 1);
    let mut extensions: Vec<String> = Vec::new();
    for known in document_types() {
        extensions.extend(known.extensions.into_iter().filter(|text| text != "*"));
    }
    if !extensions.is_empty() {
        let list = mutable_array();
        for extension in &extensions {
            send1(list, sel!("addObject:"), super::string(extension) as usize);
        }
        send1(panel, sel!("setAllowedFileTypes:"), list as usize);
        objc_release(list);
    }
    if send(panel, sel!("runModal")) as isize != MODAL_OK {
        return;
    }
    for url in items_of(send(panel, sel!("URLs"))) {
        open_url(this, url, true);
    }
}

extern "C" fn save_all(this: Id, _: Sel, _sender: Id) {
    for owner in items_of(controller(this).documents) {
        if send_bool(owner, sel!("isDocumentEdited"), 0) {
            send1(owner, sel!("saveDocument:"), this as usize);
        }
    }
}

extern "C" fn close_all(this: Id, _: Sel, delegate: Id, selector: Sel, context: usize) {
    let mut all_closed = true;
    for owner in items_of(controller(this).documents) {
        match root::is_kind_of(owner, &NSDOCUMENT) && may_close(owner) {
            true => close_document(owner, sel!("")),
            false => all_closed = false,
        }
    }
    if !delegate.is_null() && !selector.is_null() {
        let tell: extern "C" fn(Id, Sel, Id, u8, usize) = message::msg_send_as();
        tell(delegate, selector, this, u8::from(all_closed), context);
    }
}

fn note_recent(url: Id) {
    let shared = shared_controller();
    if !shared.is_null() && !url.is_null() {
        send1(shared, sel!("noteNewRecentDocumentURL:"), url as usize);
    }
}

extern "C" fn note_new_recent(this: Id, _: Sel, url: Id) {
    if url.is_null() {
        return;
    }
    let list = list_of(&mut controller(this).recent);
    let earlier = items_of(list)
        .into_iter()
        .position(|listed| same_text(listed, url));
    if let Some(index) = earlier {
        send1(list, sel!("removeObjectAtIndex:"), index);
    }
    send2(list, sel!("insertObject:atIndex:"), url as usize, 0);
    while send(list, sel!("count")) as usize > MOST_RECENT_DOCUMENTS {
        send(list, sel!("removeLastObject"));
    }
}

extern "C" fn recent_urls(this: Id, _: Sel) -> Id {
    list_of(&mut controller(this).recent)
}

extern "C" fn clear_recent(this: Id, _: Sel, _sender: Id) {
    let list = controller(this).recent;
    if !list.is_null() {
        send(list, sel!("removeAllObjects"));
    }
}

extern "C" fn most_recent_count(_: Id, _: Sel) -> usize {
    MOST_RECENT_DOCUMENTS
}

extern "C" fn controller_dealloc(this: Id, _: Sel) {
    let _ = SHARED.compare_exchange(this as usize, 0, Ordering::AcqRel, Ordering::Acquire);
    let fields = controller(this);
    for slot in [&mut fields.documents, &mut fields.recent] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

/// Whether `window` may close: when it is its document's last window and the document has
/// unsaved changes, the user is asked to save them first.
pub fn may_close_window(window: Id) -> bool {
    let Some((owner, window_controller)) = owner_of_window(window) else {
        return true;
    };
    let others = items_of(document(owner).window_controllers)
        .into_iter()
        .any(|other| other != window_controller);
    others || may_close(owner)
}

/// After `window` closed: its window controller leaves its document, and a document left with
/// no window controller closes.
pub fn window_closed(window: Id) {
    let Some((owner, window_controller)) = owner_of_window(window) else {
        return;
    };
    objc_retain(owner);
    remove_window_controller(owner, sel!(""), window_controller);
    if items_of(document(owner).window_controllers).is_empty() {
        close_document(owner, sel!(""));
    }
    objc_release(owner);
}

/// Whether the app declares document types, so it is a document-based app.
pub fn is_document_based() -> bool {
    document_types()
        .iter()
        .any(|known| known.class_name.is_some())
}

/// Opens the untitled document a document-based app shows at launch when nothing else opened
/// a window, unless the delegate declines (`applicationShouldOpenUntitledFile:`) or opens it
/// itself (`applicationOpenUntitledFile:`).
pub fn open_untitled_at_launch(application: Id, delegate: Id) {
    if !is_document_based() {
        return;
    }
    let should = sel!("applicationShouldOpenUntitledFile:");
    if responds(delegate, should) && !send_bool(delegate, should, application as usize) {
        return;
    }
    let open_itself = sel!("applicationOpenUntitledFile:");
    if responds(delegate, open_itself) {
        send1(delegate, open_itself, application as usize);
        return;
    }
    let shared = shared(class_pointer(&NSDOCUMENTCONTROLLER).cast(), sel!(""));
    if !shared.is_null() {
        open_untitled(shared, sel!(""), 1, core::ptr::null_mut());
    }
}

/// The shared document controller, if the app made one, for the end of the responder chain.
pub fn existing_controller() -> Id {
    shared_controller()
}

pub fn define(runtime: &mut Runtime) -> bool {
    let document_methods = methods![
        "init" => document_init,
        "initWithType:error:" => init_with_type,
        "initWithContentsOfURL:ofType:error:" => init_with_contents,
        "initForURL:withContentsOfURL:ofType:error:" => init_for_url,
        "dealloc" => document_dealloc,
        "fileURL" => file_url,
        "setFileURL:" => set_file_url,
        "fileType" => file_type,
        "setFileType:" => set_file_type,
        "displayName" => display_name,
        "setDisplayName:" => set_display_name,
        "windowControllers" => window_controllers,
        "addWindowController:" => add_window_controller,
        "removeWindowController:" => remove_window_controller,
        "makeWindowControllers" => make_window_controllers,
        "windowNibName" => nil_answer,
        "undoManager" => nil_answer,
        "showWindows" => show_windows,
        "setWindow:" => set_window,
        "windowForSheet" => window_for_sheet,
        "isDocumentEdited" => is_edited,
        "hasUnautosavedChanges" => is_edited,
        "updateChangeCount:" => update_change_count,
        "readFromData:ofType:error:" => read_from_data,
        "readFromURL:ofType:error:" => read_from_url,
        "dataOfType:error:" => data_of_type,
        "writeToURL:ofType:error:" => write_to_url,
        "writeSafelyToURL:ofType:forSaveOperation:error:" => write_safely,
        "saveToURL:ofType:forSaveOperation:completionHandler:" => save_to_url,
        "saveDocument:" => save_document,
        "saveDocumentAs:" => save_document_as,
        "saveDocumentTo:" => save_document_to,
        "revertDocumentToSaved:" => revert_document,
        "revertToContentsOfURL:ofType:error:" => revert_to_contents,
        "canCloseDocumentWithDelegate:shouldCloseSelector:contextInfo:" => can_close,
        "shouldCloseWindowController:delegate:shouldCloseSelector:contextInfo:" => should_close_window_controller,
        "close" => close_document,
        "performClose:" => perform_close,
        "writableTypes" => writable_types,
        "prepareSavePanel:" => yes_for,
        "fileNameExtensionForType:saveOperation:" => extension_for_type,
        "windowTitleForDocumentDisplayName:" => title_for_display_name,
    ];
    let document_class_methods = methods![
        "readableTypes" => readable_types,
        "writableTypes" => readable_types,
        "autosavesInPlace" => no,
        "autosavesDrafts" => no,
        "preservesVersions" => no,
        "usesUbiquitousStorage" => no,
    ];
    let controller_methods = methods![
        "init" => controller_init,
        "dealloc" => controller_dealloc,
        "documents" => documents,
        "addDocument:" => add_document,
        "removeDocument:" => remove_document,
        "currentDocument" => current_document,
        "documentForWindow:" => document_for_window,
        "documentForURL:" => document_for_url,
        "hasEditedDocuments" => has_edited_documents,
        "documentClassNames" => document_class_names,
        "defaultType" => default_type,
        "documentClassForType:" => document_class_for_type,
        "displayNameForType:" => display_name_for_type,
        "typeForContentsOfURL:error:" => type_for_url,
        "makeUntitledDocumentOfType:error:" => make_untitled,
        "makeDocumentWithContentsOfURL:ofType:error:" => make_with_contents,
        "openUntitledDocumentAndDisplay:error:" => open_untitled,
        "openDocumentWithContentsOfURL:display:completionHandler:" => open_with_contents,
        "newDocument:" => new_document,
        "openDocument:" => open_document,
        "saveAllDocuments:" => save_all,
        "closeAllDocumentsWithDelegate:didCloseAllSelector:contextInfo:" => close_all,
        "noteNewRecentDocumentURL:" => note_new_recent,
        "recentDocumentURLs" => recent_urls,
        "clearRecentDocuments:" => clear_recent,
        "maximumRecentDocumentCount" => most_recent_count,
    ];
    let controller_class_methods = methods![
        "sharedDocumentController" => shared,
    ];
    super::define(
        runtime,
        b"NSDocument\0",
        (&NSDOCUMENT, &NSDOCUMENT_META),
        &NSOBJECT,
        core::mem::size_of::<DocumentFields>(),
        &document_methods,
        &document_class_methods,
    ) && super::define(
        runtime,
        b"NSDocumentController\0",
        (&NSDOCUMENTCONTROLLER, &NSDOCUMENTCONTROLLER_META),
        &NSOBJECT,
        core::mem::size_of::<ControllerFields>(),
        &controller_methods,
        &controller_class_methods,
    )
}

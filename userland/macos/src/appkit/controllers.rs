//! NSViewController and NSWindowController (D406): a view controller owns a view, made by
//! `loadView` from its nib (`nibName`, File's Owner the controller) or as a plain NSView, then
//! `viewDidLoad`; a window controller owns a window, loaded from its nib the first time it is
//! asked for, shown by `showWindow:`, and a content view controller whose view becomes the
//! window's content view. Both keep the storyboard that made them.

use super::{NSRESPONDER, NSVIEW, NSVIEWCONTROLLER, NSVIEWCONTROLLER_META, NSWINDOWCONTROLLER};
use super::{NSWINDOWCONTROLLER_META, nib, store, with_text};
use crate::foundation::{NSBUNDLE, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::dispose;
use crate::objc::{Runtime, methods, send, send1, sel};

#[repr(C)]
#[derive(Debug)]
struct ViewControllerFields {
    isa: usize,
    view: Id,
    represented_object: Id,
    title: Id,
    nib_name: Id,
    nib_bundle: Id,
    storyboard: Id,
    identifier: Id,
    loaded: bool,
}

#[repr(C)]
#[derive(Debug)]
struct WindowControllerFields {
    isa: usize,
    window: Id,
    content: Id,
    nib_name: Id,
    owner: Id,
    storyboard: Id,
    document: Id,
    loaded: bool,
}

fn view_fields(object: Id) -> &'static mut ViewControllerFields {
    // SAFETY: the callers pass NSViewController instances, whose memory starts with these.
    unsafe { &mut *object.cast::<ViewControllerFields>() }
}

fn window_fields(object: Id) -> &'static mut WindowControllerFields {
    // SAFETY: the callers pass NSWindowController instances, whose memory starts with these.
    unsafe { &mut *object.cast::<WindowControllerFields>() }
}

fn main_bundle() -> Id {
    send(class_pointer(&NSBUNDLE).cast(), sel!("mainBundle"))
}

extern "C" fn view_init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn view_init_with_nib(this: Id, _: Sel, name: Id, bundle: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = view_fields(this);
    store(&mut fields.nib_name, name);
    store(&mut fields.nib_bundle, bundle);
    this
}

extern "C" fn view_dealloc(this: Id, _: Sel) {
    let fields = view_fields(this);
    for slot in [
        &mut fields.view,
        &mut fields.represented_object,
        &mut fields.title,
        &mut fields.nib_name,
        &mut fields.nib_bundle,
        &mut fields.storyboard,
        &mut fields.identifier,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn load_view(this: Id, _: Sel) {
    let fields = view_fields(this);
    if !fields.nib_name.is_null() {
        let bundle = match fields.nib_bundle.is_null() {
            true => main_bundle(),
            false => fields.nib_bundle,
        };
        let name = fields.nib_name;
        let loaded = with_text(name, |text| nib::load_from_bundle(bundle, text.as_bytes(), this));
        if let Ok(mut loaded) = loaded {
            loaded.top_level.free();
        }
    }
    if view_fields(this).view.is_null() {
        let view = new_instance(class_pointer(&NSVIEW));
        let view = send(view, sel!("init"));
        store(&mut view_fields(this).view, view);
        crate::objc::reference_count::objc_release(view);
    }
}

extern "C" fn view(this: Id, _: Sel) -> Id {
    let fields = view_fields(this);
    if !fields.loaded {
        fields.loaded = true;
        if fields.view.is_null() {
            send(this, sel!("loadView"));
        }
        send(this, sel!("viewDidLoad"));
    }
    view_fields(this).view
}

extern "C" fn set_view(this: Id, _: Sel, view: Id) {
    store(&mut view_fields(this).view, view);
}

extern "C" fn view_loaded(this: Id, _: Sel) -> u8 {
    u8::from(!view_fields(this).view.is_null())
}

extern "C" fn nothing(_: Id, _: Sel) {}

extern "C" fn nothing_animated(_: Id, _: Sel, _: u8) {}

macro_rules! object_property {
    ($getter:ident, $setter:ident, $fields:ident, $field:ident) => {
        extern "C" fn $getter(this: Id, _: Sel) -> Id {
            $fields(this).$field
        }

        extern "C" fn $setter(this: Id, _: Sel, value: Id) {
            store(&mut $fields(this).$field, value);
        }
    };
}

object_property!(represented_object, set_represented_object, view_fields, represented_object);
object_property!(view_title, set_view_title, view_fields, title);
object_property!(view_storyboard, set_view_storyboard, view_fields, storyboard);
object_property!(view_identifier, set_view_identifier, view_fields, identifier);
object_property!(window_storyboard, set_window_storyboard, window_fields, storyboard);
object_property!(document, set_document, window_fields, document);

extern "C" fn nib_name(this: Id, _: Sel) -> Id {
    view_fields(this).nib_name
}

extern "C" fn nib_bundle(this: Id, _: Sel) -> Id {
    view_fields(this).nib_bundle
}

extern "C" fn present_as_window(_: Id, _: Sel, controller: Id) {
    let window = new_instance(class_pointer(&super::NSWINDOW));
    let window = send(window, sel!("init"));
    if window.is_null() || controller.is_null() {
        return;
    }
    let holder = new_instance(class_pointer(&NSWINDOWCONTROLLER));
    let holder = window_init_with_window(holder, sel!("initWithWindow:"), window);
    set_content(holder, sel!("setContentViewController:"), controller);
    send1(holder, sel!("showWindow:"), 0);
}

extern "C" fn window_init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn window_init_with_window(this: Id, _: Sel, window: Id) -> Id {
    if this.is_null() {
        return this;
    }
    store(&mut window_fields(this).window, window);
    window_fields(this).loaded = !window.is_null();
    this
}

extern "C" fn window_init_with_nib(this: Id, _: Sel, name: Id) -> Id {
    window_init_with_nib_owner(this, sel!("initWithWindowNibName:owner:"), name, this)
}

extern "C" fn window_init_with_nib_owner(this: Id, _: Sel, name: Id, owner: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = window_fields(this);
    store(&mut fields.nib_name, name);
    fields.owner = owner;
    this
}

extern "C" fn window_dealloc(this: Id, _: Sel) {
    let fields = window_fields(this);
    for slot in [
        &mut fields.window,
        &mut fields.content,
        &mut fields.nib_name,
        &mut fields.storyboard,
        &mut fields.document,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn window(this: Id, _: Sel) -> Id {
    let fields = window_fields(this);
    if !fields.loaded {
        fields.loaded = true;
        if fields.window.is_null() && !fields.nib_name.is_null() {
            send(this, sel!("windowWillLoad"));
            let owner = match fields.owner.is_null() {
                true => this,
                false => fields.owner,
            };
            let name = fields.nib_name;
            let loaded = with_text(name, |text| {
                nib::load_from_bundle(main_bundle(), text.as_bytes(), owner)
            });
            if let Ok(mut loaded) = loaded {
                loaded.top_level.free();
            }
            send(this, sel!("windowDidLoad"));
        }
    }
    window_fields(this).window
}

extern "C" fn set_window(this: Id, _: Sel, window: Id) {
    store(&mut window_fields(this).window, window);
    let content = window_fields(this).content;
    if !window.is_null() && !content.is_null() {
        let view = send(content, sel!("view"));
        send1(window, sel!("setContentView:"), view as usize);
    }
}

extern "C" fn window_loaded(this: Id, _: Sel) -> u8 {
    u8::from(!window_fields(this).window.is_null())
}

extern "C" fn content(this: Id, _: Sel) -> Id {
    window_fields(this).content
}

extern "C" fn set_content(this: Id, _: Sel, controller: Id) {
    store(&mut window_fields(this).content, controller);
    let window = window_fields(this).window;
    if !window.is_null() && !controller.is_null() {
        let view = send(controller, sel!("view"));
        send1(window, sel!("setContentView:"), view as usize);
        let title = send(controller, sel!("title"));
        if !title.is_null() {
            send1(window, sel!("setTitle:"), title as usize);
        }
    }
}

extern "C" fn show_window(this: Id, _: Sel, _sender: Id) {
    let window = send(this, sel!("window"));
    if !window.is_null() {
        send1(window, sel!("makeKeyAndOrderFront:"), this as usize);
    }
}

extern "C" fn close(this: Id, _: Sel) {
    let window = window_fields(this).window;
    if !window.is_null() {
        send(window, sel!("close"));
    }
}

extern "C" fn window_nib_name(this: Id, _: Sel) -> Id {
    window_fields(this).nib_name
}

extern "C" fn owner(this: Id, _: Sel) -> Id {
    match window_fields(this).owner {
        owner if owner.is_null() => this,
        owner => owner,
    }
}

pub fn define(runtime: &mut Runtime) -> bool {
    let view_methods = methods![
        "init" => view_init,
        "initWithNibName:bundle:" => view_init_with_nib,
        "dealloc" => view_dealloc,
        "loadView" => load_view,
        "loadViewIfNeeded" => view,
        "view" => view,
        "setView:" => set_view,
        "isViewLoaded" => view_loaded,
        "viewDidLoad" => nothing,
        "viewWillAppear" => nothing,
        "viewDidAppear" => nothing,
        "viewWillDisappear" => nothing,
        "viewDidDisappear" => nothing,
        "viewWillLayout" => nothing,
        "viewDidLayout" => nothing,
        "representedObject" => represented_object,
        "setRepresentedObject:" => set_represented_object,
        "title" => view_title,
        "setTitle:" => set_view_title,
        "nibName" => nib_name,
        "nibBundle" => nib_bundle,
        "storyboard" => view_storyboard,
        "setStoryboard:" => set_view_storyboard,
        "identifier" => view_identifier,
        "setIdentifier:" => set_view_identifier,
        "presentViewControllerAsModalWindow:" => present_as_window,
        "dismissController:" => nothing_animated,
    ];
    let window_methods = methods![
        "init" => window_init,
        "initWithWindow:" => window_init_with_window,
        "initWithWindowNibName:" => window_init_with_nib,
        "initWithWindowNibName:owner:" => window_init_with_nib_owner,
        "dealloc" => window_dealloc,
        "window" => window,
        "setWindow:" => set_window,
        "isWindowLoaded" => window_loaded,
        "windowWillLoad" => nothing,
        "windowDidLoad" => nothing,
        "loadWindow" => nothing,
        "contentViewController" => content,
        "setContentViewController:" => set_content,
        "showWindow:" => show_window,
        "close" => close,
        "windowNibName" => window_nib_name,
        "owner" => owner,
        "storyboard" => window_storyboard,
        "setStoryboard:" => set_window_storyboard,
        "document" => document,
        "setDocument:" => set_document,
    ];
    super::define(
        runtime,
        b"NSViewController\0",
        (&NSVIEWCONTROLLER, &NSVIEWCONTROLLER_META),
        &NSRESPONDER,
        core::mem::size_of::<ViewControllerFields>(),
        &view_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSWindowController\0",
        (&NSWINDOWCONTROLLER, &NSWINDOWCONTROLLER_META),
        &NSRESPONDER,
        core::mem::size_of::<WindowControllerFields>(),
        &window_methods,
        &[],
    )
}

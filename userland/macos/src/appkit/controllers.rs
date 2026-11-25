//! NSViewController and NSWindowController (D406): a view controller owns a view, made by
//! `loadView` from its nib (`nibName`, File's Owner the controller) or as a plain NSView, then
//! `viewDidLoad`; a window controller owns a window, loaded from its nib the first time it is
//! asked for, shown by `showWindow:`, and a content view controller whose view becomes the
//! window's content view. Both keep the storyboard that made them. A view controller keeps
//! its child controllers (`addChildViewController:` and kin) and the controllers it presents
//! in windows of their own or as sheets, sends the appearance calls (`viewWillAppear`,
//! `viewDidAppear` and their disappearing pair) to itself and its children as its window
//! shows and closes, and performs its storyboard's segues (`segues.rs`, D749).

use super::{NSRESPONDER, NSVIEW, NSVIEWCONTROLLER, NSVIEWCONTROLLER_META, NSWINDOWCONTROLLER};
use super::{NSWINDOWCONTROLLER_META, nib, store, with_text};
use crate::foundation::{NSARRAY, NSBUNDLE, NSMUTABLEARRAY, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::{Runtime, message, methods, sel, send, send_bool, send1, send2};

/// A view controller's fields, which its subclasses' start with.
#[repr(C)]
#[derive(Debug)]
pub(super) struct ViewControllerFields {
    isa: usize,
    view: Id,
    represented_object: Id,
    title: Id,
    nib_name: Id,
    nib_bundle: Id,
    storyboard: Id,
    identifier: Id,
    loaded: bool,
    appeared: bool,
    children: Id,
    parent: Id,
    presented: Id,
    presenting: Id,
    holder: Id,
    templates: Id,
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
    templates: Id,
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
        &mut fields.children,
        &mut fields.presented,
        &mut fields.holder,
        &mut fields.templates,
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
        let loaded = with_text(name, |text| {
            nib::load_from_bundle(bundle, text.as_bytes(), this)
        });
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

object_property!(
    represented_object,
    set_represented_object,
    view_fields,
    represented_object
);
object_property!(view_title, set_view_title, view_fields, title);
object_property!(
    view_storyboard,
    set_view_storyboard,
    view_fields,
    storyboard
);
object_property!(
    view_identifier,
    set_view_identifier,
    view_fields,
    identifier
);
object_property!(
    window_storyboard,
    set_window_storyboard,
    window_fields,
    storyboard
);
extern "C" fn document(this: Id, _: Sel) -> Id {
    window_fields(this).document
}

extern "C" fn set_document(this: Id, _: Sel, document: Id) {
    window_fields(this).document = document;
    super::document::synchronize_window_title(this);
}

extern "C" fn nib_name(this: Id, _: Sel) -> Id {
    view_fields(this).nib_name
}

extern "C" fn nib_bundle(this: Id, _: Sel) -> Id {
    view_fields(this).nib_bundle
}

fn mutable_array() -> Id {
    objc_retain(send(class_pointer(&NSMUTABLEARRAY).cast(), sel!("array")))
}

fn list_of(slot: &mut Id) -> Id {
    if slot.is_null() {
        *slot = mutable_array();
    }
    *slot
}

fn count_of(list: Id) -> usize {
    match list.is_null() {
        true => 0,
        false => send(list, sel!("count")) as usize,
    }
}

fn items_of(list: Id) -> alloc::vec::Vec<Id> {
    (0..count_of(list))
        .map(|index| send1(list, sel!("objectAtIndex:"), index))
        .collect()
}

fn appear(controller: Id, showing: bool) {
    if controller.is_null() || view_fields(controller).appeared == showing {
        return;
    }
    let (will, did) = match showing {
        true => (sel!("viewWillAppear"), sel!("viewDidAppear")),
        false => (sel!("viewWillDisappear"), sel!("viewDidDisappear")),
    };
    send(controller, will);
    view_fields(controller).appeared = showing;
    for child in items_of(view_fields(controller).children) {
        appear(child, showing);
    }
    send(controller, did);
}

/// Sends the appearance calls to `controller` and its children as its window shows (or, for
/// `false`, closes).
pub fn set_appeared(controller: Id, showing: bool) {
    appear(controller, showing);
}

fn present_in_window(this: Id, controller: Id) {
    if controller.is_null() {
        return;
    }
    let window = send(new_instance(class_pointer(&super::NSWINDOW)), sel!("init"));
    if window.is_null() {
        return;
    }
    let holder = new_instance(class_pointer(&NSWINDOWCONTROLLER));
    let holder = window_init_with_window(holder, sel!("initWithWindow:"), window);
    objc_release(window);
    if holder.is_null() {
        return;
    }
    set_content(holder, sel!("setContentViewController:"), controller);
    store(&mut view_fields(controller).holder, holder);
    objc_release(holder);
    if !this.is_null() && this != controller {
        view_fields(controller).presenting = this;
        let presented = list_of(&mut view_fields(this).presented);
        send1(presented, sel!("addObject:"), controller as usize);
    }
    send1(holder, sel!("showWindow:"), 0);
}

extern "C" fn present_as_window(this: Id, _: Sel, controller: Id) {
    present_in_window(this, controller);
}

extern "C" fn present_as_popover(
    this: Id,
    _: Sel,
    controller: Id,
    _rect: eggos_macos_gui::geometry::Frame,
    _view: Id,
    _edge: usize,
    _behavior: isize,
) {
    present_in_window(this, controller);
}

extern "C" fn dismiss_view_controller(this: Id, _: Sel, controller: Id) {
    if controller.is_null() {
        return;
    }
    let holder = view_fields(controller).holder;
    appear(controller, false);
    if !holder.is_null() {
        close(holder, sel!("close"));
    }
    store(&mut view_fields(controller).holder, core::ptr::null_mut());
    view_fields(controller).presenting = core::ptr::null_mut();
    let presented = view_fields(this).presented;
    if !presented.is_null() {
        send1(presented, sel!("removeObject:"), controller as usize);
    }
}

extern "C" fn dismiss_controller(this: Id, _: Sel, _sender: Id) {
    let presenting = view_fields(this).presenting;
    if !presenting.is_null() {
        send1(presenting, sel!("dismissViewController:"), this as usize);
        return;
    }
    let holder = view_fields(this).holder;
    if !holder.is_null() {
        appear(this, false);
        close(holder, sel!("close"));
    }
}

extern "C" fn children(this: Id, _: Sel) -> Id {
    match view_fields(this).children.is_null() {
        true => send(class_pointer(&NSARRAY).cast(), sel!("array")),
        false => view_fields(this).children,
    }
}

fn detach(child: Id) {
    let parent = view_fields(child).parent;
    if parent.is_null() {
        return;
    }
    objc_retain(child);
    let list = view_fields(parent).children;
    if !list.is_null() {
        send1(list, sel!("removeObject:"), child as usize);
    }
    view_fields(child).parent = core::ptr::null_mut();
    if view_fields(child).appeared {
        appear(child, false);
    }
    objc_release(child);
}

extern "C" fn insert_child(this: Id, _: Sel, child: Id, index: isize) {
    if child.is_null() || child == this {
        return;
    }
    detach(child);
    let list = list_of(&mut view_fields(this).children);
    let at = (index.max(0) as usize).min(count_of(list));
    send2(list, sel!("insertObject:atIndex:"), child as usize, at);
    view_fields(child).parent = this;
    if view_fields(this).appeared {
        appear(child, true);
    }
}

extern "C" fn add_child(this: Id, sel: Sel, child: Id) {
    let end = count_of(view_fields(this).children) as isize;
    insert_child(this, sel, child, end);
}

extern "C" fn set_children(this: Id, sel: Sel, list: Id) {
    for child in items_of(view_fields(this).children) {
        detach(child);
    }
    for child in items_of(list) {
        add_child(this, sel, child);
    }
}

extern "C" fn remove_child_at(this: Id, _: Sel, index: isize) {
    let list = view_fields(this).children;
    if index < 0 || index as usize >= count_of(list) {
        return;
    }
    detach(send1(list, sel!("objectAtIndex:"), index as usize));
}

extern "C" fn remove_from_parent(this: Id, _: Sel) {
    detach(this);
}

extern "C" fn parent(this: Id, _: Sel) -> Id {
    view_fields(this).parent
}

extern "C" fn presenting(this: Id, _: Sel) -> Id {
    view_fields(this).presenting
}

extern "C" fn presented(this: Id, sel: Sel) -> Id {
    match view_fields(this).presented.is_null() {
        true => children(core::ptr::null_mut(), sel),
        false => view_fields(this).presented,
    }
}

extern "C" fn transition(this: Id, _: Sel, from: Id, to: Id, _options: usize, completion: Id) {
    if !from.is_null() && !to.is_null() {
        let old = send(from, sel!("view"));
        let new = send(to, sel!("view"));
        let superview = send(old, sel!("superview"));
        if !superview.is_null() {
            let frame: extern "C" fn(Id, Sel) -> eggos_macos_gui::geometry::Frame =
                message::msg_send_as();
            super::send_frame(new, sel!("setFrame:"), frame(old, sel!("frame")));
            send2(
                superview,
                sel!("replaceSubview:with:"),
                old as usize,
                new as usize,
            );
        }
        if view_fields(this).appeared {
            appear(from, false);
            appear(to, true);
        }
    }
    if !completion.is_null() {
        let invoke = crate::objc::block::invoke_address(completion);
        // SAFETY: a completion handler is a block taking only itself.
        let invoke: extern "C" fn(Id) = unsafe { core::mem::transmute(invoke) };
        invoke(completion);
    }
}

extern "C" fn perform_segue(this: Id, _: Sel, identifier: Id, sender: Id) {
    let fields = view_fields(this);
    super::segues::perform_from(this, (fields.templates, fields.storyboard), identifier, sender);
}

extern "C" fn window_perform_segue(this: Id, _: Sel, identifier: Id, sender: Id) {
    let fields = window_fields(this);
    super::segues::perform_from(this, (fields.templates, fields.storyboard), identifier, sender);
}

extern "C" fn should_perform(_: Id, _: Sel, _: Id, _: Id) -> u8 {
    1
}

extern "C" fn prepare(_: Id, _: Sel, _: Id, _: Id) {}

extern "C" fn set_templates(this: Id, _: Sel, templates: Id) {
    store(&mut view_fields(this).templates, templates);
}

extern "C" fn set_window_templates(this: Id, _: Sel, templates: Id) {
    store(&mut window_fields(this).templates, templates);
}

extern "C" fn title_for_display_name(_: Id, _: Sel, name: Id) -> Id {
    name
}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn ignore_flag(_: Id, _: Sel, _: u8) {}

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
        &mut fields.templates,
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
        super::document::synchronize_window_title(this);
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
        let content = window_fields(this).content;
        let visible = send_bool(window, sel!("isVisible"), 0);
        send1(window, sel!("makeKeyAndOrderFront:"), this as usize);
        if !visible {
            appear(content, true);
        }
    }
}

extern "C" fn close(this: Id, _: Sel) {
    let window = window_fields(this).window;
    if !window.is_null() {
        appear(window_fields(this).content, false);
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
        "presentViewControllerAsSheet:" => present_as_window,
        "presentViewController:asPopoverRelativeToRect:ofView:preferredEdge:behavior:" => present_as_popover,
        "dismissViewController:" => dismiss_view_controller,
        "dismissController:" => dismiss_controller,
        "childViewControllers" => children,
        "setChildViewControllers:" => set_children,
        "addChildViewController:" => add_child,
        "insertChildViewController:atIndex:" => insert_child,
        "removeChildViewControllerAtIndex:" => remove_child_at,
        "removeFromParentViewController" => remove_from_parent,
        "parentViewController" => parent,
        "presentingViewController" => presenting,
        "presentedViewControllers" => presented,
        "transitionFromViewController:toViewController:options:completionHandler:" => transition,
        "performSegueWithIdentifier:sender:" => perform_segue,
        "shouldPerformSegueWithIdentifier:sender:" => should_perform,
        "prepareForSegue:sender:" => prepare,
        "_setStoryboardSegueTemplates:" => set_templates,
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
        "windowTitleForDocumentDisplayName:" => title_for_display_name,
        "shouldCloseDocument" => no,
        "setShouldCloseDocument:" => ignore_flag,
        "performSegueWithIdentifier:sender:" => window_perform_segue,
        "shouldPerformSegueWithIdentifier:sender:" => should_perform,
        "prepareForSegue:sender:" => prepare,
        "_setStoryboardSegueTemplates:" => set_window_templates,
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

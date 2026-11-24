//! Storyboard segues (D749): `NSStoryboardSegue` (an identifier, a source and a destination
//! controller, its kind, and a perform handler for `segueWithIdentifier:source:destination:
//! performHandler:`), and the templates a compiled storyboard keeps in each controller
//! (`NSStoryboardSegueTemplate` and its kind-named subclasses), which
//! `performSegueWithIdentifier:sender:` turns into a segue: the destination made from the
//! source's storyboard, the program's segue class if it named one, `prepareForSegue:sender:`
//! sent to the source, then `perform`: a window controller shown, a view controller shown in a
//! window of its own (show, modal) or as a sheet (sheet, popover), a child for an embedding.

use super::{NSSTORYBOARDSEGUE, NSSTORYBOARDSEGUE_META, NSSTORYBOARDSEGUETEMPLATE};
use super::{NSSTORYBOARDSEGUETEMPLATE_META, NSVIEWCONTROLLER, NSWINDOWCONTROLLER, store, with_text};
use crate::foundation::{autoreleased, class_pointer, new_instance, string_from_utf8};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, class_named, message, methods, sel, send, send1, send2};

/// How a segue shows its destination.
pub mod kind {
    pub const SHOW: usize = 0;
    pub const MODAL: usize = 1;
    pub const SHEET: usize = 2;
    pub const POPOVER: usize = 3;
    pub const EMBED: usize = 4;
    pub const CUSTOM: usize = 5;
}

#[repr(C)]
#[derive(Debug)]
struct SegueFields {
    isa: usize,
    identifier: Id,
    source: Id,
    destination: Id,
    handler: Id,
    kind: usize,
}

#[repr(C)]
#[derive(Debug)]
struct TemplateFields {
    isa: usize,
    identifier: Id,
    destination: Id,
    segue_class: Id,
    kind: usize,
}

fn segue(object: Id) -> &'static mut SegueFields {
    // SAFETY: the callers pass NSStoryboardSegue instances, whose memory starts with these.
    unsafe { &mut *object.cast::<SegueFields>() }
}

fn template(object: Id) -> &'static mut TemplateFields {
    // SAFETY: the callers pass NSStoryboardSegueTemplate instances.
    unsafe { &mut *object.cast::<TemplateFields>() }
}

/// The kind a template class name (`NSStoryboard<Kind>SegueTemplate`) names.
pub fn kind_of_class(name: &[u8]) -> usize {
    let middle = name
        .strip_prefix(b"NSStoryboard")
        .and_then(|rest| rest.strip_suffix(b"SegueTemplate"))
        .unwrap_or(b"");
    match middle {
        b"Modal" | b"ModalWindow" => kind::MODAL,
        b"Sheet" => kind::SHEET,
        b"Popover" => kind::POPOVER,
        b"Embed" | b"Relationship" => kind::EMBED,
        b"Custom" => kind::CUSTOM,
        _ => kind::SHOW,
    }
}

/// A new template from a compiled storyboard: its identifier, destination identifier, the
/// segue class the program named (or nil) and its kind; one reference.
pub fn new_template(identifier: Id, destination: Id, segue_class: Id, kind_number: usize) -> Id {
    let made = new_instance(class_pointer(&NSSTORYBOARDSEGUETEMPLATE));
    if made.is_null() {
        return made;
    }
    let fields = template(made);
    store(&mut fields.identifier, identifier);
    store(&mut fields.destination, destination);
    store(&mut fields.segue_class, segue_class);
    fields.kind = kind_number;
    made
}

extern "C" fn init_segue(this: Id, _: Sel, identifier: Id, source: Id, destination: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = segue(this);
    store(&mut fields.identifier, identifier);
    store(&mut fields.source, source);
    store(&mut fields.destination, destination);
    this
}

extern "C" fn segue_with_handler(
    class: Id,
    _: Sel,
    identifier: Id,
    source: Id,
    destination: Id,
    handler: Id,
) -> Id {
    let made = send(class, sel!("alloc"));
    let made = init_segue(made, sel!(""), identifier, source, destination);
    if made.is_null() {
        return made;
    }
    let copied = match handler.is_null() {
        true => handler,
        false => send(handler, sel!("copy")),
    };
    segue(made).handler = copied;
    segue(made).kind = kind::CUSTOM;
    autoreleased(made)
}

extern "C" fn segue_dealloc(this: Id, _: Sel) {
    let fields = segue(this);
    for slot in [
        &mut fields.identifier,
        &mut fields.source,
        &mut fields.destination,
        &mut fields.handler,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn identifier(this: Id, _: Sel) -> Id {
    segue(this).identifier
}

extern "C" fn source(this: Id, _: Sel) -> Id {
    segue(this).source
}

extern "C" fn destination(this: Id, _: Sel) -> Id {
    segue(this).destination
}

fn show_destination(source: Id, destination: Id, how: usize) {
    if root::is_kind_of(destination, &NSWINDOWCONTROLLER) {
        send1(destination, sel!("showWindow:"), source as usize);
        return;
    }
    let presenter = match root::is_kind_of(source, &NSWINDOWCONTROLLER) {
        true => send(source, sel!("contentViewController")),
        false => source,
    };
    let selector = match how {
        kind::SHEET | kind::POPOVER if !presenter.is_null() => {
            sel!("presentViewControllerAsSheet:")
        }
        _ if !presenter.is_null() => sel!("presentViewControllerAsModalWindow:"),
        _ => return,
    };
    send1(presenter, selector, destination as usize);
}

extern "C" fn perform(this: Id, _: Sel) {
    let fields = segue(this);
    if !fields.handler.is_null() {
        let invoke = crate::objc::block::invoke_address(fields.handler);
        // SAFETY: a perform handler is a block taking only itself.
        let invoke: extern "C" fn(Id) = unsafe { core::mem::transmute(invoke) };
        invoke(fields.handler);
        return;
    }
    if fields.destination.is_null() {
        return;
    }
    if fields.kind == kind::EMBED {
        if root::is_kind_of(fields.source, &NSVIEWCONTROLLER) {
            send1(fields.source, sel!("addChildViewController:"), fields.destination as usize);
        }
        return;
    }
    show_destination(fields.source, fields.destination, fields.kind);
}

extern "C" fn template_dealloc(this: Id, _: Sel) {
    let fields = template(this);
    for slot in [
        &mut fields.identifier,
        &mut fields.destination,
        &mut fields.segue_class,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn template_identifier(this: Id, _: Sel) -> Id {
    template(this).identifier
}

fn find_template(templates: Id, wanted: Id) -> Id {
    if templates.is_null() || wanted.is_null() {
        return core::ptr::null_mut();
    }
    let count = send(templates, sel!("count")) as usize;
    (0..count)
        .map(|index| send1(templates, sel!("objectAtIndex:"), index))
        .find(|&found| {
            let name = template(found).identifier;
            !name.is_null() && crate::objc::send_bool(name, sel!("isEqual:"), wanted as usize)
        })
        .unwrap_or(core::ptr::null_mut())
}

/// Performs the segue `identifier` of `source`, whose templates are `templates` and whose
/// storyboard is `storyboard`, with `sender`: false when it has no such segue.
pub fn perform_from(
    source: Id,
    (templates, storyboard): (Id, Id),
    identifier: Id,
    sender: Id,
) -> bool {
    let found = find_template(templates, identifier);
    if found.is_null() || storyboard.is_null() {
        return false;
    }
    let fields = template(found);
    let destination = send1(
        storyboard,
        sel!("instantiateControllerWithIdentifier:"),
        fields.destination as usize,
    );
    let program_class = match fields.segue_class.is_null() {
        true => core::ptr::null_mut(),
        false => with_text(fields.segue_class, |name| class_named(name.as_bytes())),
    };
    let class = match program_class.is_null() {
        true => class_pointer(&NSSTORYBOARDSEGUE),
        false => program_class,
    };
    let made = send(class.cast(), sel!("alloc"));
    let initialise: extern "C" fn(Id, Sel, Id, Id, Id) -> Id = message::msg_send_as();
    let made = initialise(
        made,
        sel!("initWithIdentifier:source:destination:"),
        identifier,
        source,
        destination,
    );
    if made.is_null() {
        return false;
    }
    if root::is_kind_of(made, &NSSTORYBOARDSEGUE) {
        segue(made).kind = match program_class.is_null() {
            true => fields.kind,
            false => kind::CUSTOM,
        };
    }
    let prepare = sel!("prepareForSegue:sender:");
    if super::responds(source, prepare) {
        send2(source, prepare, made as usize, sender as usize);
    }
    send(made, sel!("perform"));
    crate::objc::reference_count::objc_release(made);
    true
}

/// A retained string of `text`, for the nib loader's template fields.
pub fn retained_text(text: &[u8]) -> Id {
    objc_retain(string_from_utf8(text))
}

pub fn define(runtime: &mut Runtime) -> bool {
    let segue_methods = methods![
        "initWithIdentifier:source:destination:" => init_segue,
        "dealloc" => segue_dealloc,
        "identifier" => identifier,
        "sourceController" => source,
        "destinationController" => destination,
        "perform" => perform,
    ];
    let segue_class_methods = methods![
        "segueWithIdentifier:source:destination:performHandler:" => segue_with_handler,
    ];
    let template_methods = methods![
        "dealloc" => template_dealloc,
        "identifier" => template_identifier,
    ];
    super::define(
        runtime,
        b"NSStoryboardSegue\0",
        (&NSSTORYBOARDSEGUE, &NSSTORYBOARDSEGUE_META),
        &NSOBJECT,
        core::mem::size_of::<SegueFields>(),
        &segue_methods,
        &segue_class_methods,
    ) && super::define(
        runtime,
        b"NSStoryboardSegueTemplate\0",
        (&NSSTORYBOARDSEGUETEMPLATE, &NSSTORYBOARDSEGUETEMPLATE_META),
        &NSOBJECT,
        core::mem::size_of::<TemplateFields>(),
        &template_methods,
        &[],
    )
}

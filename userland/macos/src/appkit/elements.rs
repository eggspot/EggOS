//! An AppKit app's windows as element trees for agents (D161, D162, D175; `wm show`, `wm act`):
//! the window server's `Query` record reaches the event loop, which answers it. A window is the
//! root element, named by its title; its views follow in pre-order from the content view, each
//! numbered from 2 in that order (hidden views and their subviews left out): a push button as a
//! button named by its title (the default button flagged), an editable text field as a text
//! field whose value is its text, a label as a label, a plain `NSView` as a group and any view
//! of the app's own class as a graphic named by its class, focusable when it accepts the first
//! responder. Each has its frame in the window's content pixels (a control its drawn 28 pixel
//! rectangle). `invoke` clicks a button as `performClick:` does, `focus` makes a focusable view
//! the first responder, `set-text` replaces a text field's text; disabled controls refuse.
//! While an alert runs, trees are answered and actions refused, as the app is modal.

use super::{NSBUTTON, NSTEXTFIELD, NSVIEW, about, application, control, is, server, view, window};
use crate::libsystem::heap;
use crate::objc::abi::Id;
use crate::objc::{class, root, sel, send1};
use eggos_element_tree::{
    Action, Bounds, Element, MAX_LABEL_BYTES, MAX_TEXT_BYTES, MAX_TREE_BYTES, Outcome, Query, Role,
    TreeWriter, flags, query_kind, tree_answer,
};
use eggos_macos_gui::geometry::{Frame, pixel_rect};
use eggos_macos_gui::paint;
use eggos_toolkit::geometry::Rect;

/// The window's element.
pub const ROOT_ELEMENT: u32 = 1;
/// The first view's element; the others follow in pre-order.
pub const FIRST_VIEW_ELEMENT: u32 = 2;
const MAX_DEPTH: usize = 32;

/// Answers `query` for the app's window (or the About panel) it names.
pub fn answer(query: &Query) {
    answer_with(query, true);
}

/// Answers `query` while an alert runs: trees as usual, actions refused.
pub fn answer_while_modal(query: &Query) {
    answer_with(query, false);
}

fn answer_with(query: &Query, may_act: bool) {
    if about::is_panel(query.window) {
        about::answer(query);
        return;
    }
    let window = application::window_with_server_id(query.window);
    if query.kind == query_kind::TREE {
        match window {
            Some(window) => answer_tree(query.serial, window),
            None => server::answer_query(query.serial, tree_answer::PIXELS_ONLY, &[]),
        }
        return;
    }
    let outcome = match (window, query.action()) {
        (None, _) => Outcome::NoSuchElement,
        (Some(_), None) => Outcome::NotSupported,
        (Some(_), Some(_)) if !may_act => Outcome::Disabled,
        (Some(window), Some(action)) => act(query.serial, window, query.element, action),
    };
    server::answer_query(query.serial, outcome.value(), &[]);
}

fn answer_tree(serial: u32, window: Id) {
    let block = heap::allocate(MAX_TREE_BYTES, false);
    if block.is_null() {
        server::answer_query(serial, tree_answer::PIXELS_ONLY, &[]);
        return;
    }
    // SAFETY: a new block of `MAX_TREE_BYTES` bytes, this function's until it is freed below.
    let bytes = unsafe { core::slice::from_raw_parts_mut(block, MAX_TREE_BYTES) };
    let length = {
        let mut tree = TreeWriter::new(&mut *bytes);
        describe(window, &mut tree);
        tree.bytes().len()
    };
    server::answer_query(serial, tree_answer::TREE, &bytes[..length]);
    heap::release(block);
}

fn copied<'a>(text: &str, out: &'a mut [u8; MAX_LABEL_BYTES]) -> &'a [u8] {
    let mut length = text.len().min(MAX_LABEL_BYTES);
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    out[..length].copy_from_slice(&text.as_bytes()[..length]);
    &out[..length]
}

fn describe(window: Id, tree: &mut TreeWriter<'_>) {
    let (width, height, title_object, content) = {
        let fields = window::fields(window);
        (
            fields.buffer.width as i32,
            fields.buffer.height as i32,
            fields.title,
            fields.content_view,
        )
    };
    let mut title = [0u8; MAX_LABEL_BYTES];
    let length = super::with_text(title_object, |text| {
        let text = if text.is_empty() {
            super::app_name()
        } else {
            text
        };
        copied(text, &mut title).len()
    });
    let root = Element::new(
        ROOT_ELEMENT,
        0,
        Role::Window,
        Bounds::new(0, 0, width, height),
    )
    .named(&title[..length]);
    if !tree.push(&root) || content.is_null() {
        return;
    }
    let mut next = FIRST_VIEW_ELEMENT;
    push_view(window, content, ROOT_ELEMENT, &mut next, 0, tree);
}

struct Described {
    role: Role,
    flags: u16,
    name: [u8; MAX_LABEL_BYTES],
    name_length: usize,
    value: [u8; MAX_LABEL_BYTES],
    value_length: usize,
}

impl Described {
    fn new(role: Role) -> Self {
        Self {
            role,
            flags: 0,
            name: [0; MAX_LABEL_BYTES],
            name_length: 0,
            value: [0; MAX_LABEL_BYTES],
            value_length: 0,
        }
    }

    fn named(mut self, text: &str) -> Self {
        self.name_length = copied(text, &mut self.name).len();
        self
    }

    fn valued(mut self, text: &str) -> Self {
        self.value_length = copied(text, &mut self.value).len();
        self
    }
}

fn is_focused(window: Id, view: Id) -> bool {
    window::is_key(window) && window::first_responder(window) == view
}

fn classify(window: Id, view: Id) -> Described {
    if is(view, &NSBUTTON) {
        let mut described =
            control::with_title(view, |title| Described::new(Role::Button).named(title));
        if control::is_default_button(view) {
            described.flags |= flags::DEFAULT;
        }
        if !control::is_enabled_control(view) {
            described.flags |= flags::DISABLED;
        }
        return described;
    }
    if is(view, &NSTEXTFIELD) {
        let editable = control::is_editable(view);
        let mut described = control::with_field_text(view, |text| match editable {
            true => Described::new(Role::TextField).valued(text),
            false => Described::new(Role::Label).named(text),
        });
        if editable {
            described.flags |= flags::FOCUSABLE;
            if is_focused(window, view) {
                described.flags |= flags::FOCUSED;
            }
            if !control::is_enabled_control(view) {
                described.flags |= flags::DISABLED;
            }
        }
        return described;
    }
    if class::class_of(view) == (&raw const NSVIEW).cast_mut() {
        return Described::new(Role::Group);
    }
    let name = core::str::from_utf8(root::class_name(view)).unwrap_or("");
    let mut described = Described::new(Role::Graphic).named(name);
    if view::accepts_first_responder(view) {
        described.flags |= flags::FOCUSABLE;
        if is_focused(window, view) {
            described.flags |= flags::FOCUSED;
        }
    }
    described
}

fn bounds_of(view: Id, height: i32) -> Rect {
    let origin = view::origin_in_window(view);
    let frame = view::fields(view).frame;
    let in_window = Frame::new(origin.x, origin.y, frame.width, frame.height);
    match is(view, &NSBUTTON) || (is(view, &NSTEXTFIELD) && control::is_editable(view)) {
        true => paint::control_rect(in_window, height),
        false => pixel_rect(in_window, height),
    }
}

fn push_view(
    window: Id,
    view: Id,
    parent: u32,
    next: &mut u32,
    depth: usize,
    tree: &mut TreeWriter<'_>,
) -> bool {
    if depth > MAX_DEPTH || view::fields(view).hidden {
        return true;
    }
    let id = *next;
    *next += 1;
    let height = window::fields(window).buffer.height as i32;
    let described = classify(window, view);
    let rect = bounds_of(view, height);
    let element = Element::new(
        id,
        parent,
        described.role,
        Bounds::new(rect.x, rect.y, rect.width, rect.height),
    )
    .named(&described.name[..described.name_length])
    .valued(&described.value[..described.value_length])
    .flagged(described.flags);
    if !tree.push(&element) {
        return false;
    }
    let mut index = 0;
    while let Some(&child) = view::fields(view).subviews.as_slice().get(index) {
        if !push_view(window, child, id, next, depth + 1, tree) {
            return false;
        }
        index += 1;
    }
    true
}

fn find_view(view: Id, wanted: u32, next: &mut u32, depth: usize) -> Option<Id> {
    if depth > MAX_DEPTH || view::fields(view).hidden {
        return None;
    }
    if *next == wanted {
        return Some(view);
    }
    *next += 1;
    let mut index = 0;
    while let Some(&child) = view::fields(view).subviews.as_slice().get(index) {
        if let Some(found) = find_view(child, wanted, next, depth + 1) {
            return Some(found);
        }
        index += 1;
    }
    None
}

fn view_with_id(window: Id, element: u32) -> Option<Id> {
    let content = window::fields(window).content_view;
    if content.is_null() || element < FIRST_VIEW_ELEMENT {
        return None;
    }
    let mut next = FIRST_VIEW_ELEMENT;
    find_view(content, element, &mut next, 0)
}

fn act(serial: u32, window: Id, element: u32, action: Action) -> Outcome {
    if element == ROOT_ELEMENT {
        return Outcome::NotSupported;
    }
    let Some(view) = view_with_id(window, element) else {
        return Outcome::NoSuchElement;
    };
    let is_button = is(view, &NSBUTTON);
    let is_field = is(view, &NSTEXTFIELD) && control::is_editable(view);
    let enabled = !(is_button || is_field) || control::is_enabled_control(view);
    match action {
        Action::Invoke if is_button => {
            if !enabled {
                return Outcome::Disabled;
            }
            control::perform_click(view);
            Outcome::Done
        }
        Action::Focus if is_field || (!is_button && view::accepts_first_responder(view)) => {
            if !enabled {
                return Outcome::Disabled;
            }
            window::make_first_responder(window, view);
            Outcome::Done
        }
        Action::SetText if is_field => {
            if !enabled {
                return Outcome::Disabled;
            }
            let mut text = [0u8; MAX_TEXT_BYTES];
            let Ok(length) = server::query_text(serial, &mut text) else {
                return Outcome::NotSupported;
            };
            let Ok(text) = core::str::from_utf8(&text[..length]) else {
                return Outcome::NotSupported;
            };
            send1(view, sel!("setStringValue:"), super::string(text) as usize);
            Outcome::Done
        }
        _ => Outcome::NotSupported,
    }
}

//! NSControl, NSButton and NSTextField (`docs/ux/macos-gui-v0.3.md` sections 2 and 3): a push
//! button drawn as the toolkit's, which sends its action when a press is released inside it; an
//! editable text field drawn and edited as the toolkit's, which sends its action on Enter; and a
//! label, plain text in its frame.

use super::event;
use super::{
    NSBUTTON, NSBUTTON_META, NSCONTROL, NSCONTROL_META, NSTEXTFIELD, NSTEXTFIELD_META, NSVIEW,
    application, drawing, is, store, store_copy, string, view, window, with_text,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::{Runtime, methods};
use eggos_macos_gui::geometry::{Frame, Point};
use eggos_macos_gui::keys::modifier;
use eggos_macos_gui::paint;
use eggos_toolkit::input::key;
use eggos_toolkit::text::Text;
use eggos_toolkit::widget::{self, Align, Escape, Field, FieldKey, Look};

const RETURN: &str = "\r";
const TEXT_CAPACITY: usize = 255;
const TITLE_CAPACITY: usize = 32;
const ALIGN_RIGHT: usize = 1;
const ALIGN_CENTRE: usize = 2;

/// A control's fields, which NSButton's and NSTextField's start with.
#[repr(C)]
#[derive(Debug)]
pub struct ControlFields {
    view: view::ViewFields,
    target: Id,
    action: usize,
    tag: isize,
    disabled: bool,
}

#[repr(C)]
#[derive(Debug)]
struct ButtonFields {
    control: ControlFields,
    title: Id,
    key_equivalent: Id,
    key_mask: u64,
    pressed: bool,
    inside: bool,
}

#[repr(C)]
#[derive(Debug)]
struct TextFieldFields {
    control: ControlFields,
    text: Text<TEXT_CAPACITY>,
    caret: usize,
    scroll: i32,
    placeholder: Id,
    placeholder_copy: Buffer<u8>,
    text_colour: Id,
    alignment: usize,
    read_only: bool,
    plain: bool,
}

fn control(object: Id) -> &'static mut ControlFields {
    // SAFETY: the callers pass NSControl instances (NSButton, NSTextField or a subclass's).
    unsafe { &mut *object.cast::<ControlFields>() }
}

fn button(object: Id) -> &'static mut ButtonFields {
    // SAFETY: the callers pass NSButton instances.
    unsafe { &mut *object.cast::<ButtonFields>() }
}

fn text_field(object: Id) -> &'static mut TextFieldFields {
    // SAFETY: the callers pass NSTextField instances.
    unsafe { &mut *object.cast::<TextFieldFields>() }
}

fn changed(object: Id) {
    let window = view::fields(object).window;
    if !window.is_null() {
        window::mark_dirty(window);
    }
}

/// Sends `object`'s action to its target (along the responder chain when it has none).
pub fn send_action(object: Id) -> bool {
    let fields = control(object);
    if fields.disabled || fields.action == 0 {
        return false;
    }
    application::send_action(fields.action as Sel, fields.target, object)
}

extern "C" fn target(this: Id, _: Sel) -> Id {
    control(this).target
}

extern "C" fn set_target(this: Id, _: Sel, target: Id) {
    control(this).target = target;
}

extern "C" fn action(this: Id, _: Sel) -> Sel {
    control(this).action as Sel
}

extern "C" fn set_action(this: Id, _: Sel, action: Sel) {
    control(this).action = action as usize;
}

extern "C" fn tag(this: Id, _: Sel) -> isize {
    control(this).tag
}

extern "C" fn set_tag(this: Id, _: Sel, tag: isize) {
    control(this).tag = tag;
}

extern "C" fn is_enabled(this: Id, _: Sel) -> u8 {
    u8::from(!control(this).disabled)
}

extern "C" fn set_enabled(this: Id, _: Sel, enabled: u8) {
    control(this).disabled = enabled == 0;
    changed(this);
}

extern "C" fn send_action_to(this: Id, _: Sel, action: Sel, target: Id) -> u8 {
    u8::from(application::send_action(action, target, this))
}

extern "C" fn empty_string(_: Id, _: Sel) -> Id {
    string("")
}

extern "C" fn ignore_object(_: Id, _: Sel, _: Id) {}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

fn location_in(object: Id, event: Id) -> Point {
    let location = event::fields(event).location;
    let origin = view::origin_in_window(object);
    Point::new(location.x - origin.x, location.y - origin.y)
}

fn inside(object: Id, event: Id) -> bool {
    view::fields(object)
        .frame
        .at_origin()
        .contains(location_in(object, event))
}

/// Whether the control `object` takes clicks and typing (it is not disabled).
pub fn is_enabled_control(object: Id) -> bool {
    !control(object).disabled
}

/// Runs `work` with the title of the button `object`.
pub fn with_title<R>(object: Id, work: impl FnOnce(&str) -> R) -> R {
    with_text(button(object).title, work)
}

/// Runs `work` with the text of the text field `object`.
pub fn with_field_text<R>(object: Id, work: impl FnOnce(&str) -> R) -> R {
    work(text_field(object).text.as_str())
}

/// Whether `object` is a button whose key equivalent is Return: its window's default button.
pub fn is_default_button(object: Id) -> bool {
    is(object, &NSBUTTON) && with_text(button(object).key_equivalent, |key| key == RETURN)
}

/// Clicks `object` (a button): its action, as `performClick:`.
pub fn perform_click(object: Id) {
    send_action(object);
}

extern "C" fn button_with_title(this: Id, _: Sel, title: Id, target: Id, action: Sel) -> Id {
    let object = view::new_with_frame_of(this, Frame::default());
    if object.is_null() {
        return object;
    }
    store_copy(&mut button(object).title, title);
    control(object).target = target;
    control(object).action = action as usize;
    crate::foundation::autoreleased(object)
}

extern "C" fn button_init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    store_copy(&mut button(this).title, string("Button"));
    this
}

extern "C" fn title(this: Id, _: Sel) -> Id {
    match button(this).title {
        none if none.is_null() => string(""),
        title => title,
    }
}

extern "C" fn set_title(this: Id, _: Sel, title: Id) {
    store_copy(&mut button(this).title, title);
    changed(this);
}

extern "C" fn key_equivalent(this: Id, _: Sel) -> Id {
    match button(this).key_equivalent {
        none if none.is_null() => string(""),
        key => key,
    }
}

extern "C" fn set_key_equivalent(this: Id, _: Sel, key: Id) {
    store_copy(&mut button(this).key_equivalent, key);
    changed(this);
}

extern "C" fn key_mask(this: Id, _: Sel) -> u64 {
    button(this).key_mask
}

extern "C" fn set_key_mask(this: Id, _: Sel, mask: u64) {
    button(this).key_mask = mask;
}

extern "C" fn perform_click_method(this: Id, _: Sel, _: Id) {
    perform_click(this);
}

extern "C" fn button_mouse_down(this: Id, _: Sel, _: Id) {
    if control(this).disabled {
        return;
    }
    let fields = button(this);
    fields.pressed = true;
    fields.inside = true;
    changed(this);
}

extern "C" fn button_mouse_dragged(this: Id, _: Sel, event: Id) {
    let now_inside = inside(this, event);
    let fields = button(this);
    if fields.pressed && fields.inside != now_inside {
        fields.inside = now_inside;
        changed(this);
    }
}

extern "C" fn button_mouse_up(this: Id, _: Sel, event: Id) {
    let fields = button(this);
    let clicked = fields.pressed && inside(this, event);
    fields.pressed = false;
    fields.inside = false;
    changed(this);
    if clicked {
        send_action(this);
    }
}

extern "C" fn button_draw_rect(this: Id, _: Sel, _: Frame) {
    let size = view::fields(this).frame;
    let enabled = !control(this).disabled;
    let fields = button(this);
    let pressed = fields.pressed && fields.inside;
    let mut title = Text::<TITLE_CAPACITY>::new();
    with_text(fields.title, |text| title.set(text));
    let primary = is_default_button(this);
    drawing::with_canvas(|canvas, height, origin, _, _| {
        paint::button(
            canvas,
            height,
            Frame::new(origin.x, origin.y, size.width, size.height),
            title.as_str(),
            primary,
            Look {
                pressed: pressed.then_some(0),
                enabled,
                ..Look::default()
            },
        );
    });
}

/// Whether `object` (a text field) takes typing: editable and drawn as a field.
pub fn is_editable(object: Id) -> bool {
    let fields = text_field(object);
    !fields.read_only && !fields.plain
}

fn placeholder_text(fields: &TextFieldFields) -> &'static str {
    let copy = fields.placeholder_copy.as_slice();
    // SAFETY: the copy is the field's own until its placeholder changes or the field is freed;
    // every `Field` made from it lives only within the method that made it, before either.
    let bytes = unsafe { core::slice::from_raw_parts(copy.as_ptr(), copy.len()) };
    core::str::from_utf8(bytes).unwrap_or("")
}

fn field_of(fields: &TextFieldFields) -> Field {
    Field {
        text: fields.text,
        caret: fields.caret,
        scroll: fields.scroll,
        placeholder: placeholder_text(fields),
        escape: Escape::Clear,
    }
}

fn keep_field(object: Id, field: &Field) {
    let fields = text_field(object);
    fields.text = field.text;
    fields.caret = field.caret;
    fields.scroll = field.scroll;
}

fn control_rect(object: Id) -> eggos_toolkit::geometry::Rect {
    let frame = view::fields(object).frame;
    let window = view::fields(object).window;
    let height = if window.is_null() {
        0
    } else {
        window::fields(window).buffer.height as i32
    };
    let origin = view::origin_in_window(object);
    paint::control_rect(
        Frame::new(origin.x, origin.y, frame.width, frame.height),
        height,
    )
}

fn set_text(object: Id, text: &str) {
    let fields = text_field(object);
    fields.text.set(text);
    fields.caret = fields.text.len();
    fields.scroll = 0;
    let mut field = field_of(fields);
    widget::keep_caret_visible(&mut field, control_rect(object));
    keep_field(object, &field);
    changed(object);
}

fn new_text_field(class: Id, text: Id, label: bool) -> Id {
    let object = view::new_with_frame_of(class, Frame::default());
    if object.is_null() {
        return object;
    }
    with_text(text, |text| set_text(object, text));
    if label {
        let fields = text_field(object);
        fields.read_only = true;
        fields.plain = true;
    }
    crate::foundation::autoreleased(object)
}

extern "C" fn text_field_with_string(this: Id, _: Sel, text: Id) -> Id {
    new_text_field(this, text, false)
}

extern "C" fn label_with_string(this: Id, _: Sel, text: Id) -> Id {
    new_text_field(this, text, true)
}

extern "C" fn string_value(this: Id, _: Sel) -> Id {
    string(text_field(this).text.as_str())
}

extern "C" fn set_string_value(this: Id, _: Sel, text: Id) {
    with_text(text, |text| set_text(this, text));
}

extern "C" fn placeholder_string(this: Id, _: Sel) -> Id {
    text_field(this).placeholder
}

extern "C" fn set_placeholder_string(this: Id, _: Sel, placeholder: Id) {
    store_copy(&mut text_field(this).placeholder, placeholder);
    let copy = &mut text_field(this).placeholder_copy;
    copy.free();
    with_text(placeholder, |text| {
        if copy.reserve(text.len()) {
            for &byte in text.as_bytes() {
                copy.push(byte);
            }
        }
    });
    changed(this);
}

extern "C" fn is_field_editable(this: Id, _: Sel) -> u8 {
    u8::from(!text_field(this).read_only)
}

extern "C" fn set_field_editable(this: Id, _: Sel, editable: u8) {
    text_field(this).read_only = editable == 0;
    changed(this);
}

extern "C" fn is_bezeled(this: Id, _: Sel) -> u8 {
    u8::from(!text_field(this).plain)
}

extern "C" fn set_bezeled(this: Id, _: Sel, bezeled: u8) {
    text_field(this).plain = bezeled == 0;
    changed(this);
}

extern "C" fn text_colour(this: Id, _: Sel) -> Id {
    text_field(this).text_colour
}

extern "C" fn set_text_colour(this: Id, _: Sel, colour: Id) {
    store(&mut text_field(this).text_colour, colour);
    changed(this);
}

extern "C" fn alignment(this: Id, _: Sel) -> usize {
    text_field(this).alignment
}

extern "C" fn set_alignment(this: Id, _: Sel, alignment: usize) {
    text_field(this).alignment = alignment;
    changed(this);
}

extern "C" fn accepts_first_responder(this: Id, _: Sel) -> u8 {
    u8::from(takes_typing(this))
}

extern "C" fn button_dealloc(this: Id, _: Sel) {
    let fields = button(this);
    store(&mut fields.title, core::ptr::null_mut());
    store(&mut fields.key_equivalent, core::ptr::null_mut());
    view::dispose(this);
}

extern "C" fn field_dealloc(this: Id, _: Sel) {
    let fields = text_field(this);
    store(&mut fields.placeholder, core::ptr::null_mut());
    store(&mut fields.text_colour, core::ptr::null_mut());
    fields.placeholder_copy.free();
    view::dispose(this);
}

fn takes_typing(object: Id) -> bool {
    is_editable(object) && !control(object).disabled
}

extern "C" fn field_mouse_down(this: Id, _: Sel, event: Id) {
    if !takes_typing(this) {
        return;
    }
    let rect = control_rect(this);
    let window = view::fields(this).window;
    let height = if window.is_null() {
        0
    } else {
        window::fields(window).buffer.height
    };
    let location = event::fields(event).location;
    let (x, _) = eggos_macos_gui::geometry::pixel_of_point(location, height as i32);
    let mut field = field_of(text_field(this));
    field.caret = widget::caret_at(&field, rect, x);
    widget::keep_caret_visible(&mut field, rect);
    keep_field(this, &field);
    changed(this);
}

extern "C" fn field_key_down(this: Id, _: Sel, event: Id) {
    if !takes_typing(this) {
        return;
    }
    let fields_of_event = event::fields(event);
    let code = fields_of_event.linux_code;
    let command =
        fields_of_event.flags & (modifier::COMMAND | modifier::CONTROL | modifier::OPTION) != 0;
    if code == key::ENTER && !command {
        send_action(this);
        return;
    }
    if code == key::ESC {
        return;
    }
    let mut field = field_of(text_field(this));
    let before = field.text;
    match widget::field_key(
        &mut field,
        code,
        fields_of_event.character,
        command,
        &before,
    ) {
        FieldKey::Unused => {}
        FieldKey::Moved | FieldKey::Changed => {
            widget::keep_caret_visible(&mut field, control_rect(this));
            keep_field(this, &field);
            changed(this);
        }
    }
}

extern "C" fn field_draw_rect(this: Id, _: Sel, _: Frame) {
    let size = view::fields(this).frame;
    let window = view::fields(this).window;
    let focused =
        !window.is_null() && window::is_key(window) && window::first_responder(window) == this;
    let enabled = !control(this).disabled;
    let fields = text_field(this);
    let field = field_of(fields);
    let label = fields.read_only || fields.plain;
    let colour =
        drawing::colour_of(fields.text_colour).unwrap_or(eggos_macos_gui::colour::DEFAULT_TEXT);
    let align = match fields.alignment {
        ALIGN_RIGHT => Align::End,
        ALIGN_CENTRE => Align::Centre,
        _ => Align::Start,
    };
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let frame = Frame::new(origin.x, origin.y, size.width, size.height);
        match label {
            true => paint::label(canvas, height, frame, field.text.as_str(), colour, align),
            false => paint::text_field(
                canvas,
                height,
                frame,
                &field,
                Look {
                    focused,
                    enabled,
                    caret_shown: true,
                    ..Look::default()
                },
            ),
        }
    });
}

pub fn define(runtime: &mut Runtime) -> bool {
    let control_methods = methods![
        "target" => target,
        "setTarget:" => set_target,
        "action" => action,
        "setAction:" => set_action,
        "tag" => tag,
        "setTag:" => set_tag,
        "isEnabled" => is_enabled,
        "setEnabled:" => set_enabled,
        "sendAction:to:" => send_action_to,
        "stringValue" => empty_string,
        "setFont:" => ignore_object,
        "setControlSize:" => ignore_word,
    ];
    let button_instance = methods![
        "initWithFrame:" => button_init_with_frame,
        "dealloc" => button_dealloc,
        "title" => title,
        "setTitle:" => set_title,
        "stringValue" => title,
        "keyEquivalent" => key_equivalent,
        "setKeyEquivalent:" => set_key_equivalent,
        "keyEquivalentModifierMask" => key_mask,
        "setKeyEquivalentModifierMask:" => set_key_mask,
        "performClick:" => perform_click_method,
        "setBezelStyle:" => ignore_word,
        "setButtonType:" => ignore_word,
        "mouseDown:" => button_mouse_down,
        "mouseDragged:" => button_mouse_dragged,
        "mouseUp:" => button_mouse_up,
        "drawRect:" => button_draw_rect,
    ];
    let button_class = methods![
        "buttonWithTitle:target:action:" => button_with_title,
    ];
    let field_instance = methods![
        "dealloc" => field_dealloc,
        "stringValue" => string_value,
        "setStringValue:" => set_string_value,
        "placeholderString" => placeholder_string,
        "setPlaceholderString:" => set_placeholder_string,
        "isEditable" => is_field_editable,
        "setEditable:" => set_field_editable,
        "isBezeled" => is_bezeled,
        "setBezeled:" => set_bezeled,
        "setBordered:" => set_bezeled,
        "setDrawsBackground:" => ignore_word,
        "setSelectable:" => ignore_word,
        "setBackgroundColor:" => ignore_object,
        "textColor" => text_colour,
        "setTextColor:" => set_text_colour,
        "alignment" => alignment,
        "setAlignment:" => set_alignment,
        "acceptsFirstResponder" => accepts_first_responder,
        "mouseDown:" => field_mouse_down,
        "keyDown:" => field_key_down,
        "drawRect:" => field_draw_rect,
    ];
    let field_class = methods![
        "textFieldWithString:" => text_field_with_string,
        "labelWithString:" => label_with_string,
    ];
    super::define(
        runtime,
        b"NSControl\0",
        (&NSCONTROL, &NSCONTROL_META),
        &NSVIEW,
        core::mem::size_of::<ControlFields>(),
        &control_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSButton\0",
        (&NSBUTTON, &NSBUTTON_META),
        &NSCONTROL,
        core::mem::size_of::<ButtonFields>(),
        &button_instance,
        &button_class,
    ) && super::define(
        runtime,
        b"NSTextField\0",
        (&NSTEXTFIELD, &NSTEXTFIELD_META),
        &NSCONTROL,
        core::mem::size_of::<TextFieldFields>(),
        &field_instance,
        &field_class,
    )
}

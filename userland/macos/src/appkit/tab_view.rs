//! NSTabView, NSTabViewItem and NSTabViewController (D751, docs/ux/macos-containers-v0.1.md
//! sections 1 and 3): a tab view draws its centred segmented strip and its content box
//! (`eggos_macos_gui::tab_view` lays them out for each `tabViewType`), shows only the selected
//! item's view, sized to the content rect, and selects by a click on a segment, by every
//! `select...` method and by Ctrl+Tab and Ctrl+Shift+Tab from anywhere in its window, with the
//! delegate's should, will and did messages. A tab view controller owns a tab view filling its
//! view, wraps each child controller's view in an item labelled by the child's title, and sends
//! the appearance calls to the child shown and the one hidden.

use super::{
    NSTABVIEW, NSTABVIEW_META, NSTABVIEWCONTROLLER, NSTABVIEWCONTROLLER_META, NSTABVIEWITEM,
    NSTABVIEWITEM_META, NSTRACKINGAREA, NSVIEW, NSVIEWCONTROLLER, drawing, event, responds,
    store, view, window, with_text,
};
use crate::foundation::{NSMUTABLEARRAY, autoreleased, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, message, methods, sel, send, send_bool, send1, send2};
use alloc::vec::Vec;
use eggos_macos_gui::geometry::{Frame, pixel_rect, round};
use eggos_macos_gui::tab_view::{
    self as geometry, BoxLook, Layout, TAB_STRIP_HEIGHT, kind, label_place,
};
use eggos_toolkit::canvas::INSET_R4;
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::widget::{clipped_text, focus_ring};

const NO_INDEX: isize = -1;
const GLYPH_WIDTH: i32 = 8;
const TRACK_ENTERED_AND_EXITED: usize = 0x01;
const TRACK_MOUSE_MOVED: usize = 0x02;
const TRACK_ACTIVE_ALWAYS: usize = 0x80;
const TRACK_IN_VISIBLE_RECT: usize = 0x200;
const TOOLBAR_BAND: i32 = 36;
const TOOLBAR_STRIP_TOP: i32 = 4;

/// `NSTabViewControllerTabStyle`'s values.
mod tab_style {
    pub const SEGMENTED_ON_TOP: isize = 0;
    pub const SEGMENTED_ON_BOTTOM: isize = 1;
    pub const TOOLBAR: isize = 2;
    pub const UNSPECIFIED: isize = -1;
}

#[repr(C)]
#[derive(Debug)]
struct TabViewFields {
    view: view::ViewFields,
    items: Id,
    selected: isize,
    kind: usize,
    delegate: Id,
    draws_background: bool,
    hover: isize,
    pressed: isize,
    font: Id,
    control_size: usize,
    tracking: Id,
    toolbar_band: bool,
}

#[repr(C)]
#[derive(Debug)]
struct ItemFields {
    isa: usize,
    label: Id,
    view: Id,
    identifier: Id,
    tab_view: Id,
    view_controller: Id,
    tool_tip: Id,
    image: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ControllerFields {
    base: super::controllers::ViewControllerFields,
    tab_view: Id,
    tab_style: isize,
    transition_options: usize,
}

fn tab_view(object: Id) -> &'static mut TabViewFields {
    // SAFETY: the callers pass NSTabView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<TabViewFields>() }
}

fn item(object: Id) -> &'static mut ItemFields {
    // SAFETY: the callers pass NSTabViewItem instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ItemFields>() }
}

fn controller(object: Id) -> &'static mut ControllerFields {
    // SAFETY: the callers pass NSTabViewController instances, whose memory starts with these.
    unsafe { &mut *object.cast::<ControllerFields>() }
}

fn items_of(this: Id) -> Vec<Id> {
    let list = tab_view(this).items;
    if list.is_null() {
        return Vec::new();
    }
    let count = send(list, sel!("count")) as usize;
    (0..count)
        .map(|index| send1(list, sel!("objectAtIndex:"), index))
        .collect()
}

fn list(this: Id) -> Id {
    let fields = tab_view(this);
    if fields.items.is_null() {
        fields.items = send(new_instance(class_pointer(&NSMUTABLEARRAY)), sel!("init"));
    }
    fields.items
}

fn label_width(label: Id) -> i32 {
    match label.is_null() {
        true => 0,
        false => with_text(label, |text| text.chars().count() as i32 * GLYPH_WIDTH),
    }
}

fn own_rect(this: Id) -> Rect {
    let frame = view::fields(this).frame;
    let top = match tab_view(this).toolbar_band {
        true => TOOLBAR_BAND - TOOLBAR_STRIP_TOP,
        false => 0,
    };
    Rect::new(0, top, round(frame.width), round(frame.height) - top)
}

fn layout_of(this: Id) -> Layout {
    let widths: Vec<i32> = items_of(this)
        .into_iter()
        .map(|each| label_width(item(each).label))
        .collect();
    geometry::layout(own_rect(this), tab_view(this).kind, &widths, TAB_STRIP_HEIGHT)
}

fn changed(this: Id) {
    send1(this, sel!("setNeedsDisplay:"), 1);
}

fn place_selected(this: Id) {
    let Some(shown) = selected_item(this) else {
        return;
    };
    let content = item(shown).view;
    if content.is_null() {
        return;
    }
    let height = round(view::fields(this).frame.height);
    let rect = layout_of(this).content;
    let frame = Frame::new(
        f64::from(rect.x),
        f64::from(height - rect.y - rect.height),
        f64::from(rect.width.max(0)),
        f64::from(rect.height.max(0)),
    );
    if view::fields(content).frame != frame {
        super::send_frame(content, sel!("setFrame:"), frame);
    }
    if view::fields(content).superview != this {
        send1(this, sel!("addSubview:"), content as usize);
    }
}

fn selected_item(this: Id) -> Option<Id> {
    let index = tab_view(this).selected;
    match index >= 0 {
        true => items_of(this).get(index as usize).copied(),
        false => None,
    }
}

fn notify_count_changed(this: Id) {
    let delegate = tab_view(this).delegate;
    let changed_sel = sel!("tabViewDidChangeNumberOfTabViewItems:");
    if responds(delegate, changed_sel) {
        send1(delegate, changed_sel, this as usize);
    }
}

fn select_index(this: Id, index: usize) -> bool {
    let all = items_of(this);
    let Some(&chosen) = all.get(index) else {
        return false;
    };
    if tab_view(this).selected == index as isize {
        return true;
    }
    let delegate = tab_view(this).delegate;
    let should = sel!("tabView:shouldSelectTabViewItem:");
    if responds(delegate, should)
        && send2(delegate, should, this as usize, chosen as usize) as u8 == 0
    {
        return false;
    }
    let will = sel!("tabView:willSelectTabViewItem:");
    if responds(delegate, will) {
        send2(delegate, will, this as usize, chosen as usize);
    }
    if let Some(old) = selected_item(this) {
        let old_view = item(old).view;
        if !old_view.is_null() && view::fields(old_view).superview == this {
            send(old_view, sel!("removeFromSuperview"));
        }
    }
    tab_view(this).selected = index as isize;
    place_selected(this);
    keep_responder(this);
    changed(this);
    let did = sel!("tabView:didSelectTabViewItem:");
    if responds(delegate, did) {
        send2(delegate, did, this as usize, chosen as usize);
    }
    true
}

fn keep_responder(this: Id) {
    let owner = view::fields(this).window;
    if owner.is_null() {
        return;
    }
    let responder = window::first_responder(owner);
    let hidden_away =
        root::is_kind_of(responder, &NSVIEW) && !view::holds(owner_root(owner), responder);
    if hidden_away {
        send1(owner, sel!("makeFirstResponder:"), this as usize);
    }
}

fn owner_root(owner: Id) -> Id {
    window::fields(owner).content_view
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    let fields = tab_view(this);
    fields.selected = NO_INDEX;
    fields.hover = NO_INDEX;
    fields.pressed = NO_INDEX;
    fields.draws_background = true;
    fields.kind = kind::TOP;
    this
}

extern "C" fn tab_dealloc(this: Id, _: Sel) {
    for each in items_of(this) {
        item(each).tab_view = core::ptr::null_mut();
    }
    let fields = tab_view(this);
    for slot in [&mut fields.items, &mut fields.font, &mut fields.tracking] {
        store(slot, core::ptr::null_mut());
    }
    view::dispose(this);
}

extern "C" fn tab_view_items(this: Id, _: Sel) -> Id {
    list(this)
}

extern "C" fn number_of_items(this: Id, _: Sel) -> usize {
    items_of(this).len()
}

extern "C" fn insert_item(this: Id, _: Sel, added: Id, index: usize) {
    if added.is_null() {
        return;
    }
    let all = list(this);
    let at = index.min(items_of(this).len());
    send2(all, sel!("insertObject:atIndex:"), added as usize, at);
    item(added).tab_view = this;
    let fields = tab_view(this);
    if fields.selected >= at as isize && fields.selected != NO_INDEX {
        fields.selected += 1;
    }
    notify_count_changed(this);
    if tab_view(this).selected == NO_INDEX {
        select_index(this, 0);
    }
    changed(this);
}

extern "C" fn add_item(this: Id, sel: Sel, added: Id) {
    let end = items_of(this).len();
    insert_item(this, sel, added, end);
}

extern "C" fn remove_item(this: Id, _: Sel, removed: Id) {
    let Some(index) = items_of(this).iter().position(|&each| each == removed) else {
        return;
    };
    objc_retain(removed);
    let was_selected = tab_view(this).selected == index as isize;
    let content = item(removed).view;
    if !content.is_null() && view::fields(content).superview == this {
        send(content, sel!("removeFromSuperview"));
    }
    send1(list(this), sel!("removeObjectAtIndex:"), index);
    item(removed).tab_view = core::ptr::null_mut();
    let remaining = items_of(this).len();
    let fields = tab_view(this);
    if was_selected {
        fields.selected = NO_INDEX;
        if let Some(next) = geometry::after_removal(index, remaining) {
            select_index(this, next);
        }
    } else if fields.selected > index as isize {
        fields.selected -= 1;
    }
    objc_release(removed);
    notify_count_changed(this);
    changed(this);
}

extern "C" fn index_of_item(this: Id, _: Sel, wanted: Id) -> isize {
    items_of(this)
        .iter()
        .position(|&each| each == wanted)
        .map_or(isize::MAX, |index| index as isize)
}

extern "C" fn index_with_identifier(this: Id, _: Sel, identifier: Id) -> isize {
    items_of(this)
        .iter()
        .position(|&each| {
            let own = item(each).identifier;
            own == identifier
                || (!own.is_null() && send_bool(own, sel!("isEqual:"), identifier as usize))
        })
        .map_or(isize::MAX, |index| index as isize)
}

extern "C" fn item_at(this: Id, _: Sel, index: usize) -> Id {
    items_of(this)
        .get(index)
        .copied()
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn selected(this: Id, _: Sel) -> Id {
    selected_item(this).unwrap_or(core::ptr::null_mut())
}

extern "C" fn select_item(this: Id, _: Sel, chosen: Id) {
    if let Some(index) = items_of(this).iter().position(|&each| each == chosen) {
        select_index(this, index);
    }
}

extern "C" fn select_at(this: Id, _: Sel, index: usize) {
    select_index(this, index);
}

extern "C" fn select_with_identifier(this: Id, sel: Sel, identifier: Id) {
    let index = index_with_identifier(this, sel, identifier);
    if index != isize::MAX {
        select_index(this, index as usize);
    }
}

extern "C" fn select_first(this: Id, _: Sel, _sender: Id) {
    select_index(this, 0);
}

extern "C" fn select_last(this: Id, _: Sel, _sender: Id) {
    let count = items_of(this).len();
    if count > 0 {
        select_index(this, count - 1);
    }
}

fn step(this: Id, forward: bool) {
    let count = items_of(this).len();
    if count == 0 {
        return;
    }
    let current = tab_view(this).selected.max(0) as usize;
    select_index(this, geometry::next_index(current, count, forward));
}

extern "C" fn select_next(this: Id, _: Sel, _sender: Id) {
    step(this, true);
}

extern "C" fn select_previous(this: Id, _: Sel, _sender: Id) {
    step(this, false);
}

extern "C" fn delegate(this: Id, _: Sel) -> Id {
    tab_view(this).delegate
}

extern "C" fn set_delegate(this: Id, _: Sel, delegate: Id) {
    tab_view(this).delegate = delegate;
}

extern "C" fn tab_view_type(this: Id, _: Sel) -> usize {
    tab_view(this).kind
}

extern "C" fn set_tab_view_type(this: Id, _: Sel, kind_number: usize) {
    tab_view(this).kind = kind_number.min(kind::NO_TABS_NO_BORDER);
    place_selected(this);
    changed(this);
}

extern "C" fn draws_background(this: Id, _: Sel) -> u8 {
    u8::from(tab_view(this).draws_background)
}

extern "C" fn set_draws_background(this: Id, _: Sel, flag: u8) {
    tab_view(this).draws_background = flag != 0;
    changed(this);
}

extern "C" fn font(this: Id, _: Sel) -> Id {
    tab_view(this).font
}

extern "C" fn set_font(this: Id, _: Sel, font: Id) {
    store(&mut tab_view(this).font, font);
}

extern "C" fn control_size(this: Id, _: Sel) -> usize {
    tab_view(this).control_size
}

extern "C" fn set_control_size(this: Id, _: Sel, size: usize) {
    tab_view(this).control_size = size;
}

extern "C" fn content_rect(this: Id, _: Sel) -> Frame {
    let height = round(view::fields(this).frame.height);
    let rect = layout_of(this).content;
    Frame::new(
        f64::from(rect.x),
        f64::from(height - rect.y - rect.height),
        f64::from(rect.width),
        f64::from(rect.height),
    )
}

extern "C" fn set_frame(this: Id, _: Sel, frame: Frame) {
    view::fields(this).frame = frame;
    place_selected(this);
    changed(this);
}

extern "C" fn set_frame_size(this: Id, sel: Sel, size: eggos_macos_gui::geometry::Size) {
    let origin = view::fields(this).frame;
    set_frame(
        this,
        sel,
        Frame::new(origin.x, origin.y, size.width, size.height),
    );
}

fn local_pixel(this: Id, event_object: Id) -> (i32, i32) {
    let origin = view::origin_in_window(this);
    let location = event::fields(event_object).location;
    let height = view::fields(this).frame.height;
    (
        round(location.x - origin.x),
        round(height - (location.y - origin.y)) - 1,
    )
}

fn segment_under(this: Id, event_object: Id) -> isize {
    let (x, y) = local_pixel(this, event_object);
    layout_of(this)
        .segment_at(x, y)
        .map_or(NO_INDEX, |index| index as isize)
}

extern "C" fn mouse_down(this: Id, _: Sel, event_object: Id) {
    let under = segment_under(this, event_object);
    tab_view(this).pressed = under;
    tab_view(this).hover = under;
    changed(this);
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event_object: Id) {
    let under = segment_under(this, event_object);
    let fields = tab_view(this);
    if fields.pressed != NO_INDEX && fields.hover != under {
        fields.hover = under;
        changed(this);
    }
}

extern "C" fn mouse_up(this: Id, _: Sel, event_object: Id) {
    let pressed = tab_view(this).pressed;
    tab_view(this).pressed = NO_INDEX;
    let under = segment_under(this, event_object);
    if pressed != NO_INDEX && pressed == under {
        select_index(this, pressed as usize);
    }
    changed(this);
}

extern "C" fn mouse_moved(this: Id, _: Sel, event_object: Id) {
    let under = segment_under(this, event_object);
    if tab_view(this).hover != under {
        tab_view(this).hover = under;
        changed(this);
    }
}

extern "C" fn mouse_exited(this: Id, _: Sel, _: Id) {
    if tab_view(this).hover != NO_INDEX {
        tab_view(this).hover = NO_INDEX;
        changed(this);
    }
}

fn track_hover(this: Id) {
    if !tab_view(this).tracking.is_null() || view::fields(this).window.is_null() {
        return;
    }
    let allocated = send(class_pointer(&NSTRACKINGAREA).cast(), sel!("alloc"));
    if allocated.is_null() {
        return;
    }
    let make: extern "C" fn(Id, Sel, Frame, usize, Id, Id) -> Id = message::msg_send_as();
    let options =
        TRACK_ENTERED_AND_EXITED | TRACK_MOUSE_MOVED | TRACK_ACTIVE_ALWAYS | TRACK_IN_VISIBLE_RECT;
    let area = make(
        allocated,
        sel!("initWithRect:options:owner:userInfo:"),
        Frame::default(),
        options,
        this,
        core::ptr::null_mut(),
    );
    if area.is_null() {
        return;
    }
    send1(this, sel!("addTrackingArea:"), area as usize);
    tab_view(this).tracking = area;
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    track_hover(this);
    place_selected(this);
    let laid = layout_of(this);
    let frame = view::fields(this).frame;
    let fields = tab_view(this);
    let (selected, hover, pressed) = (fields.selected, fields.hover, fields.pressed);
    let (draws_background, band) = (fields.draws_background, fields.toolbar_band);
    let focused = {
        let owner = view::fields(this).window;
        !owner.is_null() && window::first_responder(owner) == this
    };
    let labels: Vec<Id> = items_of(this)
        .into_iter()
        .map(|each| item(each).label)
        .collect();
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let outer = pixel_rect(
            Frame::new(origin.x, origin.y, frame.width, frame.height),
            height,
        );
        let place = |rect: Rect| rect.offset(outer.x, outer.y);
        if band {
            let strip_band = Rect::new(0, 0, outer.width, TOOLBAR_BAND);
            canvas.fill(place(strip_band), colour::MENU);
            canvas.fill(
                place(Rect::new(0, TOOLBAR_BAND - 1, outer.width, 1)),
                colour::MENU_BORDER,
            );
        }
        let boxed = laid.content_box;
        match (laid.look, draws_background) {
            (BoxLook::Filled, true) => {
                canvas.rounded(place(boxed), &INSET_R4, colour::MENU);
                canvas.outline(place(boxed), &INSET_R4, 1, colour::MENU_BORDER);
            }
            (BoxLook::Filled, false) | (BoxLook::Line, _) => {
                canvas.outline(place(boxed), &INSET_R4, 1, colour::MENU_BORDER);
            }
            (BoxLook::Nothing, _) => {}
        }
        let Some(strip) = laid.strip else {
            return;
        };
        canvas.rounded(place(strip), &INSET_R4, colour::BUTTON);
        for (index, (&segment, &label)) in laid.segments().iter().zip(&labels).enumerate() {
            let index = index as isize;
            let fill = if index == selected {
                colour::SELECTION
            } else if index == pressed && hover == pressed {
                colour::BUTTON_PRESSED
            } else if index == hover && pressed == NO_INDEX {
                colour::BUTTON_HOVER
            } else {
                colour::BUTTON
            };
            canvas.fill(place(segment), fill);
            if label.is_null() {
                continue;
            }
            with_text(label, |text| {
                let width = text.chars().count() as i32 * GLYPH_WIDTH;
                let (x, y, room) = label_place(segment, width);
                let at = place(Rect::new(x, y, room, 16));
                clipped_text(canvas, at.x, at.y, room, text, colour::TEXT);
            });
        }
        let chosen = (selected >= 0).then_some(selected as usize);
        for separator in laid.separators(chosen) {
            canvas.fill(place(separator), colour::MENU_BORDER);
        }
        canvas.outline(place(strip), &INSET_R4, 1, colour::BUTTON_BORDER);
        if focused && let Some(ring) = chosen.and_then(|index| laid.ring(index)) {
            focus_ring(canvas, place(ring), 0);
        }
    });
}

extern "C" fn accepts_first_responder(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn is_flipped(_: Id, _: Sel) -> u8 {
    0
}

/// Ctrl+Tab (or Ctrl+Shift+Tab, `backwards`) in `owner`: the tab view holding the first
/// responder, else the window's first tab view, selects its next (previous) item. Whether a
/// tab view took the key.
pub fn control_tab(owner: Id, backwards: bool) -> bool {
    let root = owner_root(owner);
    if root.is_null() {
        return false;
    }
    let responder = window::first_responder(owner);
    let mut first = core::ptr::null_mut();
    let mut holding = core::ptr::null_mut();
    view::each_view(root, &mut |each| {
        if !root::is_kind_of(each, &NSTABVIEW) || view::fields(each).hidden {
            return;
        }
        if first.is_null() {
            first = each;
        }
        if !responder.is_null() && view::holds(each, responder) {
            holding = each;
        }
    });
    let chosen = match holding.is_null() {
        true => first,
        false => holding,
    };
    if chosen.is_null() {
        return false;
    }
    step(chosen, !backwards);
    true
}

extern "C" fn item_init(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn item_init_with_identifier(this: Id, _: Sel, identifier: Id) -> Id {
    if !this.is_null() {
        store(&mut item(this).identifier, identifier);
    }
    this
}

extern "C" fn item_with_view_controller(class: Id, _: Sel, child: Id) -> Id {
    let made = send(send(class, sel!("alloc")), sel!("init"));
    if made.is_null() {
        return made;
    }
    set_item_view_controller(made, sel!(""), child);
    autoreleased(made)
}

extern "C" fn item_dealloc(this: Id, _: Sel) {
    let fields = item(this);
    for slot in [
        &mut fields.label,
        &mut fields.view,
        &mut fields.identifier,
        &mut fields.view_controller,
        &mut fields.tool_tip,
        &mut fields.image,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

extern "C" fn item_label(this: Id, _: Sel) -> Id {
    item(this).label
}

extern "C" fn set_item_label(this: Id, _: Sel, label: Id) {
    store(&mut item(this).label, label);
    let owner = item(this).tab_view;
    if !owner.is_null() {
        changed(owner);
    }
}

extern "C" fn item_view(this: Id, _: Sel) -> Id {
    let fields = item(this);
    if fields.view.is_null() && !fields.view_controller.is_null() {
        let shown = send(fields.view_controller, sel!("view"));
        store(&mut fields.view, shown);
    }
    fields.view
}

extern "C" fn set_item_view(this: Id, _: Sel, shown: Id) {
    let owner = item(this).tab_view;
    let old = item(this).view;
    if !owner.is_null() && !old.is_null() && view::fields(old).superview == owner {
        send(old, sel!("removeFromSuperview"));
    }
    store(&mut item(this).view, shown);
    if !owner.is_null() {
        place_selected(owner);
        changed(owner);
    }
}

extern "C" fn item_identifier(this: Id, _: Sel) -> Id {
    item(this).identifier
}

extern "C" fn set_item_identifier(this: Id, _: Sel, identifier: Id) {
    store(&mut item(this).identifier, identifier);
}

extern "C" fn item_tab_view(this: Id, _: Sel) -> Id {
    item(this).tab_view
}

extern "C" fn item_view_controller(this: Id, _: Sel) -> Id {
    item(this).view_controller
}

extern "C" fn set_item_view_controller(this: Id, _: Sel, child: Id) {
    store(&mut item(this).view_controller, child);
    store(&mut item(this).view, core::ptr::null_mut());
    if !child.is_null() && item(this).label.is_null() {
        let title = send(child, sel!("title"));
        store(&mut item(this).label, title);
    }
}

extern "C" fn item_tool_tip(this: Id, _: Sel) -> Id {
    item(this).tool_tip
}

extern "C" fn set_item_tool_tip(this: Id, _: Sel, tip: Id) {
    store(&mut item(this).tool_tip, tip);
}

extern "C" fn item_image(this: Id, _: Sel) -> Id {
    item(this).image
}

extern "C" fn set_item_image(this: Id, _: Sel, image: Id) {
    store(&mut item(this).image, image);
}

extern "C" fn item_state(this: Id, _: Sel) -> usize {
    let owner = item(this).tab_view;
    match !owner.is_null() && selected_item(owner) == Some(this) {
        true => 0,
        false => 1,
    }
}

fn inherited(sel: Sel) -> Option<usize> {
    crate::objc::class::lookup(class_pointer(&NSVIEWCONTROLLER), sel)
}

fn insert_child_as_view_controller(this: Id, child: Id, index: isize) {
    if let Some(found) = inherited(sel!("insertChildViewController:atIndex:")) {
        // SAFETY: NSViewController's own implementation, taking exactly these arguments.
        let insert: extern "C" fn(Id, Sel, Id, isize) = unsafe { core::mem::transmute(found) };
        insert(this, sel!("insertChildViewController:atIndex:"), child, index);
    }
}

fn controller_tab_view(this: Id) -> Id {
    let fields = controller(this);
    if fields.tab_view.is_null() {
        let made = view::new_with_frame(&NSTABVIEW, Frame::new(0.0, 0.0, 480.0, 360.0));
        if made.is_null() {
            return made;
        }
        tab_view(made).delegate = this;
        fields.tab_view = made;
        apply_style(this);
    }
    fields.tab_view
}

fn apply_style(this: Id) {
    let made = controller(this).tab_view;
    if made.is_null() {
        return;
    }
    let style = controller(this).tab_style;
    tab_view(made).toolbar_band = style == tab_style::TOOLBAR;
    tab_view(made).kind = match style {
        tab_style::SEGMENTED_ON_BOTTOM => kind::BOTTOM,
        tab_style::UNSPECIFIED => kind::NO_TABS_NO_BORDER,
        _ => kind::TOP,
    };
    place_selected(made);
    changed(made);
}

extern "C" fn controller_tab_view_method(this: Id, _: Sel) -> Id {
    controller_tab_view(this)
}

extern "C" fn controller_set_tab_view(this: Id, _: Sel, made: Id) {
    store(&mut controller(this).tab_view, made);
    if !made.is_null() {
        tab_view(made).delegate = this;
        apply_style(this);
    }
}

extern "C" fn controller_load_view(this: Id, _: Sel) {
    let made = controller_tab_view(this);
    send1(this, sel!("setView:"), made as usize);
}

extern "C" fn controller_items(this: Id, _: Sel) -> Id {
    list(controller_tab_view(this))
}

extern "C" fn controller_add_item(this: Id, _: Sel, added: Id) {
    let owner = controller_tab_view(this);
    let end = items_of(owner).len();
    controller_insert_item(this, sel!(""), added, end);
}

extern "C" fn controller_insert_item(this: Id, _: Sel, added: Id, index: usize) {
    if added.is_null() {
        return;
    }
    let child = item(added).view_controller;
    if !child.is_null() {
        let children = send(this, sel!("childViewControllers"));
        if !send_bool(children, sel!("containsObject:"), child as usize) {
            insert_child_as_view_controller(this, child, index as isize);
        }
    }
    insert_item(controller_tab_view(this), sel!(""), added, index);
}

extern "C" fn controller_remove_item(this: Id, _: Sel, removed: Id) {
    if removed.is_null() {
        return;
    }
    objc_retain(removed);
    remove_item(controller_tab_view(this), sel!(""), removed);
    let child = item(removed).view_controller;
    if !child.is_null() {
        send(child, sel!("removeFromParentViewController"));
    }
    objc_release(removed);
}

extern "C" fn controller_add_child(this: Id, _: Sel, child: Id) {
    let owner = controller_tab_view(this);
    let end = items_of(owner).len();
    controller_insert_child(this, sel!(""), child, end as isize);
}

extern "C" fn controller_insert_child(this: Id, _: Sel, child: Id, index: isize) {
    if child.is_null() {
        return;
    }
    let wrapped = item_with_view_controller(
        class_pointer(&NSTABVIEWITEM).cast(),
        sel!(""),
        child,
    );
    insert_child_as_view_controller(this, child, index);
    insert_item(controller_tab_view(this), sel!(""), wrapped, index.max(0) as usize);
}

extern "C" fn controller_selected_index(this: Id, _: Sel) -> isize {
    tab_view(controller_tab_view(this)).selected
}

extern "C" fn controller_set_selected_index(this: Id, _: Sel, index: isize) {
    if index >= 0 {
        select_index(controller_tab_view(this), index as usize);
    }
}

extern "C" fn controller_tab_style(this: Id, _: Sel) -> isize {
    controller(this).tab_style
}

extern "C" fn controller_set_tab_style(this: Id, _: Sel, style: isize) {
    controller(this).tab_style = style;
    apply_style(this);
}

extern "C" fn controller_transition_options(this: Id, _: Sel) -> usize {
    controller(this).transition_options
}

extern "C" fn controller_set_transition_options(this: Id, _: Sel, options: usize) {
    controller(this).transition_options = options;
}

extern "C" fn controller_will_select(this: Id, _: Sel, owner: Id, chosen: Id) {
    if let Some(old) = selected_item(owner) {
        let child = item(old).view_controller;
        if !child.is_null() && child != item(chosen).view_controller {
            super::controllers::set_appeared(child, false);
        }
    }
}

extern "C" fn controller_did_select(this: Id, _: Sel, _owner: Id, chosen: Id) {
    let child = item(chosen).view_controller;
    let shown = send_bool(this, sel!("isViewLoaded"), 0)
        && !view::fields(send(this, sel!("view"))).window.is_null();
    if !child.is_null() && shown {
        super::controllers::set_appeared(child, true);
    }
}

extern "C" fn controller_should_select(_: Id, _: Sel, _: Id, _: Id) -> u8 {
    1
}

extern "C" fn controller_dealloc(this: Id, _: Sel) {
    let made = controller(this).tab_view;
    if !made.is_null() {
        tab_view(made).delegate = core::ptr::null_mut();
    }
    store(&mut controller(this).tab_view, core::ptr::null_mut());
    match inherited(sel!("dealloc")) {
        Some(found) => {
            // SAFETY: NSViewController's own `dealloc`, which takes only the receiver.
            let parent: extern "C" fn(Id, Sel) = unsafe { core::mem::transmute(found) };
            parent(this, sel!("dealloc"));
        }
        None => dispose(this),
    }
}

pub fn define(runtime: &mut Runtime) -> bool {
    let tab_methods = methods![
        "initWithFrame:" => init_with_frame,
        "dealloc" => tab_dealloc,
        "tabViewItems" => tab_view_items,
        "numberOfTabViewItems" => number_of_items,
        "addTabViewItem:" => add_item,
        "insertTabViewItem:atIndex:" => insert_item,
        "removeTabViewItem:" => remove_item,
        "indexOfTabViewItem:" => index_of_item,
        "indexOfTabViewItemWithIdentifier:" => index_with_identifier,
        "tabViewItemAtIndex:" => item_at,
        "selectedTabViewItem" => selected,
        "selectTabViewItem:" => select_item,
        "selectTabViewItemAtIndex:" => select_at,
        "selectTabViewItemWithIdentifier:" => select_with_identifier,
        "selectFirstTabViewItem:" => select_first,
        "selectLastTabViewItem:" => select_last,
        "selectNextTabViewItem:" => select_next,
        "selectPreviousTabViewItem:" => select_previous,
        "delegate" => delegate,
        "setDelegate:" => set_delegate,
        "tabViewType" => tab_view_type,
        "setTabViewType:" => set_tab_view_type,
        "drawsBackground" => draws_background,
        "setDrawsBackground:" => set_draws_background,
        "font" => font,
        "setFont:" => set_font,
        "controlSize" => control_size,
        "setControlSize:" => set_control_size,
        "contentRect" => content_rect,
        "setFrame:" => set_frame,
        "setFrameSize:" => set_frame_size,
        "mouseDown:" => mouse_down,
        "mouseDragged:" => mouse_dragged,
        "mouseUp:" => mouse_up,
        "mouseMoved:" => mouse_moved,
        "mouseExited:" => mouse_exited,
        "drawRect:" => draw_rect,
        "acceptsFirstResponder" => accepts_first_responder,
        "isFlipped" => is_flipped,
    ];
    let item_methods = methods![
        "init" => item_init,
        "initWithIdentifier:" => item_init_with_identifier,
        "dealloc" => item_dealloc,
        "label" => item_label,
        "setLabel:" => set_item_label,
        "view" => item_view,
        "setView:" => set_item_view,
        "identifier" => item_identifier,
        "setIdentifier:" => set_item_identifier,
        "tabView" => item_tab_view,
        "viewController" => item_view_controller,
        "setViewController:" => set_item_view_controller,
        "toolTip" => item_tool_tip,
        "setToolTip:" => set_item_tool_tip,
        "image" => item_image,
        "setImage:" => set_item_image,
        "tabState" => item_state,
    ];
    let item_class = methods![
        "tabViewItemWithViewController:" => item_with_view_controller,
    ];
    let controller_methods = methods![
        "dealloc" => controller_dealloc,
        "loadView" => controller_load_view,
        "tabView" => controller_tab_view_method,
        "setTabView:" => controller_set_tab_view,
        "tabViewItems" => controller_items,
        "addTabViewItem:" => controller_add_item,
        "insertTabViewItem:atIndex:" => controller_insert_item,
        "removeTabViewItem:" => controller_remove_item,
        "addChildViewController:" => controller_add_child,
        "insertChildViewController:atIndex:" => controller_insert_child,
        "selectedTabViewItemIndex" => controller_selected_index,
        "setSelectedTabViewItemIndex:" => controller_set_selected_index,
        "tabStyle" => controller_tab_style,
        "setTabStyle:" => controller_set_tab_style,
        "transitionOptions" => controller_transition_options,
        "setTransitionOptions:" => controller_set_transition_options,
        "tabView:shouldSelectTabViewItem:" => controller_should_select,
        "tabView:willSelectTabViewItem:" => controller_will_select,
        "tabView:didSelectTabViewItem:" => controller_did_select,
    ];
    super::define(
        runtime,
        b"NSTabView\0",
        (&NSTABVIEW, &NSTABVIEW_META),
        &NSVIEW,
        core::mem::size_of::<TabViewFields>(),
        &tab_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSTabViewItem\0",
        (&NSTABVIEWITEM, &NSTABVIEWITEM_META),
        &NSOBJECT,
        core::mem::size_of::<ItemFields>(),
        &item_methods,
        &item_class,
    ) && super::define(
        runtime,
        b"NSTabViewController\0",
        (&NSTABVIEWCONTROLLER, &NSTABVIEWCONTROLLER_META),
        &NSVIEWCONTROLLER,
        core::mem::size_of::<ControllerFields>(),
        &controller_methods,
        &[],
    )
}

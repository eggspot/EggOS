//! NSSplitView, NSSplitViewItem and NSSplitViewController (D751,
//! docs/ux/macos-containers-v0.1.md sections 2 and 3): a split view lays its subviews out as
//! panes along its axis (`eggos_macos_gui::split_view`), draws thin or thick dividers between
//! them, shows the resize pointers over each divider's 9-pixel area, moves a divider live as it
//! is dragged within the delegate's clamps (only the two neighbours change), collapses a pane
//! dragged past half its minimum when the delegate allows it, toggles one on a double-click,
//! and shares a change of its own size among the panes. A split view controller owns a
//! vertical split view filling its view, one pane per item, and answers its delegate messages
//! from the items: a sidebar starts at 200 pixels, between 160 and 320, filled `menu.bg`.

use super::{
    NSCURSOR, NSSPLITVIEW, NSSPLITVIEW_META, NSSPLITVIEWCONTROLLER, NSSPLITVIEWCONTROLLER_META,
    NSSPLITVIEWITEM, NSSPLITVIEWITEM_META, NSTRACKINGAREA, NSVIEW, NSVIEWCONTROLLER, drawing,
    event, responds, store, string, view,
};
use crate::foundation::{
    NSMUTABLEARRAY, autoreleased, class_pointer, new_instance, notification_with_name, post,
};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1, send2};
use alloc::vec::Vec;
use eggos_macos_gui::geometry::{Frame, Size, pixel_rect, round};
use eggos_macos_gui::split_view::{self as geometry, MOST_PANES, style};
use eggos_toolkit::colour;
use eggos_toolkit::input::key;
use eggos_toolkit::geometry::Rect;

const NO_INDEX: isize = -1;
const TRACK_ENTERED_AND_EXITED: usize = 0x01;
const TRACK_MOUSE_MOVED: usize = 0x02;
const TRACK_ACTIVE_ALWAYS: usize = 0x80;
const TRACK_IN_VISIBLE_RECT: usize = 0x200;
const WILL_RESIZE: &str = "NSSplitViewWillResizeSubviewsNotification";
const DID_RESIZE: &str = "NSSplitViewDidResizeSubviewsNotification";
const SIDEBAR_WIDTH: f64 = 200.0;
const SIDEBAR_MINIMUM: f64 = 160.0;
const SIDEBAR_MAXIMUM: f64 = 320.0;
const CONTENT_LIST_MINIMUM: f64 = 200.0;
const DEFAULT_HOLDING: f32 = 250.0;
const SIDEBAR_HOLDING: f32 = 260.0;

/// `NSSplitViewItemBehavior`'s values.
mod behavior {
    pub const DEFAULT: isize = 0;
    pub const SIDEBAR: isize = 1;
    pub const CONTENT_LIST: isize = 2;
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    index: usize,
    start: i32,
    from: i32,
    lengths: [i32; MOST_PANES],
    collapsed: [bool; MOST_PANES],
}

#[repr(C)]
#[derive(Debug)]
struct SplitViewFields {
    view: view::ViewFields,
    delegate: Id,
    vertical: bool,
    style: usize,
    count: usize,
    total: i32,
    across: i32,
    lengths: [i32; MOST_PANES],
    collapsed: [bool; MOST_PANES],
    restore: [i32; MOST_PANES],
    minimums: [i32; MOST_PANES],
    maximums: [i32; MOST_PANES],
    filled: [bool; MOST_PANES],
    holding: [f32; MOST_PANES],
    drag: Option<Drag>,
    hover: isize,
    tracking: Id,
    autosave_name: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ItemFields {
    isa: usize,
    view_controller: Id,
    behavior: isize,
    collapsed: bool,
    can_collapse: bool,
    minimum: f64,
    maximum: f64,
    fraction: f64,
    holding: f32,
    collapse_behavior: isize,
    full_height: bool,
    owner: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ControllerFields {
    base: super::controllers::ViewControllerFields,
    split_view: Id,
    items: Id,
}

fn split(object: Id) -> &'static mut SplitViewFields {
    // SAFETY: the callers pass NSSplitView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<SplitViewFields>() }
}

fn item(object: Id) -> &'static mut ItemFields {
    // SAFETY: the callers pass NSSplitViewItem instances, whose memory starts with these.
    unsafe { &mut *object.cast::<ItemFields>() }
}

fn controller(object: Id) -> &'static mut ControllerFields {
    // SAFETY: the callers pass NSSplitViewController instances, whose memory starts with these.
    unsafe { &mut *object.cast::<ControllerFields>() }
}

fn panes(this: Id) -> Vec<Id> {
    view::fields(this)
        .subviews
        .as_slice()
        .iter()
        .copied()
        .take(MOST_PANES)
        .collect()
}

fn thickness_of(this: Id) -> i32 {
    geometry::thickness(split(this).style)
}

fn axis_lengths(this: Id) -> (i32, i32) {
    let frame = view::fields(this).frame;
    match split(this).vertical {
        true => (round(frame.width), round(frame.height)),
        false => (round(frame.height), round(frame.width)),
    }
}

fn length_along(this: Id, pane: Id) -> i32 {
    let frame = view::fields(pane).frame;
    match split(this).vertical {
        true => round(frame.width),
        false => round(frame.height),
    }
}

fn notify(this: Id, name: &str, delegate_sel: Sel) {
    let notification = notification_with_name(string(name), this);
    let delegate = split(this).delegate;
    if responds(delegate, delegate_sel) {
        send1(delegate, delegate_sel, notification as usize);
    }
    post(notification);
}

fn fixed_panes(this: Id) -> [bool; MOST_PANES] {
    split(this).collapsed
}

fn tile(this: Id) {
    let all = panes(this);
    let count = all.len();
    let (total, across) = axis_lengths(this);
    let divider = thickness_of(this);
    if split(this).count != count {
        let previous = split(this).count.min(MOST_PANES);
        for (index, &pane) in all.iter().enumerate().skip(previous) {
            split(this).collapsed[index] = false;
            split(this).lengths[index] = length_along(this, pane);
        }
        split(this).count = count;
        let fixed = fixed_panes(this);
        geometry::fit(&mut split(this).lengths[..count], total, divider, &fixed[..count]);
        split(this).total = total;
        split(this).across = across;
    } else if split(this).total != total || split(this).across != across {
        resize_panes(this, total, across);
    }
    place(this);
}

fn resize_panes(this: Id, total: i32, across: i32) {
    let count = split(this).count;
    let divider = thickness_of(this);
    let delegate = split(this).delegate;
    let by_delegate = sel!("splitView:resizeSubviewsWithOldSize:");
    let old = match split(this).vertical {
        true => Size {
            width: f64::from(split(this).total),
            height: f64::from(split(this).across),
        },
        false => Size {
            width: f64::from(split(this).across),
            height: f64::from(split(this).total),
        },
    };
    split(this).total = total;
    split(this).across = across;
    if responds(delegate, by_delegate) {
        let resize: extern "C" fn(Id, Sel, Id, Size) = message::msg_send_as();
        resize(delegate, by_delegate, this, old);
        for (index, pane) in panes(this).into_iter().enumerate() {
            split(this).lengths[index] = length_along(this, pane);
        }
        return;
    }
    let fixed = fixed_panes(this);
    geometry::fit(&mut split(this).lengths[..count], total, divider, &fixed[..count]);
    clamp_minimums(this);
}

fn clamp_minimums(this: Id) {
    let fields = split(this);
    let count = fields.count;
    let mut owed = 0;
    for index in 0..count {
        if fields.collapsed[index] {
            continue;
        }
        let minimum = fields.minimums[index];
        if fields.lengths[index] < minimum {
            owed += minimum - fields.lengths[index];
            fields.lengths[index] = minimum;
        } else if owed > 0 {
            let give = (fields.lengths[index] - minimum).min(owed);
            fields.lengths[index] -= give;
            owed -= give;
        }
    }
}

fn place(this: Id) {
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let height = round(view::fields(this).frame.height);
    let (lengths, collapsed, vertical, across) =
        (fields.lengths, fields.collapsed, fields.vertical, fields.across);
    for (index, pane) in panes(this).into_iter().enumerate().take(count) {
        let rect = geometry::pane_rect(&lengths[..count], index, divider, across, vertical);
        let frame = Frame::new(
            f64::from(rect.x),
            f64::from(height - rect.y - rect.height),
            f64::from(rect.width.max(0)),
            f64::from(rect.height.max(0)),
        );
        if view::fields(pane).frame != frame {
            super::send_frame(pane, sel!("setFrame:"), frame);
        }
        if view::fields(pane).hidden != collapsed[index] {
            send1(pane, sel!("setHidden:"), usize::from(collapsed[index]));
        }
    }
    send1(this, sel!("setNeedsDisplay:"), 1);
}

fn minimum_position(this: Id, index: usize) -> i32 {
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let own = geometry::divider_range(
        &fields.lengths[..count],
        index,
        divider,
        (fields.minimums[index], 0),
    )
    .0;
    ask_coordinate(this, sel!("splitView:constrainMinCoordinate:ofSubviewAt:"), own, index)
        .max(own)
}

fn maximum_position(this: Id, index: usize) -> i32 {
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let (start, mut high) = geometry::divider_range(
        &fields.lengths[..count],
        index,
        divider,
        (0, fields.minimums.get(index + 1).copied().unwrap_or(0)),
    );
    let most = fields.maximums[index];
    if most > 0 {
        high = high.min(start + most);
    }
    let asked = ask_coordinate(
        this,
        sel!("splitView:constrainMaxCoordinate:ofSubviewAt:"),
        high,
        index,
    );
    asked.min(high)
}

fn ask_coordinate(this: Id, sel: Sel, proposed: i32, index: usize) -> i32 {
    let delegate = split(this).delegate;
    if !responds(delegate, sel) {
        return proposed;
    }
    let ask: extern "C" fn(Id, Sel, Id, f64, isize) -> f64 = message::msg_send_as();
    round(ask(delegate, sel, this, f64::from(proposed), index as isize))
}

fn can_collapse(this: Id, index: usize) -> bool {
    let delegate = split(this).delegate;
    let asked = sel!("splitView:canCollapseSubview:");
    let Some(&pane) = panes(this).get(index) else {
        return false;
    };
    responds(delegate, asked) && send1(delegate, asked, pane as usize) as u8 != 0
}

fn may_toggle(this: Id, index: usize, divider_index: usize) -> bool {
    let delegate = split(this).delegate;
    let asked = sel!("splitView:shouldCollapseSubview:forDoubleClickOnDividerAtIndex:");
    let Some(&pane) = panes(this).get(index) else {
        return false;
    };
    if !responds(delegate, asked) {
        return false;
    }
    let ask: extern "C" fn(Id, Sel, Id, Id, isize) -> u8 = message::msg_send_as();
    can_collapse(this, index) && ask(delegate, asked, this, pane, divider_index as isize) != 0
}

fn set_collapsed_at(this: Id, index: usize, collapse: bool) {
    let fields = split(this);
    let count = fields.count;
    if index >= count || fields.collapsed[index] == collapse {
        return;
    }
    let neighbour = match index + 1 < count {
        true => index + 1,
        false => index.saturating_sub(1),
    };
    if collapse {
        fields.restore[index] = fields.lengths[index];
        fields.lengths[neighbour] += fields.lengths[index];
        fields.lengths[index] = 0;
    } else {
        let wanted = fields.restore[index].max(fields.minimums[index]).max(1);
        let given = wanted.min(fields.lengths[neighbour]);
        fields.lengths[neighbour] -= given;
        fields.lengths[index] = given;
    }
    fields.collapsed[index] = collapse;
}

fn move_to(this: Id, index: usize, position: i32) {
    let low = minimum_position(this, index);
    let high = maximum_position(this, index);
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let start = geometry::pane_start(&fields.lengths[..count], index, divider);
    let next_end = geometry::pane_start(&fields.lengths[..count], index + 1, divider)
        + fields.lengths[index + 1];
    let first_minimum = fields.minimums[index];
    let next_minimum = fields.minimums[index + 1];
    if position < low && position < start + first_minimum / 2 && can_collapse(this, index) {
        geometry::move_divider(&mut split(this).lengths[..count], index, divider, start);
        split(this).restore[index] = split(this).restore[index].max(first_minimum);
        split(this).collapsed[index] = true;
        return;
    }
    let next_start = next_end - divider;
    if position > high
        && position > next_start - next_minimum / 2
        && can_collapse(this, index + 1)
    {
        geometry::move_divider(&mut split(this).lengths[..count], index, divider, next_start);
        split(this).collapsed[index + 1] = true;
        return;
    }
    let fields = split(this);
    if fields.collapsed[index] && position >= start + first_minimum {
        fields.collapsed[index] = false;
    }
    if fields.collapsed[index + 1] && position <= next_start - next_minimum {
        fields.collapsed[index + 1] = false;
    }
    if fields.collapsed[index] || fields.collapsed[index + 1] {
        return;
    }
    geometry::move_divider(
        &mut split(this).lengths[..count],
        index,
        divider,
        position.clamp(low, high.max(low)),
    );
}

fn local_along(this: Id, event_object: Id) -> i32 {
    let origin = view::origin_in_window(this);
    let location = event::fields(event_object).location;
    let height = view::fields(this).frame.height;
    match split(this).vertical {
        true => round(location.x - origin.x),
        false => round(height - (location.y - origin.y)) - 1,
    }
}

fn divider_under(this: Id, along: i32) -> Option<usize> {
    let fields = split(this);
    geometry::divider_at(&fields.lengths[..fields.count], geometry::thickness(fields.style), along)
}

extern "C" fn mouse_down(this: Id, _: Sel, event_object: Id) {
    tile(this);
    let along = local_along(this, event_object);
    let Some(index) = divider_under(this, along) else {
        return;
    };
    let clicks = send(event_object, sel!("clickCount")) as isize;
    if clicks >= 2 {
        let target = match split(this).collapsed[index + 1] {
            true => index + 1,
            false => index,
        };
        if may_toggle(this, target, index) {
            notify(this, WILL_RESIZE, sel!("splitViewWillResizeSubviews:"));
            let collapse = !split(this).collapsed[target];
            set_collapsed_at(this, target, collapse);
            place(this);
            notify(this, DID_RESIZE, sel!("splitViewDidResizeSubviews:"));
        }
        return;
    }
    let fields = split(this);
    let divider = geometry::thickness(fields.style);
    let start = geometry::divider_start(&fields.lengths[..fields.count], index, divider);
    fields.drag = Some(Drag {
        index,
        start,
        from: along,
        lengths: fields.lengths,
        collapsed: fields.collapsed,
    });
    notify(this, WILL_RESIZE, sel!("splitViewWillResizeSubviews:"));
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event_object: Id) {
    let Some(drag) = split(this).drag else {
        return;
    };
    let along = local_along(this, event_object);
    let before = split(this).lengths;
    move_to(this, drag.index, drag.start + along - drag.from);
    if split(this).lengths != before {
        place(this);
        notify(this, DID_RESIZE, sel!("splitViewDidResizeSubviews:"));
    }
}

extern "C" fn mouse_up(this: Id, _: Sel, _: Id) {
    split(this).drag = None;
    send(this, sel!("resetCursorRects"));
}

extern "C" fn key_down(this: Id, _: Sel, event_object: Id) {
    let escape = event::fields(event_object).linux_code == key::ESC;
    match (split(this).drag, escape) {
        (Some(drag), true) => {
            let fields = split(this);
            fields.lengths = drag.lengths;
            fields.collapsed = drag.collapsed;
            fields.drag = None;
            place(this);
            notify(this, DID_RESIZE, sel!("splitViewDidResizeSubviews:"));
        }
        _ => {
            let superview = view::fields(this).superview;
            if !superview.is_null() {
                send1(superview, sel!("keyDown:"), event_object as usize);
            }
        }
    }
}

extern "C" fn mouse_moved(this: Id, _: Sel, event_object: Id) {
    let along = local_along(this, event_object);
    let under = divider_under(this, along).map_or(NO_INDEX, |index| index as isize);
    if split(this).hover != under {
        split(this).hover = under;
        send1(this, sel!("setNeedsDisplay:"), 1);
    }
}

extern "C" fn mouse_exited(this: Id, _: Sel, _: Id) {
    if split(this).hover != NO_INDEX {
        split(this).hover = NO_INDEX;
        send1(this, sel!("setNeedsDisplay:"), 1);
    }
}

fn track_hover(this: Id) {
    if !split(this).tracking.is_null() || view::fields(this).window.is_null() {
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
    split(this).tracking = area;
}

extern "C" fn reset_cursor_rects(this: Id, _: Sel) {
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let (vertical, across) = (fields.vertical, fields.across);
    let lengths = fields.lengths;
    let height = round(view::fields(this).frame.height);
    let shape = match vertical {
        true => sel!("resizeLeftRightCursor"),
        false => sel!("resizeUpDownCursor"),
    };
    let cursor = send(class_pointer(&NSCURSOR).cast(), shape);
    let add: extern "C" fn(Id, Sel, Frame, Id) = message::msg_send_as();
    for index in 0..count.saturating_sub(1) {
        let rect = geometry::handle_rect(&lengths[..count], index, divider, across, vertical);
        let frame = Frame::new(
            f64::from(rect.x),
            f64::from(height - rect.y - rect.height),
            f64::from(rect.width),
            f64::from(rect.height),
        );
        add(this, sel!("addCursorRect:cursor:"), frame, cursor);
    }
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    track_hover(this);
    tile(this);
    let frame = view::fields(this).frame;
    let fields = split(this);
    let count = fields.count;
    let divider = geometry::thickness(fields.style);
    let thick = fields.style != style::THIN;
    let (vertical, across, hover) = (fields.vertical, fields.across, fields.hover);
    let (lengths, filled, collapsed) = (fields.lengths, fields.filled, fields.collapsed);
    let dragging = fields.drag.map(|drag| drag.index as isize);
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let outer = pixel_rect(
            Frame::new(origin.x, origin.y, frame.width, frame.height),
            height,
        );
        let place_rect = |rect: Rect| rect.offset(outer.x, outer.y);
        for index in 0..count {
            if filled[index] && !collapsed[index] {
                let rect = geometry::pane_rect(&lengths[..count], index, divider, across, vertical);
                canvas.fill(place_rect(rect), colour::MENU);
            }
        }
        for index in 0..count.saturating_sub(1) {
            let rect = geometry::divider_rect(&lengths[..count], index, divider, across, vertical);
            let lit = hover == index as isize
                || dragging == Some(index as isize)
                || collapsed[index]
                || collapsed[index + 1];
            if !thick {
                let ink = match lit {
                    true => colour::TEXT_DIM,
                    false => colour::MENU_BORDER,
                };
                canvas.fill(place_rect(rect), ink);
                continue;
            }
            canvas.fill(place_rect(rect), colour::MENU);
            let edges = match vertical {
                true => [
                    Rect::new(rect.x, rect.y, 1, rect.height),
                    Rect::new(rect.x + rect.width - 1, rect.y, 1, rect.height),
                ],
                false => [
                    Rect::new(rect.x, rect.y, rect.width, 1),
                    Rect::new(rect.x, rect.y + rect.height - 1, rect.width, 1),
                ],
            };
            for edge in edges {
                canvas.fill(place_rect(edge), colour::MENU_BORDER);
            }
            let grip = match lit {
                true => colour::TEXT,
                false => colour::TEXT_DIM,
            };
            for dot in geometry::grip_dots(rect, vertical) {
                canvas.fill(place_rect(dot), grip);
            }
        }
    });
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    let fields = split(this);
    fields.style = style::THIN;
    fields.hover = NO_INDEX;
    fields.holding = [DEFAULT_HOLDING; MOST_PANES];
    this
}

extern "C" fn split_dealloc(this: Id, _: Sel) {
    let fields = split(this);
    for slot in [&mut fields.tracking, &mut fields.autosave_name] {
        store(slot, core::ptr::null_mut());
    }
    view::dispose(this);
}

extern "C" fn is_vertical(this: Id, _: Sel) -> u8 {
    u8::from(split(this).vertical)
}

extern "C" fn set_vertical(this: Id, _: Sel, flag: u8) {
    split(this).vertical = flag != 0;
    split(this).count = 0;
    tile(this);
}

extern "C" fn divider_style(this: Id, _: Sel) -> usize {
    split(this).style
}

extern "C" fn set_divider_style(this: Id, _: Sel, chosen: usize) {
    split(this).style = chosen;
    let (total, across) = axis_lengths(this);
    let count = split(this).count;
    if count <= MOST_PANES {
        let fixed = fixed_panes(this);
        geometry::fit(
            &mut split(this).lengths[..count],
            total,
            geometry::thickness(chosen),
            &fixed[..count],
        );
        split(this).across = across;
        place(this);
    }
}

extern "C" fn divider_thickness(this: Id, _: Sel) -> f64 {
    f64::from(thickness_of(this))
}

extern "C" fn arranged_subviews(this: Id, _: Sel) -> Id {
    send(this, sel!("subviews"))
}

extern "C" fn add_arranged(this: Id, _: Sel, added: Id) {
    send1(this, sel!("addSubview:"), added as usize);
    tile(this);
}

extern "C" fn insert_arranged(this: Id, sel: Sel, added: Id, _index: isize) {
    add_arranged(this, sel, added);
}

extern "C" fn remove_arranged(this: Id, _: Sel, removed: Id) {
    if !removed.is_null() && view::fields(removed).superview == this {
        send(removed, sel!("removeFromSuperview"));
    }
    tile(this);
}

extern "C" fn arranges_all(_: Id, _: Sel) -> u8 {
    1
}

extern "C" fn adjust_subviews(this: Id, _: Sel) {
    let (total, across) = axis_lengths(this);
    let count = panes(this).len();
    split(this).count = count;
    let fixed = fixed_panes(this);
    geometry::fit(
        &mut split(this).lengths[..count],
        total,
        thickness_of(this),
        &fixed[..count],
    );
    clamp_minimums(this);
    split(this).total = total;
    split(this).across = across;
    place(this);
}

extern "C" fn set_position(this: Id, _: Sel, position: f64, index: isize) {
    tile(this);
    let count = split(this).count;
    if index < 0 || index as usize + 1 >= count {
        return;
    }
    let index = index as usize;
    let divider = thickness_of(this);
    let wanted = round(position);
    let start = geometry::pane_start(&split(this).lengths[..count], index, divider);
    let end = geometry::pane_start(&split(this).lengths[..count], index + 1, divider)
        + split(this).lengths[index + 1];
    notify(this, WILL_RESIZE, sel!("splitViewWillResizeSubviews:"));
    if wanted <= start && can_collapse(this, index) {
        set_collapsed_at(this, index, true);
    } else if wanted >= end - divider && can_collapse(this, index + 1) {
        set_collapsed_at(this, index + 1, true);
    } else {
        move_to(this, index, wanted);
    }
    place(this);
    notify(this, DID_RESIZE, sel!("splitViewDidResizeSubviews:"));
}

extern "C" fn min_position(this: Id, _: Sel, index: isize) -> f64 {
    tile(this);
    match index >= 0 && (index as usize) + 1 < split(this).count {
        true => f64::from(minimum_position(this, index as usize)),
        false => 0.0,
    }
}

extern "C" fn max_position(this: Id, _: Sel, index: isize) -> f64 {
    tile(this);
    match index >= 0 && (index as usize) + 1 < split(this).count {
        true => f64::from(maximum_position(this, index as usize)),
        false => 0.0,
    }
}

extern "C" fn is_collapsed(this: Id, _: Sel, pane: Id) -> u8 {
    let index = panes(this).iter().position(|&each| each == pane);
    u8::from(index.is_some_and(|index| split(this).collapsed[index]))
}

extern "C" fn holding_priority(this: Id, _: Sel, index: isize) -> f32 {
    match (0..MOST_PANES as isize).contains(&index) {
        true => split(this).holding[index as usize],
        false => DEFAULT_HOLDING,
    }
}

extern "C" fn set_holding_priority(this: Id, _: Sel, priority: f32, index: isize) {
    if (0..MOST_PANES as isize).contains(&index) {
        split(this).holding[index as usize] = priority;
    }
}

extern "C" fn autosave_name(this: Id, _: Sel) -> Id {
    split(this).autosave_name
}

extern "C" fn set_autosave_name(this: Id, _: Sel, name: Id) {
    store(&mut split(this).autosave_name, name);
}

extern "C" fn split_delegate(this: Id, _: Sel) -> Id {
    split(this).delegate
}

extern "C" fn set_split_delegate(this: Id, _: Sel, delegate: Id) {
    split(this).delegate = delegate;
}

extern "C" fn divider_colour(_: Id, _: Sel) -> Id {
    core::ptr::null_mut()
}

extern "C" fn set_frame(this: Id, _: Sel, frame: Frame) {
    view::fields(this).frame = frame;
    tile(this);
}

extern "C" fn set_frame_size(this: Id, sel: Sel, size: Size) {
    let origin = view::fields(this).frame;
    set_frame(
        this,
        sel,
        Frame::new(origin.x, origin.y, size.width, size.height),
    );
}

fn pane_of_item(this: Id, chosen: Id) -> Option<usize> {
    let all = items_of(this);
    all.iter().position(|&each| each == chosen)
}

fn items_of(this: Id) -> Vec<Id> {
    let list = controller(this).items;
    if list.is_null() {
        return Vec::new();
    }
    let count = send(list, sel!("count")) as usize;
    (0..count)
        .map(|index| send1(list, sel!("objectAtIndex:"), index))
        .collect()
}

fn controller_split(this: Id) -> Id {
    let fields = controller(this);
    if fields.split_view.is_null() {
        let made = view::new_with_frame(&NSSPLITVIEW, Frame::new(0.0, 0.0, 640.0, 400.0));
        if made.is_null() {
            return made;
        }
        split(made).vertical = true;
        split(made).delegate = this;
        fields.split_view = made;
    }
    fields.split_view
}

fn apply_item(this: Id, index: usize) {
    let made = controller_split(this);
    let Some(&chosen) = items_of(this).get(index) else {
        return;
    };
    if made.is_null() || index >= MOST_PANES {
        return;
    }
    tile(made);
    let fields = item(chosen);
    let scale = |value: f64| round(value).max(0);
    let shown = split(made);
    shown.minimums[index] = scale(fields.minimum);
    shown.maximums[index] = scale(fields.maximum);
    shown.filled[index] = fields.behavior == behavior::SIDEBAR;
    shown.holding[index] = fields.holding;
    if shown.collapsed[index] != fields.collapsed {
        set_collapsed_at(made, index, fields.collapsed);
    }
    place(made);
}

fn first_length(this: Id, index: usize) {
    let made = controller_split(this);
    let Some(&chosen) = items_of(this).get(index) else {
        return;
    };
    tile(made);
    let count = split(made).count;
    if index + 1 >= count && index == 0 {
        return;
    }
    let fields = item(chosen);
    let (total, _) = axis_lengths(made);
    let wanted = match (fields.behavior, fields.fraction > 0.0) {
        (_, true) => round(f64::from(total) * fields.fraction),
        (behavior::SIDEBAR, false) => round(SIDEBAR_WIDTH),
        _ => return,
    };
    let wanted = wanted.max(round(fields.minimum)).min(match fields.maximum > 0.0 {
        true => round(fields.maximum),
        false => i32::MAX,
    });
    let divider = thickness_of(made);
    let lengths = &mut split(made).lengths[..count];
    match index + 1 < count {
        true => {
            let start = geometry::pane_start(lengths, index, divider);
            geometry::move_divider(lengths, index, divider, start + wanted);
        }
        false => {
            let start = geometry::pane_start(lengths, index - 1, divider);
            let end = start + lengths[index - 1] + divider + lengths[index];
            geometry::move_divider(lengths, index - 1, divider, end - divider - wanted);
        }
    }
    place(made);
}

extern "C" fn controller_load_view(this: Id, _: Sel) {
    let made = controller_split(this);
    send1(this, sel!("setView:"), made as usize);
}

extern "C" fn controller_split_view(this: Id, _: Sel) -> Id {
    controller_split(this)
}

extern "C" fn controller_set_split_view(this: Id, _: Sel, made: Id) {
    store(&mut controller(this).split_view, made);
    if !made.is_null() {
        split(made).delegate = this;
    }
}

extern "C" fn controller_items(this: Id, _: Sel) -> Id {
    let fields = controller(this);
    if fields.items.is_null() {
        fields.items = send(new_instance(class_pointer(&NSMUTABLEARRAY)), sel!("init"));
    }
    fields.items
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

extern "C" fn controller_insert_item(this: Id, sel: Sel, added: Id, index: isize) {
    if added.is_null() || items_of(this).len() >= MOST_PANES {
        return;
    }
    let at = (index.max(0) as usize).min(items_of(this).len());
    let all = controller_items(this, sel);
    send2(all, sel!("insertObject:atIndex:"), added as usize, at);
    item(added).owner = this;
    let child = item(added).view_controller;
    if !child.is_null() {
        insert_child_as_view_controller(this, child, at as isize);
    }
    let made = controller_split(this);
    let shown = match child.is_null() {
        true => core::ptr::null_mut(),
        false => send(child, sel!("view")),
    };
    if !shown.is_null() {
        send1(made, sel!("addSubview:"), shown as usize);
    }
    let count = items_of(this).len();
    for each in 0..count {
        apply_item(this, each);
    }
    first_length(this, at);
}

extern "C" fn controller_add_item(this: Id, sel: Sel, added: Id) {
    let end = items_of(this).len() as isize;
    controller_insert_item(this, sel, added, end);
}

extern "C" fn controller_remove_item(this: Id, _: Sel, removed: Id) {
    let Some(index) = pane_of_item(this, removed) else {
        return;
    };
    objc_retain(removed);
    send1(controller(this).items, sel!("removeObjectAtIndex:"), index);
    item(removed).owner = core::ptr::null_mut();
    let child = item(removed).view_controller;
    if !child.is_null() {
        let shown = send(child, sel!("view"));
        if !shown.is_null() {
            send(shown, sel!("removeFromSuperview"));
        }
        send(child, sel!("removeFromParentViewController"));
    }
    objc_release(removed);
    let made = controller_split(this);
    split(made).count = 0;
    tile(made);
}

extern "C" fn controller_add_child(this: Id, sel: Sel, child: Id) {
    let wrapped = item_with_controller(class_pointer(&NSSPLITVIEWITEM).cast(), sel, child);
    controller_add_item(this, sel, wrapped);
}

extern "C" fn controller_insert_child(this: Id, sel: Sel, child: Id, index: isize) {
    let wrapped = item_with_controller(class_pointer(&NSSPLITVIEWITEM).cast(), sel, child);
    controller_insert_item(this, sel, wrapped, index);
}

extern "C" fn controller_item_for(this: Id, _: Sel, child: Id) -> Id {
    items_of(this)
        .into_iter()
        .find(|&each| item(each).view_controller == child)
        .unwrap_or(core::ptr::null_mut())
}

extern "C" fn toggle_sidebar(this: Id, _: Sel, _sender: Id) {
    let all = items_of(this);
    let Some(index) = all
        .iter()
        .position(|&each| item(each).behavior == behavior::SIDEBAR)
    else {
        return;
    };
    let chosen = all[index];
    item(chosen).collapsed = !item(chosen).collapsed;
    apply_item(this, index);
}

extern "C" fn controller_can_collapse(this: Id, _: Sel, _: Id, pane: Id) -> u8 {
    u8::from(item_for_pane(this, pane).is_some_and(|each| item(each).can_collapse))
}

extern "C" fn controller_double_click(this: Id, _: Sel, _: Id, pane: Id, _: isize) -> u8 {
    u8::from(item_for_pane(this, pane).is_some_and(|each| item(each).can_collapse))
}

extern "C" fn controller_did_resize(this: Id, _: Sel, _: Id) {
    let made = controller(this).split_view;
    if made.is_null() {
        return;
    }
    for (index, each) in items_of(this).into_iter().enumerate().take(MOST_PANES) {
        item(each).collapsed = split(made).collapsed[index];
    }
}

fn item_for_pane(this: Id, pane: Id) -> Option<Id> {
    items_of(this).into_iter().find(|&each| {
        let child = item(each).view_controller;
        !child.is_null() && send(child, sel!("view")) == pane
    })
}

extern "C" fn controller_dealloc(this: Id, _: Sel) {
    let made = controller(this).split_view;
    if !made.is_null() {
        split(made).delegate = core::ptr::null_mut();
    }
    for slot in [&mut controller(this).split_view, &mut controller(this).items] {
        store(slot, core::ptr::null_mut());
    }
    match inherited(sel!("dealloc")) {
        Some(found) => {
            // SAFETY: NSViewController's own `dealloc`, which takes only the receiver.
            let parent: extern "C" fn(Id, Sel) = unsafe { core::mem::transmute(found) };
            parent(this, sel!("dealloc"));
        }
        None => dispose(this),
    }
}

fn new_item(class: Id, child: Id, kind_of_item: isize) -> Id {
    let made = send(send(class, sel!("alloc")), sel!("init"));
    if made.is_null() {
        return made;
    }
    let fields = item(made);
    store(&mut fields.view_controller, child);
    fields.behavior = kind_of_item;
    fields.holding = DEFAULT_HOLDING;
    match kind_of_item {
        behavior::SIDEBAR => {
            fields.can_collapse = true;
            fields.minimum = SIDEBAR_MINIMUM;
            fields.maximum = SIDEBAR_MAXIMUM;
            fields.holding = SIDEBAR_HOLDING;
        }
        behavior::CONTENT_LIST => fields.minimum = CONTENT_LIST_MINIMUM,
        _ => {}
    }
    autoreleased(made)
}

extern "C" fn item_with_controller(class: Id, _: Sel, child: Id) -> Id {
    new_item(class, child, behavior::DEFAULT)
}

extern "C" fn sidebar_with_controller(class: Id, _: Sel, child: Id) -> Id {
    new_item(class, child, behavior::SIDEBAR)
}

extern "C" fn content_list_with_controller(class: Id, _: Sel, child: Id) -> Id {
    new_item(class, child, behavior::CONTENT_LIST)
}

extern "C" fn item_dealloc(this: Id, _: Sel) {
    store(&mut item(this).view_controller, core::ptr::null_mut());
    dispose(this);
}

fn item_changed(this: Id) {
    let owner = item(this).owner;
    if let Some(index) = (!owner.is_null())
        .then(|| pane_of_item(owner, this))
        .flatten()
    {
        apply_item(owner, index);
    }
}

extern "C" fn item_view_controller(this: Id, _: Sel) -> Id {
    item(this).view_controller
}

extern "C" fn item_set_view_controller(this: Id, _: Sel, child: Id) {
    store(&mut item(this).view_controller, child);
}

extern "C" fn item_behavior(this: Id, _: Sel) -> isize {
    item(this).behavior
}

extern "C" fn item_is_collapsed(this: Id, _: Sel) -> u8 {
    u8::from(item(this).collapsed)
}

extern "C" fn item_set_collapsed(this: Id, _: Sel, flag: u8) {
    item(this).collapsed = flag != 0;
    item_changed(this);
}

extern "C" fn item_can_collapse(this: Id, _: Sel) -> u8 {
    u8::from(item(this).can_collapse)
}

extern "C" fn item_set_can_collapse(this: Id, _: Sel, flag: u8) {
    item(this).can_collapse = flag != 0;
}

extern "C" fn item_minimum(this: Id, _: Sel) -> f64 {
    item(this).minimum
}

extern "C" fn item_set_minimum(this: Id, _: Sel, value: f64) {
    item(this).minimum = value.max(0.0);
    item_changed(this);
}

extern "C" fn item_maximum(this: Id, _: Sel) -> f64 {
    item(this).maximum
}

extern "C" fn item_set_maximum(this: Id, _: Sel, value: f64) {
    item(this).maximum = value.max(0.0);
    item_changed(this);
}

extern "C" fn item_fraction(this: Id, _: Sel) -> f64 {
    item(this).fraction
}

extern "C" fn item_set_fraction(this: Id, _: Sel, value: f64) {
    item(this).fraction = value.clamp(0.0, 1.0);
}

extern "C" fn item_holding(this: Id, _: Sel) -> f32 {
    item(this).holding
}

extern "C" fn item_set_holding(this: Id, _: Sel, value: f32) {
    item(this).holding = value;
    item_changed(this);
}

extern "C" fn item_collapse_behavior(this: Id, _: Sel) -> isize {
    item(this).collapse_behavior
}

extern "C" fn item_set_collapse_behavior(this: Id, _: Sel, value: isize) {
    item(this).collapse_behavior = value;
}

extern "C" fn item_full_height(this: Id, _: Sel) -> u8 {
    u8::from(item(this).full_height)
}

extern "C" fn item_set_full_height(this: Id, _: Sel, flag: u8) {
    item(this).full_height = flag != 0;
}

pub fn define(runtime: &mut Runtime) -> bool {
    let split_methods = methods![
        "initWithFrame:" => init_with_frame,
        "dealloc" => split_dealloc,
        "isVertical" => is_vertical,
        "setVertical:" => set_vertical,
        "dividerStyle" => divider_style,
        "setDividerStyle:" => set_divider_style,
        "dividerThickness" => divider_thickness,
        "dividerColor" => divider_colour,
        "arrangedSubviews" => arranged_subviews,
        "addArrangedSubview:" => add_arranged,
        "insertArrangedSubview:atIndex:" => insert_arranged,
        "removeArrangedSubview:" => remove_arranged,
        "arrangesAllSubviews" => arranges_all,
        "adjustSubviews" => adjust_subviews,
        "setPosition:ofDividerAtIndex:" => set_position,
        "minPossiblePositionOfDividerAtIndex:" => min_position,
        "maxPossiblePositionOfDividerAtIndex:" => max_position,
        "isSubviewCollapsed:" => is_collapsed,
        "holdingPriorityForSubviewAtIndex:" => holding_priority,
        "setHoldingPriority:forSubviewAtIndex:" => set_holding_priority,
        "autosaveName" => autosave_name,
        "setAutosaveName:" => set_autosave_name,
        "delegate" => split_delegate,
        "setDelegate:" => set_split_delegate,
        "setFrame:" => set_frame,
        "setFrameSize:" => set_frame_size,
        "mouseDown:" => mouse_down,
        "mouseDragged:" => mouse_dragged,
        "mouseUp:" => mouse_up,
        "mouseMoved:" => mouse_moved,
        "mouseExited:" => mouse_exited,
        "keyDown:" => key_down,
        "resetCursorRects" => reset_cursor_rects,
        "drawRect:" => draw_rect,
    ];
    let item_methods = methods![
        "dealloc" => item_dealloc,
        "viewController" => item_view_controller,
        "setViewController:" => item_set_view_controller,
        "behavior" => item_behavior,
        "isCollapsed" => item_is_collapsed,
        "setCollapsed:" => item_set_collapsed,
        "canCollapse" => item_can_collapse,
        "setCanCollapse:" => item_set_can_collapse,
        "minimumThickness" => item_minimum,
        "setMinimumThickness:" => item_set_minimum,
        "maximumThickness" => item_maximum,
        "setMaximumThickness:" => item_set_maximum,
        "preferredThicknessFraction" => item_fraction,
        "setPreferredThicknessFraction:" => item_set_fraction,
        "holdingPriority" => item_holding,
        "setHoldingPriority:" => item_set_holding,
        "collapseBehavior" => item_collapse_behavior,
        "setCollapseBehavior:" => item_set_collapse_behavior,
        "allowsFullHeightLayout" => item_full_height,
        "setAllowsFullHeightLayout:" => item_set_full_height,
    ];
    let item_class = methods![
        "splitViewItemWithViewController:" => item_with_controller,
        "sidebarWithViewController:" => sidebar_with_controller,
        "contentListWithViewController:" => content_list_with_controller,
    ];
    let controller_methods = methods![
        "dealloc" => controller_dealloc,
        "loadView" => controller_load_view,
        "splitView" => controller_split_view,
        "setSplitView:" => controller_set_split_view,
        "splitViewItems" => controller_items,
        "addSplitViewItem:" => controller_add_item,
        "insertSplitViewItem:atIndex:" => controller_insert_item,
        "removeSplitViewItem:" => controller_remove_item,
        "splitViewItemForViewController:" => controller_item_for,
        "addChildViewController:" => controller_add_child,
        "insertChildViewController:atIndex:" => controller_insert_child,
        "toggleSidebar:" => toggle_sidebar,
        "splitView:canCollapseSubview:" => controller_can_collapse,
        "splitView:shouldCollapseSubview:forDoubleClickOnDividerAtIndex:" => controller_double_click,
        "splitViewDidResizeSubviews:" => controller_did_resize,
    ];
    super::define(
        runtime,
        b"NSSplitView\0",
        (&NSSPLITVIEW, &NSSPLITVIEW_META),
        &NSVIEW,
        core::mem::size_of::<SplitViewFields>(),
        &split_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSSplitViewItem\0",
        (&NSSPLITVIEWITEM, &NSSPLITVIEWITEM_META),
        &NSOBJECT,
        core::mem::size_of::<ItemFields>(),
        &item_methods,
        &item_class,
    ) && super::define(
        runtime,
        b"NSSplitViewController\0",
        (&NSSPLITVIEWCONTROLLER, &NSSPLITVIEWCONTROLLER_META),
        &NSVIEWCONTROLLER,
        core::mem::size_of::<ControllerFields>(),
        &controller_methods,
        &[],
    )
}

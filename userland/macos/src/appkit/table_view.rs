//! NSTableView, NSTableColumn and NSIndexSet (D373, docs/ux/macos-gui-v0.3.md section 11): a
//! cell-based table with a data source, the document view of a scroll view, drawn as the
//! toolkit's table: the header kept at the top of the clip view, 24-pixel rows asked of the
//! data source at every paint, single selection by pointer and keys with the `>` mark and the
//! focus ring, actions, double actions and the delegate's notifications, alternating rows,
//! grid lines and column resizing from the header.

use super::{
    NSINDEXSET, NSINDEXSET_META, NSTABLECOLUMN, NSTABLECOLUMN_META, NSTABLEVIEW,
    NSTABLEVIEW_META, NSVIEW, application, drawing, event, responds, scroll_view, store,
    store_copy, string, view, window, with_text,
};
use crate::foundation::{NSARRAY, class_pointer, new_instance, notification_with_name, post};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1, send2};
use eggos_macos_gui::geometry::{Frame, Point, pixel_rect, round};
use eggos_macos_gui::table_view::{
    self as geometry, DASHED_HORIZONTAL_GRID, HORIZONTAL_GRID, Move, VERTICAL_GRID,
};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::widget::{
    TABLE_CELL_PADDING, TABLE_FIRST_TEXT_X, TABLE_HEADER_HEIGHT, TABLE_MARK_X, TABLE_ROW_HEIGHT,
    clipped_text, focus_ring,
};

const MOST_COLUMNS: usize = 16;
const DOUBLE_CLICK_MS: u32 = 400;
const TEXT_TOP: i32 = 4;
const HEADER_TEXT_TOP: i32 = 6;
const HEADER_SEPARATOR: (i32, i32) = (6, 22);
const DEFAULT_WIDTH: f64 = 100.0;
const DEFAULT_MINIMUM: f64 = 10.0;
const DEFAULT_MAXIMUM: f64 = 1000.0;
const USER_RESIZING: usize = 2;
const RIGHT_ALIGNED: usize = 2;
const KEY_SCROLL: i32 = 48;
const MARK: &str = ">";
const KEY_UP: u16 = 0x7e;
const KEY_DOWN: u16 = 0x7d;
const KEY_LEFT: u16 = 0x7b;
const KEY_RIGHT: u16 = 0x7c;
const KEY_HOME: u16 = 0x73;
const KEY_END: u16 = 0x77;
const KEY_PAGE_UP: u16 = 0x74;
const KEY_PAGE_DOWN: u16 = 0x79;
const KEY_RETURN: u16 = 0x24;
const KEY_ENTER: u16 = 0x4c;
const KEY_ESCAPE: u16 = 0x35;
const NO_INDEX: isize = -1;
const NOT_FOUND: usize = isize::MAX as usize;

#[derive(Debug, Clone, Copy)]
struct Resize {
    column: usize,
    from: i32,
    start: f64,
}

#[repr(C)]
#[derive(Debug)]
struct TableFields {
    view: view::ViewFields,
    columns: [Id; MOST_COLUMNS],
    column_count: usize,
    data_source: Id,
    delegate: Id,
    target: Id,
    action: usize,
    double_action: usize,
    rows: usize,
    rows_known: bool,
    selected: isize,
    clicked_row: isize,
    clicked_column: isize,
    last_press: Option<(usize, u32)>,
    resize: Option<Resize>,
    header: bool,
    alternating: bool,
    grid: usize,
    disabled: bool,
    refuses_empty: bool,
    background: Id,
}

#[repr(C)]
#[derive(Debug)]
struct ColumnFields {
    isa: usize,
    identifier: Id,
    title: Id,
    width: f64,
    minimum: f64,
    maximum: f64,
    resizing: usize,
    alignment: usize,
    hidden: bool,
    table: Id,
}

#[repr(C)]
#[derive(Debug)]
struct IndexSetFields {
    isa: usize,
    index: isize,
}

fn table(object: Id) -> &'static mut TableFields {
    // SAFETY: the callers pass NSTableView instances, whose memory starts with these fields;
    // the program's single event thread is the only one that reaches them.
    unsafe { &mut *object.cast::<TableFields>() }
}

fn column(object: Id) -> &'static mut ColumnFields {
    // SAFETY: the callers pass NSTableColumn instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<ColumnFields>() }
}

fn index_set(object: Id) -> &'static mut IndexSetFields {
    // SAFETY: the callers pass NSIndexSet instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<IndexSetFields>() }
}

fn columns(this: Id) -> &'static [Id] {
    let fields = table(this);
    &fields.columns[..fields.column_count]
}

fn shown_widths(this: Id) -> ([i32; MOST_COLUMNS], [usize; MOST_COLUMNS], usize) {
    let mut widths = [0; MOST_COLUMNS];
    let mut which = [0; MOST_COLUMNS];
    let mut count = 0;
    for (index, &each) in columns(this).iter().enumerate() {
        let fields = column(each);
        if fields.hidden {
            continue;
        }
        widths[count] = round(fields.width).max(0);
        which[count] = index;
        count += 1;
    }
    (widths, which, count)
}

fn clip_of(this: Id) -> ((i32, i32), (i32, i32)) {
    let superview = view::fields(this).superview;
    scroll_view::clip_offset(superview).unwrap_or_else(|| {
        let frame = view::fields(this).frame;
        ((0, 0), (round(frame.width), round(frame.height)))
    })
}

fn row_count(this: Id) -> usize {
    let fields = table(this);
    if !fields.rows_known {
        let source = fields.data_source;
        fields.rows = match responds(source, sel!("numberOfRowsInTableView:")) {
            true => send1(source, sel!("numberOfRowsInTableView:"), this as usize) as usize,
            false => 0,
        };
        fields.rows_known = true;
    }
    table(this).rows
}

fn changed(this: Id) {
    let superview = view::fields(this).superview;
    let target = match superview.is_null() {
        true => this,
        false => superview,
    };
    send1(target, sel!("setNeedsDisplay:"), 1);
}

fn fit_frame(this: Id) {
    let (_, (width, _)) = clip_of(this);
    let (widths, _, count) = shown_widths(this);
    let columns_width: i32 = widths[..count].iter().sum();
    let height = geometry::table_height(table(this).header, row_count(this));
    let frame = &mut view::fields(this).frame;
    let wanted = (f64::from(width.max(columns_width)), f64::from(height));
    if (frame.width, frame.height) != wanted {
        frame.width = wanted.0;
        frame.height = wanted.1;
        scroll_view::document_resized(view::fields(this).superview);
    }
    changed(this);
}

fn notify(this: Id, name: &str, delegate_sel: Sel) {
    let notification = notification_with_name(string(name), this);
    let delegate = table(this).delegate;
    if responds(delegate, delegate_sel) {
        send1(delegate, delegate_sel, notification as usize);
    }
    post(notification);
}

fn body_height(this: Id) -> i32 {
    let (_, (_, height)) = clip_of(this);
    height - geometry::body_top(table(this).header)
}

fn reveal(this: Id, row: usize) {
    let ((x, y), _) = clip_of(this);
    let moved = geometry::reveal_row(row, body_height(this), y, row_count(this));
    if moved != y {
        scroll_view::scroll_clip(view::fields(this).superview, (x, moved));
    }
}

fn select(this: Id, row: isize, scroll: bool) -> bool {
    if row == table(this).selected {
        return true;
    }
    if row >= 0 {
        let delegate = table(this).delegate;
        let asking = sel!("tableView:shouldSelectRow:");
        if responds(delegate, asking) {
            let ask: extern "C" fn(Id, Sel, Id, isize) -> u8 = message::msg_send_as();
            if ask(delegate, asking, this, row) == 0 {
                return false;
            }
        }
    }
    table(this).selected = row;
    if scroll && row >= 0 {
        reveal(this, row as usize);
    }
    changed(this);
    notify(
        this,
        "NSTableViewSelectionDidChangeNotification",
        sel!("tableViewSelectionDidChange:"),
    );
    true
}

fn send_action(this: Id, double: bool) {
    let fields = table(this);
    let action = match (double, fields.double_action) {
        (true, double_action) if double_action != 0 => double_action,
        _ => fields.action,
    };
    if action != 0 && !fields.disabled {
        application::send_action(action as Sel, fields.target, this);
    }
}

fn cell_text(this: Id, column_object: Id, row: usize, work: impl FnOnce(&str)) {
    let source = table(this).data_source;
    let asking = sel!("tableView:objectValueForTableColumn:row:");
    if !responds(source, asking) {
        return work("");
    }
    let ask: extern "C" fn(Id, Sel, Id, Id, isize) -> Id = message::msg_send_as();
    let value = ask(source, asking, this, column_object, row as isize);
    let text = match value.is_null() {
        true => value,
        false => send(value, sel!("description")),
    };
    with_text(text, work);
}

fn local_pixel(this: Id, event: Id) -> (i32, i32) {
    let origin = view::origin_in_window(this);
    let location = event::fields(event).location;
    let height = view::fields(this).frame.height;
    (
        round(location.x - origin.x),
        round(height - (location.y - origin.y)) - 1,
    )
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    let fields = table(this);
    fields.selected = NO_INDEX;
    fields.clicked_row = NO_INDEX;
    fields.clicked_column = NO_INDEX;
    fields.header = true;
    this
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = table(this);
    for &each in &fields.columns[..fields.column_count] {
        column(each).table = core::ptr::null_mut();
        objc_release(each);
    }
    fields.column_count = 0;
    store(&mut fields.background, core::ptr::null_mut());
    view::dispose(this);
}

extern "C" fn add_table_column(this: Id, _: Sel, added: Id) {
    let fields = table(this);
    if added.is_null() || fields.column_count == MOST_COLUMNS {
        return;
    }
    fields.columns[fields.column_count] = objc_retain(added);
    fields.column_count += 1;
    column(added).table = this;
    fit_frame(this);
}

extern "C" fn remove_table_column(this: Id, _: Sel, removed: Id) {
    let fields = table(this);
    let Some(at) = fields.columns[..fields.column_count]
        .iter()
        .position(|&each| each == removed)
    else {
        return;
    };
    fields.columns.copy_within(at + 1..fields.column_count, at);
    fields.column_count -= 1;
    column(removed).table = core::ptr::null_mut();
    objc_release(removed);
    fit_frame(this);
}

extern "C" fn move_column(this: Id, _: Sel, from: isize, to: isize) {
    let fields = table(this);
    let count = fields.column_count as isize;
    if !(0..count).contains(&from) || !(0..count).contains(&to) {
        return;
    }
    let moving = fields.columns[from as usize];
    let (from, to) = (from as usize, to as usize);
    match from < to {
        true => fields.columns.copy_within(from + 1..=to, from),
        false => fields.columns.copy_within(to..from, to + 1),
    }
    fields.columns[to] = moving;
    changed(this);
}

extern "C" fn table_columns(this: Id, _: Sel) -> Id {
    let shown = columns(this);
    send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        shown.as_ptr() as usize,
        shown.len(),
    )
}

fn column_index(this: Id, identifier: Id) -> Option<usize> {
    columns(this).iter().position(|&each| {
        let own = column(each).identifier;
        own == identifier
            || (!own.is_null()
                && !identifier.is_null()
                && send1(own, sel!("isEqual:"), identifier as usize) as usize & 0xff != 0)
    })
}

extern "C" fn table_column_with_identifier(this: Id, _: Sel, identifier: Id) -> Id {
    column_index(this, identifier).map_or(core::ptr::null_mut(), |at| columns(this)[at])
}

extern "C" fn column_with_identifier(this: Id, _: Sel, identifier: Id) -> isize {
    column_index(this, identifier).map_or(NO_INDEX, |at| at as isize)
}

extern "C" fn number_of_columns(this: Id, _: Sel) -> isize {
    table(this).column_count as isize
}

extern "C" fn data_source(this: Id, _: Sel) -> Id {
    table(this).data_source
}

extern "C" fn set_data_source(this: Id, _: Sel, source: Id) {
    table(this).data_source = source;
    table(this).rows_known = false;
    fit_frame(this);
}

extern "C" fn delegate(this: Id, _: Sel) -> Id {
    table(this).delegate
}

extern "C" fn set_delegate(this: Id, _: Sel, delegate: Id) {
    table(this).delegate = delegate;
}

extern "C" fn target(this: Id, _: Sel) -> Id {
    table(this).target
}

extern "C" fn set_target(this: Id, _: Sel, target: Id) {
    table(this).target = target;
}

extern "C" fn action(this: Id, _: Sel) -> usize {
    table(this).action
}

extern "C" fn set_action(this: Id, _: Sel, action: usize) {
    table(this).action = action;
}

extern "C" fn double_action(this: Id, _: Sel) -> usize {
    table(this).double_action
}

extern "C" fn set_double_action(this: Id, _: Sel, action: usize) {
    table(this).double_action = action;
}

extern "C" fn reload_data(this: Id, _: Sel) {
    table(this).rows_known = false;
    let rows = row_count(this);
    let selected = table(this).selected;
    if selected >= rows as isize {
        select(this, NO_INDEX, false);
    }
    if table(this).refuses_empty && table(this).selected < 0 && rows > 0 {
        select(this, 0, true);
    }
    fit_frame(this);
}

extern "C" fn number_of_rows(this: Id, _: Sel) -> isize {
    row_count(this) as isize
}

extern "C" fn selected_row(this: Id, _: Sel) -> isize {
    table(this).selected
}

extern "C" fn clicked_row(this: Id, _: Sel) -> isize {
    table(this).clicked_row
}

extern "C" fn clicked_column(this: Id, _: Sel) -> isize {
    table(this).clicked_column
}

extern "C" fn number_of_selected_rows(this: Id, _: Sel) -> isize {
    isize::from(table(this).selected >= 0)
}

extern "C" fn select_row_indexes(this: Id, _: Sel, indexes: Id, _: u8) {
    let row = match indexes.is_null() {
        true => NO_INDEX,
        false => index_set(indexes).index,
    };
    if row < row_count(this) as isize {
        select(this, row, true);
    }
}

extern "C" fn selected_row_indexes(this: Id, _: Sel) -> Id {
    new_index_set(table(this).selected)
}

extern "C" fn deselect_all(this: Id, _: Sel, _: Id) {
    if !table(this).refuses_empty {
        select(this, NO_INDEX, false);
    }
}

extern "C" fn deselect_row(this: Id, _: Sel, row: isize) {
    if row == table(this).selected && !table(this).refuses_empty {
        select(this, NO_INDEX, false);
    }
}

extern "C" fn is_row_selected(this: Id, _: Sel, row: isize) -> u8 {
    u8::from(row >= 0 && row == table(this).selected)
}

extern "C" fn scroll_row_to_visible(this: Id, _: Sel, row: isize) {
    if row >= 0 && (row as usize) < row_count(this) {
        reveal(this, row as usize);
    }
}

extern "C" fn header_view(this: Id, _: Sel) -> Id {
    match table(this).header {
        true => this,
        false => core::ptr::null_mut(),
    }
}

extern "C" fn set_header_view(this: Id, _: Sel, header: Id) {
    table(this).header = !header.is_null();
    fit_frame(this);
}

extern "C" fn row_height(_: Id, _: Sel) -> f64 {
    f64::from(TABLE_ROW_HEIGHT)
}

extern "C" fn intercell_spacing(_: Id, _: Sel) -> Point {
    Point::default()
}

extern "C" fn uses_alternating(this: Id, _: Sel) -> u8 {
    u8::from(table(this).alternating)
}

extern "C" fn set_uses_alternating(this: Id, _: Sel, value: u8) {
    table(this).alternating = value != 0;
    changed(this);
}

extern "C" fn grid_style_mask(this: Id, _: Sel) -> usize {
    table(this).grid
}

extern "C" fn set_grid_style_mask(this: Id, _: Sel, mask: usize) {
    table(this).grid = mask;
    changed(this);
}

extern "C" fn is_enabled(this: Id, _: Sel) -> u8 {
    u8::from(!table(this).disabled)
}

extern "C" fn set_enabled(this: Id, _: Sel, value: u8) {
    table(this).disabled = value == 0;
    changed(this);
}

extern "C" fn allows_empty_selection(this: Id, _: Sel) -> u8 {
    u8::from(!table(this).refuses_empty)
}

extern "C" fn set_allows_empty_selection(this: Id, _: Sel, value: u8) {
    table(this).refuses_empty = value == 0;
}

extern "C" fn background_colour(this: Id, _: Sel) -> Id {
    table(this).background
}

extern "C" fn set_background_colour(this: Id, _: Sel, colour: Id) {
    store(&mut table(this).background, colour);
    changed(this);
}

extern "C" fn accepts_first_responder(this: Id, _: Sel) -> u8 {
    u8::from(!table(this).disabled)
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

extern "C" fn ignore_size(_: Id, _: Sel, _: Point) {}

extern "C" fn ignore_float(_: Id, _: Sel, _: f64) {}

fn header_press(this: Id, x: i32) {
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    let Some(shown) = geometry::handle_at(x, &lefts[..count], &widths[..count]) else {
        return;
    };
    let each = columns(this)[which[shown]];
    if column(each).resizing & USER_RESIZING == 0 {
        return;
    }
    table(this).resize = Some(Resize {
        column: which[shown],
        from: x,
        start: column(each).width,
    });
}

extern "C" fn mouse_down(this: Id, _: Sel, event: Id) {
    if table(this).disabled {
        return;
    }
    let window = view::fields(this).window;
    if !window.is_null() {
        window::make_first_responder(window, this);
    }
    let (x, y) = local_pixel(this, event);
    let ((_, offset), _) = clip_of(this);
    let header = table(this).header;
    let clip_y = y - offset;
    if header && clip_y < TABLE_HEADER_HEIGHT {
        header_press(this, x);
        return;
    }
    let rows = row_count(this);
    let Some(row) = geometry::row_at(clip_y, header, offset, rows) else {
        if !table(this).refuses_empty {
            select(this, NO_INDEX, false);
        }
        return;
    };
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    let clicked = geometry::column_at(x, &lefts[..count], &widths[..count]);
    table(this).clicked_row = row as isize;
    table(this).clicked_column = clicked.map_or(NO_INDEX, |shown| which[shown] as isize);
    let now = event::fields(event).time_ms;
    let double = table(this)
        .last_press
        .is_some_and(|(last, at)| last == row && now.wrapping_sub(at) <= DOUBLE_CLICK_MS);
    table(this).last_press = match double {
        true => None,
        false => Some((row, now)),
    };
    if !select(this, row as isize, true) {
        return;
    }
    send_action(this, double);
}

extern "C" fn mouse_dragged(this: Id, _: Sel, event: Id) {
    let Some(resize) = table(this).resize else {
        return;
    };
    let (x, _) = local_pixel(this, event);
    let each = columns(this)[resize.column];
    let fields = column(each);
    let width = geometry::resized(
        round(resize.start),
        x - resize.from,
        round(fields.minimum),
        round(fields.maximum),
    );
    fields.width = f64::from(width);
    fit_frame(this);
}

extern "C" fn mouse_up(this: Id, _: Sel, _: Id) {
    if table(this).resize.take().is_some() {
        notify(
            this,
            "NSTableViewColumnDidResizeNotification",
            sel!("tableViewColumnDidResize:"),
        );
    }
}

extern "C" fn key_down(this: Id, _: Sel, event: Id) {
    let code = event::fields(event).key_code;
    if code == KEY_ESCAPE
        && let Some(resize) = table(this).resize.take()
    {
        column(columns(this)[resize.column]).width = resize.start;
        fit_frame(this);
        return;
    }
    if table(this).disabled {
        return;
    }
    let step = match code {
        KEY_UP => Move::Up,
        KEY_DOWN => Move::Down,
        KEY_HOME => Move::Home,
        KEY_END => Move::End,
        KEY_PAGE_UP => Move::PageUp,
        KEY_PAGE_DOWN => Move::PageDown,
        KEY_RETURN | KEY_ENTER => {
            send_action(this, true);
            return;
        }
        KEY_LEFT | KEY_RIGHT => {
            let ((x, y), (width, _)) = clip_of(this);
            let total = round(view::fields(this).frame.width);
            let moved = match code == KEY_LEFT {
                true => x - KEY_SCROLL,
                false => x + KEY_SCROLL,
            };
            let moved = moved.clamp(0, (total - width).max(0));
            if moved != x {
                scroll_view::scroll_clip(view::fields(this).superview, (moved, y));
            }
            return;
        }
        _ => {
            let superview = view::fields(this).superview;
            if !superview.is_null() {
                send1(superview, sel!("keyDown:"), event as usize);
            }
            return;
        }
    };
    let visible = (body_height(this) / TABLE_ROW_HEIGHT).max(1) as usize;
    if let Some(row) = geometry::moved(table(this).selected, step, row_count(this), visible) {
        select(this, row as isize, true);
    }
}

fn surface_colour(this: Id) -> u32 {
    drawing::colour_of(table(this).background).map_or(colour::FIELD, |colour| colour.pixel)
}

fn draw_cells(
    this: Id,
    canvas: &mut Canvas<'_>,
    place: &dyn Fn(Rect) -> Rect,
    (row, rect): (usize, Rect),
    (lefts, widths, which): (&[i32], &[i32], &[usize]),
    text: u32,
) {
    for (shown, (&left, &width)) in lefts.iter().zip(widths).enumerate() {
        let each = columns(this)[which[shown]];
        let first = shown == 0;
        let start = match first {
            true => TABLE_FIRST_TEXT_X,
            false => TABLE_CELL_PADDING,
        };
        let space = width - start - TABLE_CELL_PADDING;
        let right = column(each).alignment == RIGHT_ALIGNED;
        cell_text(this, each, row, |cell| {
            let at = place(Rect::new(left + start, rect.y + TEXT_TOP, space, 16));
            let x = match right {
                true => at.x + (space - cell.chars().count() as i32 * 8).max(0),
                false => at.x,
            };
            clipped_text(canvas, x, at.y, space, cell, text);
        });
    }
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    let frame = view::fields(this).frame;
    let ((_, offset), (clip_width, clip_height)) = clip_of(this);
    let rows = row_count(this);
    let header = table(this).header;
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    let (lefts, widths, which) = (&lefts[..count], &widths[..count], &which[..count]);
    let width = round(frame.width).max(clip_width);
    let (first, last) = geometry::shown_rows(rows, clip_height - geometry::body_top(header), offset);
    let fields = table(this);
    let (selected, alternating, grid, disabled) =
        (fields.selected, fields.alternating, fields.grid, fields.disabled);
    let focused = {
        let window = view::fields(this).window;
        !window.is_null() && window::first_responder(window) == this
    };
    let surface = surface_colour(this);
    let text = match disabled {
        true => colour::TEXT_DIM,
        false => colour::TEXT,
    };
    drawing::with_canvas(|canvas, height, origin, _, _| {
        let outer = pixel_rect(Frame::new(origin.x, origin.y, frame.width, frame.height), height);
        let place = |rect: Rect| rect.offset(outer.x, outer.y);
        canvas.fill(place(Rect::new(0, 0, width, round(frame.height))), surface);
        for row in first..last {
            let rect = Rect::new(0, geometry::body_top(header) + row as i32 * TABLE_ROW_HEIGHT, width, TABLE_ROW_HEIGHT);
            let fill = if row as isize == selected {
                colour::SELECTION
            } else if alternating && geometry::is_alternate(row) {
                colour::MENU
            } else {
                surface
            };
            canvas.fill(place(rect), fill);
            if grid & (HORIZONTAL_GRID | DASHED_HORIZONTAL_GRID) != 0 {
                canvas.fill(place(Rect::new(0, rect.bottom() - 1, width, 1)), colour::MENU_BORDER);
            }
            if grid & VERTICAL_GRID != 0 {
                for (&left, &column_width) in lefts.iter().zip(widths) {
                    canvas.fill(place(Rect::new(left + column_width - 1, rect.y, 1, TABLE_ROW_HEIGHT)), colour::MENU_BORDER);
                }
            }
            if row as isize == selected && !disabled {
                let at = place(Rect::new(TABLE_MARK_X, rect.y + TEXT_TOP, 8, 16));
                let _ = canvas.text(at.x, at.y, MARK, colour::ACCENT, 1);
                if focused {
                    focus_ring(canvas, place(Rect::new(0, rect.y, clip_width, TABLE_ROW_HEIGHT)), 0);
                }
            }
            draw_cells(this, canvas, &place, (row, rect), (lefts, widths, which), text);
        }
        if !header {
            return;
        }
        let top = offset;
        canvas.fill(place(Rect::new(0, top, width, TABLE_HEADER_HEIGHT)), colour::MENU);
        canvas.fill(place(Rect::new(0, top + TABLE_HEADER_HEIGHT - 1, width, 1)), colour::MENU_BORDER);
        for (shown, (&left, &column_width)) in lefts.iter().zip(widths).enumerate() {
            let separator = Rect::new(
                left + column_width - 1,
                top + HEADER_SEPARATOR.0,
                1,
                HEADER_SEPARATOR.1 - HEADER_SEPARATOR.0,
            );
            canvas.fill(place(separator), colour::MENU_BORDER);
            let each = columns(this)[which[shown]];
            let space = column_width - 2 * TABLE_CELL_PADDING;
            with_text(column(each).title, |title| {
                let at = place(Rect::new(left + TABLE_CELL_PADDING, top + HEADER_TEXT_TOP, space, 16));
                let x = match column(each).alignment == RIGHT_ALIGNED {
                    true => at.x + (space - title.chars().count() as i32 * 8).max(0),
                    false => at.x,
                };
                clipped_text(canvas, x, at.y, space, title, colour::TEXT_DIM);
            });
        }
    });
}

extern "C" fn view_did_move_to_window(this: Id, _: Sel) {
    fit_frame(this);
}

extern "C" fn column_init_with_identifier(this: Id, _: Sel, identifier: Id) -> Id {
    if this.is_null() {
        return this;
    }
    let fields = column(this);
    store_copy(&mut fields.identifier, identifier);
    store_copy(&mut fields.title, string(""));
    fields.width = DEFAULT_WIDTH;
    fields.minimum = DEFAULT_MINIMUM;
    fields.maximum = DEFAULT_MAXIMUM;
    fields.resizing = USER_RESIZING | 1;
    this
}

extern "C" fn column_init(this: Id, _: Sel) -> Id {
    column_init_with_identifier(this, sel!("initWithIdentifier:"), string(""))
}

extern "C" fn column_dealloc(this: Id, _: Sel) {
    let fields = column(this);
    store(&mut fields.identifier, core::ptr::null_mut());
    store(&mut fields.title, core::ptr::null_mut());
    crate::objc::reference_count::dispose(this);
}

fn column_changed(this: Id) {
    let owner = column(this).table;
    if !owner.is_null() {
        fit_frame(owner);
    }
}

extern "C" fn column_identifier(this: Id, _: Sel) -> Id {
    column(this).identifier
}

extern "C" fn column_set_identifier(this: Id, _: Sel, identifier: Id) {
    store_copy(&mut column(this).identifier, identifier);
}

extern "C" fn column_title(this: Id, _: Sel) -> Id {
    column(this).title
}

extern "C" fn column_set_title(this: Id, _: Sel, title: Id) {
    store_copy(&mut column(this).title, title);
    column_changed(this);
}

extern "C" fn column_width(this: Id, _: Sel) -> f64 {
    column(this).width
}

extern "C" fn column_set_width(this: Id, _: Sel, width: f64) {
    let fields = column(this);
    fields.width = width.clamp(fields.minimum, fields.maximum.max(fields.minimum));
    column_changed(this);
}

extern "C" fn column_min_width(this: Id, _: Sel) -> f64 {
    column(this).minimum
}

extern "C" fn column_set_min_width(this: Id, _: Sel, width: f64) {
    column(this).minimum = width.max(0.0);
}

extern "C" fn column_max_width(this: Id, _: Sel) -> f64 {
    column(this).maximum
}

extern "C" fn column_set_max_width(this: Id, _: Sel, width: f64) {
    column(this).maximum = width.max(0.0);
}

extern "C" fn column_resizing_mask(this: Id, _: Sel) -> usize {
    column(this).resizing
}

extern "C" fn column_set_resizing_mask(this: Id, _: Sel, mask: usize) {
    column(this).resizing = mask;
}

extern "C" fn column_is_hidden(this: Id, _: Sel) -> u8 {
    u8::from(column(this).hidden)
}

extern "C" fn column_set_hidden(this: Id, _: Sel, hidden: u8) {
    column(this).hidden = hidden != 0;
    column_changed(this);
}

extern "C" fn column_table_view(this: Id, _: Sel) -> Id {
    column(this).table
}

extern "C" fn column_header_cell(this: Id, _: Sel) -> Id {
    this
}

extern "C" fn column_alignment(this: Id, _: Sel) -> usize {
    column(this).alignment
}

extern "C" fn column_set_alignment(this: Id, _: Sel, alignment: usize) {
    column(this).alignment = alignment;
    column_changed(this);
}

fn new_index_set(index: isize) -> Id {
    let object = new_instance(class_pointer(&NSINDEXSET));
    if !object.is_null() {
        index_set(object).index = index;
    }
    crate::foundation::autoreleased(object)
}

extern "C" fn index_set_with_index(_: Id, _: Sel, index: usize) -> Id {
    new_index_set(index as isize)
}

extern "C" fn index_set_empty(_: Id, _: Sel) -> Id {
    new_index_set(NO_INDEX)
}

extern "C" fn index_set_init(this: Id, _: Sel) -> Id {
    if !this.is_null() {
        index_set(this).index = NO_INDEX;
    }
    this
}

extern "C" fn index_set_init_with_index(this: Id, _: Sel, index: usize) -> Id {
    if !this.is_null() {
        index_set(this).index = index as isize;
    }
    this
}

extern "C" fn index_set_first_index(this: Id, _: Sel) -> usize {
    match index_set(this).index {
        NO_INDEX => NOT_FOUND,
        index => index as usize,
    }
}

extern "C" fn index_set_count(this: Id, _: Sel) -> usize {
    usize::from(index_set(this).index >= 0)
}

extern "C" fn index_set_contains(this: Id, _: Sel, index: usize) -> u8 {
    u8::from(index_set(this).index >= 0 && index_set(this).index as usize == index)
}

extern "C" fn index_set_copy(this: Id, _: Sel, _: usize) -> Id {
    objc_retain(this)
}

/// A table as agents see it (D378): its selected row (-1 for none), its rows and whether it
/// is disabled.
pub fn summary(this: Id) -> (isize, usize, bool) {
    (table(this).selected, row_count(this), table(this).disabled)
}

/// The rows shown now: first and one past the last.
pub fn shown_rows(this: Id) -> (usize, usize) {
    let ((_, offset), _) = clip_of(this);
    geometry::shown_rows(row_count(this), body_height(this), offset)
}

/// The header cells shown, each the column and its rect in the table's own pixels (top-left
/// origin), kept at the top of the visible part; none without a header.
pub fn header_cells(this: Id, work: &mut dyn FnMut(usize, Id, Rect) -> bool) -> bool {
    if !table(this).header {
        return true;
    }
    let ((_, offset), _) = clip_of(this);
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    for shown in 0..count {
        let rect = Rect::new(lefts[shown], offset, widths[shown], TABLE_HEADER_HEIGHT);
        if !work(shown, columns(this)[which[shown]], rect) {
            return false;
        }
    }
    true
}

/// Row `row`'s rect in the table's own pixels.
pub fn row_rect(this: Id, row: usize) -> Rect {
    let width = round(view::fields(this).frame.width);
    let top = geometry::body_top(table(this).header) + row as i32 * TABLE_ROW_HEIGHT;
    Rect::new(0, top, width, TABLE_ROW_HEIGHT)
}

/// The table's own pixel rect's top-left in its window's content pixels.
pub fn origin_pixel(this: Id) -> (i32, i32) {
    let origin = view::origin_in_window(this);
    let frame = view::fields(this).frame;
    let window = view::fields(this).window;
    let height = match window.is_null() {
        true => 0,
        false => window::fields(window).buffer.height as i32,
    };
    let rect = pixel_rect(Frame::new(origin.x, origin.y, frame.width, frame.height), height);
    (rect.x, rect.y)
}

/// Runs `work` with row `row`'s first cell and every cell joined by tabs, into `joined`.
pub fn with_row_text<R>(this: Id, row: usize, joined: &mut [u8], work: impl FnOnce(&str, &str) -> R) -> R {
    let (_, which, count) = shown_widths(this);
    let mut length = 0;
    let mut first_end = 0;
    for shown in 0..count {
        if shown > 0 && length < joined.len() {
            joined[length] = b'\t';
            length += 1;
        }
        cell_text(this, columns(this)[which[shown]], row, |cell| {
            let mut take = cell.len().min(joined.len() - length);
            while !cell.is_char_boundary(take) {
                take -= 1;
            }
            joined[length..length + take].copy_from_slice(&cell.as_bytes()[..take]);
            length += take;
        });
        if shown == 0 {
            first_end = length;
        }
    }
    let all = core::str::from_utf8(&joined[..length]).unwrap_or("");
    work(&all[..first_end.min(all.len())], all)
}

/// An agent's `select` on row `row`: chosen as a click chooses it, `action` sent; false when
/// refused.
pub fn agent_select(this: Id, row: usize, double: bool) -> bool {
    if table(this).disabled || row >= row_count(this) {
        return false;
    }
    table(this).clicked_row = row as isize;
    table(this).clicked_column = NO_INDEX;
    if !select(this, row as isize, true) {
        return false;
    }
    send_action(this, double);
    true
}

/// An agent's `set-text` on header cell `shown`: the column's width set as a drag sets it;
/// false when it refuses resizing.
pub fn agent_resize(this: Id, shown: usize, width: i32) -> bool {
    let (_, which, count) = shown_widths(this);
    if shown >= count {
        return false;
    }
    let each = columns(this)[which[shown]];
    let fields = column(each);
    if fields.resizing & USER_RESIZING == 0 {
        return false;
    }
    let width = geometry::resized(width, 0, round(fields.minimum), round(fields.maximum));
    fields.width = f64::from(width);
    fit_frame(this);
    notify(
        this,
        "NSTableViewColumnDidResizeNotification",
        sel!("tableViewColumnDidResize:"),
    );
    true
}

/// The title of column `object`.
pub fn column_title(object: Id) -> Id {
    column(object).title
}

pub fn define(runtime: &mut Runtime) -> bool {
    let table_methods = methods![
        "initWithFrame:" => init_with_frame,
        "dealloc" => dealloc,
        "addTableColumn:" => add_table_column,
        "removeTableColumn:" => remove_table_column,
        "moveColumn:toColumn:" => move_column,
        "tableColumns" => table_columns,
        "tableColumnWithIdentifier:" => table_column_with_identifier,
        "columnWithIdentifier:" => column_with_identifier,
        "numberOfColumns" => number_of_columns,
        "dataSource" => data_source,
        "setDataSource:" => set_data_source,
        "delegate" => delegate,
        "setDelegate:" => set_delegate,
        "target" => target,
        "setTarget:" => set_target,
        "action" => action,
        "setAction:" => set_action,
        "doubleAction" => double_action,
        "setDoubleAction:" => set_double_action,
        "reloadData" => reload_data,
        "noteNumberOfRowsChanged" => reload_data,
        "numberOfRows" => number_of_rows,
        "selectedRow" => selected_row,
        "clickedRow" => clicked_row,
        "clickedColumn" => clicked_column,
        "numberOfSelectedRows" => number_of_selected_rows,
        "selectRowIndexes:byExtendingSelection:" => select_row_indexes,
        "selectedRowIndexes" => selected_row_indexes,
        "deselectAll:" => deselect_all,
        "deselectRow:" => deselect_row,
        "isRowSelected:" => is_row_selected,
        "scrollRowToVisible:" => scroll_row_to_visible,
        "headerView" => header_view,
        "setHeaderView:" => set_header_view,
        "rowHeight" => row_height,
        "setRowHeight:" => ignore_float,
        "intercellSpacing" => intercell_spacing,
        "setIntercellSpacing:" => ignore_size,
        "usesAlternatingRowBackgroundColors" => uses_alternating,
        "setUsesAlternatingRowBackgroundColors:" => set_uses_alternating,
        "gridStyleMask" => grid_style_mask,
        "setGridStyleMask:" => set_grid_style_mask,
        "isEnabled" => is_enabled,
        "setEnabled:" => set_enabled,
        "allowsEmptySelection" => allows_empty_selection,
        "setAllowsEmptySelection:" => set_allows_empty_selection,
        "setAllowsMultipleSelection:" => ignore_word,
        "setAllowsColumnReordering:" => ignore_word,
        "setAllowsColumnResizing:" => ignore_word,
        "setColumnAutoresizingStyle:" => ignore_word,
        "setSelectionHighlightStyle:" => ignore_word,
        "setGridColor:" => ignore_word,
        "setSortDescriptors:" => ignore_word,
        "backgroundColor" => background_colour,
        "setBackgroundColor:" => set_background_colour,
        "acceptsFirstResponder" => accepts_first_responder,
        "mouseDown:" => mouse_down,
        "mouseDragged:" => mouse_dragged,
        "mouseUp:" => mouse_up,
        "keyDown:" => key_down,
        "drawRect:" => draw_rect,
        "viewDidMoveToWindow" => view_did_move_to_window,
    ];
    let column_methods = methods![
        "initWithIdentifier:" => column_init_with_identifier,
        "init" => column_init,
        "dealloc" => column_dealloc,
        "identifier" => column_identifier,
        "setIdentifier:" => column_set_identifier,
        "title" => column_title,
        "setTitle:" => column_set_title,
        "stringValue" => column_title,
        "setStringValue:" => column_set_title,
        "width" => column_width,
        "setWidth:" => column_set_width,
        "minWidth" => column_min_width,
        "setMinWidth:" => column_set_min_width,
        "maxWidth" => column_max_width,
        "setMaxWidth:" => column_set_max_width,
        "resizingMask" => column_resizing_mask,
        "setResizingMask:" => column_set_resizing_mask,
        "isHidden" => column_is_hidden,
        "setHidden:" => column_set_hidden,
        "tableView" => column_table_view,
        "headerCell" => column_header_cell,
        "dataCell" => column_header_cell,
        "alignment" => column_alignment,
        "setAlignment:" => column_set_alignment,
        "setEditable:" => ignore_word,
        "setSortDescriptorPrototype:" => ignore_word,
    ];
    let index_set_methods = methods![
        "init" => index_set_init,
        "initWithIndex:" => index_set_init_with_index,
        "firstIndex" => index_set_first_index,
        "lastIndex" => index_set_first_index,
        "count" => index_set_count,
        "containsIndex:" => index_set_contains,
        "copyWithZone:" => index_set_copy,
    ];
    let index_set_class_methods = methods![
        "indexSetWithIndex:" => index_set_with_index,
        "indexSet" => index_set_empty,
    ];
    super::define(
        runtime,
        b"NSTableView\0",
        (&NSTABLEVIEW, &NSTABLEVIEW_META),
        &NSVIEW,
        core::mem::size_of::<TableFields>(),
        &table_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSTableColumn\0",
        (&NSTABLECOLUMN, &NSTABLECOLUMN_META),
        &NSOBJECT,
        core::mem::size_of::<ColumnFields>(),
        &column_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSIndexSet\0",
        (&NSINDEXSET, &NSINDEXSET_META),
        &NSOBJECT,
        core::mem::size_of::<IndexSetFields>(),
        &index_set_methods,
        &index_set_class_methods,
    )
}

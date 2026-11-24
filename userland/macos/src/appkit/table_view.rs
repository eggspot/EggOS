//! NSTableView and NSTableColumn (D373, docs/ux/macos-gui-v0.3.md section 11): a cell-based
//! table with a data source, the document view of a scroll view, drawn as the toolkit's table:
//! the header kept at the top of the clip view, 24-pixel rows asked of the data source at every
//! paint, selection by pointer and keys with the `>` mark and the focus ring, actions, double
//! actions and the delegate's notifications, alternating rows, grid lines and column resizing
//! from the header. The selection is an NSMutableIndexSet (D725): with
//! `allowsMultipleSelection`, Command-click toggles a row, Shift-click and Shift with the
//! arrow keys extend from the anchor row, and `selectAll:` takes every row. The row under the
//! pointer takes the hover fill (D726), through a tracking area the table keeps on itself.
//! A column with a `sortDescriptorPrototype` sorts by a click on its title, which cycles the
//! table's `sortDescriptors` and tells the data source; its title brightens and the toolkit's
//! chevron shows the order (D735, docs/ux/macos-tables-v0.4.md section 1). Rows take
//! `rowHeight` or the delegate's `tableView:heightOfRow:`, kept as running tops (D736). When the
//! delegate answers `tableView:viewForTableColumn:row:`, the visible cells are its views, made
//! for the rows in view only and reused by identifier (`NSTableCellView`, D737).

use super::{
    NSTABLECELLVIEW, NSTABLECELLVIEW_META, NSTABLECOLUMN, NSTABLECOLUMN_META, NSTABLEVIEW,
    NSTABLEVIEW_META, NSTRACKINGAREA, NSVIEW, application, drawing, event, responds, scroll_view,
    send_frame, store, store_copy, string, view, window, with_text,
};
use crate::foundation::index_set;
use crate::foundation::sort_descriptor;
use crate::foundation::{NSARRAY, class_pointer, notification_with_name, post};
use crate::objc::buffer::Buffer;
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1, send2};
use eggos_macos_gui::geometry::{Frame, Point, pixel_rect, round};
use eggos_macos_gui::table_view::{
    self as geometry, DASHED_HORIZONTAL_GRID, HORIZONTAL_GRID, Heights, Move, TABLE_ROW_MIN,
    VARIABLE_HEIGHT_ROWS, VERTICAL_GRID, clamped_height, text_top,
};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::colour;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::widget::{
    TABLE_ARROW_ROOM, TABLE_CELL_PADDING, TABLE_FIRST_TEXT_X, TABLE_HEADER_HEIGHT, TABLE_MARK_X,
    TABLE_ROW_HEIGHT, chevron, clipped_text, focus_ring,
};

const MOST_COLUMNS: usize = 16;
const DOUBLE_CLICK_MS: u32 = 400;
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
const CHEVRON_TOP: i32 = 11;
const VISIBLE_CELL_VIEWS: usize = 512;
const MARK_COLUMN: i32 = 16;
const CELL_INSET: f64 = 8.0;
const CELL_PART: f64 = 16.0;
const TEXT_AFTER_IMAGE: f64 = 32.0;
const EMPHASIZED: isize = 1;
const CHEVRON_WIDTH: i32 = 10;
const SHIFT_FLAG: u64 = 1 << 17;
const COMMAND_FLAG: u64 = 1 << 20;
const TRACK_ENTERED_AND_EXITED: usize = 0x01;
const TRACK_MOUSE_MOVED: usize = 0x02;
const TRACK_ACTIVE_ALWAYS: usize = 0x80;
const TRACK_IN_VISIBLE_RECT: usize = 0x200;

#[derive(Debug, Clone, Copy)]
struct CellPlace {
    row: usize,
    column: usize,
    view: Id,
}

#[repr(C)]
#[derive(Debug)]
struct CellViewFields {
    view: view::ViewFields,
    text_field: Id,
    image_view: Id,
    object_value: Id,
    background_style: isize,
}

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
    selection: Id,
    anchor: isize,
    multiple: bool,
    hover: isize,
    tracking: Id,
    sort_descriptors: Id,
    header_hover: isize,
    header_pressed: isize,
    highlighted_column: Id,
    row_height: f64,
    tops: Buffer<u32>,
    heights_known: bool,
    cells: Buffer<CellPlace>,
    reuse: Buffer<Id>,
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
    sort_prototype: Id,
    indicator_image: Id,
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

fn uniform_height(this: Id) -> i32 {
    let wanted = match table(this).row_height {
        height if height > 0.0 => height,
        _ => f64::from(TABLE_ROW_HEIGHT),
    };
    clamped_height(wanted, TABLE_ROW_MIN)
}

fn heights(this: Id) -> Heights<'static> {
    let rows = row_count(this);
    let delegate = table(this).delegate;
    let asking = sel!("tableView:heightOfRow:");
    if rows > VARIABLE_HEIGHT_ROWS || !responds(delegate, asking) {
        return Heights::Uniform {
            height: uniform_height(this),
            rows,
        };
    }
    if !table(this).heights_known {
        let ask: extern "C" fn(Id, Sel, Id, isize) -> f64 = message::msg_send_as();
        let mut tops: Buffer<u32> = Buffer::new();
        tops.reserve(rows + 1);
        tops.push(0);
        let mut at: u32 = 0;
        for row in 0..rows {
            let height = clamped_height(ask(delegate, asking, this, row as isize), TABLE_ROW_MIN);
            at = at.saturating_add(height as u32);
            tops.push(at);
        }
        let mut old = core::mem::replace(&mut table(this).tops, tops);
        old.free();
        table(this).heights_known = true;
    }
    Heights::Varied(table(this).tops.as_slice())
}

fn fit_frame(this: Id) {
    let (_, (width, _)) = clip_of(this);
    let (widths, _, count) = shown_widths(this);
    let columns_width: i32 = widths[..count].iter().sum();
    let height = geometry::body_top(table(this).header) + heights(this).total();
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
    let moved = heights(this).reveal(row, body_height(this), y);
    if moved != y {
        scroll_view::scroll_clip(view::fields(this).superview, (x, moved));
    }
}

fn copy_of_selection(this: Id) -> Id {
    let wanted = index_set::new_mutable();
    if !wanted.is_null() {
        for &(start, end) in index_set::each_range(table(this).selection) {
            index_set::insert(wanted, start, end);
        }
    }
    wanted
}

fn refuse_unwanted(this: Id, wanted: Id) {
    let delegate = table(this).delegate;
    let asking = sel!("tableView:shouldSelectRow:");
    if !responds(delegate, asking) {
        return;
    }
    let ask: extern "C" fn(Id, Sel, Id, isize) -> u8 = message::msg_send_as();
    let mut ranges: Buffer<(usize, usize)> = Buffer::new();
    for &range in index_set::each_range(wanted) {
        ranges.push(range);
    }
    let current = table(this).selection;
    for &(start, end) in ranges.as_slice() {
        for row in start..end {
            let new = !index_set::contains(current, row);
            if new && ask(delegate, asking, this, row as isize) == 0 {
                index_set::remove(wanted, row, row + 1);
            }
        }
    }
    ranges.free();
}

fn last_of(set: Id) -> isize {
    index_set::each_range(set)
        .last()
        .map_or(NO_INDEX, |&(_, end)| end as isize - 1)
}

fn apply(this: Id, wanted: Id, last: isize, scroll: bool) -> bool {
    if wanted.is_null() {
        return false;
    }
    refuse_unwanted(this, wanted);
    let landed = last < 0 || index_set::contains(wanted, last as usize);
    let selected = match landed && last >= 0 {
        true => last,
        false => last_of(wanted),
    };
    let current = table(this).selection;
    if index_set::each_range(wanted) == index_set::each_range(current) {
        objc_release(wanted);
        table(this).selected = selected;
        return landed;
    }
    table(this).selection = wanted;
    objc_release(current);
    table(this).selected = selected;
    if scroll && selected >= 0 {
        reveal(this, selected as usize);
    }
    changed(this);
    notify(
        this,
        "NSTableViewSelectionDidChangeNotification",
        sel!("tableViewSelectionDidChange:"),
    );
    landed
}

fn select(this: Id, row: isize, scroll: bool) -> bool {
    let wanted = index_set::new_mutable();
    if !wanted.is_null() && row >= 0 {
        index_set::insert(wanted, row as usize, row as usize + 1);
    }
    if row >= 0 {
        table(this).anchor = row;
    }
    apply(this, wanted, row, scroll)
}

fn extend_to(this: Id, row: usize, scroll: bool) -> bool {
    let anchor = match table(this).anchor {
        NO_INDEX => row,
        anchor => anchor as usize,
    };
    let wanted = index_set::new_mutable();
    if !wanted.is_null() {
        index_set::insert(wanted, anchor.min(row), anchor.max(row) + 1);
    }
    apply(this, wanted, row as isize, scroll)
}

fn toggle(this: Id, row: usize) -> bool {
    let wanted = copy_of_selection(this);
    if wanted.is_null() {
        return false;
    }
    table(this).anchor = row as isize;
    if index_set::contains(wanted, row) {
        index_set::remove(wanted, row, row + 1);
        if index_set::count(wanted) == 0 && table(this).refuses_empty {
            objc_release(wanted);
            return false;
        }
        let last = last_of(wanted);
        apply(this, wanted, last, false);
        return false;
    }
    index_set::insert(wanted, row, row + 1);
    apply(this, wanted, row as isize, true)
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
    if view_based(this) {
        let index = columns(this).iter().position(|&each| each == column_object);
        let text = index.map_or(core::ptr::null_mut(), |index| cell_view_text(this, row, index));
        return with_text(text, work);
    }
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
    fields.selection = index_set::new_mutable();
    fields.anchor = NO_INDEX;
    fields.hover = NO_INDEX;
    fields.clicked_row = NO_INDEX;
    fields.clicked_column = NO_INDEX;
    fields.header = true;
    fields.header_hover = NO_INDEX;
    fields.header_pressed = NO_INDEX;
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
    store(&mut fields.sort_descriptors, core::ptr::null_mut());
    store(&mut fields.highlighted_column, core::ptr::null_mut());
    fields.tops.free();
    for cell in fields.cells.as_slice() {
        objc_release(cell.view);
    }
    fields.cells.free();
    for &kept in fields.reuse.as_slice() {
        objc_release(kept);
    }
    fields.reuse.free();
    objc_release(fields.selection);
    fields.selection = core::ptr::null_mut();
    objc_release(fields.tracking);
    fields.tracking = core::ptr::null_mut();
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
    table(this).heights_known = false;
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
    table(this).heights_known = false;
    retire_cells(this, &|_| false);
    let rows = row_count(this);
    if table(this).hover >= rows as isize {
        table(this).hover = NO_INDEX;
    }
    if last_of(table(this).selection) >= rows as isize {
        let wanted = copy_of_selection(this);
        if !wanted.is_null() {
            index_set::remove(wanted, rows, usize::MAX);
        }
        let selected = table(this).selected;
        apply(this, wanted, selected, false);
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
    index_set::count(table(this).selection) as isize
}

extern "C" fn select_row_indexes(this: Id, _: Sel, indexes: Id, extending: u8) {
    let wanted = match extending != 0 {
        true => copy_of_selection(this),
        false => index_set::new_mutable(),
    };
    if wanted.is_null() {
        return;
    }
    let rows = row_count(this);
    let multiple = table(this).multiple;
    for &(start, end) in index_set::each_range(indexes) {
        let end = end.min(rows);
        match multiple {
            true => index_set::insert(wanted, start, end),
            false if start < end => {
                index_set::clear(wanted);
                index_set::insert(wanted, start, start + 1);
            }
            false => {}
        }
        if !multiple {
            break;
        }
    }
    let last = last_of(wanted);
    if last >= 0 {
        table(this).anchor = last;
    }
    apply(this, wanted, last, true);
}

extern "C" fn selected_row_indexes(this: Id, _: Sel) -> Id {
    crate::foundation::autoreleased(index_set::immutable_copy(table(this).selection))
}

extern "C" fn deselect_all(this: Id, _: Sel, _: Id) {
    if !table(this).refuses_empty {
        select(this, NO_INDEX, false);
    }
}

extern "C" fn select_all(this: Id, _: Sel, _: Id) {
    if !table(this).multiple {
        return;
    }
    let wanted = index_set::new_mutable();
    if !wanted.is_null() {
        index_set::insert(wanted, 0, row_count(this));
    }
    let selected = table(this).selected;
    apply(this, wanted, selected, false);
}

extern "C" fn deselect_row(this: Id, _: Sel, row: isize) {
    if row < 0 || !index_set::contains(table(this).selection, row as usize) {
        return;
    }
    if index_set::count(table(this).selection) == 1 && table(this).refuses_empty {
        return;
    }
    let wanted = copy_of_selection(this);
    if !wanted.is_null() {
        index_set::remove(wanted, row as usize, row as usize + 1);
    }
    let last = last_of(wanted);
    apply(this, wanted, last, false);
}

extern "C" fn is_row_selected(this: Id, _: Sel, row: isize) -> u8 {
    u8::from(row >= 0 && index_set::contains(table(this).selection, row as usize))
}

extern "C" fn allows_multiple_selection(this: Id, _: Sel) -> u8 {
    u8::from(table(this).multiple)
}

extern "C" fn set_allows_multiple_selection(this: Id, _: Sel, value: u8) {
    table(this).multiple = value != 0;
    if value == 0 && index_set::count(table(this).selection) > 1 {
        let selected = table(this).selected;
        select(this, selected, false);
    }
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

extern "C" fn row_height(this: Id, _: Sel) -> f64 {
    f64::from(uniform_height(this))
}

extern "C" fn set_row_height(this: Id, _: Sel, height: f64) {
    table(this).row_height = height;
    table(this).heights_known = false;
    fit_frame(this);
}

extern "C" fn note_heights_changed(this: Id, _: Sel, _: Id) {
    table(this).heights_known = false;
    fit_frame(this);
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


fn header_column_at(this: Id, x: i32) -> Option<usize> {
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    if geometry::handle_at(x, &lefts[..count], &widths[..count]).is_some() {
        return None;
    }
    geometry::column_at(x, &lefts[..count], &widths[..count]).map(|shown| which[shown])
}

fn sortable(this: Id, index: usize) -> bool {
    columns(this)
        .get(index)
        .is_some_and(|&each| !column(each).sort_prototype.is_null())
}

fn first_descriptor(this: Id) -> Id {
    let descriptors = table(this).sort_descriptors;
    let count = match descriptors.is_null() {
        true => 0,
        false => send(descriptors, sel!("count")) as usize,
    };
    match count {
        0 => core::ptr::null_mut(),
        _ => send1(descriptors, sel!("objectAtIndex:"), 0),
    }
}

fn sorted_column(this: Id) -> Option<(usize, bool)> {
    let first = first_descriptor(this);
    let key = sort_descriptor::key_of(first);
    if key.is_null() {
        return None;
    }
    let index = columns(this).iter().position(|&each| {
        let wanted = sort_descriptor::key_of(column(each).sort_prototype);
        !wanted.is_null() && crate::objc::send_bool(wanted, sel!("isEqual:"), key as usize)
    })?;
    Some((index, sort_descriptor::is_ascending(first)))
}

fn change_sort(this: Id, descriptors: Id) {
    let old = table(this).sort_descriptors;
    if !old.is_null() {
        objc_retain(old);
    }
    let copy = match descriptors.is_null() {
        true => descriptors,
        false => send(descriptors, sel!("copy")),
    };
    store(&mut table(this).sort_descriptors, copy);
    if !copy.is_null() {
        objc_release(copy);
    }
    changed(this);
    let source = table(this).data_source;
    let asking = sel!("tableView:sortDescriptorsDidChange:");
    if responds(source, asking) {
        let old_array = match old.is_null() {
            true => send(class_pointer(&NSARRAY).cast(), sel!("array")),
            false => old,
        };
        send2(source, asking, this as usize, old_array as usize);
    }
    if !old.is_null() {
        objc_release(old);
    }
}

fn header_click(this: Id, index: usize) {
    let each = columns(this)[index];
    let prototype = column(each).sort_prototype;
    if !prototype.is_null() {
        let descriptors = match sorted_column(this) {
            Some((sorted, _)) if sorted == index => {
                let mutable = send(table(this).sort_descriptors, sel!("mutableCopy"));
                let reversed = send(first_descriptor(this), sel!("reversedSortDescriptor"));
                let replace: extern "C" fn(Id, Sel, usize, Id) = message::msg_send_as();
                replace(mutable, sel!("replaceObjectAtIndex:withObject:"), 0, reversed);
                crate::foundation::autoreleased(mutable)
            }
            _ => send1(
                class_pointer(&NSARRAY).cast(),
                sel!("arrayWithObject:"),
                prototype as usize,
            ),
        };
        change_sort(this, descriptors);
    }
    let delegate = table(this).delegate;
    let clicked = sel!("tableView:didClickTableColumn:");
    if responds(delegate, clicked) {
        send2(delegate, clicked, this as usize, each as usize);
    }
}

fn set_header_hover(this: Id, index: isize) {
    if table(this).header_hover != index {
        table(this).header_hover = index;
        changed(this);
    }
}

fn header_press(this: Id, x: i32) {
    if let Some(index) = header_column_at(this, x) {
        table(this).header_pressed = index as isize;
        changed(this);
        let delegate = table(this).delegate;
        let asking = sel!("tableView:mouseDownInHeaderOfTableColumn:");
        if responds(delegate, asking) {
            let each = columns(this)[index];
            send2(delegate, asking, this as usize, each as usize);
        }
        return;
    }
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
    let Some(row) = heights(this).row_at(clip_y, header, offset) else {
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
    let flags = event::fields(event).flags;
    let chosen = match (table(this).multiple, flags) {
        (true, flags) if flags & COMMAND_FLAG != 0 => toggle(this, row),
        (true, flags) if flags & SHIFT_FLAG != 0 => extend_to(this, row, true),
        _ => select(this, row as isize, true),
    };
    if !chosen {
        return;
    }
    send_action(this, double);
}

fn set_hover(this: Id, row: isize) {
    if table(this).hover != row {
        table(this).hover = row;
        changed(this);
    }
}

extern "C" fn mouse_moved(this: Id, _: Sel, event: Id) {
    if table(this).disabled || table(this).resize.is_some() {
        set_hover(this, NO_INDEX);
        return;
    }
    let (x, y) = local_pixel(this, event);
    let ((_, offset), _) = clip_of(this);
    let header = table(this).header;
    let clip_y = y - offset;
    let in_header = header && clip_y < TABLE_HEADER_HEIGHT;
    let row = match in_header {
        true => None,
        false => heights(this).row_at(clip_y, header, offset),
    };
    set_hover(this, row.map_or(NO_INDEX, |row| row as isize));
    let over = header_column_at(this, x).filter(|&index| in_header && sortable(this, index));
    set_header_hover(this, over.map_or(NO_INDEX, |index| index as isize));
}

extern "C" fn mouse_exited(this: Id, _: Sel, _: Id) {
    set_hover(this, NO_INDEX);
    set_header_hover(this, NO_INDEX);
}

fn track_hover(this: Id) {
    if !table(this).tracking.is_null() || view::fields(this).window.is_null() {
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
    table(this).tracking = area;
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

extern "C" fn mouse_up(this: Id, _: Sel, event: Id) {
    let pressed = core::mem::replace(&mut table(this).header_pressed, NO_INDEX);
    if pressed >= 0 {
        changed(this);
        let (x, y) = local_pixel(this, event);
        let ((_, offset), _) = clip_of(this);
        let in_header = (0..TABLE_HEADER_HEIGHT).contains(&(y - offset));
        if in_header && header_column_at(this, x) == Some(pressed as usize) {
            header_click(this, pressed as usize);
        }
        return;
    }
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
    let ((_, offset), _) = clip_of(this);
    let visible = heights(this).whole_rows(body_height(this), offset);
    let extending = table(this).multiple && event::fields(event).flags & SHIFT_FLAG != 0;
    if let Some(row) = geometry::moved(table(this).selected, step, row_count(this), visible) {
        match extending {
            true => extend_to(this, row, true),
            false => select(this, row as isize, true),
        };
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
            let at = place(Rect::new(left + start, rect.y + text_top(rect.height), space, 16));
            let x = match right {
                true => at.x + (space - cell.chars().count() as i32 * 8).max(0),
                false => at.x,
            };
            clipped_text(canvas, x, at.y, space, cell, text);
        });
    }
}

fn cell_fields(object: Id) -> &'static mut CellViewFields {
    // SAFETY: the callers pass NSTableCellView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<CellViewFields>() }
}

fn view_based(this: Id) -> bool {
    responds(table(this).delegate, sel!("tableView:viewForTableColumn:row:"))
}

fn retire_cells(this: Id, keep: &dyn Fn(CellPlace) -> bool) {
    let mut cells = core::mem::replace(&mut table(this).cells, Buffer::new());
    for &cell in cells.as_slice() {
        if keep(cell) {
            table(this).cells.push(cell);
            continue;
        }
        send(cell.view, sel!("removeFromSuperview"));
        match table(this).reuse.len() < VISIBLE_CELL_VIEWS {
            true => {
                table(this).reuse.push(cell.view);
            }
            false => objc_release(cell.view),
        }
    }
    cells.free();
}

fn arrange_cell(cell: Id) {
    if !super::is(cell, &NSTABLECELLVIEW) {
        return;
    }
    let frame = view::fields(cell).frame;
    let middle = ((frame.height - CELL_PART) / 2.0).max(0.0);
    let (image, text) = (cell_fields(cell).image_view, cell_fields(cell).text_field);
    let unframed = |part: Id| {
        !part.is_null() && {
            let own = view::fields(part).frame;
            own.width <= 0.0 && own.height <= 0.0
        }
    };
    if unframed(image) {
        send_frame(image, sel!("setFrame:"), Frame::new(CELL_INSET, middle, CELL_PART, CELL_PART));
    }
    if unframed(text) {
        let left = match image.is_null() {
            true => CELL_INSET,
            false => TEXT_AFTER_IMAGE,
        };
        let width = (frame.width - left - CELL_INSET).max(0.0);
        send_frame(text, sel!("setFrame:"), Frame::new(left, middle, width, CELL_PART));
    }
}

fn lay_out_cells(this: Id) {
    if !view_based(this) {
        return;
    }
    let ((_, offset), (_, clip_height)) = clip_of(this);
    let header = table(this).header;
    let heights = heights(this);
    let (first, last) = heights.shown(clip_height - geometry::body_top(header), offset);
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    retire_cells(this, &|cell| {
        (first..last).contains(&cell.row) && which[..count].contains(&cell.column)
    });
    let delegate = table(this).delegate;
    let asking = sel!("tableView:viewForTableColumn:row:");
    let ask: extern "C" fn(Id, Sel, Id, Id, isize) -> Id = message::msg_send_as();
    let source = table(this).data_source;
    let value_asked = sel!("tableView:objectValueForTableColumn:row:");
    let table_height = round(view::fields(this).frame.height);
    for row in first..last {
        let top = geometry::body_top(header) + heights.top(row);
        let height = heights.height(row);
        let under_header = header && top < offset + TABLE_HEADER_HEIGHT;
        let chosen = index_set::contains(table(this).selection, row);
        for shown in 0..count {
            let index = which[shown];
            let each = columns(this)[index];
            let existing = table(this)
                .cells
                .as_slice()
                .iter()
                .find(|cell| cell.row == row && cell.column == index)
                .map(|cell| cell.view);
            let cell = match existing {
                Some(cell) => cell,
                None if table(this).cells.len() >= VISIBLE_CELL_VIEWS => continue,
                None => {
                    let made = ask(delegate, asking, this, each, row as isize);
                    if made.is_null() {
                        continue;
                    }
                    objc_retain(made);
                    send1(this, sel!("addSubview:"), made as usize);
                    table(this).cells.push(CellPlace {
                        row,
                        column: index,
                        view: made,
                    });
                    made
                }
            };
            let inset = if shown == 0 { MARK_COLUMN } else { 0 };
            let frame = Frame::new(
                f64::from(lefts[shown] + inset),
                f64::from(table_height - top - height),
                f64::from((widths[shown] - inset).max(0)),
                f64::from(height),
            );
            send_frame(cell, sel!("setFrame:"), frame);
            view::fields(cell).hidden = under_header;
            if responds(source, value_asked) && responds(cell, sel!("setObjectValue:")) {
                let value_of: extern "C" fn(Id, Sel, Id, Id, isize) -> Id = message::msg_send_as();
                let value = value_of(source, value_asked, this, each, row as isize);
                send1(cell, sel!("setObjectValue:"), value as usize);
            }
            if responds(cell, sel!("setBackgroundStyle:")) {
                let style = if chosen { EMPHASIZED } else { 0 };
                send1(cell, sel!("setBackgroundStyle:"), style as usize);
            }
            arrange_cell(cell);
        }
    }
}

fn equal_objects(first: Id, second: Id) -> bool {
    !first.is_null() && crate::objc::send_bool(first, sel!("isEqual:"), second as usize)
}

extern "C" fn make_view_with_identifier(this: Id, _: Sel, identifier: Id, _: Id) -> Id {
    let same = |own: Id| own == identifier || equal_objects(own, identifier);
    let at = table(this)
        .reuse
        .as_slice()
        .iter()
        .position(|&kept| same(view::identifier_of(kept)));
    let Some(at) = at else {
        return core::ptr::null_mut();
    };
    let reuse = table(this).reuse.as_mut_slice();
    let found = reuse[at];
    reuse.copy_within(at + 1.., at);
    table(this).reuse.pop();
    if responds(found, sel!("prepareForReuse")) {
        send(found, sel!("prepareForReuse"));
    }
    crate::foundation::autoreleased(found)
}

extern "C" fn view_at_column_row(this: Id, _: Sel, column_index: isize, row: isize, _: u8) -> Id {
    table(this)
        .cells
        .as_slice()
        .iter()
        .find(|cell| cell.column as isize == column_index && cell.row as isize == row)
        .map_or(core::ptr::null_mut(), |cell| cell.view)
}

extern "C" fn cell_text_field(this: Id, _: Sel) -> Id {
    cell_fields(this).text_field
}

extern "C" fn cell_set_text_field(this: Id, _: Sel, field: Id) {
    cell_fields(this).text_field = field;
}

extern "C" fn cell_image_view(this: Id, _: Sel) -> Id {
    cell_fields(this).image_view
}

extern "C" fn cell_set_image_view(this: Id, _: Sel, image: Id) {
    cell_fields(this).image_view = image;
}

extern "C" fn cell_object_value(this: Id, _: Sel) -> Id {
    cell_fields(this).object_value
}

extern "C" fn cell_set_object_value(this: Id, _: Sel, value: Id) {
    store(&mut cell_fields(this).object_value, value);
}

extern "C" fn cell_background_style(this: Id, _: Sel) -> isize {
    cell_fields(this).background_style
}

extern "C" fn cell_set_background_style(this: Id, _: Sel, style: isize) {
    cell_fields(this).background_style = style;
}

extern "C" fn cell_dealloc(this: Id, _: Sel) {
    store(&mut cell_fields(this).object_value, core::ptr::null_mut());
    view::dispose(this);
}

/// The text a cell of a view-based table shows: its cell view's `textField`'s, or none.
pub fn cell_view_text(this: Id, row: usize, column_index: usize) -> Id {
    let cell = view_at_column_row(
        this,
        sel!("viewAtColumn:row:makeIfNecessary:"),
        column_index as isize,
        row as isize,
        0,
    );
    if cell.is_null() || !super::is(cell, &NSTABLECELLVIEW) {
        return core::ptr::null_mut();
    }
    let field = cell_fields(cell).text_field;
    match field.is_null() {
        true => field,
        false => send(field, sel!("stringValue")),
    }
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    lay_out_cells(this);
    let view_cells = view_based(this);
    let frame = view::fields(this).frame;
    let ((_, offset), (clip_width, clip_height)) = clip_of(this);
    let header = table(this).header;
    let (widths, which, count) = shown_widths(this);
    let lefts: [i32; MOST_COLUMNS] = geometry::column_lefts(&widths[..count], 0);
    let (lefts, widths, which) = (&lefts[..count], &widths[..count], &which[..count]);
    let width = round(frame.width).max(clip_width);
    let heights = heights(this);
    let (first, last) = heights.shown(clip_height - geometry::body_top(header), offset);
    let fields = table(this);
    let (selected, selection, hover, alternating, grid, disabled) = (
        fields.selected,
        fields.selection,
        fields.hover,
        fields.alternating,
        fields.grid,
        fields.disabled,
    );
    let (header_pressed, header_hover) = (fields.header_pressed, fields.header_hover);
    let sorted = sorted_column(this);
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
        let outer = pixel_rect(
            Frame::new(origin.x, origin.y, frame.width, frame.height),
            height,
        );
        let place = |rect: Rect| rect.offset(outer.x, outer.y);
        canvas.fill(place(Rect::new(0, 0, width, round(frame.height))), surface);
        for row in first..last {
            let rect = Rect::new(
                0,
                geometry::body_top(header) + heights.top(row),
                width,
                heights.height(row),
            );
            let chosen = index_set::contains(selection, row);
            let fill = if chosen {
                colour::SELECTION
            } else if row as isize == hover && !disabled {
                colour::MENU_HOVER
            } else if alternating && geometry::is_alternate(row) {
                colour::MENU
            } else {
                surface
            };
            canvas.fill(place(rect), fill);
            if grid & (HORIZONTAL_GRID | DASHED_HORIZONTAL_GRID) != 0 {
                canvas.fill(
                    place(Rect::new(0, rect.bottom() - 1, width, 1)),
                    colour::MENU_BORDER,
                );
            }
            if grid & VERTICAL_GRID != 0 {
                for (&left, &column_width) in lefts.iter().zip(widths) {
                    canvas.fill(
                        place(Rect::new(left + column_width - 1, rect.y, 1, rect.height)),
                        colour::MENU_BORDER,
                    );
                }
            }
            if chosen && !disabled {
                let at = place(Rect::new(TABLE_MARK_X, rect.y + text_top(rect.height), 8, 16));
                let _ = canvas.text(at.x, at.y, MARK, colour::ACCENT, 1);
                if focused && row as isize == selected {
                    focus_ring(
                        canvas,
                        place(Rect::new(0, rect.y, clip_width, rect.height)),
                        0,
                    );
                }
            }
            if !view_cells {
                draw_cells(
                    this,
                    canvas,
                    &place,
                    (row, rect),
                    (lefts, widths, which),
                    text,
                );
            }
        }
        if !header {
            return;
        }
        let top = offset;
        canvas.fill(
            place(Rect::new(0, top, width, TABLE_HEADER_HEIGHT)),
            colour::MENU,
        );
        canvas.fill(
            place(Rect::new(0, top + TABLE_HEADER_HEIGHT - 1, width, 1)),
            colour::MENU_BORDER,
        );
        for (shown, (&left, &column_width)) in lefts.iter().zip(widths).enumerate() {
            let separator = Rect::new(
                left + column_width - 1,
                top + HEADER_SEPARATOR.0,
                1,
                HEADER_SEPARATOR.1 - HEADER_SEPARATOR.0,
            );
            let index = which[shown];
            let each = columns(this)[index];
            let can_sort = !column(each).sort_prototype.is_null();
            let header_fill = match (can_sort, header_pressed, header_hover) {
                (true, pressed, _) if pressed == index as isize => Some(colour::BUTTON_PRESSED),
                (true, _, hover) if hover == index as isize => Some(colour::MENU_HOVER),
                _ => None,
            };
            if let Some(fill) = header_fill {
                let cell = Rect::new(left, top, column_width - 1, TABLE_HEADER_HEIGHT - 1);
                canvas.fill(place(cell), fill);
            }
            canvas.fill(place(separator), colour::MENU_BORDER);
            let right_aligned = column(each).alignment == RIGHT_ALIGNED;
            let order = sorted.filter(|&(column_index, _)| column_index == index);
            let space = match (order, right_aligned) {
                (Some(_), false) => column_width - TABLE_CELL_PADDING - TABLE_ARROW_ROOM,
                _ => column_width - 2 * TABLE_CELL_PADDING,
            };
            if let Some((_, ascending)) = order {
                let chevron_left = match right_aligned {
                    true => left + TABLE_CELL_PADDING,
                    false => left + column_width - TABLE_CELL_PADDING - CHEVRON_WIDTH,
                };
                let at = place(Rect::new(chevron_left, top + CHEVRON_TOP, CHEVRON_WIDTH, 6));
                chevron(canvas, at.x, at.y, !ascending, colour::ACCENT);
            }
            let ink = match order {
                Some(_) => colour::TEXT,
                None => colour::TEXT_DIM,
            };
            with_text(column(each).title, |title| {
                let at = place(Rect::new(
                    left + TABLE_CELL_PADDING,
                    top + HEADER_TEXT_TOP,
                    space,
                    16,
                ));
                let x = match right_aligned {
                    true => at.x + (space - title.chars().count() as i32 * 8).max(0),
                    false => at.x,
                };
                clipped_text(canvas, x, at.y, space, title, ink);
            });
        }
    });
}

extern "C" fn sort_descriptors(this: Id, _: Sel) -> Id {
    match table(this).sort_descriptors.is_null() {
        true => send(class_pointer(&NSARRAY).cast(), sel!("array")),
        false => table(this).sort_descriptors,
    }
}

extern "C" fn set_sort_descriptors(this: Id, _: Sel, descriptors: Id) {
    change_sort(this, descriptors);
}

extern "C" fn highlighted_table_column(this: Id, _: Sel) -> Id {
    match sorted_column(this) {
        Some((index, _)) => columns(this)[index],
        None => table(this).highlighted_column,
    }
}

extern "C" fn set_highlighted_table_column(this: Id, _: Sel, chosen: Id) {
    store(&mut table(this).highlighted_column, chosen);
}

extern "C" fn indicator_image_in_column(_: Id, _: Sel, each: Id) -> Id {
    match each.is_null() {
        true => each,
        false => column(each).indicator_image,
    }
}

extern "C" fn set_indicator_image(_: Id, _: Sel, image: Id, each: Id) {
    if !each.is_null() {
        store(&mut column(each).indicator_image, image);
    }
}

extern "C" fn column_sort_prototype(this: Id, _: Sel) -> Id {
    column(this).sort_prototype
}

extern "C" fn column_set_sort_prototype(this: Id, _: Sel, prototype: Id) {
    store(&mut column(this).sort_prototype, prototype);
    column_changed(this);
}

extern "C" fn view_did_move_to_window(this: Id, _: Sel) {
    track_hover(this);
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
    store(&mut fields.sort_prototype, core::ptr::null_mut());
    store(&mut fields.indicator_image, core::ptr::null_mut());
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

extern "C" fn column_get_title(this: Id, _: Sel) -> Id {
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

/// A table as agents see it (D378): its selected row (-1 for none), its rows and whether it
/// is disabled.
pub fn summary(this: Id) -> (isize, usize, bool) {
    (table(this).selected, row_count(this), table(this).disabled)
}

/// The rows shown now: first and one past the last.
pub fn shown_rows(this: Id) -> (usize, usize) {
    let ((_, offset), _) = clip_of(this);
    heights(this).shown(body_height(this), offset)
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
    let heights = heights(this);
    let top = geometry::body_top(table(this).header) + heights.top(row);
    Rect::new(0, top, width, heights.height(row))
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
    let rect = pixel_rect(
        Frame::new(origin.x, origin.y, frame.width, frame.height),
        height,
    );
    (rect.x, rect.y)
}

/// Runs `work` with row `row`'s first cell and every cell joined by tabs, into `joined`.
pub fn with_row_text<R>(
    this: Id,
    row: usize,
    joined: &mut [u8],
    work: impl FnOnce(&str, &str) -> R,
) -> R {
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
        "mouseMoved:" => mouse_moved,
        "mouseExited:" => mouse_exited,
        "selectAll:" => select_all,
        "allowsMultipleSelection" => allows_multiple_selection,
        "setAllowsMultipleSelection:" => set_allows_multiple_selection,
        "scrollRowToVisible:" => scroll_row_to_visible,
        "headerView" => header_view,
        "setHeaderView:" => set_header_view,
        "rowHeight" => row_height,
        "makeViewWithIdentifier:owner:" => make_view_with_identifier,
        "viewAtColumn:row:makeIfNecessary:" => view_at_column_row,
        "setRowHeight:" => set_row_height,
        "noteHeightOfRowsWithIndexesChanged:" => note_heights_changed,
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
        "setAllowsColumnReordering:" => ignore_word,
        "setAllowsColumnResizing:" => ignore_word,
        "setColumnAutoresizingStyle:" => ignore_word,
        "setSelectionHighlightStyle:" => ignore_word,
        "setGridColor:" => ignore_word,
        "sortDescriptors" => sort_descriptors,
        "setSortDescriptors:" => set_sort_descriptors,
        "highlightedTableColumn" => highlighted_table_column,
        "setHighlightedTableColumn:" => set_highlighted_table_column,
        "indicatorImageInTableColumn:" => indicator_image_in_column,
        "setIndicatorImage:inTableColumn:" => set_indicator_image,
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
        "title" => column_get_title,
        "setTitle:" => column_set_title,
        "stringValue" => column_get_title,
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
        "sortDescriptorPrototype" => column_sort_prototype,
        "setSortDescriptorPrototype:" => column_set_sort_prototype,
    ];
    let cell_methods = methods![
        "dealloc" => cell_dealloc,
        "textField" => cell_text_field,
        "setTextField:" => cell_set_text_field,
        "imageView" => cell_image_view,
        "setImageView:" => cell_set_image_view,
        "objectValue" => cell_object_value,
        "setObjectValue:" => cell_set_object_value,
        "backgroundStyle" => cell_background_style,
        "setBackgroundStyle:" => cell_set_background_style,
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
        b"NSTableCellView\0",
        (&NSTABLECELLVIEW, &NSTABLECELLVIEW_META),
        &NSVIEW,
        core::mem::size_of::<CellViewFields>(),
        &cell_methods,
        &[],
    )
}

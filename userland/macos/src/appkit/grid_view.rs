//! NSGridView, NSGridRow, NSGridColumn and NSGridCell (D385): views laid out in rows and
//! columns, each column as wide and each row as tall as its largest view unless the app set
//! its size, with the grid's spacing between them and each view placed in its cell by the x
//! and y placements of the cell, else its column or row, else the grid. The grid lays its
//! views out whenever they change and before it draws; a grid with no size takes the size its
//! cells need. Rows and columns take padding, can be hidden (no room, no spacing, their views
//! hidden), inserted, removed and moved, and a rectangle of cells can be merged into its first
//! cell, whose view then spans it (D732).

use super::{
    NSGRIDCELL, NSGRIDCELL_META, NSGRIDCOLUMN, NSGRIDCOLUMN_META, NSGRIDROW, NSGRIDROW_META,
    NSGRIDVIEW, NSGRIDVIEW_META, NSVIEW, view,
};
use crate::foundation::{autoreleased, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, message, methods, sel, send, send1};
use eggos_macos_gui::geometry::{Frame, Size, round};
use eggos_macos_gui::grid_view::{self as layout, DEFAULT_SPACING, Track, placement};
use eggos_toolkit::geometry::Rect;

const MOST_COLUMNS: usize = 64;
const MOST_ROWS: usize = 256;
const AUTOMATIC_BELOW: f64 = 1e-30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Merge {
    column: usize,
    columns: usize,
    row: usize,
    rows: usize,
}

impl Merge {
    fn holds(&self, column: usize, row: usize) -> bool {
        (self.column..self.column + self.columns).contains(&column)
            && (self.row..self.row + self.rows).contains(&row)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GridRange {
    location: usize,
    length: usize,
}

#[repr(C)]
#[derive(Debug)]
struct GridFields {
    view: view::ViewFields,
    cells: Buffer<Id>,
    cell_x: Buffer<u8>,
    cell_y: Buffer<u8>,
    rows: usize,
    columns: usize,
    widths: [f64; MOST_COLUMNS],
    heights: [f64; MOST_ROWS],
    column_x: [u8; MOST_COLUMNS],
    row_y: [u8; MOST_ROWS],
    row_spacing: f64,
    column_spacing: f64,
    spacing_set: bool,
    x_placement: usize,
    y_placement: usize,
    row_padding: [(f64, f64); MOST_ROWS],
    column_padding: [(f64, f64); MOST_COLUMNS],
    row_hidden: [bool; MOST_ROWS],
    column_hidden: [bool; MOST_COLUMNS],
    merges: Buffer<Merge>,
}

#[repr(C)]
#[derive(Debug)]
struct PartFields {
    isa: usize,
    grid: Id,
    index: usize,
    second: usize,
}

fn grid(object: Id) -> &'static mut GridFields {
    // SAFETY: the callers pass NSGridView instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<GridFields>() }
}

fn part(object: Id) -> &'static mut PartFields {
    // SAFETY: the callers pass NSGridRow, NSGridColumn or NSGridCell instances, whose memory
    // starts with these fields.
    unsafe { &mut *object.cast::<PartFields>() }
}

fn spacing(this: Id) -> (i32, i32) {
    let fields = grid(this);
    match fields.spacing_set {
        true => (round(fields.column_spacing), round(fields.row_spacing)),
        false => (DEFAULT_SPACING, DEFAULT_SPACING),
    }
}

fn cell_view(this: Id, column: usize, row: usize) -> Id {
    let fields = grid(this);
    fields
        .cells
        .as_slice()
        .get(row * fields.columns + column)
        .copied()
        .unwrap_or(core::ptr::null_mut())
}

fn view_size(view_object: Id) -> (i32, i32) {
    match view_object.is_null() {
        true => (0, 0),
        false => {
            let frame = view::fields(view_object).frame;
            (round(frame.width), round(frame.height))
        }
    }
}

fn track(value: f64) -> Track {
    (value >= AUTOMATIC_BELOW).then(|| round(value))
}

fn merge_at(this: Id, column: usize, row: usize) -> Option<Merge> {
    grid(this)
        .merges
        .as_slice()
        .iter()
        .copied()
        .find(|merge| merge.holds(column, row))
}

fn pixels(pair: (f64, f64)) -> (i32, i32) {
    (round(pair.0), round(pair.1))
}

fn column_padding_of(this: Id, first: usize, last: usize) -> (i32, i32) {
    let padding = &grid(this).column_padding;
    pixels((padding[first].0, padding[last].1))
}

fn row_padding_of(this: Id, first: usize, last: usize) -> (i32, i32) {
    let padding = &grid(this).row_padding;
    pixels((padding[first].0, padding[last].1))
}

fn widen_span(
    sizes: &mut [i32],
    hidden: &[bool],
    (first, count): (usize, usize),
    needed: i32,
    gap: i32,
) {
    let shown: usize = (first..first + count).filter(|&index| !hidden[index]).count();
    let have: i32 = (first..first + count)
        .filter(|&index| !hidden[index])
        .map(|index| sizes[index])
        .sum::<i32>()
        + gap * (shown as i32 - 1).max(0);
    let last = (first..first + count).rev().find(|&index| !hidden[index]);
    if let Some(last) = last
        && needed > have
    {
        sizes[last] += needed - have;
    }
}

fn measure(this: Id) -> ([i32; MOST_COLUMNS], [i32; MOST_ROWS], (i32, i32)) {
    let fields = grid(this);
    let (columns, rows) = (fields.columns, fields.rows);
    let mut column_tracks: Buffer<Track> = {
        let mut tracks = Buffer::new();
        for &width in &fields.widths[..columns] {
            tracks.push(track(width));
        }
        tracks
    };
    let mut row_tracks: Buffer<Track> = {
        let mut tracks = Buffer::new();
        for &height in &fields.heights[..rows] {
            tracks.push(track(height));
        }
        tracks
    };
    let own_size = |column: usize, row: usize| match merge_at(this, column, row) {
        Some(_) => (0, 0),
        None => view_size(cell_view(this, column, row)),
    };
    let mut widths: [i32; MOST_COLUMNS] =
        layout::sizes(column_tracks.as_slice(), rows, |column, row| own_size(column, row).0);
    let mut heights: [i32; MOST_ROWS] =
        layout::sizes(row_tracks.as_slice(), columns, |row, column| own_size(column, row).1);
    column_tracks.free();
    row_tracks.free();
    let fields = grid(this);
    let columns_info = fields.column_padding.iter().zip(&fields.column_hidden);
    for (width, (&padding, &hidden)) in widths[..columns].iter_mut().zip(columns_info) {
        let (leading, trailing) = pixels(padding);
        *width = match hidden {
            true => 0,
            false => *width + leading + trailing,
        };
    }
    let rows_info = fields.row_padding.iter().zip(&fields.row_hidden);
    for (height, (&padding, &hidden)) in heights[..rows].iter_mut().zip(rows_info) {
        let (top, bottom) = pixels(padding);
        *height = match hidden {
            true => 0,
            false => *height + top + bottom,
        };
    }
    let (column_gap, row_gap) = spacing(this);
    let merges: &[Merge] = grid(this).merges.as_slice();
    for merge in merges {
        let (width, height) = view_size(cell_view(this, merge.column, merge.row));
        let fields = grid(this);
        let last_column = merge.column + merge.columns - 1;
        let last_row = merge.row + merge.rows - 1;
        let across = column_padding_of(this, merge.column, last_column);
        let down = row_padding_of(this, merge.row, last_row);
        widen_span(
            &mut widths[..columns],
            &fields.column_hidden[..columns],
            (merge.column, merge.columns),
            width + across.0 + across.1,
            column_gap,
        );
        widen_span(
            &mut heights[..rows],
            &fields.row_hidden[..rows],
            (merge.row, merge.rows),
            height + down.0 + down.1,
            row_gap,
        );
    }
    let fields = grid(this);
    let total = (
        layout::shown_total(&widths[..columns], &fields.column_hidden[..columns], column_gap),
        layout::shown_total(&heights[..rows], &fields.row_hidden[..rows], row_gap),
    );
    (widths, heights, total)
}

fn placement_of(cell: u8, line: u8, whole: usize) -> usize {
    [usize::from(cell), usize::from(line), whole]
        .into_iter()
        .find(|&value| value != placement::INHERITED)
        .unwrap_or(placement::LEADING)
}

fn lay_out(this: Id) {
    let (widths, heights, total) = measure(this);
    let frame = &mut view::fields(this).frame;
    if frame.width <= 0.0 && frame.height <= 0.0 {
        frame.width = f64::from(total.0);
        frame.height = f64::from(total.1);
    }
    let grid_height = round(view::fields(this).frame.height);
    let (column_gap, row_gap) = spacing(this);
    let fields = grid(this);
    let (columns, rows) = (fields.columns, fields.rows);
    let lefts: [i32; MOST_COLUMNS] =
        layout::shown_starts(&widths[..columns], &fields.column_hidden[..columns], column_gap);
    let tops: [i32; MOST_ROWS] =
        layout::shown_starts(&heights[..rows], &fields.row_hidden[..rows], row_gap);
    for row in 0..rows {
        for column in 0..columns {
            let shown = cell_view(this, column, row);
            if shown.is_null() || !super::is(shown, &NSVIEW) {
                continue;
            }
            let index = row * columns + column;
            let fields = grid(this);
            let merge = merge_at(this, column, row);
            let head = merge.is_none_or(|merge| merge.column == column && merge.row == row);
            let hidden = fields.row_hidden[row] || fields.column_hidden[column] || !head;
            send1(shown, sel!("setHidden:"), usize::from(hidden));
            if hidden {
                continue;
            }
            let area = merge.unwrap_or(Merge {
                column,
                columns: 1,
                row,
                rows: 1,
            });
            let x = placement_of(
                fields.cell_x.as_slice().get(index).copied().unwrap_or(0),
                fields.column_x[column],
                fields.x_placement,
            );
            let y = placement_of(
                fields.cell_y.as_slice().get(index).copied().unwrap_or(0),
                fields.row_y[row],
                fields.y_placement,
            );
            let (left, width) = layout::span(&lefts, &widths, area.column, area.columns);
            let (top, height) = layout::span(&tops, &heights, area.row, area.rows);
            let last_column = area.column + area.columns - 1;
            let last_row = area.row + area.rows - 1;
            let fields = grid(this);
            let cell = layout::padded(
                Rect::new(left, top, width, height),
                column_padding_of(this, area.column, last_column),
                row_padding_of(this, area.row, last_row),
            );
            let placed = layout::place(cell, view_size(shown), (x, y));
            view::fields(shown).frame = Frame::new(
                f64::from(placed.x),
                f64::from(grid_height - placed.y - placed.height),
                f64::from(placed.width),
                f64::from(placed.height),
            );
        }
    }
    send1(this, sel!("setNeedsDisplay:"), 1);
}

fn set_up(this: Id) {
    let fields = grid(this);
    fields.x_placement = placement::LEADING;
    fields.y_placement = placement::LEADING;
}

fn push_cell(this: Id, shown: Id) {
    let fields = grid(this);
    let real = !shown.is_null() && super::is(shown, &NSVIEW);
    fields.cells.push(match real {
        true => shown,
        false => core::ptr::null_mut(),
    });
    fields.cell_x.push(0);
    fields.cell_y.push(0);
    if real {
        send1(this, sel!("addSubview:"), shown as usize);
    }
}

fn array_items(array: Id) -> (usize, impl Fn(usize) -> Id) {
    let count = match array.is_null() {
        true => 0,
        false => send(array, sel!("count")) as usize,
    };
    (count, move |index: usize| {
        send1(array, sel!("objectAtIndex:"), index)
    })
}

fn add_row(this: Id, views: Id) -> bool {
    let (count, item) = array_items(views);
    if grid(this).rows == MOST_ROWS {
        return false;
    }
    if grid(this).rows == 0 && grid(this).columns == 0 {
        grid(this).columns = count.min(MOST_COLUMNS);
    }
    let columns = grid(this).columns;
    for column in 0..columns {
        let shown = match column < count {
            true => item(column),
            false => core::ptr::null_mut(),
        };
        push_cell(this, shown);
    }
    grid(this).rows += 1;
    true
}

extern "C" fn init_with_frame(this: Id, _: Sel, frame: Frame) -> Id {
    if this.is_null() {
        return this;
    }
    view::fields(this).frame = frame;
    set_up(this);
    this
}

extern "C" fn grid_with_views(class: Id, _: Sel, rows: Id) -> Id {
    let made = send(class, sel!("alloc"));
    let init: extern "C" fn(Id, Sel, Frame) -> Id = message::msg_send_as();
    let made = init(made, sel!("initWithFrame:"), Frame::default());
    if made.is_null() {
        return made;
    }
    let (count, item) = array_items(rows);
    for index in 0..count {
        add_row(made, item(index));
    }
    lay_out(made);
    autoreleased(made)
}

extern "C" fn grid_with_counts(class: Id, _: Sel, columns: isize, rows: isize) -> Id {
    let made = send(class, sel!("alloc"));
    let init: extern "C" fn(Id, Sel, Frame) -> Id = message::msg_send_as();
    let made = init(made, sel!("initWithFrame:"), Frame::default());
    if made.is_null() {
        return made;
    }
    grid(made).columns = (columns.max(0) as usize).min(MOST_COLUMNS);
    for _ in 0..(rows.max(0) as usize).min(MOST_ROWS) {
        add_row(made, core::ptr::null_mut());
    }
    autoreleased(made)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = grid(this);
    fields.cells.free();
    fields.cell_x.free();
    fields.cell_y.free();
    fields.merges.free();
    view::dispose(this);
}

fn rebuilt<T: Copy>(
    old: &Buffer<T>,
    old_columns: usize,
    (rows, columns): (usize, usize),
    from: Origin<'_>,
    fill: T,
) -> Buffer<T> {
    let mut made = Buffer::new();
    made.reserve(rows * columns);
    for row in 0..rows {
        for column in 0..columns {
            let kept = from(row, column)
                .and_then(|(old_row, old_column)| {
                    old.as_slice().get(old_row * old_columns + old_column)
                })
                .copied();
            made.push(kept.unwrap_or(fill));
        }
    }
    made
}

type Origin<'a> = &'a dyn Fn(usize, usize) -> Option<(usize, usize)>;

fn reshape(this: Id, (rows, columns): (usize, usize), from: Origin<'_>) {
    let fields = grid(this);
    let old_columns = fields.columns;
    let shape = (rows, columns);
    let mut cells = rebuilt(&fields.cells, old_columns, shape, from, core::ptr::null_mut());
    let mut cell_x = rebuilt(&fields.cell_x, old_columns, (rows, columns), from, 0);
    let mut cell_y = rebuilt(&fields.cell_y, old_columns, (rows, columns), from, 0);
    core::mem::swap(&mut fields.cells, &mut cells);
    core::mem::swap(&mut fields.cell_x, &mut cell_x);
    core::mem::swap(&mut fields.cell_y, &mut cell_y);
    cells.free();
    cell_x.free();
    cell_y.free();
    fields.rows = rows;
    fields.columns = columns;
}

fn insert_track<T: Copy>(tracks: &mut [T], count: usize, at: usize, fill: T) {
    if count < tracks.len() {
        tracks.copy_within(at..count, at + 1);
        tracks[at] = fill;
    }
}

fn remove_track<T: Copy>(tracks: &mut [T], count: usize, at: usize, fill: T) {
    tracks.copy_within(at + 1..count, at);
    tracks[count - 1] = fill;
}

fn move_track<T: Copy>(tracks: &mut [T], from: usize, to: usize) {
    match from < to {
        true => tracks[from..=to].rotate_left(1),
        false => tracks[to..=from].rotate_right(1),
    }
}

fn forget_merges(this: Id) {
    let merges = &mut grid(this).merges;
    while merges.pop().is_some() {}
}

fn insert_row(this: Id, at: usize, views: Id) -> bool {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    if rows == MOST_ROWS || at > rows {
        return false;
    }
    if rows == 0 && columns == 0 {
        return add_row(this, views);
    }
    reshape(this, (rows + 1, columns), &|row, column| match row {
        row if row < at => Some((row, column)),
        row if row == at => None,
        row => Some((row - 1, column)),
    });
    let fields = grid(this);
    insert_track(&mut fields.heights, rows, at, 0.0);
    insert_track(&mut fields.row_y, rows, at, 0);
    insert_track(&mut fields.row_padding, rows, at, (0.0, 0.0));
    insert_track(&mut fields.row_hidden, rows, at, false);
    forget_merges(this);
    let (count, item) = array_items(views);
    for column in 0..columns.min(count) {
        let shown = item(column);
        if !shown.is_null() && super::is(shown, &NSVIEW) {
            grid(this).cells.as_mut_slice()[at * columns + column] = shown;
            send1(this, sel!("addSubview:"), shown as usize);
        }
    }
    true
}

fn insert_column(this: Id, at: usize, views: Id) -> bool {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    if columns == MOST_COLUMNS || at > columns {
        return false;
    }
    reshape(this, (rows, columns + 1), &|row, column| match column {
        column if column < at => Some((row, column)),
        column if column == at => None,
        column => Some((row, column - 1)),
    });
    let fields = grid(this);
    insert_track(&mut fields.widths, columns, at, 0.0);
    insert_track(&mut fields.column_x, columns, at, 0);
    insert_track(&mut fields.column_padding, columns, at, (0.0, 0.0));
    insert_track(&mut fields.column_hidden, columns, at, false);
    forget_merges(this);
    let (count, item) = array_items(views);
    for row in 0..rows.min(count) {
        let shown = item(row);
        if !shown.is_null() && super::is(shown, &NSVIEW) {
            grid(this).cells.as_mut_slice()[row * (columns + 1) + at] = shown;
            send1(this, sel!("addSubview:"), shown as usize);
        }
    }
    true
}

fn remove_views(this: Id, keep: &dyn Fn(usize, usize) -> bool) {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    for row in 0..rows {
        for column in 0..columns {
            let shown = cell_view(this, column, row);
            if !keep(row, column) && !shown.is_null() {
                send(shown, sel!("removeFromSuperview"));
            }
        }
    }
}

extern "C" fn insert_row_at(this: Id, _: Sel, at: isize, views: Id) -> Id {
    let at = at.max(0) as usize;
    if !insert_row(this, at, views) {
        return core::ptr::null_mut();
    }
    lay_out(this);
    new_part(&NSGRIDROW, this, at, 0)
}

extern "C" fn add_column_with_views(this: Id, _: Sel, views: Id) -> Id {
    let at = grid(this).columns;
    insert_column_at(this, sel!("insertColumnAtIndex:withViews:"), at as isize, views)
}

extern "C" fn insert_column_at(this: Id, _: Sel, at: isize, views: Id) -> Id {
    let at = at.max(0) as usize;
    if !insert_column(this, at, views) {
        return core::ptr::null_mut();
    }
    lay_out(this);
    new_part(&NSGRIDCOLUMN, this, at, 0)
}

extern "C" fn remove_row_at(this: Id, _: Sel, at: isize) {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    let Some(at) = usize::try_from(at).ok().filter(|&at| at < rows) else {
        return;
    };
    remove_views(this, &|row, _| row != at);
    reshape(this, (rows - 1, columns), &|row, column| match row < at {
        true => Some((row, column)),
        false => Some((row + 1, column)),
    });
    let fields = grid(this);
    remove_track(&mut fields.heights, rows, at, 0.0);
    remove_track(&mut fields.row_y, rows, at, 0);
    remove_track(&mut fields.row_padding, rows, at, (0.0, 0.0));
    remove_track(&mut fields.row_hidden, rows, at, false);
    forget_merges(this);
    lay_out(this);
}

extern "C" fn remove_column_at(this: Id, _: Sel, at: isize) {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    let Some(at) = usize::try_from(at).ok().filter(|&at| at < columns) else {
        return;
    };
    remove_views(this, &|_, column| column != at);
    reshape(this, (rows, columns - 1), &|row, column| match column < at {
        true => Some((row, column)),
        false => Some((row, column + 1)),
    });
    let fields = grid(this);
    remove_track(&mut fields.widths, columns, at, 0.0);
    remove_track(&mut fields.column_x, columns, at, 0);
    remove_track(&mut fields.column_padding, columns, at, (0.0, 0.0));
    remove_track(&mut fields.column_hidden, columns, at, false);
    forget_merges(this);
    lay_out(this);
}

fn moved_index(index: usize, from: usize, to: usize) -> usize {
    match index {
        index if index == to => from,
        index if from < to && (from..to).contains(&index) => index + 1,
        index if to < from && (to + 1..=from).contains(&index) => index - 1,
        index => index,
    }
}

extern "C" fn move_row(this: Id, _: Sel, from: isize, to: isize) {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
        return;
    };
    if from >= rows || to >= rows || from == to {
        return;
    }
    reshape(this, (rows, columns), &|row, column| Some((moved_index(row, from, to), column)));
    let fields = grid(this);
    move_track(&mut fields.heights, from, to);
    move_track(&mut fields.row_y, from, to);
    move_track(&mut fields.row_padding, from, to);
    move_track(&mut fields.row_hidden, from, to);
    forget_merges(this);
    lay_out(this);
}

extern "C" fn move_column(this: Id, _: Sel, from: isize, to: isize) {
    let fields = grid(this);
    let (rows, columns) = (fields.rows, fields.columns);
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
        return;
    };
    if from >= columns || to >= columns || from == to {
        return;
    }
    reshape(this, (rows, columns), &|row, column| Some((row, moved_index(column, from, to))));
    let fields = grid(this);
    move_track(&mut fields.widths, from, to);
    move_track(&mut fields.column_x, from, to);
    move_track(&mut fields.column_padding, from, to);
    move_track(&mut fields.column_hidden, from, to);
    forget_merges(this);
    lay_out(this);
}

fn merge(this: Id, across: GridRange, down: GridRange) {
    let fields = grid(this);
    let fits = across.length > 0
        && down.length > 0
        && across.location + across.length <= fields.columns
        && down.location + down.length <= fields.rows;
    if !fits {
        return;
    }
    let wanted = Merge {
        column: across.location,
        columns: across.length,
        row: down.location,
        rows: down.length,
    };
    let overlaps = |other: &Merge| {
        other.column < wanted.column + wanted.columns
            && wanted.column < other.column + other.columns
            && other.row < wanted.row + wanted.rows
            && wanted.row < other.row + other.rows
    };
    if fields.merges.as_slice().iter().any(overlaps) {
        return;
    }
    fields.merges.push(wanted);
    lay_out(this);
}

extern "C" fn merge_cells(this: Id, _: Sel, across: GridRange, down: GridRange) {
    merge(this, across, down);
}

extern "C" fn add_row_with_views(this: Id, _: Sel, views: Id) -> Id {
    if !add_row(this, views) {
        return core::ptr::null_mut();
    }
    lay_out(this);
    new_part(&NSGRIDROW, this, grid(this).rows - 1, 0)
}

extern "C" fn number_of_rows(this: Id, _: Sel) -> isize {
    grid(this).rows as isize
}

extern "C" fn number_of_columns(this: Id, _: Sel) -> isize {
    grid(this).columns as isize
}

fn new_part(
    class: &'static crate::objc::abi::ClassObject,
    owner: Id,
    index: usize,
    second: usize,
) -> Id {
    let made = new_instance(class_pointer(class));
    if made.is_null() {
        return made;
    }
    let fields = part(made);
    fields.grid = objc_retain(owner);
    fields.index = index;
    fields.second = second;
    autoreleased(made)
}

extern "C" fn row_at_index(this: Id, _: Sel, index: isize) -> Id {
    match (0..grid(this).rows as isize).contains(&index) {
        true => new_part(&NSGRIDROW, this, index as usize, 0),
        false => core::ptr::null_mut(),
    }
}

extern "C" fn column_at_index(this: Id, _: Sel, index: isize) -> Id {
    match (0..grid(this).columns as isize).contains(&index) {
        true => new_part(&NSGRIDCOLUMN, this, index as usize, 0),
        false => core::ptr::null_mut(),
    }
}

extern "C" fn cell_at(this: Id, _: Sel, column: isize, row: isize) -> Id {
    let fields = grid(this);
    let inside =
        (0..fields.columns as isize).contains(&column) && (0..fields.rows as isize).contains(&row);
    match inside {
        true => new_part(&NSGRIDCELL, this, column as usize, row as usize),
        false => core::ptr::null_mut(),
    }
}

extern "C" fn cell_for_view(this: Id, _: Sel, shown: Id) -> Id {
    let fields = grid(this);
    let Some(index) = fields
        .cells
        .as_slice()
        .iter()
        .position(|&each| each == shown && !shown.is_null())
    else {
        return core::ptr::null_mut();
    };
    let columns = fields.columns.max(1);
    new_part(&NSGRIDCELL, this, index % columns, index / columns)
}

extern "C" fn row_spacing(this: Id, _: Sel) -> f64 {
    f64::from(spacing(this).1)
}

extern "C" fn set_row_spacing(this: Id, _: Sel, value: f64) {
    let (column, _) = spacing(this);
    let fields = grid(this);
    fields.column_spacing = f64::from(column);
    fields.row_spacing = value.max(0.0);
    fields.spacing_set = true;
    lay_out(this);
}

extern "C" fn column_spacing(this: Id, _: Sel) -> f64 {
    f64::from(spacing(this).0)
}

extern "C" fn set_column_spacing(this: Id, _: Sel, value: f64) {
    let (_, row) = spacing(this);
    let fields = grid(this);
    fields.row_spacing = f64::from(row);
    fields.column_spacing = value.max(0.0);
    fields.spacing_set = true;
    lay_out(this);
}

extern "C" fn x_placement(this: Id, _: Sel) -> usize {
    grid(this).x_placement
}

extern "C" fn set_x_placement(this: Id, _: Sel, value: usize) {
    grid(this).x_placement = value;
    lay_out(this);
}

extern "C" fn y_placement(this: Id, _: Sel) -> usize {
    grid(this).y_placement
}

extern "C" fn set_y_placement(this: Id, _: Sel, value: usize) {
    grid(this).y_placement = value;
    lay_out(this);
}

extern "C" fn layout_method(this: Id, _: Sel) {
    lay_out(this);
}

extern "C" fn fitting_size(this: Id, _: Sel) -> Size {
    let (_, _, total) = measure(this);
    Size {
        width: f64::from(total.0),
        height: f64::from(total.1),
    }
}

extern "C" fn draw_rect(this: Id, _: Sel, _: Frame) {
    let (_, _, total) = measure(this);
    let frame = view::fields(this).frame;
    if frame.width <= 0.0 && frame.height <= 0.0 && (total.0 > 0 || total.1 > 0) {
        lay_out(this);
    }
}

extern "C" fn part_dealloc(this: Id, _: Sel) {
    let owner = part(this).grid;
    if !owner.is_null() {
        objc_release(owner);
    }
    dispose(this);
}

extern "C" fn row_height(this: Id, _: Sel) -> f64 {
    let fields = part(this);
    grid(fields.grid).heights[fields.index]
}

extern "C" fn set_row_height(this: Id, _: Sel, value: f64) {
    let fields = part(this);
    grid(fields.grid).heights[fields.index] = value;
    lay_out(fields.grid);
}

extern "C" fn row_y_placement(this: Id, _: Sel) -> usize {
    let fields = part(this);
    usize::from(grid(fields.grid).row_y[fields.index])
}

extern "C" fn set_row_y_placement(this: Id, _: Sel, value: usize) {
    let fields = part(this);
    grid(fields.grid).row_y[fields.index] = value.min(placement::FILL) as u8;
    lay_out(fields.grid);
}

macro_rules! padding_accessors {
    ($($getter:ident, $setter:ident, $tracks:ident, $side:tt;)*) => {
        $(
            extern "C" fn $getter(this: Id, _: Sel) -> f64 {
                let fields = part(this);
                grid(fields.grid).$tracks[fields.index].$side
            }

            extern "C" fn $setter(this: Id, _: Sel, value: f64) {
                let fields = part(this);
                grid(fields.grid).$tracks[fields.index].$side = value.max(0.0);
                lay_out(fields.grid);
            }
        )*
    };
}

padding_accessors! {
    row_top_padding, set_row_top_padding, row_padding, 0;
    row_bottom_padding, set_row_bottom_padding, row_padding, 1;
    column_leading_padding, set_column_leading_padding, column_padding, 0;
    column_trailing_padding, set_column_trailing_padding, column_padding, 1;
}

extern "C" fn row_is_hidden(this: Id, _: Sel) -> u8 {
    let fields = part(this);
    u8::from(grid(fields.grid).row_hidden[fields.index])
}

extern "C" fn set_row_hidden(this: Id, _: Sel, hidden: u8) {
    let fields = part(this);
    grid(fields.grid).row_hidden[fields.index] = hidden != 0;
    lay_out(fields.grid);
}

extern "C" fn column_is_hidden(this: Id, _: Sel) -> u8 {
    let fields = part(this);
    u8::from(grid(fields.grid).column_hidden[fields.index])
}

extern "C" fn set_column_hidden(this: Id, _: Sel, hidden: u8) {
    let fields = part(this);
    grid(fields.grid).column_hidden[fields.index] = hidden != 0;
    lay_out(fields.grid);
}

extern "C" fn row_merge_cells(this: Id, _: Sel, across: GridRange) {
    let fields = part(this);
    let down = GridRange {
        location: fields.index,
        length: 1,
    };
    merge(fields.grid, across, down);
}

extern "C" fn column_merge_cells(this: Id, _: Sel, down: GridRange) {
    let fields = part(this);
    let across = GridRange {
        location: fields.index,
        length: 1,
    };
    merge(fields.grid, across, down);
}

extern "C" fn row_number_of_cells(this: Id, _: Sel) -> isize {
    grid(part(this).grid).columns as isize
}

extern "C" fn row_cell_at(this: Id, _: Sel, index: isize) -> Id {
    let fields = part(this);
    cell_at(
        fields.grid,
        sel!("cellAtColumnIndex:rowIndex:"),
        index,
        fields.index as isize,
    )
}

extern "C" fn column_width(this: Id, _: Sel) -> f64 {
    let fields = part(this);
    grid(fields.grid).widths[fields.index]
}

extern "C" fn set_column_width(this: Id, _: Sel, value: f64) {
    let fields = part(this);
    grid(fields.grid).widths[fields.index] = value;
    lay_out(fields.grid);
}

extern "C" fn column_x_placement(this: Id, _: Sel) -> usize {
    let fields = part(this);
    usize::from(grid(fields.grid).column_x[fields.index])
}

extern "C" fn set_column_x_placement(this: Id, _: Sel, value: usize) {
    let fields = part(this);
    grid(fields.grid).column_x[fields.index] = value.min(placement::FILL) as u8;
    lay_out(fields.grid);
}

extern "C" fn column_number_of_cells(this: Id, _: Sel) -> isize {
    grid(part(this).grid).rows as isize
}

extern "C" fn column_cell_at(this: Id, _: Sel, index: isize) -> Id {
    let fields = part(this);
    cell_at(
        fields.grid,
        sel!("cellAtColumnIndex:rowIndex:"),
        fields.index as isize,
        index,
    )
}

fn cell_index(this: Id) -> usize {
    let fields = part(this);
    fields.second * grid(fields.grid).columns + fields.index
}

extern "C" fn cell_content_view(this: Id, _: Sel) -> Id {
    let fields = part(this);
    cell_view(fields.grid, fields.index, fields.second)
}

extern "C" fn cell_set_content_view(this: Id, _: Sel, shown: Id) {
    let owner = part(this).grid;
    let index = cell_index(this);
    let old = grid(owner)
        .cells
        .as_slice()
        .get(index)
        .copied()
        .unwrap_or(core::ptr::null_mut());
    if old == shown {
        return;
    }
    if !old.is_null() {
        view::detach(old);
    }
    let real = !shown.is_null() && super::is(shown, &NSVIEW);
    if let Some(slot) = grid(owner).cells.as_mut_slice().get_mut(index) {
        *slot = match real {
            true => shown,
            false => core::ptr::null_mut(),
        };
    }
    if real {
        send1(owner, sel!("addSubview:"), shown as usize);
    }
    lay_out(owner);
}

extern "C" fn cell_x_placement(this: Id, _: Sel) -> usize {
    let index = cell_index(this);
    usize::from(
        grid(part(this).grid)
            .cell_x
            .as_slice()
            .get(index)
            .copied()
            .unwrap_or(0),
    )
}

extern "C" fn cell_set_x_placement(this: Id, _: Sel, value: usize) {
    let index = cell_index(this);
    let owner = part(this).grid;
    if let Some(slot) = grid(owner).cell_x.as_mut_slice().get_mut(index) {
        *slot = value.min(placement::FILL) as u8;
    }
    lay_out(owner);
}

extern "C" fn cell_y_placement(this: Id, _: Sel) -> usize {
    let index = cell_index(this);
    usize::from(
        grid(part(this).grid)
            .cell_y
            .as_slice()
            .get(index)
            .copied()
            .unwrap_or(0),
    )
}

extern "C" fn cell_set_y_placement(this: Id, _: Sel, value: usize) {
    let index = cell_index(this);
    let owner = part(this).grid;
    if let Some(slot) = grid(owner).cell_y.as_mut_slice().get_mut(index) {
        *slot = value.min(placement::FILL) as u8;
    }
    lay_out(owner);
}

extern "C" fn cell_row(this: Id, _: Sel) -> Id {
    let fields = part(this);
    new_part(&NSGRIDROW, fields.grid, fields.second, 0)
}

extern "C" fn cell_column(this: Id, _: Sel) -> Id {
    let fields = part(this);
    new_part(&NSGRIDCOLUMN, fields.grid, fields.index, 0)
}

static EMPTY: crate::libsystem::Exclusive<usize> = crate::libsystem::Exclusive::new(0);

extern "C" fn empty_content_view(_: Id, _: Sel) -> Id {
    EMPTY.with(|empty| {
        if *empty == 0 {
            *empty = new_instance(class_pointer(&NSGRIDCELL)) as usize;
        }
        *empty as Id
    })
}

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

pub fn define(runtime: &mut Runtime) -> bool {
    let grid_methods = methods![
        "initWithFrame:" => init_with_frame,
        "dealloc" => dealloc,
        "addRowWithViews:" => add_row_with_views,
        "insertRowAtIndex:withViews:" => insert_row_at,
        "addColumnWithViews:" => add_column_with_views,
        "insertColumnAtIndex:withViews:" => insert_column_at,
        "removeRowAtIndex:" => remove_row_at,
        "removeColumnAtIndex:" => remove_column_at,
        "moveRowAtIndex:toIndex:" => move_row,
        "moveColumnAtIndex:toIndex:" => move_column,
        "mergeCellsInHorizontalRange:verticalRange:" => merge_cells,
        "numberOfRows" => number_of_rows,
        "numberOfColumns" => number_of_columns,
        "rowAtIndex:" => row_at_index,
        "columnAtIndex:" => column_at_index,
        "cellAtColumnIndex:rowIndex:" => cell_at,
        "cellForView:" => cell_for_view,
        "rowSpacing" => row_spacing,
        "setRowSpacing:" => set_row_spacing,
        "columnSpacing" => column_spacing,
        "setColumnSpacing:" => set_column_spacing,
        "xPlacement" => x_placement,
        "setXPlacement:" => set_x_placement,
        "yPlacement" => y_placement,
        "setYPlacement:" => set_y_placement,
        "rowAlignment" => y_placement,
        "setRowAlignment:" => ignore_word,
        "layout" => layout_method,
        "fittingSize" => fitting_size,
        "intrinsicContentSize" => fitting_size,
        "drawRect:" => draw_rect,
    ];
    let grid_class = methods![
        "gridViewWithViews:" => grid_with_views,
        "gridViewWithNumberOfColumns:rows:" => grid_with_counts,
    ];
    let row_methods = methods![
        "dealloc" => part_dealloc,
        "height" => row_height,
        "setHeight:" => set_row_height,
        "yPlacement" => row_y_placement,
        "setYPlacement:" => set_row_y_placement,
        "numberOfCells" => row_number_of_cells,
        "cellAtIndex:" => row_cell_at,
        "topPadding" => row_top_padding,
        "setTopPadding:" => set_row_top_padding,
        "bottomPadding" => row_bottom_padding,
        "setBottomPadding:" => set_row_bottom_padding,
        "isHidden" => row_is_hidden,
        "setHidden:" => set_row_hidden,
        "mergeCellsInRange:" => row_merge_cells,
    ];
    let column_methods = methods![
        "dealloc" => part_dealloc,
        "width" => column_width,
        "setWidth:" => set_column_width,
        "xPlacement" => column_x_placement,
        "setXPlacement:" => set_column_x_placement,
        "numberOfCells" => column_number_of_cells,
        "cellAtIndex:" => column_cell_at,
        "leadingPadding" => column_leading_padding,
        "setLeadingPadding:" => set_column_leading_padding,
        "trailingPadding" => column_trailing_padding,
        "setTrailingPadding:" => set_column_trailing_padding,
        "isHidden" => column_is_hidden,
        "setHidden:" => set_column_hidden,
        "mergeCellsInRange:" => column_merge_cells,
    ];
    let cell_methods = methods![
        "dealloc" => part_dealloc,
        "contentView" => cell_content_view,
        "setContentView:" => cell_set_content_view,
        "xPlacement" => cell_x_placement,
        "setXPlacement:" => cell_set_x_placement,
        "yPlacement" => cell_y_placement,
        "setYPlacement:" => cell_set_y_placement,
        "row" => cell_row,
        "column" => cell_column,
    ];
    let cell_class = methods![
        "emptyContentView" => empty_content_view,
    ];
    super::define(
        runtime,
        b"NSGridView\0",
        (&NSGRIDVIEW, &NSGRIDVIEW_META),
        &NSVIEW,
        core::mem::size_of::<GridFields>(),
        &grid_methods,
        &grid_class,
    ) && super::define(
        runtime,
        b"NSGridRow\0",
        (&NSGRIDROW, &NSGRIDROW_META),
        &NSOBJECT,
        core::mem::size_of::<PartFields>(),
        &row_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSGridColumn\0",
        (&NSGRIDCOLUMN, &NSGRIDCOLUMN_META),
        &NSOBJECT,
        core::mem::size_of::<PartFields>(),
        &column_methods,
        &[],
    ) && super::define(
        runtime,
        b"NSGridCell\0",
        (&NSGRIDCELL, &NSGRIDCELL_META),
        &NSOBJECT,
        core::mem::size_of::<PartFields>(),
        &cell_methods,
        &cell_class,
    )
}

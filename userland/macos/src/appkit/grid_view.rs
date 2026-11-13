//! NSGridView, NSGridRow, NSGridColumn and NSGridCell (D385): views laid out in rows and
//! columns, each column as wide and each row as tall as its largest view unless the app set
//! its size, with the grid's spacing between them and each view placed in its cell by the x
//! and y placements of the cell, else its column or row, else the grid. The grid lays its
//! views out whenever they change and before it draws; a grid with no size takes the size its
//! cells need.

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
    let widths: [i32; MOST_COLUMNS] =
        layout::sizes(column_tracks.as_slice(), rows, |column, row| {
            view_size(cell_view(this, column, row)).0
        });
    let heights: [i32; MOST_ROWS] = layout::sizes(row_tracks.as_slice(), columns, |row, column| {
        view_size(cell_view(this, column, row)).1
    });
    column_tracks.free();
    row_tracks.free();
    let (column_gap, row_gap) = spacing(this);
    let total = (
        layout::total(&widths[..columns], column_gap),
        layout::total(&heights[..rows], row_gap),
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
    let lefts: [i32; MOST_COLUMNS] = layout::starts(&widths[..columns], column_gap);
    let tops: [i32; MOST_ROWS] = layout::starts(&heights[..rows], row_gap);
    for row in 0..rows {
        for column in 0..columns {
            let shown = cell_view(this, column, row);
            if shown.is_null() || !super::is(shown, &NSVIEW) {
                continue;
            }
            let index = row * columns + column;
            let fields = grid(this);
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
            let cell = Rect::new(lefts[column], tops[row], widths[column], heights[row]);
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
    view::dispose(this);
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
        "setTopPadding:" => ignore_word,
        "setBottomPadding:" => ignore_word,
    ];
    let column_methods = methods![
        "dealloc" => part_dealloc,
        "width" => column_width,
        "setWidth:" => set_column_width,
        "xPlacement" => column_x_placement,
        "setXPlacement:" => set_column_x_placement,
        "numberOfCells" => column_number_of_cells,
        "cellAtIndex:" => column_cell_at,
        "setLeadingPadding:" => ignore_word,
        "setTrailingPadding:" => ignore_word,
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

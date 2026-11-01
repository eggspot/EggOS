use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use eggos_toolkit::text::Text;

const SLOTS: usize = 64;
const NO_SLOT: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RowCells {
    pub(crate) name: Text<64>,
    pub(crate) size: Text<64>,
    pub(crate) state: Text<64>,
    pub(crate) short_state: Text<64>,
    pub(crate) host: Text<64>,
}

#[derive(Debug, Default)]
struct Cells {
    rows: Vec<RowCells>,
    phone: bool,
}

static SHELVES: [AtomicPtr<Cells>; SLOTS] = [const { AtomicPtr::new(ptr::null_mut()) }; SLOTS];

#[derive(Debug)]
pub(crate) struct Shelf {
    cells: *mut Cells,
    slot: Option<usize>,
}

impl Shelf {
    pub(crate) fn new() -> Self {
        let cells = Box::into_raw(Box::<Cells>::default());
        let slot = SHELVES.iter().position(|shelf| {
            shelf
                .compare_exchange(ptr::null_mut(), cells, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        });
        Self { cells, slot }
    }

    pub(crate) fn data(&self) -> u32 {
        self.slot.map_or(NO_SLOT, |slot| slot as u32)
    }

    pub(crate) fn rows(&self) -> &[RowCells] {
        // SAFETY: `cells` came from `Box::into_raw` in `new` and is freed only in `drop`; the
        // only mutable borrow of it is inside `set`, which runs no other code while it holds it.
        let cells = unsafe { &*self.cells };
        &cells.rows
    }

    pub(crate) fn set(&mut self, rows: &[RowCells], phone: bool) {
        // SAFETY: `cells` came from `Box::into_raw` in `new` and is freed only in `drop`. The
        // shelf is not `Send`, so only the thread holding the window reads it through `cell`,
        // and only while the window's tree paints or describes, never during this call, which
        // copies `rows` in and returns before the window touches its tree again.
        let cells = unsafe { &mut *self.cells };
        cells.rows.clear();
        cells.rows.extend_from_slice(rows);
        cells.phone = phone;
    }
}

impl Drop for Shelf {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            SHELVES[slot].store(ptr::null_mut(), Ordering::Release);
        }
        // SAFETY: `cells` came from `Box::into_raw` in `new`, is no longer on any shelf, and is
        // freed only here, once.
        drop(unsafe { Box::from_raw(self.cells) });
    }
}

pub(crate) fn cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    let Some(shelf) = SHELVES.get(data as usize) else {
        return;
    };
    let cells = shelf.load(Ordering::Acquire);
    if cells.is_null() {
        return;
    }
    // SAFETY: a non-null pointer on a shelf is the `Cells` of the live `Shelf` that put it
    // there; that shelf's window owns the only tree whose table names this slot, so this runs
    // on the window's thread inside its tree's drawing or describing, while no `set` holds a
    // mutable borrow, and the shelf takes the pointer off before freeing it.
    let cells = unsafe { &*cells };
    let Some(row) = cells.rows.get(row) else {
        return;
    };
    let text = match (cells.phone, column) {
        (_, 0) => &row.name,
        (false, 1) => &row.size,
        (false, 2) => &row.state,
        (false, 3) => &row.host,
        (true, 1) => &row.short_state,
        _ => return,
    };
    *out = *text;
}

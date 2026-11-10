//! Text for a table's cells kept in a program's statics. A table asks its program for each cell
//! through a plain function ([`crate::table::CellText`]) and a `data` number, so the program
//! keeps the text it shows in a static [`CellStore`]: each window takes one slot of it, writes
//! its rows' cells there and gives the slot's number as its table's `data`. Every byte is an
//! atomic, so nothing is `unsafe`, and a window that finds every slot taken gets none and shows
//! empty cells instead of waiting for one.

use crate::text::Text;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// The most bytes of UTF-8 one cell keeps; longer text is cut at the last whole character
/// that fits.
pub const CELL_BYTES: usize = 63;
/// The `data` of a table whose window has no slot: every cell reads empty.
pub const NO_SLOT: u32 = u32::MAX;

struct StoredCell {
    length: AtomicU8,
    bytes: [AtomicU8; CELL_BYTES],
}

impl StoredCell {
    const fn new() -> Self {
        Self {
            length: AtomicU8::new(0),
            bytes: [const { AtomicU8::new(0) }; CELL_BYTES],
        }
    }

    fn store(&self, text: &str) {
        let length = cut_length(text, CELL_BYTES);
        for (cell, &byte) in self.bytes.iter().zip(&text.as_bytes()[..length]) {
            cell.store(byte, Ordering::Relaxed);
        }
        self.length.store(length as u8, Ordering::Release);
    }

    fn write(&self, out: &mut Text<64>) {
        let length = usize::from(self.length.load(Ordering::Acquire)).min(CELL_BYTES);
        let mut bytes = [0u8; CELL_BYTES];
        for (byte, cell) in bytes.iter_mut().zip(&self.bytes[..length]) {
            *byte = cell.load(Ordering::Relaxed);
        }
        out.push_str(core::str::from_utf8(&bytes[..length]).unwrap_or("?"));
    }
}

/// The length of `text` cut to at most `most` bytes, at the last whole character that fits.
pub fn cut_length(text: &str, most: usize) -> usize {
    let mut length = text.len().min(most);
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    length
}

struct Slot<const ROWS: usize, const COLUMNS: usize> {
    taken: AtomicBool,
    cells: [[StoredCell; COLUMNS]; ROWS],
}

impl<const ROWS: usize, const COLUMNS: usize> Slot<ROWS, COLUMNS> {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            cells: [const { [const { StoredCell::new() }; COLUMNS] }; ROWS],
        }
    }
}

/// `SLOTS` slots of `ROWS` rows of `COLUMNS` cells, for a `static`.
pub struct CellStore<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> {
    slots: [Slot<ROWS, COLUMNS>; SLOTS],
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> core::fmt::Debug
    for CellStore<SLOTS, ROWS, COLUMNS>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CellStore")
            .field("slots", &SLOTS)
            .field("rows", &ROWS)
            .field("columns", &COLUMNS)
            .finish()
    }
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> Default
    for CellStore<SLOTS, ROWS, COLUMNS>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> CellStore<SLOTS, ROWS, COLUMNS> {
    /// Every slot free and every cell empty.
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::<ROWS, COLUMNS>::new() }; SLOTS],
        }
    }

    /// Takes a free slot until the returned [`CellSlot`] is dropped; `None` when every slot
    /// is taken.
    pub fn take(&self) -> Option<CellSlot<'_, SLOTS, ROWS, COLUMNS>> {
        let index = self.slots.iter().position(|slot| {
            slot.taken
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })?;
        Some(CellSlot { store: self, index })
    }

    /// Writes the text of cell (`row`, `column`) of slot `data` into `out`; nothing when
    /// `data` is [`NO_SLOT`] or any of them is out of range.
    pub fn write(&self, data: u32, row: usize, column: usize, out: &mut Text<64>) {
        if let Some(cell) = usize::try_from(data)
            .ok()
            .and_then(|index| self.slots.get(index))
            .and_then(|slot| slot.cells.get(row))
            .and_then(|cells| cells.get(column))
        {
            cell.write(out);
        }
    }
}

/// One taken slot of a [`CellStore`], given back when dropped.
pub struct CellSlot<'a, const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> {
    store: &'a CellStore<SLOTS, ROWS, COLUMNS>,
    index: usize,
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> core::fmt::Debug
    for CellSlot<'_, SLOTS, ROWS, COLUMNS>
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CellSlot")
            .field("index", &self.index)
            .finish()
    }
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize>
    CellSlot<'_, SLOTS, ROWS, COLUMNS>
{
    /// The table `data` that names this slot.
    pub fn data(&self) -> u32 {
        self.index as u32
    }

    /// Keeps `text`, cut to [`CELL_BYTES`], as cell (`row`, `column`); nothing when either is
    /// out of range.
    pub fn set(&self, row: usize, column: usize, text: &str) {
        if let Some(cell) = self.store.slots[self.index]
            .cells
            .get(row)
            .and_then(|cells| cells.get(column))
        {
            cell.store(text);
        }
    }
}

impl<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize> Drop
    for CellSlot<'_, SLOTS, ROWS, COLUMNS>
{
    fn drop(&mut self) {
        self.store.slots[self.index]
            .taken
            .store(false, Ordering::Release);
    }
}

/// [`CellSlot::data`] of `slot`, or [`NO_SLOT`] without one.
pub fn data_of<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize>(
    slot: Option<&CellSlot<'_, SLOTS, ROWS, COLUMNS>>,
) -> u32 {
    slot.map_or(NO_SLOT, |slot| slot.data())
}

#[cfg(test)]
mod tests;

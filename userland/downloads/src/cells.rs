use alloc::vec::Vec;
use eggos_download_list::MAX_DOWNLOADS;
use eggos_toolkit::cell_store::{CellSlot, CellStore, data_of};
use eggos_toolkit::text::Text;

const SLOTS: usize = if cfg!(test) { 64 } else { 2 };
const COLUMNS: usize = 4;

static DOWNLOAD_CELLS: CellStore<SLOTS, MAX_DOWNLOADS, COLUMNS> = CellStore::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RowCells {
    pub(crate) name: Text<64>,
    pub(crate) size: Text<64>,
    pub(crate) state: Text<64>,
    pub(crate) short_state: Text<64>,
    pub(crate) host: Text<64>,
}

impl RowCells {
    fn shown(&self, phone: bool) -> [&str; COLUMNS] {
        match phone {
            false => [
                self.name.as_str(),
                self.size.as_str(),
                self.state.as_str(),
                self.host.as_str(),
            ],
            true => [self.name.as_str(), self.short_state.as_str(), "", ""],
        }
    }
}

#[derive(Debug)]
pub(crate) struct Shelf {
    slot: Option<CellSlot<'static, SLOTS, MAX_DOWNLOADS, COLUMNS>>,
    rows: Vec<RowCells>,
}

impl Shelf {
    pub(crate) fn new() -> Self {
        Self {
            slot: DOWNLOAD_CELLS.take(),
            rows: Vec::new(),
        }
    }

    pub(crate) fn data(&self) -> u32 {
        data_of(self.slot.as_ref())
    }

    pub(crate) fn rows(&self) -> &[RowCells] {
        &self.rows
    }

    pub(crate) fn set(&mut self, rows: &[RowCells], phone: bool) {
        self.rows.clear();
        self.rows.extend_from_slice(rows);
        let Some(slot) = &self.slot else {
            return;
        };
        for (index, row) in rows.iter().enumerate() {
            for (column, text) in row.shown(phone).into_iter().enumerate() {
                slot.set(index, column, text);
            }
        }
    }
}

pub(crate) fn cell(data: u32, row: usize, column: usize, out: &mut Text<64>) {
    DOWNLOAD_CELLS.write(data, row, column, out);
}

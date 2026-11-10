use super::{CELL_BYTES, CellStore, NO_SLOT, cut_length, data_of};
use crate::text::Text;

fn read<const SLOTS: usize, const ROWS: usize, const COLUMNS: usize>(
    store: &CellStore<SLOTS, ROWS, COLUMNS>,
    data: u32,
    row: usize,
    column: usize,
) -> String {
    let mut out = Text::<64>::new();
    store.write(data, row, column, &mut out);
    String::from(out.as_str())
}

#[test]
fn a_full_store_gives_no_slot_instead_of_waiting() {
    let store = CellStore::<2, 1, 1>::new();
    let first = store.take().expect("a free slot");
    let second = store.take().expect("a second free slot");
    assert_ne!(first.data(), second.data());
    assert!(store.take().is_none());
    drop(first);
    let again = store.take().expect("the slot given back");
    assert!(store.take().is_none());
    drop((second, again));
}

#[test]
fn a_slot_keeps_each_cell_and_other_slots_stay_apart() {
    let store = CellStore::<2, 2, 2>::new();
    let one = store.take().expect("a slot");
    let two = store.take().expect("a slot");
    one.set(0, 0, "name");
    one.set(1, 1, "size");
    two.set(0, 0, "other");
    assert_eq!(read(&store, one.data(), 0, 0), "name");
    assert_eq!(read(&store, one.data(), 1, 1), "size");
    assert_eq!(read(&store, two.data(), 0, 0), "other");
    one.set(0, 0, "shorter");
    assert_eq!(read(&store, one.data(), 0, 0), "shorter");
}

#[test]
fn out_of_range_cells_and_no_slot_read_empty_and_ignore_writes() {
    let store = CellStore::<1, 1, 1>::new();
    let slot = store.take().expect("a slot");
    slot.set(5, 0, "ignored");
    slot.set(0, 5, "ignored");
    assert_eq!(read(&store, slot.data(), 5, 0), "");
    assert_eq!(read(&store, slot.data(), 0, 5), "");
    assert_eq!(read(&store, NO_SLOT, 0, 0), "");
    assert_eq!(data_of(Some(&slot)), slot.data());
    assert_eq!(data_of::<1, 1, 1>(None), NO_SLOT);
}

#[test]
fn long_text_is_cut_at_a_whole_character() {
    let store = CellStore::<1, 1, 1>::new();
    let slot = store.take().expect("a slot");
    let long = "\u{e9}".repeat(40);
    slot.set(0, 0, &long);
    let kept = read(&store, slot.data(), 0, 0);
    assert_eq!(kept, "\u{e9}".repeat(CELL_BYTES / 2));
    assert_eq!(cut_length("ab\u{e9}", 3), 2);
    assert_eq!(cut_length("abc", 10), 3);
}

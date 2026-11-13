//! Function tables the program registers at run time for code it generates (D344):
//! `RtlAddFunctionTable`, `RtlAddGrowableFunctionTable` and `RtlInstallFunctionTableCallback`,
//! searched after the program's own exception directory, as Windows searches them after the
//! loaded images'. A table's entries are relative to its base and sorted by start, and may lie
//! anywhere in memory; the code and its unwind data lie from the base up to the end of the run
//! of pages that holds the code's end.

use super::super::{SpinLock, call_program, memory};
use super::context::unwinder;
use crate::unwind::Image;

const MOST_TABLES: usize = 64;
const CALLBACK_IDENTIFIER_BITS: u64 = 3;
const FALSE: u8 = 0;
const STATUS_SUCCESS: u32 = 0;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_NO_MEMORY: u32 = 0xc000_0017;

#[derive(Debug, Clone, Copy)]
enum Entries {
    Table { address: u64, count: u32 },
    Callback { callback: u64, context: u64 },
}

#[derive(Debug, Clone, Copy)]
struct Registered {
    identifier: u64,
    base: u64,
    code_end: u64,
    end: u64,
    entries: Entries,
}

static TABLES: SpinLock<[Option<Registered>; MOST_TABLES]> = SpinLock::new([None; MOST_TABLES]);

fn region_end(address: u64) -> Option<u64> {
    let region = memory::region_of(usize::try_from(address).ok()?).ok()?;
    Some(region.base.checked_add(region.size)? as u64)
}

fn reach(base: u64, code_end: u64) -> u64 {
    let last = code_end.saturating_sub(1).max(base);
    region_end(last).unwrap_or(code_end).max(code_end)
}

fn register(table: Registered) -> bool {
    TABLES.with(
        |tables| match tables.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(table);
                true
            }
            None => false,
        },
    )
}

fn unregister(identifier: u64) -> bool {
    TABLES.with(|tables| {
        let found = tables
            .iter_mut()
            .find(|slot| slot.is_some_and(|table| table.identifier == identifier));
        match found {
            Some(slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    })
}

fn image_of(base: u64, end: u64) -> Image<'static> {
    // SAFETY: the program generated its code and unwind data in memory it keeps mapped from
    // `base` up to `end` while the table is registered, as Windows requires; only the bytes its
    // entries name are read, and a read of memory it unmapped faults as its own read would.
    let bytes = unsafe { core::slice::from_raw_parts(base as *const u8, (end - base) as usize) };
    Image { base, bytes }
}

fn in_table(address: u64, count: u32, code: &Image<'_>, pc_rva: u64) -> Option<unwinder::Function> {
    if address == 0 {
        return None;
    }
    let size = u64::from(count).checked_mul(unwinder::RUNTIME_FUNCTION_SIZE)?;
    // SAFETY: the program registered `count` entries at `address` (or its callback returned
    // one) and keeps them while the table is registered, as Windows requires.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) };
    let table = Image {
        base: address,
        bytes,
    };
    let mut function = unwinder::lookup_in(&table, 0, u32::try_from(size).ok()?, code, pc_rva)?;
    function.entry = address.wrapping_add(function.entry).wrapping_sub(code.base);
    Some(function)
}

/// The function whose code holds `pc` in a table the program registered, with the image its
/// addresses are relative to (`Function::entry` relative to it too, wrapping when the table
/// lies below the base).
pub fn lookup(pc: u64) -> Option<(Image<'static>, unwinder::Function)> {
    let tables = TABLES.with(|tables| *tables);
    tables
        .iter()
        .flatten()
        .filter(|table| (table.base..table.code_end).contains(&pc))
        .find_map(|table| {
            let code = image_of(table.base, table.end);
            let pc_rva = pc - table.base;
            let function = match table.entries {
                Entries::Table { address, count } => in_table(address, count, &code, pc_rva),
                Entries::Callback { callback, context } => {
                    // SAFETY: the program's `PGET_RUNTIME_FUNCTION_CALLBACK`, called with the
                    // address it is asked about and the context it gave.
                    let entry = unsafe {
                        call_program(callback as usize, [pc as usize, context as usize, 0, 0])
                    };
                    in_table(entry as u64, 1, &code, pc_rva)
                }
            }?;
            Some((code, function))
        })
}

/// `RtlAddFunctionTable`: `count` entries at `table`, relative to `base`.
pub(super) extern "efiapi" fn add_function_table(table: u64, count: u32, base: u64) -> u8 {
    if table == 0 || count == 0 {
        return FALSE;
    }
    let end = reach(base, base.saturating_add(1));
    let added = register(Registered {
        identifier: table,
        base,
        code_end: end,
        end,
        entries: Entries::Table {
            address: table,
            count,
        },
    });
    u8::from(added)
}

/// `RtlDeleteFunctionTable`: a table `RtlAddFunctionTable` registered, or a callback by its
/// identifier.
pub(super) extern "efiapi" fn delete_function_table(table: u64) -> u8 {
    u8::from(unregister(table))
}

/// `RtlInstallFunctionTableCallback`: `callback` answers for the `length` bytes of code from
/// `base`; `identifier` has its two low bits set and later deletes it.
pub(super) extern "efiapi" fn install_function_table_callback(
    identifier: u64,
    base: u64,
    length: u32,
    callback: u64,
    context: u64,
    _out_of_process_dll: u64,
) -> u8 {
    if identifier & CALLBACK_IDENTIFIER_BITS != CALLBACK_IDENTIFIER_BITS || callback == 0 {
        return FALSE;
    }
    let code_end = base.saturating_add(u64::from(length));
    let installed = register(Registered {
        identifier,
        base,
        code_end,
        end: reach(base, code_end),
        entries: Entries::Callback { callback, context },
    });
    u8::from(installed)
}

/// `RtlAddGrowableFunctionTable`: `count` entries at `table` for the code from `range_base` to
/// `range_end`, more of them later by `RtlGrowFunctionTable`; the table's handle written to
/// `handle`.
///
/// # Safety
/// `handle` is null or a writable word.
pub(super) unsafe extern "efiapi" fn add_growable_function_table(
    handle: *mut u64,
    table: u64,
    count: u32,
    _maximum: u32,
    range_base: u64,
    range_end: u64,
) -> u32 {
    if handle.is_null() || table == 0 || range_end <= range_base {
        return STATUS_INVALID_PARAMETER;
    }
    let added = register(Registered {
        identifier: table,
        base: range_base,
        code_end: range_end,
        end: reach(range_base, range_end),
        entries: Entries::Table {
            address: table,
            count,
        },
    });
    if !added {
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's word for the handle.
    unsafe { handle.write_unaligned(table) };
    STATUS_SUCCESS
}

/// `RtlGrowFunctionTable`: the growable table `handle` now has `count` entries.
pub(super) extern "efiapi" fn grow_function_table(handle: u64, count: u32) {
    TABLES.with(|tables| {
        for table in tables.iter_mut().flatten() {
            if let (true, Entries::Table { count: entries, .. }) =
                (table.identifier == handle, &mut table.entries)
            {
                *entries = count;
            }
        }
    });
}

/// `RtlDeleteGrowableFunctionTable`.
pub(super) extern "efiapi" fn delete_growable_function_table(handle: u64) {
    unregister(handle);
}

//! `IShellItemArray` and its `IEnumShellItems` (D742): a fixed set of absolute item ID lists,
//! each handed out as a new `IShellItem`, with their attributes combined by `AND` or `OR`;
//! made by the file dialogs for their results and by `SHCreateShellItemArray`,
//! `SHCreateShellItemArrayFromIDLists` and `SHCreateShellItemArrayFromShellItem`. Handlers,
//! property stores and description lists are not supported.

use super::heap;
use super::shell_namespace::{
    attributes_of_list, clone_list, combined_list, list_bytes, list_of_object, new_shell_item,
};
use crate::com::guid::{self, Guid};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_FALSE, S_OK};
use crate::library::Export;
use core::sync::atomic::{AtomicU32, Ordering};

const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const SIATTRIBFLAGS_OR: u32 = 2;
const SIATTRIBFLAGS_MASK: u32 = 3;

/// `IID_IShellItemArray`.
pub(super) const SHELL_ITEM_ARRAY: Guid = Guid::new(
    0xb63e_a76d,
    0x1f85,
    0x456f,
    [0xa1, 0x9c, 0x48, 0x15, 0x9e, 0xfa, 0x85, 0x8b],
);
const ENUM_SHELL_ITEMS: Guid = Guid::new(
    0x7062_9033,
    0xe363,
    0x4a28,
    [0xa5, 0x67, 0x0d, 0xb7, 0x80, 0x06, 0xe6, 0xd7],
);

#[derive(Debug)]
#[repr(transparent)]
struct Table<const N: usize>([*const (); N]);

// SAFETY: the table holds functions' addresses, fixed for the program's life.
unsafe impl<const N: usize> Sync for Table<N> {}

impl<const N: usize> Table<N> {
    fn address(&'static self) -> usize {
        self.0.as_ptr() as usize
    }
}

#[repr(C)]
#[derive(Debug)]
struct ItemArray {
    table: usize,
    references: AtomicU32,
    lists: *mut *mut u8,
    count: usize,
}

#[repr(C)]
#[derive(Debug)]
struct Enumerator {
    table: usize,
    references: AtomicU32,
    array: *mut ItemArray,
    at: usize,
}

unsafe fn array<'a>(this: *mut ItemArray) -> &'a ItemArray {
    // SAFETY: the caller's duty: a live array.
    unsafe { &*this }
}

unsafe fn list_at<'a>(found: &ItemArray, index: usize) -> &'a [u8] {
    // SAFETY: the array owns `count` lists, each a whole list it made.
    unsafe { list_bytes(found.lists.add(index).read()) }.unwrap_or(&[0, 0])
}

fn free_lists(lists: &[*mut u8]) {
    for &list in lists {
        // SAFETY: lists the array was given to own, each freed once.
        unsafe { heap::free(list) };
    }
}

/// A new item array that owns `lists` (absolute lists in the task allocator), with one
/// reference; the lists are freed when it cannot be made.
pub(super) fn new_array(lists: &[*mut u8]) -> Result<usize, HResult> {
    if lists.iter().any(|list| list.is_null()) {
        free_lists(lists);
        return Err(E_OUTOFMEMORY);
    }
    let block = heap::allocate(lists.len().max(1) * core::mem::size_of::<usize>()).cast();
    let object = crate::com::allocate(core::mem::size_of::<ItemArray>()).cast::<ItemArray>();
    if block.is_null() || object.is_null() {
        free_lists(lists);
        // SAFETY: blocks made above, unused; null is ignored.
        unsafe {
            heap::free(block.cast());
            crate::com::free(object.cast());
        }
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the fresh block holds `lists.len()` pointers; the object block fits an array.
    unsafe {
        block.copy_from_nonoverlapping(lists.as_ptr(), lists.len());
        object.write(ItemArray {
            table: ARRAY_TABLE.address(),
            references: AtomicU32::new(1),
            lists: block,
            count: lists.len(),
        });
    }
    Ok(object as usize)
}

unsafe fn write_out<T>(out: *mut T, value: T) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a writable pointer from the program.
    unsafe { out.write_unaligned(value) };
    S_OK
}

unsafe extern "efiapi" fn array_query_interface(
    this: *mut ItemArray,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &array(this).references };
    let interfaces = [guid::UNKNOWN, SHELL_ITEM_ARRAY];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn array_add_reference(this: *mut ItemArray) -> u32 {
    // SAFETY: the caller's duty: a live array.
    unsafe { array(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "efiapi" fn array_release(this: *mut ItemArray) -> u32 {
    // SAFETY: the caller's duty: a live array it holds a reference to.
    let found = unsafe { array(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    // SAFETY: the array's own lists and block; no reference is left.
    unsafe {
        free_lists(core::slice::from_raw_parts(found.lists, found.count));
        heap::free(found.lists.cast());
        crate::com::free(this.cast());
    }
    0
}

unsafe extern "efiapi" fn array_unsupported(
    _this: *mut ItemArray,
    _first: usize,
    _second: *const Guid,
    _interface: *const Guid,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller passes a writable interface pointer, or null.
    let _ = unsafe { write_out(out, 0) };
    E_NOTIMPL
}

unsafe extern "efiapi" fn array_unsupported_list(
    _this: *mut ItemArray,
    _key: usize,
    _interface: *const Guid,
    out: *mut usize,
) -> HResult {
    // SAFETY: as above.
    let _ = unsafe { write_out(out, 0) };
    E_NOTIMPL
}

unsafe extern "efiapi" fn array_get_attributes(
    this: *mut ItemArray,
    how: u32,
    mask: u32,
    out: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live array.
    let found = unsafe { array(this) };
    let any = how & SIATTRIBFLAGS_MASK == SIATTRIBFLAGS_OR;
    let mut combined = match any {
        true => 0,
        false => u32::MAX,
    };
    for index in 0..found.count {
        // SAFETY: an index below the count.
        let attributes = attributes_of_list(unsafe { list_at(found, index) });
        combined = match any {
            true => combined | attributes,
            false => combined & attributes,
        };
    }
    let answer = match found.count {
        0 => 0,
        _ => combined & mask,
    };
    // SAFETY: the caller passes a writable mask.
    unsafe { out.write(answer) };
    if answer == mask { S_OK } else { S_FALSE }
}

unsafe extern "efiapi" fn array_get_count(this: *mut ItemArray, out: *mut u32) -> HResult {
    // SAFETY: the caller's duty: a live array, and a writable count or null.
    unsafe { write_out(out, array(this).count as u32) }
}

fn item_of(found: &ItemArray, index: usize) -> Result<usize, HResult> {
    if index >= found.count {
        return Err(E_INVALIDARG);
    }
    // SAFETY: an index below the count.
    new_shell_item(clone_list(unsafe { list_at(found, index) }))
}

unsafe extern "efiapi" fn array_get_item_at(
    this: *mut ItemArray,
    index: u32,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live array.
    let made = item_of(unsafe { array(this) }, index as usize);
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

fn new_enumerator(array: *mut ItemArray, at: usize) -> Result<usize, HResult> {
    let object = crate::com::allocate(core::mem::size_of::<Enumerator>()).cast::<Enumerator>();
    if object.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: a live array gains the enumerator's reference.
    unsafe { array_add_reference(array) };
    // SAFETY: a fresh block, sized and aligned for an enumerator.
    unsafe {
        object.write(Enumerator {
            table: ENUMERATOR_TABLE.address(),
            references: AtomicU32::new(1),
            array,
            at,
        })
    };
    Ok(object as usize)
}

unsafe extern "efiapi" fn array_enum_items(this: *mut ItemArray, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let made = new_enumerator(this, 0);
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

unsafe fn enumerator<'a>(this: *mut Enumerator) -> &'a mut Enumerator {
    // SAFETY: the caller's duty: a live enumerator, used by one thread at a time.
    unsafe { &mut *this }
}

unsafe extern "efiapi" fn enumerator_query_interface(
    this: *mut Enumerator,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &enumerator(this).references };
    let interfaces = [guid::UNKNOWN, ENUM_SHELL_ITEMS];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

unsafe extern "efiapi" fn enumerator_add_reference(this: *mut Enumerator) -> u32 {
    // SAFETY: the caller's duty: a live enumerator.
    unsafe { enumerator(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "efiapi" fn enumerator_release(this: *mut Enumerator) -> u32 {
    // SAFETY: the caller's duty: a live enumerator it holds a reference to.
    let found = unsafe { enumerator(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    // SAFETY: the enumerator's reference to its array, then its own block.
    unsafe {
        array_release(found.array);
        crate::com::free(this.cast());
    }
    0
}

unsafe extern "efiapi" fn enumerator_next(
    this: *mut Enumerator,
    wanted: u32,
    out: *mut usize,
    fetched: *mut u32,
) -> HResult {
    if out.is_null() || (wanted > 1 && fetched.is_null()) {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live enumerator over a live array.
    let found = unsafe { enumerator(this) };
    // SAFETY: as above.
    let source = unsafe { array(found.array) };
    let mut given = 0u32;
    while given < wanted && found.at < source.count {
        let Ok(item) = item_of(source, found.at) else {
            break;
        };
        // SAFETY: the caller passes room for `wanted` interface pointers.
        unsafe { out.add(given as usize).write(item) };
        given += 1;
        found.at += 1;
    }
    if !fetched.is_null() {
        // SAFETY: the caller passes a writable count.
        unsafe { fetched.write(given) };
    }
    if given == wanted { S_OK } else { S_FALSE }
}

unsafe extern "efiapi" fn enumerator_skip(this: *mut Enumerator, count: u32) -> HResult {
    // SAFETY: the caller's duty: a live enumerator over a live array.
    let found = unsafe { enumerator(this) };
    // SAFETY: as above.
    let total = unsafe { array(found.array) }.count;
    let before = found.at;
    found.at = (found.at + count as usize).min(total);
    if found.at - before == count as usize { S_OK } else { S_FALSE }
}

unsafe extern "efiapi" fn enumerator_reset(this: *mut Enumerator) -> HResult {
    // SAFETY: the caller's duty: a live enumerator.
    unsafe { enumerator(this) }.at = 0;
    S_OK
}

unsafe extern "efiapi" fn enumerator_clone(this: *mut Enumerator, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a live enumerator.
    let found = unsafe { enumerator(this) };
    let made = new_enumerator(found.array, found.at);
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

static ARRAY_TABLE: Table<10> = Table([
    export_entry!(array_query_interface),
    export_entry!(array_add_reference),
    export_entry!(array_release),
    export_entry!(array_unsupported),
    export_entry!(array_unsupported_list),
    export_entry!(array_unsupported_list),
    export_entry!(array_get_attributes),
    export_entry!(array_get_count),
    export_entry!(array_get_item_at),
    export_entry!(array_enum_items),
]);

static ENUMERATOR_TABLE: Table<7> = Table([
    export_entry!(enumerator_query_interface),
    export_entry!(enumerator_add_reference),
    export_entry!(enumerator_release),
    export_entry!(enumerator_next),
    export_entry!(enumerator_skip),
    export_entry!(enumerator_reset),
    export_entry!(enumerator_clone),
]);

unsafe fn hand_out_array(made: Result<usize, HResult>, out: *mut usize) -> HResult {
    if out.is_null() {
        if let Ok(object) = made {
            // SAFETY: the array just made, whose one reference goes.
            unsafe { array_release(object as *mut ItemArray) };
        }
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(*made.as_ref().unwrap_or(&0)) };
    made.map_or_else(|failure| failure, |_| S_OK)
}

fn lists_of(count: usize, list: impl Fn(usize) -> *mut u8) -> Result<usize, HResult> {
    let mut made = [core::ptr::null_mut(); 256];
    if count > made.len() {
        return Err(E_INVALIDARG);
    }
    for (index, slot) in made.iter_mut().enumerate().take(count) {
        *slot = list(index);
    }
    new_array(&made[..count])
}

unsafe extern "efiapi" fn sh_create_shell_item_array(
    parent: *const u8,
    folder: usize,
    count: u32,
    children: *const *const u8,
    out: *mut usize,
) -> HResult {
    if children.is_null() && count != 0 {
        return E_INVALIDARG;
    }
    // SAFETY: `SHCreateShellItemArray`'s contract: a readable parent list, or null.
    let given = unsafe { list_bytes(parent) }.is_some();
    let owned = match given {
        true => None,
        // SAFETY: the contract's other form: a live folder whose list is the parent.
        false => unsafe { list_of_object(folder) },
    };
    // SAFETY: the given parent list or the folder's copy, both whole lists.
    let parent_bytes = unsafe { list_bytes(owned.unwrap_or(parent.cast_mut())) };
    let made = lists_of(count as usize, |index| {
        // SAFETY: `count` readable child lists.
        let child = unsafe { list_bytes(children.add(index).read()) }.unwrap_or(&[0, 0]);
        match parent_bytes {
            Some(parent) => combined_list(parent, child),
            None => clone_list(child),
        }
    });
    if let Some(list) = owned {
        // SAFETY: the folder's list copy made above.
        unsafe { heap::free(list) };
    }
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_array(made, out) }
}

unsafe extern "efiapi" fn sh_create_shell_item_array_from_id_lists(
    count: u32,
    lists: *const *const u8,
    out: *mut usize,
) -> HResult {
    if lists.is_null() && count != 0 {
        return E_INVALIDARG;
    }
    let made = lists_of(count as usize, |index| {
        // SAFETY: `count` readable absolute lists.
        let list = unsafe { list_bytes(lists.add(index).read()) }.unwrap_or(&[0, 0]);
        clone_list(list)
    });
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_array(made, out) }
}

unsafe extern "efiapi" fn sh_create_shell_item_array_from_shell_item(
    item: usize,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller passes a writable interface pointer.
    unsafe { out.write(0) };
    // SAFETY: `SHCreateShellItemArrayFromShellItem`'s contract: a live shell item.
    let Some(list) = (unsafe { list_of_object(item) }) else {
        return E_INVALIDARG;
    };
    let made = match new_array(&[list]) {
        Ok(made) => made,
        Err(failure) => return failure,
    };
    // SAFETY: the array just made; its creation reference goes after the query.
    unsafe {
        let answer = array_query_interface(made as *mut ItemArray, interface, out);
        array_release(made as *mut ItemArray);
        answer
    }
}

pub(super) const EXPORTS: &[Export] = exports![
    b"SHCreateShellItemArray" => sh_create_shell_item_array,
    b"SHCreateShellItemArrayFromIDLists" => sh_create_shell_item_array_from_id_lists,
    b"SHCreateShellItemArrayFromShellItem" => sh_create_shell_item_array_from_shell_item,
];

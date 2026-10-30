//! COM's libraries `ole32` and `oleaut32` (D178), over the pure parts in `crate::com`: each
//! thread's apartment (`CoInitializeEx`, `OleInitialize` and their ends), counted in the TEB's
//! `ReservedForOle` word, which is ole32's own on Windows too; the task allocator
//! (`CoTaskMemAlloc` and its family, `CoGetMalloc`'s `IMalloc`) over the layer's heap; CLSIDs
//! and ProgIDs; `CoCreateInstance` for the one class the layer provides in process,
//! `Scripting.Dictionary`; `CreateStreamOnHGlobal`; the tables of methods of the layer's
//! objects, whose entries keep the program's `x18` on aarch64 (`abi.rs`); and oleaut32's
//! exports (`oleaut32.rs`), which programs import by ordinal. There is no registry, no
//! out-of-process server, no running object table and no moniker: such calls fail as Windows
//! reports a class, object or name it cannot find.

mod oleaut32;

pub(super) use oleaut32::{EXPORTS as OLEAUT32_EXPORTS, by_ordinal as oleaut32_by_ordinal};

use super::{heap, thread_environment};
use crate::com::guid::{self, Guid, TEXT_LENGTH};
use crate::com::{
    E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, HResult, S_FALSE, S_OK, dictionary,
    enumerator, memory_stream, wide_text,
};
use crate::library::Export;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

const RPC_E_CHANGED_MODE: HResult = 0x8001_0106_u32 as i32;
const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0_u32 as i32;
const CO_E_CLASSSTRING: HResult = 0x8004_01f3_u32 as i32;
const MK_E_SYNTAX: HResult = 0x8004_01e4_u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154_u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110_u32 as i32;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const MEMCTX_TASK: u32 = 1;
const TEB_RESERVED_FOR_OLE: usize = 0x1758;
const COM_COUNT_MASK: u64 = 0xffff_ffff;
const OLE_COUNT_SHIFT: u32 = 32;
const OLE_COUNT_MASK: u64 = 0xffff;
const MULTITHREADED: u64 = 1 << 63;
const IID_IMALLOC: Guid = Guid::new(0x0000_0002, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);

static MULTITHREADED_THREADS: AtomicU32 = AtomicU32::new(0);

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

static DICTIONARY: Table<22> = Table([
    export_entry!(dictionary::query_interface),
    export_entry!(dictionary::add_reference),
    export_entry!(dictionary::release),
    export_entry!(dictionary::get_type_info_count),
    export_entry!(dictionary::get_type_info),
    export_entry!(dictionary::get_ids_of_names),
    export_entry!(dictionary::invoke, two_stack_arguments),
    export_entry!(dictionary::put_item_reference),
    export_entry!(dictionary::put_item),
    export_entry!(dictionary::get_item),
    export_entry!(dictionary::add),
    export_entry!(dictionary::get_count),
    export_entry!(dictionary::exists),
    export_entry!(dictionary::items),
    export_entry!(dictionary::put_key),
    export_entry!(dictionary::keys),
    export_entry!(dictionary::remove),
    export_entry!(dictionary::remove_all),
    export_entry!(dictionary::put_compare_mode),
    export_entry!(dictionary::get_compare_mode),
    export_entry!(dictionary::new_enum),
    export_entry!(dictionary::get_hash_value),
]);

static ENUMERATOR: Table<7> = Table([
    export_entry!(enumerator::query_interface),
    export_entry!(enumerator::add_reference),
    export_entry!(enumerator::release),
    export_entry!(enumerator::next),
    export_entry!(enumerator::skip),
    export_entry!(enumerator::reset),
    export_entry!(enumerator::clone),
]);

static STREAM: Table<14> = Table([
    export_entry!(memory_stream::query_interface),
    export_entry!(memory_stream::add_reference),
    export_entry!(memory_stream::release),
    export_entry!(memory_stream::read),
    export_entry!(memory_stream::write),
    export_entry!(memory_stream::seek),
    export_entry!(memory_stream::set_size),
    export_entry!(memory_stream::copy_to),
    export_entry!(memory_stream::commit),
    export_entry!(memory_stream::revert),
    export_entry!(memory_stream::lock_region),
    export_entry!(memory_stream::lock_region),
    export_entry!(memory_stream::stat),
    export_entry!(memory_stream::clone),
]);

static TASK_ALLOCATOR_TABLE: Table<9> = Table([
    export_entry!(allocator_query_interface),
    export_entry!(allocator_add_reference),
    export_entry!(allocator_release),
    export_entry!(allocator_alloc),
    export_entry!(allocator_realloc),
    export_entry!(allocator_free),
    export_entry!(allocator_get_size),
    export_entry!(allocator_did_alloc),
    export_entry!(allocator_heap_minimize),
]);

static TASK_ALLOCATOR: &Table<9> = &TASK_ALLOCATOR_TABLE;

#[derive(Debug, Clone, Copy)]
struct Apartment {
    initializations: u32,
    ole_initializations: u16,
    multithreaded: bool,
}

fn apartment_word() -> *mut u64 {
    (thread_environment::current() + TEB_RESERVED_FOR_OLE) as *mut u64
}

fn apartment() -> Apartment {
    // SAFETY: the word is in the calling thread's TEB, 8-byte aligned; only this thread uses it.
    let word = unsafe { apartment_word().read() };
    Apartment {
        initializations: (word & COM_COUNT_MASK) as u32,
        ole_initializations: ((word >> OLE_COUNT_SHIFT) & OLE_COUNT_MASK) as u16,
        multithreaded: word & MULTITHREADED != 0,
    }
}

fn set_apartment(apartment: Apartment) {
    let mut word = u64::from(apartment.initializations)
        | (u64::from(apartment.ole_initializations) << OLE_COUNT_SHIFT);
    if apartment.multithreaded {
        word |= MULTITHREADED;
    }
    // SAFETY: as in `apartment`.
    unsafe { apartment_word().write(word) };
}

fn com_is_initialized() -> bool {
    apartment().initializations > 0 || MULTITHREADED_THREADS.load(Ordering::Acquire) > 0
}

extern "efiapi" fn co_initialize_ex(reserved: usize, flags: u32) -> HResult {
    if reserved != 0 {
        return E_INVALIDARG;
    }
    let multithreaded = flags & COINIT_APARTMENTTHREADED == 0;
    let mut state = apartment();
    if state.initializations > 0 {
        if state.multithreaded != multithreaded {
            return RPC_E_CHANGED_MODE;
        }
        state.initializations = state.initializations.saturating_add(1);
        set_apartment(state);
        return S_FALSE;
    }
    state.initializations = 1;
    state.multithreaded = multithreaded;
    if multithreaded {
        MULTITHREADED_THREADS.fetch_add(1, Ordering::AcqRel);
    }
    set_apartment(state);
    S_OK
}

extern "efiapi" fn co_initialize(reserved: usize) -> HResult {
    co_initialize_ex(reserved, COINIT_APARTMENTTHREADED)
}

extern "efiapi" fn co_uninitialize() {
    let mut state = apartment();
    if state.initializations == 0 {
        return;
    }
    state.initializations -= 1;
    if state.initializations == 0 && state.multithreaded {
        MULTITHREADED_THREADS.fetch_sub(1, Ordering::AcqRel);
        state.multithreaded = false;
    }
    set_apartment(state);
}

extern "efiapi" fn ole_initialize(reserved: usize) -> HResult {
    let initialized = co_initialize_ex(reserved, COINIT_APARTMENTTHREADED);
    if initialized < 0 {
        return initialized;
    }
    let mut state = apartment();
    state.ole_initializations = state.ole_initializations.saturating_add(1);
    set_apartment(state);
    initialized
}

extern "efiapi" fn ole_uninitialize() {
    let mut state = apartment();
    if state.ole_initializations == 0 {
        return;
    }
    state.ole_initializations -= 1;
    set_apartment(state);
    co_uninitialize();
}

extern "efiapi" fn ole_flush_clipboard() -> HResult {
    S_OK
}

extern "efiapi" fn co_task_mem_alloc(size: usize) -> *mut u8 {
    heap::allocate_with(size, false)
}

unsafe extern "efiapi" fn co_task_mem_realloc(block: *mut u8, size: usize) -> *mut u8 {
    if block.is_null() {
        return heap::allocate_with(size, false);
    }
    if size == 0 {
        // SAFETY: `CoTaskMemRealloc`'s contract: a block the task allocator handed out.
        unsafe { heap::free(block) };
        return ptr::null_mut();
    }
    // SAFETY: as above.
    unsafe { heap::heap_realloc(0, 0, block, size) }
}

unsafe extern "efiapi" fn co_task_mem_free(block: *mut u8) {
    // SAFETY: `CoTaskMemFree`'s contract: null or a block the task allocator handed out.
    unsafe { heap::free(block) }
}

unsafe extern "efiapi" fn allocator_query_interface(
    this: usize,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: `QueryInterface`'s contract: null or a GUID.
    let wanted = unsafe { Guid::read(interface) };
    let found = wanted.is_some_and(|wanted| wanted == guid::UNKNOWN || wanted == IID_IMALLOC);
    // SAFETY: as above: a writable pointer.
    unsafe { result.write_unaligned(if found { this } else { 0 }) };
    if found { S_OK } else { E_NOINTERFACE }
}

extern "efiapi" fn allocator_add_reference(_this: usize) -> u32 {
    1
}

extern "efiapi" fn allocator_release(_this: usize) -> u32 {
    1
}

extern "efiapi" fn allocator_alloc(_this: usize, size: usize) -> *mut u8 {
    co_task_mem_alloc(size)
}

unsafe extern "efiapi" fn allocator_realloc(_this: usize, block: *mut u8, size: usize) -> *mut u8 {
    // SAFETY: `IMalloc::Realloc`'s contract, as `CoTaskMemRealloc`'s.
    unsafe { co_task_mem_realloc(block, size) }
}

unsafe extern "efiapi" fn allocator_free(_this: usize, block: *mut u8) {
    // SAFETY: `IMalloc::Free`'s contract, as `CoTaskMemFree`'s.
    unsafe { co_task_mem_free(block) }
}

unsafe extern "efiapi" fn allocator_get_size(_this: usize, block: *mut u8) -> usize {
    if block.is_null() {
        return usize::MAX;
    }
    // SAFETY: `IMalloc::GetSize`'s contract: a block the allocator handed out.
    unsafe { heap::heap_size(0, 0, block) }
}

unsafe extern "efiapi" fn allocator_did_alloc(_this: usize, block: *mut u8) -> i32 {
    if block.is_null() {
        return -1;
    }
    // SAFETY: `IMalloc::DidAlloc`'s contract: a block some allocator handed out; the heap
    // checks its header.
    let size = unsafe { heap::heap_size(0, 0, block) };
    i32::from(size != usize::MAX)
}

extern "efiapi" fn allocator_heap_minimize(_this: usize) {}

unsafe extern "efiapi" fn co_get_malloc(context: u32, result: *mut usize) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    let allocator = match context {
        MEMCTX_TASK => &raw const TASK_ALLOCATOR as usize,
        _ => 0,
    };
    // SAFETY: `CoGetMalloc`'s contract: a writable pointer.
    unsafe { result.write_unaligned(allocator) };
    if allocator == 0 { E_INVALIDARG } else { S_OK }
}

fn class_of_program_id(name: &[u16]) -> Option<Guid> {
    let wanted = dictionary::PROGRAM_ID;
    let matches = name.len() == wanted.len()
        && name
            .iter()
            .zip(wanted)
            .all(|(&unit, &byte)| unit < 0x80 && (unit as u8).eq_ignore_ascii_case(&byte));
    matches.then_some(dictionary::CLASS)
}

unsafe fn answer_class(result: *mut Guid, class: Option<Guid>) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(class.unwrap_or(guid::NULL)) };
    match class {
        Some(_) => S_OK,
        None => CO_E_CLASSSTRING,
    }
}

unsafe extern "efiapi" fn clsid_from_prog_id(name: *const u16, result: *mut Guid) -> HResult {
    if name.is_null() || result.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `CLSIDFromProgID`'s contract: a NUL-terminated name.
    let name = unsafe { wide_text(name) };
    // SAFETY: as above: a writable CLSID.
    unsafe { answer_class(result, class_of_program_id(name)) }
}

unsafe extern "efiapi" fn clsid_from_string(text: *const u16, result: *mut Guid) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    if text.is_null() {
        // SAFETY: `CLSIDFromString`'s contract: a writable CLSID.
        return unsafe { answer_class(result, Some(guid::NULL)) };
    }
    // SAFETY: as above: a NUL-terminated string.
    let text = unsafe { wide_text(text) };
    let class = match text.first() {
        Some(&brace) if brace == u16::from(b'{') => Guid::parse(text),
        Some(_) => class_of_program_id(text),
        None => None,
    };
    // SAFETY: as above.
    unsafe { answer_class(result, class) }
}

unsafe extern "efiapi" fn string_from_guid2(
    guid: *const Guid,
    text: *mut u16,
    capacity: i32,
) -> i32 {
    // SAFETY: `StringFromGUID2`'s contract: a GUID.
    let Some(value) = (unsafe { Guid::read(guid) }) else {
        return 0;
    };
    if text.is_null() || capacity < (TEXT_LENGTH + 1) as i32 {
        return 0;
    }
    let units = value.text();
    let bytes = units.as_ptr().cast::<u8>();
    // SAFETY: the caller's buffer holds `capacity` >= 39 units.
    unsafe { ptr::copy_nonoverlapping(bytes, text.cast::<u8>(), TEXT_LENGTH * 2) };
    // SAFETY: as above: the NUL's place.
    unsafe { text.wrapping_add(TEXT_LENGTH).write_unaligned(0) };
    (TEXT_LENGTH + 1) as i32
}

fn dictionary_tables() -> dictionary::Tables {
    dictionary::Tables {
        dictionary: DICTIONARY.address(),
        enumerator: ENUMERATOR.address(),
    }
}

unsafe extern "efiapi" fn co_create_instance(
    class: *const Guid,
    outer: usize,
    context: u32,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: `CoCreateInstance`'s contract: a writable pointer.
    unsafe { result.write_unaligned(0) };
    // SAFETY: as above: null or a CLSID.
    let Some(class) = (unsafe { Guid::read(class) }) else {
        return E_INVALIDARG;
    };
    if interface.is_null() {
        return E_INVALIDARG;
    }
    if !com_is_initialized() {
        return CO_E_NOTINITIALIZED;
    }
    if class != dictionary::CLASS || context & CLSCTX_INPROC_SERVER == 0 {
        return REGDB_E_CLASSNOTREG;
    }
    if outer != 0 {
        return CLASS_E_NOAGGREGATION;
    }
    let object = dictionary::create(dictionary_tables());
    if object.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a live dictionary; the interface and result are the caller's.
    let answered = unsafe { dictionary::query_interface(object, interface, result) };
    // SAFETY: the creation's reference goes; the caller's, if any, keeps it.
    unsafe { dictionary::release(object) };
    answered
}

unsafe extern "efiapi" fn co_get_object(
    name: *const u16,
    _bind_options: usize,
    _interface: *const Guid,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: `CoGetObject`'s contract: a writable pointer.
    unsafe { result.write_unaligned(0) };
    match name.is_null() {
        true => E_INVALIDARG,
        false => MK_E_SYNTAX,
    }
}

unsafe extern "efiapi" fn create_stream_on_hglobal(
    block: *mut u8,
    delete_on_release: i32,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `CreateStreamOnHGlobal`'s contract: a writable pointer.
    unsafe { result.write_unaligned(0) };
    let (memory, size, owns) = if block.is_null() {
        (heap::allocate(0), 0, true)
    } else {
        // SAFETY: as above: an `HGLOBAL`, which is a block of the layer's heap.
        let size = unsafe { heap::heap_size(0, 0, block) };
        (block, size, delete_on_release != 0)
    };
    if memory.is_null() {
        return E_OUTOFMEMORY;
    }
    if size == usize::MAX {
        return E_INVALIDARG;
    }
    let stream = memory_stream::create(STREAM.address(), memory, size, owns);
    if stream.is_null() {
        if block.is_null() {
            // SAFETY: the block is this call's, unused.
            unsafe { heap::free(memory) };
        }
        return E_OUTOFMEMORY;
    }
    // SAFETY: as above.
    unsafe { result.write_unaligned(stream as usize) };
    S_OK
}

pub(super) const OLE32_EXPORTS: &[Export] = exports![
    b"CoInitialize" => co_initialize,
    b"CoInitializeEx" => co_initialize_ex,
    b"CoUninitialize" => co_uninitialize,
    b"OleInitialize" => ole_initialize,
    b"OleUninitialize" => ole_uninitialize,
    b"OleFlushClipboard" => ole_flush_clipboard,
    b"CoTaskMemAlloc" => co_task_mem_alloc,
    b"CoTaskMemRealloc" => co_task_mem_realloc,
    b"CoTaskMemFree" => co_task_mem_free,
    b"CoGetMalloc" => co_get_malloc,
    b"CLSIDFromString" => clsid_from_string,
    b"CLSIDFromProgID" => clsid_from_prog_id,
    b"StringFromGUID2" => string_from_guid2,
    b"CoCreateInstance" => co_create_instance,
    b"CoGetObject" => co_get_object,
    b"CreateStreamOnHGlobal" => create_stream_on_hglobal,
];

//! Type libraries for programs (D745): `LoadTypeLib`, `LoadTypeLibEx`, `LoadRegTypeLib`,
//! `QueryPathOfRegTypeLib`, `RegisterTypeLib` and `UnRegisterTypeLib` over the reader in
//! `crate::com::type_library`, from `.tlb` files or the program's own `TYPELIB` resource, and
//! the registry's `HKEY_CLASSES_ROOT\TypeLib` and `Interface` keys. `ITypeLib` lists the
//! types, finds them by id or name and documents them; `ITypeInfo` gives a type's attributes,
//! functions (`FUNCDESC`), names, implemented types and documentation, maps names to member
//! ids, creates a coclass's objects, and `Invoke`s a member through the object's vtable as
//! `IDispatch` does for a dual interface. `DispGetIDsOfNames`, `DispInvoke` and
//! `CreateStdDispatch` build on it. Variables, type comparers, DLL entries and arguments of
//! floating-point kinds passed by value (which travel in vector registers) are not served.

use super::super::icon;
use super::super::registry;
use super::super::resource::{self, Name};
use crate::com::conversion;
use crate::com::guid::{self, Guid};
use crate::com::type_library::{self, Function, Library as Parsed, TypeDesc, type_kind};
use crate::com::variant::{self, Variant, kind};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK, bstr};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const TYPE_E_CANTLOADLIBRARY: HResult = 0x8002_9c4a_u32 as i32;
const TYPE_E_ELEMENTNOTFOUND: HResult = 0x8002_802b_u32 as i32;
const TYPE_E_LIBNOTREGISTERED: HResult = 0x8002_801d_u32 as i32;
const TYPE_E_REGISTRYACCESS: HResult = 0x8002_8018_u32 as i32;
const TYPE_E_BADMODULEKIND: HResult = 0x8002_80bd_u32 as i32;
const DISP_E_MEMBERNOTFOUND: HResult = 0x8002_0003_u32 as i32;
const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
const DISP_E_PARAMNOTFOUND: HResult = 0x8002_0004_u32 as i32;
const DISP_E_PARAMNOTOPTIONAL: HResult = 0x8002_000f_u32 as i32;
const DISP_E_BADPARAMCOUNT: HResult = 0x8002_000e_u32 as i32;
const DISP_E_EXCEPTION: HResult = 0x8002_0009_u32 as i32;
const DISP_E_UNKNOWNNAME: HResult = 0x8002_0006_u32 as i32;
const VT_HRESULT: u16 = 25;
const VT_VOID: u16 = 24;
const MEMBER_NIL: i32 = -1;
const CLSCTX_SERVER: u32 = 0x15;
const REGKIND_REGISTER: u32 = 1;
const PARAMFLAG_FLCID: u32 = 0x4;
const PARAMFLAG_FRETVAL: u32 = 0x8;
const PARAMFLAG_FOPT: u32 = 0x10;
const MOST_ARGUMENTS: usize = 8;
const TYPE_ATTRIBUTE_BYTES: usize = 96;
const LIBRARY_ATTRIBUTE_BYTES: usize = 32;
const FUNCTION_DESC_BYTES: usize = 88;
const ELEMENT_DESC_BYTES: usize = 32;
const TYPE_DESC_BYTES: usize = 16;
const POINTER_BYTES: u16 = 8;
const EXCEPTION_STATUS: usize = 56;
const LARGEST_RESULT: usize = 8;
const UNIVERSAL_MARSHALER: &str = "{00020424-0000-0000-C000-000000000046}";
const TYPE_LIBRARY_KEY: &str = "HKEY_CLASSES_ROOT\\TypeLib\\";
const INTERFACE_KEY: &str = "HKEY_CLASSES_ROOT\\Interface\\";
const TYPE_LIBRARY: Guid = Guid::new(0x0002_0402, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const TYPE_INFO: Guid = Guid::new(0x0002_0401, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);

#[repr(C)]
#[derive(Debug)]
struct Library {
    table: usize,
    references: AtomicU32,
    bytes: Vec<u8>,
    path: Vec<u16>,
}

#[repr(C)]
#[derive(Debug)]
struct Info {
    table: usize,
    references: AtomicU32,
    library: *mut Library,
    index: usize,
}

#[repr(C)]
#[derive(Debug)]
struct StandardDispatch {
    table: usize,
    references: AtomicU32,
    object: usize,
    info: usize,
}

unsafe fn library<'a>(this: *mut Library) -> &'a Library {
    // SAFETY: the caller's duty: a live library object.
    unsafe { &*this }
}

unsafe fn info<'a>(this: *mut Info) -> &'a Info {
    // SAFETY: the caller's duty: a live type information object.
    unsafe { &*this }
}

fn parsed(found: &Library) -> Option<Parsed<'_>> {
    Parsed::parse(&found.bytes)
}

unsafe fn write_out<T>(out: *mut T, value: T) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty: a writable pointer from the program.
    unsafe { out.write_unaligned(value) };
    S_OK
}

fn ascii_bstr(bytes: &[u8]) -> *mut u16 {
    bstr::from_ascii(bytes)
}

unsafe fn write_text(out: *mut *mut u16, bytes: Option<&[u8]>) {
    if !out.is_null() {
        let made = bytes.map_or(core::ptr::null_mut(), ascii_bstr);
        // SAFETY: the caller's duty: a writable BSTR pointer.
        unsafe { out.write_unaligned(made) };
    }
}

fn lowered(unit: u16) -> u16 {
    match unit {
        0x41..=0x5a => unit + 0x20,
        other => other,
    }
}

fn same_name(stored: &[u8], wanted: &[u16]) -> bool {
    stored.len() == wanted.len()
        && stored
            .iter()
            .zip(wanted)
            .all(|(&byte, &unit)| lowered(u16::from(byte)) == lowered(unit))
}

fn new_library(bytes: Vec<u8>, path: Vec<u16>) -> Result<*mut Library, HResult> {
    if Parsed::parse(&bytes).is_none() {
        return Err(TYPE_E_CANTLOADLIBRARY);
    }
    let object = crate::com::allocate(core::mem::size_of::<Library>()).cast::<Library>();
    if object.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: a fresh block, sized and aligned for a library.
    unsafe {
        object.write(Library {
            table: super::TYPE_LIB.address(),
            references: AtomicU32::new(1),
            bytes,
            path,
        })
    };
    Ok(object)
}

fn new_info(library: *mut Library, index: usize) -> Result<*mut Info, HResult> {
    let object = crate::com::allocate(core::mem::size_of::<Info>()).cast::<Info>();
    if object.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: a live library gains the type's reference; a fresh block for the type.
    unsafe {
        library_add_reference(library);
        object.write(Info {
            table: super::TYPE_INFO.address(),
            references: AtomicU32::new(1),
            library,
            index,
        });
    }
    Ok(object)
}

fn bytes_of_file(path: &[u16]) -> Result<Vec<u8>, HResult> {
    let block = icon::read_file(path).map_err(|_| TYPE_E_CANTLOADLIBRARY)?;
    let bytes = block.bytes();
    if bytes.starts_with(b"MZ") {
        let kind: Vec<u16> = "TYPELIB".encode_utf16().collect();
        let (address, size) =
            resource::find(0, Name::Text(&kind), Name::Id(1)).ok_or(TYPE_E_CANTLOADLIBRARY)?;
        // SAFETY: the program's resource data, mapped for the process's life.
        let data = unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) };
        return Ok(data.to_vec());
    }
    Ok(bytes.to_vec())
}

fn load(path: &[u16]) -> Result<*mut Library, HResult> {
    let bytes = bytes_of_file(path)?;
    new_library(bytes, path.to_vec())
}

fn hexadecimal(value: u32) -> alloc::string::String {
    let mut text = alloc::string::String::new();
    let digits = b"0123456789abcdef";
    let mut started = false;
    for shift in (0..8).rev() {
        let digit = (value >> (shift * 4)) & 0xf;
        if digit != 0 || started || shift == 0 {
            started = true;
            text.push(char::from(digits[digit as usize]));
        }
    }
    text
}

fn guid_text(id: &Guid) -> alloc::string::String {
    id.text()
        .iter()
        .filter_map(|&unit| char::from_u32(u32::from(unit)))
        .collect()
}

fn version_key(id: &Guid, (major, minor): (u16, u16)) -> alloc::string::String {
    let mut key = alloc::string::String::from(TYPE_LIBRARY_KEY);
    key.push_str(&guid_text(id));
    key.push('\\');
    key.push_str(&hexadecimal(u32::from(major)));
    key.push('.');
    key.push_str(&hexadecimal(u32::from(minor)));
    key
}

fn registered_path(id: &Guid, version: (u16, u16), locale: u32) -> Option<Vec<u16>> {
    let base = version_key(id, version);
    for locale in [locale, locale & 0x3ff, 0, 9] {
        for platform in ["win64", "win32"] {
            let mut key = base.clone();
            key.push('\\');
            key.push_str(&hexadecimal(locale));
            key.push('\\');
            key.push_str(platform);
            if let Some(path) = registry::string_value(&key, &[]) {
                return Some(path);
            }
        }
    }
    None
}

fn register(found: &Library, path: &[u16]) -> HResult {
    let Some(parsed) = parsed(found) else {
        return TYPE_E_CANTLOADLIBRARY;
    };
    let Some(id) = parsed.guid().map(Guid::from_bytes) else {
        return TYPE_E_REGISTRYACCESS;
    };
    let (major, minor) = parsed.version();
    let base = version_key(&id, (major, minor));
    let name: Vec<u16> = parsed.name().unwrap_or(b"").iter().map(|&byte| byte.into()).collect();
    let mut file_key = base.clone();
    file_key.push('\\');
    file_key.push_str(&hexadecimal(parsed.locale()));
    file_key.push_str("\\win64");
    let mut flags_key = base.clone();
    flags_key.push_str("\\FLAGS");
    let flags: Vec<u16> = hexadecimal(parsed.flags()).encode_utf16().collect();
    let written = registry::set_string_value(&base, &[], &name)
        && registry::set_string_value(&file_key, &[], path)
        && registry::set_string_value(&flags_key, &[], &flags);
    if !written {
        return TYPE_E_REGISTRYACCESS;
    }
    let library_text: Vec<u16> = guid_text(&id).encode_utf16().collect();
    let version: Vec<u16> = {
        let mut text = hexadecimal(u32::from(major));
        text.push('.');
        text.push_str(&hexadecimal(u32::from(minor)));
        text.encode_utf16().collect()
    };
    let version_name: Vec<u16> = "Version".encode_utf16().collect();
    let marshaler: Vec<u16> = UNIVERSAL_MARSHALER.encode_utf16().collect();
    for index in 0..parsed.type_count() {
        let Some(entry) = parsed.type_info(index) else {
            continue;
        };
        if !matches!(entry.kind(), type_kind::INTERFACE | type_kind::DISPATCH) {
            continue;
        }
        let Some(interface) = entry.guid().map(Guid::from_bytes) else {
            continue;
        };
        let mut key = alloc::string::String::from(INTERFACE_KEY);
        key.push_str(&guid_text(&interface));
        let type_name: Vec<u16> =
            entry.name().unwrap_or(b"").iter().map(|&byte| byte.into()).collect();
        let mut library_key = key.clone();
        library_key.push_str("\\TypeLib");
        let mut stub_key = key.clone();
        stub_key.push_str("\\ProxyStubClsid32");
        let _ = registry::set_string_value(&key, &[], &type_name)
            && registry::set_string_value(&library_key, &[], &library_text)
            && registry::set_string_value(&library_key, &version_name, &version)
            && registry::set_string_value(&stub_key, &[], &marshaler);
    }
    S_OK
}

unsafe fn hand_out_library(made: Result<*mut Library, HResult>, out: *mut usize) -> HResult {
    if out.is_null() {
        if let Ok(made) = made {
            // SAFETY: the library just made, whose one reference goes.
            unsafe { library_release(made) };
        }
        return E_POINTER;
    }
    // SAFETY: the caller's writable interface pointer.
    unsafe { out.write_unaligned(made.map_or(0, |made| made as usize)) };
    made.map_or_else(|status| status, |_| S_OK)
}

/// `LoadTypeLibEx`.
///
/// # Safety
/// `path` must be a NUL-terminated name and `out` writable.
pub(super) unsafe extern "efiapi" fn load_type_lib_ex(
    path: *const u16,
    how: u32,
    out: *mut usize,
) -> HResult {
    if path.is_null() {
        // SAFETY: the caller's interface pointer.
        return unsafe { hand_out_library(Err(E_INVALIDARG), out) };
    }
    // SAFETY: the caller's duty.
    let path = unsafe { crate::com::wide_text(path) };
    let made = load(path);
    if let (Ok(found), REGKIND_REGISTER) = (made, how) {
        // SAFETY: the library just made.
        let _ = register(unsafe { library(found) }, path);
    }
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_library(made, out) }
}

/// `LoadTypeLib`.
///
/// # Safety
/// As `load_type_lib_ex`.
pub(super) unsafe extern "efiapi" fn load_type_lib(path: *const u16, out: *mut usize) -> HResult {
    // SAFETY: the caller's duty.
    unsafe { load_type_lib_ex(path, 0, out) }
}

/// `LoadRegTypeLib`.
///
/// # Safety
/// `id` must be a GUID and `out` writable.
pub(super) unsafe extern "efiapi" fn load_reg_type_lib(
    id: *const Guid,
    major: u16,
    minor: u16,
    locale: u32,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let Some(id) = (unsafe { Guid::read(id) }) else {
        // SAFETY: the caller's interface pointer.
        return unsafe { hand_out_library(Err(E_INVALIDARG), out) };
    };
    let made = registered_path(&id, (major, minor), locale)
        .ok_or(TYPE_E_LIBNOTREGISTERED)
        .and_then(|path| load(&path));
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_library(made, out) }
}

/// `QueryPathOfRegTypeLib`.
///
/// # Safety
/// `id` must be a GUID and `out` writable.
pub(super) unsafe extern "efiapi" fn query_path_of_reg_type_lib(
    id: *const Guid,
    major: u16,
    minor: u16,
    locale: u32,
    out: *mut *mut u16,
) -> HResult {
    // SAFETY: the caller's duty.
    let Some(id) = (unsafe { Guid::read(id) }) else {
        return E_INVALIDARG;
    };
    let Some(path) = registered_path(&id, (major, minor), locale) else {
        return TYPE_E_LIBNOTREGISTERED;
    };
    let made = bstr::from_units(&path);
    // SAFETY: the caller's writable BSTR pointer.
    let status = unsafe { write_out(out, made) };
    match made.is_null() {
        true => E_OUTOFMEMORY,
        false => status,
    }
}

/// `RegisterTypeLib`.
///
/// # Safety
/// `object` must be a live `ITypeLib` and `path` a NUL-terminated name.
pub(super) unsafe extern "efiapi" fn register_type_lib(
    object: usize,
    path: *const u16,
    _help: *const u16,
) -> HResult {
    if object == 0 || path.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty: a COM object's first word is its table.
    if unsafe { (object as *const usize).read() } != super::TYPE_LIB.address() {
        return E_NOTIMPL;
    }
    // SAFETY: the layer's own library, as its table says; the caller's path.
    unsafe { register(library(object as *mut Library), crate::com::wide_text(path)) }
}

/// `UnRegisterTypeLib`: the registry keeps what was written (the layer's registry has no
/// tree removal here), so it answers success.
pub(super) extern "efiapi" fn un_register_type_lib() -> HResult {
    S_OK
}

pub(super) unsafe extern "efiapi" fn library_query_interface(
    this: *mut Library,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &library(this).references };
    let interfaces = [guid::UNKNOWN, TYPE_LIBRARY];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

pub(super) unsafe extern "efiapi" fn library_add_reference(this: *mut Library) -> u32 {
    // SAFETY: the caller's duty.
    unsafe { library(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) unsafe extern "efiapi" fn library_release(this: *mut Library) -> u32 {
    // SAFETY: the caller's duty.
    let found = unsafe { library(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if last {
        // SAFETY: the library's fields and block; no reference is left.
        unsafe {
            core::ptr::drop_in_place(this);
            crate::com::free(this.cast());
        }
    }
    left
}

pub(super) unsafe extern "efiapi" fn library_type_count(this: *mut Library) -> u32 {
    // SAFETY: the caller's duty.
    parsed(unsafe { library(this) }).map_or(0, |parsed| parsed.type_count() as u32)
}

unsafe fn hand_out_info(made: Result<*mut Info, HResult>, out: *mut usize) -> HResult {
    if out.is_null() {
        if let Ok(made) = made {
            // SAFETY: the type just made, whose one reference goes.
            unsafe { info_release(made) };
        }
        return E_POINTER;
    }
    // SAFETY: the caller's writable interface pointer.
    unsafe { out.write_unaligned(made.map_or(0, |made| made as usize)) };
    made.map_or_else(|status| status, |_| S_OK)
}

pub(super) unsafe extern "efiapi" fn library_type_info(
    this: *mut Library,
    index: u32,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let count = unsafe { library_type_count(this) };
    let made = match index < count {
        true => new_info(this, index as usize),
        false => Err(TYPE_E_ELEMENTNOTFOUND),
    };
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_info(made, out) }
}

pub(super) unsafe extern "efiapi" fn library_type_kind(
    this: *mut Library,
    index: u32,
    out: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = parsed(unsafe { library(this) }).and_then(|parsed| {
        parsed.type_info(index as usize).map(|entry| entry.kind())
    });
    match found {
        // SAFETY: the caller's writable kind.
        Some(found) => unsafe { write_out(out, found) },
        None => TYPE_E_ELEMENTNOTFOUND,
    }
}

pub(super) unsafe extern "efiapi" fn library_type_of_guid(
    this: *mut Library,
    id: *const Guid,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let wanted = unsafe { Guid::read(id) }.map(|id| id.bytes());
    // SAFETY: as above.
    let index = wanted.and_then(|wanted| parsed(unsafe { library(this) })?.find(&wanted));
    let made = index.ok_or(TYPE_E_ELEMENTNOTFOUND).and_then(|index| new_info(this, index));
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_info(made, out) }
}

pub(super) unsafe extern "efiapi" fn library_attributes(
    this: *mut Library,
    out: *mut *mut u8,
) -> HResult {
    // SAFETY: the caller's duty.
    let Some(parsed) = parsed(unsafe { library(this) }) else {
        return TYPE_E_CANTLOADLIBRARY;
    };
    let block = crate::com::allocate(LIBRARY_ATTRIBUTE_BYTES);
    if block.is_null() {
        return E_OUTOFMEMORY;
    }
    let (major, minor) = parsed.version();
    // SAFETY: a fresh block of `TLIBATTR`'s 32 bytes.
    unsafe {
        block.copy_from_nonoverlapping(parsed.guid().unwrap_or([0; 16]).as_ptr(), 16);
        block.add(16).cast::<u32>().write_unaligned(parsed.locale());
        block.add(20).cast::<u32>().write_unaligned(parsed.system());
        block.add(24).cast::<u16>().write_unaligned(major);
        block.add(26).cast::<u16>().write_unaligned(minor);
        block.add(28).cast::<u16>().write_unaligned(parsed.flags() as u16);
    }
    // SAFETY: the caller's writable pointer.
    unsafe { write_out(out, block) }
}

pub(super) unsafe extern "efiapi" fn release_block(_this: usize, block: *mut u8) {
    // SAFETY: a block the layer allocated for the caller, or null.
    unsafe { crate::com::free(block) };
}

pub(super) unsafe extern "efiapi" fn not_implemented(_this: usize, out: *mut usize) -> HResult {
    if !out.is_null() {
        // SAFETY: the caller's writable pointer.
        unsafe { out.write_unaligned(0) };
    }
    E_NOTIMPL
}

pub(super) unsafe extern "efiapi" fn library_documentation(
    this: *mut Library,
    index: i32,
    name: *mut *mut u16,
    documentation: *mut *mut u16,
    context: *mut u32,
    help_file: *mut *mut u16,
) -> HResult {
    // SAFETY: the caller's duty.
    let Some(parsed) = parsed(unsafe { library(this) }) else {
        return TYPE_E_CANTLOADLIBRARY;
    };
    let (found_name, found_documentation) = match index {
        -1 => (parsed.name(), None),
        _ => match parsed.type_info(index as usize) {
            Some(entry) => (entry.name(), entry.documentation()),
            None => return TYPE_E_ELEMENTNOTFOUND,
        },
    };
    // SAFETY: the caller's pointers, each writable or null.
    unsafe {
        write_text(name, found_name);
        write_text(documentation, found_documentation);
        write_text(help_file, None);
        if !context.is_null() {
            context.write_unaligned(0);
        }
    }
    S_OK
}

fn find_named(parsed: &Parsed<'_>, wanted: &[u16]) -> Option<(usize, i32)> {
    (0..parsed.type_count()).find_map(|index| {
        let entry = parsed.type_info(index)?;
        if entry.name().is_some_and(|name| same_name(name, wanted)) {
            return Some((index, MEMBER_NIL));
        }
        (0..entry.function_count()).find_map(|at| {
            let function = entry.function(at)?;
            let name = parsed.name_at(function.name)?;
            same_name(name, wanted).then_some((index, function.id))
        })
    })
}

pub(super) unsafe extern "efiapi" fn library_is_name(
    this: *mut Library,
    name: *mut u16,
    _hash: u32,
    found: *mut i32,
) -> HResult {
    if name.is_null() || found.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let wanted = unsafe { crate::com::wide_text(name) };
    // SAFETY: as above.
    let hit = parsed(unsafe { library(this) }).and_then(|parsed| find_named(&parsed, wanted));
    // SAFETY: the caller's writable flag.
    unsafe { found.write_unaligned(i32::from(hit.is_some())) };
    S_OK
}

pub(super) unsafe extern "efiapi" fn library_find_name(
    this: *mut Library,
    name: *mut u16,
    _hash: u32,
    infos: *mut usize,
    ids: *mut i32,
    count: *mut u16,
) -> HResult {
    if name.is_null() || infos.is_null() || ids.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's duty.
    let wanted = unsafe { crate::com::wide_text(name) };
    // SAFETY: as above.
    let hit = parsed(unsafe { library(this) }).and_then(|parsed| find_named(&parsed, wanted));
    // SAFETY: the caller's count, at least one entry asked for when not zero.
    let room = unsafe { count.read_unaligned() };
    let Some((index, id)) = hit.filter(|_| room > 0) else {
        // SAFETY: as above.
        unsafe { count.write_unaligned(0) };
        return S_OK;
    };
    match new_info(this, index) {
        Ok(made) => {
            // SAFETY: the caller's arrays of `room` entries and its count.
            unsafe {
                infos.write_unaligned(made as usize);
                ids.write_unaligned(id);
                count.write_unaligned(1);
            }
            S_OK
        }
        Err(status) => status,
    }
}

pub(super) unsafe extern "efiapi" fn info_query_interface(
    this: *mut Info,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &info(this).references };
    let interfaces = [guid::UNKNOWN, TYPE_INFO];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

pub(super) unsafe extern "efiapi" fn info_add_reference(this: *mut Info) -> u32 {
    // SAFETY: the caller's duty.
    unsafe { info(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) unsafe extern "efiapi" fn info_release(this: *mut Info) -> u32 {
    // SAFETY: the caller's duty.
    let found = unsafe { info(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if last {
        // SAFETY: the type's reference to its library, then its block.
        unsafe {
            library_release(found.library);
            crate::com::free(this.cast());
        }
    }
    left
}

fn with_entry<R>(
    this: *mut Info,
    work: impl FnOnce(&Parsed<'_>, type_library::TypeInfo<'_, '_>) -> Result<R, HResult>,
) -> Result<R, HResult> {
    // SAFETY: the callers pass a live type, whose library stays alive with it.
    let found = unsafe { info(this) };
    // SAFETY: as above.
    let parsed = parsed(unsafe { library(found.library) }).ok_or(TYPE_E_CANTLOADLIBRARY)?;
    let entry = parsed.type_info(found.index).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
    work(&parsed, entry)
}

pub(super) unsafe extern "efiapi" fn info_type_attributes(
    this: *mut Info,
    out: *mut *mut u8,
) -> HResult {
    let made = with_entry(this, |parsed, entry| {
        let block = crate::com::allocate(TYPE_ATTRIBUTE_BYTES);
        if block.is_null() {
            return Err(E_OUTOFMEMORY);
        }
        let (major, minor) = entry.version();
        // SAFETY: a fresh block of `TYPEATTR`'s 96 bytes.
        unsafe {
            block.copy_from_nonoverlapping(entry.guid().unwrap_or([0; 16]).as_ptr(), 16);
            block.add(16).cast::<u32>().write_unaligned(parsed.locale());
            block.add(24).cast::<i32>().write_unaligned(MEMBER_NIL);
            block.add(28).cast::<i32>().write_unaligned(MEMBER_NIL);
            block.add(40).cast::<u32>().write_unaligned(entry.instance_size());
            block.add(44).cast::<u32>().write_unaligned(entry.kind());
            block.add(48).cast::<u16>().write_unaligned(entry.function_count() as u16);
            block.add(50).cast::<u16>().write_unaligned(entry.variable_count() as u16);
            block.add(52).cast::<u16>().write_unaligned(entry.implemented_count() as u16);
            let vtable = entry.vtable_slots() as u16 * POINTER_BYTES;
            block.add(54).cast::<u16>().write_unaligned(vtable);
            block.add(56).cast::<u16>().write_unaligned(entry.alignment() as u16);
            block.add(58).cast::<u16>().write_unaligned(entry.flags() as u16);
            block.add(60).cast::<u16>().write_unaligned(major);
            block.add(62).cast::<u16>().write_unaligned(minor);
        }
        Ok(block)
    });
    match made {
        // SAFETY: the caller's writable pointer.
        Ok(block) => unsafe { write_out(out, block) },
        Err(status) => status,
    }
}

fn pointer_depth(desc: &TypeDesc) -> usize {
    match desc {
        TypeDesc::Pointer(inner) | TypeDesc::SafeArray(inner) => 1 + pointer_depth(inner),
        _ => 0,
    }
}

struct Pool {
    next: *mut u8,
}

impl Pool {
    fn write(&mut self, at: *mut u8, desc: &TypeDesc) {
        // SAFETY: `at` is a `TYPEDESC` of the description's block, sized for every nesting.
        unsafe {
            at.add(8).cast::<u16>().write_unaligned(desc.kind());
            match desc {
                TypeDesc::Pointer(inner) | TypeDesc::SafeArray(inner) => {
                    let nested = self.next;
                    self.next = self.next.add(TYPE_DESC_BYTES);
                    at.cast::<usize>().write_unaligned(nested as usize);
                    self.write(nested, inner);
                }
                TypeDesc::UserDefined(reference) => {
                    at.cast::<i32>().write_unaligned(*reference);
                }
                _ => {}
            }
        }
    }
}

fn function_block(function: &Function) -> Result<*mut u8, HResult> {
    let count = function.parameters.len();
    let nested = pointer_depth(&function.result)
        + function.parameters.iter().map(|parameter| pointer_depth(&parameter.desc)).sum::<usize>();
    let elements = FUNCTION_DESC_BYTES;
    let pool = elements + count * ELEMENT_DESC_BYTES;
    let block = crate::com::allocate(pool + nested * TYPE_DESC_BYTES);
    if block.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the fresh block holds the description, its parameters and the nested types.
    let mut writer = Pool {
        next: unsafe { block.add(pool) },
    };
    // SAFETY: as above.
    unsafe {
        block.cast::<i32>().write_unaligned(function.id);
        let parameters = match count {
            0 => 0,
            _ => block.add(elements) as usize,
        };
        block.add(16).cast::<usize>().write_unaligned(parameters);
        block.add(24).cast::<u32>().write_unaligned(function.function_kind);
        block.add(28).cast::<u32>().write_unaligned(function.invoke_kind);
        block.add(32).cast::<u32>().write_unaligned(function.call_convention);
        block.add(36).cast::<i16>().write_unaligned(count as i16);
        block.add(38).cast::<i16>().write_unaligned(function.optional as i16);
        let offset = function.slot * POINTER_BYTES;
        block.add(40).cast::<i16>().write_unaligned(offset as i16);
        writer.write(block.add(48), &function.result);
        block.add(80).cast::<u16>().write_unaligned(function.flags);
        for (index, parameter) in function.parameters.iter().enumerate() {
            let element = block.add(elements + index * ELEMENT_DESC_BYTES);
            writer.write(element, &parameter.desc);
            element.add(24).cast::<u16>().write_unaligned(parameter.flags as u16);
        }
    }
    Ok(block)
}

pub(super) unsafe extern "efiapi" fn info_function_description(
    this: *mut Info,
    index: u32,
    out: *mut *mut u8,
) -> HResult {
    let made = with_entry(this, |_, entry| {
        let function = entry.function(index as usize).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
        function_block(&function)
    });
    match made {
        // SAFETY: the caller's writable pointer.
        Ok(block) => unsafe { write_out(out, block) },
        Err(status) => status,
    }
}

fn function_with_id(entry: &type_library::TypeInfo<'_, '_>, id: i32) -> Option<Function> {
    (0..entry.function_count())
        .filter_map(|index| entry.function(index))
        .find(|function| function.id == id)
}

pub(super) unsafe extern "efiapi" fn info_names(
    this: *mut Info,
    id: i32,
    names: *mut *mut u16,
    room: u32,
    count: *mut u32,
) -> HResult {
    if names.is_null() || count.is_null() {
        return E_INVALIDARG;
    }
    let found = with_entry(this, |parsed, entry| {
        let function = function_with_id(&entry, id).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
        let mut all = Vec::new();
        all.push(parsed.name_at(function.name));
        all.extend(
            function
                .parameters
                .iter()
                .map(|parameter| parsed.name_at(parameter.name)),
        );
        let given = all.len().min(room as usize);
        for (index, name) in all.into_iter().take(given).enumerate() {
            // SAFETY: the caller's array of `room` BSTRs.
            unsafe { write_text(names.add(index), name) };
        }
        Ok(given as u32)
    });
    match found {
        // SAFETY: the caller's writable count.
        Ok(given) => unsafe { write_out(count, given) },
        Err(status) => status,
    }
}

pub(super) unsafe extern "efiapi" fn info_implemented_reference(
    this: *mut Info,
    index: u32,
    out: *mut i32,
) -> HResult {
    match with_entry(this, |_, entry| {
        entry.implemented(index as usize).ok_or(TYPE_E_ELEMENTNOTFOUND)
    }) {
        // SAFETY: the caller's writable reference.
        Ok(reference) => unsafe { write_out(out, reference) },
        Err(status) => status,
    }
}

pub(super) unsafe extern "efiapi" fn info_implemented_flags(
    this: *mut Info,
    index: u32,
    out: *mut u32,
) -> HResult {
    match with_entry(this, |_, entry| {
        entry.implemented_flags(index as usize).ok_or(TYPE_E_ELEMENTNOTFOUND)
    }) {
        // SAFETY: the caller's writable flags.
        Ok(flags) => unsafe { write_out(out, flags) },
        Err(status) => status,
    }
}

pub(super) unsafe extern "efiapi" fn info_ids_of_names(
    this: *mut Info,
    names: *const *const u16,
    count: u32,
    ids: *mut i32,
) -> HResult {
    if names.is_null() || ids.is_null() || count == 0 {
        return E_INVALIDARG;
    }
    let found = with_entry(this, |parsed, entry| {
        // SAFETY: the caller's `count` names, each NUL-terminated.
        let wanted = unsafe { crate::com::wide_text(names.read()) };
        let function = (0..entry.function_count())
            .filter_map(|index| entry.function(index))
            .find(|function| {
                parsed
                    .name_at(function.name)
                    .is_some_and(|name| same_name(name, wanted))
            })
            .ok_or(DISP_E_UNKNOWNNAME)?;
        let mut status = S_OK;
        // SAFETY: the caller's room for `count` ids.
        unsafe { ids.write_unaligned(function.id) };
        for at in 1..count as usize {
            // SAFETY: as above.
            let name = unsafe { crate::com::wide_text(names.add(at).read()) };
            let position = function.parameters.iter().position(|parameter| {
                parsed
                    .name_at(parameter.name)
                    .is_some_and(|stored| same_name(stored, name))
            });
            let id = position.map_or(MEMBER_NIL, |position| position as i32);
            if position.is_none() {
                status = DISP_E_UNKNOWNNAME;
            }
            // SAFETY: as above.
            unsafe { ids.add(at).write_unaligned(id) };
        }
        Ok(status)
    });
    found.unwrap_or_else(|status| status)
}

pub(super) unsafe extern "efiapi" fn info_documentation(
    this: *mut Info,
    id: i32,
    name: *mut *mut u16,
    documentation: *mut *mut u16,
    context: *mut u32,
    help_file: *mut *mut u16,
) -> HResult {
    let found = with_entry(this, |parsed, entry| match id {
        MEMBER_NIL => Ok((entry.name(), entry.documentation())),
        _ => {
            let function = function_with_id(&entry, id).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
            Ok((parsed.name_at(function.name), None))
        }
    });
    let (found_name, found_documentation) = match found {
        Ok(found) => found,
        Err(status) => return status,
    };
    // SAFETY: the caller's pointers, each writable or null.
    unsafe {
        write_text(name, found_name);
        write_text(documentation, found_documentation);
        write_text(help_file, None);
        if !context.is_null() {
            context.write_unaligned(0);
        }
    }
    S_OK
}

pub(super) unsafe extern "efiapi" fn info_dll_entry(_this: *mut Info) -> HResult {
    TYPE_E_BADMODULEKIND
}

pub(super) unsafe extern "efiapi" fn info_variable(
    _this: *mut Info,
    _index: u32,
    out: *mut usize,
) -> HResult {
    if !out.is_null() {
        // SAFETY: the caller's writable pointer.
        unsafe { out.write_unaligned(0) };
    }
    TYPE_E_ELEMENTNOTFOUND
}

pub(super) unsafe extern "efiapi" fn info_address_of_member(
    _this: *mut Info,
    _id: i32,
    _kind: u32,
    out: *mut usize,
) -> HResult {
    if !out.is_null() {
        // SAFETY: the caller's writable pointer.
        unsafe { out.write_unaligned(0) };
    }
    TYPE_E_BADMODULEKIND
}

pub(super) unsafe extern "efiapi" fn info_mops(
    _this: *mut Info,
    _id: i32,
    out: *mut *mut u16,
) -> HResult {
    // SAFETY: the caller's writable BSTR pointer, or null.
    unsafe { write_text(out, None) };
    S_OK
}

pub(super) unsafe extern "efiapi" fn info_reference_type(
    this: *mut Info,
    reference: i32,
    out: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let library = unsafe { info(this) }.library;
    let made = with_entry(this, |parsed, _| {
        parsed.type_of_reference(reference).ok_or(TYPE_E_ELEMENTNOTFOUND)
    })
    .and_then(|index| new_info(library, index));
    // SAFETY: the caller's interface pointer.
    unsafe { hand_out_info(made, out) }
}

pub(super) unsafe extern "efiapi" fn info_create_instance(
    this: *mut Info,
    outer: usize,
    interface: *const Guid,
    out: *mut usize,
) -> HResult {
    let class = match with_entry(this, |_, entry| match entry.kind() {
        type_kind::COCLASS => entry.guid().ok_or(TYPE_E_ELEMENTNOTFOUND),
        _ => Err(TYPE_E_BADMODULEKIND),
    }) {
        Ok(class) => Guid::from_bytes(class),
        Err(status) => return status,
    };
    // SAFETY: the caller's interface and result, passed on.
    unsafe { super::co_create_instance(&class, outer, CLSCTX_SERVER, interface, out) }
}

pub(super) unsafe extern "efiapi" fn info_containing_library(
    this: *mut Info,
    out: *mut usize,
    index: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { info(this) };
    if !index.is_null() {
        // SAFETY: the caller's writable index.
        unsafe { index.write_unaligned(found.index as u32) };
    }
    if out.is_null() {
        return S_OK;
    }
    // SAFETY: the type's live library gains the caller's reference.
    unsafe {
        library_add_reference(found.library);
        out.write_unaligned(found.library as usize);
    }
    S_OK
}

struct Cells {
    values: [Variant; MOST_ARGUMENTS],
    owned: [bool; MOST_ARGUMENTS],
}

impl Cells {
    fn clear(&mut self) {
        for (value, owned) in self.values.iter_mut().zip(self.owned) {
            if owned {
                // SAFETY: a VARIANT the call made and owns.
                unsafe { variant::clear(value) };
            }
        }
    }
}

fn user_kind(parsed: &Parsed<'_>, reference: i32) -> Option<u16> {
    let entry = parsed.type_info(parsed.type_of_reference(reference)?)?;
    match entry.kind() {
        type_kind::ENUM => Some(kind::I4),
        type_kind::DISPATCH => Some(kind::DISPATCH),
        type_kind::INTERFACE | type_kind::COCLASS => Some(kind::UNKNOWN),
        _ => None,
    }
}

fn value_kind(parsed: &Parsed<'_>, desc: &TypeDesc) -> Option<u16> {
    match desc {
        TypeDesc::Base(kind::R4 | kind::R8 | kind::DATE) => None,
        TypeDesc::Base(base) => Some(*base),
        TypeDesc::UserDefined(reference) => user_kind(parsed, *reference),
        TypeDesc::SafeArray(inner) => match **inner {
            TypeDesc::Base(element) => Some(kind::ARRAY | element),
            _ => None,
        },
        _ => None,
    }
}

fn pointee_kind(parsed: &Parsed<'_>, desc: &TypeDesc) -> Option<u16> {
    match desc {
        TypeDesc::Pointer(inner) => match &**inner {
            TypeDesc::Base(base) => Some(*base),
            TypeDesc::Pointer(innermost) => match &**innermost {
                TypeDesc::UserDefined(reference) => user_kind(parsed, *reference),
                _ => None,
            },
            other => value_kind(parsed, other),
        },
        _ => None,
    }
}

fn place_of(base: u16, cell: &mut Variant) -> usize {
    match base {
        kind::VARIANT | kind::DECIMAL => cell as *mut Variant as usize,
        _ => cell.data.as_mut_ptr() as usize,
    }
}

unsafe fn argument_word(
    parsed: &Parsed<'_>,
    desc: &TypeDesc,
    given: Option<&Variant>,
    cell: (&mut Variant, &mut bool),
) -> Result<usize, HResult> {
    let (cell, owned) = cell;
    if let Some(base) = pointee_kind(parsed, desc) {
        let given = given.ok_or(DISP_E_PARAMNOTOPTIONAL)?;
        if given.kind == base | kind::BYREF {
            return Ok(given.pointer());
        }
        // SAFETY: the caller's VARIANT, valid for the call.
        let viewed = unsafe { variant::view(given) }?;
        *cell = match base {
            kind::VARIANT => {
                // SAFETY: as above.
                unsafe { variant::duplicate(&viewed) }?
            }
            // SAFETY: as above.
            _ => unsafe { conversion::change(&viewed, 0, base) }?,
        };
        *owned = true;
        return Ok(place_of(base, cell));
    }
    if desc == &TypeDesc::Base(kind::VARIANT) {
        *cell = match given {
            // SAFETY: the caller's VARIANT, valid for the call.
            Some(given) => unsafe { variant::duplicate(&variant::view(given)?) }?,
            None => Variant::new(kind::ERROR, DISP_E_PARAMNOTFOUND as u32 as u64),
        };
        *owned = true;
        return Ok(cell as *mut Variant as usize);
    }
    let base = value_kind(parsed, desc).ok_or(DISP_E_TYPEMISMATCH)?;
    let given = given.ok_or(DISP_E_PARAMNOTOPTIONAL)?;
    // SAFETY: the caller's VARIANT, valid for the call.
    let viewed = unsafe { variant::view(given) }?;
    // SAFETY: as above.
    *cell = unsafe { conversion::change(&viewed, 0, base) }?;
    *owned = true;
    Ok(cell.data[0] as usize)
}

unsafe fn invoke_through_vtable(
    instance: usize,
    function: &Function,
    parsed: &Parsed<'_>,
    (arguments, locale): (&[Variant], u32),
    result: *mut Variant,
) -> Result<HResult, (HResult, u32)> {
    if function.parameters.len() > MOST_ARGUMENTS {
        return Err((DISP_E_BADPARAMCOUNT, 0));
    }
    let mut cells = Cells {
        values: [Variant::EMPTY; MOST_ARGUMENTS],
        owned: [false; MOST_ARGUMENTS],
    };
    let mut words = [0usize; 9];
    words[0] = instance;
    let mut next = 0usize;
    let mut retval: Option<(usize, u16)> = None;
    for (index, parameter) in function.parameters.iter().enumerate() {
        let word = if parameter.flags & PARAMFLAG_FLCID != 0 {
            Ok(locale as usize)
        } else if parameter.flags & PARAMFLAG_FRETVAL != 0 {
            match pointee_kind(parsed, &parameter.desc) {
                Some(base) => {
                    retval = Some((index, base));
                    cells.values[index] = Variant::new(base, 0);
                    Ok(place_of(base, &mut cells.values[index]))
                }
                None => Err((DISP_E_TYPEMISMATCH, 0)),
            }
        } else {
            let position = next;
            next += 1;
            let given = arguments
                .len()
                .checked_sub(position + 1)
                .map(|at| &arguments[at]);
            if given.is_none() && parameter.flags & PARAMFLAG_FOPT == 0 {
                Err((DISP_E_PARAMNOTOPTIONAL, position as u32))
            } else {
                let cell = (&mut cells.values[index], &mut cells.owned[index]);
                // SAFETY: the caller's arguments, valid for the call.
                unsafe { argument_word(parsed, &parameter.desc, given, cell) }
                    .map_err(|status| (status, position as u32))
            }
        };
        match word {
            Ok(word) => words[index + 1] = word,
            Err(failure) => {
                cells.clear();
                return Err(failure);
            }
        }
    }
    if next < arguments.len() {
        cells.clear();
        return Err((DISP_E_BADPARAMCOUNT, 0));
    }
    // SAFETY: a live object: its first word is its table, which holds the function's slot.
    let address = unsafe {
        (instance as *const *const usize)
            .read()
            .add(usize::from(function.slot))
            .read()
    };
    // SAFETY: the object's method with its arguments, every cell alive across the call.
    let returned = unsafe { super::super::call_program_nine(address, words) };
    let Some(returned) = returned else {
        cells.clear();
        return Err((E_NOTIMPL, 0));
    };
    let status = match function.result.kind() {
        VT_HRESULT => returned as u32 as i32,
        _ => S_OK,
    };
    let produced = match (retval, function.result.kind()) {
        (Some((index, base)), _) if status >= 0 => {
            let cell = cells.values[index];
            cells.owned[index] = false;
            Some(match base {
                kind::VARIANT => cell,
                _ => Variant::new(base, cell.data[0]),
            })
        }
        (None, VT_HRESULT | VT_VOID) => None,
        (None, other) => value_kind(parsed, &function.result)
            .filter(|_| other != VT_VOID)
            .map(|base| Variant::new(base, returned as u64 & word_mask(base))),
        _ => None,
    };
    cells.clear();
    if let Some(value) = produced {
        if result.is_null() {
            let mut value = value;
            // SAFETY: a value the call made that nobody asked for.
            unsafe { variant::clear(&mut value) };
        } else {
            // SAFETY: the caller's result VARIANT, which it initialized.
            unsafe { variant::replace(result, value) };
        }
    }
    Ok(status)
}

fn word_mask(base: u16) -> u64 {
    match variant::value_size(base).unwrap_or(LARGEST_RESULT) {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => u64::MAX,
    }
}

unsafe fn read_parameters<'a>(parameters: *const u8) -> &'a [Variant] {
    if parameters.is_null() {
        return &[];
    }
    // SAFETY: the caller's `DISPPARAMS`: its arguments and their count.
    unsafe {
        let values = parameters.cast::<*const Variant>().read_unaligned();
        let count = parameters.add(16).cast::<u32>().read_unaligned() as usize;
        match values.is_null() || count == 0 {
            true => &[],
            false => core::slice::from_raw_parts(values, count),
        }
    }
}

pub(super) unsafe extern "efiapi" fn info_invoke(
    this: *mut Info,
    instance: usize,
    member: i32,
    flags: u16,
    parameters: *const u8,
    result: *mut Variant,
    exception: *mut u8,
    argument_error: *mut u32,
) -> HResult {
    if instance == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's `DISPPARAMS`.
    let arguments = unsafe { read_parameters(parameters) };
    let outcome = with_entry(this, |parsed, entry| {
        let function = (0..entry.function_count())
            .filter_map(|index| entry.function(index))
            .find(|function| {
                function.id == member && function.invoke_kind & u32::from(flags) != 0
            })
            .filter(|function| function.function_kind <= 1)
            .ok_or(DISP_E_MEMBERNOTFOUND)?;
        // SAFETY: the caller's object and arguments.
        Ok(unsafe { invoke_through_vtable(instance, &function, parsed, (arguments, 0), result) })
    });
    match outcome {
        Err(status) => status,
        Ok(Err((status, position))) => {
            if !argument_error.is_null() {
                // SAFETY: the caller's writable argument index.
                unsafe { argument_error.write_unaligned(position) };
            }
            status
        }
        Ok(Ok(status)) if status < 0 => {
            if !exception.is_null() {
                // SAFETY: the caller's `EXCEPINFO`, its 64 bytes.
                unsafe {
                    exception.write_bytes(0, 64);
                    exception.add(EXCEPTION_STATUS).cast::<i32>().write_unaligned(status);
                }
            }
            DISP_E_EXCEPTION
        }
        Ok(Ok(_)) => S_OK,
    }
}

/// `DispGetIDsOfNames`.
///
/// # Safety
/// `info` must be a live `ITypeInfo`; the rest as `GetIDsOfNames` says.
pub(super) unsafe extern "efiapi" fn disp_get_ids_of_names(
    info: usize,
    names: *const *const u16,
    count: u32,
    ids: *mut i32,
) -> HResult {
    if info == 0 {
        return E_INVALIDARG;
    }
    let arguments = [names as usize, count as usize, ids as usize];
    // SAFETY: the type's `GetIDsOfNames`, slot 10.
    unsafe { crate::com::call_method(info, 10, arguments) as u32 as i32 }
}

/// `DispInvoke`: `ITypeInfo::Invoke` on `instance`.
///
/// # Safety
/// `info` must be a live `ITypeInfo` and the rest as `Invoke` says.
pub(super) unsafe extern "efiapi" fn disp_invoke(
    instance: usize,
    info: usize,
    member: i32,
    flags: u16,
    parameters: *const u8,
    result: *mut Variant,
    exception: *mut u8,
    argument_error: *mut u32,
) -> HResult {
    if info == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the layer's own type information, when its table says so.
    if unsafe { (info as *const usize).read() } != super::TYPE_INFO.address() {
        return E_NOTIMPL;
    }
    // SAFETY: the caller's duty, passed on.
    unsafe {
        info_invoke(
            info as *mut Info,
            instance,
            member,
            flags,
            parameters,
            result,
            exception,
            argument_error,
        )
    }
}

unsafe fn standard<'a>(this: *mut StandardDispatch) -> &'a StandardDispatch {
    // SAFETY: the caller's duty: a live standard dispatch object.
    unsafe { &*this }
}

/// `CreateStdDispatch`: an `IDispatch` over `object` described by `info`.
///
/// # Safety
/// `info` must be a live `ITypeInfo`, `object` the object it describes, and `out` writable.
pub(super) unsafe extern "efiapi" fn create_std_dispatch(
    _outer: usize,
    object: usize,
    info: usize,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if object == 0 || info == 0 {
        // SAFETY: the caller's writable pointer.
        unsafe { out.write_unaligned(0) };
        return E_INVALIDARG;
    }
    let made =
        crate::com::allocate(core::mem::size_of::<StandardDispatch>()).cast::<StandardDispatch>();
    if made.is_null() {
        // SAFETY: as above.
        unsafe { out.write_unaligned(0) };
        return E_OUTOFMEMORY;
    }
    // SAFETY: the type gains the object's reference; a fresh block for the object.
    unsafe {
        crate::com::add_reference(info);
        made.write(StandardDispatch {
            table: super::STANDARD_DISPATCH.address(),
            references: AtomicU32::new(1),
            object,
            info,
        });
        out.write_unaligned(made as usize);
    }
    S_OK
}

pub(super) unsafe extern "efiapi" fn standard_query_interface(
    this: *mut StandardDispatch,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &standard(this).references };
    let interfaces = [guid::UNKNOWN, guid::DISPATCH];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

pub(super) unsafe extern "efiapi" fn standard_add_reference(this: *mut StandardDispatch) -> u32 {
    // SAFETY: the caller's duty.
    unsafe { standard(this) }.references.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) unsafe extern "efiapi" fn standard_release(this: *mut StandardDispatch) -> u32 {
    // SAFETY: the caller's duty.
    let found = unsafe { standard(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if last {
        // SAFETY: the object's reference to its type, then its block.
        unsafe {
            crate::com::release(found.info);
            crate::com::free(this.cast());
        }
    }
    left
}

pub(super) unsafe extern "efiapi" fn standard_type_info_count(
    _this: *mut StandardDispatch,
    count: *mut u32,
) -> HResult {
    // SAFETY: the caller's writable count.
    unsafe { write_out(count, 1) }
}

pub(super) unsafe extern "efiapi" fn standard_type_info(
    this: *mut StandardDispatch,
    index: u32,
    _locale: u32,
    out: *mut usize,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if index != 0 {
        // SAFETY: the caller's writable pointer.
        unsafe { out.write_unaligned(0) };
        return TYPE_E_ELEMENTNOTFOUND;
    }
    // SAFETY: the object's live type gains the caller's reference.
    unsafe {
        let info = standard(this).info;
        crate::com::add_reference(info);
        out.write_unaligned(info);
    }
    S_OK
}

pub(super) unsafe extern "efiapi" fn standard_ids_of_names(
    this: *mut StandardDispatch,
    _interface: *const Guid,
    names: *const *const u16,
    count: u32,
    _locale: u32,
    ids: *mut i32,
) -> HResult {
    // SAFETY: the object's live type.
    unsafe { disp_get_ids_of_names(standard(this).info, names, count, ids) }
}

pub(super) unsafe extern "efiapi" fn standard_invoke(
    this: *mut StandardDispatch,
    member: i32,
    _interface: *const Guid,
    _locale: u32,
    flags: u16,
    parameters: *const u8,
    result: *mut Variant,
    exception: *mut u8,
    argument_error: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { standard(this) };
    // SAFETY: the object's type and object; the rest the caller's.
    unsafe {
        disp_invoke(
            found.object,
            found.info,
            member,
            flags,
            parameters,
            result,
            exception,
            argument_error,
        )
    }
}


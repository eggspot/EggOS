//! The using program's side of a class served out of process (D400): `CoCreateInstance` with
//! `CLSCTX_LOCAL_SERVER` connects to the class's socket (`channel`), first starting its server
//! from the registry's `LocalServer32` command line with `-Embedding` when nothing listens yet,
//! and asks it for an object. The answer is a proxy: an `IDispatch` whose `GetIDsOfNames` and
//! `Invoke` travel to the server and wait for its answer. Objects in results become proxies on
//! the same connection, which closes when the last of them is released. The program's own
//! objects cannot travel to the server yet (`DISP_E_TYPEMISMATCH`): the server would have to
//! call back into the program.

use super::super::{file, processes, registry};
use super::channel;
use crate::com::guid::{self, Guid};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Value};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK, bstr};
use crate::linux;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const CO_E_SERVER_EXEC_FAILURE: HResult = 0x8008_0005_u32 as i32;
const RPC_E_DISCONNECTED: HResult = 0x8001_0108_u32 as i32;
const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
const DISP_E_BADINDEX: HResult = 0x8002_000b_u32 as i32;
const DISP_E_BADPARAMCOUNT: HResult = 0x8002_000e_u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154_u32 as i32;
const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;
const START_ATTEMPTS: u32 = 600;
const START_PAUSE_NANOSECONDS: u64 = 50_000_000;
const EMBEDDING: &str = " -Embedding";
const CLASSES_KEY: &str = "HKEY_CLASSES_ROOT\\CLSID\\";
const LOCAL_SERVER_KEY: &str = "\\LocalServer32";
const EXCEPTION_BYTES: usize = 64;
const EXCEPTION_SOURCE: usize = 8;
const EXCEPTION_DESCRIPTION: usize = 16;
const EXCEPTION_HELP_FILE: usize = 24;
const EXCEPTION_HELP_CONTEXT: usize = 32;
const EXCEPTION_STATUS: usize = 56;
const PARAMETERS_NAMED: usize = 8;
const PARAMETERS_COUNT: usize = 16;
const PARAMETERS_NAMED_COUNT: usize = 20;

#[repr(C)]
#[derive(Debug)]
struct Link {
    references: AtomicU32,
    fd: usize,
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct Proxy {
    table: usize,
    references: AtomicU32,
    link: *mut Link,
    id: u32,
}

fn connect(class: &Guid) -> Option<usize> {
    let place = channel::place_of(class, false)?;
    let fd = linux::socket(AF_UNIX, SOCK_STREAM, 0).ok()?;
    match linux::connect(fd, place.address()) {
        Ok(_) => Some(fd),
        Err(_) => {
            let _ = linux::close(fd);
            None
        }
    }
}

fn server_command(class: &Guid) -> Option<Vec<u16>> {
    let mut path = alloc::string::String::from(CLASSES_KEY);
    for unit in class.text() {
        path.push(char::from(u8::try_from(unit).ok()?));
    }
    path.push_str(LOCAL_SERVER_KEY);
    let mut command = registry::string_value(&path, &[])?;
    command.extend(EMBEDDING.encode_utf16());
    Some(command)
}

fn start_server(class: &Guid) -> Result<(), HResult> {
    let command = server_command(class).ok_or(REGDB_E_CLASSNOTREG)?;
    let launch = processes::Launch {
        application: None,
        command_line: &command,
        directory: None,
        environment: processes::Environment::Inherited,
        streams: [0, 1, 2],
        own_group: false,
        inherit_handles: false,
        suspended: false,
    };
    let (handle, _) = processes::launch(&launch).map_err(|_| CO_E_SERVER_EXEC_FAILURE)?;
    let _ = file::close_handle(handle);
    Ok(())
}

fn connect_or_start(class: &Guid) -> Result<usize, HResult> {
    if let Some(fd) = connect(class) {
        return Ok(fd);
    }
    start_server(class)?;
    for _ in 0..START_ATTEMPTS {
        if let Some(fd) = connect(class) {
            return Ok(fd);
        }
        linux::sleep_nanoseconds(START_PAUSE_NANOSECONDS);
    }
    Err(CO_E_SERVER_EXEC_FAILURE)
}

fn new_link(fd: usize) -> *mut Link {
    let link = crate::com::allocate(core::mem::size_of::<Link>()).cast::<Link>();
    if link.is_null() {
        let _ = linux::close(fd);
        return link;
    }
    // SAFETY: a fresh block, sized and aligned for a link.
    unsafe {
        link.write(Link {
            references: AtomicU32::new(1),
            fd,
        })
    };
    link
}

unsafe fn drop_link(link: *mut Link) {
    // SAFETY: the caller's duty.
    let found = unsafe { &*link };
    if crate::com::drop_reference(&found.references).1 {
        let _ = linux::close(found.fd);
        // SAFETY: the link's block; no reference is left.
        unsafe { crate::com::free(link.cast()) };
    }
}

fn new_proxy(link: *mut Link, id: u32) -> Result<*mut Proxy, HResult> {
    let proxy = crate::com::allocate(core::mem::size_of::<Proxy>()).cast::<Proxy>();
    if proxy.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the caller's live link, which gains the proxy's reference.
    unsafe { &*link }.references.fetch_add(1, Ordering::Relaxed);
    // SAFETY: a fresh block, sized and aligned for a proxy.
    unsafe {
        proxy.write(Proxy {
            table: super::PROXY.address(),
            references: AtomicU32::new(1),
            link,
            id,
        })
    };
    Ok(proxy)
}

/// Makes an object of `class` in its server and gives the program a proxy for `interface`.
///
/// # Safety
/// `interface` must be a GUID and `result` writable.
pub(super) unsafe fn create(class: &Guid, interface: *const Guid, result: *mut usize) -> HResult {
    let fd = match connect_or_start(class) {
        Ok(fd) => fd,
        Err(status) => return status,
    };
    let link = new_link(fd);
    if link.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the link just made, which this call holds.
    let answer = ask(unsafe { &*link }, &wire::create(class.bytes()))
        .and_then(|body| wire::read_created(&body));
    let made = match answer {
        Some((status, id)) if status >= 0 && id != 0 => new_proxy(link, id),
        Some((status, _)) if status < 0 => Err(status),
        _ => Err(RPC_E_DISCONNECTED),
    };
    // SAFETY: this call's reference; a proxy made keeps its own.
    unsafe { drop_link(link) };
    let proxy = match made {
        Ok(proxy) => proxy,
        Err(status) => return status,
    };
    // SAFETY: a live proxy; the interface and result are the caller's.
    let status = unsafe { query_interface(proxy, interface, result) };
    // SAFETY: the creation's reference; the caller's, if any, keeps the proxy.
    unsafe { release(proxy) };
    status
}

fn ask(link: &Link, message: &[u8]) -> Option<Vec<u8>> {
    if !channel::send(link.fd, message) {
        return None;
    }
    channel::receive(link.fd)
}

unsafe fn proxy<'a>(this: *mut Proxy) -> &'a Proxy {
    // SAFETY: the caller's duty: a live proxy.
    unsafe { &*this }
}

/// `IUnknown::QueryInterface`: `IUnknown` and `IDispatch`.
///
/// # Safety
/// `this` must be a live proxy; the others as `QueryInterface` says.
pub(super) unsafe extern "efiapi" fn query_interface(
    this: *mut Proxy,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let references = unsafe { &proxy(this).references };
    let interfaces = [guid::UNKNOWN, guid::DISPATCH];
    // SAFETY: the caller's duty.
    unsafe { crate::com::answer_query(this as usize, references, interface, result, &interfaces) }
}

/// `IUnknown::AddRef`.
///
/// # Safety
/// `this` must be a live proxy.
pub(super) unsafe extern "efiapi" fn add_reference(this: *mut Proxy) -> u32 {
    // SAFETY: the caller's duty.
    unsafe { proxy(this) }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

/// `IUnknown::Release`: the last reference releases the server's object and the proxy's
/// share of the connection.
///
/// # Safety
/// `this` must be a live proxy the caller holds a reference to.
pub(super) unsafe extern "efiapi" fn release(this: *mut Proxy) -> u32 {
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    let (left, last) = crate::com::drop_reference(&found.references);
    if !last {
        return left;
    }
    // SAFETY: the proxy's live link.
    let _ = ask(unsafe { &*found.link }, &wire::release(found.id));
    // SAFETY: the proxy's own reference to its link.
    unsafe { drop_link(found.link) };
    // SAFETY: the proxy's block; no reference is left.
    unsafe { crate::com::free(this.cast()) };
    0
}

/// `IDispatch::GetTypeInfoCount`: a proxy offers no type information.
///
/// # Safety
/// `count` must be null or writable.
pub(super) unsafe extern "efiapi" fn get_type_info_count(
    _this: *mut Proxy,
    count: *mut u32,
) -> HResult {
    if count.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    unsafe { count.write(0) };
    S_OK
}

/// `IDispatch::GetTypeInfo`: there is none.
///
/// # Safety
/// `result` must be null or writable.
pub(super) unsafe extern "efiapi" fn get_type_info(
    _this: *mut Proxy,
    _index: u32,
    _locale: u32,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    unsafe { result.write(0) };
    DISP_E_BADINDEX
}

/// `IDispatch::GetIDsOfNames`, answered by the server.
///
/// # Safety
/// `this` must be a live proxy, `names` `count` NUL-terminated names and `numbers` room for
/// `count` numbers.
pub(super) unsafe extern "efiapi" fn get_ids_of_names(
    this: *mut Proxy,
    _interface: *const Guid,
    names: *const *const u16,
    count: u32,
    locale: u32,
    numbers: *mut i32,
) -> HResult {
    let count = count as usize;
    if names.is_null() || numbers.is_null() || count == 0 || count > wire::MOST_ITEMS {
        return E_INVALIDARG;
    }
    let mut texts: Vec<&[u16]> = Vec::new();
    for index in 0..count {
        // SAFETY: the caller's duty: `count` name pointers, each a NUL-terminated name.
        let name = unsafe { names.add(index).read() };
        // SAFETY: as above.
        texts.push(unsafe { crate::com::wide_text(name) });
    }
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    // SAFETY: the proxy's live link.
    let answer = ask(
        unsafe { &*found.link },
        &wire::names(found.id, locale, &texts),
    )
    .and_then(|body| wire::read_named(&body));
    let Some((status, given)) = answer else {
        return RPC_E_DISCONNECTED;
    };
    for (index, number) in given.iter().take(count).enumerate() {
        // SAFETY: the caller's duty: room for `count` numbers.
        unsafe { numbers.add(index).write(*number) };
    }
    status
}

fn value_of(link: *mut Link, held: &Variant) -> Result<Value, HResult> {
    // SAFETY: a valid VARIANT of the program's arguments.
    let held = unsafe { variant::view(held) }?;
    if held.kind & kind::ARRAY != 0 {
        return Err(DISP_E_TYPEMISMATCH);
    }
    Ok(match held.kind {
        kind::DECIMAL => Value::Decimal(variant::decimal_bytes(&held)),
        kind::BSTR => match held.pointer() {
            0 => Value::Text(None),
            // SAFETY: the VARIANT holds a live BSTR.
            text => Value::Text(Some(unsafe { bstr::units(text as *const u16) }.to_vec())),
        },
        kind::DISPATCH | kind::UNKNOWN => match held.pointer() {
            0 => Value::Object {
                kind: held.kind,
                id: 0,
            },
            object => {
                // SAFETY: a live COM object: its first word is its table.
                let table = unsafe { (object as *const usize).read() };
                // SAFETY: the layer's own proxy, when the table says so.
                let on_link = table == super::PROXY.address()
                    && unsafe { proxy(object as *mut Proxy) }.link == link;
                if !on_link {
                    return Err(DISP_E_TYPEMISMATCH);
                }
                Value::Object {
                    kind: held.kind,
                    // SAFETY: as above.
                    id: unsafe { proxy(object as *mut Proxy) }.id,
                }
            }
        },
        other => Value::Plain {
            kind: other,
            word: held.data[0],
        },
    })
}

fn variant_of(link: *mut Link, value: &Value) -> Result<Variant, HResult> {
    Ok(match value {
        Value::Plain { kind, word } => Variant::new(*kind, *word),
        Value::Decimal(bytes) => variant::decimal_variant(bytes),
        Value::Text(None) => Variant::new(kind::BSTR, 0),
        Value::Text(Some(units)) => {
            let text = bstr::from_units(units);
            if text.is_null() {
                return Err(E_OUTOFMEMORY);
            }
            Variant::new(kind::BSTR, text as u64)
        }
        Value::Object { kind, id: 0 } => Variant::new(*kind, 0),
        Value::Object { kind, id } => Variant::new(*kind, new_proxy(link, *id)? as u64),
    })
}

unsafe fn read_at<T: Copy>(base: *const u8, offset: usize) -> T {
    // SAFETY: the caller's duty: a readable `T` at `base + offset`.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}

fn exception_into(record: *mut u8, exception: &wire::Exception) {
    let texts = [
        (EXCEPTION_SOURCE, &exception.source),
        (EXCEPTION_DESCRIPTION, &exception.description),
        (EXCEPTION_HELP_FILE, &exception.help_file),
    ];
    // SAFETY: the program's writable `EXCEPINFO`, its 64 bytes.
    unsafe {
        record.write_bytes(0, EXCEPTION_BYTES);
        record.cast::<u16>().write_unaligned(exception.code);
        for (at, text) in texts {
            let made = match text.is_empty() {
                true => core::ptr::null_mut(),
                false => bstr::from_units(text),
            };
            record
                .add(at)
                .cast::<usize>()
                .write_unaligned(made as usize);
        }
        record
            .add(EXCEPTION_HELP_CONTEXT)
            .cast::<u32>()
            .write_unaligned(exception.help_context);
        record
            .add(EXCEPTION_STATUS)
            .cast::<i32>()
            .write_unaligned(exception.status);
    }
}

/// `IDispatch::Invoke`, carried out by the server.
///
/// # Safety
/// `this` must be a live proxy and the others as `Invoke` says.
pub(super) unsafe extern "efiapi" fn invoke(
    this: *mut Proxy,
    member: i32,
    _interface: *const Guid,
    locale: u32,
    flags: u16,
    parameters: *const u8,
    result: *mut Variant,
    exception: *mut u8,
    argument_error: *mut u32,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    let link = found.link;
    let (mut arguments, mut named) = (Vec::new(), Vec::new());
    if !parameters.is_null() {
        // SAFETY: the caller's `DISPPARAMS`: its arguments, named numbers and their counts.
        let (values, numbers, count, named_count) = unsafe {
            (
                read_at::<*const Variant>(parameters, 0),
                read_at::<*const i32>(parameters, PARAMETERS_NAMED),
                read_at::<u32>(parameters, PARAMETERS_COUNT) as usize,
                read_at::<u32>(parameters, PARAMETERS_NAMED_COUNT) as usize,
            )
        };
        if count > wire::MOST_ITEMS || named_count > count || (count > 0 && values.is_null()) {
            return DISP_E_BADPARAMCOUNT;
        }
        for index in 0..count {
            // SAFETY: as above: `count` VARIANTs.
            let held = unsafe { values.add(index).read_unaligned() };
            match value_of(link, &held) {
                Ok(value) => arguments.push(value),
                Err(status) => {
                    if !argument_error.is_null() {
                        // SAFETY: the caller's writable argument index.
                        unsafe { argument_error.write(index as u32) };
                    }
                    return status;
                }
            }
        }
        for index in 0..named_count {
            // SAFETY: as above: `named_count` numbers.
            named.push(unsafe { numbers.add(index).read_unaligned() });
        }
    }
    let message = wire::invoke(found.id, (member, locale, flags), &arguments, &named);
    // SAFETY: the proxy's live link.
    let Some(answer) = ask(unsafe { &*link }, &message).and_then(|body| wire::read_invoked(&body))
    else {
        return RPC_E_DISCONNECTED;
    };
    if !argument_error.is_null() {
        // SAFETY: the caller's writable argument index.
        unsafe { argument_error.write(answer.argument_error) };
    }
    if let (Some(given), false) = (&answer.exception, exception.is_null()) {
        exception_into(exception, given);
    }
    if !result.is_null() {
        let value = match variant_of(link, &answer.result) {
            Ok(value) => value,
            Err(status) => return status,
        };
        // SAFETY: the caller's VARIANT for the result, which it initialized.
        unsafe { variant::replace(result, value) };
    } else if let Ok(mut value) = variant_of(link, &answer.result) {
        // SAFETY: the result nobody asked for, made here.
        unsafe { variant::clear(&mut value) };
    }
    answer.status
}

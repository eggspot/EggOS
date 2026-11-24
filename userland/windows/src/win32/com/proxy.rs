//! The using program's side of a class served out of process (D400): `CoCreateInstance` with
//! `CLSCTX_LOCAL_SERVER` connects to the class's socket (`channel`), first starting its server
//! from the registry's `LocalServer32` command line with `-Embedding` when nothing listens yet,
//! and asks it for an object. The answer is a proxy: an `IDispatch` whose `GetIDsOfNames` and
//! `Invoke` travel to the server and wait for its answer. Objects in results become proxies on
//! the same connection, which closes when the last of them is released. Arrays travel, and
//! arguments passed by reference are written back with the values the server left in them
//! (D743). Calls go both ways (D744): the program's own objects travel as objects it serves on
//! the connection (its event sinks, its callbacks), and while either end waits for an answer
//! it serves the other end's requests. A proxy asked for another interface asks the other
//! end: `IConnectionPointContainer` and `IConnectionPoint` get proxies of their own
//! (`FindConnectionPoint`, `Advise`, `Unadvise`, `GetConnectionInterface`), and an interface
//! the object serves at its `IDispatch` gets a dispatch proxy. The same proxies serve a
//! server calling back into the program it serves.

use super::super::{file, processes, registry};
use super::carry::{self, Objects};
use super::{channel, local_server};
use crate::com::guid::{self, Guid};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Reference, Value, request};
use crate::com::{E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, HResult, S_OK, bstr};
use crate::linux;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

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
const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
const CLOSED: usize = usize::MAX;
const CONNECTION_POINT_CONTAINER: Guid = Guid::new(
    0xb196_b284,
    0xbab4,
    0x101a,
    [0xb6, 0x9c, 0x00, 0xaa, 0x00, 0x34, 0x1d, 0x07],
);
const CONNECTION_POINT: Guid = Guid::new(
    0xb196_b286,
    0xbab4,
    0x101a,
    [0xb6, 0x9c, 0x00, 0xaa, 0x00, 0x34, 0x1d, 0x07],
);

/// One end of a connection: its socket, and whether the link closes it when it goes (the
/// using end's) or the serving end's connection does.
#[repr(C)]
#[derive(Debug)]
pub(super) struct Link {
    references: AtomicU32,
    fd: AtomicUsize,
    owns: bool,
}

impl Link {
    fn fd(&self) -> usize {
        self.fd.load(Ordering::Acquire)
    }
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct Proxy {
    table: usize,
    references: AtomicU32,
    link: *mut Link,
    id: u32,
    interface: Guid,
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

fn allocate_link(fd: usize, owns: bool) -> *mut Link {
    let link = crate::com::allocate(core::mem::size_of::<Link>()).cast::<Link>();
    if !link.is_null() {
        // SAFETY: a fresh block, sized and aligned for a link.
        unsafe {
            link.write(Link {
                references: AtomicU32::new(1),
                fd: AtomicUsize::new(fd),
                owns,
            })
        };
    }
    link
}

fn new_link(fd: usize) -> *mut Link {
    let link = allocate_link(fd, true);
    if link.is_null() || !local_server::adopt_connection(fd, link as usize) {
        let _ = linux::close(fd);
        if !link.is_null() {
            // SAFETY: the link just made, which nothing else holds.
            unsafe { crate::com::free(link.cast()) };
        }
        return core::ptr::null_mut();
    }
    link
}

/// A link for the serving end's connection `fd`, which the connection closes; one reference,
/// the connection's. Null without memory.
pub(super) fn connection_link(fd: usize) -> *mut Link {
    allocate_link(fd, false)
}

/// Marks `link` closed: its connection's socket is gone, so its proxies' calls fail rather
/// than reach whatever reuses the descriptor.
///
/// # Safety
/// `link` must be a live link.
pub(super) unsafe fn close_link(link: *mut Link) {
    // SAFETY: the caller's duty.
    unsafe { &*link }.fd.store(CLOSED, Ordering::Release);
}

/// Gives back one reference to `link`; the last closes a using end's connection.
///
/// # Safety
/// `link` must be a live link the caller holds a reference to.
pub(super) unsafe fn drop_link(link: *mut Link) {
    // SAFETY: the caller's duty.
    let found = unsafe { &*link };
    if crate::com::drop_reference(&found.references).1 {
        let fd = found.fd();
        if found.owns && fd != CLOSED {
            local_server::close_connection(fd);
        }
        // SAFETY: the link's block; no reference is left.
        unsafe { crate::com::free(link.cast()) };
    }
}

fn new_proxy(link: *mut Link, id: u32) -> Result<*mut Proxy, HResult> {
    new_proxy_with(link, id, super::PROXY.address(), guid::DISPATCH)
}

fn new_proxy_with(
    link: *mut Link,
    id: u32,
    table: usize,
    interface: Guid,
) -> Result<*mut Proxy, HResult> {
    let proxy = crate::com::allocate(core::mem::size_of::<Proxy>()).cast::<Proxy>();
    if proxy.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: the caller's live link, which gains the proxy's reference.
    unsafe { &*link }.references.fetch_add(1, Ordering::Relaxed);
    // SAFETY: a fresh block, sized and aligned for a proxy.
    unsafe {
        proxy.write(Proxy {
            table,
            references: AtomicU32::new(1),
            link,
            id,
            interface,
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
    let fd = link.fd();
    if fd == CLOSED || !channel::send(fd, message) {
        return None;
    }
    loop {
        let body = channel::receive(fd)?;
        if wire::is_reply(&body) {
            return Some(body);
        }
        let answer = local_server::serve_request(fd, &body)?;
        if !channel::send(fd, &answer) {
            return None;
        }
    }
}

fn is_proxy_table(table: usize) -> bool {
    [
        super::PROXY.address(),
        super::CONTAINER_PROXY.address(),
        super::POINT_PROXY.address(),
    ]
    .contains(&table)
}

fn own_interfaces(table: usize) -> [Guid; 2] {
    if table == super::CONTAINER_PROXY.address() {
        [guid::UNKNOWN, CONNECTION_POINT_CONTAINER]
    } else if table == super::POINT_PROXY.address() {
        [guid::UNKNOWN, CONNECTION_POINT]
    } else {
        [guid::UNKNOWN, guid::DISPATCH]
    }
}

unsafe fn proxy<'a>(this: *mut Proxy) -> &'a Proxy {
    // SAFETY: the caller's duty: a live proxy.
    unsafe { &*this }
}

fn remote_interface(found: &Proxy, wanted: Guid) -> Result<*mut Proxy, HResult> {
    // SAFETY: the proxy's live link.
    let link = unsafe { &*found.link };
    let answer = ask(link, &wire::query(found.id, wanted.bytes()))
        .and_then(|body| wire::read_numbered(&body, request::QUERY));
    let (status, id, dispatch) = answer.ok_or(RPC_E_DISCONNECTED)?;
    if status < 0 {
        return Err(status);
    }
    let table = if wanted == CONNECTION_POINT_CONTAINER {
        super::CONTAINER_PROXY.address()
    } else if wanted == CONNECTION_POINT {
        super::POINT_PROXY.address()
    } else if dispatch {
        super::PROXY.address()
    } else {
        let _ = ask(link, &wire::release(id));
        return Err(E_NOINTERFACE);
    };
    new_proxy_with(found.link, id, table, wanted)
}

/// `IUnknown::QueryInterface`: the proxy's own interfaces here, any other from the object.
///
/// # Safety
/// `this` must be a live proxy; the others as `QueryInterface` says.
pub(super) unsafe extern "efiapi" fn query_interface(
    this: *mut Proxy,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    let own = own_interfaces(found.table);
    // SAFETY: the caller's duty: null or a GUID.
    let wanted = unsafe { Guid::read(interface) };
    let Some(wanted) = wanted.filter(|wanted| !own.contains(wanted)) else {
        let references = &found.references;
        // SAFETY: the caller's duty.
        return unsafe {
            crate::com::answer_query(this as usize, references, interface, result, &own)
        };
    };
    if result.is_null() {
        return E_POINTER;
    }
    let made = remote_interface(found, wanted);
    // SAFETY: the caller's writable interface pointer.
    unsafe { result.write(made.map_or(0, |made| made as usize)) };
    made.map_or_else(|status| status, |_| S_OK)
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

/// How objects travel on one link: a proxy of the same link goes back as the other end's
/// object, any other object is served on the link; objects that arrive become proxies, or are
/// this end's own again.
pub(super) struct LinkObjects {
    link: *mut Link,
}

impl LinkObjects {
    pub(super) fn new(link: *mut Link) -> Self {
        Self { link }
    }

    fn fd(&self) -> usize {
        // SAFETY: a live link the caller holds.
        unsafe { &*self.link }.fd()
    }
}

impl Objects for LinkObjects {
    fn object_value(&mut self, kind: u16, object: usize) -> Result<Value, HResult> {
        // SAFETY: a live COM object: its first word is its table.
        let table = unsafe { (object as *const usize).read() };
        if is_proxy_table(table) {
            // SAFETY: the layer's own proxy, as its table says.
            let found = unsafe { proxy(object as *mut Proxy) };
            if found.link == self.link {
                return Ok(Value::Peer { kind, id: found.id });
            }
        }
        // SAFETY: a live object; the connection's table takes this reference.
        unsafe { crate::com::add_reference(object) };
        match local_server::hand_out(self.fd(), object) {
            0 => Err(RPC_E_DISCONNECTED),
            id => Ok(Value::Object { kind, id }),
        }
    }

    fn object_of(&mut self, value: &Value) -> Result<usize, HResult> {
        match *value {
            Value::Object { id, .. } => new_proxy(self.link, id).map(|made| made as usize),
            Value::Peer { id, .. } => {
                let object = local_server::object_of(self.fd(), id).ok_or(DISP_E_TYPEMISMATCH)?;
                // SAFETY: a live object of the connection; its new holder gets a reference.
                unsafe { crate::com::add_reference(object) };
                Ok(object)
            }
            _ => Err(E_INVALIDARG),
        }
    }
}

fn variant_of(link: *mut Link, value: &Value) -> Result<Variant, HResult> {
    carry::variant_of(&mut LinkObjects::new(link), value)
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
    let (mut by_reference, mut places) = (Vec::new(), Vec::new());
    let mut objects = LinkObjects::new(link);
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
            if held.kind & kind::BYREF != 0 && held.pointer() != 0 {
                let base = held.kind & !kind::BYREF;
                by_reference.push(Reference {
                    index: index as u16,
                    kind: base,
                });
                places.push((held.pointer(), base));
            }
            match carry::value_of(&mut objects, &held) {
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
    let call = (member, locale, flags);
    let message = wire::invoke(found.id, call, &arguments, &named, &by_reference);
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
    for (&(place, base), changed) in places.iter().zip(&answer.changed) {
        if let Ok(value) = variant_of(link, changed) {
            // SAFETY: the program's place its `VT_BYREF` argument points at, of kind `base`.
            unsafe { carry::write_back(place, base, value) };
        }
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

/// `IConnectionPointContainer::FindConnectionPoint`, answered by the object.
///
/// # Safety
/// `this` must be a live container proxy, `interface` a GUID and `result` writable.
pub(super) unsafe extern "efiapi" fn find_connection_point(
    this: *mut Proxy,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's writable interface pointer.
    unsafe { result.write(0) };
    // SAFETY: the caller's duty: null or a GUID.
    let Some(wanted) = (unsafe { Guid::read(interface) }) else {
        return E_INVALIDARG;
    };
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    let message = wire::find_point(found.id, wanted.bytes());
    // SAFETY: the proxy's live link.
    let answer = ask(unsafe { &*found.link }, &message)
        .and_then(|body| wire::read_numbered(&body, request::FIND_POINT));
    let Some((status, id, _)) = answer else {
        return RPC_E_DISCONNECTED;
    };
    if status < 0 {
        return status;
    }
    match new_proxy_with(found.link, id, super::POINT_PROXY.address(), wanted) {
        Ok(point) => {
            // SAFETY: as above.
            unsafe { result.write(point as usize) };
            S_OK
        }
        Err(failure) => failure,
    }
}

/// `EnumConnectionPoints`, `GetConnectionPointContainer` and `EnumConnections`: not offered.
///
/// # Safety
/// `result` must be null or writable.
pub(super) unsafe extern "efiapi" fn not_offered(_this: *mut Proxy, result: *mut usize) -> HResult {
    if !result.is_null() {
        // SAFETY: the caller's duty.
        unsafe { result.write(0) };
    }
    E_NOTIMPL
}

/// `IConnectionPoint::GetConnectionInterface`: the interface the point was found for.
///
/// # Safety
/// `this` must be a live point proxy and `result` null or writable.
pub(super) unsafe extern "efiapi" fn get_connection_interface(
    this: *mut Proxy,
    result: *mut Guid,
) -> HResult {
    if result.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    unsafe { result.write_unaligned(proxy(this).interface) };
    S_OK
}

/// `IConnectionPoint::Advise`: the sink, the program's own object, is served on the
/// connection and the object's point connects it.
///
/// # Safety
/// `this` must be a live point proxy, `sink` a live object and `cookie` null or writable.
pub(super) unsafe extern "efiapi" fn advise(
    this: *mut Proxy,
    sink: usize,
    cookie: *mut u32,
) -> HResult {
    if sink == 0 || cookie.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    let sink = match LinkObjects::new(found.link).object_value(kind::UNKNOWN, sink) {
        Ok(sink) => sink,
        Err(status) => return status,
    };
    // SAFETY: the proxy's live link.
    let answer = ask(unsafe { &*found.link }, &wire::advise(found.id, &sink))
        .and_then(|body| wire::read_numbered(&body, request::ADVISE));
    let Some((status, number, _)) = answer else {
        return RPC_E_DISCONNECTED;
    };
    // SAFETY: the caller's writable cookie.
    unsafe { cookie.write(if status < 0 { 0 } else { number }) };
    status
}

/// `IConnectionPoint::Unadvise`.
///
/// # Safety
/// `this` must be a live point proxy.
pub(super) unsafe extern "efiapi" fn unadvise(this: *mut Proxy, cookie: u32) -> HResult {
    // SAFETY: the caller's duty.
    let found = unsafe { proxy(this) };
    // SAFETY: the proxy's live link.
    ask(unsafe { &*found.link }, &wire::unadvise(found.id, cookie))
        .and_then(|body| wire::read_status(&body, request::UNADVISE))
        .unwrap_or(RPC_E_DISCONNECTED)
}

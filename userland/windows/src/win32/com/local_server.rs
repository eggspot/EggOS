//! Class objects a program registers (`CoRegisterClassObject`, D400): the same process creates
//! their objects directly, and a class registered with `CLSCTX_LOCAL_SERVER` also listens on
//! its socket (`channel`) for other programs. Their requests are served on the registering
//! thread when it looks for messages, as Windows serves a single-threaded apartment's calls
//! (any thread for one registered from the multithreaded apartment): an object is made through
//! the class object's `IClassFactory` and asked for `IDispatch`, and each connection keeps the
//! objects it was given, numbered, until it releases them or goes. Arguments passed by
//! reference are held in cells for the call and their values sent back after it (D743). A
//! program using a class out of process keeps its own end in the same table (D744): the
//! objects it hands the server (sinks, callbacks) are numbered there, and its connection is
//! served like a server's, so the server can call back while the program waits or later from
//! its message loop. Queries for other interfaces and the connection-point calls are served
//! too, and objects that arrive become proxies on the connection's link.

use super::super::{SpinLock, thread_environment};
use super::carry;
use super::channel;
use super::proxy::{self, Link, LinkObjects};
use crate::com::guid::{self, Guid};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Exception, Invoked, Reference, Request, Value, request};
use crate::com::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, S_OK, bstr};
use crate::linux::{self, POLL_ERROR, POLL_HANG_UP, POLL_IN, Watched};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

const CLSCTX_INPROC_SERVER: u32 = 0x1;
const CLSCTX_LOCAL_SERVER: u32 = 0x4;
const REGCLS_SINGLEUSE: u32 = 0;
const REGCLS_SUSPENDED: u32 = 4;
const CO_E_OBJISREG: HResult = 0x8004_01fb_u32 as i32;
const CO_E_OBJNOTREG: HResult = 0x8004_01fc_u32 as i32;
const DISP_E_EXCEPTION: HResult = 0x8002_0009_u32 as i32;
const DISP_E_BADPARAMCOUNT: HResult = 0x8002_000e_u32 as i32;
const RPC_E_DISCONNECTED: HResult = 0x8001_0108_u32 as i32;
const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
static NO_INTERFACE: Guid = guid::NULL;
const SLOT_CREATE_INSTANCE: usize = 3;
const SLOT_GET_IDS_OF_NAMES: usize = 5;
const SLOT_INVOKE: usize = 6;
const SLOT_FIND_CONNECTION_POINT: usize = 4;
const SLOT_ADVISE: usize = 5;
const SLOT_UNADVISE: usize = 6;
const CONNECTION_POINT_CONTAINER: Guid = Guid::new(
    0xb196_b284,
    0xbab4,
    0x101a,
    [0xb6, 0x9c, 0x00, 0xaa, 0x00, 0x34, 0x1d, 0x07],
);
const BACKLOG: usize = 16;
const EXCEPTION_BYTES: usize = 64;
const EXCEPTION_SOURCE: usize = 8;
const EXCEPTION_DESCRIPTION: usize = 16;
const EXCEPTION_HELP_FILE: usize = 24;
const EXCEPTION_HELP_CONTEXT: usize = 32;
const EXCEPTION_STATUS: usize = 56;
const EADDRINUSE: u16 = 98;
const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;

/// The `IClassFactory` interface's id.
pub(super) const CLASS_FACTORY: Guid = Guid::new(0x0000_0001, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);

#[derive(Debug, Clone, Copy)]
struct Registration {
    cookie: u32,
    class: Guid,
    factory: usize,
    context: u32,
    single_use: bool,
    thread: u32,
    listener: Option<usize>,
    armed: bool,
}

#[derive(Debug)]
struct Connection {
    fd: usize,
    thread: u32,
    armed: bool,
    objects: Vec<Entry>,
    next: u32,
    link: usize,
    owns_link: bool,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: u32,
    object: usize,
    handed_out: u32,
}

static CLASSES: SpinLock<Vec<Registration>> = SpinLock::new(Vec::new());
static CONNECTIONS: SpinLock<Vec<Connection>> = SpinLock::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);
static SERVER_REFERENCES: AtomicU32 = AtomicU32::new(0);
static SUSPENDED: AtomicU32 = AtomicU32::new(0);

fn listen_on(class: &Guid) -> Option<usize> {
    let place = channel::place_of(class, true)?;
    let fd = linux::socket(AF_UNIX, SOCK_STREAM, 0).ok()?;
    if let Err(errno) = linux::bind(fd, place.address()) {
        let replaced = errno.0 == EADDRINUSE
            && linux::remove(place.path(), 0).is_ok()
            && linux::bind(fd, place.address()).is_ok();
        if !replaced {
            let _ = linux::close(fd);
            return None;
        }
    }
    if linux::listen(fd, BACKLOG).is_err() {
        let _ = linux::close(fd);
        let _ = linux::remove(place.path(), 0);
        return None;
    }
    Some(fd)
}

fn stop_listening(class: &Guid, listener: usize) {
    let _ = linux::close(listener);
    if let Some(place) = channel::place_of(class, false) {
        let _ = linux::remove(place.path(), 0);
    }
}

fn serving_thread() -> u32 {
    match super::is_multithreaded() {
        true => 0,
        false => thread_environment::thread_id(),
    }
}

/// `CoRegisterClassObject`.
pub(super) unsafe extern "efiapi" fn co_register_class_object(
    class: *const Guid,
    factory: usize,
    context: u32,
    flags: u32,
    cookie: *mut u32,
) -> HResult {
    if cookie.is_null() || factory == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: `CoRegisterClassObject`'s contract: null or a CLSID.
    let Some(class) = (unsafe { Guid::read(class) }) else {
        return E_INVALIDARG;
    };
    let taken = CLASSES.with(|classes| {
        classes
            .iter()
            .any(|registered| registered.class == class && registered.context & context != 0)
    });
    if taken {
        return CO_E_OBJISREG;
    }
    let suspended = flags & REGCLS_SUSPENDED != 0;
    let listener = match context & CLSCTX_LOCAL_SERVER != 0 && !suspended {
        true => listen_on(&class),
        false => None,
    };
    if suspended {
        SUSPENDED.fetch_add(1, Ordering::AcqRel);
    }
    // SAFETY: the program's class object, which the registration keeps a reference to.
    unsafe { crate::com::add_reference(factory) };
    let number = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
    CLASSES.with(|classes| {
        classes.push(Registration {
            cookie: number,
            class,
            factory,
            context,
            single_use: flags & !REGCLS_SUSPENDED == REGCLS_SINGLEUSE,
            thread: serving_thread(),
            listener,
            armed: listener.is_some(),
        })
    });
    // SAFETY: the caller passes a writable cookie.
    unsafe { cookie.write(number) };
    S_OK
}

/// `CoRevokeClassObject`.
pub(super) extern "efiapi" fn co_revoke_class_object(cookie: u32) -> HResult {
    let found = CLASSES.with(|classes| {
        let at = classes
            .iter()
            .position(|registered| registered.cookie == cookie)?;
        Some(classes.remove(at))
    });
    let Some(registration) = found else {
        return CO_E_OBJNOTREG;
    };
    if let Some(listener) = registration.listener {
        stop_listening(&registration.class, listener);
    }
    // SAFETY: the registration's own reference to the class object.
    unsafe { crate::com::release(registration.factory) };
    S_OK
}

/// `CoResumeClassObjects`: the classes registered suspended start listening.
pub(super) extern "efiapi" fn co_resume_class_objects() -> HResult {
    SUSPENDED.store(0, Ordering::Release);
    let waiting: Vec<(u32, Guid)> = CLASSES.with(|classes| {
        classes
            .iter()
            .filter(|registered| {
                registered.context & CLSCTX_LOCAL_SERVER != 0 && registered.listener.is_none()
            })
            .map(|registered| (registered.cookie, registered.class))
            .collect()
    });
    for (cookie, class) in waiting {
        let listener = listen_on(&class);
        CLASSES.with(|classes| {
            let found = classes
                .iter_mut()
                .find(|registered| registered.cookie == cookie);
            if let Some(registered) = found {
                registered.listener = listener;
                registered.armed = listener.is_some();
            }
        });
    }
    S_OK
}

/// `CoSuspendClassObjects`: no class takes new connections until resumed.
pub(super) extern "efiapi" fn co_suspend_class_objects() -> HResult {
    let listening: Vec<(Guid, usize)> = CLASSES.with(|classes| {
        classes
            .iter_mut()
            .filter_map(|registered| {
                let listener = registered.listener.take()?;
                registered.armed = false;
                Some((registered.class, listener))
            })
            .collect()
    });
    for (class, listener) in listening {
        stop_listening(&class, listener);
    }
    SUSPENDED.fetch_add(1, Ordering::AcqRel);
    S_OK
}

/// `CoAddRefServerProcess`.
pub(super) extern "efiapi" fn co_add_ref_server_process() -> u32 {
    SERVER_REFERENCES.fetch_add(1, Ordering::AcqRel) + 1
}

/// `CoReleaseServerProcess`: the last reference suspends every class, as Windows does.
pub(super) extern "efiapi" fn co_release_server_process() -> u32 {
    let left = SERVER_REFERENCES
        .fetch_sub(1, Ordering::AcqRel)
        .saturating_sub(1);
    if left == 0 {
        co_suspend_class_objects();
    }
    left
}

/// The class object this process registered for `class` in `context`, with a reference.
pub(super) fn registered_factory(class: &Guid, context: u32) -> Option<usize> {
    let factory = CLASSES.with(|classes| {
        classes
            .iter()
            .find(|registered| {
                registered.class == *class
                    && registered.context & context & (CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER)
                        != 0
            })
            .map(|registered| registered.factory)
    })?;
    // SAFETY: the registration holds the class object alive; the caller gets its own reference.
    unsafe { crate::com::add_reference(factory) };
    Some(factory)
}

/// Makes an object of the class object `factory` and asks it for `interface`, as
/// `IClassFactory::CreateInstance` does.
///
/// # Safety
/// `factory` must be a live class object, `interface` a GUID and `result` writable.
pub(super) unsafe fn create_with(
    factory: usize,
    outer: usize,
    interface: *const Guid,
    result: *mut usize,
) -> HResult {
    // SAFETY: the caller's duty; `CreateInstance` is the factory's fourth method.
    unsafe {
        crate::com::call_method(
            factory,
            SLOT_CREATE_INSTANCE,
            [outer, interface as usize, result as usize],
        ) as u32 as i32
    }
}

fn armed_watch(fd: usize) -> Watched {
    Watched {
        fd: fd as i32,
        asked: POLL_IN,
        reported: 0,
    }
}

/// The listening sockets and connections waiting to be read into `out`; how many.
pub fn watched(out: &mut [Watched]) -> usize {
    let mut count = 0;
    CLASSES.with(|classes| {
        for registered in classes.iter().filter(|registered| registered.armed) {
            if let (Some(listener), Some(slot)) = (registered.listener, out.get_mut(count)) {
                *slot = armed_watch(listener);
                count += 1;
            }
        }
    });
    CONNECTIONS.with(|connections| {
        for connection in connections.iter().filter(|connection| connection.armed) {
            if let Some(slot) = out.get_mut(count) {
                *slot = armed_watch(connection.fd);
                count += 1;
            }
        }
    });
    count
}

/// After a wait: the sockets that reported wait for their thread to serve them, unwatched
/// until then, so a thread busy elsewhere does not keep the waits spinning.
pub fn reported(watched: &[Watched]) {
    for one in watched.iter().filter(|one| one.reported != 0) {
        let fd = one.fd as usize;
        CLASSES.with(|classes| {
            for registered in classes.iter_mut() {
                if registered.listener == Some(fd) {
                    registered.armed = false;
                }
            }
        });
        CONNECTIONS.with(|connections| {
            for connection in connections.iter_mut() {
                if connection.fd == fd {
                    connection.armed = false;
                }
            }
        });
    }
}

fn mine(thread: u32, me: u32) -> bool {
    thread == 0 || thread == me
}

fn ready(fd: usize) -> u16 {
    let mut one = [armed_watch(fd)];
    match linux::wait_for_events(&mut one, Some(0)) {
        Ok(1..) => one[0].reported,
        _ => 0,
    }
}

/// Serves what waits for the calling thread: new connections and one request on each
/// connection that has one. Called whenever the thread looks for a message.
pub fn serve_ready() {
    let me = thread_environment::thread_id();
    let listeners: Vec<(u32, usize)> = CLASSES.with(|classes| {
        classes
            .iter_mut()
            .filter(|registered| mine(registered.thread, me))
            .filter_map(|registered| {
                registered.armed = registered.listener.is_some();
                Some((registered.cookie, registered.listener?))
            })
            .collect()
    });
    for (cookie, listener) in listeners {
        if ready(listener) & POLL_IN != 0 {
            accept_one(cookie, listener);
        }
    }
    let readable: Vec<usize> = CONNECTIONS.with(|connections| {
        connections
            .iter_mut()
            .filter(|connection| mine(connection.thread, me))
            .map(|connection| {
                connection.armed = true;
                connection.fd
            })
            .collect()
    });
    for fd in readable {
        let events = ready(fd);
        if events & POLL_IN != 0 {
            serve_one(fd);
        } else if events & (POLL_HANG_UP | POLL_ERROR) != 0 {
            close_connection(fd);
        }
    }
}

fn accept_one(cookie: u32, listener: usize) {
    let Ok(fd) = linux::accept(listener, 0) else {
        return;
    };
    let found = CLASSES.with(|classes| {
        let registered = classes
            .iter()
            .find(|registered| registered.cookie == cookie)?;
        Some((registered.single_use, registered.class, registered.thread))
    });
    let Some((single_use, class, thread)) = found else {
        let _ = linux::close(fd);
        return;
    };
    if single_use {
        CLASSES.with(|classes| {
            let found = classes
                .iter_mut()
                .find(|registered| registered.cookie == cookie);
            if let Some(registered) = found {
                registered.listener = None;
                registered.armed = false;
            }
        });
        stop_listening(&class, listener);
    }
    CONNECTIONS.with(|connections| {
        connections.push(Connection {
            fd,
            thread,
            armed: true,
            objects: Vec::new(),
            next: 1,
            link: 0,
            owns_link: false,
        })
    });
}

/// Keeps a using program's connection `fd` (whose link is `link`) in the table, served on the
/// calling thread, so the objects it hands the server are numbered and the server's calls
/// back are answered.
pub(super) fn adopt_connection(fd: usize, link: usize) -> bool {
    let thread = thread_environment::thread_id();
    CONNECTIONS.with(|connections| {
        connections.push(Connection {
            fd,
            thread,
            armed: true,
            objects: Vec::new(),
            next: 1,
            link,
            owns_link: false,
        })
    });
    true
}

/// Closes connection `fd`: its socket, the objects it was given, and its link's calls.
pub(super) fn close_connection(fd: usize) {
    let removed = CONNECTIONS.with(|connections| {
        let at = connections
            .iter()
            .position(|connection| connection.fd == fd)?;
        let connection = connections.remove(at);
        Some((connection.objects, connection.link, connection.owns_link))
    });
    let _ = linux::close(fd);
    let Some((objects, link, owns_link)) = removed else {
        return;
    };
    for entry in objects {
        // SAFETY: the connection's own reference to each object it was given.
        unsafe { crate::com::release(entry.object) };
    }
    if link != 0 {
        // SAFETY: the connection's link, live while the connection or its proxies hold it.
        unsafe {
            proxy::close_link(link as *mut Link);
            if owns_link {
                proxy::drop_link(link as *mut Link);
            }
        }
    }
}

fn link_of(fd: usize) -> *mut Link {
    let known = CONNECTIONS.with(|connections| {
        let connection = connections.iter().find(|connection| connection.fd == fd)?;
        Some(connection.link)
    });
    match known {
        None => core::ptr::null_mut(),
        Some(0) => {
            let made = proxy::connection_link(fd);
            if made.is_null() {
                return made;
            }
            CONNECTIONS.with(|connections| {
                if let Some(connection) = connections.iter_mut().find(|found| found.fd == fd) {
                    connection.link = made as usize;
                    connection.owns_link = true;
                }
            });
            made
        }
        Some(link) => link as *mut Link,
    }
}

/// Numbers `object` on connection `fd`, taking the caller's reference; 0 when the connection
/// is gone.
pub(super) fn hand_out(fd: usize, object: usize) -> u32 {
    CONNECTIONS
        .with(|connections| {
            let connection = connections
                .iter_mut()
                .find(|connection| connection.fd == fd)?;
            let known = connection
                .objects
                .iter_mut()
                .find(|entry| entry.object == object);
            if let Some(entry) = known {
                entry.handed_out += 1;
                return Some((entry.id, true));
            }
            let id = connection.next;
            connection.next += 1;
            connection.objects.push(Entry {
                id,
                object,
                handed_out: 1,
            });
            Some((id, false))
        })
        .map_or(0, |(id, known)| {
            if known {
                // SAFETY: the entry keeps one reference; the caller's goes.
                unsafe { crate::com::release(object) };
            }
            id
        })
}

/// The object numbered `id` on connection `fd`.
pub(super) fn object_of(fd: usize, id: u32) -> Option<usize> {
    CONNECTIONS.with(|connections| {
        let connection = connections.iter().find(|connection| connection.fd == fd)?;
        let entry = connection.objects.iter().find(|entry| entry.id == id)?;
        Some(entry.object)
    })
}

fn serve_one(fd: usize) {
    let Some(body) = channel::receive(fd) else {
        close_connection(fd);
        return;
    };
    if wire::is_reply(&body) {
        return;
    }
    let Some(answer) = serve_request(fd, &body) else {
        close_connection(fd);
        return;
    };
    if !channel::send(fd, &answer) {
        close_connection(fd);
    }
}

/// The answer to the request `body` that arrived on connection `fd`; `None` for a malformed
/// one.
pub(super) fn serve_request(fd: usize, body: &[u8]) -> Option<Vec<u8>> {
    Some(match wire::read_request(body)? {
        Request::Create { class } => create(fd, &Guid::from_bytes(class)),
        Request::Names {
            object,
            locale,
            names,
        } => numbers_of_names(fd, object, locale, &names),
        Request::Invoke {
            object,
            member,
            locale,
            flags,
            arguments,
            named,
            by_reference,
        } => {
            let call = (member, locale, flags);
            invoke(fd, object, call, (&arguments, &named), &by_reference)
        }
        Request::Release { object } => release(fd, object),
        Request::Query { object, interface } => query(fd, object, &Guid::from_bytes(interface)),
        Request::FindPoint { object, interface } => {
            find_point(fd, object, &Guid::from_bytes(interface))
        }
        Request::Advise { object, sink } => advise(fd, object, &sink),
        Request::Unadvise { object, cookie } => unadvise(fd, object, cookie),
    })
}

fn query(fd: usize, object: u32, interface: &Guid) -> Vec<u8> {
    let Some(target) = object_of(fd, object) else {
        return wire::numbered(request::QUERY, RPC_E_DISCONNECTED, 0, false);
    };
    let mut found = 0usize;
    // SAFETY: a live object of the connection; the result is local.
    let status = unsafe { crate::com::query_interface(target, interface, &mut found) };
    if status < 0 || found == 0 {
        let status = if status < 0 { status } else { E_POINTER };
        return wire::numbered(request::QUERY, status, 0, false);
    }
    let mut dispatch = 0usize;
    // SAFETY: as above.
    let answered = unsafe { crate::com::query_interface(target, &guid::DISPATCH, &mut dispatch) };
    let same = answered >= 0 && dispatch == found;
    // SAFETY: the reference the second query gave, if any.
    unsafe { crate::com::release(dispatch) };
    match hand_out(fd, found) {
        0 => wire::numbered(request::QUERY, RPC_E_DISCONNECTED, 0, false),
        id => wire::numbered(request::QUERY, S_OK, id, same),
    }
}

fn find_point(fd: usize, object: u32, interface: &Guid) -> Vec<u8> {
    let Some(target) = object_of(fd, object) else {
        return wire::numbered(request::FIND_POINT, RPC_E_DISCONNECTED, 0, false);
    };
    let mut container = 0usize;
    // SAFETY: a live object of the connection; the result is local.
    let status =
        unsafe { crate::com::query_interface(target, &CONNECTION_POINT_CONTAINER, &mut container) };
    if status < 0 || container == 0 {
        let status = if status < 0 { status } else { E_POINTER };
        return wire::numbered(request::FIND_POINT, status, 0, false);
    }
    let mut point = 0usize;
    let arguments = [interface as *const Guid as usize, &raw mut point as usize, 0];
    // SAFETY: the container's `FindConnectionPoint`, with the interface and a local result.
    let status =
        unsafe { crate::com::call_method(container, SLOT_FIND_CONNECTION_POINT, arguments) };
    // SAFETY: the reference the query gave.
    unsafe { crate::com::release(container) };
    let status = status as u32 as i32;
    if status < 0 || point == 0 {
        let status = if status < 0 { status } else { E_POINTER };
        return wire::numbered(request::FIND_POINT, status, 0, false);
    }
    match hand_out(fd, point) {
        0 => wire::numbered(request::FIND_POINT, RPC_E_DISCONNECTED, 0, false),
        id => wire::numbered(request::FIND_POINT, S_OK, id, false),
    }
}

fn advise(fd: usize, object: u32, sink: &Value) -> Vec<u8> {
    let (Some(point), link) = (object_of(fd, object), link_of(fd)) else {
        return wire::numbered(request::ADVISE, RPC_E_DISCONNECTED, 0, false);
    };
    if link.is_null() {
        return wire::numbered(request::ADVISE, E_OUTOFMEMORY, 0, false);
    }
    let mut held = match carry::variant_of(&mut LinkObjects::new(link), sink) {
        Ok(held) => held,
        Err(status) => return wire::numbered(request::ADVISE, status, 0, false),
    };
    let mut cookie = 0u32;
    let arguments = [held.pointer(), &raw mut cookie as usize, 0];
    // SAFETY: the point's `Advise`, with the sink and a local cookie.
    let status = unsafe { crate::com::call_method(point, SLOT_ADVISE, arguments) } as u32 as i32;
    // SAFETY: this call's reference to the sink; the point keeps its own.
    unsafe { variant::clear(&mut held) };
    wire::numbered(request::ADVISE, status, cookie, false)
}

fn unadvise(fd: usize, object: u32, cookie: u32) -> Vec<u8> {
    let Some(point) = object_of(fd, object) else {
        return wire::status_reply(request::UNADVISE, RPC_E_DISCONNECTED);
    };
    // SAFETY: the point's `Unadvise` with the cookie its `Advise` gave.
    let status =
        unsafe { crate::com::call_method(point, SLOT_UNADVISE, [cookie as usize, 0, 0]) };
    wire::status_reply(request::UNADVISE, status as u32 as i32)
}

fn create(fd: usize, class: &Guid) -> Vec<u8> {
    let Some(factory) = registered_factory(class, CLSCTX_LOCAL_SERVER) else {
        return wire::created(super::REGDB_E_CLASSNOTREG, 0);
    };
    let mut object = 0usize;
    // SAFETY: a live class object with a reference this call holds; the result is local.
    let status = unsafe { create_with(factory, 0, &guid::DISPATCH, &mut object) };
    // SAFETY: the reference `registered_factory` gave.
    unsafe { crate::com::release(factory) };
    if status < 0 || object == 0 {
        return wire::created(if status < 0 { status } else { E_POINTER }, 0);
    }
    wire::created(S_OK, hand_out(fd, object))
}

fn method(object: usize, slot: usize) -> usize {
    // SAFETY: a live COM object: its first word is its table, which holds the slot.
    unsafe { (object as *const *const usize).read().add(slot).read() }
}

fn numbers_of_names(fd: usize, object: u32, locale: u32, names: &[Vec<u16>]) -> Vec<u8> {
    let Some(target) = object_of(fd, object) else {
        return wire::named(RPC_E_DISCONNECTED, &[]);
    };
    let mut texts: Vec<Vec<u16>> = Vec::new();
    for name in names {
        let mut text = name.clone();
        text.push(0);
        texts.push(text);
    }
    let pointers: Vec<*const u16> = texts.iter().map(|text| text.as_ptr()).collect();
    let mut numbers = alloc::vec![0i32; names.len()];
    let arguments = [
        target,
        &raw const NO_INTERFACE as usize,
        pointers.as_ptr() as usize,
        names.len(),
        locale as usize,
        numbers.as_mut_ptr() as usize,
        0,
        0,
        0,
    ];
    let address = method(target, SLOT_GET_IDS_OF_NAMES);
    // SAFETY: the object's `GetIDsOfNames` with the names and room for their numbers, all
    // alive across the call.
    let status = unsafe { super::super::call_program_nine(address, arguments) };
    match status {
        Some(status) => wire::named(status as u32 as i32, &numbers),
        None => wire::named(E_NOTIMPL, &[]),
    }
}

fn exception_text(record: &[u8; EXCEPTION_BYTES], at: usize) -> Vec<u16> {
    let pointer = usize::from_le_bytes(record[at..at + 8].try_into().unwrap_or_default());
    if pointer == 0 {
        return Vec::new();
    }
    // SAFETY: the program's `EXCEPINFO` holds a BSTR here, or null.
    let units = unsafe { bstr::units(pointer as *const u16) }.to_vec();
    // SAFETY: as above: the BSTR is the caller's to free.
    unsafe { bstr::free(pointer as *mut u16) };
    units
}

fn exception_of(record: &[u8; EXCEPTION_BYTES]) -> Exception {
    let word = |at: usize| u32::from_le_bytes(record[at..at + 4].try_into().unwrap_or_default());
    Exception {
        code: u16::from_le_bytes([record[0], record[1]]),
        source: exception_text(record, EXCEPTION_SOURCE),
        description: exception_text(record, EXCEPTION_DESCRIPTION),
        help_file: exception_text(record, EXCEPTION_HELP_FILE),
        help_context: word(EXCEPTION_HELP_CONTEXT),
        status: word(EXCEPTION_STATUS) as i32,
    }
}

fn failed(status: HResult) -> Vec<u8> {
    wire::invoked(&Invoked {
        status,
        result: Value::Plain {
            kind: kind::EMPTY,
            word: 0,
        },
        argument_error: 0,
        exception: None,
        changed: Vec::new(),
    })
}

fn clear_all(values: &mut [Variant]) {
    for value in values {
        // SAFETY: a VARIANT this call made and owns.
        unsafe { variant::clear(value) };
    }
}

fn invoke(
    fd: usize,
    object: u32,
    (member, locale, flags): (i32, u32, u16),
    (arguments, named): (&[Value], &[i32]),
    by_reference: &[Reference],
) -> Vec<u8> {
    let Some(target) = object_of(fd, object) else {
        return failed(RPC_E_DISCONNECTED);
    };
    if named.len() > arguments.len() {
        return failed(DISP_E_BADPARAMCOUNT);
    }
    let link = link_of(fd);
    if link.is_null() {
        return failed(E_OUTOFMEMORY);
    }
    let mut objects = LinkObjects::new(link);
    let mut values: Vec<Variant> = Vec::new();
    for argument in arguments {
        match carry::variant_of(&mut objects, argument) {
            Ok(value) => values.push(value),
            Err(status) => {
                clear_all(&mut values);
                return failed(status);
            }
        }
    }
    let mut cells: Vec<Variant> = Vec::new();
    for reference in by_reference {
        let index = usize::from(reference.index);
        let owned = core::mem::replace(&mut values[index], Variant::EMPTY);
        cells.push(owned);
    }
    for (reference, cell) in by_reference.iter().zip(cells.iter_mut()) {
        let place = carry::cell_place(reference.kind, cell);
        values[usize::from(reference.index)] =
            Variant::new(reference.kind | kind::BYREF, place as u64);
    }
    let named_numbers = match named.is_empty() {
        true => 0,
        false => named.as_ptr() as usize,
    };
    let parameters: [usize; 3] = [
        values.as_mut_ptr() as usize,
        named_numbers,
        values.len() | named.len() << 32,
    ];
    let mut result = Variant::EMPTY;
    let mut exception = [0u8; EXCEPTION_BYTES];
    let mut argument_error = 0u32;
    let call = [
        target,
        member as u32 as usize,
        &raw const NO_INTERFACE as usize,
        locale as usize,
        usize::from(flags),
        parameters.as_ptr() as usize,
        &raw mut result as usize,
        exception.as_mut_ptr() as usize,
        &raw mut argument_error as usize,
    ];
    // SAFETY: the object's `Invoke` with a `DISPPARAMS` (its arguments, named numbers and
    // counts) and room for its result, exception and argument error, all alive across the call.
    let status = unsafe { super::super::call_program_nine(method(target, SLOT_INVOKE), call) };
    for reference in by_reference {
        values[usize::from(reference.index)] = Variant::EMPTY;
    }
    clear_all(&mut values);
    let mut changed = Vec::new();
    for (reference, cell) in by_reference.iter().zip(cells.iter_mut()) {
        let mut now = carry::cell_value(reference.kind, cell);
        let carried = carry::value_of(&mut objects, &now);
        changed.push(carried.unwrap_or(Value::Plain {
            kind: kind::EMPTY,
            word: 0,
        }));
        if reference.kind == kind::DECIMAL {
            continue;
        }
        // SAFETY: the value the cell held after the call, which this call owns.
        unsafe { variant::clear(&mut now) };
        *cell = Variant::EMPTY;
    }
    let Some(status) = status.map(|status| status as u32 as i32) else {
        return failed(E_NOTIMPL);
    };
    let carried = carry::value_of(&mut objects, &result);
    // SAFETY: the result the program gave, now carried.
    unsafe { variant::clear(&mut result) };
    let exception = (status == DISP_E_EXCEPTION).then(|| exception_of(&exception));
    match carried {
        Ok(result) => wire::invoked(&Invoked {
            status,
            result,
            argument_error,
            exception,
            changed,
        }),
        Err(failure) => failed(failure),
    }
}

fn release(fd: usize, object: u32) -> Vec<u8> {
    let gone = CONNECTIONS.with(|connections| {
        let connection = connections
            .iter_mut()
            .find(|connection| connection.fd == fd)?;
        let at = connection
            .objects
            .iter()
            .position(|entry| entry.id == object)?;
        let entry = &mut connection.objects[at];
        entry.handed_out -= 1;
        match entry.handed_out {
            0 => Some(Some(connection.objects.remove(at).object)),
            _ => Some(None),
        }
    });
    match gone {
        Some(Some(object)) => {
            // SAFETY: the connection's own reference to the object, now given back.
            unsafe { crate::com::release(object) };
            wire::released(S_OK)
        }
        Some(None) => wire::released(S_OK),
        None => wire::released(RPC_E_DISCONNECTED),
    }
}

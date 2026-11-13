//! Class objects a program registers (`CoRegisterClassObject`, D400): the same process creates
//! their objects directly, and a class registered with `CLSCTX_LOCAL_SERVER` also listens on
//! its socket (`channel`) for other programs. Their requests are served on the registering
//! thread when it looks for messages, as Windows serves a single-threaded apartment's calls
//! (any thread for one registered from the multithreaded apartment): an object is made through
//! the class object's `IClassFactory` and asked for `IDispatch`, and each connection keeps the
//! objects it was given, numbered, until it releases them or goes.

use super::super::{SpinLock, thread_environment};
use super::channel;
use crate::com::guid::{self, Guid};
use crate::com::variant::{self, Variant, kind};
use crate::com::wire::{self, Exception, Invoked, Request, Value};
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
const DISP_E_TYPEMISMATCH: HResult = 0x8002_0005_u32 as i32;
const DISP_E_BADPARAMCOUNT: HResult = 0x8002_000e_u32 as i32;
const RPC_E_DISCONNECTED: HResult = 0x8001_0108_u32 as i32;
const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
static NO_INTERFACE: Guid = guid::NULL;
const SLOT_CREATE_INSTANCE: usize = 3;
const SLOT_GET_IDS_OF_NAMES: usize = 5;
const SLOT_INVOKE: usize = 6;
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
        })
    });
}

fn close_connection(fd: usize) {
    let objects = CONNECTIONS.with(|connections| {
        let at = connections
            .iter()
            .position(|connection| connection.fd == fd)?;
        Some(connections.remove(at).objects)
    });
    let _ = linux::close(fd);
    for entry in objects.into_iter().flatten() {
        // SAFETY: the connection's own reference to each object it was given.
        unsafe { crate::com::release(entry.object) };
    }
}

fn hand_out(fd: usize, object: usize) -> u32 {
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

fn object_of(fd: usize, id: u32) -> Option<usize> {
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
    let answer = match wire::read_request(&body) {
        Some(Request::Create { class }) => create(fd, &Guid::from_bytes(class)),
        Some(Request::Names {
            object,
            locale,
            names,
        }) => numbers_of_names(fd, object, locale, &names),
        Some(Request::Invoke {
            object,
            member,
            locale,
            flags,
            arguments,
            named,
        }) => invoke(fd, object, (member, locale, flags), &arguments, &named),
        Some(Request::Release { object }) => release(fd, object),
        None => {
            close_connection(fd);
            return;
        }
    };
    if !channel::send(fd, &answer) {
        close_connection(fd);
    }
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

fn variant_of(fd: usize, value: &Value) -> Result<Variant, HResult> {
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
        Value::Object { kind, id } => {
            let object = object_of(fd, *id).ok_or(DISP_E_TYPEMISMATCH)?;
            // SAFETY: a live object of the connection; the VARIANT holds its own reference.
            unsafe { crate::com::add_reference(object) };
            Variant::new(*kind, object as u64)
        }
    })
}

fn value_of(fd: usize, held: &Variant) -> Result<Value, HResult> {
    // SAFETY: a valid VARIANT the program gave back.
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
                // SAFETY: a live object the VARIANT holds; the connection takes a reference.
                unsafe { crate::com::add_reference(object) };
                Value::Object {
                    kind: held.kind,
                    id: hand_out(fd, object),
                }
            }
        },
        other => Value::Plain {
            kind: other,
            word: held.data[0],
        },
    })
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
    })
}

fn invoke(
    fd: usize,
    object: u32,
    (member, locale, flags): (i32, u32, u16),
    arguments: &[Value],
    named: &[i32],
) -> Vec<u8> {
    let Some(target) = object_of(fd, object) else {
        return failed(RPC_E_DISCONNECTED);
    };
    if named.len() > arguments.len() {
        return failed(DISP_E_BADPARAMCOUNT);
    }
    let mut values: Vec<Variant> = Vec::new();
    for argument in arguments {
        match variant_of(fd, argument) {
            Ok(value) => values.push(value),
            Err(status) => {
                for value in &mut values {
                    // SAFETY: a VARIANT this call made.
                    unsafe { variant::clear(value) };
                }
                return failed(status);
            }
        }
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
    for value in &mut values {
        // SAFETY: the arguments this call made; the callee kept its own references.
        unsafe { variant::clear(value) };
    }
    let Some(status) = status.map(|status| status as u32 as i32) else {
        return failed(E_NOTIMPL);
    };
    let carried = value_of(fd, &result);
    // SAFETY: the result the program gave, now carried.
    unsafe { variant::clear(&mut result) };
    let exception = (status == DISP_E_EXCEPTION).then(|| exception_of(&exception));
    match carried {
        Ok(result) => wire::invoked(&Invoked {
            status,
            result,
            argument_error,
            exception,
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

//! Signal-driven I/O (D701): `fcntl`'s `F_SETOWN`, `F_GETOWN`, `F_SETOWN_EX`, `F_GETOWN_EX`,
//! `F_SETSIG` and `F_GETSIG`, `O_ASYNC` (`F_SETFL` and `FIOASYNC`), the sockets' owner
//! `ioctl`s (`FIOSETOWN`, `SIOCSPGRP`, `FIOGETOWN`, `SIOCGPGRP`), and `SIGURG` to a socket's
//! owner when the peer marks urgent data.
//!
//! An open file description with an owner or `O_ASYNC` has a registration here. A file that
//! keeps a wake list (pipes, pseudo-terminals, `AF_UNIX` sockets, `inotify`) is looked at after
//! each wake of its list, which runs with no lock held, so the signal is sent by the thread that
//! made the change once its own locks are dropped, never from inside a watcher. The console and
//! `AF_INET` sockets, woken through the console's input key, are looked at when console input
//! or network traffic arrives. A registration is signalled when its file reports events it did
//! not report at the last look, and an `AF_INET` socket with data to read also after each
//! delivery, as its reads are not looked at. Files Linux sends no `SIGIO` for (regular files,
//! `epoll`, `eventfd`, timers) keep `O_ASYNC` without a signal, as on Linux. With nothing
//! registered, a wake costs the relaxed load it cost before.

use super::Errno;
use super::credentials::Kind;
use super::descriptors::{EventFile, FileKind, OpenFile};
use super::poll::{self, WakeList, event};
use super::process_table::{Running, TABLE};
use super::signal::{SIGIO, SIGURG};
use super::signals::{self, Info, code};
use crate::process::Process;
use crate::sync::SpinLock;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use eggos_hal::Platform;

/// `fcntl`'s owner and signal commands (`F_*`), the same on every architecture once its own
/// numbers are rewritten ([`crate::linux::abi`]).
pub mod command {
    pub const SETOWN: u32 = 8;
    pub const GETOWN: u32 = 9;
    pub const SETSIG: u32 = 10;
    pub const GETSIG: u32 = 11;
    pub const SETOWN_EX: u32 = 15;
    pub const GETOWN_EX: u32 = 16;
}

/// `struct f_owner_ex`'s `type` (`F_OWNER_*`).
mod owner_type {
    pub const THREAD: i32 = 0;
    pub const PROCESS: i32 = 1;
    pub const PROCESS_GROUP: i32 = 2;
}

const OWNER_EX_SIZE: usize = 8;
const LAST_SIGNAL: u32 = 64;
const LOOKED_AT: u16 = event::IN | event::OUT | event::PRI;

/// Who a file's signals go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A process (`F_SETOWN` with a positive id, `F_OWNER_PID`).
    Process(u32),
    /// Every process of a group (`F_SETOWN` with a negative id, `F_OWNER_PGRP`).
    ProcessGroup(u32),
    /// One thread (`F_OWNER_TID`).
    Thread { process: u32, thread: u32 },
}

impl Owner {
    fn number(self) -> i32 {
        match self {
            Self::Process(id) => id as i32,
            Self::ProcessGroup(group) => (group as i32).wrapping_neg(),
            Self::Thread { thread, .. } => thread as i32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetterIds {
    real: u32,
    effective: u32,
}

#[derive(Debug)]
enum Source {
    List(Arc<WakeList>),
    InputKey,
}

#[derive(Debug)]
struct Registration {
    id: u64,
    file: Weak<OpenFile>,
    source: Option<Source>,
    socket: bool,
    inet: bool,
    owner: Option<Owner>,
    setter: SetterIds,
    signal: u32,
    fd: u32,
    asynchronous: bool,
    last_events: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    List(usize),
    InputKey { delivered: bool },
}

struct Delivery {
    owner: Owner,
    setter: SetterIds,
    info: Info,
}

impl Registration {
    fn is_on(&self, list: &WakeList) -> bool {
        matches!(&self.source, Some(Source::List(own)) if core::ptr::eq(Arc::as_ptr(own), list))
    }

    fn watches_input_key(&self) -> bool {
        matches!(self.source, Some(Source::InputKey))
            && (self.asynchronous || self.inet && self.owner.is_some())
    }

    fn is_in(&self, scope: Scope) -> bool {
        match scope {
            Scope::List(list) => {
                self.asynchronous
                    && matches!(&self.source, Some(Source::List(own)) if Arc::as_ptr(own) as usize == list)
            }
            Scope::InputKey { .. } => self.watches_input_key(),
        }
    }

    fn is_needed(&self) -> bool {
        self.owner.is_some() || self.asynchronous
    }

    fn look(&mut self, now: u16, scope: Scope) -> [Option<Delivery>; 2] {
        let new = now & !self.last_events;
        self.last_events = now;
        let Some(owner) = self.owner else {
            return [None, None];
        };
        let urgent = (self.socket && new & event::PRI != 0).then_some(Delivery {
            owner,
            setter: self.setter,
            info: Info::from_kernel(SIGURG),
        });
        let still_readable = matches!(scope, Scope::InputKey { delivered: true })
            && self.inet
            && now & event::IN != 0;
        let reason = match new {
            0 if still_readable => event::IN,
            new => new,
        };
        let ready = (self.asynchronous && reason != 0).then(|| Delivery {
            owner,
            setter: self.setter,
            info: self.ready_info(reason),
        });
        [urgent, ready]
    }

    fn ready_info(&self, new: u16) -> Info {
        if self.signal == 0 {
            return Info::from_kernel(SIGIO);
        }
        let (code, band) = reason(new);
        let band = crate::linux::abi::poll_events_out(band);
        Info::ready_descriptor(self.signal, code, u64::from(band), self.fd)
    }
}

/// The `si_code` and `si_band` of a signal for events `new`, the most pressing first: an error,
/// a hang-up, urgent data, data, then room to write.
fn reason(new: u16) -> (i32, u16) {
    if new & event::ERR != 0 {
        (code::POLL_ERR, event::ERR)
    } else if new & event::HUP != 0 {
        (code::POLL_HUP, event::HUP | event::ERR)
    } else if new & event::PRI != 0 {
        (code::POLL_PRI, event::PRI | event::RDBAND)
    } else if new & event::IN != 0 {
        (code::POLL_IN, event::IN | event::RDNORM | event::MSG)
    } else {
        (code::POLL_OUT, event::OUT | event::WRNORM | event::WRBAND)
    }
}

struct Registered {
    entries: Vec<Registration>,
    next_id: u64,
}

impl Registered {
    fn position(&self, file: *const OpenFile) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| core::ptr::eq(entry.file.as_ptr(), file))
    }

    fn refresh(&self, source: Option<&Source>) {
        ANY.store(self.entries.len(), Ordering::Relaxed);
        let input = self
            .entries
            .iter()
            .filter(|entry| entry.watches_input_key())
            .count();
        INPUT_KEY_WATCHED.store(input, Ordering::Relaxed);
        if let Some(Source::List(list)) = source {
            let owned = self
                .entries
                .iter()
                .any(|entry| entry.asynchronous && entry.is_on(list));
            list.set_signals_owners(owned);
        }
    }
}

static REGISTERED: SpinLock<Registered> = SpinLock::new(Registered {
    entries: Vec::new(),
    next_id: 0,
});
static ANY: AtomicUsize = AtomicUsize::new(0);
static INPUT_KEY_WATCHED: AtomicUsize = AtomicUsize::new(0);
static LIST_LOOKER: AtomicUsize = AtomicUsize::new(0);

/// Whether any open file description has an owner or `O_ASYNC`: one relaxed load, for the
/// files' last close.
#[inline]
pub fn any() -> bool {
    ANY.load(Ordering::Relaxed) != 0
}

/// Lets the wake lists, which are not generic, look at their files on platform `P`.
pub fn set_platform<P: Platform>() {
    LIST_LOOKER.store(
        look_at_list::<P> as fn(usize) as usize,
        Ordering::Release,
    );
}

fn look_at_list<P: Platform>(list: usize) {
    scan::<P>(Scope::List(list));
}

/// `list` was woken and a registration with `O_ASYNC` is on it: its files are looked at and
/// their owners signalled. Called by [`WakeList::wake`] with no lock held.
pub fn list_woken(list: &WakeList) {
    let looker = LIST_LOOKER.load(Ordering::Acquire);
    if looker == 0 {
        return;
    }
    // SAFETY: only `set_platform` stores a non-zero value, a `fn(usize)`.
    let looker: fn(usize) = unsafe { core::mem::transmute(looker) };
    looker(list as *const WakeList as usize);
}

/// Console input or network traffic arrived (`delivered`: data reached a socket): the console
/// and `AF_INET` sockets with an owner are looked at. Call it with no lock held.
#[inline]
pub fn input_arrived<P: Platform>(delivered: bool) {
    if INPUT_KEY_WATCHED.load(Ordering::Relaxed) != 0 {
        scan::<P>(Scope::InputKey { delivered });
    }
}

/// The open file description at `file` is being dropped: its registration ends.
pub fn file_dropped(file: *const OpenFile) {
    let removed = {
        let mut registered = REGISTERED.lock();
        let Some(index) = registered.position(file) else {
            return;
        };
        let removed = registered.entries.swap_remove(index);
        registered.refresh(removed.source.as_ref());
        removed
    };
    drop(removed);
}

fn source_of(kind: &FileKind) -> Option<Source> {
    match kind {
        FileKind::Pipe(_)
        | FileKind::PseudoTerminal(_)
        | FileKind::Socket(_)
        | FileKind::Event(EventFile::Inotify(_)) => poll::wake_list(kind).map(Source::List),
        FileKind::Console | FileKind::InetSocket(_) => Some(Source::InputKey),
        _ => None,
    }
}

fn setter_ids<P: Platform>(process: &Process<P>) -> SetterIds {
    let ids = process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(Kind::User);
    SetterIds {
        real: ids.real,
        effective: ids.effective,
    }
}

/// Changes `file`'s registration with `change`, making one if it has none: `ENOMEM` without
/// memory for it. A registration left with neither an owner nor `O_ASYNC` ends.
fn update<P: Platform>(
    process: &Process<P>,
    file: &Arc<OpenFile>,
    change: impl FnOnce(&mut Registration),
) -> Result<(), Errno> {
    let now = poll::file_events::<P>(file.kind(), LOOKED_AT, &|| 0);
    let setter = setter_ids(process);
    let removed = {
        let mut registered = REGISTERED.lock();
        let index = match registered.position(Arc::as_ptr(file)) {
            Some(index) => index,
            None => {
                registered
                    .entries
                    .try_reserve(1)
                    .map_err(|_| Errno::ENOMEM)?;
                let id = registered.next_id;
                registered.next_id += 1;
                registered.entries.push(Registration {
                    id,
                    file: Arc::downgrade(file),
                    source: source_of(file.kind()),
                    socket: matches!(file.kind(), FileKind::Socket(_) | FileKind::InetSocket(_)),
                    inet: matches!(file.kind(), FileKind::InetSocket(_)),
                    owner: None,
                    setter,
                    signal: 0,
                    fd: 0,
                    asynchronous: false,
                    last_events: now,
                });
                registered.entries.len() - 1
            }
        };
        let entry = &mut registered.entries[index];
        change(entry);
        entry.last_events = now;
        let removed = match entry.is_needed() {
            true => None,
            false => Some(registered.entries.swap_remove(index)),
        };
        let source = match &removed {
            Some(removed) => removed.source.as_ref(),
            None => registered.entries[index].source.as_ref(),
        };
        let list = match source {
            Some(Source::List(list)) => Some(Source::List(Arc::clone(list))),
            _ => None,
        };
        registered.refresh(list.as_ref());
        removed
    };
    drop(removed);
    Ok(())
}

fn with_registration<T>(file: &Arc<OpenFile>, read: impl FnOnce(&Registration) -> T) -> Option<T> {
    let registered = REGISTERED.lock();
    registered
        .position(Arc::as_ptr(file))
        .map(|index| read(&registered.entries[index]))
}

/// Turns `O_ASYNC` on or off for `file`, open as descriptor `fd` (its `si_fd`), as `F_SETFL`
/// and `FIOASYNC` do. The caller sets the status flag itself.
pub fn set_asynchronous<P: Platform>(
    process: &Process<P>,
    file: &Arc<OpenFile>,
    fd: u32,
    on: bool,
) -> Result<(), Errno> {
    if !on && with_registration(file, |_| ()).is_none() {
        return Ok(());
    }
    update(process, file, |entry| {
        entry.asynchronous = on;
        entry.fd = fd;
    })
}

fn owner_of(file: &Arc<OpenFile>) -> Option<Owner> {
    with_registration(file, |entry| entry.owner).flatten()
}

fn set_owner<P: Platform>(
    process: &Process<P>,
    file: &Arc<OpenFile>,
    owner: Option<Owner>,
) -> Result<(), Errno> {
    if owner.is_none() && with_registration(file, |_| ()).is_none() {
        return Ok(());
    }
    let setter = setter_ids(process);
    update(process, file, |entry| {
        entry.owner = owner;
        entry.setter = setter;
    })
}

fn process_exists(id: u32) -> bool {
    TABLE.lock().target(id).is_some()
}

fn group_exists(group: u32) -> bool {
    let table = TABLE.lock();
    table.target(group).is_some() || !table.group_targets(group).is_empty()
}

/// The owner `F_SETOWN`, `FIOSETOWN` and `SIOCSPGRP` name with `who`: none for 0, a process for
/// a positive id, a group for a negative one. As Linux: `EINVAL` for the most negative `int`,
/// `ESRCH` when nothing has the id.
fn owner_named(who: i32) -> Result<Option<Owner>, Errno> {
    match who {
        0 => Ok(None),
        i32::MIN => Err(Errno::EINVAL),
        ..0 => {
            let group = who.unsigned_abs();
            group_exists(group)
                .then_some(Some(Owner::ProcessGroup(group)))
                .ok_or(Errno::ESRCH)
        }
        _ => {
            let id = who as u32;
            process_exists(id)
                .then_some(Some(Owner::Process(id)))
                .ok_or(Errno::ESRCH)
        }
    }
}

fn thread_named<P: Platform>(process: &Process<P>, thread: u32) -> Result<Owner, Errno> {
    let own = process.id().get();
    if thread == own || signals::has_live_thread(process, thread) {
        return Ok(Owner::Thread {
            process: own,
            thread,
        });
    }
    match process_exists(thread) {
        true => Ok(Owner::Thread {
            process: thread,
            thread,
        }),
        false => Err(Errno::ESRCH),
    }
}

fn owner_ex_named<P: Platform>(
    process: &Process<P>,
    bytes: [u8; OWNER_EX_SIZE],
) -> Result<Option<Owner>, Errno> {
    let kind = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
    let id = i32::from_ne_bytes(bytes[4..].try_into().unwrap());
    if !matches!(
        kind,
        owner_type::THREAD | owner_type::PROCESS | owner_type::PROCESS_GROUP
    ) {
        return Err(Errno::EINVAL);
    }
    if id <= 0 {
        return match id {
            0 => Ok(None),
            _ => Err(Errno::ESRCH),
        };
    }
    let id = id as u32;
    match kind {
        owner_type::THREAD => thread_named(process, id).map(Some),
        owner_type::PROCESS => process_exists(id)
            .then_some(Some(Owner::Process(id)))
            .ok_or(Errno::ESRCH),
        _ => group_exists(id)
            .then_some(Some(Owner::ProcessGroup(id)))
            .ok_or(Errno::ESRCH),
    }
}

fn owner_ex_bytes(owner: Option<Owner>) -> [u8; OWNER_EX_SIZE] {
    let (kind, id) = match owner {
        None => (owner_type::PROCESS, 0),
        Some(Owner::Process(id)) => (owner_type::PROCESS, id),
        Some(Owner::ProcessGroup(group)) => (owner_type::PROCESS_GROUP, group),
        Some(Owner::Thread { thread, .. }) => (owner_type::THREAD, thread),
    };
    let mut bytes = [0; OWNER_EX_SIZE];
    bytes[..4].copy_from_slice(&kind.to_ne_bytes());
    bytes[4..].copy_from_slice(&(id as i32).to_ne_bytes());
    bytes
}

/// `fcntl(fd, command, argument)` for the owner and signal commands ([`command`]) on `file`.
/// As Linux: `F_SETOWN` takes an `int` (see [`owner_named`]); `F_SETSIG` `EINVAL` for a signal
/// above 64, 0 meaning `SIGIO` with no `siginfo_t` details; `F_SETOWN_EX` and `F_GETOWN_EX`
/// read and write a `struct f_owner_ex` at `argument` (`EFAULT` if it is not the program's
/// memory, `EINVAL` for another `type`, `ESRCH` for an id nothing has); `ENOMEM` without memory
/// for the registration.
pub fn fcntl<P: Platform>(
    process: &Process<P>,
    file: &Arc<OpenFile>,
    command: u32,
    argument: u64,
) -> Result<u64, Errno> {
    match command {
        command::GETOWN => Ok(owner_of(file).map_or(0, Owner::number) as i64 as u64),
        command::SETOWN => {
            let owner = owner_named(argument as u32 as i32)?;
            set_owner(process, file, owner).map(|()| 0)
        }
        command::GETSIG => Ok(u64::from(
            with_registration(file, |entry| entry.signal).unwrap_or(0),
        )),
        command::SETSIG => {
            let signal = argument as u32;
            if signal > LAST_SIGNAL {
                return Err(Errno::EINVAL);
            }
            if signal == 0 && with_registration(file, |_| ()).is_none() {
                return Ok(0);
            }
            update(process, file, |entry| entry.signal = signal).map(|()| 0)
        }
        command::GETOWN_EX => {
            let bytes = owner_ex_bytes(owner_of(file));
            process
                .write_user_memory(argument, &bytes)
                .map(|()| 0)
                .map_err(|_| Errno::EFAULT)
        }
        command::SETOWN_EX => {
            let mut bytes = [0; OWNER_EX_SIZE];
            process
                .read_user_memory(argument, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            let owner = owner_ex_named(process, bytes)?;
            set_owner(process, file, owner).map(|()| 0)
        }
        _ => Err(Errno::EINVAL),
    }
}

/// The sockets' owner `ioctl`s on `file`: `FIOSETOWN` and `SIOCSPGRP` set the owner from the
/// `int` at `argument` as `F_SETOWN` does, `FIOGETOWN` and `SIOCGPGRP` write it there as
/// `F_GETOWN` reads it. `None` for another request or a file that is not a socket.
pub fn socket_ioctl<P: Platform>(
    process: &Process<P>,
    file: &Arc<OpenFile>,
    request: u32,
    argument: u64,
) -> Option<Result<u64, Errno>> {
    use crate::linux::abi::tty;
    if !matches!(file.kind(), FileKind::Socket(_) | FileKind::InetSocket(_)) {
        return None;
    }
    match request {
        tty::FIOSETOWN | tty::SIOCSPGRP => {
            let mut value = [0; 4];
            Some(
                process
                    .read_user_memory(argument, &mut value)
                    .map_err(|_| Errno::EFAULT)
                    .and_then(|()| owner_named(i32::from_ne_bytes(value)))
                    .and_then(|owner| set_owner(process, file, owner))
                    .map(|()| 0),
            )
        }
        tty::FIOGETOWN | tty::SIOCGPGRP => {
            let value = owner_of(file).map_or(0, Owner::number);
            Some(
                process
                    .write_user_memory(argument, &value.to_ne_bytes())
                    .map(|()| 0)
                    .map_err(|_| Errno::EFAULT),
            )
        }
        _ => None,
    }
}

fn scan<P: Platform>(scope: Scope) {
    let mut looked: Vec<(u64, Arc<OpenFile>)> = Vec::new();
    {
        let registered = REGISTERED.lock();
        for entry in registered.entries.iter().filter(|entry| entry.is_in(scope)) {
            if looked.try_reserve(1).is_err() {
                break;
            }
            if let Some(file) = entry.file.upgrade() {
                looked.push((entry.id, file));
            }
        }
    }
    let mut seen: Vec<(u64, u16)> = Vec::new();
    let mut deliveries: Vec<Delivery> = Vec::new();
    if seen.try_reserve_exact(looked.len()).is_ok()
        && deliveries.try_reserve_exact(looked.len() * 2).is_ok()
    {
        for (id, file) in &looked {
            seen.push((*id, poll::file_events::<P>(file.kind(), LOOKED_AT, &|| 0)));
        }
        let mut registered = REGISTERED.lock();
        for (id, now) in seen {
            if let Some(entry) = registered.entries.iter_mut().find(|entry| entry.id == id) {
                deliveries.extend(entry.look(now, scope).into_iter().flatten());
            }
        }
    }
    drop(looked);
    for delivery in &deliveries {
        deliver::<P>(delivery);
    }
}

fn deliver<P: Platform>(delivery: &Delivery) {
    match delivery.owner {
        Owner::Process(id) => {
            let running = TABLE.lock().target(id).and_then(|target| target.running);
            if let Some(running) = running {
                send_to::<P>(&running, None, delivery);
            }
        }
        Owner::ProcessGroup(group) => {
            let targets = TABLE.lock().group_targets(group);
            for running in targets.into_iter().filter_map(|target| target.running) {
                send_to::<P>(&running, None, delivery);
            }
        }
        Owner::Thread { process, thread } => {
            let running = TABLE.lock().target(process).and_then(|target| target.running);
            if let Some(running) = running {
                send_to::<P>(&running, Some(thread), delivery);
            }
        }
    }
}

fn send_to<P: Platform>(running: &Running, thread: Option<u32>, delivery: &Delivery) {
    let Some(process) = running.process.upgrade() else {
        return;
    };
    let Ok(process) = process.downcast::<Process<P>>() else {
        return;
    };
    if !may_signal(&process, delivery.setter) {
        return;
    }
    match thread {
        Some(thread) => signals::send_to_thread_from_outside::<P>(&process, thread, delivery.info),
        None => signals::send_from_outside::<P>(&process, &running.first_thread, delivery.info),
    }
}

/// Whether the owner set by a process with `setter`'s user ids may be signalled (Linux's
/// `sigio_perm`): the setter's effective id is root's, or its real or effective id is the
/// target's real or saved one.
fn may_signal<P: Platform>(process: &Process<P>, setter: SetterIds) -> bool {
    let target = process
        .credentials_and_limits()
        .lock()
        .credentials
        .ids(Kind::User);
    setter.effective == 0
        || [target.real, target.saved]
            .iter()
            .any(|&id| id == setter.effective || id == setter.real)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(owner: Option<Owner>, asynchronous: bool, inet: bool) -> Registration {
        Registration {
            id: 0,
            file: Weak::new(),
            source: Some(Source::InputKey),
            socket: true,
            inet,
            owner,
            setter: SetterIds {
                real: 1000,
                effective: 1000,
            },
            signal: 0,
            fd: 3,
            asynchronous,
            last_events: event::OUT,
        }
    }

    #[test]
    fn owners_read_back_as_f_getown_numbers_them() {
        assert_eq!(Owner::Process(42).number(), 42);
        assert_eq!(Owner::ProcessGroup(42).number(), -42);
        assert_eq!(
            Owner::Thread {
                process: 7,
                thread: 9
            }
            .number(),
            9
        );
    }

    #[test]
    fn only_new_events_signal_and_urgent_data_sends_sigurg_without_o_async() {
        let mut entry = registration(Some(Owner::Process(5)), false, true);
        let scope = Scope::InputKey { delivered: false };
        let [urgent, ready] = entry.look(event::OUT | event::PRI, scope);
        assert_eq!(urgent.map(|delivery| delivery.info.signal()), Some(SIGURG));
        assert!(ready.is_none());
        let [urgent, _] = entry.look(event::OUT | event::PRI, scope);
        assert!(urgent.is_none());
    }

    #[test]
    fn a_readable_inet_socket_is_signalled_again_after_each_delivery() {
        let mut entry = registration(Some(Owner::Process(5)), true, true);
        let delivered = Scope::InputKey { delivered: true };
        let [_, first] = entry.look(event::OUT | event::IN, delivered);
        assert_eq!(first.map(|delivery| delivery.info.signal()), Some(SIGIO));
        let [_, again] = entry.look(event::OUT | event::IN, delivered);
        assert!(again.is_some());
        let [_, quiet] = entry.look(event::OUT | event::IN, Scope::InputKey { delivered: false });
        assert!(quiet.is_none());
    }

    #[test]
    fn without_an_owner_nothing_is_sent() {
        let mut entry = registration(None, true, false);
        let [urgent, ready] = entry.look(event::IN | event::PRI, Scope::InputKey { delivered: true });
        assert!(urgent.is_none() && ready.is_none());
    }

    #[test]
    fn reasons_put_errors_first_and_give_linux_bands() {
        assert_eq!(reason(event::IN | event::ERR), (code::POLL_ERR, event::ERR));
        assert_eq!(reason(event::HUP), (code::POLL_HUP, event::HUP | event::ERR));
        assert_eq!(
            reason(event::IN),
            (code::POLL_IN, event::IN | event::RDNORM | event::MSG)
        );
        assert_eq!(
            reason(event::OUT),
            (code::POLL_OUT, event::OUT | event::WRNORM | event::WRBAND)
        );
    }

    #[test]
    fn f_owner_ex_bytes_carry_type_then_id() {
        let bytes = owner_ex_bytes(Some(Owner::ProcessGroup(12)));
        assert_eq!(i32::from_ne_bytes(bytes[..4].try_into().unwrap()), 2);
        assert_eq!(i32::from_ne_bytes(bytes[4..].try_into().unwrap()), 12);
        let none = owner_ex_bytes(None);
        assert_eq!(i32::from_ne_bytes(none[..4].try_into().unwrap()), 1);
        assert_eq!(i32::from_ne_bytes(none[4..].try_into().unwrap()), 0);
    }
}

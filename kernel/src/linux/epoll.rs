//! `epoll` (epoll(7)): `epoll_create` (x86_64 only), `epoll_create1`, `epoll_ctl`, `epoll_wait`
//! (x86_64 only), `epoll_pwait` and `epoll_pwait2`, the event loop of GLib's GIO, Qt, SDL3,
//! winit, libuv and tokio. An `epoll` instance is a file ([`super::descriptors::EventFile`]): it
//! can be watched with `poll` or by another instance, up to [`MAX_NESTING`] deep.
//!
//! Its interest list holds an item per (open file description, descriptor), as Linux keys it:
//! a descriptor duplicated or closed leaves the item to the description, which goes once the
//! last descriptor of it is closed. An item of a file that keeps a wake list (pipes, `AF_UNIX`
//! sockets, `eventfd`s) is on that list, and the file's changes queue it on the instance's
//! ready list, which never allocates (room for every item is reserved when it
//! is added); `epoll_wait` looks only at the queued items, and puts back the level-triggered
//! ones it reported, so it never looks at the whole interest list. An item of a file that can
//! become ready with nothing to wake it (timers, `signalfd`s, the console and the other files
//! woken through the console's input key, nested instances) is looked at by every wait; a wait
//! ends at the earliest timer expiry among them to look again.
//!
//! `EPOLLET` reports an item when it was woken and is ready (for the files looked at by every
//! wait, when its events or its file's count of expiries or pending signals changed since it
//! was last reported), `EPOLLONESHOT` disarms an item once reported until `EPOLL_CTL_MOD`,
//! `EPOLLEXCLUSIVE` is accepted where Linux accepts it and wakes every waiter, which Linux
//! allows, and `EPOLLWAKEUP` is ignored, as Linux does for a process without
//! `CAP_BLOCK_SUSPEND`; EggOS never suspends.

use super::descriptors::{EventFile, FileKind, OpenFile, open_file_limit, open_flag};
use super::memory_devices::MemoryDevice;
use super::poll::{self, Deadline, INPUT_EVENTS, InputFiles, Polled, Waiting, WakeList, Watcher};
use super::signals;
use super::terminal::Waited;
use super::thread::LinuxThread;
use super::{Errno, ends_in_user_space};
use crate::console;
use crate::process::Process;
use crate::sync::{LeafSpinLock, SpinLock};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::ops::ControlFlow;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use eggos_hal::Platform;
use eggos_hal::user::UserContext;

/// `epoll_create1`'s flags (`EPOLL_*`).
pub mod flag {
    use super::open_flag;

    /// The descriptor is closed by `execve`.
    pub const CLOEXEC: u32 = open_flag::CLOEXEC;
}

/// `epoll_ctl`'s operations (`EPOLL_CTL_*`).
pub mod op {
    /// Add a file's descriptor to the interest list.
    pub const ADD: u32 = 1;
    /// Remove it.
    pub const DEL: u32 = 2;
    /// Change its events and data.
    pub const MOD: u32 = 3;
}

/// The events of a `struct epoll_event` (`EPOLL*`): `poll`'s in the low 16 bits, and the
/// input flags above.
pub mod event {
    use super::poll::event;

    pub const IN: u32 = event::IN as u32;
    pub const PRI: u32 = event::PRI as u32;
    pub const OUT: u32 = event::OUT as u32;
    pub const ERR: u32 = event::ERR as u32;
    pub const HUP: u32 = event::HUP as u32;
    pub const RDNORM: u32 = event::RDNORM as u32;
    pub const RDBAND: u32 = event::RDBAND as u32;
    pub const WRNORM: u32 = event::WRNORM as u32;
    pub const WRBAND: u32 = event::WRBAND as u32;
    pub const RDHUP: u32 = event::RDHUP as u32;
    /// Wake one of several instances watching one file (each is woken here).
    pub const EXCLUSIVE: u32 = 1 << 28;
    /// Keep the machine awake while the event waits: ignored.
    pub const WAKEUP: u32 = 1 << 29;
    /// Disarm the item once it is reported.
    pub const ONESHOT: u32 = 1 << 30;
    /// Report the item once per change, not while it stays ready.
    pub const ET: u32 = 1 << 31;
}

/// The size of a `struct epoll_event`: x86_64 packs its 32-bit events and 64-bit data in 12
/// bytes, the others align the data.
pub const EVENT_SIZE: usize = if cfg!(target_arch = "x86_64") { 12 } else { 16 };

/// The longest chain of `epoll` instances watching one another, as Linux allows.
pub const MAX_NESTING: usize = 5;

const MAX_EVENTS: u64 = i32::MAX as u64 / EVENT_SIZE as u64;

const EVENTS_PER_WAIT: usize = 64;

const ALWAYS: u32 = event::ERR | event::HUP;

const WITH_EXCLUSIVE: u32 =
    event::IN | event::OUT | event::ERR | event::HUP | event::WAKEUP | event::ET | event::EXCLUSIVE;

static NESTING: SpinLock<()> = SpinLock::new(());

#[derive(Debug, Clone, Copy)]
struct Settings {
    events: u32,
    data: u64,
    armed: bool,
    last: Option<(u32, u64)>,
}

#[derive(Debug)]
struct Item {
    me: Weak<Item>,
    epoll: Weak<Epoll>,
    ready: Arc<ReadyQueue>,
    file: Weak<OpenFile>,
    key: (usize, u32),
    list: Option<Arc<WakeList>>,
    looked_at: bool,
    queued: AtomicBool,
    removed: AtomicBool,
    settings: LeafSpinLock<Settings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    Event { events: u32, data: u64, again: bool },
    Nothing,
    Gone,
}

impl Item {
    fn queue(&self) -> bool {
        if self.queued.swap(true, Ordering::AcqRel) {
            return false;
        }
        match self.me.upgrade() {
            Some(me) => {
                self.ready.push(me);
                true
            }
            None => false,
        }
    }

    fn forget_list(&self) {
        if let (Some(list), Some(me)) = (&self.list, self.me.upgrade()) {
            let watcher: Arc<dyn Watcher> = me;
            drop(list.unwatch(&watcher));
        }
    }

    fn report<P: Platform>(&self, pending: &dyn Fn() -> u64) -> Report {
        if self.removed.load(Ordering::Acquire) {
            return Report::Nothing;
        }
        let Some(file) = self.file.upgrade() else {
            return Report::Gone;
        };
        let asked = *self.settings.lock();
        if !asked.armed {
            return Report::Nothing;
        }
        let wanted = asked.events | ALWAYS;
        let returned =
            u32::from(poll::file_events::<P>(file.kind(), wanted as u16, pending)) & wanted;
        let edge = self.looked_at && asked.events & event::ET != 0;
        let count = match edge {
            true => count_of::<P>(file.kind(), pending),
            false => 0,
        };
        drop(file);
        let mut settings = self.settings.lock();
        if returned == 0 {
            settings.last = None;
            return Report::Nothing;
        }
        let changed = settings.events != asked.events || settings.data != asked.data;
        if changed || !settings.armed || (edge && settings.last == Some((returned, count))) {
            return Report::Nothing;
        }
        settings.last = Some((returned, count));
        if asked.events & event::ONESHOT != 0 {
            settings.armed = false;
        }
        Report::Event {
            events: returned,
            data: asked.data,
            again: asked.events & (event::ET | event::ONESHOT) == 0,
        }
    }

    fn would_report<P: Platform>(&self, pending: &dyn Fn() -> u64, depth: usize) -> bool {
        let Some(file) = self.file.upgrade() else {
            return false;
        };
        let asked = *self.settings.lock();
        if !asked.armed || self.removed.load(Ordering::Acquire) {
            return false;
        }
        let wanted = asked.events | ALWAYS;
        match file.kind() {
            FileKind::Event(EventFile::Epoll(inner)) => {
                wanted & event::IN != 0 && inner.has_events::<P>(pending, depth + 1)
            }
            kind => u32::from(poll::file_events::<P>(kind, wanted as u16, pending)) & wanted != 0,
        }
    }
}

fn count_of<P: Platform>(kind: &FileKind, pending: &dyn Fn() -> u64) -> u64 {
    match kind {
        FileKind::Event(EventFile::TimerFd(timer)) => timer.counted_expiries::<P>(),
        FileKind::Event(EventFile::SignalFd(signal_file)) => pending() & signal_file.mask(),
        _ => 0,
    }
}

impl Watcher for Item {
    fn woken(&self) {
        if self.looked_at || self.queue() {
            self.ready.wake_list.wake();
        }
    }

    fn epoll(&self) -> Option<Arc<Epoll>> {
        self.epoll.upgrade()
    }
}

#[derive(Debug)]
struct ReadyQueue {
    queue: LeafSpinLock<VecDeque<Arc<Item>>>,
    wake_list: Arc<WakeList>,
}

impl ReadyQueue {
    fn push(&self, item: Arc<Item>) {
        let mut queue = self.queue.lock();
        match queue.len() < queue.capacity() {
            true => queue.push_back(item),
            false => item.queued.store(false, Ordering::Release),
        }
    }

    fn pop(&self) -> Option<Arc<Item>> {
        let mut queue = self.queue.lock();
        let item = queue.pop_front()?;
        item.queued.store(false, Ordering::Release);
        Some(item)
    }
}

#[derive(Debug, Default)]
struct Interest {
    items: Vec<Arc<Item>>,
    looked_at: Vec<Arc<Item>>,
    sweep_closed_at: usize,
}

const FEWEST_ITEMS_BETWEEN_SWEEPS: usize = 64;

impl Interest {
    fn forget_closed(&mut self) {
        for item in self
            .items
            .extract_if(.., |item| item.file.strong_count() == 0)
        {
            item.removed.store(true, Ordering::Release);
            item.forget_list();
        }
        self.looked_at.retain(|item| item.file.strong_count() > 0);
    }

    fn forget_closed_when_grown(&mut self) {
        if self.items.len() >= self.sweep_closed_at {
            self.forget_closed();
            self.sweep_closed_at = (2 * self.items.len()).max(FEWEST_ITEMS_BETWEEN_SWEEPS);
        }
    }
}

/// An `epoll` instance: see the module documentation.
#[derive(Debug)]
pub struct Epoll {
    interest: SpinLock<Interest>,
    ready: Arc<ReadyQueue>,
    next_looked_at: AtomicUsize,
}

fn key_of(file: &Arc<OpenFile>, fd: u32) -> (usize, u32) {
    (Arc::as_ptr(file) as usize, fd)
}

impl Epoll {
    /// An instance with an empty interest list.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            interest: SpinLock::new(Interest::default()),
            ready: Arc::new(ReadyQueue {
                queue: LeafSpinLock::new_leaf(VecDeque::new()),
                wake_list: WakeList::shared(),
            }),
            next_looked_at: AtomicUsize::new(0),
        })
    }

    /// The threads and instances that wait for it.
    pub fn wake_list(&self) -> &Arc<WakeList> {
        &self.ready.wake_list
    }

    /// How many items its interest list holds.
    pub fn interest_count(&self) -> usize {
        self.interest.lock().items.len()
    }

    /// `EPOLL_CTL_ADD` of `target`, open as `fd`, with `events` and `data`: see [`epoll_ctl`].
    pub fn add(
        self: &Arc<Self>,
        target: &Arc<OpenFile>,
        fd: u32,
        events: u32,
        data: u64,
    ) -> Result<(), Errno> {
        let nested = match target.kind() {
            FileKind::Event(EventFile::Epoll(inner)) => Some(Arc::clone(inner)),
            _ => None,
        };
        let _nesting = nested.as_ref().map(|_| NESTING.lock());
        if let Some(inner) = &nested {
            check_nesting(self, inner)?;
        }
        if !crate::heap::may_grow_for_user(size_of::<Item>() + 4 * size_of::<usize>()) {
            return Err(Errno::ENOMEM);
        }
        let key = key_of(target, fd);
        let looked_at = poll::needs_looking_at(target.kind());
        let item = Arc::new_cyclic(|me| Item {
            me: Weak::clone(me),
            epoll: Arc::downgrade(self),
            ready: Arc::clone(&self.ready),
            file: Arc::downgrade(target),
            key,
            list: poll::wake_list(target.kind()),
            looked_at,
            queued: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            settings: LeafSpinLock::new_leaf(Settings {
                events,
                data,
                armed: true,
                last: None,
            }),
        });
        {
            let mut interest = self.interest.lock();
            interest.forget_closed_when_grown();
            let index = match interest.items.binary_search_by(|item| item.key.cmp(&key)) {
                Ok(_) => return Err(Errno::EEXIST),
                Err(index) => index,
            };
            interest.items.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
            interest
                .looked_at
                .try_reserve(1)
                .map_err(|_| Errno::ENOMEM)?;
            let items = interest.items.len() + 1;
            self.ready
                .queue
                .lock()
                .try_reserve(items)
                .map_err(|_| Errno::ENOMEM)?;
            if let Some(list) = &item.list {
                let watcher: Arc<dyn Watcher> = Arc::clone(&item) as Arc<dyn Watcher>;
                list.watch(&watcher)?;
            }
            interest.items.insert(index, Arc::clone(&item));
            if looked_at {
                interest.looked_at.push(Arc::clone(&item));
            }
        }
        if looked_at || item.queue() {
            self.ready.wake_list.wake();
        }
        Ok(())
    }

    /// `EPOLL_CTL_DEL` of `target`, open as `fd`: see [`epoll_ctl`].
    pub fn remove(&self, target: &Arc<OpenFile>, fd: u32) -> Result<(), Errno> {
        let key = key_of(target, fd);
        let item = {
            let mut interest = self.interest.lock();
            let index = interest
                .items
                .binary_search_by(|item| item.key.cmp(&key))
                .map_err(|_| Errno::ENOENT)?;
            let item = interest.items.remove(index);
            if item.looked_at {
                interest
                    .looked_at
                    .retain(|other| !Arc::ptr_eq(other, &item));
            }
            item
        };
        item.removed.store(true, Ordering::Release);
        item.forget_list();
        Ok(())
    }

    /// `EPOLL_CTL_MOD` of `target`, open as `fd`, to `events` and `data`: see [`epoll_ctl`].
    pub fn modify(
        &self,
        target: &Arc<OpenFile>,
        fd: u32,
        events: u32,
        data: u64,
    ) -> Result<(), Errno> {
        let key = key_of(target, fd);
        let item = {
            let interest = self.interest.lock();
            let index = interest
                .items
                .binary_search_by(|item| item.key.cmp(&key))
                .map_err(|_| Errno::ENOENT)?;
            let item = Arc::clone(&interest.items[index]);
            let mut settings = item.settings.lock();
            if settings.events & event::EXCLUSIVE != 0 {
                return Err(Errno::EINVAL);
            }
            *settings = Settings {
                events,
                data,
                armed: true,
                last: None,
            };
            drop(settings);
            item
        };
        if item.looked_at || item.queue() {
            self.ready.wake_list.wake();
        }
        Ok(())
    }

    fn collect<P: Platform>(&self, out: &mut [(u32, u64)], pending: &dyn Fn() -> u64) -> usize {
        let mut count = 0;
        let mut gone = false;
        let queued = self.ready.queue.lock().len();
        for _ in 0..queued {
            if count == out.len() {
                break;
            }
            let Some(item) = self.ready.pop() else {
                break;
            };
            match item.report::<P>(pending) {
                Report::Event {
                    events,
                    data,
                    again,
                } => {
                    out[count] = (events, data);
                    count += 1;
                    if again {
                        item.queue();
                    }
                }
                Report::Gone => gone = true,
                Report::Nothing => {}
            }
        }
        let looked_at = self.items_of(|interest| &interest.looked_at, |_| true);
        let start = self.next_looked_at.load(Ordering::Relaxed);
        for step in 0..looked_at.len() {
            if count == out.len() {
                break;
            }
            match looked_at[(start + step) % looked_at.len()].report::<P>(pending) {
                Report::Event { events, data, .. } => {
                    out[count] = (events, data);
                    count += 1;
                }
                Report::Gone => gone = true,
                Report::Nothing => {}
            }
        }
        if !looked_at.is_empty() {
            self.next_looked_at
                .store((start + 1) % looked_at.len(), Ordering::Relaxed);
        }
        drop(looked_at);
        if gone {
            self.forget_closed();
        }
        count
    }

    fn forget_closed(&self) {
        self.interest.lock().forget_closed();
    }

    fn has_events<P: Platform>(&self, pending: &dyn Fn() -> u64, depth: usize) -> bool {
        if depth > MAX_NESTING {
            return false;
        }
        let items = self.items_of(
            |interest| &interest.items,
            |item| item.looked_at || item.queued.load(Ordering::Acquire),
        );
        items
            .iter()
            .any(|item| item.would_report::<P>(pending, depth))
    }

    fn items_of(
        &self,
        list: impl Fn(&Interest) -> &Vec<Arc<Item>>,
        wanted: impl Fn(&Item) -> bool,
    ) -> Vec<Arc<Item>> {
        let interest = self.interest.lock();
        let mut items = Vec::new();
        let candidates = list(&interest).iter().filter(|item| wanted(item));
        if items.try_reserve_exact(candidates.clone().count()).is_ok() {
            items.extend(candidates.cloned());
        }
        drop(interest);
        items
    }

    /// The `poll` events of `requested` it reports: readable while an item would report
    /// anything. `depth` is how deep in other instances it is looked at.
    pub fn events<P: Platform>(
        &self,
        requested: u16,
        pending: &dyn Fn() -> u64,
        depth: usize,
    ) -> u16 {
        match requested & INPUT_EVENTS != 0 && self.has_events::<P>(pending, depth) {
            true => requested & INPUT_EVENTS,
            false => 0,
        }
    }

    fn looked_at_files(&self) -> Result<Vec<(Arc<OpenFile>, u32)>, Errno> {
        let interest = self.interest.lock();
        let mut files = Vec::new();
        files
            .try_reserve_exact(interest.looked_at.len())
            .map_err(|_| Errno::ENOMEM)?;
        for item in &interest.looked_at {
            if let Some(file) = item.file.upgrade() {
                files.push((file, item.settings.lock().events));
            }
        }
        drop(interest);
        Ok(files)
    }

    /// The earliest clock tick at which a file looked at by every wait may become ready with
    /// nothing to wake it ([`poll::wake_deadline`]).
    pub fn next_wake<P: Platform>(&self, depth: usize) -> Option<Deadline> {
        if depth > MAX_NESTING {
            return None;
        }
        let files = self.looked_at_files().ok()?;
        files
            .iter()
            .filter_map(|(file, _)| match file.kind() {
                FileKind::Event(EventFile::Epoll(inner)) => inner.next_wake::<P>(depth + 1),
                kind => poll::wake_deadline::<P>(kind),
            })
            .min()
    }

    /// Whether a wait for it waits on the console's input key: one of its files, or of the
    /// instances it watches, is woken through it ([`poll::uses_input_key`]).
    pub fn uses_input_key(&self, depth: usize) -> bool {
        if depth > MAX_NESTING {
            return false;
        }
        let Ok(files) = self.looked_at_files() else {
            return true;
        };
        files.iter().any(|(file, _)| match file.kind() {
            FileKind::Event(EventFile::Epoll(inner)) => inner.uses_input_key(depth + 1),
            kind => poll::uses_input_key(kind),
        })
    }

    /// Adds its files woken through the console's input key, and those of the instances it
    /// watches, to `files` ([`poll::add_input_files`]).
    pub fn add_input_files(&self, files: &mut InputFiles, depth: usize) -> Result<(), Errno> {
        if depth > MAX_NESTING {
            return Ok(());
        }
        for (file, events) in self.looked_at_files()? {
            match file.kind() {
                FileKind::Event(EventFile::Epoll(inner)) => {
                    inner.add_input_files(files, depth + 1)?;
                }
                _ => poll::add_input_files(&file, (events | ALWAYS) as u16, files)?,
            }
        }
        Ok(())
    }

    fn nested(&self) -> Result<Vec<Arc<Epoll>>, Errno> {
        let mut nested = Vec::new();
        for (file, _) in self.all_files()? {
            if let FileKind::Event(EventFile::Epoll(inner)) = file.kind() {
                nested.try_reserve(1).map_err(|_| Errno::ENOMEM)?;
                nested.push(Arc::clone(inner));
            }
        }
        Ok(nested)
    }

    fn all_files(&self) -> Result<Vec<(Arc<OpenFile>, u32)>, Errno> {
        let interest = self.interest.lock();
        let mut files = Vec::new();
        files
            .try_reserve_exact(interest.items.len())
            .map_err(|_| Errno::ENOMEM)?;
        for item in &interest.items {
            if let Some(file) = item.file.upgrade() {
                files.push((file, item.key.1));
            }
        }
        drop(interest);
        Ok(files)
    }

    fn depth_below(&self, forbidden: &Epoll, depth: usize) -> Result<usize, Errno> {
        if core::ptr::eq(self, forbidden) || depth > MAX_NESTING {
            return Err(Errno::ELOOP);
        }
        let mut deepest = 0;
        for inner in self.nested()? {
            deepest = deepest.max(inner.depth_below(forbidden, depth + 1)?);
        }
        Ok(deepest + 1)
    }

    fn depth_above(&self, depth: usize) -> Result<usize, Errno> {
        if depth > MAX_NESTING {
            return Err(Errno::ELOOP);
        }
        let mut highest = 0;
        for outer in self.ready.wake_list.epolls()? {
            highest = highest.max(outer.depth_above(depth + 1)?);
        }
        Ok(highest + 1)
    }
}

impl Drop for Epoll {
    fn drop(&mut self) {
        let interest = core::mem::take(self.interest.get_mut());
        for item in &interest.items {
            item.removed.store(true, Ordering::Release);
            item.forget_list();
        }
        let queued = core::mem::take(&mut *self.ready.queue.lock());
        drop(queued);
        drop(interest);
        self.ready.wake_list.wake();
    }
}

fn check_nesting(outer: &Epoll, inner: &Epoll) -> Result<(), Errno> {
    let below = inner.depth_below(outer, 1)?;
    let above = outer.depth_above(1)?;
    match above + below > MAX_NESTING {
        true => Err(Errno::ELOOP),
        false => Ok(()),
    }
}

fn can_be_watched(kind: &FileKind) -> bool {
    match kind {
        FileKind::Regular(_)
        | FileKind::Directory(_)
        | FileKind::Path(_)
        | FileKind::Snapshot(_)
        | FileKind::MemoryFile(_) => false,
        FileKind::MemoryDevice(device, _) => {
            matches!(device, MemoryDevice::Random | MemoryDevice::Urandom)
        }
        FileKind::Console
        | FileKind::PseudoTerminal(_)
        | FileKind::Pipe(_)
        | FileKind::Window(_)
        | FileKind::AgentChannel(_)
        | FileKind::Socket(_)
        | FileKind::InetSocket(_)
        | FileKind::Event(_) => true,
    }
}

/// `epoll_create1(flags)`: a new instance with an empty interest list, open for reading and
/// writing, its descriptor returned. As Linux: `EINVAL` for flags other than `EPOLL_CLOEXEC`,
/// `EMFILE` without a free descriptor, `ENOMEM` without memory.
pub fn epoll_create1<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    flags: u64,
) -> Result<u64, Errno> {
    let flags = flags as u32;
    if flags & !flag::CLOEXEC != 0 {
        return Err(Errno::EINVAL);
    }
    let size = size_of::<Epoll>() + size_of::<ReadyQueue>() + size_of::<WakeList>();
    if !crate::heap::may_grow_for_user(size) {
        return Err(Errno::ENOMEM);
    }
    let file = Arc::new(OpenFile::new(
        FileKind::Event(EventFile::Epoll(Epoll::new())),
        open_flag::RDWR,
    ));
    let limit = open_file_limit(process);
    let fd = thread
        .files()
        .lock()
        .open(file, flags & flag::CLOEXEC != 0, limit)?;
    Ok(u64::from(fd))
}

/// `epoll_create(size)`: [`epoll_create1`] with no flags; `size`, an `int`, must be positive
/// (`EINVAL`), and is not used otherwise, as on Linux. x86_64 only.
#[cfg(target_arch = "x86_64")]
pub fn epoll_create<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    size: u64,
) -> Result<u64, Errno> {
    if size as u32 as i32 <= 0 {
        return Err(Errno::EINVAL);
    }
    epoll_create1(process, thread, 0)
}

fn read_event<P: Platform>(process: &Process<P>, address: u64) -> Result<(u32, u64), Errno> {
    let mut bytes = [0u8; EVENT_SIZE];
    process
        .read_user_memory(address, &mut bytes)
        .map_err(|_| Errno::EFAULT)?;
    let events = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
    let data = u64::from_ne_bytes(bytes[EVENT_SIZE - 8..].try_into().unwrap());
    Ok((events, data))
}

/// `epoll_ctl(epfd, op, fd, event)`: changes the interest list of the instance `epfd` names for
/// the file `fd` names. `EPOLL_CTL_ADD` adds it with the `struct epoll_event` at `event` (its
/// events and data), `EPOLL_CTL_MOD` changes its events and data and arms it again,
/// `EPOLL_CTL_DEL` removes it (`event` is not read). An added or changed item is looked at by
/// the next wait. As Linux, in this order: `EFAULT` if `event` is not the program's memory (but
/// for `EPOLL_CTL_DEL`); `EBADF` if `epfd` or `fd` is not open (or open with `O_PATH`); `EPERM`
/// if `fd` cannot be waited for (a regular file, a directory, `/dev/null`); `EINVAL` if `epfd`
/// is not an instance or is `fd`; with `EPOLLEXCLUSIVE`, `EINVAL` for `EPOLL_CTL_MOD`, for
/// events other than `EPOLLIN`, `EPOLLOUT`, `EPOLLERR`, `EPOLLHUP`, `EPOLLWAKEUP` and
/// `EPOLLET`, or if `fd` is an instance; `ELOOP` if adding an instance would make a loop or a
/// chain deeper than [`MAX_NESTING`]; `EEXIST` to add an item there already, `ENOENT` to change
/// or remove one that is not; `EINVAL` to change an item added with `EPOLLEXCLUSIVE`; `EINVAL`
/// for another `op`; `ENOMEM` without memory. `EPOLLWAKEUP` is dropped.
pub fn epoll_ctl<P: Platform>(
    process: &Process<P>,
    thread: &LinuxThread,
    epfd: u64,
    op: u64,
    fd: u64,
    event_address: u64,
) -> Result<u64, Errno> {
    let op = op as u32;
    let asked = match op {
        op::DEL => None,
        _ => Some(read_event(process, event_address)?),
    };
    let fd = fd as u32;
    let epoll_file = thread.files().file(u64::from(epfd as u32))?;
    let target = thread.files().file(u64::from(fd))?;
    if epoll_file.is_location_only() || target.is_location_only() {
        return Err(Errno::EBADF);
    }
    if !can_be_watched(target.kind()) {
        return Err(Errno::EPERM);
    }
    let FileKind::Event(EventFile::Epoll(epoll)) = epoll_file.kind() else {
        return Err(Errno::EINVAL);
    };
    if Arc::ptr_eq(&epoll_file, &target) {
        return Err(Errno::EINVAL);
    }
    let (events, data) = asked.unwrap_or((0, 0));
    let events = events & !event::WAKEUP;
    if events & event::EXCLUSIVE != 0
        && (op == op::MOD
            || events & !WITH_EXCLUSIVE != 0
            || matches!(target.kind(), FileKind::Event(EventFile::Epoll(_))))
    {
        return Err(Errno::EINVAL);
    }
    match op {
        op::ADD => epoll.add(&target, fd, events, data),
        op::DEL => epoll.remove(&target, fd),
        op::MOD => epoll.modify(&target, fd, events, data),
        _ => Err(Errno::EINVAL),
    }?;
    Ok(0)
}

/// Gathers the events of `epoll` into `out` for `thread`, the calling thread of `process`,
/// waiting until `deadline` (`None` for no wait) if there are none: see the module
/// documentation. A signal due before or during the wait interrupts it.
pub fn wait<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    epoll: &Arc<Epoll>,
    out: &mut [(u32, u64)],
    deadline: Option<Deadline>,
) -> Polled {
    let collect = |thread: &LinuxThread, out: &mut [(u32, u64)]| {
        let pending = || signals::pending_in(process, thread, u64::MAX);
        epoll.collect::<P>(out, &pending)
    };
    let found = collect(thread, out);
    if found > 0 {
        return Polled::Ready(found as u64);
    }
    let Some(deadline) = deadline else {
        return Polled::Ready(0);
    };
    if signals::is_due(thread) {
        return Polled::Interrupted;
    }
    let by_input = epoll.uses_input_key(0);
    let key = match by_input {
        true => console::INPUT_KEY,
        false => poll::poll_key(process, thread),
    };
    let lists = [Arc::clone(epoll.wake_list())];
    let waiting = Waiting::start::<P>(key, lists.iter());
    let polled = loop {
        waiting.take_woken();
        let found = collect(thread, out);
        if found > 0 {
            break Polled::Ready(found as u64);
        }
        if P::clock_ticks() >= deadline {
            break Polled::Ready(0);
        }
        let mut until = epoll
            .next_wake::<P>(0)
            .map_or(deadline, |wake| wake.min(deadline));
        let mut input_files = InputFiles::new();
        if by_input && epoll.add_input_files(&mut input_files, 0).is_err() {
            until = until.min(poll::soon::<P>());
        }
        let until = waiting.until::<P>(until);
        let woken = || waiting.is_woken() || poll::input_files_ready(&input_files);
        match poll::wait_once::<P>(process, thread, waiting.key(), until, woken) {
            ControlFlow::Break(()) => break Polled::Ready(0),
            ControlFlow::Continue(Waited::Signalled) => break Polled::Interrupted,
            ControlFlow::Continue(Waited::Ready | Waited::TimedOut) => {}
        }
    };
    waiting.stop(lists.iter());
    polled
}

fn write_events<P: Platform>(
    process: &Process<P>,
    address: u64,
    events: &[(u32, u64)],
) -> Result<(), Errno> {
    let mut bytes = [0u8; EVENTS_PER_WAIT * EVENT_SIZE];
    for (index, (reported, data)) in events.iter().enumerate() {
        let at = index * EVENT_SIZE;
        bytes[at..at + 4].copy_from_slice(&reported.to_ne_bytes());
        bytes[at + EVENT_SIZE - 8..at + EVENT_SIZE].copy_from_slice(&data.to_ne_bytes());
    }
    process
        .write_user_memory(address, &bytes[..events.len() * EVENT_SIZE])
        .map_err(|_| Errno::EFAULT)
}

fn checked_wait<P: Platform>(
    thread: &LinuxThread,
    epfd: u64,
    events: u64,
    max: u64,
) -> Result<(Arc<Epoll>, usize), Errno> {
    let max = max as u32 as i32;
    if max <= 0 || max as u64 > MAX_EVENTS {
        return Err(Errno::EINVAL);
    }
    if !ends_in_user_space::<P>(events, max as u64 * EVENT_SIZE as u64) {
        return Err(Errno::EFAULT);
    }
    let file = thread.files().file(u64::from(epfd as u32))?;
    if file.is_location_only() {
        return Err(Errno::EBADF);
    }
    match file.kind() {
        FileKind::Event(EventFile::Epoll(epoll)) => {
            Ok((Arc::clone(epoll), (max as usize).min(EVENTS_PER_WAIT)))
        }
        _ => Err(Errno::EINVAL),
    }
}

fn wait_call<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
    [epfd, events, max]: [u64; 3],
    deadline: Option<Deadline>,
    mask: Option<u64>,
) -> ControlFlow<()> {
    let (epoll, max) = match checked_wait::<P>(thread, epfd, events, max) {
        Ok(checked) => checked,
        Err(errno) => {
            context.set_syscall_result(errno.result());
            return ControlFlow::Continue(());
        }
    };
    let thread_mask = mask.map(|mask| signals::replace_blocked(process, thread, mask));
    let mut out = [(0u32, 0u64); EVENTS_PER_WAIT];
    let out = &mut out[..max];
    let result = loop {
        match wait::<P>(process, thread, &epoll, out, deadline) {
            Polled::Interrupted => {
                context.set_syscall_result(Errno::EINTR.result());
                if signals::deliver_restoring_mask(process, thread, context, thread_mask)? {
                    return ControlFlow::Continue(());
                }
            }
            Polled::Ready(count) => {
                break write_events(process, events, &out[..count as usize]).map(|()| count);
            }
        }
    };
    if let Some(thread_mask) = thread_mask {
        signals::replace_blocked(process, thread, thread_mask);
    }
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    ControlFlow::Continue(())
}

/// `epoll_wait(epfd, events, maxevents, timeout)`: [`epoll_pwait`] with no signal mask. x86_64
/// only: AArch64's and RISC-V's C libraries call `epoll_pwait`.
#[cfg(target_arch = "x86_64")]
pub fn epoll_wait<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [epfd, events, max, timeout, ..] = context.syscall_arguments();
    let deadline = poll::milliseconds_deadline::<P>(timeout);
    wait_call(
        process,
        thread,
        context,
        [epfd, events, max],
        deadline,
        None,
    )
}

/// `epoll_pwait(epfd, events, maxevents, timeout, sigmask, sigsetsize)`: reports the events of
/// the instance `epfd` names, at most `maxevents` (up to 64 a call; the others stay for the
/// next) as `struct epoll_event`s at `events`, and returns how many; if there are none, it
/// first waits `timeout` milliseconds (an `int`: negative waits for good, 0 not at all). With a
/// `sigmask`, the signals in it are blocked instead of the thread's during the call. As Linux,
/// in this order: `EINVAL` if `sigsetsize` is not 8 and `EFAULT` if the mask is not the
/// program's memory; `EINVAL` if `maxevents` (an `int`) is not positive or asks for more than
/// `INT_MAX` bytes, `EFAULT` if they do not fit in user space, `EBADF` if `epfd` is not open,
/// `EINVAL` if it is not an instance; `EINTR` once a signal's handler starts, never restarted
/// (a signal its action discards lets the wait go on); `EFAULT` if the events cannot be
/// written. Sets the call's result.
pub fn epoll_pwait<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [epfd, events, max, timeout, mask, set_size] = context.syscall_arguments();
    let mask = match mask {
        0 => Ok(None),
        mask => signals::read_set(process, mask, set_size).map(Some),
    };
    match mask {
        Ok(mask) => {
            let deadline = poll::milliseconds_deadline::<P>(timeout);
            wait_call(
                process,
                thread,
                context,
                [epfd, events, max],
                deadline,
                mask,
            )
        }
        Err(errno) => {
            context.set_syscall_result(errno.result());
            ControlFlow::Continue(())
        }
    }
}

/// `epoll_pwait2(epfd, events, maxevents, timeout, sigmask, sigsetsize)`: [`epoll_pwait`] with
/// the `struct timespec` at `timeout` (null: wait for good), which is not written back. As
/// Linux: `EFAULT` if it is not the program's memory and `EINVAL` for negative seconds or
/// nanoseconds outside 0 to 999999999, before the mask's errors.
pub fn epoll_pwait2<P: Platform>(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [epfd, events, max, timeout, mask, set_size] = context.syscall_arguments();
    let checked = poll::read_timespec_timeout::<P>(process, timeout).and_then(|timeout| {
        let mask = match mask {
            0 => None,
            mask => Some(signals::read_set(process, mask, set_size)?),
        };
        Ok((timeout.deadline, mask))
    });
    match checked {
        Ok((deadline, mask)) => wait_call(
            process,
            thread,
            context,
            [epfd, events, max],
            deadline,
            mask,
        ),
        Err(errno) => {
            context.set_syscall_result(errno.result());
            ControlFlow::Continue(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::eventfd::EventFd;
    use crate::tests::TestPlatform;

    fn no_signals() -> u64 {
        0
    }

    fn counter_file(counter: &Arc<EventFd>) -> Arc<OpenFile> {
        Arc::new(OpenFile::new(
            FileKind::Event(EventFile::EventFd(Arc::clone(counter))),
            open_flag::RDWR,
        ))
    }

    fn epoll_file(epoll: &Arc<Epoll>) -> Arc<OpenFile> {
        Arc::new(OpenFile::new(
            FileKind::Event(EventFile::Epoll(Arc::clone(epoll))),
            open_flag::RDWR,
        ))
    }

    fn collected(epoll: &Epoll) -> Vec<(u32, u64)> {
        let mut out = [(0, 0); 8];
        let count = epoll.collect::<TestPlatform>(&mut out, &no_signals);
        out[..count].to_vec()
    }

    #[test]
    fn a_level_triggered_item_is_reported_while_ready_and_an_edge_triggered_one_once() {
        let epoll = Epoll::new();
        let counter = EventFd::new(0, false);
        let file = counter_file(&counter);
        epoll.add(&file, 4, event::IN, 44).unwrap();
        assert_eq!(epoll.add(&file, 4, event::IN, 44), Err(Errno::EEXIST));
        assert!(collected(&epoll).is_empty());
        assert!(counter.add(1));
        assert_eq!(collected(&epoll), [(event::IN, 44)]);
        assert_eq!(collected(&epoll), [(event::IN, 44)]);
        assert_eq!(counter.take(), Some(1));
        assert!(collected(&epoll).is_empty());
        epoll.modify(&file, 4, event::IN | event::ET, 45).unwrap();
        assert!(counter.add(1));
        assert_eq!(collected(&epoll), [(event::IN, 45)]);
        assert!(collected(&epoll).is_empty());
        assert!(counter.add(1));
        assert_eq!(collected(&epoll), [(event::IN, 45)]);
        epoll.remove(&file, 4).unwrap();
        assert_eq!(epoll.remove(&file, 4), Err(Errno::ENOENT));
        assert!(counter.add(1));
        assert!(collected(&epoll).is_empty());
        assert!(!counter.wake_list().is_watched());
    }

    #[test]
    fn a_one_shot_item_is_disarmed_until_changed_and_a_closed_file_leaves_the_list() {
        let epoll = Epoll::new();
        let counter = EventFd::new(1, false);
        let file = counter_file(&counter);
        epoll.add(&file, 3, event::IN | event::ONESHOT, 7).unwrap();
        assert_eq!(collected(&epoll), [(event::IN, 7)]);
        assert!(collected(&epoll).is_empty());
        epoll.modify(&file, 3, event::IN | event::OUT, 8).unwrap();
        assert_eq!(collected(&epoll), [(event::IN | event::OUT, 8)]);
        assert_eq!(epoll.interest_count(), 1);
        drop(file);
        assert!(collected(&epoll).is_empty());
        assert_eq!(epoll.interest_count(), 0);
        assert!(!counter.wake_list().is_watched());
    }

    #[test]
    fn items_of_closed_files_that_never_became_ready_leave_the_list_as_it_grows() {
        let epoll = Epoll::new();
        let counters: Vec<Arc<EventFd>> = (0..4 * FEWEST_ITEMS_BETWEEN_SWEEPS)
            .map(|_| EventFd::new(0, false))
            .collect();
        for (fd, counter) in counters.iter().enumerate() {
            let file = counter_file(counter);
            epoll.add(&file, fd as u32, event::IN, 0).unwrap();
            drop(file);
        }
        assert!(epoll.interest_count() <= FEWEST_ITEMS_BETWEEN_SWEEPS);
        let swept = counters
            .iter()
            .filter(|counter| !counter.wake_list().is_watched())
            .count();
        assert!(swept >= counters.len() - FEWEST_ITEMS_BETWEEN_SWEEPS);
    }

    #[test]
    fn instances_watch_one_another_up_to_five_deep_and_never_in_a_loop() {
        let chain: Vec<Arc<Epoll>> = (0..6).map(|_| Epoll::new()).collect();
        let files: Vec<Arc<OpenFile>> = chain.iter().map(epoll_file).collect();
        for (index, pair) in chain.windows(2).enumerate().take(4) {
            pair[0].add(&files[index + 1], 9, event::IN, 0).unwrap();
        }
        assert_eq!(chain[4].add(&files[5], 9, event::IN, 0), Err(Errno::ELOOP));
        assert_eq!(chain[3].add(&files[0], 9, event::IN, 0), Err(Errno::ELOOP));
        let counter = EventFd::new(0, false);
        let counted = counter_file(&counter);
        chain[4].add(&counted, 1, event::IN, 5).unwrap();
        assert_eq!(
            chain[0].events::<TestPlatform>(event::IN as u16, &no_signals, 0),
            0
        );
        assert!(counter.add(1));
        assert_eq!(
            chain[0].events::<TestPlatform>(event::IN as u16, &no_signals, 0),
            event::IN as u16
        );
    }

    #[test]
    fn only_files_with_a_poll_can_be_watched() {
        use crate::linux::memory_devices::MemoryDevice;
        let node = crate::vfs::root();
        assert!(!can_be_watched(&FileKind::Directory(node)));
        assert!(!can_be_watched(&FileKind::MemoryDevice(
            MemoryDevice::Null,
            node
        )));
        assert!(can_be_watched(&FileKind::MemoryDevice(
            MemoryDevice::Urandom,
            node
        )));
        assert!(can_be_watched(&FileKind::Console));
    }
}

//! Named kernel objects shared between Windows processes (D189): events, mutexes, semaphores
//! and file mappings created with a name live in one table, [`Shared`], in a file every
//! Windows process that uses a name maps. A name is `Global\` or `Local\` (either case) and the
//! rest, or the rest alone in `Local\`; one namespace holds every kind. Each [`Slot`] keeps its
//! name, kind, state and the processes holding references to it ([`Holder`]); a slot whose
//! holders are all gone is free again, and holders whose process ended are dropped by [`prune`],
//! which also abandons the mutexes such a process owned. Only the layer's lock word in the table
//! guards it; everything here works on the slots under that lock.

use crate::errors::code as error;
use core::sync::atomic::AtomicU32;

/// The UTF-16 units a name keeps after its namespace prefix.
pub const NAME_UNITS: usize = 120;
/// The processes that can hold one object at once.
pub const HOLDERS: usize = 16;
/// The named objects that can exist at once on the machine.
pub const SLOTS: usize = 128;

/// The kinds of named object, as a slot stores them.
pub mod kind {
    pub const FREE: u32 = 0;
    pub const EVENT: u32 = 1;
    pub const MUTEX: u32 = 2;
    pub const SEMAPHORE: u32 = 3;
    pub const SECTION: u32 = 4;
}

const BACKSLASH: u16 = b'\\' as u16;

/// Where a name lives: `Local\` (also a name with no prefix) or `Global\`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Local = 0,
    Global = 1,
}

/// A parsed object name: its namespace and the units after the prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name<'a> {
    pub namespace: Namespace,
    pub units: &'a [u16],
}

fn strip_prefix<'a>(name: &'a [u16], prefix: &[u8]) -> Option<&'a [u16]> {
    let head = name.get(..prefix.len())?;
    let same = head.iter().zip(prefix).all(|(unit, byte)| {
        u8::try_from(*unit).is_ok_and(|unit| unit.to_ascii_lowercase() == *byte)
    });
    same.then(|| &name[prefix.len()..])
}

/// Parses the object name `name` (no NUL, not empty): `ERROR_PATH_NOT_FOUND` for a backslash
/// after the prefix, `ERROR_INVALID_NAME` for a prefix with nothing after it and
/// `ERROR_FILENAME_EXCED_RANGE` past [`NAME_UNITS`].
pub fn parse(name: &[u16]) -> Result<Name<'_>, u32> {
    let (namespace, units) = match strip_prefix(name, b"global\\") {
        Some(rest) => (Namespace::Global, rest),
        None => match strip_prefix(name, b"local\\") {
            Some(rest) => (Namespace::Local, rest),
            None => (Namespace::Local, name),
        },
    };
    if units.contains(&BACKSLASH) {
        return Err(error::PATH_NOT_FOUND);
    }
    if units.is_empty() {
        return Err(error::INVALID_NAME);
    }
    if units.len() > NAME_UNITS {
        return Err(error::FILENAME_EXCED_RANGE);
    }
    Ok(Name { namespace, units })
}

/// A process holding `count` references (handles and views) to a slot's object.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Holder {
    pub process: u32,
    pub count: u32,
}

/// The state of a named object; each kind uses its own fields.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub signaled: u32,
    pub manual_reset: u32,
    pub count: i32,
    pub maximum: i32,
    pub owner_process: u32,
    pub owner_thread: u32,
    pub recursion: u32,
    pub abandoned: u32,
    pub size: u64,
    pub protection: u32,
    pub file_backed: u32,
}

impl State {
    pub fn event(manual_reset: bool, signaled: bool) -> Self {
        Self {
            signaled: u32::from(signaled),
            manual_reset: u32::from(manual_reset),
            ..Self::default()
        }
    }

    /// A mutex, owned once by `owner` (process, thread) when given.
    pub fn mutex(owner: Option<(u32, u32)>) -> Self {
        let (owner_process, owner_thread) = owner.unwrap_or((0, 0));
        Self {
            owner_process,
            owner_thread,
            recursion: u32::from(owner.is_some()),
            ..Self::default()
        }
    }

    pub fn semaphore(count: i32, maximum: i32) -> Self {
        Self {
            count,
            maximum,
            ..Self::default()
        }
    }

    /// A file mapping of `size` bytes with the `PAGE_*` protection `protection`, of a file or
    /// of the page file.
    pub fn section(size: u64, protection: u32, file_backed: bool) -> Self {
        Self {
            size,
            protection,
            file_backed: u32::from(file_backed),
            ..Self::default()
        }
    }
}

/// One named object in the shared table.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    pub kind: u32,
    pub namespace: u32,
    pub generation: u32,
    pub name_length: u32,
    pub name: [u16; NAME_UNITS],
    pub holders: [Holder; HOLDERS],
    pub state: State,
}

/// The shared table as the file lays it out; all zeros is an empty table.
#[repr(C)]
#[derive(Debug)]
pub struct Shared {
    pub lock: AtomicU32,
    pub last_generation: u32,
    pub slots: [Slot; SLOTS],
}

/// The bytes the shared table's file holds.
pub const SHARED_SIZE: usize = core::mem::size_of::<Shared>();

impl Slot {
    fn is_named(&self, name: &Name<'_>) -> bool {
        self.kind != kind::FREE
            && self.namespace == name.namespace as u32
            && self.name.get(..self.name_length as usize) == Some(name.units)
    }

    /// Adds a reference held by `process`; `false` when [`HOLDERS`] processes hold it already.
    pub fn hold(&mut self, process: u32) -> bool {
        let holders = &mut self.holders;
        if let Some(holder) = holders
            .iter_mut()
            .find(|holder| holder.count > 0 && holder.process == process)
        {
            holder.count += 1;
            return true;
        }
        match holders.iter_mut().find(|holder| holder.count == 0) {
            Some(holder) => {
                *holder = Holder { process, count: 1 };
                true
            }
            None => false,
        }
    }

    /// Gives back one reference `process` holds; whether that freed the slot.
    pub fn release(&mut self, process: u32) -> bool {
        if let Some(holder) = self
            .holders
            .iter_mut()
            .find(|holder| holder.count > 0 && holder.process == process)
        {
            holder.count -= 1;
        }
        self.free_if_unheld()
    }

    fn free_if_unheld(&mut self) -> bool {
        let unheld = self.kind != kind::FREE && self.holders.iter().all(|holder| holder.count == 0);
        if unheld {
            self.kind = kind::FREE;
        }
        unheld
    }

    /// Whether a wait by `thread` of `process` would be satisfied now.
    pub fn is_signaled(&self, process: u32, thread: u32) -> bool {
        let state = &self.state;
        match self.kind {
            kind::EVENT => state.signaled != 0,
            kind::MUTEX => {
                state.owner_thread == 0
                    || (state.owner_process == process && state.owner_thread == thread)
            }
            kind::SEMAPHORE => state.count > 0,
            _ => false,
        }
    }

    /// Takes the signalled object for `thread` of `process`; whether it was an abandoned mutex.
    pub fn acquire(&mut self, process: u32, thread: u32) -> bool {
        let state = &mut self.state;
        match self.kind {
            kind::EVENT if state.manual_reset == 0 => state.signaled = 0,
            kind::MUTEX => {
                state.owner_process = process;
                state.owner_thread = thread;
                state.recursion += 1;
                return core::mem::take(&mut state.abandoned) != 0;
            }
            kind::SEMAPHORE => state.count -= 1,
            _ => {}
        }
        false
    }

    /// `ReleaseMutex` by `thread` of `process`: whether the mutex is now free, or
    /// `ERROR_NOT_OWNER`.
    pub fn release_mutex(&mut self, process: u32, thread: u32) -> Result<bool, u32> {
        let state = &mut self.state;
        let owns = state.owner_process == process && state.owner_thread == thread;
        if !owns || state.recursion == 0 {
            return Err(error::NOT_OWNER);
        }
        state.recursion -= 1;
        if state.recursion == 0 {
            state.owner_process = 0;
            state.owner_thread = 0;
        }
        Ok(state.recursion == 0)
    }

    /// `ReleaseSemaphore` by `release`: the count before, or `ERROR_TOO_MANY_POSTS`.
    pub fn release_semaphore(&mut self, release: i32) -> Result<i32, u32> {
        let state = &mut self.state;
        match state.count.checked_add(release) {
            Some(new) if new <= state.maximum => Ok(core::mem::replace(&mut state.count, new)),
            _ => Err(error::TOO_MANY_POSTS),
        }
    }

    /// Abandons the mutex if `thread` of `process` (any of its threads for `None`) owns it;
    /// whether it did.
    pub fn abandon_if_owned_by(&mut self, process: u32, thread: Option<u32>) -> bool {
        let state = &mut self.state;
        let owned = self.kind == kind::MUTEX
            && state.owner_thread != 0
            && state.owner_process == process
            && thread.is_none_or(|thread| state.owner_thread == thread);
        if owned {
            state.owner_process = 0;
            state.owner_thread = 0;
            state.recursion = 0;
            state.abandoned = 1;
        }
        owned
    }
}

/// What [`create`] found: a new object, or one that already had the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    New(usize),
    Existing(usize),
}

/// Creates the object `name` of `kind` with `state`, held by `process`, or holds the existing
/// one of that name: `ERROR_INVALID_HANDLE` when the name belongs to another kind,
/// `ERROR_NOT_ENOUGH_MEMORY` when the table or the object's holders are full.
pub fn create(
    shared: &mut Shared,
    name: &Name<'_>,
    object_kind: u32,
    state: State,
    process: u32,
) -> Result<Created, u32> {
    if name.units.len() > NAME_UNITS {
        return Err(error::FILENAME_EXCED_RANGE);
    }
    if let Some(index) = shared.slots.iter().position(|slot| slot.is_named(name)) {
        let slot = &mut shared.slots[index];
        if slot.kind != object_kind {
            return Err(error::INVALID_HANDLE);
        }
        return match slot.hold(process) {
            true => Ok(Created::Existing(index)),
            false => Err(error::NOT_ENOUGH_MEMORY),
        };
    }
    let index = shared
        .slots
        .iter()
        .position(|slot| slot.kind == kind::FREE)
        .ok_or(error::NOT_ENOUGH_MEMORY)?;
    shared.last_generation = shared.last_generation.wrapping_add(1);
    let mut slot = Slot {
        kind: object_kind,
        namespace: name.namespace as u32,
        generation: shared.last_generation,
        name_length: name.units.len() as u32,
        name: [0; NAME_UNITS],
        holders: [Holder::default(); HOLDERS],
        state,
    };
    slot.name[..name.units.len()].copy_from_slice(name.units);
    slot.holders[0] = Holder { process, count: 1 };
    shared.slots[index] = slot;
    Ok(Created::New(index))
}

/// Holds the existing object `name` of `kind` for `process`: `ERROR_FILE_NOT_FOUND` when no
/// object has the name, `ERROR_INVALID_HANDLE` when it is another kind.
pub fn open(
    shared: &mut Shared,
    name: &Name<'_>,
    object_kind: u32,
    process: u32,
) -> Result<usize, u32> {
    let index = shared
        .slots
        .iter()
        .position(|slot| slot.is_named(name))
        .ok_or(error::FILE_NOT_FOUND)?;
    let slot = &mut shared.slots[index];
    if slot.kind != object_kind {
        return Err(error::INVALID_HANDLE);
    }
    match slot.hold(process) {
        true => Ok(index),
        false => Err(error::NOT_ENOUGH_MEMORY),
    }
}

/// Drops the references of processes that `alive` says ended and abandons the mutexes they
/// owned; `freed` gets each slot that has no holder left (and is free once it returns).
pub fn prune(
    shared: &mut Shared,
    alive: &mut impl FnMut(u32) -> bool,
    freed: &mut impl FnMut(&Slot),
) {
    for slot in shared
        .slots
        .iter_mut()
        .filter(|slot| slot.kind != kind::FREE)
    {
        for holder in slot.holders.iter_mut().filter(|holder| holder.count > 0) {
            if !alive(holder.process) {
                holder.count = 0;
            }
        }
        let owner = slot.state.owner_process;
        if slot.kind == kind::MUTEX && slot.state.owner_thread != 0 && !alive(owner) {
            slot.abandon_if_owned_by(owner, None);
        }
        let copy = *slot;
        if slot.free_if_unheld() {
            freed(&copy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> ([u16; 64], usize) {
        let mut units = [0u16; 64];
        let mut length = 0;
        for unit in text.encode_utf16() {
            units[length] = unit;
            length += 1;
        }
        (units, length)
    }

    fn empty() -> Shared {
        // SAFETY: all zeros is a valid, empty table (an unlocked word, free slots).
        unsafe { core::mem::zeroed() }
    }

    fn name_of(text: &str, run: impl FnOnce(&Name<'_>)) {
        let (units, length) = wide(text);
        run(&parse(&units[..length]).unwrap());
    }

    #[test]
    fn prefixes_pick_the_namespace_ignoring_case() {
        let (units, length) = wide("gLoBaL\\Egg");
        let name = parse(&units[..length]).unwrap();
        assert_eq!(name.namespace, Namespace::Global);
        assert_eq!(name.units, &units[length - 3..length]);
        let (units, length) = wide("LOCAL\\Egg");
        assert_eq!(parse(&units[..length]).unwrap().namespace, Namespace::Local);
        let (units, length) = wide("Egg");
        let name = parse(&units[..length]).unwrap();
        assert_eq!((name.namespace, name.units.len()), (Namespace::Local, 3));
    }

    #[test]
    fn a_backslash_after_the_prefix_is_a_missing_path() {
        let (units, length) = wide("Session\\1\\Egg");
        assert_eq!(parse(&units[..length]), Err(error::PATH_NOT_FOUND));
        let (units, length) = wide("Global\\a\\b");
        assert_eq!(parse(&units[..length]), Err(error::PATH_NOT_FOUND));
        let (units, length) = wide("Global\\");
        assert_eq!(parse(&units[..length]), Err(error::INVALID_NAME));
    }

    #[test]
    fn names_longer_than_a_slot_are_refused() {
        let long = [u16::from(b'a'); NAME_UNITS + 1];
        assert_eq!(parse(&long), Err(error::FILENAME_EXCED_RANGE));
        assert!(parse(&long[..NAME_UNITS]).is_ok());
    }

    #[test]
    fn creating_a_name_twice_holds_the_same_object() {
        let mut shared = empty();
        name_of("Egg", |name| {
            let first = create(&mut shared, name, kind::EVENT, State::event(true, false), 7);
            assert_eq!(first, Ok(Created::New(0)));
            let again = create(&mut shared, name, kind::EVENT, State::event(false, true), 8);
            assert_eq!(again, Ok(Created::Existing(0)));
            assert_eq!(shared.slots[0].state, State::event(true, false));
            assert_eq!(
                create(&mut shared, name, kind::MUTEX, State::mutex(None), 7),
                Err(error::INVALID_HANDLE)
            );
        });
        name_of("Global\\Egg", |name| {
            let global = create(&mut shared, name, kind::MUTEX, State::mutex(None), 7);
            assert_eq!(global, Ok(Created::New(1)));
        });
    }

    #[test]
    fn opening_needs_the_name_and_its_kind() {
        let mut shared = empty();
        name_of("Egg", |name| {
            assert_eq!(
                open(&mut shared, name, kind::EVENT, 1),
                Err(error::FILE_NOT_FOUND)
            );
            create(
                &mut shared,
                name,
                kind::SEMAPHORE,
                State::semaphore(0, 2),
                1,
            )
            .unwrap();
            assert_eq!(
                open(&mut shared, name, kind::EVENT, 2),
                Err(error::INVALID_HANDLE)
            );
            assert_eq!(open(&mut shared, name, kind::SEMAPHORE, 2), Ok(0));
        });
        let holders = &shared.slots[0].holders;
        assert_eq!(
            holders[0],
            Holder {
                process: 1,
                count: 1
            }
        );
        assert_eq!(
            holders[1],
            Holder {
                process: 2,
                count: 1
            }
        );
    }

    #[test]
    fn the_last_release_frees_the_slot_and_its_name() {
        let mut shared = empty();
        name_of("Egg", |name| {
            create(
                &mut shared,
                name,
                kind::EVENT,
                State::event(false, false),
                1,
            )
            .unwrap();
            open(&mut shared, name, kind::EVENT, 1).unwrap();
            assert!(!shared.slots[0].release(1));
            assert!(shared.slots[0].release(1));
            assert_eq!(
                open(&mut shared, name, kind::EVENT, 1),
                Err(error::FILE_NOT_FOUND)
            );
            let next = create(
                &mut shared,
                name,
                kind::EVENT,
                State::event(false, false),
                1,
            );
            assert_eq!(next, Ok(Created::New(0)));
            assert_eq!(shared.slots[0].generation, 2);
        });
    }

    #[test]
    fn a_full_table_and_full_holders_are_out_of_memory() {
        let mut shared = empty();
        for slot in shared.slots.iter_mut() {
            slot.kind = kind::EVENT;
            slot.holders[0] = Holder {
                process: 1,
                count: 1,
            };
        }
        name_of("Egg", |name| {
            assert_eq!(
                create(&mut shared, name, kind::EVENT, State::default(), 1),
                Err(error::NOT_ENOUGH_MEMORY)
            );
        });
        let slot = &mut shared.slots[0];
        for process in 2..=HOLDERS as u32 {
            assert!(slot.hold(process));
        }
        assert!(!slot.hold(99));
        assert!(slot.hold(2));
    }

    #[test]
    fn mutexes_count_recursion_and_check_their_owner() {
        let mut shared = empty();
        name_of("Egg", |name| {
            create(
                &mut shared,
                name,
                kind::MUTEX,
                State::mutex(Some((5, 50))),
                5,
            )
            .unwrap();
        });
        let slot = &mut shared.slots[0];
        assert!(slot.is_signaled(5, 50));
        assert!(!slot.is_signaled(5, 51));
        assert!(!slot.is_signaled(6, 50));
        assert!(!slot.acquire(5, 50));
        assert_eq!(slot.release_mutex(6, 50), Err(error::NOT_OWNER));
        assert_eq!(slot.release_mutex(5, 50), Ok(false));
        assert_eq!(slot.release_mutex(5, 50), Ok(true));
        assert_eq!(slot.release_mutex(5, 50), Err(error::NOT_OWNER));
        assert!(slot.is_signaled(6, 60));
    }

    #[test]
    fn events_and_semaphores_are_taken_as_their_kind_says() {
        let mut shared = empty();
        name_of("Auto", |name| {
            create(&mut shared, name, kind::EVENT, State::event(false, true), 1).unwrap();
        });
        name_of("Manual", |name| {
            create(&mut shared, name, kind::EVENT, State::event(true, true), 1).unwrap();
        });
        name_of("Count", |name| {
            create(
                &mut shared,
                name,
                kind::SEMAPHORE,
                State::semaphore(1, 2),
                1,
            )
            .unwrap();
        });
        shared.slots[0].acquire(1, 1);
        shared.slots[1].acquire(1, 1);
        assert!(!shared.slots[0].is_signaled(1, 1));
        assert!(shared.slots[1].is_signaled(1, 1));
        let semaphore = &mut shared.slots[2];
        semaphore.acquire(1, 1);
        assert!(!semaphore.is_signaled(1, 1));
        assert_eq!(semaphore.release_semaphore(2), Ok(0));
        assert_eq!(semaphore.release_semaphore(1), Err(error::TOO_MANY_POSTS));
        assert_eq!(semaphore.state.count, 2);
    }

    #[test]
    fn pruning_drops_ended_processes_and_abandons_their_mutexes() {
        let mut shared = empty();
        name_of("Held", |name| {
            create(
                &mut shared,
                name,
                kind::MUTEX,
                State::mutex(Some((9, 90))),
                9,
            )
            .unwrap();
            open(&mut shared, name, kind::MUTEX, 3).unwrap();
        });
        name_of("Gone", |name| {
            create(
                &mut shared,
                name,
                kind::SECTION,
                State::section(4096, 4, false),
                9,
            )
            .unwrap();
        });
        let mut freed_generations = [0u32; 2];
        let mut freed = 0;
        prune(&mut shared, &mut |process| process != 9, &mut |slot| {
            freed_generations[freed] = slot.generation;
            freed += 1;
        });
        assert_eq!((freed, freed_generations[0]), (1, 2));
        assert_eq!(shared.slots[1].kind, kind::FREE);
        let held = &mut shared.slots[0];
        assert_eq!(held.kind, kind::MUTEX);
        assert!(held.is_signaled(3, 30));
        assert!(held.acquire(3, 30));
        assert!(!held.abandon_if_owned_by(3, Some(31)));
        assert!(held.abandon_if_owned_by(3, Some(30)));
        assert!(held.acquire(3, 32));
    }
}

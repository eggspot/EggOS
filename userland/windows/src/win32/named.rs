//! Named objects shared between processes (D189): the table of `named_objects.rs` lives in
//! `/dev/shm/eggos-windows-objects`, which a process maps the first time it uses a name, so a
//! program that never names an object never maps it. Its lock word holds the id of the process
//! that holds it, taken over when that process has ended. Each handle or view of a named
//! object holds a [`Reference`] to its slot; creating or opening a name first drops the
//! references of ended processes. The file behind a named mapping of the page file is
//! `/dev/shm/eggos-windows-section-<generation>`, removed when its slot is freed.

use super::{SpinLock, error};
use crate::linux;
use crate::named_objects::{self, Created, Name, SHARED_SIZE, Shared, Slot, State, kind};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use eggos_runtime::errno::Errno;

const TABLE_PATH: &[u8] = b"/dev/shm/eggos-windows-objects\0";
const SECTION_PREFIX: &[u8] = b"/dev/shm/eggos-windows-section-";
const SECTION_PATH_CAPACITY: usize = 64;
const SPINS_BEFORE_CHECK: u32 = 128;

static TABLE: AtomicUsize = AtomicUsize::new(0);
static MAPPING: SpinLock<()> = SpinLock::new(());
static PROCESS: AtomicU32 = AtomicU32::new(0);

/// This process's id, as the shared table records holders and owners.
pub fn process() -> u32 {
    match PROCESS.load(Ordering::Relaxed) {
        0 => {
            let id = linux::process_id();
            PROCESS.store(id, Ordering::Relaxed);
            id
        }
        id => id,
    }
}

/// Whether process `id` still runs (a process that ended but was not reaped yet counts too).
pub fn is_alive(id: u32) -> bool {
    id == process() || !matches!(linux::kill(id, 0), Err(errno) if errno.0 == Errno::ESRCH.0)
}

fn table_address() -> Option<usize> {
    let mapped = TABLE.load(Ordering::Acquire);
    if mapped != 0 {
        return Some(mapped);
    }
    MAPPING.with(|_| {
        let mapped = TABLE.load(Ordering::Acquire);
        if mapped != 0 {
            return Some(mapped);
        }
        let length = SHARED_SIZE.next_multiple_of(linux::PAGE_SIZE);
        let fd = linux::open(TABLE_PATH, linux::O_RDWR | linux::O_CREAT).ok()?;
        let address = grow(fd, length as u64)
            .then(|| linux::map_shared(fd, length, 0).ok())
            .flatten();
        let _ = linux::close(fd);
        let address = address?;
        TABLE.store(address, Ordering::Release);
        Some(address)
    })
}

/// Makes the file `fd` at least `length` bytes long; whether it is.
pub fn grow(fd: usize, length: u64) -> bool {
    let size = linux::fstat(fd).map_or(0, |words| words[linux::STAT_SIZE_WORD]);
    size >= length || linux::truncate(fd, length).is_ok()
}

fn lock_word<'a>(address: usize) -> &'a AtomicU32 {
    // SAFETY: the table's mapping lives as long as the process, and its first word, 4-byte
    // aligned at the start of a page, is the lock, which every process reaches atomically.
    unsafe { AtomicU32::from_ptr(address as *mut u32) }
}

fn lock(address: usize) {
    let me = process();
    let mut spins = 0u32;
    loop {
        let word = lock_word(address);
        let holder = match word.compare_exchange_weak(0, me, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return,
            Err(holder) => holder,
        };
        spins = spins.wrapping_add(1);
        if !spins.is_multiple_of(SPINS_BEFORE_CHECK) {
            core::hint::spin_loop();
            continue;
        }
        let taken_over = holder != 0
            && !is_alive(holder)
            && word
                .compare_exchange(holder, me, Ordering::Acquire, Ordering::Relaxed)
                .is_ok();
        if taken_over {
            return;
        }
        linux::yield_now();
    }
}

/// Runs `work` on the shared table under its lock; `None` when the table cannot be mapped.
/// Callers never nest it, and take it only after the object table's lock (`sync.rs`).
pub fn locked<R>(work: impl FnOnce(&mut Shared) -> R) -> Option<R> {
    let address = table_address()?;
    lock(address);
    // SAFETY: the mapping holds a `Shared` (all zeros is an empty one), and the lock is held,
    // so no other thread or process changes it until it is released below.
    let result = work(unsafe { &mut *(address as *mut Shared) });
    lock_word(address).store(0, Ordering::Release);
    Some(result)
}

/// A reference that a handle or a view holds to a slot of the shared table, given back when
/// dropped.
#[derive(Debug)]
pub struct Reference {
    index: usize,
    generation: u32,
}

impl Reference {
    /// The referenced slot's index in the shared table.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Runs `work` on the referenced slot under the table's lock.
    pub fn with_slot<R>(&self, work: impl FnOnce(&mut Slot) -> R) -> Option<R> {
        locked(|shared| work(&mut shared.slots[self.index]))
    }

    /// The referenced slot of the locked table `shared`.
    pub fn slot<'a>(&self, shared: &'a mut Shared) -> &'a mut Slot {
        &mut shared.slots[self.index]
    }

    /// Another reference to the same object, for a view; `None` when its holders are full.
    pub fn another(&self) -> Option<Reference> {
        let held = self.with_slot(|slot| slot.hold(process()))?;
        held.then_some(Reference {
            index: self.index,
            generation: self.generation,
        })
    }

    /// The file behind this named mapping of the page file.
    pub fn section_path(&self) -> SectionPath {
        SectionPath::new(&[self.generation])
    }
}

impl Drop for Reference {
    fn drop(&mut self) {
        let freed = locked(|shared| {
            let slot = &mut shared.slots[self.index];
            let copy = *slot;
            slot.release(process()).then_some(copy)
        });
        if let Some(Some(slot)) = freed {
            forget(&slot);
        }
    }
}

fn forget(slot: &Slot) {
    if slot.kind == kind::SECTION && slot.state.file_backed == 0 {
        let path = SectionPath::new(&[slot.generation]);
        let _ = linux::remove(path.with_nul(), 0);
    }
}

/// A named object a process created or opened: its reference, its state then, and whether it
/// existed already.
#[derive(Debug)]
pub struct Named {
    pub reference: Reference,
    pub state: State,
    pub existed: bool,
}

fn named(shared: &Shared, index: usize, existed: bool) -> Named {
    let slot = &shared.slots[index];
    Named {
        reference: Reference {
            index,
            generation: slot.generation,
        },
        state: slot.state,
        existed,
    }
}

fn prune(shared: &mut Shared) {
    named_objects::prune(shared, &mut is_alive, &mut |slot: &Slot| forget(slot));
}

/// Creates the object `name` of `object_kind` with `state`, or holds the one of that name.
pub fn create(name: &Name<'_>, object_kind: u32, state: State) -> Result<Named, u32> {
    locked(|shared| {
        prune(shared);
        match named_objects::create(shared, name, object_kind, state, process())? {
            Created::New(index) => Ok(named(shared, index, false)),
            Created::Existing(index) => Ok(named(shared, index, true)),
        }
    })
    .unwrap_or(Err(error::NOT_ENOUGH_MEMORY))
}

/// Holds the existing object `name` of `object_kind`.
pub fn open(name: &Name<'_>, object_kind: u32) -> Result<Named, u32> {
    locked(|shared| {
        prune(shared);
        let index = named_objects::open(shared, name, object_kind, process())?;
        Ok(named(shared, index, true))
    })
    .unwrap_or(Err(error::FILE_NOT_FOUND))
}

/// Abandons the mutex in `slot` if the process owning it ended.
pub fn abandon_if_owner_ended(slot: &mut Slot) {
    let owner = slot.state.owner_process;
    if slot.kind == kind::MUTEX && slot.state.owner_thread != 0 && !is_alive(owner) {
        slot.abandon_if_owned_by(owner, None);
    }
}

/// A NUL-terminated path in `/dev/shm` for the file behind a mapping of the page file.
pub struct SectionPath {
    bytes: [u8; SECTION_PATH_CAPACITY],
    length: usize,
}

impl SectionPath {
    /// The path for an unnamed mapping: this process's id and `serial`.
    pub fn unnamed(serial: u32) -> Self {
        Self::new(&[process(), serial])
    }

    fn new(numbers: &[u32]) -> Self {
        let mut bytes = [0u8; SECTION_PATH_CAPACITY];
        bytes[..SECTION_PREFIX.len()].copy_from_slice(SECTION_PREFIX);
        let mut length = SECTION_PREFIX.len();
        for (position, number) in numbers.iter().enumerate() {
            if position > 0 {
                bytes[length] = b'-';
                length += 1;
            }
            let mut digits = [0u8; 10];
            let mut count = 0;
            let mut rest = *number;
            loop {
                digits[count] = b'0' + (rest % 10) as u8;
                count += 1;
                rest /= 10;
                if rest == 0 {
                    break;
                }
            }
            for digit in digits[..count].iter().rev() {
                bytes[length] = *digit;
                length += 1;
            }
        }
        Self { bytes, length }
    }

    pub fn with_nul(&self) -> &[u8] {
        &self.bytes[..=self.length]
    }
}

//! `futex`: threads wait until a 32-bit word in their memory changes, and are woken by
//! whoever changes it (D89, D90). Waiters are kept per word in a fixed table of
//! buckets, each a list of waiters on their own stacks, so waiting and waking never allocate.
//! A thread's exit releases the robust locks it holds ([`release_robust_list`], D93).

use super::Errno;
use super::signals::{self, Interrupted};
use super::thread::{LinuxThread, unless_signalled_or_ended};
use super::time;
use crate::process::Process;
use crate::sync::{SpinLock, SpinLockGuard};
use crate::thread::{ParkEnd, Parker};
use core::cell::UnsafeCell;
use core::ops::ControlFlow;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering, fence};
use eggos_hal::Platform;
use eggos_hal::paging::Format;
use eggos_hal::user::UserContext;

/// The operations and flags of `futex`'s `op` argument.
pub mod op {
    pub const WAIT: u32 = 0;
    pub const WAKE: u32 = 1;
    pub const REQUEUE: u32 = 3;
    pub const CMP_REQUEUE: u32 = 4;
    pub const WAIT_BITSET: u32 = 9;
    pub const WAKE_BITSET: u32 = 10;
    pub const PRIVATE_FLAG: u32 = 128;
    pub const CLOCK_REALTIME: u32 = 256;
}

const MATCH_ANY: u32 = u32::MAX;
const WORD_SIZE: u64 = 4;
const BUCKET_COUNT: usize = 256;

/// The word a waiter waits on: the address space it is in, by its process's id, which all its
/// threads share, while no memory is shared with another process, its user address, and whether
/// the futex is shared (no `FUTEX_PRIVATE_FLAG`): as on Linux, private and shared waits on one
/// word are apart, and a wake reaches only its own kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    space_and_kind: u64,
    address: u64,
}

impl Key {
    /// What [`interrupt`] leaves in the link of a waiter it unlinked: an address no word of 4
    /// bytes has, in no address space.
    const INTERRUPTED: Self = Self {
        space_and_kind: u64::MAX,
        address: u64::MAX,
    };

    pub const fn new(space: u32, address: u64, shared: bool) -> Self {
        Self {
            space_and_kind: (space as u64) << 1 | shared as u64,
            address,
        }
    }

    fn space(self) -> u32 {
        (self.space_and_kind >> 1) as u32
    }

    fn bucket_index(self) -> usize {
        let mixed =
            (self.space_and_kind << 32 ^ self.address >> 2).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        (mixed >> 56) as usize % BUCKET_COUNT
    }
}

static BUCKETS: [SpinLock<Bucket>; BUCKET_COUNT] =
    [const { SpinLock::new(Bucket { head: None }) }; BUCKET_COUNT];

struct Bucket {
    head: Option<NonNull<Waiter>>,
}

// SAFETY: the waiters a bucket links are reached only with the bucket's lock held, and each
// stays in place while it is linked: its thread returns only once it is unlinked, either by a
// waker that then unparks it and touches it no more, or by the thread itself with the lock
// held.
unsafe impl Send for Bucket {}

impl Bucket {
    fn unpark<P: Platform>(&mut self, waiter: NonNull<Waiter>) {
        // SAFETY: a waiter unlinked from this bucket with its lock still held, so its thread
        // has not returned and it is in place; only its parker's address is taken.
        let parker = unsafe { &raw const (*waiter.as_ptr()).parker };
        // SAFETY: the waiter's thread returns only once its parker is marked unparked, so the
        // parker is in place until then.
        unsafe { Parker::unpark::<P>(parker) };
    }
}

struct Waiter {
    bitset: u32,
    parker: Parker,
    bucket_index: AtomicUsize,
    thread_slot: Option<NonNull<WaitSlot>>,
    link: UnsafeCell<Link>,
}

/// Where a thread waits, while it does: the index of the bucket its waiter is in, which
/// [`requeue`] keeps up to date, so [`interrupt`] finds the waiter without knowing its word.
/// A Linux thread has one for its life (each wait it makes passes it to
/// [`wait_interruptibly`]).
#[derive(Debug)]
pub struct WaitSlot {
    bucket_index: AtomicUsize,
}

impl WaitSlot {
    const NOT_WAITING: usize = usize::MAX;

    pub const fn new() -> Self {
        Self {
            bucket_index: AtomicUsize::new(Self::NOT_WAITING),
        }
    }
}

impl Default for WaitSlot {
    fn default() -> Self {
        Self::new()
    }
}

struct Link {
    next: Option<NonNull<Waiter>>,
    queued: bool,
    key: Key,
}

impl Waiter {
    fn lock_current_bucket(&self) -> SpinLockGuard<'static, Bucket> {
        loop {
            let index = self.bucket_index.load(Ordering::Acquire);
            let bucket = BUCKETS[index].lock();
            if self.bucket_index.load(Ordering::Relaxed) == index {
                return bucket;
            }
        }
    }
}

impl WaitSlot {
    fn lock_current_bucket(&self) -> Option<SpinLockGuard<'static, Bucket>> {
        loop {
            let index = self.bucket_index.load(Ordering::Acquire);
            let bucket = BUCKETS.get(index)?.lock();
            if self.bucket_index.load(Ordering::Relaxed) == index {
                return Some(bucket);
            }
        }
    }
}

impl Bucket {
    fn link(&mut self, waiter: NonNull<Waiter>) -> &mut Link {
        // SAFETY: a waiter in this bucket or about to be, in place while the bucket's lock is
        // held; only its link changes, inside its `UnsafeCell`.
        let cell = unsafe { &(*waiter.as_ptr()).link };
        // SAFETY: only this bucket's lock holder touches the link, which `&mut self` proves
        // the caller is, and the reference ends with that borrow.
        unsafe { &mut *cell.get() }
    }

    fn push_back(&mut self, waiter: NonNull<Waiter>, key: Key) {
        *self.link(waiter) = Link {
            next: None,
            queued: true,
            key,
        };
        let mut last = None;
        let mut following = self.head;
        while let Some(candidate) = following {
            last = Some(candidate);
            following = self.link(candidate).next;
        }
        match last {
            Some(last) => self.link(last).next = Some(waiter),
            None => self.head = Some(waiter),
        }
    }

    fn unlink(&mut self, previous: Option<NonNull<Waiter>>, waiter: NonNull<Waiter>) {
        let next = self.link(waiter).next;
        match previous {
            Some(previous) => self.link(previous).next = next,
            None => self.head = next,
        }
        self.link(waiter).queued = false;
    }

    fn remove(&mut self, waiter: NonNull<Waiter>) {
        let mut previous = None;
        let mut following = self.head;
        while let Some(candidate) = following {
            if candidate == waiter {
                self.unlink(previous, waiter);
                return;
            }
            previous = Some(candidate);
            following = self.link(candidate).next;
        }
    }
}

/// Waits on `key` until a [`wake`] whose bitset shares a bit with `bitset` takes this
/// waiter (on `key`, or on the word a [`requeue`] moved it to), or the clock reaches
/// `deadline` ([`Platform::clock_ticks`], `u64::MAX` for none). `still_expected` runs first,
/// with the word's bucket locked so no wake can slip in between: its error, such as `EAGAIN`
/// when the word no longer holds the expected value, ends the call at once. `ETIMEDOUT` at the
/// deadline. Call it as a blocking system call runs: with interrupts masked and no lock held.
pub fn wait<P: Platform>(
    key: Key,
    bitset: u32,
    deadline: u64,
    still_expected: impl FnOnce() -> Result<(), Errno>,
) -> Result<u64, Errno> {
    wait_in::<P, false>(key, bitset, deadline, None, still_expected)
}

/// Waits as [`wait`] does, or until [`interrupt`] ends the wait through `slot`, the waiting
/// thread's: `EINTR` then. `still_expected` runs after `slot` shows where the thread waits, so
/// an [`interrupt`] after anything it reads finds the waiter.
pub fn wait_interruptibly<P: Platform>(
    key: Key,
    bitset: u32,
    deadline: u64,
    slot: &WaitSlot,
    still_expected: impl FnOnce() -> Result<(), Errno>,
) -> Result<u64, Errno> {
    slot.bucket_index
        .store(key.bucket_index(), Ordering::Relaxed);
    fence(Ordering::SeqCst);
    let waited = wait_in::<P, true>(key, bitset, deadline, Some(slot), still_expected);
    slot.bucket_index
        .store(WaitSlot::NOT_WAITING, Ordering::Relaxed);
    waited
}

#[inline(always)]
fn wait_in<P: Platform, const INTERRUPTIBLE: bool>(
    key: Key,
    bitset: u32,
    deadline: u64,
    thread_slot: Option<&WaitSlot>,
    still_expected: impl FnOnce() -> Result<(), Errno>,
) -> Result<u64, Errno> {
    let index = key.bucket_index();
    let waiter = Waiter {
        bitset,
        parker: Parker::new(),
        bucket_index: AtomicUsize::new(index),
        thread_slot: thread_slot.map(NonNull::from),
        link: UnsafeCell::new(Link {
            next: None,
            queued: false,
            key,
        }),
    };
    let pointer = NonNull::from(&waiter);
    {
        let mut bucket = BUCKETS[index].lock();
        still_expected()?;
        bucket.push_back(pointer, key);
    }
    if waiter.parker.park_until::<P>(deadline) == ParkEnd::DeadlineReached {
        let mut bucket = waiter.lock_current_bucket();
        if bucket.link(pointer).queued {
            bucket.remove(pointer);
            return Err(Errno::ETIMEDOUT);
        }
    }
    // SAFETY: the waiter is unlinked (its parker was unparked, or its bucket showed it not
    // queued), so nothing but this thread reaches its link any more, and whoever unlinked it
    // wrote the link before it unparked the parker or released the bucket's lock, which this
    // thread has observed since.
    let interrupted = INTERRUPTIBLE && unsafe { (*waiter.link.get()).key } == Key::INTERRUPTED;
    match interrupted {
        true => Err(Errno::EINTR),
        false => Ok(0),
    }
}

/// Ends the wait of the thread whose slot `slot` is, if it waits ([`wait`] returns `EINTR`):
/// for a signal sent to it, which it then looks at. Call it after whatever the waiter's
/// `still_expected` reads was changed: a wait that began before is ended, and one that
/// begins after sees the change.
pub fn interrupt<P: Platform>(slot: &WaitSlot) {
    fence(Ordering::SeqCst);
    let Some(mut bucket) = slot.lock_current_bucket() else {
        return;
    };
    let mut previous = None;
    let mut following = bucket.head;
    while let Some(candidate) = following {
        following = bucket.link(candidate).next;
        // SAFETY: a waiter in the bucket, in place while the bucket's lock is held; its
        // thread's slot pointer is only compared.
        let thread_slot = unsafe { candidate.as_ref() }.thread_slot;
        if thread_slot == Some(NonNull::from(slot)) {
            bucket.unlink(previous, candidate);
            bucket.link(candidate).key = Key::INTERRUPTED;
            bucket.unpark::<P>(candidate);
            return;
        }
        previous = Some(candidate);
    }
}

/// Wakes the waiters on `key` whose bitset shares a bit with `bitset`, in the order they
/// began to wait, at most `limit` of them; returns how many it woke.
pub fn wake<P: Platform>(key: Key, bitset: u32, limit: u32) -> u32 {
    let mut bucket = BUCKETS[key.bucket_index()].lock();
    let mut woken = 0;
    let mut previous = None;
    let mut following = bucket.head;
    while woken < limit
        && let Some(candidate) = following
    {
        let link = bucket.link(candidate);
        following = link.next;
        // SAFETY: a waiter in the bucket, in place while the bucket's lock is held; its
        // bitset is only read.
        let waiter_bitset = unsafe { candidate.as_ref() }.bitset;
        if link.key == key && waiter_bitset & bitset != 0 {
            bucket.unlink(previous, candidate);
            bucket.unpark::<P>(candidate);
            woken += 1;
        } else {
            previous = Some(candidate);
        }
    }
    woken
}

/// Wakes every waiter in address space `space` (see [`Key::new`]), on any word, as an ended
/// process's threads must stop waiting; returns how many it woke. Each wakes as from a
/// [`wake`].
pub fn wake_space<P: Platform>(space: u32) -> u32 {
    let mut woken = 0;
    for bucket in &BUCKETS {
        let mut bucket = bucket.lock();
        let mut previous = None;
        let mut following = bucket.head;
        while let Some(candidate) = following {
            let link = bucket.link(candidate);
            following = link.next;
            if link.key.space() == space {
                bucket.unlink(previous, candidate);
                bucket.unpark::<P>(candidate);
                woken += 1;
            } else {
                previous = Some(candidate);
            }
        }
    }
    woken
}

/// Wakes at most `wake_limit` of the waiters on `from`, in the order they began to wait, and
/// moves at most `requeue_limit` of the next ones to `to`, behind its own waiters and in the
/// same order, whatever their bitsets; returns how many it woke and how many it moved.
/// `still_expected` runs first, with both words' buckets locked: its error ends the call
/// before anything moves.
pub fn requeue<P: Platform>(
    from: Key,
    to: Key,
    wake_limit: u32,
    requeue_limit: u32,
    still_expected: impl FnOnce() -> Result<(), Errno>,
) -> Result<(u32, u32), Errno> {
    let (from_index, to_index) = (from.bucket_index(), to.bucket_index());
    let (mut from_bucket, mut to_bucket) = match from_index.cmp(&to_index) {
        core::cmp::Ordering::Equal => (BUCKETS[from_index].lock(), None),
        core::cmp::Ordering::Less => {
            let first = BUCKETS[from_index].lock();
            (first, Some(BUCKETS[to_index].lock()))
        }
        core::cmp::Ordering::Greater => {
            let first = BUCKETS[to_index].lock();
            (BUCKETS[from_index].lock(), Some(first))
        }
    };
    still_expected()?;
    let (mut woken, mut requeued) = (0, 0);
    let mut previous = None;
    let mut following = from_bucket.head;
    while (woken < wake_limit || requeued < requeue_limit)
        && let Some(candidate) = following
    {
        let link = from_bucket.link(candidate);
        following = link.next;
        if link.key != from {
            previous = Some(candidate);
            continue;
        }
        if woken == wake_limit && from == to {
            previous = Some(candidate);
            requeued += 1;
            continue;
        }
        from_bucket.unlink(previous, candidate);
        if woken < wake_limit {
            from_bucket.unpark::<P>(candidate);
            woken += 1;
            continue;
        }
        // SAFETY: a waiter that was in the bucket, in place while the bucket's lock is held;
        // its bucket index is atomic.
        let waiter = unsafe { candidate.as_ref() };
        waiter.bucket_index.store(to_index, Ordering::Release);
        if let Some(thread_slot) = waiter.thread_slot {
            // SAFETY: the slot of the waiter's thread outlives the wait
            // ([`wait_interruptibly`]); its bucket index is atomic.
            unsafe { thread_slot.as_ref() }
                .bucket_index
                .store(to_index, Ordering::Release);
        }
        match &mut to_bucket {
            Some(bucket) => bucket.push_back(candidate, to),
            None => from_bucket.push_back(candidate, to),
        }
        requeued += 1;
    }
    Ok((woken, requeued))
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`: `FUTEX_WAIT`, `FUTEX_WAKE`,
/// `FUTEX_REQUEUE`, `FUTEX_CMP_REQUEUE`, `FUTEX_WAIT_BITSET` and `FUTEX_WAKE_BITSET`, private
/// or shared, with Linux's checks in Linux's order: a wait's timeout first, then the operation
/// and its flags (`ENOSYS`), a requeue's negative counts, the bitset and each word's alignment
/// (`EINVAL`), then the words themselves (`EFAULT`, then `EAGAIN`). A requeue's second count
/// is the `timeout` argument's low 32 bits, and it returns how many it woke and moved
/// together. The other operations are `ENOSYS`.
///
/// A wait ends when its process ends, and when a signal is due to the waiting thread,
/// `thread`, as on Linux: if a handler starts, a wait with no timeout is made again once it
/// returns if its action has [`super::signals::flag::RESTART`], and fails with `EINTR`
/// otherwise; a wait with a timeout fails with `EINTR`. If no handler starts (the signals
/// were discarded), the wait goes on, its word checked again, to the same deadline. With the
/// program's registers `context`, whose result it sets; breaks when the process ends.
pub fn futex<P: Platform>(
    process: &Process<P>,
    thread: &mut LinuxThread,
    context: &mut UserContext,
) -> ControlFlow<()> {
    let [
        address,
        operation,
        value,
        timeout,
        second_address,
        third_value,
    ] = context.syscall_arguments();
    let operation = operation as u32;
    let command = operation & !(op::PRIVATE_FLAG | op::CLOCK_REALTIME);
    let deadline = match (command, timeout) {
        (op::WAIT | op::WAIT_BITSET, 0) => Ok(u64::MAX),
        (op::WAIT, _) => time::deadline_after::<P>(process, timeout),
        (op::WAIT_BITSET, _) => time::deadline_at::<P>(process, timeout),
        _ => Ok(u64::MAX),
    };
    let call = Call {
        address,
        command,
        shared: operation & op::PRIVATE_FLAG == 0,
        clock_realtime: operation & op::CLOCK_REALTIME != 0,
        value,
        timeout,
        second_address,
        third_value,
    };
    let result = loop {
        match deadline.and_then(|deadline| call.run::<P>(process, thread, deadline)) {
            Err(Errno::EINTR) => {}
            result => break result,
        }
        if process.has_ended() {
            return ControlFlow::Break(());
        }
        signals::take_in_sent(process, thread);
        if !signals::is_due(thread) {
            continue;
        }
        let interrupted = match timeout {
            0 => Interrupted::RestartsIfAsked {
                number: context.syscall_number(),
                first_argument: address,
            },
            _ => Interrupted::Fails,
        };
        if signals::deliver_interrupting(process, thread, context, interrupted)? {
            return ControlFlow::Continue(());
        }
    };
    context.set_syscall_result(result.unwrap_or_else(Errno::result));
    signals::deliver_sent(process, thread, context)
}

/// One `futex` call's arguments, decoded.
#[derive(Debug, Clone, Copy)]
struct Call {
    address: u64,
    command: u32,
    shared: bool,
    clock_realtime: bool,
    value: u64,
    timeout: u64,
    second_address: u64,
    third_value: u64,
}

impl Call {
    /// Runs the call, a wait ending at `deadline`, for `thread` of `process`: `EINTR` when
    /// the wait was interrupted ([`interrupt`]) or a signal was sent before it began.
    fn run<P: Platform>(
        self,
        process: &Process<P>,
        thread: &LinuxThread,
        deadline: u64,
    ) -> Result<u64, Errno> {
        let Self {
            address,
            command,
            shared,
            clock_realtime,
            value,
            timeout,
            second_address,
            third_value,
        } = self;
        let bitset = match command {
            op::WAIT | op::WAKE | op::REQUEUE | op::CMP_REQUEUE => MATCH_ANY,
            op::WAIT_BITSET | op::WAKE_BITSET => third_value as u32,
            _ => return Err(Errno::ENOSYS),
        };
        if clock_realtime && command != op::WAIT_BITSET {
            return Err(Errno::ENOSYS);
        }
        if bitset == 0 {
            return Err(Errno::EINVAL);
        }
        let key = |address| Key::new(process.id().get(), address, shared);
        match command {
            op::WAKE | op::WAKE_BITSET => {
                check_word(process, address, shared)?;
                Ok(u64::from(wake::<P>(
                    key(address),
                    bitset,
                    wake_limit(value),
                )))
            }
            op::REQUEUE | op::CMP_REQUEUE => {
                let (Ok(wake_limit), Ok(requeue_limit)) =
                    (u32::try_from(value as i32), u32::try_from(timeout as i32))
                else {
                    return Err(Errno::EINVAL);
                };
                check_word(process, address, shared)?;
                check_word(process, second_address, shared)?;
                let (woken, requeued) = requeue::<P>(
                    key(address),
                    key(second_address),
                    wake_limit,
                    requeue_limit,
                    || match command {
                        op::CMP_REQUEUE => expect_word(process, address, third_value),
                        _ => Ok(()),
                    },
                )?;
                Ok(u64::from(woken) + u64::from(requeued))
            }
            _ => {
                if !address.is_multiple_of(WORD_SIZE) {
                    return Err(Errno::EINVAL);
                }
                let unless_signalled_or_ended = unless_signalled_or_ended(process, thread);
                wait_interruptibly::<P>(key(address), bitset, deadline, thread.wait_slot(), || {
                    expect_word(process, address, value)?;
                    unless_signalled_or_ended()
                })
            }
        }
    }
}

/// The bits of a robust lock's word ([`release_robust_list`]): a waiter may be blocked on it,
/// its owner died holding it, and the owner's thread id.
pub mod robust {
    pub const WAITERS: u32 = 0x8000_0000;
    pub const OWNER_DIED: u32 = 0x4000_0000;
    pub const OWNER: u32 = 0x3fff_ffff;
    /// At most this many locks on one list are released, so a list that loops ends.
    pub const LIST_LIMIT: usize = 2048;
    /// Set in a pointer to a lock entry, the lock is a priority-inheritance one.
    pub const PRIORITY_INHERITANCE: u64 = 1;
}

/// Releases the robust locks thread `tid` of `process` holds as it exits, from the list whose
/// head (Linux's `struct robust_list_head`: the first entry, the signed offset from each entry
/// to its lock word, the entry being taken or released) is at `head`, as Linux: each lock word
/// whose owner is `tid` keeps its waiters bit, gets the owner-died bit and no owner, and one
/// waiter on it as a shared futex is woken (none for a priority-inheritance lock); every entry
/// up to [`robust::LIST_LIMIT`], until the list comes back to its head, then the pending
/// entry. A word or pointer that is not the program's memory ends the walk.
pub fn release_robust_list<P: Platform>(process: &Process<P>, tid: u32, head: u64) {
    let Ok([first, offset, pending]) = read_user_words::<P, 3>(process, head) else {
        return;
    };
    let address = |entry: u64| entry & !robust::PRIORITY_INHERITANCE;
    let mut entry = first;
    for _ in 0..robust::LIST_LIMIT {
        if address(entry) == head {
            break;
        }
        let Ok([next]) = read_user_words::<P, 1>(process, address(entry)) else {
            return;
        };
        if address(entry) != address(pending)
            && release_robust_lock(process, tid, entry, offset).is_err()
        {
            return;
        }
        entry = next;
    }
    if address(pending) != 0 {
        let _ = release_robust_lock(process, tid, pending, offset);
    }
}

fn release_robust_lock<P: Platform>(
    process: &Process<P>,
    tid: u32,
    entry: u64,
    offset: u64,
) -> Result<(), Errno> {
    let address = (entry & !robust::PRIORITY_INHERITANCE).wrapping_add(offset);
    if !address.is_multiple_of(WORD_SIZE)
        || address > <P::PageTableFormat as Format>::USER_END - WORD_SIZE
    {
        return Err(Errno::EFAULT);
    }
    let released = process
        .with_user_word(address, true, |word| {
            word.try_update(Ordering::SeqCst, Ordering::Relaxed, |value| {
                (value & robust::OWNER == tid)
                    .then_some(value & robust::WAITERS | robust::OWNER_DIED)
            })
        })
        .or_else(
            |_| match read_word(process, address)? & robust::OWNER == tid {
                true => Err(Errno::EFAULT),
                false => Ok(Err(0)),
            },
        )?;
    if let Ok(old) = released
        && old & robust::WAITERS != 0
        && entry & robust::PRIORITY_INHERITANCE == 0
    {
        wake::<P>(Key::new(process.id().get(), address, true), MATCH_ANY, 1);
    }
    Ok(())
}

fn read_user_words<P: Platform, const N: usize>(
    process: &Process<P>,
    address: u64,
) -> Result<[u64; N], Errno> {
    let mut bytes = [[0; 8]; N];
    process
        .read_user_memory(address, bytes.as_flattened_mut())
        .map_err(|_| Errno::EFAULT)?;
    Ok(bytes.map(u64::from_ne_bytes))
}

fn check_word<P: Platform>(process: &Process<P>, address: u64, shared: bool) -> Result<(), Errno> {
    if !address.is_multiple_of(WORD_SIZE) {
        return Err(Errno::EINVAL);
    }
    if address > <P::PageTableFormat as Format>::USER_END - WORD_SIZE {
        return Err(Errno::EFAULT);
    }
    if shared {
        read_word(process, address)?;
    }
    Ok(())
}

fn expect_word<P: Platform>(process: &Process<P>, address: u64, value: u64) -> Result<(), Errno> {
    match read_word(process, address)? == value as u32 {
        true => Ok(()),
        false => Err(Errno::EAGAIN),
    }
}

fn wake_limit(value: u64) -> u32 {
    (value as u32 as i32).max(1) as u32
}

fn read_word<P: Platform>(process: &Process<P>, address: u64) -> Result<u32, Errno> {
    process
        .with_user_word(address, false, |word| word.load(Ordering::Acquire))
        .map_err(|_| Errno::EFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_of_zero_or_fewer_waiters_wakes_one_as_on_linux() {
        assert_eq!(wake_limit(0), 1);
        assert_eq!(wake_limit(u64::from(u32::MAX)), 1);
        assert_eq!(wake_limit(0xffff_ffff_0000_0003), 3);
        assert_eq!(wake_limit(i32::MAX as u64), i32::MAX as u32);
        assert_eq!(wake_limit(1 << 31), 1);
    }
}

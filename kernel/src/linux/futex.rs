//! `futex`: threads wait until a 32-bit word in their memory changes, and are woken by
//! whoever changes it (D89). Waiters are kept per word in a fixed table of
//! buckets, each a list of waiters on their own stacks, so waiting and waking never allocate.

use super::Errno;
use super::time;
use crate::process::Process;
use crate::sync::SpinLock;
use crate::thread::{ParkEnd, Parker};
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use eggos_hal::Platform;
use eggos_hal::paging::Format;

/// The operations and flags of `futex`'s `op` argument.
pub mod op {
    pub const WAIT: u32 = 0;
    pub const WAKE: u32 = 1;
    pub const WAIT_BITSET: u32 = 9;
    pub const WAKE_BITSET: u32 = 10;
    pub const PRIVATE_FLAG: u32 = 128;
    pub const CLOCK_REALTIME: u32 = 256;
}

const MATCH_ANY: u32 = u32::MAX;
const WORD_SIZE: u64 = 4;
const BUCKET_COUNT: usize = 256;

/// The word a waiter waits on: the address space it is in, by its process's id while a
/// process has one thread and no memory shared with another, its user address, and whether
/// the futex is shared (no `FUTEX_PRIVATE_FLAG`): as on Linux, private and shared waits on one
/// word are apart, and a wake reaches only its own kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    space_and_kind: u64,
    address: u64,
}

impl Key {
    pub const fn new(space: u32, address: u64, shared: bool) -> Self {
        Self {
            space_and_kind: (space as u64) << 1 | shared as u64,
            address,
        }
    }

    fn bucket(self) -> &'static SpinLock<Bucket> {
        let mixed =
            (self.space_and_kind << 32 ^ self.address >> 2).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        &BUCKETS[(mixed >> 56) as usize % BUCKET_COUNT]
    }
}

static BUCKETS: [SpinLock<Bucket>; BUCKET_COUNT] =
    [const { SpinLock::new(Bucket { head: None }) }; BUCKET_COUNT];

struct Bucket {
    head: Option<NonNull<Waiter>>,
}

// SAFETY: the waiters a bucket links are reached only with the bucket's lock held, and each
// stays in place until its thread has taken that lock and unlinked it or found it unlinked.
unsafe impl Send for Bucket {}

struct Waiter {
    key: Key,
    bitset: u32,
    parker: Parker,
    link: UnsafeCell<Link>,
}

struct Link {
    next: Option<NonNull<Waiter>>,
    queued: bool,
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

    fn push_back(&mut self, waiter: NonNull<Waiter>) {
        *self.link(waiter) = Link {
            next: None,
            queued: true,
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
/// waiter, or the clock reaches `deadline` ([`Platform::clock_ticks`], `u64::MAX` for none).
/// `still_expected` runs first, with the word's bucket locked so no wake can slip in between:
/// its error, such as `EAGAIN` when the word no longer holds the expected value, ends the
/// call at once. `ETIMEDOUT` at the deadline. Call it as a blocking system call runs: with
/// interrupts masked and no lock held.
pub fn wait<P: Platform>(
    key: Key,
    bitset: u32,
    deadline: u64,
    still_expected: impl FnOnce() -> Result<(), Errno>,
) -> Result<u64, Errno> {
    let waiter = Waiter {
        key,
        bitset,
        parker: Parker::new(),
        link: UnsafeCell::new(Link {
            next: None,
            queued: false,
        }),
    };
    let pointer = NonNull::from(&waiter);
    let bucket_lock = key.bucket();
    {
        let mut bucket = bucket_lock.lock();
        still_expected()?;
        bucket.push_back(pointer);
    }
    let park_end = waiter.parker.park_until::<P>(deadline);
    let mut bucket = bucket_lock.lock();
    if !bucket.link(pointer).queued {
        return Ok(0);
    }
    bucket.remove(pointer);
    drop(bucket);
    debug_assert_eq!(park_end, ParkEnd::DeadlineReached);
    Err(Errno::ETIMEDOUT)
}

/// Wakes the waiters on `key` whose bitset shares a bit with `bitset`, in the order they
/// began to wait, at most `limit` of them; returns how many it woke.
pub fn wake<P: Platform>(key: Key, bitset: u32, limit: u32) -> u32 {
    let mut bucket = key.bucket().lock();
    let mut woken = 0;
    let mut previous = None;
    let mut following = bucket.head;
    while woken < limit
        && let Some(candidate) = following
    {
        following = bucket.link(candidate).next;
        // SAFETY: a waiter in the bucket, in place while the bucket's lock is held; its key,
        // bitset and parker are only read.
        let waiter = unsafe { candidate.as_ref() };
        if waiter.key == key && waiter.bitset & bitset != 0 {
            bucket.unlink(previous, candidate);
            waiter.parker.unpark::<P>();
            woken += 1;
        } else {
            previous = Some(candidate);
        }
    }
    woken
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`: `FUTEX_WAIT`, `FUTEX_WAKE`,
/// `FUTEX_WAIT_BITSET` and `FUTEX_WAKE_BITSET`, private or shared, with Linux's checks in
/// Linux's order: a wait's timeout first, then the operation and its flags (`ENOSYS`), the
/// bitset and the word's alignment (`EINVAL`), the word itself (`EFAULT`, then `EAGAIN`). A
/// waiting thread cannot be interrupted by a signal yet: only its own process could send one,
/// and its only thread is waiting. The other operations are `ENOSYS`.
pub fn futex<P: Platform>(
    process: &Process<P>,
    address: u64,
    operation: u64,
    value: u64,
    timeout: u64,
    bitset: u64,
) -> Result<u64, Errno> {
    let operation = operation as u32;
    let command = operation & !(op::PRIVATE_FLAG | op::CLOCK_REALTIME);
    let deadline = match (command, timeout) {
        (op::WAIT | op::WAIT_BITSET, 0) => u64::MAX,
        (op::WAIT, _) => time::deadline_after::<P>(process, timeout)?,
        (op::WAIT_BITSET, _) => time::deadline_at::<P>(process, timeout)?,
        _ => u64::MAX,
    };
    let bitset = match command {
        op::WAIT | op::WAKE => MATCH_ANY,
        op::WAIT_BITSET | op::WAKE_BITSET => bitset as u32,
        _ => return Err(Errno::ENOSYS),
    };
    if operation & op::CLOCK_REALTIME != 0 && command != op::WAIT_BITSET {
        return Err(Errno::ENOSYS);
    }
    if bitset == 0 || !address.is_multiple_of(WORD_SIZE) {
        return Err(Errno::EINVAL);
    }
    let shared = operation & op::PRIVATE_FLAG == 0;
    let key = Key::new(process.id().get(), address, shared);
    match command {
        op::WAKE | op::WAKE_BITSET => {
            if address > <P::PageTableFormat as Format>::USER_END - WORD_SIZE {
                return Err(Errno::EFAULT);
            }
            if shared {
                read_word(process, address)?;
            }
            Ok(u64::from(wake::<P>(key, bitset, wake_limit(value))))
        }
        _ => wait::<P>(key, bitset, deadline, || {
            match read_word(process, address)? == value as u32 {
                true => Ok(()),
                false => Err(Errno::EAGAIN),
            }
        }),
    }
}

fn wake_limit(value: u64) -> u32 {
    (value as u32 as i32).max(1) as u32
}

fn read_word<P: Platform>(process: &Process<P>, address: u64) -> Result<u32, Errno> {
    let mut word = [0; WORD_SIZE as usize];
    process
        .read_user_memory(address, &mut word)
        .map_err(|_| Errno::EFAULT)?;
    Ok(u32::from_ne_bytes(word))
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

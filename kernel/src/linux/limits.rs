//! Resource limits (M6, D59): `getrlimit`, `setrlimit` and `prlimit64`. glibc reads the
//! stack limit at startup (the default thread stack size), Rust's std reads it for the main
//! thread's guard, and shells and build tools read and lower the open-file limit.
//!
//! Each process has a soft and a hard limit per resource ([`ResourceLimits`]), starting from
//! [`DEFAULTS`]. A process may lower either and raise its soft limit up to its hard one; raising
//! a hard limit needs privilege (Linux's `CAP_SYS_RESOURCE`), which no process has until
//! processes have credentials ([`may_raise_hard_limits`]), so it is `EPERM`: lowering a hard
//! limit stays final, as a sandbox relies on. The limits are kept and reported; each is
//! enforced when its resource exists (the stack's 8 MiB region already matches its default).

use super::Errno;
use crate::process::Process;
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_hal::Platform;

/// A resource's limits: the soft one, which applies, and the hard one, the most the soft one
/// may be raised to. `struct rlimit64` (and `struct rlimit` on 64-bit architectures):
/// two 64-bit words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub soft: u64,
    pub hard: u64,
}

impl Limit {
    /// No limit (`RLIM_INFINITY`).
    pub const INFINITY: u64 = u64::MAX;

    const UNLIMITED: Self = Self::both(Self::INFINITY);

    const fn both(limit: u64) -> Self {
        Self {
            soft: limit,
            hard: limit,
        }
    }

    /// The structure as the program sees it.
    fn to_bytes(self) -> [u8; LIMIT_SIZE] {
        let mut bytes = [0; LIMIT_SIZE];
        bytes[..8].copy_from_slice(&self.soft.to_ne_bytes());
        bytes[8..].copy_from_slice(&self.hard.to_ne_bytes());
        bytes
    }

    fn from_bytes(bytes: [u8; LIMIT_SIZE]) -> Self {
        let (soft, hard) = bytes.split_at(8);
        Self {
            soft: u64::from_ne_bytes(soft.try_into().unwrap()),
            hard: u64::from_ne_bytes(hard.try_into().unwrap()),
        }
    }
}

/// The size of `struct rlimit64`.
const LIMIT_SIZE: usize = 16;

/// Linux's resource numbers (`RLIMIT_*`). These are the generic ones, which AArch64, RISC-V and
/// x86_64 use; MIPS, SPARC and Alpha number some differently.
pub mod resource {
    /// CPU time, in seconds.
    pub const CPU: u32 = 0;
    /// The largest file the process may create, in bytes.
    pub const FSIZE: u32 = 1;
    /// The data segment: the program break and private mappings, in bytes.
    pub const DATA: u32 = 2;
    /// The main thread's stack, in bytes.
    pub const STACK: u32 = 3;
    /// The largest core dump, in bytes.
    pub const CORE: u32 = 4;
    /// Resident memory (ignored by Linux since 2.6).
    pub const RSS: u32 = 5;
    /// Processes and threads of the process's user.
    pub const NPROC: u32 = 6;
    /// One more than the highest file descriptor the process may open.
    pub const NOFILE: u32 = 7;
    /// Locked memory, in bytes.
    pub const MEMLOCK: u32 = 8;
    /// The address space, in bytes.
    pub const AS: u32 = 9;
    /// File locks.
    pub const LOCKS: u32 = 10;
    /// Signals queued for the process's user.
    pub const SIGPENDING: u32 = 11;
    /// Bytes in the user's POSIX message queues.
    pub const MSGQUEUE: u32 = 12;
    /// The ceiling of the nice value, as `20 - limit`.
    pub const NICE: u32 = 13;
    /// The highest real-time priority.
    pub const RTPRIO: u32 = 14;
    /// CPU time a real-time thread may take without blocking, in microseconds.
    pub const RTTIME: u32 = 15;
    /// How many resources there are (`RLIM_NLIMITS`).
    pub const COUNT: usize = 16;

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    compile_error!("check Linux's RLIMIT_* numbers for this architecture");
}

/// The most file descriptors a process may be allowed (Linux's `fs.nr_open` default): a higher
/// `RLIMIT_NOFILE` is `EPERM`, whatever the process's privilege.
pub const MAX_OPEN_FILES: u64 = 1 << 20;

/// The limits a process starts with, one per resource: those a current Linux distribution gives
/// its processes (systemd's open-file limit, 1024 soft and 524288 hard, since no process can
/// raise a hard limit yet; 8 MiB of locked memory, as since Linux 5.16).
pub const DEFAULTS: [Limit; resource::COUNT] = {
    use resource::*;
    let mut limits = [Limit::UNLIMITED; resource::COUNT];
    limits[STACK as usize].soft = super::exec::STACK_RESERVE;
    limits[CORE as usize].soft = 0;
    limits[NPROC as usize] = Limit::both(DEFAULT_TASKS);
    limits[NOFILE as usize] = Limit {
        soft: 1024,
        hard: 524_288,
    };
    limits[MEMLOCK as usize] = Limit::both(8 << 20);
    limits[SIGPENDING as usize] = Limit::both(DEFAULT_TASKS);
    limits[MSGQUEUE as usize] = Limit::both(819_200);
    limits[NICE as usize] = Limit::both(0);
    limits[RTPRIO as usize] = Limit::both(0);
    limits
};

/// `RLIMIT_NPROC` and `RLIMIT_SIGPENDING`'s default, which Linux computes from the memory size.
/// A fixed value until they are enforced, so every machine reports the same.
const DEFAULT_TASKS: u64 = 65_536;

/// A process's limits: [`DEFAULTS`] until it first changes one, then a copy of its own on the
/// heap. Most processes never change theirs, so a process costs a word for its limits, and
/// starting one copies nothing (a copy in `Process::new` was +1.3% on riscv64's
/// `linux_exec_load`).
#[derive(Debug, Default)]
pub struct ResourceLimits(Option<Box<[Limit; resource::COUNT]>>);

/// Why a limit could not be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeError {
    /// No resource has that number.
    NoSuchResource,
    /// The soft limit is above the hard one.
    SoftAboveHard,
    /// The hard limit would rise, or the open-file limit pass [`MAX_OPEN_FILES`].
    NotPermitted,
    /// There was no memory for the process's own copy.
    OutOfMemory,
}

/// Whether the process may raise its hard limits: Linux's `CAP_SYS_RESOURCE`. No process may
/// until processes have credentials; then the privileged ones (`init`, M10) will.
fn may_raise_hard_limits() -> bool {
    false
}

impl ResourceLimits {
    /// The defaults.
    pub const fn new() -> Self {
        Self(None)
    }

    fn all(&self) -> &[Limit; resource::COUNT] {
        self.0.as_deref().unwrap_or(&DEFAULTS)
    }

    /// `resource`'s limits, or `None` for a number with no resource.
    pub fn get(&self, resource: u32) -> Option<Limit> {
        self.all().get(resource as usize).copied()
    }

    /// Sets `resource`, a resource's number, to `new` if allowed (see the module documentation)
    /// and returns the limits it had. The first change that changes anything makes the
    /// process's own copy.
    fn set(&mut self, resource: u32, new: Limit) -> Result<Limit, ChangeError> {
        let current = self.get(resource).ok_or(ChangeError::NoSuchResource)?;
        if new.soft > new.hard {
            return Err(ChangeError::SoftAboveHard);
        }
        if (resource == resource::NOFILE && new.hard > MAX_OPEN_FILES)
            || (new.hard > current.hard && !may_raise_hard_limits())
        {
            return Err(ChangeError::NotPermitted);
        }
        if new != current {
            let own = match &mut self.0 {
                Some(own) => own,
                None => self.0.insert(own_copy()?),
            };
            own[resource as usize] = new;
        }
        Ok(current)
    }
}

/// A copy of [`DEFAULTS`] on the heap, or [`ChangeError::OutOfMemory`].
fn own_copy() -> Result<Box<[Limit; resource::COUNT]>, ChangeError> {
    let mut limits = Vec::new();
    limits
        .try_reserve_exact(resource::COUNT)
        .map_err(|_| ChangeError::OutOfMemory)?;
    limits.extend_from_slice(&DEFAULTS);
    // Exactly `COUNT` limits, in a buffer of exactly that capacity: no reallocation.
    Ok(limits.into_boxed_slice().try_into().unwrap())
}

/// `getrlimit(resource, limit)`: as `prlimit64(0, resource, NULL, limit)`, but a null `limit`
/// is `EFAULT`, as on Linux.
pub fn getrlimit<P: Platform>(
    process: &Process<P>,
    resource: u64,
    limit: u64,
) -> Result<u64, Errno> {
    change(process, 0, resource, None, Some(limit))
}

/// `setrlimit(resource, limit)`: as `prlimit64(0, resource, limit, NULL)`, but a null `limit`
/// is `EFAULT`, as on Linux.
pub fn setrlimit<P: Platform>(
    process: &Process<P>,
    resource: u64,
    limit: u64,
) -> Result<u64, Errno> {
    change(process, 0, resource, Some(limit), None)
}

/// `prlimit64(pid, resource, new, old)`: sets `resource`'s limits to the `struct rlimit64` at
/// `new`, unless that is null, and copies the ones it had to `old`, unless that is null. See
/// [`change`].
pub fn prlimit64<P: Platform>(
    process: &Process<P>,
    pid: u64,
    resource: u64,
    new: u64,
    old: u64,
) -> Result<u64, Errno> {
    let pointer = |address| (address != 0).then_some(address);
    change(process, pid, resource, pointer(new), pointer(old))
}

/// What the three calls share: sets `resource`'s limits to the structure at `new`, if any, and
/// copies the ones it had to `old`, if any. As Linux, in this order: `EFAULT` if `new` is not
/// the program's memory; `ESRCH` for a `pid` other than 0 or the caller's (no other process is
/// reachable yet); `EINVAL` for a number with no resource or a soft limit above the hard one;
/// `EPERM` for an open-file limit above [`MAX_OPEN_FILES`] or a raised hard limit; then, with
/// the new limits set, `EFAULT` if `old` is not the program's writable memory. `pid` (a
/// `pid_t`) and `resource` are 32-bit: the registers' upper halves are ignored.
fn change<P: Platform>(
    process: &Process<P>,
    pid: u64,
    resource: u64,
    new: Option<u64>,
    old: Option<u64>,
) -> Result<u64, Errno> {
    let new = match new {
        None => None,
        Some(address) => {
            let mut bytes = [0; LIMIT_SIZE];
            process
                .read_user_memory(address, &mut bytes)
                .map_err(|_| Errno::EFAULT)?;
            Some(Limit::from_bytes(bytes))
        }
    };
    let pid = pid as u32;
    if pid != 0 && pid != process.id().get() {
        return Err(Errno::ESRCH);
    }
    let resource = resource as u32;
    // Copied out before the program's memory is touched: the lock is never held across a copy.
    let previous = {
        let mut limits = process.resource_limits().lock();
        match new {
            None => limits.get(resource).ok_or(Errno::EINVAL)?,
            Some(new) => limits.set(resource, new).map_err(|error| match error {
                ChangeError::NoSuchResource | ChangeError::SoftAboveHard => Errno::EINVAL,
                ChangeError::NotPermitted => Errno::EPERM,
                ChangeError::OutOfMemory => Errno::ENOMEM,
            })?,
        }
    };
    if let Some(address) = old {
        process
            .write_user_memory(address, &previous.to_bytes())
            .map_err(|_| Errno::EFAULT)?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use resource::*;

    #[test]
    fn defaults_are_a_linux_first_process_s() {
        let defaults = ResourceLimits::new();
        let limit = |resource| defaults.get(resource).unwrap();
        assert_eq!(
            limit(STACK),
            Limit {
                soft: 8 << 20,
                hard: Limit::INFINITY
            }
        );
        assert_eq!(
            limit(NOFILE),
            Limit {
                soft: 1024,
                hard: 524_288
            }
        );
        assert_eq!(limit(CORE).soft, 0);
        assert_eq!(limit(CPU), Limit::UNLIMITED);
        assert_eq!(limit(AS), Limit::UNLIMITED);
        assert_eq!(limit(RTTIME), Limit::UNLIMITED);
        assert_eq!(defaults.get(COUNT as u32), None);
        assert_eq!(defaults.get(u32::MAX), None);
        // Every default is a valid limit.
        assert!(DEFAULTS.iter().all(|limit| limit.soft <= limit.hard));
    }

    #[test]
    fn limits_lower_freely_and_rise_only_to_the_hard_limit() {
        let mut limits = ResourceLimits::new();
        let stack = |soft, hard| Limit { soft, hard };
        // No change, and a number with no resource: still the defaults, with no copy made.
        let default_stack = DEFAULTS[STACK as usize];
        assert_eq!(limits.set(STACK, default_stack), Ok(default_stack));
        assert_eq!(
            limits.set(COUNT as u32, default_stack),
            Err(ChangeError::NoSuchResource)
        );
        assert!(limits.0.is_none());
        // The soft limit up to the hard one, and back down.
        assert_eq!(
            limits.set(STACK, stack(Limit::INFINITY, Limit::INFINITY)),
            Ok(default_stack)
        );
        assert!(limits.0.is_some());
        assert_eq!(
            limits
                .set(STACK, stack(1 << 20, Limit::INFINITY))
                .map(|_| ()),
            Ok(())
        );
        // Lowering the hard limit is final.
        assert_eq!(
            limits.set(STACK, stack(1 << 20, 2 << 20)).map(|_| ()),
            Ok(())
        );
        assert_eq!(
            limits.set(STACK, stack(1 << 20, 4 << 20)),
            Err(ChangeError::NotPermitted)
        );
        assert_eq!(
            limits.set(STACK, stack(4 << 20, 2 << 20)),
            Err(ChangeError::SoftAboveHard)
        );
        assert_eq!(limits.get(STACK), Some(stack(1 << 20, 2 << 20)));
        // The open-file limit: up to the hard limit, which then cannot rise again.
        assert_eq!(
            limits.set(NOFILE, stack(4096, 4096)),
            Ok(stack(1024, 524_288))
        );
        assert_eq!(
            limits.set(NOFILE, stack(4097, 4097)),
            Err(ChangeError::NotPermitted)
        );
        // Never past `MAX_OPEN_FILES`, nor unlimited.
        let mut limits = ResourceLimits::new();
        limits.0 = Some(own_copy().unwrap());
        limits.0.as_mut().unwrap()[NOFILE as usize] = Limit::UNLIMITED;
        for hard in [MAX_OPEN_FILES + 1, Limit::INFINITY] {
            assert_eq!(
                limits.set(NOFILE, stack(1024, hard)),
                Err(ChangeError::NotPermitted)
            );
        }
        assert_eq!(
            limits.set(NOFILE, stack(1024, MAX_OPEN_FILES)).map(|_| ()),
            Ok(())
        );
    }

    #[test]
    fn the_structure_is_two_native_words() {
        let limit = Limit {
            soft: 0x0102_0304_0506_0708,
            hard: Limit::INFINITY,
        };
        let bytes = limit.to_bytes();
        assert_eq!(&bytes[..8], &0x0102_0304_0506_0708u64.to_ne_bytes());
        assert_eq!(&bytes[8..], &[0xff; 8]);
        assert_eq!(Limit::from_bytes(bytes), limit);
    }
}
